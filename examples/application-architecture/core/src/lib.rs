#![no_std]

#[derive(Clone, Copy)]
pub enum Message {
    Increment,
    Decrement,
    Reset,
}
#[derive(Default)]
pub struct Counter {
    value: u8,
}
impl Counter {
    pub fn value(&self) -> u8 {
        self.value
    }
}
/// Logic is separate from the data and can be shared by different frontends.
pub struct CounterLogic;
impl xila_application::Application for CounterLogic {
    type Model = Counter;
    type Message = Message;
    type Error = core::convert::Infallible;
    type View<'a> = u8;

    fn update(&mut self, model: &mut Counter, message: Message) -> Result<(), Self::Error> {
        model.value = match message {
            Message::Increment => model.value.saturating_add(1).min(10),
            Message::Decrement => model.value.saturating_sub(1),
            Message::Reset => 0,
        };
        Ok(())
    }
    fn view<'a>(&'a self, model: &'a Counter) -> u8 {
        model.value()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use xila_application::Application;
    #[test]
    fn updates_stay_within_the_counter_range() {
        let mut counter = Counter::default();
        CounterLogic
            .update(&mut counter, Message::Decrement)
            .unwrap();
        assert_eq!(counter.value(), 0);
        CounterLogic
            .update(&mut counter, Message::Increment)
            .unwrap();
        assert_eq!(counter.value(), 1);
        for _ in 0..20 {
            CounterLogic
                .update(&mut counter, Message::Increment)
                .unwrap();
        }
        assert_eq!(counter.value(), 10);
        CounterLogic.update(&mut counter, Message::Reset).unwrap();
        assert_eq!(counter.value(), 0);
    }
}
