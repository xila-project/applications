#![no_std]
extern crate alloc;

mod cli;
mod install;

use alloc::{boxed::Box, string::{String, ToString}, vec::Vec};
use core::{fmt::Write, num::NonZeroUsize};
use xila::executable::{self, ExecutableTrait, Standard};

pub struct PackageExecutable;

impl ExecutableTrait for PackageExecutable {
    fn main(standard: Standard<'static>, arguments: Vec<String>) -> executable::MainFuture {
        Box::pin(async move { main(standard, arguments).await })
    }
}

async fn run(standard: &mut Standard<'static>, arguments: &[String]) -> Result<(), String> {
    let command = cli::parse(arguments).map_err(String::from)?;
    let vfs = standard.context.virtual_file_system;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    match command {
        cli::Command::Help => {
            write!(standard.out(), "{}", cli::USAGE).map_err(|_| "write failed")?;
        }
        cli::Command::List => {
            for name in install::list(vfs, task).await? {
                writeln!(standard.out(), "{name}").map_err(|_| "write failed")?;
            }
        }
        cli::Command::Install {
            archive,
            public_key,
            allow_untrusted,
        } => {
            let package = install::install(vfs, task, archive, public_key, allow_untrusted).await?;
            if allow_untrusted {
                writeln!(
                    standard.out(),
                    "Installed {package} (publisher NOT authenticated; hashes verified)."
                )
                .map_err(|_| "write failed")?;
            } else {
                writeln!(
                    standard.out(),
                    "Installed {package} (Ed25519 signature and payload hashes verified)."
                )
                .map_err(|_| "write failed")?;
            }
            writeln!(
                standard.out(),
                "Installed under {}/{}; no signature checks at launch.",
                install::INSTALLED_ROOT,
                package
            )
            .map_err(|_| "write failed")?;
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
        let _ = writeln!(standard.error(), "package: {problem}\n{}", cli::USAGE);
        let _ = standard.error().flush().await;
        return Err(NonZeroUsize::new(1).unwrap());
    }
    Ok(())
}
