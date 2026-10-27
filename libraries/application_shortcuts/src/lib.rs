#![no_std]
extern crate alloc;
use alloc::{format, string::String};
embedded_i18n::track_locales!();

pub fn shortcut(name: &str) -> Option<String> {
    let (label, icon, color, guest) = match name {
        "calculator" => ("Calculator", "Ca", [158, 158, 158], true),
        "clock" => ("Clock", "Cl", [59, 130, 246], true),
        "pixel_painter" => (
            embedded_i18n::translate!("applications.pixel_painter"),
            "Pa",
            [142, 36, 170],
            true,
        ),
        "synth" => ("Synth", "Sy", [103, 58, 183], true),
        "text_editor" => (
            embedded_i18n::translate!("applications.text_editor"),
            "Te",
            [76, 110, 245],
            true,
        ),
        "weather" => ("Weather", "We", [33, 150, 243], true),
        "file_manager" => ("File manager", "Fm", [0, 188, 212], false),
        "terminal" => ("Terminal", ">_", [0, 0, 0], false),
        "settings" => (
            embedded_i18n::translate!("applications.settings"),
            "Se",
            [158, 158, 158],
            false,
        ),
        "activity" => (
            embedded_i18n::translate!("applications.activity"),
            "Ac",
            [0, 150, 136],
            false,
        ),
        "debug" => ("Debug", "Db", [235, 87, 87], false),
        _ => return None,
    };
    let command = if guest { "wasm" } else { name };
    let arguments = if guest {
        format!("\"/applications/executables/{name}\"")
    } else {
        String::new()
    };
    Some(format!(
        r#"{{"name": "{label}","command": "/applications/executables/{command}","arguments": [{arguments}],"terminal":false,"icon_string":"{icon}","icon_color":[{},{},{}]}}"#,
        color[0], color[1], color[2]
    ))
}
