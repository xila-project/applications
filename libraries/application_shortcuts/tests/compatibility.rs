#[test]
fn shortcuts_match_existing_installation_metadata() {
    for name in [
        "calculator",
        "clock",
        "pixel_painter",
        "synth",
        "text_editor",
        "weather",
        "file_manager",
        "terminal",
        "settings",
        "activity",
        "debug",
    ] {
        assert_eq!(
            application_shortcuts::shortcut(name),
            core_application_metadata::shortcut(name),
            "{name}"
        );
    }
    assert_eq!(application_shortcuts::shortcut("unknown"), None);
}
