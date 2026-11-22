#![cfg(feature = "host")]
#![allow(clippy::disallowed_types, reason = "host-only measurement")]
//! Measures what loading the shipped applications costs under each translation
//! mode: the heap peak while the module is built from its source, what stays
//! resident once the source is released, and the load time. Building the
//! guests is slow, so this only runs on request:
//! `cargo test -p xila-wasm --test translation_real_guests -- --ignored --nocapture`.

#[path = "support/applications.rs"]
mod applications;
#[path = "support/heap.rs"]
mod heap;

use heap::TRACKING_MEMORY_MANAGER;
use std::time::Instant;
use wasm::{Translation, engine_configuration};
use wasmi::{Engine, Module};

const GUESTS: [&str; 5] = [
    "xila-calculator",
    "xila-clock",
    "xila-text-editor",
    "xila-pixel-painter",
    "xila-synth",
];

#[test]
#[ignore = "builds every guest; run on request to measure"]
fn translation_mode_load_cost_of_the_shipped_applications() {
    println!("| Guest | Source | Translation | Peak load heap | Resident module | Load time |");
    println!("|---|---:|---|---:|---:|---:|");
    for guest in GUESTS {
        let path = applications::build(guest).unwrap();
        for translation in [Translation::Eager, Translation::Lazy] {
            let engine = Engine::new(&engine_configuration(translation));
            let baseline = TRACKING_MEMORY_MANAGER.reset_peak_to_current();
            let started = Instant::now();
            let source = std::fs::read(&path).unwrap();
            let source_length = source.len();
            let module = Module::new(&engine, &source).unwrap();
            drop(source);
            let elapsed = started.elapsed();
            let peak = TRACKING_MEMORY_MANAGER.peak_bytes() - baseline;
            let resident = TRACKING_MEMORY_MANAGER.current_live_bytes() - baseline;
            println!(
                "| {guest} | {source_length} B | {translation:?} | {peak} B | {resident} B | {} µs |",
                elapsed.as_micros()
            );
            drop(module);
        }
    }
}
