use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[derive(Deserialize)]
struct ShortcutIcon {
    icon_string: String,
    icon_color: [u8; 3],
}

fn main() {
    let shortcut = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("shortcut.json");
    println!("cargo:rerun-if-changed=../wasm_guest_build.rs");
    println!("cargo:rerun-if-changed={}", shortcut.display());

    if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
        // Use 1-byte linear memory pages (custom page sizes proposal) so the initial memory is
        // exactly what the module needs instead of a multiple of 64 KiB.
        println!("cargo:rustc-link-arg-bins=--page-size=1");
        // The stack is placed first, so an overflow traps instead of corrupting data. Measured peak
        // stack depth over the host tests is at most 11.3 KiB (synth) for these guests; 32 KiB keeps
        // at least a 2.9x margin while saving 32 KiB of initial linear memory per guest.
        println!("cargo:rustc-link-arg-bins=-zstack-size=32768");
    }

    let contents =
        fs::read(&shortcut).unwrap_or_else(|error| panic!("{}: {error}", shortcut.display()));
    let ShortcutIcon {
        icon_string,
        icon_color: [red, green, blue],
    } = serde_json::from_slice(&contents)
        .unwrap_or_else(|error| panic!("{}: {error}", shortcut.display()));
    assert!(
        !icon_string.as_bytes().contains(&0),
        "{}: icon_string must not contain a NUL byte",
        shortcut.display()
    );

    let generated = format!(
        "const ICON_TEXT: &core::ffi::CStr = c{icon_string:?};\n\
         const ICON_COLOR: wasm::Color = wasm::Color {{ red: {red}, green: {green}, blue: {blue} }};\n"
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("shortcut_icon.rs");
    fs::write(&output, generated).unwrap_or_else(|error| panic!("{}: {error}", output.display()));
}
