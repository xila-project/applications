#![no_std]
use example_counter_core::{Counter, Message};
#[derive(Debug, PartialEq, Eq)]
pub struct UnknownCommand;
pub fn handle_command(counter: &mut Counter, command: &str) -> Result<u8, UnknownCommand> {
    let message = match command {
        "add" => Message::Increment,
        "subtract" => Message::Decrement,
        "reset" => Message::Reset,
        _ => return Err(UnknownCommand),
    };
    counter.update(message);
    Ok(counter.value())
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
