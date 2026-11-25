//! Main screen: a header with the title and the connection state of the
//! emulation device, then tabs for the media keys, the shortcut buttons of
//! the active profile, the optional mouse pad and the settings.
//!
//! The buttons are disabled while no computer listens, and a clear message
//! shows when this device has no emulation device node at all.

use core::{ffi::CStr, ptr::null_mut};
use std::{
    ffi::CString,
    thread::sleep,
    time::{Duration, Instant},
};

use embedded_i18n::t;
use shortcuts::{
    channel,
    configuration::Settings,
    hid::{MouseButtons, consumer},
    layout::Layout,
    pointer::{Pad, Scroll},
    profile::{self, Profile, Profiles},
    shortcut::Shortcut,
    storage::Storage,
};
use wasm::*;

use crate::{device::Emulation, widgets::*};

include!(concat!(env!("OUT_DIR"), "/shortcut_icon.rs"));

const ERROR_COLOR: u32 = 0xEF4444;
const SUCCESS_COLOR: u32 = 0x22C55E;
const MUTED_COLOR: u32 = 0x71717A;
const PAD_COLOR: u32 = 0x27272F;
const PAD_BORDER_COLOR: u32 = 0x52525B;

/// How often the connection state is polled.
const STATUS_POLL_PERIOD: Duration = Duration::from_millis(1000);
/// Windows narrower than this show two shortcut buttons per row.
const WIDE_MINIMUM_WIDTH: i32 = 400;
/// Width of the scroll strip, in pixels.
const SCROLL_STRIP_WIDTH: i32 = 28;

/// The notice text of an application error.
fn error_text(error: shortcuts::Error) -> String {
    match error {
        shortcuts::Error::Storage => String::from(t!("errors.storage")),
        shortcuts::Error::TooLarge => String::from(t!("errors.too_large")),
        shortcuts::Error::Configuration => String::from(t!("errors.configuration")),
        shortcuts::Error::Profiles => String::from(t!("errors.profiles")),
        shortcuts::Error::Missing => String::from(t!("errors.missing")),
        shortcuts::Error::Busy => String::from(t!("errors.busy")),
        shortcuts::Error::NotConnected => String::from(t!("errors.not_connected")),
        shortcuts::Error::Io => String::from(t!("errors.io")),
        shortcuts::Error::InvalidStatus => String::from(t!("errors.invalid_status")),
    }
}

/// One media button and the consumer usage it sends.
struct MediaButton {
    object: *mut Object,
    usage: u16,
}

/// One shortcut button and its parsed shortcut.
struct ShortcutButton {
    object: *mut Object,
    shortcut: Shortcut,
}

/// One mouse click button and the buttons it presses.
struct ClickButton {
    object: *mut Object,
    buttons: MouseButtons,
}

pub struct Interface {
    window: *mut Object,
    status: *mut Object,
    notice: *mut Object,
    tabs: *mut Object,
    media_buttons: Vec<MediaButton>,
    grid: *mut Object,
    shortcut_buttons: Vec<ShortcutButton>,
    pad: *mut Object,
    scroll_strip: *mut Object,
    click_buttons: Vec<ClickButton>,
    profile_dropdown: *mut Object,
    layout_dropdown: *mut Object,
    mouse_switch: *mut Object,
    device: Option<Emulation>,
    storage: Option<Storage>,
    settings: Settings,
    profiles: Profiles,
    pad_state: Pad,
    scroll_state: Scroll,
    connected: bool,
    caps_lock: bool,
    next_poll: Instant,
    close_requested: bool,
    shown_status: String,
    columns: i32,
}

impl Interface {
    pub fn new() -> wasm::Result<Self> {
        unsafe {
            let window = window_create()?;
            window_set_icon(window, ICON_TEXT.as_ptr(), ICON_COLOR)?;
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_remove_flag(window, ObjectFlag::Scrollable)?;
            object_set_padding(window, 4)?;
            let columns = if object_get_width(window)? < WIDE_MINIMUM_WIDTH {
                2
            } else {
                3
            };

            let header = flex(window, FlexFlow::Row)?;
            object_set_padding(header, 2)?;
            let title = label(header, t!("header.title"), FONT_LARGE)?;
            object_set_width(title, 0)?;
            object_set_flex_grow(title, 1)?;
            let status = label(header, "", FONT_SMALL)?;
            object_set_text_color(status, MUTED_COLOR, 0)?;

            let notice = paragraph(window, "", FONT_SMALL)?;
            object_set_text_color(notice, ERROR_COLOR, 0)?;
            set_hidden(notice, true)?;

            Ok(Self {
                window,
                status,
                notice,
                tabs: null_mut(),
                media_buttons: Vec::new(),
                grid: null_mut(),
                shortcut_buttons: Vec::new(),
                pad: null_mut(),
                scroll_strip: null_mut(),
                click_buttons: Vec::new(),
                profile_dropdown: null_mut(),
                layout_dropdown: null_mut(),
                mouse_switch: null_mut(),
                device: None,
                storage: Storage::from_environment(),
                settings: Settings::default(),
                profiles: Profiles::default(),
                pad_state: Pad::new(),
                scroll_state: Scroll::new(),
                connected: false,
                caps_lock: false,
                next_poll: Instant::now(),
                close_requested: false,
                shown_status: String::new(),
                columns,
            })
        }
    }

    /// Reads the files, opens the device and builds the tabs.
    fn start(&mut self) -> wasm::Result<()> {
        let mut startup_error: Option<shortcuts::Error> = None;
        if let Some(storage) = &self.storage {
            match storage.load_settings() {
                Ok(Some(settings)) => self.settings = settings,
                Ok(None) => (),
                Err(_) => startup_error = Some(shortcuts::Error::Configuration),
            }
            match storage.load_profiles() {
                Ok(Some(profiles)) => self.profiles = profiles,
                Ok(None) => (),
                Err(error) => {
                    if startup_error.is_none() {
                        startup_error = Some(error);
                    }
                }
            }
        }
        // The device node can be absent on a Xila without a human interface
        // lower half: the message says so and the buttons stay disabled.
        match Emulation::open() {
            Ok(device) => self.device = Some(device),
            Err(shortcuts::Error::Missing) => {
                self.set_notice(t!("status.no_device"))?;
            }
            Err(error) => {
                startup_error = Some(error);
            }
        }
        if let Some(error) = startup_error {
            self.set_notice(&error_text(error))?;
        }
        unsafe { self.build_tabs()? };
        self.poll_status();
        Ok(())
    }

    /// Shows `text` in the notice line, or hides the line when empty.
    fn set_notice(&mut self, text: &str) -> wasm::Result<()> {
        unsafe {
            set_hidden(self.notice, text.is_empty())?;
            set_text(self.notice, text)?;
        }
        Ok(())
    }

    /// Recreates the tab view and every tab. Used at start and when the
    /// mouse pad setting changes the set of pages.
    unsafe fn build_tabs(&mut self) -> wasm::Result<()> {
        unsafe {
            if !self.tabs.is_null() {
                delete(self.tabs)?;
            }
            self.media_buttons.clear();
            self.shortcut_buttons.clear();
            self.click_buttons.clear();
            self.grid = null_mut();
            self.pad = null_mut();
            self.scroll_strip = null_mut();
            self.profile_dropdown = null_mut();
            self.layout_dropdown = null_mut();
            self.mouse_switch = null_mut();

            let tabs = tabview_create(self.window)?;
            object_set_width(tabs, percentage(100)?)?;
            object_set_flex_grow(tabs, 1)?;
            self.tabs = tabs;

            self.build_media_tab(t!(c"tabs.media"))?;
            self.build_shortcuts_tab(t!(c"tabs.shortcuts"))?;
            if self.settings.mouse_pad {
                self.build_mouse_tab(t!(c"tabs.mouse"))?;
            }
            self.build_settings_tab(t!(c"tabs.settings"))?;
            self.apply_connection()?;
            Ok(())
        }
    }

    unsafe fn tab(&self, name: &CStr) -> wasm::Result<*mut Object> {
        unsafe {
            let tab = tabview_add_tab(self.tabs, name.as_ptr())?;
            object_set_flex_flow(tab, FlexFlow::Column)?;
            object_set_flex_align(tab, FlexAlign::Start, FlexAlign::Center, FlexAlign::Center)?;
            object_set_font(tab, FONT_MEDIUM, 0)?;
            object_set_padding(tab, 6)?;
            Ok(tab)
        }
    }

    /// The media keys page: play/pause, previous, next, stop, volume down,
    /// volume up and mute, as consumer control usages.
    unsafe fn build_media_tab(&mut self, name: &CStr) -> wasm::Result<()> {
        unsafe {
            let tab = self.tab(name)?;
            let row = container(tab)?;
            object_set_width(row, percentage(100)?)?;
            object_set_flex_grow(row, 1)?;
            object_set_flex_flow(row, FlexFlow::RowWrap)?;
            object_set_flex_align(row, FlexAlign::Center, FlexAlign::Center, FlexAlign::Center)?;
            object_add_flag(row, ObjectFlag::Scrollable)?;
            let controls: [(&str, u16); 7] = [
                (SYMBOL_PLAY_PAUSE, consumer::PLAY_PAUSE),
                (SYMBOL_PREVIOUS, consumer::PREVIOUS_TRACK),
                (SYMBOL_NEXT, consumer::NEXT_TRACK),
                (SYMBOL_STOP, consumer::STOP),
                (SYMBOL_VOLUME_DOWN, consumer::VOLUME_DOWN),
                (SYMBOL_VOLUME_UP, consumer::VOLUME_UP),
                (SYMBOL_MUTE, consumer::MUTE),
            ];
            for (symbol, usage) in controls {
                let (object, caption) = labelled_button(row, symbol, OUTLINE)?;
                object_set_font(caption, FONT_LARGE, 0)?;
                object_set_width(object, percentage(23)?)?;
                self.media_buttons.push(MediaButton { object, usage });
            }
            Ok(())
        }
    }

    /// The shortcuts page: a grid of the buttons of the active profile.
    unsafe fn build_shortcuts_tab(&mut self, name: &CStr) -> wasm::Result<()> {
        unsafe {
            let tab = self.tab(name)?;
            let grid = container(tab)?;
            object_set_width(grid, percentage(100)?)?;
            object_set_flex_grow(grid, 1)?;
            object_set_flex_flow(grid, FlexFlow::RowWrap)?;
            object_set_flex_align(grid, FlexAlign::Start, FlexAlign::Start, FlexAlign::Start)?;
            object_add_flag(grid, ObjectFlag::Scrollable)?;
            self.grid = grid;
            self.rebuild_shortcuts()?;
            Ok(())
        }
    }

    /// Recreates the shortcut buttons from the active profile and layout.
    unsafe fn rebuild_shortcuts(&mut self) -> wasm::Result<()> {
        unsafe {
            for button in self.shortcut_buttons.drain(..) {
                delete(button.object)?;
            }
            let buttons = self.profiles.buttons(self.settings.profile);
            let compiled = profile::compile(&buttons, self.settings.layout);
            let width = if self.columns == 3 { 31 } else { 48 };
            for button in compiled {
                let object = button_create(self.grid)?;
                object_set_theme_variant(object, OUTLINE)?;
                object_set_width(object, percentage(width)?)?;
                object_set_flex_flow(object, FlexFlow::Column)?;
                object_set_flex_align(
                    object,
                    FlexAlign::Center,
                    FlexAlign::Center,
                    FlexAlign::Center,
                )?;
                let caption = label_create(object)?;
                set_text(caption, &button.label)?;
                object_set_font(caption, FONT_SMALL, 0)?;
                object_set_text_align(caption, TextAlign::Center as u32, 0)?;
                let keys = label_create(object)?;
                let text = button.shortcut.display_text(self.settings.layout);
                set_text(keys, &text)?;
                object_set_font(keys, FONT_SMALL, 0)?;
                object_set_text_color(keys, MUTED_COLOR, 0)?;
                object_set_text_align(keys, TextAlign::Center as u32, 0)?;
                self.shortcut_buttons.push(ShortcutButton {
                    object,
                    shortcut: button.shortcut,
                });
            }
            self.apply_connection()?;
            Ok(())
        }
    }

    /// The mouse pad page: a touch area for relative movement, a scroll
    /// strip, and the left and right click buttons.
    unsafe fn build_mouse_tab(&mut self, name: &CStr) -> wasm::Result<()> {
        unsafe {
            let tab = self.tab(name)?;
            paragraph(tab, t!("mouse.pad_hint"), FONT_SMALL)?;
            let main = flex(tab, FlexFlow::Row)?;
            object_set_flex_grow(main, 1)?;
            object_set_width(main, percentage(100)?)?;

            let pad = object_create(main)?;
            object_set_flex_grow(pad, 1)?;
            object_set_background(pad, PAD_COLOR, 0xFF, 0)?;
            object_set_border(pad, PAD_BORDER_COLOR, 1, 0, 0)?;
            object_set_padding(pad, 4)?;
            object_add_flag(pad, ObjectFlag::Clickable)?;
            object_remove_flag(pad, ObjectFlag::Scrollable)?;
            self.pad = pad;

            let strip = object_create(main)?;
            object_set_width(strip, SCROLL_STRIP_WIDTH)?;
            object_set_background(strip, PAD_COLOR, 0xFF, 0)?;
            object_set_border(strip, PAD_BORDER_COLOR, 1, 0, 0)?;
            object_add_flag(strip, ObjectFlag::Clickable)?;
            object_remove_flag(strip, ObjectFlag::Scrollable)?;
            self.scroll_strip = strip;

            let clicks = flex(tab, FlexFlow::Row)?;
            object_set_width(clicks, percentage(100)?)?;
            let (left, _) = labelled_button(clicks, t!("mouse.left_click"), PRIMARY)?;
            let (right, _) = labelled_button(clicks, t!("mouse.right_click"), OUTLINE)?;
            self.click_buttons.push(ClickButton {
                object: left,
                buttons: MouseButtons::LEFT,
            });
            self.click_buttons.push(ClickButton {
                object: right,
                buttons: MouseButtons::RIGHT,
            });
            paragraph(tab, t!("mouse.scroll_hint"), FONT_SMALL)?;
            Ok(())
        }
    }

    /// The settings page: the profile of the computer, the keyboard layout
    /// and the mouse pad switch.
    unsafe fn build_settings_tab(&mut self, name: &CStr) -> wasm::Result<()> {
        unsafe {
            let tab = self.tab(name)?;
            object_set_flex_align(tab, FlexAlign::Start, FlexAlign::Start, FlexAlign::Start)?;

            let profile_row = flex(tab, FlexFlow::Row)?;
            let profile_label = label(profile_row, t!("settings.computer"), FONT_MEDIUM)?;
            object_set_width(profile_label, 0)?;
            object_set_flex_grow(profile_label, 1)?;
            let profile_dropdown = dropdown_create(profile_row)?;
            let options = [
                String::from(t!("settings.windows")),
                String::from(t!("settings.macos")),
                String::from(t!("settings.gnome")),
            ];
            let options = CString::new(options.join("\n")).map_err(|_| -1)?;
            dropdown_set_options(profile_dropdown, options.as_ptr())?;
            let selected = Profile::ALL
                .iter()
                .position(|profile| *profile == self.settings.profile)
                .unwrap_or(0) as u32;
            dropdown_set_selected(profile_dropdown, selected)?;
            self.profile_dropdown = profile_dropdown;

            let layout_row = flex(tab, FlexFlow::Row)?;
            let layout_label = label(layout_row, t!("settings.keyboard_layout"), FONT_MEDIUM)?;
            object_set_width(layout_label, 0)?;
            object_set_flex_grow(layout_label, 1)?;
            let layout_dropdown = dropdown_create(layout_row)?;
            let options = [
                String::from(t!("settings.qwerty")),
                String::from(t!("settings.azerty")),
                String::from(t!("settings.qwertz")),
            ];
            let options = CString::new(options.join("\n")).map_err(|_| -1)?;
            dropdown_set_options(layout_dropdown, options.as_ptr())?;
            let selected = Layout::ALL
                .iter()
                .position(|layout| *layout == self.settings.layout)
                .unwrap_or(0) as u32;
            dropdown_set_selected(layout_dropdown, selected)?;
            self.layout_dropdown = layout_dropdown;

            let mouse_row = flex(tab, FlexFlow::Row)?;
            let mouse_label = label(mouse_row, t!("settings.mouse_pad"), FONT_MEDIUM)?;
            object_set_width(mouse_label, 0)?;
            object_set_flex_grow(mouse_label, 1)?;
            let mouse_switch = switch_create(mouse_row)?;
            if self.settings.mouse_pad {
                object_add_state(mouse_switch, STATE_CHECKED)?;
            }
            self.mouse_switch = mouse_switch;
            Ok(())
        }
    }

    /// Asks the device for the connection state and updates the header.
    fn poll_status(&mut self) {
        self.next_poll = Instant::now() + STATUS_POLL_PERIOD;
        let Some(device) = &mut self.device else {
            return;
        };
        match channel::query_status(device) {
            Ok(status) => {
                self.connected = status.connected;
                self.caps_lock = status.caps_lock();
                let _ = unsafe { self.apply_connection() };
                let _ = self.update_status();
            }
            Err(_) => {
                self.connected = false;
                let _ = unsafe { self.apply_connection() };
                let _ = self.update_status();
            }
        }
    }

    /// Enables or disables every control with the connection state.
    unsafe fn apply_connection(&mut self) -> wasm::Result<()> {
        unsafe {
            let disabled = !self.connected || self.device.is_none();
            let state = if disabled { STATE_DISABLED } else { 0 };
            for object in self
                .media_buttons
                .iter()
                .map(|button| button.object)
                .chain(self.shortcut_buttons.iter().map(|button| button.object))
                .chain(self.click_buttons.iter().map(|button| button.object))
                .chain([self.pad, self.scroll_strip])
            {
                if object.is_null() {
                    continue;
                }
                if disabled {
                    object_add_state(object, state)?;
                } else {
                    object_remove_state(object, state)?;
                }
            }
            Ok(())
        }
    }

    /// The header status line: the connection state, and the caps lock
    /// indicator when the computer reports it.
    fn update_status(&mut self) -> wasm::Result<()> {
        let (text, color) = if self.device.is_none() {
            (String::new(), MUTED_COLOR)
        } else if self.connected {
            let mut text = String::from(t!("status.connected"));
            if self.caps_lock {
                text.push_str(", ");
                text.push_str(&String::from(t!("status.caps_lock")));
            }
            (text, SUCCESS_COLOR)
        } else {
            (String::from(t!("status.not_connected")), ERROR_COLOR)
        };
        if text != self.shown_status {
            unsafe {
                set_text(self.status, &text)?;
                object_set_text_color(self.status, color, 0)?;
            }
            self.shown_status = text;
        }
        Ok(())
    }

    /// Sends one action to the device, showing its error in the notice line.
    fn act(&mut self, action: impl FnOnce(&mut Emulation) -> shortcuts::Result<()>) {
        let Some(device) = &mut self.device else {
            return;
        };
        match action(device) {
            Ok(()) => {
                let _ = self.set_notice("");
            }
            Err(error) => {
                let _ = self.set_notice(&error_text(error));
            }
        }
    }

    fn handle(&mut self, code: u32, target: *mut Object, x: i32, y: i32) -> wasm::Result<()> {
        if code == EventCode::Clicked as u32 {
            if let Some(button) = self.media_buttons.iter().find(|b| b.object == target) {
                let usage = button.usage;
                self.act(|device| channel::tap_consumer(device, usage));
                return Ok(());
            }
            if let Some(button) = self
                .shortcut_buttons
                .iter()
                .find(|b| b.object == target)
                .map(|b| b.shortcut.clone())
            {
                self.act(|device| button.tap(device));
                return Ok(());
            }
            if let Some(button) = self.click_buttons.iter().find(|b| b.object == target) {
                let buttons = button.buttons;
                self.act(|device| channel::mouse_click(device, buttons));
                return Ok(());
            }
            return Ok(());
        }
        if code == EventCode::ValueChanged as u32 {
            if target == self.profile_dropdown {
                let selected = unsafe { dropdown_get_selected(self.profile_dropdown)? } as usize;
                if let Some(&profile) = Profile::ALL.get(selected) {
                    self.settings.profile = profile;
                    self.save_settings();
                    unsafe { self.rebuild_shortcuts()? };
                }
                return Ok(());
            }
            if target == self.layout_dropdown {
                let selected = unsafe { dropdown_get_selected(self.layout_dropdown)? } as usize;
                if let Some(&layout) = Layout::ALL.get(selected) {
                    self.settings.layout = layout;
                    self.save_settings();
                    unsafe { self.rebuild_shortcuts()? };
                }
                return Ok(());
            }
            if target == self.mouse_switch {
                let state = unsafe { object_get_state(self.mouse_switch)? };
                self.settings.mouse_pad = state & STATE_CHECKED != 0;
                self.save_settings();
                unsafe { self.build_tabs()? };
                return Ok(());
            }
            return Ok(());
        }
        // Pointer events on the mouse pad and the scroll strip.
        if target == self.pad {
            if code == EventCode::Pressed as u32 {
                self.pad_state.press(x, y);
            } else if code == EventCode::Pressing as u32 {
                let (dx, dy) = self.pad_state.drag(x, y);
                if dx != 0 || dy != 0 {
                    self.act(|device| channel::mouse_move(device, MouseButtons::NONE, dx, dy));
                }
            } else if code == EventCode::Released as u32 || code == EventCode::PressLost as u32 {
                self.pad_state.release();
            }
            return Ok(());
        }
        if target == self.scroll_strip {
            if code == EventCode::Pressed as u32 {
                self.scroll_state.press(y);
            } else if code == EventCode::Pressing as u32 {
                let wheel = self.scroll_state.drag(y);
                if wheel != 0 {
                    self.act(|device| channel::mouse_scroll(device, wheel));
                }
            } else if code == EventCode::Released as u32 || code == EventCode::PressLost as u32 {
                self.scroll_state.release();
            }
        }
        Ok(())
    }

    fn save_settings(&self) {
        if let Some(storage) = &self.storage {
            let _ = storage.save_settings(&self.settings);
        }
    }

    pub fn run(&mut self) -> wasm::Result<()> {
        self.start()?;
        loop {
            if self.close_requested {
                unsafe { delete(self.window)? };
                return Ok(());
            }
            // Bound the work per turn even if an input device floods the
            // window queue.
            for _ in 0..32 {
                let mut code = EventCode::All as u32;
                let mut target: *mut Object = null_mut();
                let (mut x, mut y, mut _width, mut _height) = (0, 0, 0, 0);
                unsafe {
                    window_pop_pointer_event(
                        self.window,
                        &mut code,
                        (&mut target as *mut *mut Object).cast(),
                        &mut x,
                        &mut y,
                        &mut _width,
                        &mut _height,
                    )?;
                }
                if code == EVENT_CLOSE_REQUESTED {
                    self.close_requested = true;
                    break;
                }
                if code == EventCode::All as u32 {
                    break;
                }
                self.handle(code, target, x, y)?;
                if self.close_requested {
                    break;
                }
            }
            if self.close_requested {
                continue;
            }
            if Instant::now() >= self.next_poll {
                self.poll_status();
            }
            sleep(Duration::from_millis(50));
        }
    }
}
