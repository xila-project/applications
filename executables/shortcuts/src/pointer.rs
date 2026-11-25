//! Mouse pad and scroll strip arithmetic.
//!
//! The pad sends relative movements: each pointer position gives a delta
//! from the previous one. A delta larger than one `i8` step is clamped and
//! the remainder is kept for the next event, so a fast drag moves the
//! pointer by several reports without losing distance. The scroll strip
//! turns vertical drags into wheel steps.

/// A touch area that reports relative pointer movement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pad {
    last: Option<(i32, i32)>,
    /// Distance not sent yet, because it did not fit in one `i8` step.
    pending: (i32, i32),
}

impl Pad {
    pub const fn new() -> Self {
        Self {
            last: None,
            pending: (0, 0),
        }
    }

    /// A press at (`x`, `y`) starts a drag; it moves nothing yet.
    pub fn press(&mut self, x: i32, y: i32) {
        self.last = Some((x, y));
        self.pending = (0, 0);
    }

    /// The movement to send for a pointer now at (`x`, `y`), each component
    /// clamped to -127..127, so the -128 of `i8` stays unused. The remainder
    /// stays pending.
    pub fn drag(&mut self, x: i32, y: i32) -> (i8, i8) {
        let Some((last_x, last_y)) = self.last else {
            self.last = Some((x, y));
            return (0, 0);
        };
        self.last = Some((x, y));
        let total_x = (x - last_x) + self.pending.0;
        let total_y = (y - last_y) + self.pending.1;
        let step_x = total_x.clamp(-127, 127);
        let step_y = total_y.clamp(-127, 127);
        self.pending = (total_x - step_x, total_y - step_y);
        (step_x as i8, step_y as i8)
    }

    /// The drag ended; the next press starts fresh.
    pub fn release(&mut self) {
        self.last = None;
        self.pending = (0, 0);
    }
}

/// A vertical strip that reports wheel steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scroll {
    last: Option<i32>,
    /// Pixels of drag not turned into a wheel step yet.
    pending: i32,
}

impl Scroll {
    /// Pixels of drag per wheel step.
    pub const PIXELS_PER_STEP: i32 = 24;

    pub const fn new() -> Self {
        Self {
            last: None,
            pending: 0,
        }
    }

    /// A press at `y` starts a drag on the strip.
    pub fn press(&mut self, y: i32) {
        self.last = Some(y);
        self.pending = 0;
    }

    /// The wheel steps to send for a pointer now at `y`, positive up: a
    /// drag up scrolls up. Dragging down gives negative steps. One step per
    /// [`Scroll::PIXELS_PER_STEP`] pixels of drag; the remainder stays
    /// pending in pixels.
    pub fn drag(&mut self, y: i32) -> i8 {
        let Some(last_y) = self.last else {
            self.last = Some(y);
            return 0;
        };
        self.last = Some(y);
        // Moving up (y decreases) scrolls up (wheel positive).
        self.pending += last_y - y;
        let steps = self.pending / Self::PIXELS_PER_STEP;
        self.pending %= Self::PIXELS_PER_STEP;
        steps.clamp(-127, 127) as i8
    }

    /// The drag ended.
    pub fn release(&mut self) {
        self.last = None;
        self.pending = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_sends_relative_deltas() {
        let mut pad = Pad::new();
        assert_eq!(pad.drag(10, 10), (0, 0)); // no press yet: anchors
        pad.press(10, 10);
        assert_eq!(pad.drag(10, 10), (0, 0));
        assert_eq!(pad.drag(15, 8), (5, -2));
        assert_eq!(pad.drag(15, 8), (0, 0));
        pad.release();
        pad.press(0, 0);
        assert_eq!(pad.drag(-3, 4), (-3, 4));
    }

    #[test]
    fn pad_clamps_large_deltas_and_keeps_the_remainder() {
        let mut pad = Pad::new();
        pad.press(0, 0);
        // A fast drag of 300 px right: reports of 127 while the remainder
        // exceeds one step, then the rest. The movement never reaches the
        // -128 of i8.
        assert_eq!(pad.drag(300, 0), (127, 0));
        assert_eq!(pad.drag(300, 0), (127, 0));
        assert_eq!(pad.drag(310, 0), (56, 0));
        assert_eq!(pad.drag(310, 0), (0, 0));
        // And back to the left: -127 per report.
        assert_eq!(pad.drag(0, 0), (-127, 0));
        assert_eq!(pad.drag(0, 0), (-127, 0));
        assert_eq!(pad.drag(2, 0), (-54, 0));
        assert_eq!(pad.drag(2, 0), (0, 0));
    }

    #[test]
    fn scroll_turns_drags_into_wheel_steps() {
        let mut scroll = Scroll::new();
        assert_eq!(scroll.drag(100), 0); // anchors without a press
        scroll.press(100);
        assert_eq!(scroll.drag(100), 0);
        // 24 px up is one step up.
        assert_eq!(scroll.drag(100 - 24), 1);
        assert_eq!(scroll.drag(100 - 24), 0);
        // 12 px more do not make a step yet.
        assert_eq!(scroll.drag(100 - 36), 0);
        // Another 12 px complete the step.
        assert_eq!(scroll.drag(100 - 48), 1);
        // Dragging down scrolls down.
        assert_eq!(scroll.drag(100), -2);
        scroll.release();
        scroll.press(0);
        assert_eq!(scroll.drag(24), -1);
    }

    #[test]
    fn scroll_clamps_very_fast_drags() {
        let mut scroll = Scroll::new();
        scroll.press(0);
        // 10000 px up: clamped to 127 steps, the remainder is dropped only
        // beyond the i8 range of the accumulated total.
        assert_eq!(scroll.drag(-10_000), 127);
        assert_eq!(scroll.drag(-10_000), 0);
    }
}
