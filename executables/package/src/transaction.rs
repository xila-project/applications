//! The Xila virtual file system as storage for package_manager's
//! crash-consistent installation transactions.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use portable_atomic::{AtomicBool, Ordering};
use xila::{
    file_system::{Kind, Path},
    task::TaskIdentifier,
    virtual_file_system::{Directory, File, IoErrorKind, VirtualFileSystem},
};
use xila_package_manager::installation::{self, Layout, Recovery, Storage};

use crate::{install, repository};

/// Journal of the package operation in flight.
pub const JOURNAL: &str = "/configuration/system/packages/transaction.json";

pub const LAYOUT: Layout<'static> = Layout {
    bundles: install::INSTALLED_ROOT,
    origins: repository::ORIGINS,
    journal: JOURNAL,
};

/// One package operation at a time on this device.
static BUSY: AtomicBool = AtomicBool::new(false);

pub struct VirtualStorage {
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
}

impl VirtualStorage {
    pub fn new(vfs: &'static VirtualFileSystem, task: TaskIdentifier) -> Self {
        Self { vfs, task }
    }
}

fn error(context: &str, path: &str, detail: impl core::fmt::Display) -> String {
    format!("{context} {path}: {detail}")
}

impl Storage for VirtualStorage {
    type Error = String;

    async fn exists(&mut self, path: &str) -> Result<bool, String> {
        match self
            .vfs
            .get_link_statistics_for_task(self.task, &Path::from_str(path))
            .await
        {
            Ok(_) => Ok(true),
            Err(IoErrorKind::NotFound) => Ok(false),
            Err(e) => Err(error("cannot inspect", path, e)),
        }
    }

    async fn rename(&mut self, from: &str, to: &str) -> Result<(), String> {
        self.vfs
            .rename(self.task, &Path::from_str(from), &Path::from_str(to))
            .await
            .map_err(|e| error("cannot rename", from, e))
    }

    async fn remove(&mut self, path: &str) -> Result<(), String> {
        match self
            .vfs
            .get_link_statistics_for_task(self.task, &Path::from_str(path))
            .await
        {
            Err(IoErrorKind::NotFound) => Ok(()),
            Err(e) => Err(error("cannot inspect", path, e)),
            Ok(statistics) if statistics.kind == Kind::Directory => {
                install::remove_tree(self.vfs, self.task, path).await
            }
            Ok(_) => self
                .vfs
                .remove(self.task, Path::from_str(path))
                .await
                .map_err(|e| error("cannot remove", path, e)),
        }
    }

    async fn read(&mut self, path: &str, limit: usize) -> Result<Option<Vec<u8>>, String> {
        let mut buffer = Vec::new();
        match File::read_from_path(self.vfs, self.task, Path::from_str(path), &mut buffer).await {
            Ok(()) if buffer.len() <= limit => Ok(Some(buffer)),
            Ok(()) => Err(format!("{path} is larger than {limit} bytes")),
            Err(IoErrorKind::NotFound) => Ok(None),
            Err(e) => Err(error("cannot read", path, e)),
        }
    }

    async fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        // Writes everything, then closes the file, which flushes it.
        File::write_to_path(self.vfs, self.task, Path::from_str(path), bytes)
            .await
            .map_err(|e| error("cannot write", path, e))
    }

    async fn list(&mut self, path: &str) -> Result<Vec<String>, String> {
        let mut directory = Directory::open(self.vfs, self.task, Path::from_str(path))
            .await
            .map_err(|e| error("cannot open", path, e))?;
        let mut names = Vec::new();
        let scan = async {
            while let Some(entry) = directory
                .read()
                .await
                .map_err(|e| error("cannot list", path, e))?
            {
                if !matches!(entry.name.as_str(), "." | "..") {
                    names.push(entry.name);
                }
            }
            Ok::<_, String>(())
        }
        .await;
        let close = directory
            .close(self.vfs)
            .await
            .map_err(|e| error("cannot close", path, e));
        scan.and(close)?;
        Ok(names)
    }
}

/// Exclusive right to change the installed packages, after recovery of any
/// interrupted operation. Released on drop.
pub struct Session {
    pub storage: VirtualStorage,
    pub recovery: Recovery,
}

impl Drop for Session {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}

/// Take the package lock, create the package directories and recover.
/// The task must already be privileged.
pub async fn begin(
    vfs: &'static VirtualFileSystem,
    task: TaskIdentifier,
) -> Result<Session, String> {
    if BUSY.swap(true, Ordering::Acquire) {
        return Err("another package operation is running".to_string());
    }
    let mut session = Session {
        storage: VirtualStorage::new(vfs, task),
        recovery: Recovery::default(),
    };
    repository::ensure_directory(vfs, task, install::INSTALLED_ROOT).await?;
    repository::ensure_directory(vfs, task, repository::ORIGINS).await?;
    session.recovery = installation::recover(&mut session.storage, &LAYOUT)
        .await
        .map_err(|e| format!("cannot recover the interrupted package operation: {e}"))?;
    Ok(session)
}

/// A line describing what recovery did, if anything.
pub fn describe(recovery: &Recovery) -> Option<String> {
    let (transaction, outcome) = recovery.transaction.as_ref()?;
    let operation = match transaction.operation {
        xila_package_manifest::Operation::Install => "installation",
        xila_package_manifest::Operation::Upgrade => "upgrade",
        xila_package_manifest::Operation::Remove => "removal",
    };
    let result = match outcome {
        installation::Outcome::Completed => "completed",
        installation::Outcome::RolledBack => "rolled back",
    };
    Some(format!(
        "Recovered the interrupted {operation} of {}: {result}.",
        transaction.name
    ))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::privilege;
    use xila::{file_system::Permissions, users::UserIdentifier};
    use xila_package_manager::installation::{Activated, Mode, Outcome};
    use xila_package_manifest::{
        ORIGIN_SCHEMA_URL, Operation, Origin, REPOSITORY_FORMAT_VERSION,
        TRANSACTION_FORMAT_VERSION, TRANSACTION_SCHEMA_URL, Transaction, encode_transaction,
    };

    const NAME: &str = "xila.clock";
    const DATA: &str = "/configuration/applications/xila.clock.json";

    fn origin(version: &str) -> Origin {
        Origin {
            schema: Some(ORIGIN_SCHEMA_URL.into()),
            format_version: REPOSITORY_FORMAT_VERSION,
            repository: Some("xila".into()),
            name: NAME.into(),
            version: version.into(),
            archive_sha256: "a".repeat(64),
        }
    }

    /// A sealed bundle of `version`, as the installer leaves it after verification.
    async fn bundle(
        vfs: &'static VirtualFileSystem,
        task: TaskIdentifier,
        name: &str,
        version: &str,
    ) {
        let path = format!("{}/{name}", install::INSTALLED_ROOT);
        Directory::create(vfs, task, Path::from_str(&path))
            .await
            .unwrap();
        let executables = format!("{path}/executables");
        Directory::create(vfs, task, Path::from_str(&executables))
            .await
            .unwrap();
        File::write_to_path(
            vfs,
            task,
            Path::from_str(&format!("{executables}/clock-{version}.wasm")),
            b"\0asm",
        )
        .await
        .unwrap();
        File::write_to_path(
            vfs,
            task,
            Path::from_str(&format!("{path}/version")),
            version.as_bytes(),
        )
        .await
        .unwrap();
        for (target, mode) in [(&executables, 0o555), (&path, 0o555)] {
            vfs.set_permissions(
                task,
                Path::from_str(target),
                Permissions::from_octal(mode).unwrap(),
            )
            .await
            .unwrap();
        }
    }

    async fn installed_version(storage: &mut VirtualStorage) -> Option<String> {
        let bytes = storage
            .read(&format!("{}/{NAME}/version", install::INSTALLED_ROOT), 64)
            .await
            .unwrap()?;
        Some(String::from_utf8(bytes).unwrap())
    }

    #[xila::task::test(task_path = xila::task)]
    async fn package_operations_on_the_virtual_file_system() {
        let standard = testing::initialize(false, false).await;
        let context = standard.context;
        let vfs = context.virtual_file_system;
        let tasks = context.task_manager;
        let task = tasks.get_current_task_identifier().await;

        // Administrators become root for the operation only; others are refused.
        let administrator = context
            .users_manager
            .get_user_identifier("administrator")
            .await
            .unwrap();
        tasks.set_user(task, administrator).await.unwrap();
        let elevation = privilege::elevate(&standard).await.unwrap();
        assert_eq!(tasks.get_user(task).await.unwrap(), UserIdentifier::ROOT);
        elevation.restore(&standard).await;
        assert_eq!(tasks.get_user(task).await.unwrap(), administrator);
        let guest = UserIdentifier::new(42);
        let guest_group = xila::users::GroupIdentifier::new(42);
        context
            .users_manager
            .add_group(guest_group, "guest", &[])
            .await
            .unwrap();
        context
            .users_manager
            .add_user(guest, "guest", guest_group, "/users/guest")
            .await
            .unwrap();
        tasks.set_user(task, guest).await.unwrap();
        assert!(privilege::elevate(&standard).await.is_err());
        assert_eq!(tasks.get_user(task).await.unwrap(), guest);
        tasks.set_user(task, UserIdentifier::ROOT).await.unwrap();

        File::write_to_path(vfs, task, Path::from_str(DATA), b"{\"alarm\":7}")
            .await
            .unwrap();

        // Install, with abandoned temporary entries cleaned on the way in.
        let mut session = begin(vfs, task).await.unwrap();
        assert!(begin(vfs, task).await.is_err(), "operations are serialized");
        bundle(vfs, task, ".incoming-a", "0.1.0").await;
        assert_eq!(
            installation::activate(
                &mut session.storage,
                &LAYOUT,
                ".incoming-a",
                origin("0.1.0"),
                Mode::Install
            )
            .await,
            Ok(Activated::Installed)
        );
        assert_eq!(
            installed_version(&mut session.storage).await.as_deref(),
            Some("0.1.0")
        );
        bundle(vfs, task, ".incoming-abandoned", "0.9.0").await;
        drop(session);
        let mut session = begin(vfs, task).await.unwrap();
        assert_eq!(session.recovery.removed, [".incoming-abandoned"]);

        // An upgrade interrupted right after moving the old bundle aside.
        bundle(vfs, task, ".incoming-b", "0.2.0").await;
        let journal = Transaction {
            schema: Some(TRANSACTION_SCHEMA_URL.into()),
            format_version: TRANSACTION_FORMAT_VERSION,
            operation: Operation::Upgrade,
            name: NAME.into(),
            staging: Some(".incoming-b".into()),
            previous: Some(format!(".previous-{NAME}")),
            origin: Some(origin("0.2.0")),
            previous_origin: Some(origin("0.1.0")),
        };
        session
            .storage
            .write(JOURNAL, encode_transaction(&journal).unwrap().as_bytes())
            .await
            .unwrap();
        session
            .storage
            .rename(
                &LAYOUT.bundle(NAME),
                &LAYOUT.bundle(&format!(".previous-{NAME}")),
            )
            .await
            .unwrap();
        drop(session);
        // Power returns: the next package command finishes the upgrade.
        let mut session = begin(vfs, task).await.unwrap();
        assert_eq!(
            describe(&session.recovery).as_deref(),
            Some("Recovered the interrupted upgrade of xila.clock: completed.")
        );
        assert_eq!(
            session.recovery.transaction.as_ref().unwrap().1,
            Outcome::Completed
        );
        assert_eq!(
            installed_version(&mut session.storage).await.as_deref(),
            Some("0.2.0")
        );
        assert!(
            !session
                .storage
                .exists(&LAYOUT.bundle(&format!(".previous-{NAME}")))
                .await
                .unwrap()
        );
        assert!(!session.storage.exists(JOURNAL).await.unwrap());
        assert!(
            !session
                .storage
                .exists(&format!(
                    "{}/{NAME}/executables/clock-0.1.0.wasm",
                    install::INSTALLED_ROOT
                ))
                .await
                .unwrap(),
            "obsolete files go with the old bundle"
        );
        let origin = installation::origin(&mut session.storage, &LAYOUT, NAME)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(origin.version, "0.2.0");

        // Removal deletes the sealed bundle and its origin, never user data.
        let removed = installation::remove(&mut session.storage, &LAYOUT, NAME)
            .await
            .unwrap();
        assert_eq!(removed.unwrap().version, "0.2.0");
        assert!(!session.storage.exists(&LAYOUT.bundle(NAME)).await.unwrap());
        assert!(
            installation::origin(&mut session.storage, &LAYOUT, NAME)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            session.storage.read(DATA, 64).await.unwrap().as_deref(),
            Some(&b"{\"alarm\":7}"[..])
        );
        assert_eq!(
            install::list(vfs, task).await.unwrap(),
            Vec::<String>::new()
        );
    }
}
