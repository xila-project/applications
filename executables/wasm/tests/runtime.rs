#![cfg(feature = "host")]
extern crate abi_definitions;
testing::instantiate_global_allocator!();
use std::{
    num::NonZeroU32,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use wasm::{GlobalStore, WasiContext, add_wasi_to_linker, execution::run};
use wasmi::{Config, Engine, Linker, Module, Store};

#[xila::task::test(task_path=xila::task)]
async fn resumable_execution_yields_sleeps_and_preserves_real_traps() {
    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let mut linker = Linker::new(&engine);
    add_wasi_to_linker(&mut linker).unwrap();
    linker
        .func_wrap("host", "fail", || -> Result<(), wasmi::Error> {
            Err(wasmi::Error::new("real host failure"))
        })
        .unwrap();
    let word = if cfg!(feature = "memory_64") {
        "i64"
    } else {
        "i32"
    };
    for kind in [
        "fuel",
        "sleep",
        "yield",
        "clock_check",
        "exit",
        "exit_error",
        "host_error",
        "wasm_error",
    ] {
        let progress = Arc::new(AtomicUsize::new(0));
        let observed = progress.clone();
        let (ticker, _) = context
            .task_manager
            .spawn(task, "Progress", None, move |_| async move {
                for _ in 0..10 {
                    observed.fetch_add(1, Ordering::SeqCst);
                    xila::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        // Register the competing task before measuring guest scheduling; its
        // spawn wrapper itself may need more than one poll to become runnable.
        while progress.load(Ordering::SeqCst) == 0 {
            xila::task::yield_now().await;
        }
        progress.store(0, Ordering::SeqCst);
        let body=match kind {
            "fuel"=>"(local $i i32) (loop $loop local.get $i i32.const 1 i32.add local.tee $i i32.const 100000 i32.lt_u br_if $loop)".to_string(),
            "sleep"=>format!("i32.const 17 i32.const 1 i32.store i32.const 25 i64.const 20000000 i64.store i32.const 9 i32.const 0 i32.store8 {word}.const 1 {word}.const 100 {word}.const 1 {word}.const 200 call $poll if unreachable end"),
            "yield"=>"(local $i i32) (loop $loop call $yield if unreachable end local.get $i i32.const 1 i32.add local.tee $i i32.const 10 i32.lt_u br_if $loop)".to_string(),
            "clock_check"=>format!("i32.const 1 i64.const 0 {word}.const 25 call $clock if unreachable end i32.const 25 i64.load i64.const 10000000000 i64.gt_u if unreachable end"),
            "exit"=>"i32.const 0 call $exit unreachable".to_string(),
            "exit_error"=>"i32.const 7 call $exit unreachable".to_string(),
            "host_error"=>"call $fail".to_string(),
            _=>"unreachable".to_string(),
        };
        let wasm=wat::parse_str(format!(r#"(module
            (import "wasi_snapshot_preview1" "poll_oneoff" (func $poll (param {word} {word} {word} {word})(result i32)))
            (import "wasi_snapshot_preview1" "sched_yield" (func $yield (result i32)))
            (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
            (import "wasi_snapshot_preview1" "clock_time_get" (func $clock (param i32 i64 {word}) (result i32)))
            (import "host" "fail" (func $fail)) (memory (export "memory") 1)
            (func (export "_start") {body}))"#)).unwrap();
        let module = Module::new(&engine, &wasm[..]).unwrap();
        let mut store = Store::new(
            &engine,
            GlobalStore {
                control: Default::default(),
                context,
                #[cfg(feature = "graphics")]
                graphics: wasm::graphics::GraphicsState::new(context.graphics_manager),
                pending: None,
                wasi: WasiContext {
                    files: Default::default(),
                    rights: Default::default(),
                    next_fd: 3,
                    arguments: vec![],
                    environment: vec![],
                    task,
                    prestats: Default::default(),
                    exit_code: None,
                },
            },
        );
        store.set_fuel(1000).unwrap();
        let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
        let start = instance.get_typed_func::<(), ()>(&store, "_start").unwrap();
        let result = run(&mut store, start, NonZeroU32::new(1000).unwrap()).await;
        assert_eq!(
            result.is_ok(),
            matches!(kind, "fuel" | "sleep" | "yield" | "clock_check" | "exit"),
            "{kind}: {result:?}"
        );
        if matches!(kind, "fuel" | "sleep" | "yield") {
            assert!(
                progress.load(Ordering::SeqCst) > 0,
                "no scheduler progress during {kind}"
            );
        }
        if kind == "sleep" {
            use wasm::translation::{GuestMemory, GuestPointer, WasmUsize};
            let memory = instance.get_memory(&store, "memory").unwrap();
            let view = GuestMemory::new(memory.data_mut(&mut store));
            assert_eq!(view.read(GuestPointer::<WasmUsize>::new(200)), Some(1));
        }
        ticker.join().await;
    }
}

fn looping_guest(
    context: &'static xila::executable::ExecutableContext,
    task: xila::task::TaskIdentifier,
    control: Arc<wasm::ExecutionControl>,
) -> (Store<GlobalStore>, wasmi::TypedFunc<(), ()>) {
    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let mut linker = Linker::new(&engine);
    add_wasi_to_linker(&mut linker).unwrap();
    let wasm = wat::parse_str(
        r#"(module (memory (export "memory") 1) (func (export "_start") (loop $spin br $spin)))"#,
    )
    .unwrap();
    let module = Module::new(&engine, &wasm[..]).unwrap();
    let mut store = Store::new(
        &engine,
        GlobalStore {
            control,
            context,
            #[cfg(feature = "graphics")]
            graphics: wasm::graphics::GraphicsState::new(context.graphics_manager),
            pending: None,
            wasi: WasiContext {
                files: Default::default(),
                next_fd: 3,
                arguments: vec![],
                environment: vec![],
                task,
                prestats: vec![],
                exit_code: None,
            },
        },
    );
    store.set_fuel(1000).unwrap();
    let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
    let start = instance.get_typed_func::<(), ()>(&store, "_start").unwrap();
    (store, start)
}

#[xila::task::test(task_path=xila::task)]
async fn termination_signal_or_control_token_stops_only_that_guest() {
    let standard = testing::initialize(false, false).await;
    let context = standard.context;
    let manager = context.task_manager;
    let root = manager.get_current_task_identifier().await;
    let spawn_guest = async |control: Arc<wasm::ExecutionControl>| {
        manager
            .spawn(root, "Guest", None, move |task| async move {
                let (mut store, start) = looping_guest(context, task, control);
                let result = run(&mut store, start, NonZeroU32::new(1000).unwrap()).await;
                // Dropping the store releases the guest's WASI handles.
                drop(store);
                matches!(&result, Err(error) if format!("{error:?}") == "Interrupted")
            })
            .await
            .unwrap()
    };

    let signaled_control = Arc::new(wasm::ExecutionControl::new());
    let (signaled, signaled_task) = spawn_guest(signaled_control.clone()).await;
    let token_control = Arc::new(wasm::ExecutionControl::new());
    let (tokened, _) = spawn_guest(token_control.clone()).await;
    for _ in 0..10 {
        xila::task::yield_now().await;
    }
    assert!(!signaled.is_finished() && !tokened.is_finished());

    manager
        .send_signal(signaled_task, xila::task::Signal::Termination)
        .await
        .unwrap();
    assert!(signaled.join().await, "signaled guest was not interrupted");
    // The signal targeted one task: the other guest is still running.
    for _ in 0..10 {
        xila::task::yield_now().await;
    }
    assert!(!tokened.is_finished());
    assert!(!signaled_control.is_stopped());

    token_control.request_stop();
    assert!(tokened.join().await, "stopped guest was not interrupted");
}
