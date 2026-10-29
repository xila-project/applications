//! Quarantined, bounded-memory Xila package installation.
//!
//! A .xpk is only trusted after EOF and the TAR terminator have been checked,
//! every payload hash has matched, and (unless explicitly overridden) its
//! Ed25519 manifest signature has been verified with a trusted key.
//!
//! We never check signatures again at application launch: the installed files
//! are owned and protected by the Xila filesystem / permission model.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::sync::atomic::{AtomicUsize, Ordering};
use tar_no_std::{StreamEvent, TarStreamParser};
use xila::{
    file_system::{AccessFlags, CreateFlags, Flags, Kind, Path, Permissions},
    task::TaskIdentifier,
    virtual_file_system::{Directory, File, VirtualFileSystem},
};
use xila_package_manifest::{MANIFEST_PATH, Manifest, SIGNATURE_PATH, validate_path};
use xila_package_reader::StreamingVerifier;
use xila_package_verification::PublicKey;

const BLOCK_BYTES: usize = 512;
pub const INSTALLED_ROOT: &str = "/applications/packages";
pub const DEFAULT_TRUSTED_KEY: &str = "/configuration/system/packages/trusted.pub";
static NEXT_STAGING: AtomicUsize = AtomicUsize::new(1);

fn error(context: &str, detail: impl core::fmt::Display) -> String {
    format!("{context}: {detail}")
}

async fn ensure_directory(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    path: &str,
) -> Result<(), String> {
    match Directory::create(vfs, task, Path::from_str(path)).await {
        Ok(()) => Ok(()),
        Err(xila::virtual_file_system::Error::AlreadyExists) => {
            let stat = vfs
                .get_link_statistics_for_task(task, &Path::from_str(path))
                .await
                .map_err(|e| error("cannot inspect directory", e))?;
            if stat.kind == Kind::Directory {
                Ok(())
            } else {
                Err(format!("not a directory: {path}"))
            }
        }
        Err(e) => Err(error("cannot create directory", e)),
    }
}

/// Reject non-regular files and disallow traversing a TAR-controlled path.
/// The package reader additionally checks strict archive order and hashes.
async fn open_target(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    staging: &str,
    entry: &str,
) -> Result<File<'static>, String> {
    if !matches!(entry, MANIFEST_PATH | SIGNATURE_PATH) {
        validate_path(entry).map_err(|_| "unsafe package file path".to_string())?;
    }
    let mut parent = staging.to_string();
    let mut fragments = entry.split('/').peekable();
    while let Some(part) = fragments.next() {
        if fragments.peek().is_none() {
            break;
        }
        parent.push('/');
        parent.push_str(part);
        ensure_directory(vfs, task, &parent).await?;
    }
    let path = format!("{staging}/{entry}");
    File::open(
        vfs,
        task,
        Path::from_str(&path),
        Flags::new(
            AccessFlags::Write,
            Some(CreateFlags::CREATE_EXCLUSIVE),
            None,
        ),
    )
    .await
    .map_err(|e| error("cannot create package file", e))
}

async fn write_all(file: &mut File<'_>, bytes: &[u8]) -> Result<(), String> {
    let mut offset = 0;
    while offset < bytes.len() {
        let count = file
            .write(&bytes[offset..])
            .await
            .map_err(|e| error("package write failed", e))?;
        if count == 0 {
            return Err("package write stalled".into());
        }
        offset += count;
    }
    Ok(())
}

/// Read an entire 512-byte TAR block while handling short VFS reads.
/// Partial blocks are always rejected, never treated as a clean EOF.
async fn next_block(file: &mut File<'_>, output: &mut [u8; BLOCK_BYTES]) -> Result<bool, String> {
    let mut received = 0;
    while received < output.len() {
        let n = file
            .read(&mut output[received..])
            .await
            .map_err(|e| error("package read failed", e))?;
        if n == 0 {
            return if received == 0 {
                Ok(false)
            } else {
                Err("truncated TAR block".into())
            };
        }
        received += n;
    }
    Ok(true)
}

async fn close_output(
    vfs: &VirtualFileSystem,
    output: &mut Option<File<'static>>,
) -> Result<(), String> {
    if let Some(mut file) = output.take() {
        let flush = file
            .flush()
            .await
            .map_err(|e| error("file flush failed", e));
        let close = file
            .close(vfs)
            .await
            .map_err(|e| error("file close failed", e));
        flush.and(close)?;
    }
    Ok(())
}

async fn read_public_key(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    path: &str,
) -> Result<PublicKey, String> {
    let mut file = File::open(vfs, task, Path::from_str(path), AccessFlags::Read.into())
        .await
        .map_err(|e| error("cannot open trusted public key", e))?;
    let operation = async {
        let mut raw = [0u8; 33];
        let mut received = 0;
        while received < raw.len() {
            let count = file
                .read(&mut raw[received..])
                .await
                .map_err(|e| error("public key read failed", e))?;
            if count == 0 {
                break;
            }
            received += count;
        }
        if received != 32 {
            return Err("trusted public key must contain exactly 32 raw bytes".into());
        }
        PublicKey::from_slice(&raw[..32])
            .map_err(|_| "trusted Ed25519 public key is invalid".to_string())
    }
    .await;
    let close = file
        .close(vfs)
        .await
        .map_err(|e| error("public key close failed", e));
    match (operation, close) {
        (Err(e), _) => Err(e),
        (Ok(_), Err(e)) => Err(e),
        (Ok(key), Ok(())) => Ok(key),
    }
}

fn check_compatibility(manifest: &Manifest) -> Result<(), String> {
    if manifest.target != "wasm32-wasip1" {
        return Err("package is not a WASI Preview 1 application".into());
    }
    if manifest.minimum_xila_abi > abi_declarations::APPLICATION_ABI_VERSION {
        return Err(format!(
            "package requires Xila ABI {}, runtime supports {}",
            manifest.minimum_xila_abi,
            abi_declarations::APPLICATION_ABI_VERSION
        ));
    }
    // Until capability consent and sandbox policy are implemented, do not
    // silently grant declared capabilities to newly installed code.
    if !manifest.capabilities.is_empty() {
        return Err("package requests capabilities; installation policy is not implemented".into());
    }
    let app = manifest
        .application
        .as_ref()
        .ok_or_else(|| "package is not an application".to_string())?;
    if !app.entrypoint.starts_with("bin/")
        || !app.entrypoint.ends_with(".wasm")
        || !manifest
            .files
            .iter()
            .any(|file| file.path == app.entrypoint)
    {
        return Err("application entrypoint is not a bundled WASI binary".into());
    }
    Ok(())
}

async fn extract(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    archive: &str,
    staging: &str,
    trusted: Option<&PublicKey>,
) -> Result<Manifest, String> {
    let mut source = File::open(vfs, task, Path::from_str(archive), AccessFlags::Read.into())
        .await
        .map_err(|e| error("cannot open package archive", e))?;

    let operation = async {
        let mut verifier = StreamingVerifier::new(trusted);
        let mut parser = TarStreamParser::new();
        let mut active: Option<File<'static>> = None;
        let mut block = [0u8; BLOCK_BYTES];

        let result = async {
            while next_block(&mut source, &mut block).await? {
                // First authenticate this block (SHA-256 and signature policy).
                // A corrupt chunk is never accepted even if the TAR parser
                // itself considers its header structurally sound.
                verifier
                    .push(&block)
                    .map_err(|e| format!("package rejected: {e:?}"))?;

                let event = parser
                    .parse_block(&block)
                    .map_err(|e| format!("TAR parsing failed: {e:?}"))?;

                match event {
                    Some(StreamEvent::Entry(entry)) => {
                        close_output(vfs, &mut active).await?;
                        let name = entry.filename();
                        let path = name.as_str().map_err(|_| "invalid TAR filename")?;
                        let file = open_target(vfs, task, staging, path).await?;
                        active = Some(file);
                        if entry.payload_size() == 0 {
                            close_output(vfs, &mut active).await?;
                        }
                    }
                    Some(StreamEvent::Data { bytes, remaining }) => {
                        let output = active
                            .as_mut()
                            .ok_or_else(|| "TAR payload without an open file".to_string())?;
                        write_all(output, bytes).await?;
                        if remaining == 0 {
                            close_output(vfs, &mut active).await?;
                        }
                    }
                    Some(StreamEvent::End) | None => {}
                }
            }
            if active.is_some() {
                return Err("incomplete package file".into());
            }
            parser
                .finish()
                .map_err(|e| format!("invalid TAR end: {e:?}"))?;
            let manifest = verifier
                .finish()
                .map_err(|e| format!("package verification failed: {e:?}"))?;
            check_compatibility(&manifest)?;
            Ok(manifest)
        }
        .await;
        // Never abandon an open VFS handle on validation failure.
        let closed = close_output(vfs, &mut active).await;
        result.and(closed)
    }
    .await;
    let close = source
        .close(vfs)
        .await
        .map_err(|e| error("archive close failed", e));
    match (operation, close) {
        (Err(e), _) => Err(e),
        (Ok(_), Err(e)) => Err(e),
        (Ok(manifest), Ok(())) => Ok(manifest),
    }
}

/// Remove failed / interrupted staging trees without touching live installs.
/// Directory listing is completed and closed before any child is removed.
async fn remove_tree(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    root: &str,
) -> Result<(), String> {
    let mut pending: Vec<(String, bool)> = alloc::vec![(root.to_string(), false)];
    while let Some((path, remove)) = pending.pop() {
        if remove {
            vfs.remove(task, Path::from_str(&path))
                .await
                .map_err(|e| error("cannot clean staging entry", e))?;
            continue;
        }
        let mut directory = Directory::open(vfs, task, Path::from_str(&path))
            .await
            .map_err(|e| error("cannot open staging directory", e))?;
        let mut children = Vec::new();
        let scan = async {
            while let Some(entry) = directory
                .read()
                .await
                .map_err(|e| error("cannot read staging directory", e))?
            {
                if matches!(entry.name.as_str(), "." | "..") {
                    continue;
                }
                children.push((
                    format!("{path}/{}", entry.name),
                    entry.kind == Kind::Directory,
                ));
            }
            Ok::<_, String>(())
        }
        .await;
        let close = directory
            .close(vfs)
            .await
            .map_err(|e| error("cannot close staging directory", e));
        scan.and(close)?;
        pending.push((path, true));
        for (child, directory) in children {
            pending.push((child, !directory));
        }
    }
    Ok(())
}

async fn seal_tree(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    root: &str,
) -> Result<(), String> {
    let mut pending = alloc::vec![root.to_string()];
    let readonly_file =
        Permissions::from_octal(0o444).map_err(|_| "invalid file mode".to_string())?;
    let readonly_directory =
        Permissions::from_octal(0o555).map_err(|_| "invalid directory mode".to_string())?;
    while let Some(path) = pending.pop() {
        let stat = vfs
            .get_link_statistics_for_task(task, &Path::from_str(&path))
            .await
            .map_err(|e| error("cannot inspect staged path", e))?;
        if stat.kind == Kind::Directory {
            let mut directory = Directory::open(vfs, task, Path::from_str(&path))
                .await
                .map_err(|e| error("cannot open staged directory", e))?;
            let mut children = Vec::new();
            let scan = async {
                while let Some(entry) = directory
                    .read()
                    .await
                    .map_err(|e| error("cannot read staged directory", e))?
                {
                    if !matches!(entry.name.as_str(), "." | "..") {
                        children.push(format!("{path}/{}", entry.name));
                    }
                }
                Ok::<_, String>(())
            }
            .await;
            let close = directory
                .close(vfs)
                .await
                .map_err(|e| error("close failed", e));
            scan.and(close)?;
            pending.extend(children);
            vfs.set_permissions(task, Path::from_str(&path), readonly_directory)
                .await
                .map_err(|e| error("cannot seal package directory", e))?;
        } else if stat.kind == Kind::File {
            vfs.set_permissions(task, Path::from_str(&path), readonly_file)
                .await
                .map_err(|e| error("cannot seal package file", e))?;
        } else {
            return Err("special file found in staged package".into());
        }
    }
    Ok(())
}

async fn new_staging(vfs: &VirtualFileSystem, task: TaskIdentifier) -> Result<String, String> {
    for _ in 0..64 {
        let id = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
        let path = format!("{INSTALLED_ROOT}/.incoming-{id:x}");
        match Directory::create(vfs, task, Path::from_str(&path)).await {
            Ok(()) => {
                // No unverified executable bytes may be visible to another
                // user. The directory initially contains no files; restrict
                // it before opening the untrusted archive or writing payloads.
                let private = Permissions::from_octal(0o700)
                    .map_err(|_| "invalid quarantine permissions".to_string())?;
                if let Err(error) = vfs.set_permissions(task, Path::from_str(&path), private).await {
                    let _ = vfs.remove(task, Path::from_str(&path)).await;
                    return Err(error("cannot protect staging directory", error));
                }
                return Ok(path);
            }
            Err(xila::virtual_file_system::Error::AlreadyExists) => continue,
            Err(e) => return Err(error("cannot create package staging directory", e)),
        }
    }
    Err("could not allocate a staging directory".into())
}

/// Install once, never overwrite an existing installation. Reject unknown
/// publishers by default. --allow-untrusted bypasses ONLY publisher identity.
pub async fn install(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    archive: &str,
    key_path: Option<&str>,
    allow_untrusted: bool,
) -> Result<String, String> {
    if allow_untrusted && key_path.is_some() {
        return Err("cannot combine --public-key with --allow-untrusted".into());
    }
    // Xila VFS permissions on /applications are the authorization boundary.
    // No caller-provided capability may elevate the installing task.
    ensure_directory(vfs, task, INSTALLED_ROOT).await?;
    let key = if allow_untrusted {
        None
    } else {
        Some(read_public_key(vfs, task, key_path.unwrap_or(DEFAULT_TRUSTED_KEY)).await?)
    };

    let staging = new_staging(vfs, task).await?;
    let operation = async {
        let manifest = extract(vfs, task, archive, &staging, key.as_ref()).await?;
        let dest = format!("{INSTALLED_ROOT}/{}", manifest.name);
        // Never silently replace already-installed code. Root-authorized
        // concurrent installs require a higher-level exclusive transaction.
        match vfs
            .get_link_statistics_for_task(task, &Path::from_str(&dest))
            .await
        {
            Err(xila::virtual_file_system::Error::FileSystem(
                xila::file_system::Error::NotFound,
            )) => {}
            Ok(_) => return Err(format!("package {} is already installed", manifest.name)),
            Err(e) => return Err(error("cannot inspect installation destination", e)),
        }
        seal_tree(vfs, task, &staging).await?;
        vfs.rename(task, &Path::from_str(&staging), &Path::from_str(&dest))
            .await
            .map_err(|e| error("cannot publish verified package", e))?;
        Ok(manifest.name)
    }
    .await;

    match operation {
        Ok(name) => Ok(name),
        Err(problem) => {
            if let Err(cleanup) = remove_tree(vfs, task, &staging).await {
                Err(format!("{problem}; cleanup also failed: {cleanup}"))
            } else {
                Err(problem)
            }
        }
    }
}

pub async fn list(vfs: &VirtualFileSystem, task: TaskIdentifier) -> Result<Vec<String>, String> {
    let mut directory = Directory::open(vfs, task, Path::from_str(INSTALLED_ROOT))
        .await
        .map_err(|e| error("cannot open installed packages", e))?;
    let mut result = Vec::new();
    let operation = async {
        while let Some(entry) = directory
            .read()
            .await
            .map_err(|e| error("cannot list installed packages", e))?
        {
            if entry.kind == Kind::Directory
                && !entry.name.starts_with('.')
                && !matches!(entry.name.as_str(), "." | "..")
            {
                result.push(entry.name);
            }
        }
        result.sort();
        Ok::<_, String>(())
    }
    .await;
    let close = directory
        .close(vfs)
        .await
        .map_err(|e| error("close failed", e));
    operation.and(close)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_capabilities_and_external_executable() {
        use xila_package_manifest::{Application, FORMAT_VERSION, ManifestFile};
        let mut manifest = Manifest {
            schema: None,
            format_version: FORMAT_VERSION,
            name: "test.clock".into(),
            version: "1".into(),
            target: "wasm32-wasip1".into(),
            minimum_xila_abi: 1,
            capabilities: alloc::vec![],
            files: alloc::vec![ManifestFile {
                path: "bin/clock.wasm".into(),
                size: 4,
                sha256: "0".repeat(64),
            }],
            application: Some(Application {
                entrypoint: "bin/clock.wasm".into(),
                display_name: "Clock".into(),
                icon: String::new(),
                show_in_launcher: true,
            }),
        };
        assert!(check_compatibility(&manifest).is_ok());
        manifest.capabilities.push("network".into());
        assert!(check_compatibility(&manifest).is_err());
        manifest.capabilities.clear();
        manifest.application.as_mut().unwrap().entrypoint = "/system/executables/wasm".into();
        assert!(check_compatibility(&manifest).is_err());
    }
}
