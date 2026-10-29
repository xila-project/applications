#![no_std]
use example_counter_core::{Counter, Message};
use xila_application::{Frontend, dispatch};

#[derive(Clone, Copy)]
pub enum Button {
    Plus,
    Minus,
    Clear,
}
#[derive(Default)]
struct ButtonFrontend {
    value: u8,
}
impl Frontend<Counter> for ButtonFrontend {
    type Input = Button;
    type Error = core::convert::Infallible;

    fn translate(&mut self, button: Button) -> Result<Option<Message>, Self::Error> {
        Ok(Some(match button {
            Button::Plus => Message::Increment,
            Button::Minus => Message::Decrement,
            Button::Clear => Message::Reset,
        }))
    }
    fn render(&mut self, counter: &Counter) -> Result<(), Self::Error> {
        self.value = counter.value();
        Ok(())
    }
}
pub fn handle_button(counter: &mut Counter, button: Button) -> u8 {
    let mut frontend = ButtonFrontend::default();
    let _ = dispatch(counter, &mut frontend, button);
    frontend.value
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
