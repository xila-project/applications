use example_counter_cli::handle_command;
use example_counter_core::Counter;
use example_counter_gui::{Button, handle_button};

#[test]
fn graphical_and_command_frontends_apply_the_same_state_transitions() {
    let mut graphical = Counter::default();
    let mut command = Counter::default();
    for (button, text) in [
        (Button::Plus, "add"),
        (Button::Plus, "add"),
        (Button::Minus, "subtract"),
        (Button::Clear, "reset"),
    ] {
        assert_eq!(
            handle_button(&mut graphical, button),
            handle_command(&mut command, text).unwrap()
        );
    }
}

#[test]
fn frontend_switching_preserves_the_same_application_state() {
    let mut counter = Counter::default();
    assert_eq!(handle_button(&mut counter, Button::Plus), 1);
    assert_eq!(handle_command(&mut counter, "add"), Ok(2));
    assert_eq!(handle_button(&mut counter, Button::Minus), 1);
    assert!(handle_command(&mut counter, "invalid").is_err());
    assert_eq!(counter.value(), 1);
}
