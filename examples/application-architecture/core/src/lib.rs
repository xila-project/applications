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
    pub fn update(&mut self, message: Message) {
        self.value = match message {
            Message::Increment => self.value.saturating_add(1).min(10),
            Message::Decrement => self.value.saturating_sub(1),
            Message::Reset => 0,
        };
    }
    pub fn value(&self) -> u8 {
        self.value
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updates_stay_within_the_counter_range() {
        let mut counter = Counter::default();
        counter.update(Message::Decrement);
        assert_eq!(counter.value(), 0);
        counter.update(Message::Increment);
        assert_eq!(counter.value(), 1);
        for _ in 0..20 {
            counter.update(Message::Increment);
        }
        assert_eq!(counter.value(), 10);
        counter.update(Message::Reset);
        assert_eq!(counter.value(), 0);
    }
}
