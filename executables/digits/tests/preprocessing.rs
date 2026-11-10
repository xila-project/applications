use digits::{prepare, top_three};
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
