#![no_std]
use example_counter_core::{Counter, Message};
#[derive(Clone, Copy)]
pub enum Button {
    Plus,
    Minus,
    Clear,
}
pub fn handle_button(counter: &mut Counter, button: Button) -> u8 {
    let message = match button {
        Button::Plus => Message::Increment,
        Button::Minus => Message::Decrement,
        Button::Clear => Message::Reset,
    };
    counter.update(message);
    counter.value()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn button_translates_input_and_returns_view_data() {
        let mut counter = Counter::default();
        assert_eq!(handle_button(&mut counter, Button::Plus), 1);
        assert_eq!(handle_button(&mut counter, Button::Minus), 0);
        assert_eq!(handle_button(&mut counter, Button::Clear), 0);
    }
}
