//! WebAssembly guests never run with an identity granted by the Shell's
//! `sudo`, whether the runner is the command given to `sudo` or is started by
//! a native program that `sudo` ran. The runtime checks Core's granted
//! identity mark, not the environment.
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

/// The runner's arguments. The fixture is not part of an installed package,
/// so it runs only in developer mode; this way, the granted identity is the
/// only reason left for refusing it.
fn guest_arguments() -> Vec<String> {
    vec!["--developer-mode".into(), GUEST.into()]
}

/// Exit status of the runner when the guest ran (it exits with 7).
const GUEST_RAN: isize = -7;
/// Exit status of the runner when it refused to start the guest.
const GUEST_REFUSED: isize = -1;

/// A native program that runs the guest through the runner, like any native
/// wrapper started by `sudo` could. It gives the runner an explicit
/// environment without `Sudo_user`, so only Core's mark can stop the guest.
struct Wrapper;
impl ExecutableTrait for Wrapper {
    fn main(standard: Standard<'static>, _: Vec<String>) -> MainFuture {
        Box::pin(async move {
            let launch =
                Launch::default().with_environment(vec![EnvironmentVariable::new("User", "root")]);
            let status = executable::execute_with(
                standard.context,
                RUNNER,
                guest_arguments(),
                standard.duplicate().await.unwrap(),
                None,
                launch,
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

/// Launch `path` with `arguments` and `launch`, and return its exit status.
async fn launch(
    standard: &Standard<'static>,
    path: &str,
    arguments: Vec<String>,
    launch: Launch,
) -> isize {
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

/// The launch that `sudo` uses: root identity and an explicit environment.
fn as_sudo() -> Launch {
    Launch::default()
        .with_environment(vec![
            EnvironmentVariable::new("User", "root"),
            EnvironmentVariable::new("Paths", "/applications/executables"),
            EnvironmentVariable::new(command_line_shell::SUDO_USER_VARIABLE, "administrator"),
        ])
        .with_identity(UserIdentifier::ROOT, GroupIdentifier::ROOT)
}

#[xila::task::test(task_path = xila::task)]
async fn guests_run_without_a_granted_identity() {
    let standard = setup().await;
    assert_eq!(
        launch(&standard, RUNNER, guest_arguments(), Launch::default()).await,
        GUEST_RAN
    );
}

#[xila::task::test(task_path = xila::task)]
async fn the_sudo_variable_alone_does_not_refuse_a_guest() {
    // The check does not read the environment, which any task can change.
    let standard = setup().await;
    let environment = Launch::default().with_environment(vec![EnvironmentVariable::new(
        command_line_shell::SUDO_USER_VARIABLE,
        "administrator",
    )]);
    assert_eq!(
        launch(&standard, RUNNER, guest_arguments(), environment).await,
        GUEST_RAN
    );
}

#[xila::task::test(task_path = xila::task)]
async fn the_runner_given_to_sudo_refuses_to_start_a_guest() {
    let standard = setup().await;
    assert_eq!(
        launch(&standard, RUNNER, guest_arguments(), as_sudo()).await,
        GUEST_REFUSED
    );
}

#[xila::task::test(task_path = xila::task)]
async fn a_native_program_run_by_sudo_cannot_start_a_guest() {
    let standard = setup().await;
    // The wrapper drops `Sudo_user` from the runner's environment, and the
    // runner still inherits Core's mark.
    assert_eq!(
        launch(&standard, WRAPPER, Vec::new(), as_sudo()).await,
        GUEST_REFUSED
    );
}

#[xila::task::test(task_path = xila::task)]
async fn a_granted_identity_of_another_user_also_refuses_a_guest() {
    // `sudo -u USER` grants an identity too.
    let standard = setup().await;
    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    let user = standard.context.task_manager.get_user(task).await.unwrap();
    let group = standard.context.task_manager.get_group(task).await.unwrap();
    let same_identity = Launch::default().with_identity(user, group);
    assert_eq!(
        launch(&standard, RUNNER, guest_arguments(), same_identity).await,
        GUEST_REFUSED
    );
}
