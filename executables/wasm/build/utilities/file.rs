use std::{fmt::Display, fs, path::Path};

use super::format::format_rust;

pub fn write_generated_rust(path: &Path, source: impl Display) {
    fs::write(
        path,
        format!("/// Auto-generated file for Xila bindings WASM module\n/// Do not edit manually.\n\n{source}"),
    )
    .expect("failed to write generated guest bindings");
    format_rust(path);
}
