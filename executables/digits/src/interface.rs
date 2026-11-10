use crate::device::Classifier;
use embedded_i18n::t;
use pixel_painter::drawing::{Drawing, HEIGHT, Viewport, WIDTH};
use std::{ffi::CString, ptr::null_mut, thread::sleep, time::Duration};
use wasm::*;

struct Interface {
    window: *mut Object,
    canvas: *mut Object,
    actions: *mut Object,
    result: *mut Object,
    drawing: Drawing,
    classifier: Option<Classifier>,
    // White ink on black, matching the tensor preview and MNIST polarity.
    palette: [u32; 2],
    _actions: [*const i8; 4],
}

pub fn run() -> wasm::Result<()> {
    let mut ui = unsafe { Interface::new()? };
    loop {
        let (mut repaint, mut predict) = (false, false);
        for _ in 0..32 {
            let mut code = 0;
            let mut target: *mut Object = null_mut();
            let (mut x, mut y, mut width, mut height) = (0, 0, 0, 0);
            unsafe {
                window_pop_pointer_event(
                    ui.window,
                    &mut code,
                    (&mut target as *mut *mut Object).cast(),
                    &mut x,
                    &mut y,
                    &mut width,
                    &mut height,
                )?;
            }
            if code == EventCode::All as u32 {
                break;
            }
            if code == EVENT_CLOSE_REQUESTED {
                unsafe {
                    object_delete(u16::try_from(ui.window as usize).map_err(|_| -1)?)?;
                }
                return Ok(());
            }
            if target == ui.canvas {
                if code == EventCode::Pressed as u32 || code == EventCode::Pressing as u32 {
                    if let Some((x, y)) = Viewport::new(0, 0, width, height).map(x, y) {
                        if code == EventCode::Pressed as u32 {
                            ui.drawing.begin_stroke(x, y, 1, 5);
                        } else {
                            ui.drawing.continue_stroke(x, y, 1, 5);
                        }
                        repaint = true;
                        ui.status(t!("status.drawing"))?;
                    } else {
                        ui.drawing.end_stroke();
                    }
                } else if code == EventCode::Released as u32 || code == EventCode::PressLost as u32
                {
                    ui.drawing.end_stroke();
                    predict = true;
                }
            } else if target == ui.actions && code == EventCode::Clicked as u32 {
                ui.drawing.end_stroke();
                match unsafe { buttonmatrix_get_selected_button(ui.actions)? } {
                    0 => {
                        predict = true;
                    }
                    1 => {
                        ui.drawing.undo();
                        repaint = true;
                        predict = true;
                    }
                    2 => {
                        ui.drawing.clear();
                        repaint = true;
                        predict = true;
                    }
                    _ => {}
                }
            }
        }
        if repaint {
            ui.upload()?;
        }
        if predict {
            ui.predict()?;
        }
        sleep(Duration::from_millis(16));
    }
}

impl Interface {
    unsafe fn new() -> wasm::Result<Self> {
        unsafe {
            let window = window_create()?;
            window_set_icon(
                window,
                c"09".as_ptr(),
                Color {
                    red: 64,
                    green: 160,
                    blue: 220,
                },
            )?;
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_set_padding(window, 4)?;
            let title = label_create(window)?;
            label_set_text(title, t!(c"app.title").as_ptr().cast_mut())?;
            let actions = buttonmatrix_create(window)?;
            object_set_width(actions, percentage(100)?)?;
            object_set_height(actions, 38)?;
            let action_map = [
                t!(c"controls.recognize").as_ptr(),
                t!(c"controls.undo").as_ptr(),
                t!(c"controls.clear").as_ptr(),
                c"".as_ptr(),
            ];
            buttonmatrix_set_map(actions, action_map.as_ptr())?;
            let canvas = indexed_canvas_create(window, WIDTH as u16, HEIGHT as u16)?;
            object_set_width(canvas, percentage(100)?)?;
            object_set_flex_grow(canvas, 1)?;
            let result = label_create(window)?;
            object_set_width(result, percentage(100)?)?;
            for object in [canvas, actions] {
                object_add_flag(object, ObjectFlag::EventBubble)?;
            }
            let ui = Self {
                window,
                canvas,
                actions,
                result,
                drawing: Drawing::new(),
                classifier: Classifier::open().ok(),
                palette: [0x101216, 0xffffff],
                _actions: action_map,
            };
            ui.upload()?;
            ui.status(if ui.classifier.is_some() {
                t!("status.ready")
            } else {
                t!("errors.unavailable")
            })?;
            Ok(ui)
        }
    }

    fn status(&self, text: &str) -> wasm::Result<()> {
        let text = CString::new(text).map_err(|_| -1)?;
        unsafe { label_set_text(self.result, text.as_ptr().cast_mut()) }
    }

    fn upload(&self) -> wasm::Result<()> {
        unsafe {
            indexed_canvas_set_pixels(
                self.canvas,
                self.drawing.pixels().as_ptr(),
                self.drawing.pixels().len(),
                self.palette.as_ptr(),
                self.palette.len(),
            )
        }
    }

    fn predict(&mut self) -> wasm::Result<()> {
        let Some(input) = digits::prepare(self.drawing.pixels()) else {
            return self.status(t!("status.ready"));
        };
        if self.classifier.is_none() {
            self.classifier = Classifier::open().ok();
        }
        let Some(classifier) = self.classifier.as_mut() else {
            return self.status(t!("errors.unavailable"));
        };
        match classifier.predict(&input) {
            Ok(scores) => self.status(&format!(
                "{}: {} ({:.0}%)\n{}: {} {:.0}%   {} {:.0}%   {} {:.0}%",
                t!("results.digit"),
                scores[0].0,
                scores[0].1 * 100.0,
                t!("results.scores"),
                scores[0].0,
                scores[0].1 * 100.0,
                scores[1].0,
                scores[1].1 * 100.0,
                scores[2].0,
                scores[2].1 * 100.0
            )),
            Err(error) => {
                eprintln!("Digits inference: {error}");
                self.classifier = None;
                self.status(t!("errors.inference"))
            }
        }
    }
}
