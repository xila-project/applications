#![cfg(feature = "host")]
extern crate abi_definitions;
drivers_std::memory::instantiate_global_allocator!();

use xila::{
    executable::{self, Standard},
    virtual_file_system::File,
};

async fn execute(standard: &Standard<'static>, arguments: Vec<String>) -> isize {
    executable::execute(
        standard.context,
        "/applications/executables/wasm",
        arguments,
        standard.duplicate().await.unwrap(),
        None,
    )
    .await
    .unwrap()
    .join()
    .await
}

#[xila::task::test(task_path = xila::task)]
async fn wasm_runs_only_start_and_rejects_legacy_install_mode() {
    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;
    executable::mount_executables!(
        context,
        context.virtual_file_system,
        task,
        &[("/applications/executables/wasm", wasm::WasmExecutable)]
    )
    .await
    .unwrap();

    // __install may exist in an old third-party binary, but must never run.
    let binary = wat::parse_str(
        r#"(module
          (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
          (func (export "_start") i32.const 0 call $exit)
          (func (export "__install") i32.const 7 call $exit))"#,
    )
    .unwrap();
    let path = "/applications/executables/start-only-fixture.wasm";
    File::write_to_path(context.virtual_file_system, task, path, &binary)
        .await
        .unwrap();

    assert_eq!(execute(&standard, vec![path.into()]).await, 0);
    assert_eq!(
        execute(&standard, vec!["--install".into(), path.into()]).await,
        -1
    );
}
