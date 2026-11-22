#![no_std]

embedded_i18n::i18n!("locales");

extern crate alloc;
#[cfg(target_arch = "wasm32")]
extern crate std;

pub mod api;
#[cfg(test)]
mod docs;
pub mod model;
pub mod net;
pub mod state;
pub mod trigger;
pub mod view;
