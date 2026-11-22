//! WebAssembly guests never run with an identity granted by the Shell's
//! `sudo`, whether the runner is the command given to `sudo` or is started by
//! a native program that `sudo` ran.
#![cfg(all(
    feature = "host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
testing::instantiate_global_allocator!();

use xila::{
    executable::{self, ExecutableTrait, Launch, MainFuture, Standard},
    task::EnvironmentVariable,
    users::{GroupIdentifier, UserIdentifier},
    virtual_file_system::File,
};

const RUNNER: &str = "/applications/executables/wasm";
const GUEST: &str = "/applications/executables/sudo-fixture.wasm";
const WRAPPER: &str = "/applications/executables/wrapper";

/// Exit status of the runner when the guest ran (it exits with 7).
const GUEST_RAN: isize = -7;
/// Exit status of the runner when it refused to start the guest.
const GUEST_REFUSED: isize = -1;

/// A native program that runs the guest through the runner, like any native
/// wrapper started by `sudo` could.
struct Wrapper;
impl ExecutableTrait for Wrapper {
    fn main(standard: Standard<'static>, _: Vec<String>) -> MainFuture {
        Box::pin(async move {
            let status = executable::execute(
                standard.context,
                RUNNER,
                vec![GUEST.into()],
                standard.duplicate().await.unwrap(),
                None,
            )
            .await
            .unwrap()
            .join()
            .await;
            core::num::NonZeroUsize::new(status.unsigned_abs()).map_or(Ok(()), Err)
        })
    }
}

async fn setup() -> Standard<'static> {
    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;
    executable::mount_executables!(
        context,
        context.virtual_file_system,
        task,
        &[(RUNNER, wasm::WasmExecutable), (WRAPPER, Wrapper)]
    )
    .await
    .unwrap();
    let binary = wat::parse_str(
        r#"(module
          (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
          (func (export "_start") i32.const 7 call $exit))"#,
    )
    .unwrap();
    File::write_to_path(context.virtual_file_system, task, GUEST, &binary)
        .await
        .unwrap();
    standard
}

/// Launch `path` with `arguments` the way `sudo` does: as root, with an
/// explicit environment holding the `sudo` marker.
async fn launch_as_sudo(standard: &Standard<'static>, path: &str, arguments: Vec<String>) -> isize {
    let launch = Launch::default()
        .with_environment(vec![
            EnvironmentVariable::new("User", "root"),
            EnvironmentVariable::new("Paths", "/applications/executables"),
            EnvironmentVariable::new(wasm::SUDO_USER_VARIABLE, "administrator"),
        ])
        .with_identity(UserIdentifier::ROOT, GroupIdentifier::ROOT);
    executable::execute_with(
        standard.context,
        path,
        arguments,
        standard.duplicate().await.unwrap(),
        None,
        launch,
    )
    .await
    .unwrap()
    .join()
    .await
}

#[test]
fn the_marker_matches_the_shell() {
    assert_eq!(
        wasm::SUDO_USER_VARIABLE,
        command_line_shell::SUDO_USER_VARIABLE
    );
}

#[xila::task::test(task_path = xila::task)]
async fn guests_run_without_the_sudo_marker() {
    let standard = setup().await;
    let status = executable::execute(
        standard.context,
        RUNNER,
        vec![GUEST.into()],
        standard.duplicate().await.unwrap(),
        None,
    )
    .await
    .unwrap()
    .join()
    .await;
    assert_eq!(status, GUEST_RAN);
}

#[xila::task::test(task_path = xila::task)]
async fn the_runner_given_to_sudo_refuses_to_start_a_guest() {
    let standard = setup().await;
    assert_eq!(
        launch_as_sudo(&standard, RUNNER, vec![GUEST.into()]).await,
        GUEST_REFUSED
    );
}

#[xila::task::test(task_path = xila::task)]
async fn a_native_program_run_by_sudo_cannot_start_a_guest() {
    let standard = setup().await;
    // The runner inherits the marker from the wrapper's environment.
    assert_eq!(
        launch_as_sudo(&standard, WRAPPER, Vec::new()).await,
        GUEST_REFUSED
    );
}
