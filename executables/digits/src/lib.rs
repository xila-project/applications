#![no_std]

use pixel_painter::drawing::{HEIGHT, PIXELS, WIDTH};

pub const SIDE: usize = 28;
pub const INPUT_LEN: usize = SIDE * SIDE;

/// Default runtime loader node path for neural network models.
pub const LOADER_PATH: &str = "/devices/nn/cpu";

/// Legacy device path for Platforms boot-time ROM mount.
///
/// Kept for backward compatibility with Platforms until it switches to runtime loading.
pub const DEVICE_PATH: &str = "/devices/nn/digits";

/// The TensorFlow Lite model the guest loads at runtime through [`LOADER_PATH`].
pub const MODEL_BYTES: &[u8] = include_bytes!("../models/mnist.tflite");

/// Crop ink, preserve its aspect ratio in a 20×20 box, center in MNIST's 28×28
/// frame, and area-average to retain thin strokes. Palette zero is background.
pub fn prepare(pixels: &[u8]) -> Option<[f32; INPUT_LEN]> {
    if pixels.len() != PIXELS {
        return None;
    }
    let (mut left, mut right, mut top, mut bottom) = (WIDTH, 0, HEIGHT, 0);
    for (index, &pixel) in pixels.iter().enumerate() {
        if pixel != 0 {
            left = left.min(index % WIDTH);
            right = right.max(index % WIDTH);
            top = top.min(index / WIDTH);
            bottom = bottom.max(index / WIDTH);
        }
    }
    if left == WIDTH {
        return None;
    }
    let (width, height) = (right - left + 1, bottom - top + 1);
    let largest = width.max(height);
    let (out_width, out_height) = (
        (width * 20 / largest).max(1),
        (height * 20 / largest).max(1),
    );
    let (offset_x, offset_y) = ((SIDE - out_width) / 2, (SIDE - out_height) / 2);
    let mut output = [0.0; INPUT_LEN];
    // Integer area overlap avoids floating-coordinate edge errors.
    for oy in 0..out_height {
        for ox in 0..out_width {
            let mut ink = 0usize;
            for sy in oy * height / out_height..((oy + 1) * height).div_ceil(out_height) {
                let overlap_y = ((oy + 1) * height).min((sy + 1) * out_height)
                    - (oy * height).max(sy * out_height);
                for sx in ox * width / out_width..((ox + 1) * width).div_ceil(out_width) {
                    if pixels[(top + sy) * WIDTH + left + sx] != 0 {
                        let overlap_x = ((ox + 1) * width).min((sx + 1) * out_width)
                            - (ox * width).max(sx * out_width);
                        ink += overlap_x * overlap_y;
                    }
                }
            }
            output[(offset_y + oy) * SIDE + offset_x + ox] = ink as f32 / (width * height) as f32;
        }
    }
    Some(output)
}

/// Model outputs are softmax probabilities, not logits. Reject corrupt output.
pub fn top_three(scores: [f32; 10]) -> Option<[(u8, f32); 3]> {
    if scores
        .iter()
        .any(|p| !p.is_finite() || !(0.0..=1.0).contains(p))
        || (scores.iter().sum::<f32>() - 1.0).abs() > 0.02
    {
        return None;
    }
    let mut ranked = core::array::from_fn::<_, 10, _>(|i| (i as u8, scores[i]));
    ranked.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    Some([ranked[0], ranked[1], ranked[2]])
}

/// Legacy aligned model wrapper for Platforms boot-time ROM mount.
///
/// Kept for backward compatibility with Platforms until it switches to runtime loading.
#[cfg(feature = "model")]
#[repr(align(16))]
pub struct AlignedModel(pub [u8; include_bytes!("../models/mnist.tflite").len()]);

/// Legacy static model instance for Platforms boot-time ROM mount.
///
/// Kept for backward compatibility with Platforms until it switches to runtime loading.
#[cfg(feature = "model")]
pub static MODEL: AlignedModel = AlignedModel(*include_bytes!("../models/mnist.tflite"));
