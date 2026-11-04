//! Configured package repositories and installation from them.
//!
//! Each repository is pinned to a root key chosen by the user (passed with
//! `--root-key`, or accepted on first use after comparing its fingerprint).
//! Everything else is verified from that key with `xila_package_manager::trust`:
//! root → repository key → index, entry and developer key → archive.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use xila::{
    file_system::{AccessFlags, CreateFlags, Flags, Kind, Path, Permissions},
    task::TaskIdentifier,
    virtual_file_system::{Directory, File, VirtualFileSystem},
};
use xila_package_manager::trust::{self, TrustedRoot};
use xila_package_manifest::{
    INDEX_PATH, Index, MAX_MANIFEST_BYTES, ORIGIN_SCHEMA_URL, Origin, REPOSITORY_FORMAT_VERSION,
    ROOT_PATH, RegistryEntry, Repository, SIGNATURE_SUFFIX, decode_origin, decode_repository,
    encode_origin, encode_repository,
};
use xila_package_verification::{ContentVerifier, PublicKey};

use crate::{
    http::{self, Sink},
    install,
};

pub const REPOSITORIES: &str = "/configuration/system/packages/repositories";
pub const ORIGINS: &str = "/configuration/system/packages/installed";
/// Largest index accepted on a device (the format allows more).
const MAX_INDEX_BYTES: usize = 256 * 1024;
const SIGNATURE_BYTES: usize = 64;
const TARGET: &str = "wasm32-wasip1";

fn error(context: &str, detail: impl core::fmt::Display) -> String {
    format!("{context}: {detail}")
}

async fn ensure_directory(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    path: &str,
) -> Result<(), String> {
    let mut current = String::new();
    for part in path.split('/').filter(|part| !part.is_empty()) {
        current.push('/');
        current.push_str(part);
        match Directory::create(vfs, task, Path::from_str(&current)).await {
            Ok(()) | Err(xila::virtual_file_system::Error::AlreadyExists) => {}
            Err(e) => return Err(error(&format!("cannot create {current}"), e)),
        }
    }
    Ok(())
}

async fn read_file(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    path: &str,
) -> Result<Option<Vec<u8>>, String> {
    let mut buffer = Vec::new();
    match File::read_from_path(vfs, task, Path::from_str(path), &mut buffer).await {
        Ok(()) => Ok(Some(buffer)),
        Err(xila::virtual_file_system::Error::FileSystem(xila::file_system::Error::NotFound)) => {
            Ok(None)
        }
        Err(e) => Err(error(&format!("cannot read {path}"), e)),
    }
}

/// Replace `path` atomically: write a sibling, then rename over it.
async fn write_file(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    let temporary = format!("{path}.new");
    let _ = vfs.remove(task, Path::from_str(&temporary)).await;
    File::write_to_path(vfs, task, Path::from_str(&temporary), bytes)
        .await
        .map_err(|e| error(&format!("cannot write {temporary}"), e))?;
    let _ = vfs.remove(task, Path::from_str(path)).await;
    vfs.rename(task, &Path::from_str(&temporary), &Path::from_str(path))
        .await
        .map_err(|e| error(&format!("cannot replace {path}"), e))
}

fn configuration_path(name: &str) -> String {
    format!("{REPOSITORIES}/{name}.json")
}

fn origin_path(package: &str) -> String {
    format!("{ORIGINS}/{package}.json")
}

/// Every configured repository, sorted by name.
pub async fn list(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
) -> Result<Vec<Repository>, String> {
    let mut directory = match Directory::open(vfs, task, Path::from_str(REPOSITORIES)).await {
        Ok(directory) => directory,
        Err(xila::virtual_file_system::Error::FileSystem(xila::file_system::Error::NotFound)) => {
            return Ok(Vec::new());
        }
        Err(e) => return Err(error("cannot open repositories", e)),
    };
    let mut names = Vec::new();
    let scan = async {
        while let Some(entry) = directory
            .read()
            .await
            .map_err(|e| error("cannot list repositories", e))?
        {
            if entry.kind == Kind::File
                && let Some(name) = entry.name.strip_suffix(".json")
            {
                names.push(name.to_string());
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
    names.sort();
    let mut repositories = Vec::with_capacity(names.len());
    for name in names {
        repositories.push(load(vfs, task, &name).await?);
    }
    Ok(repositories)
}

async fn load(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    name: &str,
) -> Result<Repository, String> {
    let bytes = read_file(vfs, task, &configuration_path(name))
        .await?
        .ok_or_else(|| format!("unknown repository {name}"))?;
    let repository = decode_repository(&bytes)
        .map_err(|e| error(&format!("invalid repository {name}"), format!("{e:?}")))?;
    if repository.name != name {
        return Err(format!(
            "repository file {name}.json names {}",
            repository.name
        ));
    }
    Ok(repository)
}

async fn save(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    repository: &Repository,
) -> Result<(), String> {
    ensure_directory(vfs, task, REPOSITORIES).await?;
    let text =
        encode_repository(repository).map_err(|e| error("invalid repository", format!("{e:?}")))?;
    write_file(
        vfs,
        task,
        &configuration_path(&repository.name),
        text.as_bytes(),
    )
    .await
}

pub async fn remove(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    name: &str,
) -> Result<(), String> {
    load(vfs, task, name).await?;
    vfs.remove(task, Path::from_str(&configuration_path(name)))
        .await
        .map_err(|e| error(&format!("cannot remove repository {name}"), e))
}

async fn fetch_signed(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    repository: &Repository,
    path: &str,
    limit: usize,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let bytes = http::get_bytes(vfs, task, &repository.file_url(path), limit).await?;
    let signature = http::get_bytes(
        vfs,
        task,
        &repository.file_url(&format!("{path}{SIGNATURE_SUFFIX}")),
        SIGNATURE_BYTES,
    )
    .await?;
    Ok((bytes, signature))
}

fn rejected(repository: &str, path: &str, problem: trust::Error) -> String {
    format!("{repository}: {path} rejected: {problem}")
}

/// A repository whose root key the user still has to confirm.
pub struct Pending {
    repository: Repository,
    pub fingerprint: String,
    /// Whether the key was supplied by the user (no confirmation needed).
    pub pinned: bool,
}

impl Pending {
    pub fn name(&self) -> &str {
        &self.repository.name
    }

    pub fn root_key(&self) -> &str {
        &self.repository.root_key
    }
}

fn base64(bytes: &[u8; 32]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(44);
    for chunk in bytes.chunks(3) {
        let mut block = [0u8; 3];
        block[..chunk.len()].copy_from_slice(chunk);
        let value = u32::from_be_bytes([0, block[0], block[1], block[2]]);
        for index in 0..4 {
            output.push(if index <= chunk.len() {
                TABLE[(value >> (18 - 6 * index) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    output
}

/// Fetch a new repository's root and check it, without saving anything.
pub async fn prepare(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    name: &str,
    url: &str,
    root_key: Option<&str>,
) -> Result<Pending, String> {
    if read_file(vfs, task, &configuration_path(name))
        .await?
        .is_some()
    {
        return Err(format!("repository {name} already exists"));
    }
    // Validate the name and URL before any network access; the key is set below.
    let placeholder = root_key.unwrap_or("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
    let mut repository =
        Repository::new(name, url.trim_end_matches('/'), placeholder).map_err(|_| {
            "invalid repository: name must be lowercase, URL must be HTTPS, key canonical base64"
                .to_string()
        })?;
    let (bytes, signature) =
        fetch_signed(vfs, task, &repository, ROOT_PATH, MAX_MANIFEST_BYTES).await?;
    let (key, pinned) = match root_key {
        Some(_) => {
            let key = repository
                .root_key_bytes()
                .map_err(|_| "invalid --root-key".to_string())?;
            TrustedRoot::bootstrap(&bytes, &signature, &[key], 0)
                .map_err(|e| rejected(name, ROOT_PATH, e))?;
            (key, true)
        }
        None => {
            let (_, key) = TrustedRoot::first_use(&bytes, &signature)
                .map_err(|e| rejected(name, ROOT_PATH, e))?;
            (key, false)
        }
    };
    repository.root_key = base64(&key);
    Ok(Pending {
        fingerprint: trust::fingerprint(&key),
        pinned,
        repository,
    })
}

/// Save a confirmed repository after a first successful synchronization.
pub async fn add(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    pending: Pending,
) -> Result<Repository, String> {
    let (repository, _, _) = synchronize(vfs, task, pending.repository).await?;
    save(vfs, task, &repository).await?;
    Ok(repository)
}

/// Authenticate the current root and index, refusing anything older than
/// what this device already accepted.
async fn synchronize(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    mut repository: Repository,
) -> Result<(Repository, TrustedRoot, Index), String> {
    let name = repository.name.clone();
    let key = repository
        .root_key_bytes()
        .map_err(|_| format!("{name}: invalid root key"))?;
    let (root, root_signature) =
        fetch_signed(vfs, task, &repository, ROOT_PATH, MAX_MANIFEST_BYTES).await?;
    let trusted = TrustedRoot::bootstrap(&root, &root_signature, &[key], repository.root_version)
        .map_err(|e| rejected(&name, ROOT_PATH, e))?;
    let (index, index_signature) =
        fetch_signed(vfs, task, &repository, INDEX_PATH, MAX_INDEX_BYTES).await?;
    // Without a trustworthy clock, expiry is not checked; the version still is.
    let index = trusted
        .verify_index(&index, &index_signature, repository.index_version, None)
        .map_err(|e| rejected(&name, INDEX_PATH, e))?;
    repository.root_version = trusted.version();
    repository.index_version = index.version;
    Ok((repository, trusted, index))
}

struct Match {
    repository: Repository,
    trusted: TrustedRoot,
    index: Index,
    entry: RegistryEntry,
}

async fn find(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    repository: Repository,
    name: &str,
    version: Option<&str>,
) -> Result<Option<Match>, String> {
    let (repository, trusted, index) = synchronize(vfs, task, repository).await?;
    save(vfs, task, &repository).await?;
    let paths = match trust::candidates(&index, name, version) {
        Ok(paths) => paths,
        Err(trust::Error::NotListed) => return Ok(None),
        Err(e) => return Err(rejected(&repository.name, name, e)),
    };
    for path in paths {
        let (bytes, signature) =
            fetch_signed(vfs, task, &repository, &path, MAX_MANIFEST_BYTES).await?;
        trust::check_listed(&index, &path, &bytes)
            .map_err(|e| rejected(&repository.name, &path, e))?;
        let entry = trusted
            .verify_entry(&bytes, &signature)
            .map_err(|e| rejected(&repository.name, &path, e))?;
        if trust::compatible(&entry, TARGET, abi_declarations::APPLICATION_ABI_VERSION) {
            return Ok(Some(Match {
                repository,
                trusted,
                index,
                entry,
            }));
        }
    }
    Ok(None)
}

async fn publisher(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    found: &Match,
) -> Result<PublicKey, String> {
    let path = trust::developer_key_path(&found.entry.publisher);
    let name = &found.repository.name;
    let (bytes, signature) =
        fetch_signed(vfs, task, &found.repository, &path, MAX_MANIFEST_BYTES).await?;
    trust::check_listed(&found.index, &path, &bytes).map_err(|e| rejected(name, &path, e))?;
    let key = found
        .trusted
        .verify_developer_key(&bytes, &signature)
        .map_err(|e| rejected(name, &path, e))?;
    trust::publisher_key(&found.entry, &key).map_err(|e| rejected(name, &path, e))
}

fn digest(hex: &str) -> Option<[u8; 32]> {
    let value = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let bytes = hex.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut output = [0u8; 32];
    for (out, pair) in output.iter_mut().zip(bytes.chunks(2)) {
        *out = (value(pair[0])? << 4) | value(pair[1])?;
    }
    Some(output)
}

/// Streams the archive to a file while checking its exact size and SHA-256.
struct Download {
    file: File<'static>,
    verifier: Option<ContentVerifier>,
}

impl Sink for Download {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.verifier
            .as_mut()
            .ok_or("download already finished")?
            .update(bytes)
            .map_err(|_| "archive is larger than its registry entry".to_string())?;
        let mut offset = 0;
        while offset < bytes.len() {
            let count = self
                .file
                .write(&bytes[offset..])
                .await
                .map_err(|e| error("cannot write download", e))?;
            if count == 0 {
                return Err("download write stalled".into());
            }
            offset += count;
        }
        Ok(())
    }
}

async fn download(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    entry: &RegistryEntry,
    path: &str,
) -> Result<(), String> {
    let expected = digest(&entry.archive.sha256).ok_or("invalid archive digest")?;
    let verifier = ContentVerifier::new(entry.archive.size, expected)
        .map_err(|_| "invalid archive size".to_string())?;
    let file = File::open(
        vfs,
        task,
        Path::from_str(path),
        Flags::new(
            AccessFlags::Write,
            Some(CreateFlags::CREATE_EXCLUSIVE),
            None,
        ),
    )
    .await
    .map_err(|e| error("cannot create download file", e))?;
    let private = Permissions::from_octal(0o600).ok_or("invalid file mode")?;
    let mut sink = Download {
        file,
        verifier: Some(verifier),
    };
    let result = async {
        vfs.set_permissions(task, Path::from_str(path), private)
            .await
            .map_err(|e| error("cannot protect download", e))?;
        http::get(vfs, task, &entry.archive.url, &mut sink).await?;
        sink.verifier
            .take()
            .ok_or("download already finished")?
            .verify()
            .map_err(|_| "archive size or SHA-256 differs from its registry entry".to_string())
    }
    .await;
    let close = sink
        .file
        .close(vfs)
        .await
        .map_err(|e| error("cannot close download", e));
    result.and(close)
}

pub async fn origin(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    package: &str,
) -> Result<Option<Origin>, String> {
    match read_file(vfs, task, &origin_path(package)).await? {
        Some(bytes) => decode_origin(&bytes)
            .map(Some)
            .map_err(|e| error(&format!("invalid origin of {package}"), format!("{e:?}"))),
        None => Ok(None),
    }
}

pub async fn record_origin(
    vfs: &VirtualFileSystem,
    task: TaskIdentifier,
    repository: Option<&str>,
    name: &str,
    version: &str,
    archive_sha256: &str,
) -> Result<(), String> {
    let origin = Origin {
        schema: Some(ORIGIN_SCHEMA_URL.into()),
        format_version: REPOSITORY_FORMAT_VERSION,
        repository: repository.map(Into::into),
        name: name.into(),
        version: version.into(),
        archive_sha256: archive_sha256.into(),
    };
    let text = encode_origin(&origin).map_err(|e| error("invalid origin", format!("{e:?}")))?;
    ensure_directory(vfs, task, ORIGINS).await?;
    write_file(vfs, task, &origin_path(name), text.as_bytes()).await
}

/// What was installed, and from where.
pub struct Installed {
    pub name: String,
    pub version: String,
    pub repository: String,
}

/// Resolve `name` (optionally at `version`) in the configured repositories,
/// verify everything from their root keys, download and install it.
pub async fn install(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
    name: &str,
    version: Option<&str>,
    only: Option<&str>,
) -> Result<Installed, String> {
    if let Some(existing) = origin(vfs, task, name).await? {
        return Err(format!(
            "{name} {} is already installed (from {}); updates are not supported yet",
            existing.version,
            existing.repository.as_deref().unwrap_or("a local archive")
        ));
    }
    let repositories = match only {
        Some(only) => alloc::vec![load(vfs, task, only).await?],
        None => list(vfs, task).await?,
    };
    if repositories.is_empty() {
        return Err("no repository configured; add one with `package repository add`".into());
    }
    let mut matches = Vec::new();
    for repository in repositories {
        if let Some(found) = find(vfs, task, repository, name, version).await? {
            matches.push(found);
        }
    }
    let found = match matches.len() {
        0 => {
            return Err(format!(
                "{name} not found for {TARGET} in the configured repositories"
            ));
        }
        1 => matches.pop().expect("one match"),
        _ => {
            let names: Vec<&str> = matches.iter().map(|m| m.repository.name.as_str()).collect();
            return Err(format!(
                "{name} is available from several repositories ({}); choose one with --repository",
                names.join(", ")
            ));
        }
    };
    let key = publisher(vfs, task, &found).await?;

    install::ensure_root(vfs, task).await?;
    let path = install::new_download_path();
    let result = async {
        download(vfs, task, &found.entry, &path).await?;
        install::install_verified(vfs, task, &path, Some(&key), Some(&found.entry)).await
    }
    .await;
    let _ = vfs.remove(task, Path::from_str(&path)).await;
    let (installed, _) = result?;
    record_origin(
        vfs,
        task,
        Some(&found.repository.name),
        &installed,
        &found.entry.version,
        &found.entry.archive.sha256,
    )
    .await?;
    Ok(Installed {
        name: installed,
        version: found.entry.version,
        repository: found.repository.name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_lowercase_hex_digests_only() {
        let hex = "6976f8d118066b7294ac051160505b2c410fa9213cffa6a7c889946cf6fdbfcd";
        assert_eq!(digest(hex).unwrap()[..2], [0x69, 0x76]);
        assert!(digest(&hex.to_uppercase()).is_none());
        assert!(digest("00").is_none());
    }

    #[test]
    fn encodes_keys_like_the_registry() {
        let key = [0u8; 32];
        assert_eq!(base64(&key), "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
        let repository =
            Repository::new("xila", "https://registry.xila.dev", &base64(&[7u8; 32])).unwrap();
        assert_eq!(repository.root_key_bytes().unwrap(), [7u8; 32]);
    }

    #[test]
    fn paths_live_under_package_configuration() {
        assert!(REPOSITORIES.starts_with(Path::PACKAGE_CONFIGURATION.as_str()));
        assert!(ORIGINS.starts_with(Path::PACKAGE_CONFIGURATION.as_str()));
        assert_eq!(
            configuration_path("xila"),
            "/configuration/system/packages/repositories/xila.json"
        );
        assert!(install::INSTALLED_ROOT.starts_with("/applications"));
    }
}
