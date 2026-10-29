#![no_std]
use example_counter_core::{Counter, Message};
use xila_application::{DispatchError, Frontend, dispatch};
#[derive(Debug, PartialEq, Eq)]
pub struct UnknownCommand;
#[derive(Default)]
struct CommandFrontend<'a> {
    value: u8,
    marker: core::marker::PhantomData<&'a str>,
}
impl<'a> Frontend<Counter> for CommandFrontend<'a> {
    type Input = &'a str;
    type Error = UnknownCommand;

    fn translate(&mut self, command: &str) -> Result<Option<Message>, Self::Error> {
        Ok(Some(match command {
            "add" => Message::Increment,
            "subtract" => Message::Decrement,
            "reset" => Message::Reset,
            _ => return Err(UnknownCommand),
        }))
    }
    fn render(&mut self, counter: &Counter) -> Result<(), Self::Error> {
        self.value = counter.value();
        Ok(())
    }
}
pub fn handle_command(counter: &mut Counter, command: &str) -> Result<u8, UnknownCommand> {
    let mut frontend = CommandFrontend::default();
    match dispatch(counter, &mut frontend, command) {
        Ok(_) => Ok(frontend.value),
        Err(DispatchError::Frontend(error)) => Err(error),
        Err(DispatchError::Application(never)) => match never {},
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_translates_input_and_returns_view_data() {
        let mut counter = Counter::default();
        assert_eq!(handle_command(&mut counter, "add"), Ok(1));
        assert_eq!(handle_command(&mut counter, "subtract"), Ok(0));
        assert_eq!(handle_command(&mut counter, "reset"), Ok(0));
        assert_eq!(handle_command(&mut counter, "unknown"), Err(UnknownCommand));
        assert_eq!(counter.value(), 0);
    }
}
