use core::ptr::null_mut;
use std::{ffi::CString, thread::sleep, time::Duration};

use nanoaudio_synth::Waveform;
use synth::Keyboard;
use wasm::*;

use crate::device::AudioOutput;

/// Note names by semitone; `keys` and `Keyboard` use the same indices.
const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
/// Semitones of the white and black keys across one octave plus the top C.
const WHITE_KEYS: [usize; 8] = [0, 2, 4, 5, 7, 9, 11, 12];
/// Black keys with the white key they sit after.
const BLACK_KEYS: [(usize, usize); 5] = [(1, 0), (3, 1), (6, 3), (8, 4), (10, 5)];
const TONES: [Waveform; 3] = [Waveform::Sine, Waveform::Square, Waveform::Triangle];
const TONE_NAMES: [&str; 3] = ["Sine", "Square", "Triangle"];

/// shadcn button variants, numbered as `lv_theme_shadcn_variant_t`.
const PRIMARY: u32 = 0;
const SECONDARY: u32 = 1;
const OUTLINE: u32 = 2;

const FONT_SMALL: u32 = 0;
const FONT_LARGE: u32 = 2;

/// Windows narrower or shorter than this use the compact arrangement.
const WIDE_MINIMUM_WIDTH: i32 = 560;
const WIDE_MINIMUM_HEIGHT: i32 = 360;

#[derive(Clone, Copy)]
enum Control {
    OctaveDown,
    OctaveUp,
    Retry,
}

pub struct Interface {
    window: *mut Object,
    status: *mut Object,
    retry: *mut Object,
    settings: *mut Object,
    playing: *mut Object,
    octave: *mut Object,
    tone_dropdown: *mut Object,
    volume_slider: *mut Object,
    volume_label: *mut Object,
    keys: [*mut Object; 13],
    controls: Vec<(*mut Object, Control)>,
    keyboard: Keyboard,
    compact: bool,
    tone: usize,
    volume: u8,
    audio: Option<AudioOutput>,
}

fn text(label: *mut Object, value: &str) -> wasm::Result<()> {
    let value = CString::new(value).map_err(|_| -1)?;
    unsafe { label_set_text(label, value.as_ptr()) }
}

/// Plain container without the card styling `object_create` gets from the theme.
unsafe fn container(parent: *mut Object) -> wasm::Result<*mut Object> {
    unsafe {
        let object = object_create(parent)?;
        object_set_background(object, 0, 0, 0)?;
        object_set_border(object, 0, 0, 0, 0)?;
        object_set_padding(object, 0)?;
        object_remove_flag(object, ObjectFlag::Scrollable)?;
        Ok(object)
    }
}

/// Wrapping row of controls, vertically centred.
unsafe fn row(parent: *mut Object) -> wasm::Result<*mut Object> {
    unsafe {
        let row = container(parent)?;
        object_set_size(row, percentage(100)?, size_content())?;
        object_set_flex_flow(row, FlexFlow::RowWrap)?;
        object_set_flex_align(row, FlexAlign::Start, FlexAlign::Center, FlexAlign::Center)?;
        object_set_padding(row, 6)?;
        Ok(row)
    }
}

unsafe fn label(parent: *mut Object, value: &str, font: u32) -> wasm::Result<*mut Object> {
    unsafe {
        let label = label_create(parent)?;
        object_set_font(label, font, 0)?;
        text(label, value)?;
        Ok(label)
    }
}

unsafe fn button(parent: *mut Object, value: &str, variant: u32) -> wasm::Result<*mut Object> {
    unsafe {
        let button = button_create(parent)?;
        object_set_theme_variant(button, variant)?;
        let caption = label_create(button)?;
        text(caption, value)?;
        object_align(caption, Align::Center, 0, 0)?;
        Ok(button)
    }
}

/// `C5` for MIDI note 72.
fn note_name(note: u8) -> String {
    format!(
        "{}{}",
        NOTE_NAMES[usize::from(note % 12)],
        i16::from(note / 12) - 1
    )
}

impl Interface {
    pub fn new() -> wasm::Result<Self> {
        unsafe {
            let window = window_create()?;
            window_set_icon(
                window,
                c"Sy".as_ptr(),
                Color {
                    red: 234,
                    green: 88,
                    blue: 12,
                },
            )?;
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_remove_flag(window, ObjectFlag::Scrollable)?;
            object_update_layout(window)?;
            let compact = object_get_width(window)? < WIDE_MINIMUM_WIDTH
                || object_get_height(window)? < WIDE_MINIMUM_HEIGHT;
            object_set_padding(window, if compact { 4 } else { 8 })?;

            // Status: audio state, a retry action only when silent, and the
            // note being played.
            let status_row = row(window)?;
            let status = label(status_row, "", FONT_SMALL)?;
            let retry = button(status_row, "Retry audio", OUTLINE)?;
            let playing = label(status_row, "", FONT_SMALL)?;
            object_set_width(playing, 0)?;
            object_set_flex_grow(playing, 1)?;
            object_set_text_align(playing, TextAlign::Right as u32, 0)?;

            // Controls: octave stepper, waveform and volume. Compact windows
            // give the volume its own line.
            let controls_card = card_create(window)?;
            object_set_size(controls_card, percentage(100)?, size_content())?;
            object_set_flex_flow(controls_card, FlexFlow::Column)?;
            object_set_padding(controls_card, if compact { 2 } else { 6 })?;
            object_remove_flag(controls_card, ObjectFlag::Scrollable)?;
            let first = row(controls_card)?;
            let octave_down = button(first, "Oct -", OUTLINE)?;
            let octave = label(first, "", if compact { FONT_SMALL } else { FONT_LARGE })?;
            let octave_up = button(first, "Oct +", OUTLINE)?;
            let tone_dropdown = dropdown_create(first)?;
            let tones = CString::new(TONE_NAMES.join("\n")).map_err(|_| -1)?;
            dropdown_set_options(tone_dropdown, tones.as_ptr())?;
            object_set_width(tone_dropdown, if compact { 100 } else { 120 })?;
            if compact {
                // Small, 32 px controls leave the keyboard most of a 240 px screen.
                for control in [octave_down, octave_up, tone_dropdown, retry] {
                    object_set_font(control, FONT_SMALL, 0)?;
                    object_set_min_max_size(control, 0, 16_384, 0, 32, 0)?;
                    object_set_height(control, 32)?;
                }
                object_add_flag(playing, ObjectFlag::Hidden)?;
            }
            let volume_row = if compact { row(controls_card)? } else { first };
            label(
                volume_row,
                if compact { "Vol" } else { "Volume" },
                FONT_SMALL,
            )?;
            let volume_slider = slider_create(volume_row)?;
            range_set_range(volume_slider, 0, 100)?;
            object_set_width(volume_slider, 0)?;
            object_set_flex_grow(volume_slider, 1)?;
            object_set_min_max_size(volume_slider, 80, 16_384, 0, 16_384, 0)?;
            let volume_label = label(volume_row, "", FONT_SMALL)?;
            // Summary line for accessibility and tests: octave, sound, volume.
            let settings = label(controls_card, "", FONT_SMALL)?;
            object_add_flag(settings, ObjectFlag::Hidden)?;

            // Keyboard: white keys across the remaining space, black keys on top.
            let keyboard_area = container(window)?;
            object_set_width(keyboard_area, percentage(100)?)?;
            object_set_flex_grow(keyboard_area, 1)?;
            // Keeps the keys clear of the shell's home handle.
            let inset = container(window)?;
            object_set_size(inset, percentage(100)?, 4)?;
            object_update_layout(window)?;
            let width = object_get_width(keyboard_area)?;
            let height = object_get_height(keyboard_area)?;
            let white_width = width / WHITE_KEYS.len() as i32;
            let black_width = white_width * 3 / 5;
            let mut keys = [null_mut(); 13];
            for (position, semitone) in WHITE_KEYS.iter().enumerate() {
                let key = button_create(keyboard_area)?;
                object_set_theme_variant(key, PRIMARY)?;
                object_set_min_max_size(key, 0, 16_384, 0, 16_384, 0)?;
                object_set_size(key, white_width - 4, height)?;
                object_set_pos(key, position as i32 * white_width + 2, 0)?;
                // Note names sit at the bottom, like key labels on a piano. Buttons
                // lay out their children with flex, so align through it.
                object_set_flex_flow(key, FlexFlow::Column)?;
                object_set_flex_align(key, FlexAlign::End, FlexAlign::Center, FlexAlign::Center)?;
                label(key, NOTE_NAMES[semitone % 12], FONT_SMALL)?;
                keys[*semitone] = key;
            }
            for (semitone, after) in BLACK_KEYS {
                let key = button(keyboard_area, "", SECONDARY)?;
                object_set_min_max_size(key, 0, 16_384, 0, 16_384, 0)?;
                object_set_size(key, black_width, height * 3 / 5)?;
                object_set_pos(key, (after as i32 + 1) * white_width - black_width / 2, 0)?;
                keys[semitone] = key;
            }

            let ui = Self {
                window,
                status,
                retry,
                settings,
                playing,
                octave,
                tone_dropdown,
                volume_slider,
                volume_label,
                keys,
                controls: vec![
                    (octave_down, Control::OctaveDown),
                    (octave_up, Control::OctaveUp),
                    (retry, Control::Retry),
                ],
                keyboard: Keyboard::default(),
                compact,
                tone: 0,
                volume: 70,
                audio: AudioOutput::open().ok(),
            };
            range_set_value(ui.volume_slider, i32::from(ui.volume), false)?;
            ui.render_settings()?;
            ui.render_status()?;
            ui.show_playing()?;
            Ok(ui)
        }
    }

    fn render_status(&self) -> wasm::Result<()> {
        let ready = self.audio.is_some();
        text(
            self.status,
            match (ready, self.compact) {
                (true, _) => "Audio ready",
                (false, true) => "Silent mode: no audio",
                (false, false) => "Silent mode: enable host audio, then Retry audio",
            },
        )?;
        unsafe {
            if ready {
                object_add_flag(self.retry, ObjectFlag::Hidden)
            } else {
                object_remove_flag(self.retry, ObjectFlag::Hidden)
            }
        }
    }

    fn render_settings(&self) -> wasm::Result<()> {
        text(self.octave, &format!("Octave {}", self.keyboard.octave()))?;
        text(self.volume_label, &format!("{}%", self.volume))?;
        text(
            self.settings,
            &format!(
                "Octave {} | {} | Volume {}%",
                self.keyboard.octave(),
                TONE_NAMES[self.tone],
                self.volume
            ),
        )
    }

    /// Lists held notes, or a hint when nothing plays.
    fn show_playing(&self) -> wasm::Result<()> {
        let held: Vec<String> = (0..self.keys.len())
            .filter_map(|key| self.keyboard.held(key))
            .map(note_name)
            .collect();
        if held.is_empty() {
            text(self.playing, "Hold a key to play")
        } else {
            text(self.playing, &format!("Playing {}", held.join(" ")))
        }
    }

    fn apply_volume(&mut self) {
        if let Some(audio) = &mut self.audio
            && audio.set_volume(self.volume).is_err()
        {
            self.audio = None;
        }
    }

    fn act(&mut self, control: Control) -> wasm::Result<()> {
        match control {
            Control::OctaveDown | Control::OctaveUp => {
                let delta = if matches!(control, Control::OctaveDown) {
                    -1
                } else {
                    1
                };
                if self.keyboard.shift_octave(delta)
                    && let Some(audio) = &mut self.audio
                {
                    audio.stop_all();
                }
                self.show_playing()?;
            }
            Control::Retry if self.audio.is_none() => {
                self.audio = AudioOutput::open().ok();
                if let Some(audio) = &mut self.audio {
                    audio.set_waveform(TONES[self.tone]);
                }
                self.apply_volume();
            }
            Control::Retry => {}
        }
        self.render_status()?;
        self.render_settings()
    }

    fn changed(&mut self, target: *mut Object) -> wasm::Result<()> {
        if target == self.tone_dropdown {
            let selected = unsafe { dropdown_get_selected(self.tone_dropdown)? } as usize;
            self.tone = selected.min(TONES.len() - 1);
            if let Some(audio) = &mut self.audio {
                audio.set_waveform(TONES[self.tone]);
            }
        } else if target == self.volume_slider {
            let value = unsafe { range_get_value(self.volume_slider)? };
            self.volume = value.clamp(0, 100) as u8;
            self.apply_volume();
            self.render_status()?;
        } else {
            return Ok(());
        }
        self.render_settings()
    }

    pub unsafe fn run(&mut self) -> wasm::Result<()> {
        loop {
            // Drain a bounded number of events before producing the next PCM block.
            for _ in 0..32 {
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
                    self.audio = None;
                    unsafe {
                        object_delete(u32::try_from(self.window as usize).map_err(|_| -1)?)?;
                    }
                    return Ok(());
                }
                if code == EventCode::All as u32 {
                    break;
                }
                if let Some(index) = self.keys.iter().position(|key| *key == target) {
                    if code == EventCode::Pressed as u32 {
                        if let Some(note) = self.keyboard.press(index) {
                            if let Some(audio) = &mut self.audio {
                                audio.note_on(note);
                            }
                            self.show_playing()?;
                        }
                    } else if (code == EventCode::Released as u32
                        || code == EventCode::PressLost as u32)
                        && let Some(note) = self.keyboard.release(index)
                    {
                        if let Some(audio) = &mut self.audio {
                            audio.note_off(note);
                        }
                        self.show_playing()?;
                    }
                } else if code == EventCode::Clicked as u32
                    && let Some(&(_, control)) =
                        self.controls.iter().find(|(object, _)| *object == target)
                {
                    self.act(control)?;
                } else if code == EventCode::ValueChanged as u32 {
                    self.changed(target)?;
                }
            }
            if self.audio.as_mut().is_some_and(|audio| {
                audio
                    .pump()
                    .is_err_and(|error| error.kind() != std::io::ErrorKind::WouldBlock)
            }) {
                self.audio = None;
                self.render_status()?;
            }
            sleep(Duration::from_millis(2));
        }
    }
}
