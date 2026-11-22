//! Quarantined, bounded-memory Xila package installation.
//!
//! A .xpk is only trusted after EOF and the TAR terminator have been checked,
//! every payload hash has matched, and (unless explicitly overridden) its
//! Ed25519 manifest signature has been verified with a key the device trusts
//! through a repository: root key → repository key → registry entry and
//! developer key → package signature → file hashes.
//!
//! The verified bundle is then activated by package_manager's
//! crash-consistent installation transactions (see `transaction`).
//!
//! We never check signatures again at application launch: the installed files
//! are owned and protected by the Xila filesystem / permission model.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use portable_atomic::{AtomicUsize, Ordering};
use tar_no_std::{BLOCKSIZE, StreamEvent, TarStreamParser};
use xila::{
    file_system::{AccessFlags, CreateFlags, Flags, Kind, Path, Permissions},
    task::TaskIdentifier,
    virtual_file_system::{Directory, File, VirtualFileSystem},
};
use xila_package_manager::installation::{self, Activated, Mode};
use xila_package_manifest::{
    DOWNLOAD_PREFIX, MANIFEST_PATH, MAX_MANIFEST_BYTES, Manifest, ORIGIN_SCHEMA_URL, Origin,
    REPOSITORY_FORMAT_VERSION, RegistryEntry, SIGNATURE_PATH, STAGING_PREFIX, XilaVersion,
    validate_path,
};
use xila_package_reader::StreamingVerifier;
use xila_package_verification::{PublicKey, Sha256Verifier};

use crate::transaction::{LAYOUT, Session};

/// Same as `Path::APPLICATION_BUNDLES`; a `&str` because `Path::as_str` is not `const`.
pub const INSTALLED_ROOT: &str = "/applications/bundles";
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
        Err(xila::virtual_file_system::IoErrorKind::AlreadyExists) => {
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
async fn next_block(file: &mut File<'_>, output: &mut [u8; BLOCKSIZE]) -> Result<bool, String> {
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

pub(crate) const fn running_xila_version() -> XilaVersion {
    XilaVersion::new(xila_version::RUNNING.major, xila_version::RUNNING.minor)
}

fn check_compatibility(manifest: &Manifest) -> Result<(), String> {
    if manifest.target != "wasm32-wasip1" {
        return Err("package is not a WASI Preview 1 application".into());
    }
    let running = running_xila_version();
    if !xila_package_manifest::compatible(manifest.xila_version, running) {
        return Err(format!(
            "package requires Xila {}, running Xila {}",
            manifest.xila_version, running
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
    if !xila_package_manifest::is_bundle_executable(&app.entrypoint)
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
        let mut block = [0u8; BLOCKSIZE];

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
                    Some(StreamEvent::Entry { filename, size, .. }) => {
                        close_output(vfs, &mut active).await?;
                        let path = filename.as_str().map_err(|_| "invalid TAR filename")?;
                        let file = open_target(vfs, task, staging, path).await?;
                        active = Some(file);
                        if size == 0 {
                            close_output(vfs, &mut active).await?;
                        }
                    }
                    Some(StreamEvent::Data { data, remaining }) => {
                        let output = active
                            .as_mut()
                            .ok_or_else(|| "TAR payload without an open file".to_string())?;
                        write_all(output, data).await?;
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
        match (result, closed) {
            (Err(e), _) => Err(e),
            (Ok(_), Err(e)) => Err(e),
            (Ok(manifest), Ok(())) => Ok(manifest),
        }
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
pub(crate) async fn remove_tree(
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
        Permissions::from_octal(0o444).ok_or_else(|| "invalid file mode".to_string())?;
    let readonly_directory =
        Permissions::from_octal(0o555).ok_or_else(|| "invalid directory mode".to_string())?;
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

/// A new private staging directory: its name in the bundles directory and its path.
async fn new_staging(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
) -> Result<(String, String), String> {
    for _ in 0..64 {
        let id = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
        let name = format!("{STAGING_PREFIX}{id:x}");
        let path = format!("{INSTALLED_ROOT}/{name}");
        match Directory::create(vfs, task, Path::from_str(&path)).await {
            Ok(()) => {
                // No unverified executable bytes may be visible to another
                // user. The directory initially contains no files; restrict
                // it before opening the untrusted archive or writing payloads.
                let private = Permissions::from_octal(0o700)
                    .ok_or_else(|| "invalid quarantine permissions".to_string())?;
                if let Err(problem) = vfs
                    .set_permissions(task, Path::from_str(&path), private)
                    .await
                {
                    let _ = vfs.remove(task, Path::from_str(&path)).await;
                    return Err(error("cannot protect staging directory", problem));
                }
                return Ok((name, path));
            }
            Err(xila::virtual_file_system::IoErrorKind::AlreadyExists) => continue,
            Err(e) => return Err(error("cannot create package staging directory", e)),
        }
    }
    Err("could not allocate a staging directory".into())
}

/// A fresh path for a downloaded archive, hidden from `list` like staging.
pub fn new_download_path() -> String {
    let id = NEXT_STAGING.fetch_add(1, Ordering::Relaxed);
    format!("{INSTALLED_ROOT}/{DOWNLOAD_PREFIX}{id:x}.xpk")
}

/// Lowercase hexadecimal SHA-256 of a file.
pub async fn archive_sha256(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    archive: &str,
) -> Result<String, String> {
    let mut file = File::open(vfs, task, Path::from_str(archive), AccessFlags::Read.into())
        .await
        .map_err(|e| error("cannot open package archive", e))?;
    let operation = async {
        let mut hasher = Sha256Verifier::new();
        let mut buffer = [0u8; BLOCKSIZE];
        loop {
            let count = file
                .read(&mut buffer)
                .await
                .map_err(|e| error("package read failed", e))?;
            if count == 0 {
                break;
            }
            hasher
                .update(&buffer[..count])
                .map_err(|_| "archive too large".to_string())?;
        }
        let digest = hasher
            .finalize()
            .map_err(|_| "hashing failed".to_string())?;
        Ok::<_, String>(digest.iter().map(|b| format!("{b:02x}")).collect())
    }
    .await;
    let close = file
        .close(vfs)
        .await
        .map_err(|e| error("archive close failed", e));
    match (operation, close) {
        (Err(e), _) => Err(e),
        (Ok(_), Err(e)) => Err(e),
        (Ok(digest), Ok(())) => Ok(digest),
    }
}

/// A package installed or upgraded by [`install_verified`].
pub struct Installed {
    pub name: String,
    pub version: String,
    pub activated: Activated,
}

/// Verify `archive` into a staging directory, then activate it.
/// With `expected`, every field the registry entry copied from the signed
/// manifest must match before anything is published. `repository` and
/// `archive_sha256` are recorded as the package's origin.
#[allow(clippy::too_many_arguments)]
pub async fn install_verified(
    session: &mut Session,
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    archive: &str,
    key: Option<&PublicKey>,
    expected: Option<&RegistryEntry>,
    repository: Option<&str>,
    archive_sha256: &str,
    mode: Mode,
) -> Result<Installed, String> {
    let (staging_name, staging) = new_staging(vfs, task).await?;
    let verified = async {
        let manifest = extract(vfs, task, archive, &staging, key).await?;
        if let Some(entry) = expected {
            entry
                .check_manifest(&manifest)
                .map_err(|_| "package differs from its registry entry".to_string())?;
        }
        seal_tree(vfs, task, &staging).await?;
        Ok::<_, String>(manifest)
    }
    .await;
    let manifest = match verified {
        Ok(manifest) => manifest,
        Err(problem) => {
            return match remove_tree(vfs, task, &staging).await {
                Ok(()) => Err(problem),
                Err(cleanup) => Err(format!("{problem}; cleanup also failed: {cleanup}")),
            };
        }
    };
    let origin = Origin {
        schema: Some(ORIGIN_SCHEMA_URL.into()),
        format_version: REPOSITORY_FORMAT_VERSION,
        repository: repository.map(Into::into),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        archive_sha256: archive_sha256.into(),
    };
    match installation::activate(&mut session.storage, &LAYOUT, &staging_name, origin, mode).await {
        Ok(activated) => Ok(Installed {
            name: manifest.name,
            version: manifest.version,
            activated,
        }),
        Err(problem) => {
            // Finish or undo a journaled step, and delete the staging directory.
            let recovered = installation::recover(&mut session.storage, &LAYOUT).await;
            let problem = format!("cannot install {}: {problem}", manifest.name);
            match recovered {
                Ok(_) => Err(problem),
                Err(cleanup) => Err(format!("{problem}; recovery also failed: {cleanup}")),
            }
        }
    }
}

/// Read the manifest at the start of a local archive, without trusting it:
/// it only names the registry entry that will authenticate the archive.
pub async fn read_manifest(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    archive: &str,
) -> Result<Manifest, String> {
    let mut source = File::open(vfs, task, Path::from_str(archive), AccessFlags::Read.into())
        .await
        .map_err(|e| error("cannot open package archive", e))?;
    let operation = async {
        let mut parser = TarStreamParser::new();
        let mut block = [0u8; BLOCKSIZE];
        let mut manifest: Option<Vec<u8>> = None;
        while next_block(&mut source, &mut block).await? {
            match parser
                .parse_block(&block)
                .map_err(|e| format!("TAR parsing failed: {e:?}"))?
            {
                Some(StreamEvent::Entry { filename, size, .. }) => {
                    let path = filename.as_str().map_err(|_| "invalid TAR filename")?;
                    if path != MANIFEST_PATH || manifest.is_some() {
                        break;
                    }
                    if size > MAX_MANIFEST_BYTES {
                        return Err("package manifest is too large".to_string());
                    }
                    manifest = Some(Vec::with_capacity(size));
                }
                Some(StreamEvent::Data { data, remaining }) => {
                    let bytes = manifest
                        .as_mut()
                        .ok_or_else(|| "package does not start with its manifest".to_string())?;
                    bytes.extend_from_slice(data);
                    if remaining == 0 {
                        return xila_package_manifest::decode(bytes)
                            .map_err(|e| format!("invalid package manifest: {e:?}"));
                    }
                }
                Some(StreamEvent::End) | None => {}
            }
        }
        Err("package does not start with its manifest".to_string())
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

/// Install or upgrade a local archive verified with an explicitly chosen
/// developer key (`key_path`), or, with `None`, without authenticating its
/// publisher (hashes are still verified). Archives trusted through a
/// repository go through `repository::install_archive` instead.
pub async fn install_local(
    session: &mut Session,
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    archive: &str,
    key_path: Option<&str>,
    mode: Mode,
) -> Result<Installed, String> {
    let key = match key_path {
        Some(path) => Some(read_public_key(vfs, task, path).await?),
        None => None,
    };
    let digest = archive_sha256(vfs, task, archive).await?;
    install_verified(
        session,
        vfs,
        task,
        archive,
        key.as_ref(),
        None,
        None,
        &digest,
        mode,
    )
    .await
}

/// The manifest of the installed package `name`, or `None` when it is not
/// installed. It was verified at installation, and the sealed bundle keeps
/// it unchanged, so it is read back without verifying it again.
pub async fn installed_manifest(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    name: &str,
) -> Result<Option<Manifest>, String> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return Err(format!("invalid package name {name}"));
    }
    let path = format!("{}/{MANIFEST_PATH}", LAYOUT.bundle(name));
    let size = match vfs
        .get_link_statistics_for_task(task, &Path::from_str(&path))
        .await
    {
        Ok(statistics) => statistics.size,
        Err(xila::virtual_file_system::IoErrorKind::NotFound) => return Ok(None),
        Err(e) => return Err(error(&format!("cannot inspect {path}"), e)),
    };
    if size > MAX_MANIFEST_BYTES as u64 {
        return Err(format!("{path} is too large"));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    File::read_from_path(vfs, task, Path::from_str(&path), &mut bytes)
        .await
        .map_err(|e| error(&format!("cannot read {path}"), e))?;
    xila_package_manifest::decode(&bytes)
        .map(Some)
        .map_err(|e| format!("invalid manifest of {name}: {e:?}"))
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
    use xila_package_manifest::{Application, FORMAT_VERSION, ManifestFile};

    fn test_manifest() -> Manifest {
        Manifest {
            schema: None,
            format_version: FORMAT_VERSION,
            name: "test.clock".into(),
            version: "1".into(),
            target: "wasm32-wasip1".into(),
            xila_version: running_xila_version(),
            capabilities: alloc::vec![],
            files: alloc::vec![ManifestFile {
                path: "executables/clock.wasm".into(),
                size: 4,
                sha256: "0".repeat(64),
            }],
            application: Some(Application {
                entrypoint: "executables/clock.wasm".into(),
                display_name: "Clock".into(),
                icon: String::new(),
                show_in_launcher: true,
            }),
            metadata: None,
        }
    }

    #[test]
    fn paths_follow_core_hierarchy() {
        assert_eq!(INSTALLED_ROOT, Path::APPLICATION_BUNDLES.as_str());
        let journal_directory = crate::transaction::JOURNAL.rsplit_once('/').unwrap().0;
        assert_eq!(journal_directory, Path::PACKAGE_CONFIGURATION.as_str());
        let download = new_download_path();
        assert!(xila_package_manifest::is_temporary_entry(
            download.rsplit_once('/').unwrap().1
        ));
    }

    #[test]
    fn rejects_unsupported_capabilities_and_external_executable() {
        let mut manifest = test_manifest();
        assert!(check_compatibility(&manifest).is_ok());
        manifest.capabilities.push("network".into());
        assert!(check_compatibility(&manifest).is_err());
        manifest.capabilities.clear();
        manifest.application.as_mut().unwrap().entrypoint = "/system/executables/wasm".into();
        assert!(check_compatibility(&manifest).is_err());
    }

    #[test]
    fn rejects_incompatible_xila_version() {
        let running = running_xila_version();
        let older_pre_release = (running.major == 0)
            .then(|| running.minor.checked_sub(1))
            .flatten()
            .map(|minor| XilaVersion::new(0, minor));
        for incompatible in [
            Some(XilaVersion::new(
                running.major,
                running.minor.wrapping_add(1),
            )),
            Some(XilaVersion::new(
                running.major.wrapping_add(1),
                running.minor,
            )),
            older_pre_release,
        ]
        .into_iter()
        .flatten()
        {
            let mut manifest = test_manifest();
            manifest.xila_version = incompatible;
            let problem = check_compatibility(&manifest).unwrap_err();
            assert!(problem.contains(&incompatible.to_string()), "{problem}");
            assert!(problem.contains(&running.to_string()), "{problem}");
        }
    }
}
