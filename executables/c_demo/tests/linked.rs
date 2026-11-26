//! Runs the linked C demo as a real native process on the test executor:
//! checks its output and its home file, then runs two instances concurrently,
//! each as its own login user.
#![cfg(all(
    target_os = "linux",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]

testing::instantiate_global_allocator!();

use executable::{ExecutableContext, Launch, Standard, execute_with, mount_executables};
use file_system::{AccessFlags, CreateFlags, Flags};
use task::TaskIdentifier;
use virtual_file_system::{File, VirtualFileSystem};

const EXECUTABLE: &str = "/applications/executables/c_demo";
const NOTE: &str = "A file written through Xila's C ABI.\n";

/// The environment: two login users with provisioned homes, and the demo mounted.
async fn setup() -> (&'static ExecutableContext, TaskIdentifier) {
    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;
    let virtual_file_system = context.virtual_file_system;
    let group = context
        .users_manager
        .get_group_identifier("administrator")
        .await
        .unwrap();
    let authentication_context = authentication::Context {
        virtual_file_system,
        task_manager: context.task_manager,
        users_manager: context.users_manager,
        task,
    };
    let administrator = context
        .users_manager
        .get_user_identifier("administrator")
        .await
        .unwrap();
    let second = authentication::create_user(&authentication_context, "second", "", group, None)
        .await
        .unwrap();
    for user in [administrator, second] {
        virtual_file_system
            .ensure_user_home(task, user)
            .await
            .unwrap();
    }
    mount_executables!(
        context,
        virtual_file_system,
        task,
        &[(EXECUTABLE, c_demo::CDemoExecutable)]
    )
    .await
    .unwrap();
    (context, task)
}

/// Standard streams backed by files, so the output can be read back.
async fn streams(
    context: &'static ExecutableContext,
    task: TaskIdentifier,
    label: &str,
) -> Standard<'static> {
    let virtual_file_system = context.virtual_file_system;
    let create = Flags::new(AccessFlags::READ_WRITE, Some(CreateFlags::Create), None);
    let input = File::open(
        virtual_file_system,
        task,
        "/devices/standard_io/null",
        AccessFlags::Read.into(),
    )
    .await
    .unwrap();
    let output = File::open(
        virtual_file_system,
        task,
        format!("/{label}.out").as_str(),
        create,
    )
    .await
    .unwrap();
    let error = File::open(
        virtual_file_system,
        task,
        format!("/{label}.err").as_str(),
        create,
    )
    .await
    .unwrap();
    Standard::new(context, input, output, error)
}

async fn read_all(
    virtual_file_system: &'static VirtualFileSystem,
    task: TaskIdentifier,
    path: &str,
) -> String {
    let mut file = File::open(virtual_file_system, task, path, AccessFlags::Read.into())
        .await
        .unwrap();
    let mut text = Vec::new();
    let mut chunk = [0; 256];
    loop {
        let count = file.read(&mut chunk).await.unwrap();
        if count == 0 {
            break;
        }
        text.extend_from_slice(&chunk[..count]);
    }
    file.close(virtual_file_system).await.unwrap();
    String::from_utf8(text).unwrap()
}

#[task::test]
async fn linked_c_demo_prints_and_writes_its_home_file() {
    let (context, task) = setup().await;
    let virtual_file_system = context.virtual_file_system;
    let administrator = context
        .users_manager
        .get_user_identifier("administrator")
        .await
        .unwrap();
    let group = context
        .users_manager
        .get_group_identifier("administrator")
        .await
        .unwrap();
    let standard = streams(context, task, "single").await;
    let handle = execute_with(
        context,
        EXECUTABLE,
        vec!["foo".to_string()],
        standard,
        None,
        Launch::default().with_identity(administrator, group),
    )
    .await
    .unwrap();
    assert_eq!(handle.join().await, 0);

    let out = read_all(virtual_file_system, task, "/single.out").await;
    assert!(out.contains("Hello from native C!\n"), "{out}");
    assert!(out.contains("argument: foo\n"), "{out}");
    assert!(out.contains("ABI: "), "{out}");
    assert!(out.contains("monotonic: "), "{out}");
    assert!(out.contains("realtime: "), "{out}");
    assert!(out.contains(NOTE), "{out}");
    assert_eq!(read_all(virtual_file_system, task, "/single.err").await, "");
    // The program wrote its note in the home it was granted.
    let note = read_all(virtual_file_system, task, "/users/administrator/c-demo.txt").await;
    assert_eq!(note, NOTE);
}

#[task::test]
async fn two_linked_c_demo_instances_run_concurrently() {
    let (context, task) = setup().await;
    let virtual_file_system = context.virtual_file_system;
    let group = context
        .users_manager
        .get_group_identifier("administrator")
        .await
        .unwrap();
    let administrator = context
        .users_manager
        .get_user_identifier("administrator")
        .await
        .unwrap();
    let second = context
        .users_manager
        .get_user_identifier("second")
        .await
        .unwrap();

    // Start both before joining either: they run concurrently on this executor.
    let first = execute_with(
        context,
        EXECUTABLE,
        vec!["foo".to_string()],
        streams(context, task, "first").await,
        None,
        Launch::default().with_identity(administrator, group),
    )
    .await
    .unwrap();
    let second_handle = execute_with(
        context,
        EXECUTABLE,
        vec!["bar".to_string()],
        streams(context, task, "second").await,
        None,
        Launch::default().with_identity(second, group),
    )
    .await
    .unwrap();
    assert_eq!(first.join().await, 0);
    assert_eq!(second_handle.join().await, 0);

    let out = read_all(virtual_file_system, task, "/first.out").await;
    assert!(out.contains("Hello from native C!\n"), "{out}");
    assert!(out.contains("argument: foo\n"), "{out}");
    assert!(out.contains("ABI: "), "{out}");
    assert!(out.contains(NOTE), "{out}");
    let out = read_all(virtual_file_system, task, "/second.out").await;
    assert!(out.contains("Hello from native C!\n"), "{out}");
    assert!(out.contains("argument: bar\n"), "{out}");
    assert!(out.contains("ABI: "), "{out}");
    assert!(out.contains(NOTE), "{out}");
    // Each instance wrote its own user's home.
    assert_eq!(
        read_all(virtual_file_system, task, "/users/administrator/c-demo.txt").await,
        NOTE
    );
    assert_eq!(
        read_all(virtual_file_system, task, "/users/second/c-demo.txt").await,
        NOTE
    );
}
