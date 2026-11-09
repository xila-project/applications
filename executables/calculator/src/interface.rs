use crate::engine::evaluate_expression;
use crate::keypad::{
    ButtonIdentifier, CLEAR_LABEL, COMPACT_BASIC, COMPACT_SCIENTIFIC, Key, Layout, WIDE_BASIC,
    WIDE_SCIENTIFIC, cells, columns, variant,
};
use embedded_i18n::t;
use std::{ffi::CString, ptr::null_mut, thread::sleep};
use wasm::*;

/// Windows narrower or shorter than this show one pad at a time.
const WIDE_MINIMUM_WIDTH: i32 = 560;
const WIDE_MINIMUM_HEIGHT: i32 = 360;

const FONT_SMALL: u32 = 0;
const FONT_LARGE: u32 = 2;
const FONT_EXTRA_LARGE: u32 = 3;

/// `LV_LAYOUT_GRID`.
const LAYOUT_GRID: u32 = 2;
/// Gap between keys; pages also use it as outer padding, so the gap between
/// the wide layout's two pages is doubled and the groups read separately.
const KEY_GAP: i32 = 4;

/// Keypad pages, created on first use and then only shown or hidden, because
/// guest object handles are not recycled. Function pages sit on a muted panel
/// so their quiet ghost keys still read as one block.
const PAGES: [(Layout, bool); 4] = [
    (WIDE_SCIENTIFIC, true),
    (WIDE_BASIC, false),
    (COMPACT_BASIC, false),
    (COMPACT_SCIENTIFIC, true),
];
const SURFACE_MUTED: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrangement {
    Wide,
    CompactBasic,
    CompactScientific,
}

pub struct Interface {
    window: *mut Object,
    history_label: *mut Object,
    display_label: *mut Object,
    mode: *mut Object,
    keypad: *mut Object,
    /// Indexed like [`PAGES`].
    pages: [Option<*mut Object>; 4],
    keys: Vec<(*mut Object, Key)>,
    arrangement: Option<Arrangement>,
    current_expression: String,
    show_result: bool,
    is_radian_mode: bool,
}

/// Makes a plain layout object invisible so only its children are drawn.
unsafe fn make_transparent(object: *mut Object) -> wasm::Result<()> {
    unsafe {
        object_set_background(object, 0, 0, 0)?;
        object_set_border(object, 0, 0, 0, 0)?;
        object_set_padding(object, 0)?;
        object_remove_flag(object, ObjectFlag::Scrollable)
    }
}

impl Interface {
    pub fn new() -> wasm::Result<Self> {
        unsafe {
            let window = window_create()?;
            window_set_icon(
                window,
                c"Ca".as_ptr(),
                Color {
                    red: 0,
                    green: 0,
                    blue: 0,
                },
            )?;
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_set_padding(window, 8)?;
            object_remove_flag(window, ObjectFlag::Scrollable)?;

            // Display: angle mode and the previous expression on top, result below.
            let display = card_create(window)?;
            object_set_width(display, percentage(100)?)?;
            object_set_height(display, size_content())?;
            object_set_flex_flow(display, FlexFlow::RowWrap)?;
            object_set_flex_align(
                display,
                FlexAlign::End,
                FlexAlign::Center,
                FlexAlign::Center,
            )?;
            object_set_padding(display, 8)?;
            object_remove_flag(display, ObjectFlag::Scrollable)?;

            // Angle unit: a binary choice, so a labelled switch (off = radians).
            let mode_row = object_create(display)?;
            make_transparent(mode_row)?;
            // Padding also sets the flex gap between the label and the switch.
            object_set_padding(mode_row, 6)?;
            object_set_size(mode_row, size_content(), size_content())?;
            object_set_flex_flow(mode_row, FlexFlow::Row)?;
            object_set_flex_align(
                mode_row,
                FlexAlign::Start,
                FlexAlign::Center,
                FlexAlign::Center,
            )?;
            let mode_label = label_create(mode_row)?;
            let degrees = CString::new(
                t!("angle_mode.degrees_indicator")
                    .trim_matches(|c: char| c == '[' || c == ']' || c.is_whitespace()),
            )
            .map_err(|_| -1)?;
            label_set_text(mode_label, degrees.as_ptr())?;
            let mode = switch_create(mode_row)?;

            let history_label = label_create(display)?;
            object_set_flex_grow(history_label, 1)?;
            object_set_font(history_label, FONT_SMALL, 0)?;
            object_set_text_align(history_label, TextAlign::Right as u32, 0)?;
            label_set_text(history_label, c"".as_ptr())?;

            let display_label = label_create(display)?;
            object_set_text_align(display_label, TextAlign::Right as u32, 0)?;

            // Keypad: one or two key grids sharing the remaining height.
            let keypad = object_create(window)?;
            make_transparent(keypad)?;
            object_set_width(keypad, percentage(100)?)?;
            object_set_flex_grow(keypad, 1)?;
            object_set_flex_flow(keypad, FlexFlow::Row)?;

            // Keeps the last key row clear of the shell's home handle; the
            // window's flex gap supplies most of the inset.
            let inset = object_create(window)?;
            make_transparent(inset)?;
            object_set_size(inset, percentage(100)?, 2)?;

            let mut interface = Self {
                window,
                history_label,
                display_label,
                mode,
                keypad,
                pages: [None; 4],
                keys: Vec::new(),
                arrangement: None,
                current_expression: String::new(),
                show_result: false,
                is_radian_mode: true,
            };
            interface.resize()?;
            Ok(interface)
        }
    }

    unsafe fn create_page(&mut self, layout: Layout, panel: bool) -> wasm::Result<*mut Object> {
        unsafe {
            let page = object_create(self.keypad)?;
            if panel {
                object_set_surface(page, SURFACE_MUTED)?;
                object_set_border(page, 0, 0, 0, 0)?;
                object_remove_flag(page, ObjectFlag::Scrollable)?;
            } else {
                make_transparent(page)?;
            }
            object_set_height(page, percentage(100)?)?;
            object_set_width(page, 0)?;
            object_set_flex_grow(page, columns(layout) as u8)?;
            object_set_padding(page, KEY_GAP)?;
            object_set_layout(page, LAYOUT_GRID)?;
            let fraction = grid_fr(1)?;
            let mut tracks = vec![fraction; columns(layout)];
            tracks.push(COORD_MAX); // LV_GRID_TEMPLATE_LAST
            let mut rows = vec![fraction; layout.len()];
            rows.push(COORD_MAX);
            object_set_grid_dsc_array(page, tracks.as_ptr(), rows.as_ptr())?;

            // Translated "Clear" words outgrow the ~56 px keys of five-column
            // pages on 320 px screens; those keep the universal "C" label.
            let clear = if columns(layout) < 5 {
                t!("controls.clear")
            } else {
                CLEAR_LABEL
            };
            for (column, span, row, label, key) in cells(layout) {
                let button = button_create(page)?;
                object_set_theme_variant(button, variant(key) as u32)?;
                // Rows share the height instead of the theme's minimum touch
                // height, which would push the last rows off a 240 px screen.
                object_set_min_max_size(button, 0, 16_384, 0, 16_384, 0)?;
                object_set_padding(button, 0)?;
                object_set_grid_cell(
                    button,
                    GridAlign::Stretch,
                    column as i32,
                    span as i32,
                    GridAlign::Stretch,
                    row as i32,
                    1,
                )?;
                let text = if key == Key::Action(ButtonIdentifier::Clear) {
                    clear
                } else {
                    label
                };
                let text = CString::new(text).map_err(|_| -1)?;
                let label = label_create(button)?;
                label_set_text(label, text.as_ptr())?;
                object_align(label, Align::Center, 0, 0)?;
                self.keys.push((button, key));
            }
            Ok(page)
        }
    }

    unsafe fn arrange(&mut self, arrangement: Arrangement) -> wasm::Result<()> {
        if self.arrangement == Some(arrangement) {
            return Ok(());
        }
        let was_wide = self.arrangement.map(|a| a == Arrangement::Wide);
        let wide = arrangement == Arrangement::Wide;
        let visible: &[usize] = match arrangement {
            Arrangement::Wide => &[0, 1],
            Arrangement::CompactBasic => &[2],
            Arrangement::CompactScientific => &[3],
        };
        unsafe {
            for (index, (layout, panel)) in PAGES.into_iter().enumerate() {
                match (self.pages[index], visible.contains(&index)) {
                    (None, true) => self.pages[index] = Some(self.create_page(layout, panel)?),
                    (Some(page), true) => object_remove_flag(page, ObjectFlag::Hidden)?,
                    (Some(page), false) => object_add_flag(page, ObjectFlag::Hidden)?,
                    (None, false) => {}
                }
            }
            if was_wide != Some(wide) {
                if wide {
                    object_remove_flag(self.history_label, ObjectFlag::Hidden)?;
                    // The result gets its own full-width line under mode and history.
                    object_set_flex_grow(self.display_label, 0)?;
                    object_set_width(self.display_label, percentage(100)?)?;
                    object_set_font(self.display_label, FONT_EXTRA_LARGE, 0)?;
                } else {
                    object_add_flag(self.history_label, ObjectFlag::Hidden)?;
                    // One line: mode selector on the left, result filling the rest.
                    object_set_width(self.display_label, 0)?;
                    object_set_flex_grow(self.display_label, 1)?;
                    object_set_font(self.display_label, FONT_LARGE, 0)?;
                }
            }
        }
        self.arrangement = Some(arrangement);
        Ok(())
    }

    unsafe fn resize(&mut self) -> wasm::Result<()> {
        unsafe {
            object_update_layout(self.window)?;
            let wide = object_get_width(self.window)? >= WIDE_MINIMUM_WIDTH
                && object_get_height(self.window)? >= WIDE_MINIMUM_HEIGHT;
            let arrangement = match (wide, self.arrangement) {
                (true, _) => Arrangement::Wide,
                (false, Some(Arrangement::CompactScientific)) => Arrangement::CompactScientific,
                (false, _) => Arrangement::CompactBasic,
            };
            self.arrange(arrangement)
        }
    }

    fn handle_key(&mut self, key: Key) -> wasm::Result<()> {
        match key {
            Key::Action(action) => self.handle_button_press(action),
            Key::ShowScientific => unsafe { self.arrange(Arrangement::CompactScientific) },
            Key::ShowBasic => unsafe { self.arrange(Arrangement::CompactBasic) },
        }
    }

    fn update_display(&mut self) -> wasm::Result<()> {
        let text = CString::new(if self.current_expression.is_empty() {
            "0"
        } else {
            &self.current_expression
        })
        .map_err(|_| -1)?;
        unsafe { label_set_text(self.display_label, text.as_ptr()) }
    }

    fn set_history(&mut self, text: &str) -> wasm::Result<()> {
        let text = CString::new(text).map_err(|_| -1)?;
        unsafe { label_set_text(self.history_label, text.as_ptr()) }
    }

    fn handle_button_press(&mut self, button: ButtonIdentifier) -> wasm::Result<()> {
        match button {
            ButtonIdentifier::Clear => {
                self.current_expression.clear();
                self.show_result = false;
                self.set_history("")?;
            }
            ButtonIdentifier::Backspace => {
                if !self.current_expression.is_empty() {
                    self.current_expression.pop();
                }
                self.show_result = false;
            }
            ButtonIdentifier::Equals => {
                if !self.current_expression.is_empty() {
                    let expression = std::mem::take(&mut self.current_expression);
                    match evaluate_expression(&expression, self.is_radian_mode) {
                        Ok(result) => {
                            self.set_history(&format!("{expression} ="))?;
                            self.current_expression = result.to_string();
                        }
                        Err(e) => {
                            self.set_history(&expression)?;
                            self.current_expression =
                                format!("{}", t!("errors.calculation_with_details", arg0 = e));
                        }
                    }
                    self.show_result = true;
                }
            }
            // Basic trigonometric functions
            ButtonIdentifier::Sin => self.push_str("sin("),
            ButtonIdentifier::Cos => self.push_str("cos("),
            ButtonIdentifier::Tan => self.push_str("tan("),

            // Hyperbolic functions
            ButtonIdentifier::Sinh => self.push_str("sinh("),
            ButtonIdentifier::Cosh => self.push_str("cosh("),
            ButtonIdentifier::Tanh => self.push_str("tanh("),

            // Logarithmic functions
            ButtonIdentifier::Ln => self.push_str("ln("),
            ButtonIdentifier::Log10 => self.push_str("log("),

            // Root and power functions
            ButtonIdentifier::Sqrt => self.push_str("sqrt("),
            ButtonIdentifier::Square => self.push_str("sqr("),
            ButtonIdentifier::Cube => self.push_str("cube("),
            ButtonIdentifier::TenPower => self.push_str("pow10("),

            // Other functions
            ButtonIdentifier::Factorial => self.push_char('!'),
            ButtonIdentifier::Inverse => self.push_str("inv("),
            ButtonIdentifier::Abs => self.push_str("abs("),
            ButtonIdentifier::Random => {
                // Generate random number immediately and insert it
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};

                let mut hasher = DefaultHasher::new();
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
                    .hash(&mut hasher);
                let hash = hasher.finish();
                let random_value = (hash as f64) / (u64::MAX as f64);
                self.push_str(&format!("{:.10}", random_value));
            }

            // Constants
            ButtonIdentifier::Pi => self.push_str("pi"),
            ButtonIdentifier::E => self.push_str("e"),

            // Parentheses and operators
            ButtonIdentifier::LeftParenthesis => self.push_str("("),
            ButtonIdentifier::RightParenthesis => self.push_char(')'),
            ButtonIdentifier::Decimal => self.push_str("."),

            // Operator buttons that continue the expression
            ButtonIdentifier::Power => self.push_operator('^'),
            ButtonIdentifier::Divide => self.push_operator('/'),
            ButtonIdentifier::Multiply => self.push_operator('*'),
            ButtonIdentifier::Subtract => self.push_operator('-'),
            ButtonIdentifier::Add => self.push_operator('+'),
            ButtonIdentifier::Percent => self.push_operator('%'),
            ButtonIdentifier::Mod => self.push_str(" mod "),

            // Number buttons
            ButtonIdentifier::Zero => self.push_char('0'),
            ButtonIdentifier::One => self.push_char('1'),
            ButtonIdentifier::Two => self.push_char('2'),
            ButtonIdentifier::Three => self.push_char('3'),
            ButtonIdentifier::Four => self.push_char('4'),
            ButtonIdentifier::Five => self.push_char('5'),
            ButtonIdentifier::Six => self.push_char('6'),
            ButtonIdentifier::Seven => self.push_char('7'),
            ButtonIdentifier::Eight => self.push_char('8'),
            ButtonIdentifier::Nine => self.push_char('9'),
        }

        self.update_display()
    }

    fn push_str(&mut self, text: &str) {
        if self.show_result {
            self.current_expression.clear();
            self.show_result = false;
        }
        self.current_expression.push_str(text);
    }

    fn push_char(&mut self, ch: char) {
        if self.show_result {
            self.current_expression.clear();
            self.show_result = false;
        }
        self.current_expression.push(ch);
    }

    fn push_operator(&mut self, op: char) {
        // For operators, if showing result, keep it and append the operator
        if self.show_result {
            self.show_result = false;
        }
        self.current_expression.push(op);
    }

    pub unsafe fn run(&mut self) -> wasm::Result<()> {
        self.update_display()?;
        loop {
            let mut code = EventCode::All as u32;
            let mut target: *mut Object = null_mut();
            unsafe {
                window_pop_event(
                    self.window,
                    &mut code,
                    (&mut target as *mut *mut Object).cast(),
                )?;
            }
            if code == EVENT_CLOSE_REQUESTED {
                return Ok(());
            }
            if code == EventCode::SizeChanged as u32 && target == self.window {
                unsafe { self.resize()? };
            } else if code == EventCode::ValueChanged as u32 && target == self.mode {
                let state = unsafe { object_get_state(self.mode)? };
                self.is_radian_mode = state & STATE_CHECKED == 0;
            } else if code == EventCode::Clicked as u32 {
                if let Some(&(_, key)) = self.keys.iter().find(|(button, _)| *button == target) {
                    self.handle_key(key)?;
                }
            } else if code == EventCode::All as u32 {
                sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}
