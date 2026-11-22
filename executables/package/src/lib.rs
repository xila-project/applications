#![no_std]
extern crate alloc;
#[cfg(test)]
extern crate std;

mod cli;
mod http;
mod information;
mod install;
mod privilege;
mod repository;
mod transaction;

#[cfg(all(test, target_os = "linux"))]
testing::instantiate_global_allocator!();

use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::{fmt::Write, num::NonZeroUsize};
use xila::executable::{self, ExecutableTrait, Standard};
use xila_package_manager::installation::{self, Activated, Mode, Storage};

pub struct PackageExecutable;

impl ExecutableTrait for PackageExecutable {
    fn main(standard: Standard<'static>, arguments: Vec<String>) -> executable::MainFuture {
        Box::pin(async move { main(standard, arguments).await })
    }
}

async fn write_line(standard: &mut Standard<'static>, line: &str) -> Result<(), String> {
    writeln!(standard.out(), "{line}").map_err(|_| "write failed".to_string())
}

/// Read one line from standard input; `None` at end of input.
async fn read_line(standard: &mut Standard<'static>) -> Result<Option<String>, String> {
    let mut line = String::new();
    let mut byte = [0u8; 1];
    loop {
        let count = standard
            .input()
            .read(&mut byte)
            .await
            .map_err(|e| e.to_string())?;
        if count == 0 {
            return Ok((!line.is_empty()).then_some(line));
        }
        if byte[0] == b'\n' {
            return Ok(Some(line));
        }
        if line.len() >= 64 {
            return Err("answer too long".into());
        }
        line.push(char::from(byte[0]));
    }
}

async fn repository_command(
    standard: &mut Standard<'static>,
    operation: cli::RepositoryOperation<'_>,
) -> Result<(), String> {
    let vfs = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    match operation {
        cli::RepositoryOperation::List => {
            for repository in repository::list(vfs, task).await? {
                let key = repository
                    .root_key_bytes()
                    .map(|key| xila_package_manager::trust::fingerprint(&key))
                    .unwrap_or_else(|_| "invalid key".into());
                write_line(
                    standard,
                    &alloc::format!(
                        "{}\t{}\troot key {key}\troot v{} index v{}",
                        repository.name,
                        repository.url,
                        repository.root_version,
                        repository.index_version
                    ),
                )
                .await?;
            }
        }
        cli::RepositoryOperation::Remove { name } => {
            repository::remove(vfs, task, name).await?;
            write_line(standard, &alloc::format!("Removed repository {name}.")).await?;
        }
        cli::RepositoryOperation::Add {
            name,
            url,
            root_key,
            yes,
        } => {
            let pending = repository::prepare(vfs, task, name, url, root_key).await?;
            if !pending.pinned {
                write_line(
                    standard,
                    &alloc::format!(
                        "The transport does not authenticate {url}. Compare this root key with one\nobtained from its publisher another way before trusting it:\n  key:         {}\n  fingerprint: {}",
                        pending.root_key(),
                        pending.fingerprint
                    ),
                )
                .await?;
                if !yes {
                    write!(
                        standard.out(),
                        "Trust this root key for {}? [y/N] ",
                        pending.name()
                    )
                    .map_err(|_| "write failed")?;
                    standard.out().flush().await.map_err(|e| e.to_string())?;
                    let answer = read_line(standard).await?.unwrap_or_default();
                    if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
                        return Err("repository not added".into());
                    }
                }
            }
            let fingerprint = pending.fingerprint.clone();
            let added = repository::add(vfs, task, pending).await?;
            write_line(
                standard,
                &alloc::format!(
                    "Added repository {} (root key {fingerprint}, root v{}, index v{}).",
                    added.name,
                    added.root_version,
                    added.index_version
                ),
            )
            .await?;
        }
    }
    Ok(())
}

fn describe(installed: &install::Installed) -> String {
    match &installed.activated {
        Activated::Installed => format!("Installed {} {}", installed.name, installed.version),
        Activated::Upgraded {
            previous: Some(previous),
        } => format!(
            "Upgraded {} from {} to {}",
            installed.name, previous.version, installed.version
        ),
        Activated::Upgraded { previous: None } => {
            format!("Upgraded {} to {}", installed.name, installed.version)
        }
    }
}

/// Install or upgrade a package from a repository or a local archive.
async fn install_command(
    standard: &mut Standard<'static>,
    session: &mut transaction::Session,
    options: cli::Install<'_>,
    mode: Mode,
) -> Result<(), String> {
    let vfs = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    let line = match options.target() {
        cli::Target::Package { name, version } => {
            if options.public_key.is_some() || options.allow_untrusted {
                return Err(
                    "--public-key and --allow-untrusted only apply to local archives".into(),
                );
            }
            let installed =
                repository::install(session, vfs, task, name, version, options.repository, mode)
                    .await?;
            format!(
                "{} from {} (verified from its root key: entry, publisher, signature and hashes).",
                describe(&installed.installed),
                installed.repository
            )
        }
        cli::Target::Archive(archive) => {
            if options.allow_untrusted || options.public_key.is_some() {
                if options.repository.is_some() {
                    return Err(
                        "--repository does not apply with --public-key or --allow-untrusted".into(),
                    );
                }
                let installed =
                    install::install_local(session, vfs, task, archive, options.public_key, mode)
                        .await?;
                if options.allow_untrusted {
                    format!(
                        "{} (publisher NOT authenticated; hashes verified).",
                        describe(&installed)
                    )
                } else {
                    format!(
                        "{} (Ed25519 signature with the given key and payload hashes verified).",
                        describe(&installed)
                    )
                }
            } else {
                let installed = repository::install_archive(
                    session,
                    vfs,
                    task,
                    archive,
                    options.repository,
                    mode,
                )
                .await?;
                format!(
                    "{} (verified through {}: entry, publisher, signature and hashes).",
                    describe(&installed.installed),
                    installed.repository
                )
            }
        }
    };
    write_line(standard, &line).await
}

/// `package info`: the installed copy offline, or the repositories' entries.
async fn information_command(
    standard: &mut Standard<'static>,
    options: cli::Information<'_>,
) -> Result<(), String> {
    let vfs = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    let (name, version) = options.name_and_version();
    let manifest = install::installed_manifest(vfs, task, name).await?;
    let blocks = match manifest {
        Some(manifest) if !options.queries_repositories() => {
            let origin = repository::origin(vfs, task, name).await?;
            alloc::vec![information::installed(&manifest, origin.as_ref())]
        }
        installed => {
            let installed = installed.as_ref().map(|manifest| manifest.version.as_str());
            let offers =
                repository::available(vfs, task, name, version, options.repository).await?;
            let wanted = match version {
                Some(version) => format!("{name} {version}"),
                None => name.to_string(),
            };
            let (warnings, blocks) = information::offered(&offers, &wanted, installed);
            for warning in warnings {
                let _ = writeln!(standard.error(), "{warning}");
            }
            blocks?
        }
    };
    for (position, lines) in blocks.iter().enumerate() {
        if position != 0 {
            write_line(standard, "").await?;
        }
        for line in lines {
            write_line(standard, line).await?;
        }
    }
    Ok(())
}

/// `package search`: names in the repositories' signed indexes.
async fn search_command(
    standard: &mut Standard<'static>,
    options: cli::Search<'_>,
) -> Result<(), String> {
    let vfs = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    let mut found = false;
    for searched in repository::search(vfs, task, options.query, options.repository).await? {
        let packages = match searched.packages {
            Ok(packages) => packages,
            Err(problem) => {
                let _ = writeln!(
                    standard.error(),
                    "package: skipping repository {}: {problem}",
                    searched.repository
                );
                continue;
            }
        };
        for package in packages {
            found = true;
            let installed = match repository::origin(vfs, task, &package.name).await? {
                Some(origin) => format!("\tinstalled {}", origin.version),
                None => String::new(),
            };
            write_line(
                standard,
                &format!(
                    "{}\t{}\t{}{installed}",
                    package.name, package.version, searched.repository
                ),
            )
            .await?;
        }
    }
    if !found {
        write_line(standard, "No package found.").await?;
    }
    Ok(())
}

/// Commands that change the installed packages; the task is privileged.
async fn change(standard: &mut Standard<'static>, command: cli::Command<'_>) -> Result<(), String> {
    let vfs = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    if let cli::Command::Repository(cli::RepositoryCommand { command }) = command {
        return repository_command(standard, command).await;
    }
    let mut session = transaction::begin(vfs, task).await?;
    if let Some(line) = transaction::describe(&session.recovery) {
        write_line(standard, &line).await?;
    }
    match command {
        cli::Command::Install(options) => {
            install_command(standard, &mut session, options, Mode::Install).await
        }
        cli::Command::Upgrade(options) => {
            install_command(standard, &mut session, options, Mode::Upgrade).await
        }
        cli::Command::Remove { name } => {
            let removed = installation::remove(&mut session.storage, &transaction::LAYOUT, name)
                .await
                .map_err(|e| format!("cannot remove {name}: {e}"))?;
            let version = removed
                .map(|origin| format!(" {}", origin.version))
                .unwrap_or_default();
            write_line(
                standard,
                &format!("Removed {name}{version}; its user data is kept."),
            )
            .await
        }
        cli::Command::Recover => {
            let removed = session.recovery.removed.len();
            if session.recovery.transaction.is_none() && removed == 0 {
                write_line(standard, "Nothing to recover.").await?;
            } else if removed != 0 {
                write_line(
                    standard,
                    &format!("Removed {removed} abandoned temporary entries."),
                )
                .await?;
            }
            Ok(())
        }
        cli::Command::List
        | cli::Command::Info(_)
        | cli::Command::Search(_)
        | cli::Command::Repository(_) => Ok(()),
    }
}

async fn run(standard: &mut Standard<'static>, arguments: &[String]) -> Result<(), String> {
    let command = match cli::parse(arguments) {
        Ok(command) => command,
        Err(error) if error.is_help() || error.is_version() => {
            writeln!(standard.out(), "{error}").map_err(|_| "write failed")?;
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    };
    let vfs = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    match command {
        cli::Command::List => {
            if transaction::VirtualStorage::new(vfs, task)
                .exists(transaction::JOURNAL)
                .await?
            {
                let _ = writeln!(
                    standard.error(),
                    "package: an interrupted package operation is pending; run `package recover`"
                );
            }
            for name in install::list(vfs, task).await? {
                let origin = match repository::origin(vfs, task, &name).await? {
                    Some(origin) => format!(
                        "\t{}\t{}",
                        origin.version,
                        origin.repository.as_deref().unwrap_or("local")
                    ),
                    None => String::new(),
                };
                write_line(standard, &format!("{name}{origin}")).await?;
            }
        }
        cli::Command::Info(options) => information_command(standard, options).await?,
        cli::Command::Search(options) => search_command(standard, options).await?,
        cli::Command::Repository(cli::RepositoryCommand {
            command: cli::RepositoryOperation::List,
        }) => {
            repository_command(standard, cli::RepositoryOperation::List).await?;
        }
        command => {
            let elevation = privilege::elevate(standard).await?;
            let result = change(standard, command).await;
            elevation.restore(standard).await;
            result?;
        }
    }
    standard.out().flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// The native command uses the Xila VFS exclusively, never the host OS.
pub async fn main(
    mut standard: Standard<'static>,
    arguments: Vec<String>,
) -> Result<(), NonZeroUsize> {
    if let Err(problem) = run(&mut standard, &arguments).await {
        let _ = writeln!(standard.error(), "package: {problem}");
        let _ = standard.error().flush().await;
        return Err(NonZeroUsize::new(1).unwrap());
    }
    Ok(())
}
