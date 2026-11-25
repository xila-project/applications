//! Home Assistant: shows and controls Home Assistant entities on a Xila
//! device.
//!
//! The library holds everything but the user interface, so that it is tested
//! on the host: the configuration, the Home Assistant requests and their
//! streaming parsers, the entity states, the service calls and the storage.

embedded_i18n::i18n!("locales");

pub mod api;
pub mod configuration;
pub mod error;
pub mod json;
pub mod net;
pub mod service;
pub mod state;
pub mod storage;
pub mod text;
