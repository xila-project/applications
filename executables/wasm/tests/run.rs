#![cfg(feature = "host")]
testing::instantiate_global_allocator!();

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

    assert_eq!(
        execute(&standard, vec!["--developer-mode".into(), path.into()]).await,
        0
    );
    assert_eq!(
        execute(
            &standard,
            vec!["--developer-mode".into(), "--install".into(), path.into()]
        )
        .await,
        -1
    );
}

/// A guest whose entry point changes state, then calls a function far larger
/// than one execution slice. With lazy translation that callee is translated
/// mid-run; this must never end the run, whatever the slice size.
#[xila::task::test(task_path = xila::task)]
async fn lazy_translation_of_a_large_callee_survives_small_slices() {
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

    let body = "i32.const 1 drop ".repeat(4000);
    let binary = wat::parse_str(format!(
        r#"(module
          (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
          (global $seen (mut i32) (i32.const 0))
          (func $large (result i32) {body} i32.const 5)
          (func (export "_start")
            (global.set $seen (i32.add (global.get $seen) (i32.const 1)))
            (call $large)
            (global.get $seen)
            i32.add
            call $exit))"#
    ))
    .unwrap();
    let path = "/applications/executables/large-callee-fixture.wasm";
    File::write_to_path(context.virtual_file_system, task, path, &binary)
        .await
        .unwrap();

    for translation in ["eager", "lazy"] {
        for limit in ["1", "10", "100000"] {
            let arguments = vec![
                "--developer-mode".into(),
                "--translation".into(),
                translation.into(),
                "--instruction-limit".into(),
                limit.into(),
                path.into(),
            ];
            assert_eq!(
                execute(&standard, arguments).await,
                // `executable::execute` reports a guest's exit code negated.
                -6,
                "{translation} translation with a slice of {limit} instructions"
            );
        }
    }
}
