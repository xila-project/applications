//! Dashboard: Grafana panel values and charts on a Xila device.
//!
//! The library holds everything but the user interface, so that it is tested
//! on the host: the configuration, the Grafana requests and their streaming
//! parsers, the dashboard import and the value formatting.

embedded_i18n::i18n!("locales");

pub mod configuration;
pub mod error;
pub mod format;
pub mod grafana;
pub mod import;
pub mod json;
pub mod model;
pub mod net;
pub mod query;
pub mod response;
pub mod storage;
pub mod text;
