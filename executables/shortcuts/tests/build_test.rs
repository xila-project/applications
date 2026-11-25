use std::{fs, path::Path};

#[test]
fn workspace_registers_shortcuts_crate() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml");
    let manifest = fs::read_to_string(manifest).unwrap();

    assert!(manifest.contains(
        "shortcuts = { package = \"xila-shortcuts\", path = \"executables/shortcuts\" }"
    ));
    assert!(manifest.contains("\"executables/shortcuts\""));
}
