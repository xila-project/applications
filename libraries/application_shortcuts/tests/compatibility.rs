#[test]
fn shortcuts_preserve_the_existing_localized_installation_metadata() {
    let expected = match option_env!("EMBEDDED_I18N_LOCALE").unwrap_or("en") {
        "fr" => include_str!("fixtures/fr.txt"),
        "es" => include_str!("fixtures/es.txt"),
        "de" => include_str!("fixtures/de.txt"),
        "pt-BR" => include_str!("fixtures/pt-BR.txt"),
        "it" => include_str!("fixtures/it.txt"),
        "pl" => include_str!("fixtures/pl.txt"),
        "zh-CN" => include_str!("fixtures/zh-CN.txt"),
        _ => include_str!("fixtures/en.txt"),
    };
    for line in expected.lines() {
        let (name, shortcut) = line.split_once('\t').unwrap();
        assert_eq!(
            application_shortcuts::shortcut(name).as_deref(),
            Some(shortcut),
            "{name}"
        );
    }
    assert_eq!(application_shortcuts::shortcut("unknown"), None);
}
