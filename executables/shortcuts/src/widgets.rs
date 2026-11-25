//! Small helpers over the guest graphics calls.

use std::ffi::CString;
use wasm::*;

/// shadcn button variants, numbered as `lv_theme_shadcn_variant_t`.
pub const PRIMARY: u32 = 0;
pub const OUTLINE: u32 = 2;

pub const FONT_SMALL: u32 = 0;
pub const FONT_MEDIUM: u32 = 1;
pub const FONT_LARGE: u32 = 2;

/// LVGL icon font glyphs for the media controls.
pub const SYMBOL_PLAY_PAUSE: &str = "\u{F04C}";
pub const SYMBOL_PREVIOUS: &str = "\u{F048}";
pub const SYMBOL_NEXT: &str = "\u{F051}";
pub const SYMBOL_STOP: &str = "\u{F04D}";
pub const SYMBOL_VOLUME_DOWN: &str = "\u{F027}";
pub const SYMBOL_VOLUME_UP: &str = "\u{F028}";
pub const SYMBOL_MUTE: &str = "\u{F026}";

/// Plain container without the card styling `object_create` gets from the theme.
pub unsafe fn container(parent: *mut Object) -> wasm::Result<*mut Object> {
    unsafe {
        let object = object_create(parent)?;
        object_set_background(object, 0, 0, 0)?;
        object_set_border(object, 0, 0, 0, 0)?;
        object_set_padding(object, 0)?;
        object_remove_flag(object, ObjectFlag::Scrollable)?;
        Ok(object)
    }
}

/// Full-width container laying its children out in a row or a column.
pub unsafe fn flex(parent: *mut Object, flow: FlexFlow) -> wasm::Result<*mut Object> {
    unsafe {
        let object = container(parent)?;
        object_set_size(object, percentage(100)?, size_content())?;
        object_set_flex_flow(object, flow)?;
        object_set_flex_align(
            object,
            FlexAlign::Start,
            FlexAlign::Center,
            FlexAlign::Center,
        )?;
        Ok(object)
    }
}

pub unsafe fn label(parent: *mut Object, text: &str, font: u32) -> wasm::Result<*mut Object> {
    unsafe {
        let label = label_create(parent)?;
        object_set_font(label, font, 0)?;
        set_text(label, text)?;
        Ok(label)
    }
}

/// A label that takes the whole width and wraps.
pub unsafe fn paragraph(parent: *mut Object, text: &str, font: u32) -> wasm::Result<*mut Object> {
    unsafe {
        let label = label(parent, text, font)?;
        object_set_width(label, percentage(100)?)?;
        Ok(label)
    }
}

/// `text` as a C string. A NUL character in a text cannot cross the C
/// boundary: it is dropped instead of failing the whole screen.
fn c_string(text: &str) -> wasm::Result<CString> {
    CString::new(text.replace('\0', "")).map_err(|_| -1)
}

pub unsafe fn set_text(label: *mut Object, text: &str) -> wasm::Result<()> {
    let text = c_string(text)?;
    unsafe { label_set_text(label, text.as_ptr()) }
}

pub unsafe fn set_hidden(object: *mut Object, hidden: bool) -> wasm::Result<()> {
    unsafe {
        if hidden {
            object_add_flag(object, ObjectFlag::Hidden)
        } else {
            object_remove_flag(object, ObjectFlag::Hidden)
        }
    }
}

/// A button and its caption label.
pub unsafe fn labelled_button(
    parent: *mut Object,
    text: &str,
    variant: u32,
) -> wasm::Result<(*mut Object, *mut Object)> {
    unsafe {
        let button = button_create(parent)?;
        object_set_theme_variant(button, variant)?;
        let caption = label_create(button)?;
        set_text(caption, text)?;
        object_align(caption, Align::Center, 0, 0)?;
        Ok((button, caption))
    }
}

pub unsafe fn delete(object: *mut Object) -> wasm::Result<()> {
    unsafe { object_delete(u32::try_from(object as usize).map_err(|_| -1)?) }
}
