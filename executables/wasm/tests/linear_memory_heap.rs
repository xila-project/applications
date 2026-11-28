#![cfg(feature = "host")]
#![allow(clippy::disallowed_types, reason = "host-only integration test")]

#[path = "support/heap.rs"]
mod heap;

use heap::TRACKING_MEMORY_MANAGER;
use xila::virtual_file_system::File;

#[xila::task::test(task_path = xila::task)]
async fn guest_growth_does_not_increase_peak_heap() {
    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;
    let path = "/linear-memory.wasm";
    const MAXIMUM_BYTES: usize = 8 * 65_536;

    let mut peaks = [0; 2];
    for (index, growth) in [0, 7].into_iter().enumerate() {
        let binary = wat::parse_str(format!(
            r#"(module
                (memory 1 8)
                (func (export "_start")
                    i32.const {growth}
                    memory.grow
                    i32.const 1
                    i32.ne
                    if unreachable end))"#
        ))
        .unwrap();
        File::write_to_path(context.virtual_file_system, task, path, &binary)
            .await
            .unwrap();
        drop(binary);
        let runner_standard = standard.duplicate().await.unwrap();
        let baseline = TRACKING_MEMORY_MANAGER.reset_peak_to_current();
        wasm::execute_with_control(
            runner_standard,
            vec!["--developer-mode".into(), path.into()],
            Default::default(),
        )
        .await
        .unwrap();
        peaks[index] = TRACKING_MEMORY_MANAGER.peak_bytes() - baseline;
        let retained = TRACKING_MEMORY_MANAGER
            .current_live_bytes()
            .saturating_sub(baseline);
        assert!(retained < MAXIMUM_BYTES / 16, "retained {retained} bytes");
    }

    let [without_growth, with_growth] = peaks;
    println!("Peak without growth: {without_growth} bytes. Peak with growth: {with_growth} bytes.");
    assert!(without_growth >= MAXIMUM_BYTES);
    assert!(without_growth.abs_diff(with_growth) < MAXIMUM_BYTES / 16);
}
