//! Small helpers over the guest graphics calls.

use std::ffi::{CStr, CString};
use wasm::*;

/// shadcn button variants, numbered as `lv_theme_shadcn_variant_t`.
pub const PRIMARY: u32 = 0;
pub const OUTLINE: u32 = 2;
pub const GHOST: u32 = 3;
pub const DESTRUCTIVE: u32 = 4;

pub const FONT_SMALL: u32 = 0;
pub const FONT_MEDIUM: u32 = 1;
pub const FONT_LARGE: u32 = 2;
pub const FONT_EXTRA_LARGE: u32 = 3;

pub const SYMBOL_SETTINGS: &str = "\u{F013}";
pub const SYMBOL_REFRESH: &str = "\u{F021}";

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

/// `text` as a C string. Text from Grafana (titles, series names, errors) can
/// hold a NUL character, which a C string cannot: it is dropped, instead of
/// failing the whole screen.
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

pub unsafe fn button(parent: *mut Object, text: &str, variant: u32) -> wasm::Result<*mut Object> {
    unsafe { labelled_button(parent, text, variant).map(|(button, _)| button) }
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

/// One-line text field with an initial value.
pub unsafe fn field(parent: *mut Object, value: &str, maximum: u32) -> wasm::Result<*mut Object> {
    unsafe {
        let area = textarea_create(parent)?;
        textarea_set_one_line(area, true)?;
        textarea_set_max_length(area, maximum)?;
        object_set_width(area, percentage(100)?)?;
        set_field(area, value)?;
        Ok(area)
    }
}

pub unsafe fn set_field(area: *mut Object, value: &str) -> wasm::Result<()> {
    let value = c_string(value)?;
    unsafe { textarea_set_text(area, value.as_ptr()) }
}

pub unsafe fn field_text(area: *mut Object) -> wasm::Result<String> {
    unsafe {
        let length = textarea_get_text_length(area)?.max(0) as usize;
        let mut buffer = vec![0i8; length + 1];
        textarea_get_text(area, buffer.as_mut_ptr(), buffer.len())?;
        Ok(CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned())
    }
}

pub unsafe fn dropdown(
    parent: *mut Object,
    options: &[&str],
    selected: usize,
) -> wasm::Result<*mut Object> {
    unsafe {
        let dropdown = dropdown_create(parent)?;
        let options = CString::new(options.join("\n")).map_err(|_| -1)?;
        dropdown_set_options(dropdown, options.as_ptr())?;
        dropdown_set_selected(dropdown, selected as u32)?;
        object_set_width(dropdown, percentage(100)?)?;
        Ok(dropdown)
    }
}

pub unsafe fn is_checked(object: *mut Object) -> wasm::Result<bool> {
    unsafe { Ok(object_get_state(object)? & STATE_CHECKED != 0) }
}

pub unsafe fn set_checked(object: *mut Object, checked: bool) -> wasm::Result<()> {
    unsafe {
        if checked {
            object_add_state(object, STATE_CHECKED)
        } else {
            object_remove_state(object, STATE_CHECKED)
        }
    }
}

pub unsafe fn delete(object: *mut Object) -> wasm::Result<()> {
    unsafe { object_delete(u32::try_from(object as usize).map_err(|_| -1)?) }
}
