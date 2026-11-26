use crate::error::{Error, Result};
use xila::{
    file_system::{Kind, Path, PathOwned},
    virtual_file_system::Directory,
};

pub async fn resolve(
    executable_context: &'static xila::executable::ExecutableContext,
    command: &str,
    paths: &[&Path],
) -> Result<PathOwned> {
    if command.is_empty() || command == "." || command == ".." || command.contains('/') {
        return Err(Error::CommandNotFound);
    }

    let virtual_file_system = &executable_context.virtual_file_system;
    let task = executable_context
        .task_manager
        .get_current_task_identifier()
        .await;

    for path in paths {
        if path.is_empty() || !path.is_valid() {
            continue;
        }
        let Ok(mut directory) = Directory::open(virtual_file_system, task, path).await else {
            continue;
        };
        let mut found = false;
        while let Ok(Some(entry)) = directory.read().await {
            // A directory is not an executable: keep looking, also in the later `Paths` entries.
            if entry.name == command && entry.kind != Kind::Directory {
                found = true;
                break;
            }
        }
        let _ = directory.close(virtual_file_system).await;
        if found {
            return path.append(command).ok_or(Error::InvalidPath);
        }
    }

    Err(Error::CommandNotFound)
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use crate::Error;
    testing::instantiate_global_allocator!();

    use xila::{
        file_system::{AccessFlags, CreateFlags, Flags, Path},
        virtual_file_system::{Directory, File},
    };

    async fn create_file(context: &'static xila::executable::ExecutableContext, path: &str) {
        let virtual_file_system = &context.virtual_file_system;
        let task = context.task_manager.get_current_task_identifier().await;

        let file = File::open(
            virtual_file_system,
            task,
            Path::from_str(path),
            Flags::new(AccessFlags::Write, Some(CreateFlags::Create), None),
        )
        .await
        .unwrap();

        file.close(virtual_file_system).await.unwrap();
    }

    async fn create_directory(context: &'static xila::executable::ExecutableContext, path: &str) {
        let virtual_file_system = &context.virtual_file_system;
        let task = context.task_manager.get_current_task_identifier().await;

        Directory::create(virtual_file_system, task, Path::from_str(path))
            .await
            .unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[xila::task::test(task_path = xila::task)]
    async fn resolve_returns_path_from_later_search_directory() {
        let standard = testing::initialize(false, false).await;
        let executable_context = standard.context;

        create_directory(executable_context, "/resolver_test_a").await;
        create_directory(executable_context, "/resolver_test_b").await;
        create_file(executable_context, "/resolver_test_b/hello").await;

        let result = resolve(
            executable_context,
            "hello",
            &[
                Path::from_str("/resolver_test_a"),
                Path::from_str("/resolver_test_b"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(result.as_str(), "/resolver_test_b/hello");
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[xila::task::test(task_path = xila::task)]
    async fn resolve_prefers_first_matching_directory() {
        let standard = testing::initialize(false, false).await;
        let executable_context = standard.context;

        create_directory(executable_context, "/resolver_test_first").await;
        create_directory(executable_context, "/resolver_test_second").await;
        create_file(executable_context, "/resolver_test_first/tool").await;
        create_file(executable_context, "/resolver_test_second/tool").await;

        let result = resolve(
            executable_context,
            "tool",
            &[
                Path::from_str("/resolver_test_first"),
                Path::from_str("/resolver_test_second"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(result.as_str(), "/resolver_test_first/tool");
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[xila::task::test(task_path = xila::task)]
    async fn resolve_returns_command_not_found_for_unknown_command() {
        let standard = testing::initialize(false, false).await;
        let executable_context = standard.context;

        create_directory(executable_context, "/resolver_test_empty").await;

        let result = resolve(
            executable_context,
            "missing_command",
            &[Path::from_str("/resolver_test_empty")],
        )
        .await;

        assert!(matches!(result, Err(Error::CommandNotFound)));
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[xila::task::test(task_path = xila::task)]
    async fn resolve_rejects_command_tokens_that_are_not_plain_names() {
        let standard = testing::initialize(false, false).await;
        let executable_context = standard.context;

        for command in ["", ".", "..", "sub/tool", "/absolute/path"] {
            let result = resolve(executable_context, command, &[Path::from_str("/")]).await;
            assert!(
                matches!(result, Err(Error::CommandNotFound)),
                "command {command:?} must be rejected"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[xila::task::test(task_path = xila::task)]
    async fn resolve_skips_a_directory_matching_the_command_in_an_earlier_search_directory() {
        let standard = testing::initialize(false, false).await;
        let executable_context = standard.context;

        create_directory(executable_context, "/resolver_shadow").await;
        // A directory with the command's name must not shadow the executable.
        create_directory(executable_context, "/resolver_shadow/tool").await;
        create_directory(executable_context, "/resolver_real").await;
        create_file(executable_context, "/resolver_real/tool").await;

        let result = resolve(
            executable_context,
            "tool",
            &[
                Path::from_str("/resolver_shadow"),
                Path::from_str("/resolver_real"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(result.as_str(), "/resolver_real/tool");
    }

    struct Mounted;
    impl xila::executable::ExecutableTrait for Mounted {
        fn main(
            _: xila::executable::Standard<'static>,
            _: alloc::vec::Vec<alloc::string::String>,
        ) -> xila::executable::MainFuture {
            alloc::boxed::Box::pin(async { Ok(()) })
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[xila::task::test(task_path = xila::task)]
    async fn resolve_skips_empty_and_invalid_segments_and_resolves_a_mounted_executable() {
        let standard = testing::initialize(false, false).await;
        let executable_context = standard.context;
        let task = executable_context
            .task_manager
            .get_current_task_identifier()
            .await;

        create_directory(executable_context, "/resolver_mounted").await;
        xila::executable::mount_executables!(
            executable_context,
            executable_context.virtual_file_system,
            task,
            &[(&"/resolver_mounted/package", Mounted)]
        )
        .await
        .unwrap();

        let result = resolve(
            executable_context,
            "package",
            &[
                Path::from_str(""),
                Path::from_str("/bad path"),
                Path::from_str("/resolver_mounted"),
            ],
        )
        .await
        .unwrap();

        assert_eq!(result.as_str(), "/resolver_mounted/package");

        // `mount_executables!` mounts a character device, which stays resolvable.
        let statistics = executable_context
            .virtual_file_system
            .get_statistics(&"/resolver_mounted/package")
            .await
            .unwrap();
        assert_eq!(statistics.kind, xila::file_system::Kind::CharacterDevice);
    }
}
