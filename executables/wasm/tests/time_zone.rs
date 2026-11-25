//! A WebAssembly guest receives the system time zone as `TZ` through WASI
//! `environ_get`, and a guest started after a change receives the new value.
#![cfg(all(
    feature = "host",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]

extern crate alloc;

use wasm::WasmExecutable;
use xila::executable::{self, Standard, build_crate, mount_executables};
use xila::file_system::Path;
use xila::virtual_file_system::File;

async fn run_guest(standard: Standard<'static>) -> alloc::vec::Vec<u8> {
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;
    let result = executable::execute(
        context,
        "/applications/executables/wasm",
        vec![
            "--developer-mode".to_string(),
            "/test_wasm.wasm".to_string(),
            "--".to_string(),
            "--write-time-zone".to_string(),
            "/time_zone.txt".to_string(),
        ],
        standard,
        None,
    )
    .await
    .unwrap()
    .join()
    .await;
    assert_eq!(result, 0);
    let mut contents = alloc::vec::Vec::new();
    File::read_from_path(
        context.virtual_file_system,
        task,
        &Path::new("/time_zone.txt"),
        &mut contents,
    )
    .await
    .unwrap();
    contents
}

#[xila::task::test(task_path = xila::task)]
async fn guests_receive_the_system_time_zone() {
    testing::instantiate_global_allocator!();

    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let virtual_file_system = context.virtual_file_system;
    let task = context.task_manager.get_current_task_identifier().await;

    let binary_path = build_crate("xila-wasm-wasm-test").unwrap();
    testing::load_file(virtual_file_system, binary_path, "/test_wasm.wasm")
        .await
        .unwrap();
    mount_executables!(
        context,
        virtual_file_system,
        task,
        &[("/applications/executables/wasm", WasmExecutable)]
    )
    .await
    .unwrap();

    // Start-up publishes the stored setting; here there is none, so UTC.
    xila::local_time::initialize(context.task_manager, virtual_file_system, task).await;
    let second = standard.duplicate().await.unwrap();
    assert_eq!(run_guest(standard).await, b"UTC0");

    // A change applies to the next guest.
    let paris =
        xila::local_time::Setting::new(Some("Europe/Paris"), "CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
    xila::local_time::set(
        context.task_manager,
        context.users_manager,
        virtual_file_system,
        task,
        &paris,
    )
    .await
    .unwrap();
    assert_eq!(run_guest(second).await, b"CET-1CEST,M3.5.0,M10.5.0/3");
}
