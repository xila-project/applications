//! Load-time integrity of installed package executables.
//!
//! A path inside [`BUNDLES_DIRECTORY`] names a **managed** executable: the
//! installer verified its package (manifest signature, unless explicitly
//! overridden at installation, and every file hash) and sealed the bundle,
//! so the installed `.xila/manifest.json` is the authoritative record of the
//! expected size and SHA-256 of each file. Before compiling a managed
//! executable, the loader reads that manifest, then hashes the executable
//! while reading it into the buffer that is compiled. Anything that does not
//! match is refused; no option disables this check for managed paths.
//!
//! Any other path names an **unmanaged** executable, such as a development
//! build. It has no authenticated expected digest, so it runs only in
//! explicit developer mode and without verification.
//!
//! This is load-time validation: bytes are verified once, when they are read
//! for compilation. It does not re-verify resources on each access, and it is
//! not a runtime integrity guarantee like fs-verity.

use alloc::{vec, vec::Vec};
use core::fmt;
use manifest::{EXECUTABLES_DIRECTORY, MANIFEST_PATH, MAX_MANIFEST_BYTES};
use verification::ContentVerifier;
use xila::{
    file_system::{AccessFlags, Kind, Path, PathOwned, Permission, Statistics},
    task::TaskIdentifier,
    users::{GroupIdentifier, UserIdentifier},
    virtual_file_system::{File, IoErrorKind, VirtualFileSystem},
};

/// Installed package bundles, one directory per package identifier.
pub const BUNDLES_DIRECTORY: &str = "/applications/bundles";

/// How the loader treats a path, decided before anything is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin<'a> {
    /// `executables/<name>` inside the bundle of package `identifier`.
    Package {
        identifier: &'a str,
        executable: &'a str,
    },
    /// Outside installed bundles: no authenticated expected digest exists.
    Unmanaged,
}

/// Why a WebAssembly executable is refused before it is compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntegrityError {
    /// The path is relative or contains empty, `.` or `..` components.
    NotNormalized,
    /// The executable is not part of an installed package and developer mode is off.
    Unmanaged,
    /// The bundle directory is missing or is an interrupted installation.
    NotInstalled,
    /// Only files directly inside a bundle's `executables` directory may run.
    NotAPackageExecutable,
    /// The bundle has no installed manifest.
    MissingManifest,
    /// The installed manifest cannot be read or decoded.
    InvalidManifest,
    /// The installed manifest describes another package.
    PackageMismatch,
    /// The installed manifest does not declare this executable.
    NotDeclared,
    /// A bundle file or directory is a link, or another kind than expected.
    UnexpectedKind,
    /// A bundle file or directory may be written by a non-root user.
    Unprotected,
    /// The file was replaced between its inspection and its opening.
    Replaced,
    /// The executable's size differs from the installed manifest.
    SizeMismatch,
    /// The executable's SHA-256 differs from the installed manifest.
    DigestMismatch,
    /// Reading the bundle failed.
    FileSystem(IoErrorKind),
}

impl fmt::Display for IntegrityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotNormalized => {
                formatter.write_str("the path must be absolute, without empty, . or .. components")
            }
            Self::Unmanaged => formatter.write_str(
                "it is not part of an installed package; pass --developer-mode to run an unverified development executable",
            ),
            Self::NotInstalled => formatter.write_str("its package is not installed"),
            Self::NotAPackageExecutable => {
                formatter.write_str("only files in a package's executables directory may run")
            }
            Self::MissingManifest => formatter.write_str("its package has no installed manifest"),
            Self::InvalidManifest => formatter.write_str("its installed package manifest is invalid"),
            Self::PackageMismatch => {
                formatter.write_str("its installed manifest describes another package")
            }
            Self::NotDeclared => formatter.write_str("its installed package manifest does not declare it"),
            Self::UnexpectedKind => {
                formatter.write_str("its package contains a link or an unexpected file kind")
            }
            Self::Unprotected => formatter.write_str("its package files are writable by non-root users"),
            Self::Replaced => formatter.write_str("it was replaced while being opened"),
            Self::SizeMismatch => {
                formatter.write_str("its size does not match its installed package manifest")
            }
            Self::DigestMismatch => {
                formatter.write_str("its SHA-256 does not match its installed package manifest")
            }
            Self::FileSystem(error) => write!(formatter, "its package cannot be read ({error})"),
        }
    }
}

impl core::error::Error for IntegrityError {}

pub type Result<T> = core::result::Result<T, IntegrityError>;

fn is_normalized(path: &str) -> bool {
    path.strip_prefix('/').is_some_and(|relative| {
        relative
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
    })
}

/// Classify `path` lexically. Paths that are not normalized are refused, so
/// `.`, `..` or repeated separators cannot move a managed path out of the
/// managed category. A link elsewhere that points into a bundle is
/// unmanaged and only runs in developer mode, with the caller's permissions.
pub fn classify(path: &str) -> Result<Origin<'_>> {
    if !is_normalized(path) {
        return Err(IntegrityError::NotNormalized);
    }
    let Some(rest) = path
        .strip_prefix(BUNDLES_DIRECTORY)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return Ok(Origin::Unmanaged);
    };
    let (identifier, executable) = rest
        .split_once('/')
        .ok_or(IntegrityError::NotAPackageExecutable)?;
    // Staging (`.incoming-*`) and download directories are never installed.
    if identifier.starts_with('.') {
        return Err(IntegrityError::NotInstalled);
    }
    if !manifest::is_bundle_executable(executable) {
        return Err(IntegrityError::NotAPackageExecutable);
    }
    Ok(Origin::Package {
        identifier,
        executable,
    })
}

/// Only root may change a protected entry: it is owned by root, others cannot
/// write it, and its group can write it only if that group is root.
fn is_protected(statistics: &Statistics) -> bool {
    let permissions = statistics.permissions;
    statistics.user == UserIdentifier::ROOT
        && !permissions.get_others().contains(Permission::Write)
        && (statistics.group == GroupIdentifier::ROOT
            || !permissions.get_group().contains(Permission::Write))
}

fn check_entry(statistics: &Statistics, kind: Kind) -> Result<()> {
    if statistics.kind != kind {
        return Err(IntegrityError::UnexpectedKind);
    }
    if !is_protected(statistics) {
        return Err(IntegrityError::Unprotected);
    }
    Ok(())
}

fn join(base: &str, relative: &str) -> Result<PathOwned> {
    Path::new(base)
        .join(Path::new(relative))
        .ok_or(IntegrityError::NotNormalized)
}

async fn link_statistics(
    virtual_file_system: &VirtualFileSystem,
    task: TaskIdentifier,
    path: &Path,
    missing: IntegrityError,
) -> Result<Statistics> {
    match virtual_file_system
        .get_link_statistics_for_task(task, &path)
        .await
    {
        Ok(statistics) => Ok(statistics),
        Err(IoErrorKind::NotFound) => Err(missing),
        Err(error) => Err(IntegrityError::FileSystem(error)),
    }
}

/// Open a protected regular file and return its size. The entry is inspected
/// without following links, then the opened handle is inspected again: a
/// file swapped in between (which needs root, given the protected parents)
/// has another inode and is refused.
async fn open_protected<'a>(
    virtual_file_system: &'a VirtualFileSystem,
    task: TaskIdentifier,
    path: &Path,
    missing: IntegrityError,
) -> Result<(File<'a>, u64)> {
    let inspected = link_statistics(virtual_file_system, task, path, missing.clone()).await?;
    check_entry(&inspected, Kind::File)?;
    let mut file = match File::open(virtual_file_system, task, path, AccessFlags::Read.into()).await
    {
        Ok(file) => file,
        Err(IoErrorKind::NotFound) => return Err(missing),
        Err(error) => return Err(IntegrityError::FileSystem(error)),
    };
    let opened = match file.get_statistics().await {
        Ok(statistics) => statistics,
        Err(error) => {
            let _ = file.close(virtual_file_system).await;
            return Err(IntegrityError::FileSystem(error));
        }
    };
    let checked = check_entry(&opened, Kind::File).and(if opened.inode == inspected.inode {
        Ok(opened.size)
    } else {
        Err(IntegrityError::Replaced)
    });
    match checked {
        Ok(size) => Ok((file, size)),
        Err(error) => {
            let _ = file.close(virtual_file_system).await;
            Err(error)
        }
    }
}

/// Fill `buffer` completely, then confirm the file has no further byte.
/// `consume` sees each newly read range exactly once, in order.
async fn read_exact_and_end(
    file: &mut File<'_>,
    buffer: &mut [u8],
    mut consume: impl FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let mut received = 0;
    while received < buffer.len() {
        let count = file
            .read(&mut buffer[received..])
            .await
            .map_err(IntegrityError::FileSystem)?;
        if count == 0 {
            return Err(IntegrityError::SizeMismatch);
        }
        consume(&buffer[received..received + count])?;
        received += count;
    }
    let mut probe = [0_u8; 1];
    match file.read(&mut probe).await {
        Ok(0) => Ok(()),
        Ok(_) => Err(IntegrityError::SizeMismatch),
        Err(error) => Err(IntegrityError::FileSystem(error)),
    }
}

async fn close_after<T>(
    virtual_file_system: &VirtualFileSystem,
    file: File<'_>,
    result: Result<T>,
) -> Result<T> {
    let closed = file.close(virtual_file_system).await;
    let value = result?;
    closed.map_err(IntegrityError::FileSystem)?;
    Ok(value)
}

/// Expected size and digest of `executable`, from the installed manifest.
async fn expected_content(
    virtual_file_system: &VirtualFileSystem,
    task: TaskIdentifier,
    bundle: &Path,
    identifier: &str,
    executable: &str,
) -> Result<(u64, [u8; 32])> {
    let path = join(bundle.as_str(), MANIFEST_PATH)?;
    let (mut file, size) = open_protected(
        virtual_file_system,
        task,
        &path,
        IntegrityError::MissingManifest,
    )
    .await?;
    let result = async {
        let size = usize::try_from(size)
            .ok()
            .filter(|&size| size <= MAX_MANIFEST_BYTES)
            .ok_or(IntegrityError::InvalidManifest)?;
        let mut bytes = vec![0_u8; size];
        read_exact_and_end(&mut file, &mut bytes, |_| Ok(()))
            .await
            .map_err(|error| match error {
                IntegrityError::SizeMismatch => IntegrityError::InvalidManifest,
                error => error,
            })?;
        let manifest = manifest::decode(&bytes).map_err(|_| IntegrityError::InvalidManifest)?;
        if manifest.name != identifier {
            return Err(IntegrityError::PackageMismatch);
        }
        let declared = manifest
            .file(executable)
            .ok_or(IntegrityError::NotDeclared)?;
        let digest = declared
            .digest()
            .map_err(|_| IntegrityError::InvalidManifest)?;
        Ok((declared.size, digest))
    }
    .await;
    close_after(virtual_file_system, file, result).await
}

/// Read a managed executable into memory, refusing it unless its exact
/// bytes match the size and SHA-256 recorded by its installed manifest.
///
/// The returned buffer is the one that was hashed, so the verified bytes
/// are exactly the bytes compiled; a concurrent modification of the file
/// makes the digest differ. Working memory beyond that buffer is the
/// transient manifest (at most `MAX_MANIFEST_BYTES`, released before the
/// executable is read) and the fixed-size SHA-256 state.
pub async fn read_package_executable(
    virtual_file_system: &VirtualFileSystem,
    task: TaskIdentifier,
    identifier: &str,
    executable: &str,
) -> Result<Vec<u8>> {
    let bundle = join(BUNDLES_DIRECTORY, identifier)?;
    // The bundle and the directories holding the manifest and the
    // executable must be real, root-protected directories, so the files
    // below cannot be renamed, replaced or redirected by a link.
    let bundles = link_statistics(
        virtual_file_system,
        task,
        Path::new(BUNDLES_DIRECTORY),
        IntegrityError::NotInstalled,
    )
    .await?;
    check_entry(&bundles, Kind::Directory)?;
    let statistics = link_statistics(
        virtual_file_system,
        task,
        &bundle,
        IntegrityError::NotInstalled,
    )
    .await?;
    check_entry(&statistics, Kind::Directory)?;
    let metadata = MANIFEST_PATH
        .rsplit_once('/')
        .map_or(MANIFEST_PATH, |(directory, _)| directory);
    for (directory, missing) in [
        (metadata, IntegrityError::MissingManifest),
        (EXECUTABLES_DIRECTORY, IntegrityError::NotDeclared),
    ] {
        let directory = join(bundle.as_str(), directory)?;
        let statistics = link_statistics(virtual_file_system, task, &directory, missing).await?;
        check_entry(&statistics, Kind::Directory)?;
    }

    let (expected_size, expected_digest) =
        expected_content(virtual_file_system, task, &bundle, identifier, executable).await?;

    let path = join(bundle.as_str(), executable)?;
    let (mut file, size) = open_protected(
        virtual_file_system,
        task,
        &path,
        IntegrityError::NotDeclared,
    )
    .await?;
    let result = async {
        if size != expected_size {
            return Err(IntegrityError::SizeMismatch);
        }
        let length = usize::try_from(expected_size).map_err(|_| IntegrityError::SizeMismatch)?;
        let mut verifier = ContentVerifier::new(expected_size, expected_digest)
            .map_err(|_| IntegrityError::SizeMismatch)?;
        let mut buffer = vec![0_u8; length];
        read_exact_and_end(&mut file, &mut buffer, |chunk| {
            verifier
                .update(chunk)
                .map_err(|_| IntegrityError::SizeMismatch)
        })
        .await?;
        verifier.verify().map_err(|error| match error {
            verification::Error::DigestMismatch => IntegrityError::DigestMismatch,
            _ => IntegrityError::SizeMismatch,
        })?;
        Ok(buffer)
    }
    .await;
    close_after(virtual_file_system, file, result).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_installed_package_executables() {
        assert_eq!(
            classify("/applications/bundles/xila.clock/executables/clock.wasm"),
            Ok(Origin::Package {
                identifier: "xila.clock",
                executable: "executables/clock.wasm",
            })
        );
        for unmanaged in [
            "/temporary/clock.wasm",
            "/applications/executables/clock.wasm",
            "/applications/bundlesx/xila.clock/executables/clock.wasm",
            "/applications/bundles",
        ] {
            assert_eq!(classify(unmanaged), Ok(Origin::Unmanaged), "{unmanaged}");
        }
    }

    #[test]
    fn aliases_cannot_leave_the_managed_category() {
        for alias in [
            "clock.wasm",
            "applications/bundles/xila.clock/executables/clock.wasm",
            "//applications/bundles/xila.clock/executables/clock.wasm",
            "/applications//bundles/xila.clock/executables/clock.wasm",
            "/applications/./bundles/xila.clock/executables/clock.wasm",
            "/applications/x/../bundles/xila.clock/executables/clock.wasm",
            "/applications/bundles/xila.clock/executables/clock.wasm/",
        ] {
            assert_eq!(
                classify(alias),
                Err(IntegrityError::NotNormalized),
                "{alias}"
            );
        }
    }

    #[test]
    fn refuses_staging_and_files_outside_the_executables_directory() {
        assert_eq!(
            classify("/applications/bundles/.incoming-1/executables/clock.wasm"),
            Err(IntegrityError::NotInstalled)
        );
        for path in [
            "/applications/bundles/xila.clock",
            "/applications/bundles/xila.clock/.xila/manifest.json",
            "/applications/bundles/xila.clock/clock.wasm",
            "/applications/bundles/xila.clock/executables/nested/clock.wasm",
        ] {
            assert_eq!(
                classify(path),
                Err(IntegrityError::NotAPackageExecutable),
                "{path}"
            );
        }
    }
}
