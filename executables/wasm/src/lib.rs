//! Xila WebAssembly runtime and guest API (`wasm`).
//!
//! This crate exposes two feature-selected surfaces:
//!
//! - **Guest API (`guest` / `default_guest` features, target `wasm32-wasip1`)**:
//!   The `#![no_std]` guest interface linked by Xila WebAssembly applications.
//!   It re-exports `fd_control` for custom VFS descriptor control commands
//!   over `wasi_snapshot_preview1` and, when the `graphics` feature is enabled,
//!   the generated LVGL widget, window, style, layout, file-picker, and event
//!   functions backed by the `host::xila_graphics_call` import, along with
//!   `EventCode` and `EVENT_CLOSE_REQUESTED`.
//! - **Host runtime (`host` feature)**:
//!   The `wasmi`-based WebAssembly virtual machine executable (`WasmDevice`),
//!   WASI Preview 1 host imports (`WasiContext`, `add_wasi_to_linker`), linear
//!   memory pointer translation (`translation`), and host-side LVGL graphics
//!   call dispatch (`graphics`).

#![no_std]

extern crate alloc;

#[cfg(feature = "graphics")]
mod graphics_event;

#[cfg(feature = "graphics")]
pub use graphics_event::{EVENT_CLOSE_REQUESTED, EventCode};

#[cfg(feature = "host")]
mod host;

#[cfg(feature = "host")]
pub use host::*;

#[cfg(feature = "guest")]
mod guest;

#[cfg(feature = "guest")]
pub use guest::*;
