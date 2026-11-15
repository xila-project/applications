use crate::storage;
use embedded_i18n::t;
use pixel_painter::drawing::{Drawing, HEIGHT, PALETTE, Viewport, WIDTH};
use std::{
    ffi::{CStr, CString},
    ptr::null_mut,
    thread::sleep,
    time::Duration,
};
use wasm::*;

struct Interface {
    window: *mut Object,
    canvas: *mut Object,
    swatches: Vec<*mut Object>,
    controls: Vec<(*mut Object, u32)>,
    // Compact views are created once; hiding them preserves widget handles.
    pages: [*mut Object; 3],
    navigation: Vec<*mut Object>,
    slider: *mut Object,
    brush_label: *mut Object,
    path: *mut Object,
    keyboard: *mut Object,
    status: *mut Object,
    drawing: Drawing,
    color: u8,
    brush: u8,
    revision: u64,
    confirm_open: Option<u64>,
    confirm_close: Option<u64>,
}

pub fn run() -> wasm::Result<()> {
    let mut ui = unsafe { Interface::new()? };
    loop {
        let mut repaint = false;
        for _ in 0..32 {
            let mut code = 0u32;
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
                if !ui.drawing.is_dirty() || ui.confirm_close == Some(ui.revision) {
                    unsafe {
                        object_delete(u32::try_from(ui.window as usize).map_err(|_| -1)?)?;
                    }
                    return Ok(());
                }
                ui.status(t!("warnings.unsaved_close_again"))?;
                ui.confirm_close = Some(ui.revision);
            } else if target == ui.canvas {
                if code == EventCode::Pressed as u32 || code == EventCode::Pressing as u32 {
                    if let Some((x, y)) = Viewport::new(0, 0, width, height).map(x, y) {
                        if code == EventCode::Pressed as u32 {
                            ui.drawing.begin_stroke(x, y, ui.color, ui.brush);
                        } else {
                            ui.drawing.continue_stroke(x, y, ui.color, ui.brush);
                        }
                        ui.revision = ui.revision.wrapping_add(1);
                        repaint = true;
                    } else {
                        ui.drawing.end_stroke();
                    }
                } else if code == EventCode::Released as u32 || code == EventCode::PressLost as u32
                {
                    ui.drawing.end_stroke();
                }
            } else if code == EventCode::ValueChanged as u32 && target == ui.slider {
                // The drawing engine uses odd, centered square brushes.
                ui.brush = (unsafe { range_get_value(ui.slider)? } as u8) * 2 - 1;
                let text = CString::new(format!("{}: {} px", t!("controls.brush"), ui.brush))
                    .map_err(|_| -1)?;
                unsafe { label_set_text(ui.brush_label, text.as_ptr())? };
            } else if code == EventCode::Clicked as u32 {
                if let Some(index) = ui.swatches.iter().position(|&button| button == target) {
                    ui.color = index as u8;
                    ui.drawing.end_stroke();
                    unsafe { ui.select_color()? };
                }
                if let Some((_, action)) = ui.controls.iter().find(|(button, _)| *button == target)
                {
                    let action = *action;
                    ui.drawing.end_stroke();
                    match action {
                        0 => {
                            ui.drawing.undo();
                            ui.revision = ui.revision.wrapping_add(1);
                            repaint = true;
                        }
                        1 => {
                            ui.drawing.clear();
                            ui.revision = ui.revision.wrapping_add(1);
                            repaint = true;
                        }
                        2 | 3 => repaint |= ui.file_action(action == 3)?,
                        10..=12 => unsafe { ui.show_page((action - 10) as usize)? },
                        _ => (),
                    }
                }
            } else if code == EventCode::Focused as u32 && target == ui.path {
                unsafe {
                    keyboard_set_textarea(ui.keyboard, ui.path)?;
                    object_remove_flag(ui.keyboard, ObjectFlag::Hidden)?;
                }
            } else if (code == EventCode::Ready as u32 || code == EventCode::Cancel as u32)
                && target == ui.keyboard
            {
                unsafe {
                    object_add_flag(ui.keyboard, ObjectFlag::Hidden)?;
                }
            }
        }
        if repaint {
            ui.upload()?;
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
                c"Pa".as_ptr(),
                Color {
                    red: 142,
                    green: 36,
                    blue: 170,
                },
            )?;
            object_update_layout(window)?;
            let compact = object_get_width(window)? < 560 || object_get_height(window)? < 360;
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_set_padding(window, if compact { 4 } else { 12 })?;
            object_remove_flag(window, ObjectFlag::Scrollable)?;
            object_set_font(window, 1, 0)?;
            let header = Self::container(window, FlexFlow::Row, 4)?;
            object_set_height(header, 40)?;
            let mut controls = Vec::new();
            let mut navigation = Vec::new();
            if compact {
                for (index, text) in [
                    t!(c"controls.paint"),
                    t!(c"controls.tools"),
                    t!(c"controls.file"),
                ]
                .into_iter()
                .enumerate()
                {
                    let button = Self::button(header, text, 2)?;
                    object_set_flex_grow(button, 1)?;
                    controls.push((button, 10 + index as u32));
                    navigation.push(button);
                }
            } else {
                let title = Self::label(header, t!(c"app.title"))?;
                object_set_font(title, 2, 0)?;
                object_set_flex_grow(title, 1)?;
            }
            let undo = Self::button(header, t!(c"controls.undo"), 2)?;
            controls.push((undo, 0));

            let body = Self::container(
                window,
                if compact {
                    FlexFlow::Column
                } else {
                    FlexFlow::Row
                },
                0,
            )?;
            object_set_flex_grow(body, 1)?;
            if !compact {
                object_set_padding(body, 8)?;
            }
            let panel = if compact {
                Self::container(body, FlexFlow::Column, 0)?
            } else {
                let card = card_create(body)?;
                object_set_flex_flow(card, FlexFlow::Column)?;
                object_remove_flag(card, ObjectFlag::Scrollable)?;
                card
            };
            if compact {
                object_set_height(panel, percentage(100)?)?;
                object_add_flag(panel, ObjectFlag::Hidden)?;
            } else {
                object_set_width(panel, 184)?;
                object_set_height(panel, percentage(100)?)?;
                object_set_padding(panel, 8)?;
            }
            let tools = Self::container(panel, FlexFlow::Column, 4)?;
            object_set_height(tools, size_content())?;
            let palette = Self::container(tools, FlexFlow::Row, 4)?;
            object_set_height(palette, if compact { 38 } else { 80 })?;
            object_set_layout(palette, 2)?;
            let fr = grid_fr(1)?;
            let mut columns = vec![fr; if compact { 8 } else { 4 }];
            columns.push(COORD_MAX);
            let rows = if compact {
                vec![fr, COORD_MAX]
            } else {
                vec![fr, fr, COORD_MAX]
            };
            object_set_grid_dsc_array(palette, columns.as_ptr(), rows.as_ptr())?;
            let mut swatches = Vec::new();
            for (index, &color) in PALETTE.iter().enumerate() {
                let button = button_create(palette)?;
                object_set_min_max_size(button, 0, 16384, 0, 16384, 0)?;
                object_set_padding(button, 5)?;
                let count = if compact { 8 } else { 4 };
                object_set_grid_cell(
                    button,
                    GridAlign::Stretch,
                    (index % count) as i32,
                    1,
                    GridAlign::Stretch,
                    (index / count) as i32,
                    1,
                )?;
                let chip = object_create(button)?;
                object_set_size(chip, percentage(100)?, percentage(100)?)?;
                object_set_background(chip, color, 255, 0)?;
                object_set_border(chip, 0, 0, 0, 0)?;
                object_set_padding(chip, 0)?;
                object_remove_flag(chip, ObjectFlag::Clickable)?;
                object_remove_flag(chip, ObjectFlag::Scrollable)?;
                swatches.push(button);
            }
            let brush_row = Self::container(tools, FlexFlow::Row, 8)?;
            object_set_height(brush_row, 32)?;
            let brush_label = Self::label(brush_row, c"")?;
            let slider = slider_create(brush_row)?;
            range_set_range(slider, 1, 3)?;
            range_set_value(slider, 1, false)?;
            object_set_flex_grow(slider, 1)?;
            object_set_height(slider, 8)?;
            object_set_flex_align(
                brush_row,
                FlexAlign::Start,
                FlexAlign::Center,
                FlexAlign::Center,
            )?;
            if !compact {
                object_set_flex_flow(brush_row, FlexFlow::Column)?;
                object_set_height(brush_row, 64)?;
                object_set_width(slider, percentage(85)?)?;
                object_set_flex_grow(slider, 0)?;
            }
            let clear = Self::button(tools, t!(c"controls.clear"), 4)?;
            object_set_width(clear, percentage(100)?)?;
            controls.push((clear, 1));
            let files = Self::container(if compact { body } else { panel }, FlexFlow::Column, 4)?;
            object_set_height(
                files,
                if compact {
                    percentage(100)?
                } else {
                    size_content()
                },
            )?;
            object_add_flag(files, ObjectFlag::Scrollable)?;
            if compact {
                object_add_flag(files, ObjectFlag::Hidden)?;
            }
            Self::label(files, t!(c"controls.file"))?;
            let path = textarea_create(files)?;
            textarea_set_one_line(path, true)?;
            textarea_set_max_length(path, 240)?;
            let initial = std::env::args()
                .nth(1)
                .or_else(storage::default_path)
                .unwrap_or_default();
            let initial = CString::new(initial).map_err(|_| -1)?;
            textarea_set_text(path, initial.as_ptr())?;
            object_set_width(path, percentage(100)?)?;
            object_set_height(path, 36)?;
            let file_actions = Self::container(files, FlexFlow::Row, 4)?;
            object_set_height(file_actions, 40)?;
            for (text, action, variant) in
                [(t!(c"controls.open"), 3, 2), (t!(c"controls.save"), 2, 0)]
            {
                let button = Self::button(file_actions, text, variant)?;
                object_set_flex_grow(button, 1)?;
                controls.push((button, action));
            }
            let canvas = indexed_canvas_create(body, WIDTH as u16, HEIGHT as u16)?;
            object_set_flex_grow(canvas, 1)?;
            if compact {
                object_set_width(canvas, percentage(100)?)?;
            } else {
                object_set_height(canvas, percentage(100)?)?;
                object_set_width(canvas, 0)?;
            }
            object_remove_flag(canvas, ObjectFlag::Scrollable)?;
            let status = Self::label(window, c"")?;
            object_set_width(status, percentage(100)?)?;
            let keyboard = keyboard_create(files)?;
            object_set_width(keyboard, percentage(100)?)?;
            object_set_height(keyboard, 90)?;
            object_add_flag(keyboard, ObjectFlag::Hidden)?;
            let mut ui = Self {
                window,
                canvas,
                swatches,
                controls,
                pages: if compact {
                    [canvas, panel, files]
                } else {
                    [null_mut(); 3]
                },
                navigation,
                slider,
                brush_label,
                path,
                keyboard,
                status,
                drawing: Drawing::new(),
                color: 1,
                brush: 1,
                revision: 0,
                confirm_open: None,
                confirm_close: None,
            };
            let text = CString::new(format!("{}: 1 px", t!("controls.brush"))).map_err(|_| -1)?;
            label_set_text(brush_label, text.as_ptr())?;
            ui.select_color()?;
            if compact {
                ui.show_page(0)?;
            }
            ui.status(t!("status.ready_white_erases"))?;
            ui.upload()?;
            Ok(ui)
        }
    }

    unsafe fn container(
        parent: *mut Object,
        flow: FlexFlow,
        padding: i32,
    ) -> wasm::Result<*mut Object> {
        unsafe {
            let object = object_create(parent)?;
            object_set_background(object, 0, 0, 0)?;
            object_set_border(object, 0, 0, 0, 0)?;
            object_set_padding(object, padding)?;
            object_set_width(object, percentage(100)?)?;
            object_set_flex_flow(object, flow)?;
            object_remove_flag(object, ObjectFlag::Scrollable)?;
            Ok(object)
        }
    }

    unsafe fn label(parent: *mut Object, text: &CStr) -> wasm::Result<*mut Object> {
        unsafe {
            let label = label_create(parent)?;
            label_set_text(label, text.as_ptr())?;
            Ok(label)
        }
    }

    unsafe fn button(parent: *mut Object, text: &CStr, variant: u32) -> wasm::Result<*mut Object> {
        unsafe {
            let button = button_create(parent)?;
            object_set_theme_variant(button, variant)?;
            object_set_min_max_size(button, 0, 16384, 0, 16384, 0)?;
            object_set_height(button, 32)?;
            object_set_padding(button, 4)?;
            let label = Self::label(button, text)?;
            object_align(label, Align::Center, 0, 0)?;
            Ok(button)
        }
    }

    unsafe fn select_color(&self) -> wasm::Result<()> {
        unsafe {
            for (index, &button) in self.swatches.iter().enumerate() {
                object_set_theme_variant(button, if index == self.color as usize { 0 } else { 2 })?;
            }
            Ok(())
        }
    }

    unsafe fn show_page(&mut self, selected: usize) -> wasm::Result<()> {
        unsafe {
            for (index, &page) in self.pages.iter().enumerate() {
                if selected == index {
                    object_remove_flag(page, ObjectFlag::Hidden)?;
                } else {
                    object_add_flag(page, ObjectFlag::Hidden)?;
                }
                object_set_theme_variant(
                    self.navigation[index],
                    if selected == index { 0 } else { 2 },
                )?;
            }
            object_add_flag(self.keyboard, ObjectFlag::Hidden)?;
            Ok(())
        }
    }

    fn status(&self, text: &str) -> wasm::Result<()> {
        let text = CString::new(text).map_err(|_| -1)?;
        unsafe { label_set_text(self.status, text.as_ptr().cast_mut()) }
    }

    fn upload(&self) -> wasm::Result<()> {
        unsafe {
            indexed_canvas_set_pixels(
                self.canvas,
                self.drawing.pixels().as_ptr(),
                self.drawing.pixels().len(),
                PALETTE.as_ptr(),
                PALETTE.len(),
            )
        }
    }

    fn file_action(&mut self, opening: bool) -> wasm::Result<bool> {
        if opening && self.drawing.is_dirty() && self.confirm_open != Some(self.revision) {
            self.confirm_open = Some(self.revision);
            self.status(t!("warnings.unsaved_open_again"))?;
            return Ok(false);
        }
        self.confirm_open = None;
        // LVGL limits characters, not UTF-8 bytes: allow four bytes per character.
        let mut bytes = [0i8; 240 * 4 + 1];
        unsafe {
            textarea_get_text(self.path, bytes.as_mut_ptr(), bytes.len())?;
        }
        let path = unsafe { CStr::from_ptr(bytes.as_ptr()) }
            .to_str()
            .unwrap_or("");
        if !path.starts_with('/') || path.ends_with('/') {
            self.status(t!("file_path.placeholder"))?;
            return Ok(false);
        }
        if opening {
            match storage::read(path) {
                Ok(bytes) if self.drawing.replace_from_bytes(&bytes).is_ok() => {
                    self.revision = self.revision.wrapping_add(1);
                    self.status(t!("status.opened"))?;
                    return Ok(true);
                }
                Ok(_) => self.status(t!("errors.invalid_file_drawing_preserved"))?,
                Err(_) => self.status(t!("errors.open_failed"))?,
            }
        } else if storage::write(path, &self.drawing.encode()).is_ok() {
            self.drawing.mark_saved();
            self.status(t!("status.saved"))?;
        } else {
            self.status(t!("errors.save_failed_drawing_preserved"))?;
        }
        Ok(false)
    }
}
