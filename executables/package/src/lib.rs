#![no_std]
extern crate alloc;

mod cli;
mod http;
mod install;
mod repository;

use alloc::{
    boxed::Box,
    string::{String, ToString},
    vec::Vec,
};
use core::{fmt::Write, num::NonZeroUsize};
use xila::executable::{self, ExecutableTrait, Standard};

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
            install::ensure_root(vfs, task).await?;
            repository::remove(vfs, task, name).await?;
            write_line(standard, &alloc::format!("Removed repository {name}.")).await?;
        }
        cli::RepositoryOperation::Add {
            name,
            url,
            root_key,
            yes,
        } => {
            install::ensure_root(vfs, task).await?;
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
            for name in install::list(vfs, task).await? {
                let origin = match repository::origin(vfs, task, &name).await? {
                    Some(origin) => alloc::format!(
                        "\t{}\t{}",
                        origin.version,
                        origin.repository.as_deref().unwrap_or("local")
                    ),
                    None => String::new(),
                };
                write_line(standard, &alloc::format!("{name}{origin}")).await?;
            }
        }
        cli::Command::Repository(cli::RepositoryCommand { command }) => {
            repository_command(standard, command).await?;
        }
        cli::Command::Install(options) => match options.target() {
            cli::Target::Package { name, version } => {
                if options.public_key.is_some() || options.allow_untrusted {
                    return Err(
                        "--public-key and --allow-untrusted only apply to local archives".into(),
                    );
                }
                let installed =
                    repository::install(vfs, task, name, version, options.repository).await?;
                write_line(
                    standard,
                    &alloc::format!(
                        "Installed {} {} from {} (verified from its root key: entry, publisher, signature and hashes).",
                        installed.name, installed.version, installed.repository
                    ),
                )
                .await?;
            }
            cli::Target::Archive(archive) => {
                if options.repository.is_some() {
                    return Err("--repository only applies to package names".into());
                }
                let digest = install::archive_sha256(vfs, task, archive).await?;
                let (package, version) = install::install(
                    vfs,
                    task,
                    archive,
                    options.public_key,
                    options.allow_untrusted,
                )
                .await?;
                repository::record_origin(vfs, task, None, &package, &version, &digest).await?;
                if options.allow_untrusted {
                    write_line(
                        standard,
                        &alloc::format!(
                            "Installed {package} (publisher NOT authenticated; hashes verified)."
                        ),
                    )
                    .await?;
                } else {
                    write_line(
                        standard,
                        &alloc::format!(
                            "Installed {package} (Ed25519 signature and payload hashes verified)."
                        ),
                    )
                    .await?;
                }
            }
        },
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
