#![cfg(feature = "host")]
#![allow(clippy::disallowed_types, reason = "host-only integration test")]

#[path = "support/heap.rs"]
mod heap;

use heap::TRACKING_MEMORY_MANAGER;
use xila::{synchronization::Arc, virtual_file_system::File};

fn encode_unsigned_leb128(mut value: usize, output: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn append_custom_section(binary: &mut Vec<u8>, name: &str, payload_len: usize) {
    let mut name_prefix = Vec::new();
    encode_unsigned_leb128(name.len(), &mut name_prefix);
    name_prefix.extend_from_slice(name.as_bytes());

    let section_len = name_prefix.len() + payload_len;
    binary.push(0x00); // custom section identifier
    encode_unsigned_leb128(section_len, binary);
    binary.extend_from_slice(&name_prefix);
    binary.resize(binary.len() + payload_len, 0xab);
}

fn build_yielding_guest_with_custom_padding(padding_bytes: usize) -> Vec<u8> {
    let mut binary = wat::parse_str(
        r#"(module
            (import "wasi_snapshot_preview1" "sched_yield" (func $yield (result i32)))
            (memory (export "memory") 1)
            (func (export "_start")
                (loop $spin
                    call $yield
                    drop
                    br $spin)))"#,
    )
    .unwrap();
    append_custom_section(&mut binary, "padding", padding_bytes);
    binary
}

fn build_yielding_guest_with_data_segment(data_bytes: usize) -> Vec<u8> {
    let pages = data_bytes.div_ceil(65_536) + 1;
    let payload = "a".repeat(data_bytes);
    wat::parse_str(format!(
        r#"(module
            (import "wasi_snapshot_preview1" "sched_yield" (func $yield (result i32)))
            (memory (export "memory") {pages})
            (data (i32.const 0) "{payload}")
            (func (export "_start")
                (loop $spin
                    call $yield
                    drop
                    br $spin)))"#
    ))
    .unwrap()
}

/// A guest carrying `functions` bodies it never calls, each `operations`
/// arithmetic steps long, like the library code a real application links.
fn build_yielding_guest_with_uncalled_code(functions: usize, operations: usize) -> Vec<u8> {
    let body =
        "local.get 0 i32.const 3 i32.mul i32.const 7 i32.add local.set 0 ".repeat(operations);
    let mut text = String::from(
        r#"(module
            (import "wasi_snapshot_preview1" "sched_yield" (func $yield (result i32)))
            (memory (export "memory") 1)
            (func (export "_start")
                (loop $spin
                    call $yield
                    drop
                    br $spin))"#,
    );
    for index in 0..functions {
        text.push_str(&format!(
            "\n(func (export \"uncalled_{index}\") (param i32) (result i32) {body} local.get 0)"
        ));
    }
    text.push(')');
    wat::parse_str(text).unwrap()
}

#[xila::task::test(task_path = xila::task)]
async fn launcher_releases_webassembly_source_bytes_after_compilation() {
    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;

    const CUSTOM_PADDING_BYTES: usize = 256 * 1024;
    const DATA_SEGMENT_BYTES: usize = 128 * 1024;
    const INVALID_BYTES: usize = 256 * 1024;
    const CODE_FUNCTIONS: usize = 128;
    const CODE_FUNCTION_OPERATIONS: usize = 128;

    let path = "/measured-guest.wasm";

    let scenarios: [(&str, Vec<u8>); 4] = [
        ("custom_0k", build_yielding_guest_with_custom_padding(0)),
        (
            "custom_256k",
            build_yielding_guest_with_custom_padding(CUSTOM_PADDING_BYTES),
        ),
        (
            "data_128k",
            build_yielding_guest_with_data_segment(DATA_SEGMENT_BYTES),
        ),
        (
            "code_heavy",
            build_yielding_guest_with_uncalled_code(CODE_FUNCTIONS, CODE_FUNCTION_OPERATIONS),
        ),
    ];

    let mut small_running_delta = 0;
    let mut padded_running_delta = 0;
    let mut code_heavy_peak = [0; 2];

    println!(
        "| Scenario | Source | Translation | Peak launch heap delta | Live heap delta while running |"
    );
    println!("|---|---:|---|---:|---:|");
    for (label, binary) in scenarios {
        let source_length = binary.len();
        File::write_to_path(context.virtual_file_system, task, path, &binary)
            .await
            .unwrap();
        drop(binary);

        for (mode_index, translation) in ["eager", "lazy"].into_iter().enumerate() {
            let baseline = TRACKING_MEMORY_MANAGER.reset_peak_to_current();
            let control = Arc::new(wasm::ExecutionControl::new());
            let guest_control = control.clone();
            let guest_standard = standard.duplicate().await.unwrap();
            let arguments = vec![
                "--developer-mode".to_string(),
                "--translation".to_string(),
                translation.to_string(),
                path.to_string(),
            ];

            let (handle, _) = context
                .task_manager
                .spawn(task, "MeasuredGuest", None, move |_| async move {
                    wasm::execute_with_control(guest_standard, arguments, guest_control).await
                })
                .await
                .unwrap();

            // Let the guest load, compile, instantiate, and enter its cooperative yield loop.
            for _ in 0..32 {
                xila::task::yield_now().await;
            }
            assert!(
                !handle.is_finished(),
                "{label} ({translation}): guest exited unexpectedly before stop request"
            );

            let running_live = TRACKING_MEMORY_MANAGER.current_live_bytes();
            let launch_peak = TRACKING_MEMORY_MANAGER.peak_bytes();

            control.request_stop();
            let result = handle.join().await;
            assert!(
                result.is_err(),
                "{label} ({translation}): stopped guest should report interruption"
            );

            let peak_delta = launch_peak.saturating_sub(baseline);
            let running_delta = running_live.saturating_sub(baseline);
            println!(
                "| {label} | {source_length} B | {translation} | {peak_delta} B | {running_delta} B |"
            );
            match (label, translation) {
                ("custom_0k", "eager") => small_running_delta = running_delta,
                ("custom_256k", _) => {
                    assert!(
                        peak_delta >= CUSTOM_PADDING_BYTES,
                        "positive control: allocator must observe the {CUSTOM_PADDING_BYTES}-byte source buffer during load (peak delta {peak_delta} B)"
                    );
                    if translation == "eager" {
                        padded_running_delta = running_delta;
                    }
                }
                ("code_heavy", _) => code_heavy_peak[mode_index] = peak_delta,
                _ => {}
            }
        }
    }

    // Translating only the functions that run must lower the load peak of a
    // guest that carries much more code than it calls.
    let [eager_peak, lazy_peak] = code_heavy_peak;
    assert!(
        lazy_peak < eager_peak,
        "expected lazy translation to lower the load peak ({lazy_peak} B lazy, {eager_peak} B eager)"
    );

    // The 256 KiB custom section only exists in the source bytes and must be
    // released once compilation finishes, before the guest executes.
    assert!(
        padded_running_delta.saturating_sub(small_running_delta) < CUSTOM_PADDING_BYTES / 4,
        "expected padded guest live heap ({padded_running_delta} B) to stay close to unpadded guest ({small_running_delta} B)"
    );

    // Failed load: invalid WebAssembly bytes must fail cleanly and release the source buffer.
    {
        let invalid = vec![0xff_u8; INVALID_BYTES];
        File::write_to_path(context.virtual_file_system, task, path, &invalid)
            .await
            .unwrap();
    }
    let baseline = TRACKING_MEMORY_MANAGER.reset_peak_to_current();
    let failed_standard = standard.duplicate().await.unwrap();
    let failed_result = wasm::execute_with_control(
        failed_standard,
        vec!["--developer-mode".to_string(), path.to_string()],
        Arc::new(wasm::ExecutionControl::new()),
    )
    .await;
    assert!(failed_result.is_err());
    let failed_after = TRACKING_MEMORY_MANAGER.current_live_bytes();
    assert!(
        failed_after.saturating_sub(baseline) < INVALID_BYTES / 4,
        "expected failed load to release the {INVALID_BYTES}-byte source buffer (retained {} B)",
        failed_after.saturating_sub(baseline)
    );
}
