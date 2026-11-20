use std::{path::Path, process::Command};

pub fn format_rust(path: &Path) {
    let status = Command::new("rustfmt")
        .arg(path)
        .status()
        .expect("failed to run rustfmt");
    assert!(status.success(), "rustfmt failed for {}", path.display());
}
