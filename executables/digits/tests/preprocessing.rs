use core::time::Duration;
use digits::{percentage, prepare, tenths_of_milliseconds, top_three};
use pixel_painter::drawing::{Drawing, PIXELS};

#[test]
fn blank_and_malformed_drawings_have_no_prediction() {
    assert!(prepare(&[0; PIXELS]).is_none());
    assert!(prepare(&[1; 10]).is_none());
}

#[test]
fn translated_strokes_produce_the_same_centered_input() {
    let mut first = Drawing::new();
    first.begin_stroke(5, 10, 1, 3);
    first.continue_stroke(5, 50, 1, 3);
    let mut second = Drawing::new();
    second.begin_stroke(90, 40, 1, 3);
    second.continue_stroke(90, 80, 1, 3);
    let input = prepare(first.pixels()).expect("nonblank drawing");
    assert_eq!(Some(input), prepare(second.pixels()));
    assert!(input.iter().all(|v| (0.0..=1.0).contains(v)));
    assert!(input.iter().any(|&v| v > 0.5));
    // MNIST: bright ink, dark background, with a four-pixel border.
    assert!(input[..28 * 4].iter().all(|&v| v == 0.0));
    assert!(input[28 * 24..].iter().all(|&v| v == 0.0));
}

#[test]
fn rankings_are_descending_and_reject_invalid_scores() {
    let scores = [0.02, 0.03, 0.1, 0.02, 0.5, 0.02, 0.02, 0.25, 0.02, 0.02];
    assert_eq!(top_three(scores), Some([(4, 0.5), (7, 0.25), (2, 0.1)]));
    assert!(top_three([f32::NAN; 10]).is_none());
    assert!(top_three([f32::INFINITY; 10]).is_none());
    assert!(top_three([-1.0; 10]).is_none());
    assert!(top_three([0.0; 10]).is_none());
}

#[test]
fn percentages_round_half_up_and_stay_in_range() {
    assert_eq!(percentage(0.0), 0);
    assert_eq!(percentage(1.0), 100);
    assert_eq!(percentage(0.994), 99);
    assert_eq!(percentage(0.996), 100);
    assert_eq!(percentage(0.125), 13);
    assert_eq!(percentage(0.004), 0);
    assert_eq!(percentage(0.005), 1);
    assert_eq!(percentage(f32::NAN), 0);
    assert_eq!(percentage(-1.0), 0);
}

#[test]
fn inference_time_is_shown_in_tenths_of_milliseconds() {
    assert_eq!(tenths_of_milliseconds(Duration::ZERO), 0);
    assert_eq!(tenths_of_milliseconds(Duration::from_micros(49)), 0);
    assert_eq!(tenths_of_milliseconds(Duration::from_micros(50)), 1);
    assert_eq!(tenths_of_milliseconds(Duration::from_micros(1_949)), 19);
    assert_eq!(tenths_of_milliseconds(Duration::from_micros(1_950)), 20);
    assert_eq!(tenths_of_milliseconds(Duration::from_millis(12)), 120);
    assert_eq!(
        tenths_of_milliseconds(Duration::from_secs(3600)),
        36_000_000
    );
}
