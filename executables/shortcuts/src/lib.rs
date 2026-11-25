//! Shortcuts: turns the device into a remote for the computer it is
//! connected to.
//!
//! The library holds everything but the user interface and the device
//! transport, so that it is tested on the host: the mirrored human interface
//! message contract, the write protocol with its retries, the keyboard
//! layouts, the shortcut parser, the profiles of the supported computers,
//! the settings and the storage, and the mouse pad arithmetic.

extern crate alloc;

embedded_i18n::i18n!("locales");

pub mod channel;
pub mod configuration;
pub mod error;
pub mod hid;
pub mod layout;
pub mod pointer;
pub mod profile;
pub mod shortcut;
pub mod storage;

pub use error::{Error, Result};
