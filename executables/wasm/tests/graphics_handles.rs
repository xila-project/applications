#![cfg(all(feature = "host", feature = "graphics"))]
extern crate abi_definitions;
testing::instantiate_global_allocator!();
#[path = "support/graphics.rs"]
mod support;

use wasm::{
    graphics::{Function, GraphicsCall, GraphicsError, GraphicsState, dispatch},
    translation::{GuestMemory, GuestPointer, WasmUsize},
};
use xila::graphics::lvgl_rust::Owner;

fn call(
    state: &mut GraphicsState,
    memory: &mut GuestMemory<'_>,
    function: Function,
    args: &[WasmUsize],
) -> Result<WasmUsize, GraphicsError> {
    let mut arguments = [0; 7];
    arguments[..args.len()].copy_from_slice(args);
    dispatch(
        state,
        memory,
        GraphicsCall {
            function: function.id(),
            arguments,
            count: args.len() as i32,
            result: 4,
        },
    )?;
    Ok(memory.read(GuestPointer::new(4)).unwrap())
}

#[xila::task::test(task_path = xila::task)]
async fn handles_check_owner_generation_class_and_slot() {
    let _standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let mut bytes = [0; 128];
    let mut memory = GuestMemory::new(&mut bytes);
    let mut first = GraphicsState::new(Some(manager));
    let mut second = GraphicsState::new(Some(manager));
    let root = call(&mut first, &mut memory, Function::WindowCreate, &[]).unwrap();
    let child = call(&mut first, &mut memory, Function::ObjectCreate, &[root]).unwrap();
    assert_eq!(
        call(&mut second, &mut memory, Function::ObjectDelete, &[child]),
        Err(GraphicsError::InvalidHandle)
    );
    for forged in [0, u32::MAX as WasmUsize, child ^ 0x10000, child & !0xffff] {
        assert_eq!(
            call(&mut first, &mut memory, Function::ObjectDelete, &[forged]),
            Err(GraphicsError::InvalidHandle)
        );
    }
    assert_eq!(
        call(
            &mut first,
            &mut memory,
            Function::LabelSetText,
            &[child, 100]
        ),
        Err(GraphicsError::InvalidHandle)
    );
    call(&mut first, &mut memory, Function::ObjectDelete, &[child]).unwrap();
    for _ in 0..600 {
        let current = call(&mut first, &mut memory, Function::ObjectCreate, &[root]).unwrap();
        assert_ne!(current, child);
        assert_eq!(
            call(&mut first, &mut memory, Function::ObjectDelete, &[child]),
            Err(GraphicsError::InvalidHandle)
        );
        call(&mut first, &mut memory, Function::ObjectDelete, &[current]).unwrap();
    }
    let parent = call(&mut first, &mut memory, Function::ObjectCreate, &[root]).unwrap();
    let descendant = call(&mut first, &mut memory, Function::ObjectCreate, &[parent]).unwrap();
    call(&mut first, &mut memory, Function::ObjectDelete, &[parent]).unwrap();
    assert_eq!(
        call(
            &mut first,
            &mut memory,
            Function::ObjectDelete,
            &[descendant]
        ),
        Err(GraphicsError::InvalidHandle)
    );
    drop(first);
    drop(second);
    assert_eq!(manager.get_window_count().await.unwrap(), 0);
    let mut replacement = GraphicsState::new(Some(manager));
    assert_eq!(
        call(
            &mut replacement,
            &mut memory,
            Function::ObjectDelete,
            &[root]
        ),
        Err(GraphicsError::InvalidHandle)
    );
}

#[xila::task::test(task_path = xila::task)]
async fn owner_exhaustion_is_reported_and_teardown_releases_ids() {
    let _standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let mut bytes = [0; 32];
    let mut memory = GuestMemory::new(&mut bytes);
    let mut states = Vec::new();
    for _ in 0..Owner::MAX {
        let mut state = GraphicsState::new(Some(manager));
        call(&mut state, &mut memory, Function::Percentage, &[50]).unwrap();
        states.push(state);
    }
    let mut extra = GraphicsState::new(Some(manager));
    assert_eq!(
        call(&mut extra, &mut memory, Function::WindowCreate, &[]),
        Err(GraphicsError::ResourceLimit)
    );
    assert_eq!(manager.get_window_count().await.unwrap(), 0);
    drop(states.pop());
    call(&mut extra, &mut memory, Function::WindowCreate, &[]).unwrap();
    drop(extra);
    drop(states);
    assert_eq!(manager.get_window_count().await.unwrap(), 0);
}

#[xila::task::test(task_path = xila::task)]
async fn cascade_delete_and_teardown_release_all_retained_resources() {
    use wasm::translation::GuestValue;
    use xila::graphics::lvgl;
    use xila::memory::ManagerTrait;

    let standard = testing::initialize(false, false).await;
    let manager = support::manager().await;
    let mut bytes = vec![0u8; 4096];
    bytes[1000..1500].fill(b'X');
    bytes[1500] = 0;
    bytes[1600..1602].copy_from_slice(b"/\0");
    let mut memory = GuestMemory::new(&mut bytes);

    // Button map with 4 x 500-byte strings + terminator.
    for i in 0..4 {
        memory
            .write(
                GuestPointer::new(400 + i * WasmUsize::SIZE as WasmUsize),
                1000 as WasmUsize,
            )
            .unwrap();
    }
    memory
        .write(
            GuestPointer::new(400 + 4 * WasmUsize::SIZE as WasmUsize),
            0 as WasmUsize,
        )
        .unwrap();

    // Grid tracks (20 cols + sentinel, 20 rows + sentinel).
    let last = lvgl::LV_GRID_TEMPLATE_LAST as i32;
    for i in 0..20 {
        memory.write(GuestPointer::new(600 + i * 4), 10i32).unwrap();
        memory.write(GuestPointer::new(720 + i * 4), 10i32).unwrap();
    }
    memory.write(GuestPointer::new(600 + 80), last).unwrap();
    memory.write(GuestPointer::new(720 + 80), last).unwrap();

    // 16 Line points (16 * 8 bytes).
    for i in 0..16 {
        memory
            .write(GuestPointer::new(2000 + i * 8), i as i32)
            .unwrap();
        memory
            .write(GuestPointer::new(2004 + i * 8), (i * 2) as i32)
            .unwrap();
    }

    // 16 Calendar dates (16 * 4 bytes).
    for i in 0..16 {
        let off = 2200 + i * 4;
        memory.write(GuestPointer::new(off), 2025u16).unwrap();
        memory.write(GuestPointer::new(off + 2), 1u8).unwrap();
        memory
            .write(GuestPointer::new(off + 3), (i + 1) as u8)
            .unwrap();
    }

    // Chart values.
    for i in 0..8 {
        memory
            .write(GuestPointer::new(2400 + i * 4), (i * 10) as i32)
            .unwrap();
    }

    // 4 pixels + 2-color palette.
    memory
        .write(GuestPointer::new(2500), 0x01000100u32)
        .unwrap();
    memory.write(GuestPointer::new(2520), 0xffffffu32).unwrap();
    memory.write(GuestPointer::new(2524), 0x112233u32).unwrap();

    let task = standard
        .context
        .task_manager
        .get_current_task_identifier()
        .await;
    let mut state =
        GraphicsState::with_file_system(Some(manager), standard.context.virtual_file_system, task);
    let root = call(&mut state, &mut memory, Function::WindowCreate, &[]).unwrap();
    let container = call(&mut state, &mut memory, Function::ObjectCreate, &[root]).unwrap();

    // Combined Grid + Map on one widget exercises Resource::GridWith.
    let matrix = call(
        &mut state,
        &mut memory,
        Function::ButtonmatrixCreate,
        &[container],
    )
    .unwrap();
    call(
        &mut state,
        &mut memory,
        Function::ButtonmatrixSetMap,
        &[matrix, 400],
    )
    .unwrap();
    call(
        &mut state,
        &mut memory,
        Function::ObjectSetGridDscArray,
        &[matrix, 600, 720],
    )
    .unwrap();

    // Calendar + Grid, Line, Chart, IndexedCanvas, TabView+Tab, Tileview+Tile.
    let calendar = call(
        &mut state,
        &mut memory,
        Function::CalendarCreate,
        &[container],
    )
    .unwrap();
    call(
        &mut state,
        &mut memory,
        Function::CalendarSetHighlights,
        &[calendar, 2200, 16],
    )
    .unwrap();
    call(
        &mut state,
        &mut memory,
        Function::ObjectSetGridDscArray,
        &[calendar, 600, 720],
    )
    .unwrap();

    let line = call(&mut state, &mut memory, Function::LineCreate, &[container]).unwrap();
    call(
        &mut state,
        &mut memory,
        Function::LineSetPoints,
        &[line, 2000, 16],
    )
    .unwrap();

    let canvas = call(
        &mut state,
        &mut memory,
        Function::IndexedCanvasCreate,
        &[container, 255, 256],
    )
    .unwrap();
    let chart = call(&mut state, &mut memory, Function::ChartCreate, &[container]).unwrap();
    call(
        &mut state,
        &mut memory,
        Function::ChartSetPointCount,
        &[chart, 8],
    )
    .unwrap();
    let series = call(
        &mut state,
        &mut memory,
        Function::ChartAddSeriesSlot,
        &[chart, 0xff0000],
    )
    .unwrap();
    let before_chart_values = __XILA_TESTING_MEMORY_MANAGER.get_used();
    call(
        &mut state,
        &mut memory,
        Function::ChartSetValues,
        &[chart, series, 2400, 8],
    )
    .unwrap();
    assert_eq!(
        __XILA_TESTING_MEMORY_MANAGER.get_used(),
        before_chart_values,
        "chart_set_values must not allocate on the host"
    );

    let tabs = call(
        &mut state,
        &mut memory,
        Function::TabviewCreate,
        &[container],
    )
    .unwrap();
    let tab = call(
        &mut state,
        &mut memory,
        Function::TabviewAddTab,
        &[tabs, 1600],
    )
    .unwrap();
    let tab_label = call(&mut state, &mut memory, Function::LabelCreate, &[tab]).unwrap();

    let tiles = call(
        &mut state,
        &mut memory,
        Function::TileviewCreate,
        &[container],
    )
    .unwrap();
    let tile = call(
        &mut state,
        &mut memory,
        Function::TileviewAddTile,
        &[tiles, 0, 0, 15],
    )
    .unwrap();

    let with_resources = __XILA_TESTING_MEMORY_MANAGER.get_used();
    call(
        &mut state,
        &mut memory,
        Function::ObjectDelete,
        &[container],
    )
    .unwrap();

    for stale in [
        matrix, calendar, line, canvas, chart, tabs, tab, tab_label, tiles, tile,
    ] {
        assert_eq!(
            call(&mut state, &mut memory, Function::ObjectDelete, &[stale]),
            Err(GraphicsError::InvalidHandle)
        );
    }
    let after_delete = __XILA_TESTING_MEMORY_MANAGER.get_used();
    assert!(
        with_resources.saturating_sub(after_delete) >= 65_280,
        "cascade delete must free canvas + retained resources (freed {} bytes)",
        with_resources.saturating_sub(after_delete)
    );

    // Full canvas quota must be available again after cascade delete.
    let canvas2 = call(
        &mut state,
        &mut memory,
        Function::IndexedCanvasCreate,
        &[root, 2, 2],
    )
    .unwrap();
    call(
        &mut state,
        &mut memory,
        Function::IndexedCanvasSetPixels,
        &[canvas2, 2500, 4, 2520, 2],
    )
    .unwrap();

    // Also attach a FilePicker and TabView+Tab to verify clean VM teardown via delete_owner.
    let picker = call(
        &mut state,
        &mut memory,
        Function::FilePickerCreate,
        &[root, 0, 1600],
    )
    .unwrap();
    assert_ne!(picker, 0);
    let tabs2 = call(&mut state, &mut memory, Function::TabviewCreate, &[root]).unwrap();
    let _tab2 = call(
        &mut state,
        &mut memory,
        Function::TabviewAddTab,
        &[tabs2, 1600],
    )
    .unwrap();

    drop(state);
    assert_eq!(manager.get_window_count().await.unwrap(), 0);
}
