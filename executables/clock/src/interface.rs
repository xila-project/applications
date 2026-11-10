use clock::{
    Alarm, AlarmAlert, Settings, Stopwatch, Timer, join_duration, remaining_permille,
    split_duration, storage::Store,
};
use embedded_i18n::t;
use std::{
    ffi::{CStr, CString},
    ptr::null_mut,
    thread::sleep,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use wasm::*;

/// shadcn button variants, numbered as `lv_theme_shadcn_variant_t`.
const PRIMARY: u32 = 0;
const OUTLINE: u32 = 2;
const GHOST: u32 = 3;
const DESTRUCTIVE: u32 = 4;

const FONT_SMALL: u32 = 0;
const FONT_MEDIUM: u32 = 1;
const FONT_LARGE: u32 = 2;
const FONT_EXTRA_LARGE: u32 = 3;

const TAB_STOPWATCH: u32 = 2;

/// Windows narrower or shorter than this use the compact arrangement.
const WIDE_MINIMUM_WIDTH: i32 = 560;
const WIDE_MINIMUM_HEIGHT: i32 = 360;

#[derive(Clone, Copy)]
enum Action {
    AlarmDay(usize),
    AlarmSave,
    AlarmDisable,
    AlarmDelete,
    StopwatchToggle,
    Lap,
    StopwatchReset,
    TimerStart,
    TimerToggle,
    TimerRestart,
    TimerCancel,
    Snooze,
    Dismiss,
}

/// Labels whose text is refreshed by `render`, indexed into `rendered`.
#[derive(Clone, Copy)]
enum Text {
    Alert,
    Time,
    Date,
    AlarmStatus,
    Stopwatch,
    Laps,
    TimerRemaining,
    TimerDuration,
}
const TEXT_COUNT: usize = 8;

struct Interface {
    window: *mut Object,
    tabs: *mut Object,
    compact: bool,
    /// Seconds ring around the time (wide windows only).
    clock_ring: *mut Object,
    laps_card: *mut Object,
    controls: Vec<(*mut Object, Action)>,
    texts: [*mut Object; TEXT_COUNT],
    rendered: [String; TEXT_COUNT],
    banner: *mut Object,
    snooze: *mut Object,
    format_switch: *mut Object,
    alarm_hour: *mut Object,
    alarm_minute: *mut Object,
    days: [*mut Object; 7],
    selected_days: [bool; 7],
    timer_picker: *mut Object,
    timer_rollers: [*mut Object; 3],
    timer_ring: *mut Object,
    timer_idle_actions: *mut Object,
    timer_active_actions: *mut Object,
    /// Cached visibility, so `render` only touches objects that change.
    shown: Shown,
    settings: Settings,
    store: Option<Store>,
    persistence_error: bool,
    save_pending: bool,
    watch: Stopwatch,
    timer: Timer,
    alarm_alert: AlarmAlert,
}

#[derive(Default, PartialEq, Eq, Clone, Copy)]
struct Shown {
    banner: Option<bool>,
    snooze: Option<bool>,
    timer_active: Option<bool>,
    laps: Option<bool>,
}

pub fn run() -> wasm::Result<()> {
    // The native epoch is supplied through WASI's monotonic clock.
    let epoch = Instant::now();
    let mut ui = unsafe { Interface::new()? };
    let mut last_refresh = None;
    let mut alarm_sound = clock::sound::AlarmSound::default();
    loop {
        let now = epoch.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|value| value.as_secs());
        let mut dirty = ui.tick(now, wall);
        // Bound work per turn even if an input device floods the window queue.
        for _ in 0..32 {
            let mut code = 0u32;
            let mut target: *mut Object = null_mut();
            unsafe {
                window_pop_event(
                    ui.window,
                    &mut code,
                    (&mut target as *mut *mut Object).cast(),
                )?;
            }
            if code == EVENT_CLOSE_REQUESTED {
                // Never drop a change made just before closing.
                ui.flush_save();
                unsafe {
                    object_delete(u16::try_from(ui.window as usize).map_err(|_| -1)?)?;
                }
                return Ok(());
            }
            if code == EventCode::All as u32 {
                break;
            }
            if code == EventCode::Clicked as u32
                && let Some((_, action)) = ui
                    .controls
                    .iter()
                    .find(|(object, _)| *object == target)
                    .copied()
            {
                unsafe {
                    ui.act(action, now, wall)?;
                }
                dirty = true;
            }
            if code == EventCode::ValueChanged as u32 {
                if target == ui.format_switch {
                    let state = unsafe { object_get_state(ui.format_switch)? };
                    ui.settings.twelve_hour = state & STATE_CHECKED != 0;
                    ui.persist();
                }
                dirty = true;
            }
        }
        let visible_stopwatch =
            unsafe { tabview_get_tab_active(ui.tabs)? } == TAB_STOPWATCH && ui.watch.is_running();
        let interval = if visible_stopwatch { 100 } else { 1000 };
        if dirty || last_refresh.is_none_or(|last| now.saturating_sub(last) >= interval) {
            unsafe {
                ui.render(now, wall)?;
            }
            last_refresh = Some(now);
        }
        if ui.save_pending {
            let failed = ui.persistence_error;
            ui.flush_save();
            if ui.persistence_error != failed {
                // Show or clear the save error banner right away.
                unsafe { ui.render(now, wall)? };
            }
        }
        alarm_sound.update(ui.alarm_alert.is_ringing() || ui.timer.finished());
        // There is no blocking guest window-event API yet. Cooperatively wait
        // between bounded event checks; elapsed durations do not count polls.
        // A ringing tone needs short turns to keep the audio queue filled.
        sleep(Duration::from_millis(if alarm_sound.is_active() {
            5
        } else {
            100
        }));
    }
}

/// Roller options `00` to `count - 1`, one per line.
fn two_digit_options(count: u32) -> CString {
    let options = (0..count)
        .map(|value| format!("{value:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    CString::new(options).expect("digits contain no NUL")
}

impl Interface {
    unsafe fn new() -> wasm::Result<Self> {
        unsafe {
            let (store, settings, persistence_error) =
                match std::env::var("XILA_APPLICATION_CONFIGURATION") {
                    Ok(directory) => match Store::load(&directory) {
                        Ok((store, settings)) => (Some(store), settings, false),
                        Err(error) => {
                            eprintln!("Clock settings unavailable: {error}");
                            (None, Settings::default(), true)
                        }
                    },
                    Err(error) => {
                        eprintln!("Clock settings directory unavailable: {error}");
                        (None, Settings::default(), true)
                    }
                };
            let window = window_create()?;
            // Ensure a partially constructed window is released on FFI failure.
            match Self::build(window, store, settings, persistence_error) {
                Ok(ui) => Ok(ui),
                Err(error) => {
                    if let Ok(handle) = u16::try_from(window as usize) {
                        let _ = object_delete(handle);
                    }
                    Err(error)
                }
            }
        }
    }

    unsafe fn build(
        window: *mut Object,
        store: Option<Store>,
        settings: Settings,
        persistence_error: bool,
    ) -> wasm::Result<Self> {
        unsafe {
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_set_padding(window, 6)?;
            object_remove_flag(window, ObjectFlag::Scrollable)?;
            window_set_icon(
                window,
                c"Cl".as_ptr(),
                Color {
                    red: 59,
                    green: 130,
                    blue: 246,
                },
            )?;
            let mut ui = Self {
                window,
                tabs: null_mut(),
                compact: false,
                clock_ring: null_mut(),
                laps_card: null_mut(),
                controls: Vec::with_capacity(24),
                texts: [null_mut(); TEXT_COUNT],
                rendered: core::array::from_fn(|_| String::new()),
                banner: null_mut(),
                snooze: null_mut(),
                format_switch: null_mut(),
                alarm_hour: null_mut(),
                alarm_minute: null_mut(),
                days: [null_mut(); 7],
                selected_days: core::array::from_fn(|index| {
                    settings
                        .alarm
                        .is_some_and(|alarm| alarm.weekdays & (1 << index) != 0)
                }),
                timer_picker: null_mut(),
                timer_rollers: [null_mut(); 3],
                timer_ring: null_mut(),
                timer_idle_actions: null_mut(),
                timer_active_actions: null_mut(),
                shown: Shown::default(),
                settings,
                store,
                persistence_error,
                save_pending: false,
                watch: Stopwatch::default(),
                timer: Timer::default(),
                alarm_alert: AlarmAlert::default(),
            };

            // Alert banner: only shown while something needs attention.
            let banner = card_create(window)?;
            object_set_width(banner, percentage(100)?)?;
            object_set_height(banner, size_content())?;
            object_set_flex_flow(banner, FlexFlow::RowWrap)?;
            object_set_flex_align(banner, FlexAlign::End, FlexAlign::Center, FlexAlign::Center)?;
            object_set_padding(banner, 6)?;
            object_remove_flag(banner, ObjectFlag::Scrollable)?;
            object_add_flag(banner, ObjectFlag::Hidden)?;
            ui.banner = banner;
            let alert = ui.label(banner, "", FONT_LARGE)?;
            object_set_width(alert, 0)?;
            object_set_flex_grow(alert, 1)?;
            object_set_min_max_size(alert, 120, 16_384, 0, 16_384, 0)?;
            ui.texts[Text::Alert as usize] = alert;
            ui.snooze = ui.button(
                banner,
                t!(c"alerts.snooze_five_minutes"),
                OUTLINE,
                Action::Snooze,
            )?;
            ui.button(banner, t!(c"alerts.dismiss"), PRIMARY, Action::Dismiss)?;

            object_update_layout(window)?;
            ui.compact = object_get_width(window)? < WIDE_MINIMUM_WIDTH
                || object_get_height(window)? < WIDE_MINIMUM_HEIGHT;

            let tabs = tabview_create(window)?;
            object_set_width(tabs, percentage(100)?)?;
            object_set_flex_grow(tabs, 1)?;
            if ui.compact {
                // Smaller tab titles keep all four tabs on a 320 px screen;
                // each tab page restores the normal font for its content.
                object_set_font(tabs, FONT_SMALL, 0)?;
            }
            ui.tabs = tabs;
            ui.build_clock_tab()?;
            ui.build_alarm_tab()?;
            ui.build_stopwatch_tab()?;
            ui.build_timer_tab()?;
            Ok(ui)
        }
    }

    unsafe fn build_clock_tab(&mut self) -> wasm::Result<()> {
        unsafe {
            let tab = self.tab(t!(c"tabs.clock"))?;
            if self.compact {
                self.texts[Text::Time as usize] = self.centered(tab, FONT_EXTRA_LARGE)?;
                self.texts[Text::Date as usize] = self.centered(tab, FONT_LARGE)?;
            } else {
                // A seconds ring frames the time like a watch face.
                let ring = Self::ring(tab, 280, 60)?;
                let time = self.label(ring, "", FONT_EXTRA_LARGE)?;
                object_align(time, Align::Center, 0, -12)?;
                let date = self.label(ring, "", FONT_MEDIUM)?;
                object_align(date, Align::Center, 0, 24)?;
                self.texts[Text::Time as usize] = time;
                self.texts[Text::Date as usize] = date;
                self.clock_ring = ring;
            }
            let note = self.centered(tab, FONT_SMALL)?;
            Self::text(note, t!("clock.instructions.utc_only"))?;

            let format_row = self.row(tab)?;
            let format_label = label_create(format_row)?;
            label_set_text(
                format_label,
                t!(c"clock.controls.toggle_hour_format").as_ptr(),
            )?;
            let format_switch = switch_create(format_row)?;
            if self.settings.twelve_hour {
                object_add_state(format_switch, STATE_CHECKED)?;
            }
            self.format_switch = format_switch;
            Ok(())
        }
    }

    unsafe fn build_alarm_tab(&mut self) -> wasm::Result<()> {
        unsafe {
            let tab = self.tab(t!(c"tabs.alarm"))?;
            let picker = self.row(tab)?;
            let (hour, minute) = self
                .settings
                .alarm
                .map_or((7, 0), |alarm| (alarm.hour, alarm.minute));
            self.alarm_hour = self.roller(picker, 24, u32::from(hour), true)?;
            self.label(picker, ":", FONT_LARGE)?;
            self.alarm_minute = self.roller(picker, 60, u32::from(minute), true)?;
            self.label(picker, "UTC", FONT_SMALL)?;

            // Weekday chips: filled when selected, outlined otherwise.
            let days = self.row(tab)?;
            let names = [
                t!(c"alarm.weekdays.monday"),
                t!(c"alarm.weekdays.tuesday"),
                t!(c"alarm.weekdays.wednesday"),
                t!(c"alarm.weekdays.thursday"),
                t!(c"alarm.weekdays.friday"),
                t!(c"alarm.weekdays.saturday"),
                t!(c"alarm.weekdays.sunday"),
            ];
            for (index, name) in names.iter().enumerate() {
                let variant = if self.selected_days[index] {
                    PRIMARY
                } else {
                    OUTLINE
                };
                self.days[index] = self.button(days, name, variant, Action::AlarmDay(index))?;
            }

            self.texts[Text::AlarmStatus as usize] = self.centered(tab, FONT_SMALL)?;
            let actions = self.row(tab)?;
            self.button(
                actions,
                t!(c"alarm.controls.save_and_enable"),
                PRIMARY,
                Action::AlarmSave,
            )?;
            self.button(
                actions,
                t!(c"alarm.controls.disable"),
                OUTLINE,
                Action::AlarmDisable,
            )?;
            self.button(
                actions,
                t!(c"alarm.controls.delete"),
                DESTRUCTIVE,
                Action::AlarmDelete,
            )?;
            let note = self.centered(tab, FONT_SMALL)?;
            Self::text(
                note,
                &format!(
                    "{}\n{}",
                    t!("alarm.instructions.repeat_days_utc"),
                    t!("alerts.instructions")
                ),
            )?;
            Ok(())
        }
    }

    unsafe fn build_stopwatch_tab(&mut self) -> wasm::Result<()> {
        unsafe {
            let tab = self.tab(t!(c"tabs.stopwatch"))?;
            // Laps grow below the readout, so this tab starts at the top.
            object_set_flex_align(tab, FlexAlign::Start, FlexAlign::Center, FlexAlign::Center)?;
            self.texts[Text::Stopwatch as usize] = self.centered(tab, FONT_EXTRA_LARGE)?;
            let actions = self.row(tab)?;
            self.button(
                actions,
                t!(c"stopwatch.controls.toggle_pause"),
                PRIMARY,
                Action::StopwatchToggle,
            )?;
            self.button(
                actions,
                t!(c"stopwatch.controls.record_lap"),
                OUTLINE,
                Action::Lap,
            )?;
            self.button(
                actions,
                t!(c"stopwatch.controls.reset"),
                GHOST,
                Action::StopwatchReset,
            )?;
            // Laps scroll inside their own card, newest first.
            let laps = card_create(tab)?;
            object_set_width(laps, percentage(100)?)?;
            object_set_flex_grow(laps, 1)?;
            object_set_min_max_size(laps, 0, 16_384, 48, 16_384, 0)?;
            object_set_padding(laps, 8)?;
            // Shown once the first lap is recorded.
            object_add_flag(laps, ObjectFlag::Hidden)?;
            let text = self.label(laps, "", FONT_MEDIUM)?;
            object_set_width(text, percentage(100)?)?;
            self.texts[Text::Laps as usize] = text;
            self.laps_card = laps;
            Ok(())
        }
    }

    unsafe fn build_timer_tab(&mut self) -> wasm::Result<()> {
        unsafe {
            let tab = self.tab(t!(c"tabs.timer"))?;
            // Idle: pick hours, minutes and seconds.
            let picker = self.row(tab)?;
            let (hours, minutes, seconds) = split_duration(300);
            self.timer_rollers[0] = self.roller(picker, 24, hours, false)?;
            self.label(picker, ":", FONT_LARGE)?;
            self.timer_rollers[1] = self.roller(picker, 60, minutes, false)?;
            self.label(picker, ":", FONT_LARGE)?;
            self.timer_rollers[2] = self.roller(picker, 60, seconds, false)?;
            self.timer_picker = picker;

            // Running, paused or finished: a progress ring with the time left.
            let ring = Self::ring(tab, if self.compact { 120 } else { 240 }, 1000)?;
            object_add_flag(ring, ObjectFlag::Hidden)?;
            let remaining_font = if self.compact {
                FONT_LARGE
            } else {
                FONT_EXTRA_LARGE
            };
            let remaining = self.label(ring, "", remaining_font)?;
            object_align(remaining, Align::Center, 0, -8)?;
            self.texts[Text::TimerRemaining as usize] = remaining;
            let duration = self.label(ring, "", FONT_SMALL)?;
            object_align(
                duration,
                Align::Center,
                0,
                if self.compact { 16 } else { 24 },
            )?;
            self.texts[Text::TimerDuration as usize] = duration;
            self.timer_ring = ring;

            let idle = self.row(tab)?;
            self.button(
                idle,
                t!(c"timer.controls.start"),
                PRIMARY,
                Action::TimerStart,
            )?;
            self.timer_idle_actions = idle;
            let active = self.row(tab)?;
            self.button(
                active,
                t!(c"timer.controls.toggle_pause"),
                PRIMARY,
                Action::TimerToggle,
            )?;
            self.button(
                active,
                t!(c"timer.controls.restart"),
                OUTLINE,
                Action::TimerRestart,
            )?;
            self.button(
                active,
                t!(c"timer.controls.cancel"),
                DESTRUCTIVE,
                Action::TimerCancel,
            )?;
            object_add_flag(active, ObjectFlag::Hidden)?;
            self.timer_active_actions = active;
            Ok(())
        }
    }

    /// Read-only progress ring with values `0..=maximum`.
    unsafe fn ring(parent: *mut Object, size: i32, maximum: i32) -> wasm::Result<*mut Object> {
        unsafe {
            let ring = arc_create(parent)?;
            object_set_size(ring, size, size)?;
            range_set_range(ring, 0, maximum)?;
            range_set_value(ring, 0, false)?;
            object_remove_flag(ring, ObjectFlag::Clickable)?;
            // An indicator, not a control: no draggable knob.
            object_set_background(ring, 0, 0, PART_KNOB)?;
            object_set_border(ring, 0, 0, 0, PART_KNOB)?;
            object_set_padding(ring, 0)?;
            Ok(ring)
        }
    }

    /// Two-digit picker. Times of day wrap like a watch; durations start at 0.
    /// The host retains 15 copies of a wrapping roller's options, so only
    /// the two alarm rollers wrap to stay within the per-app options budget.
    unsafe fn roller(
        &self,
        parent: *mut Object,
        count: u32,
        selected: u32,
        wrap: bool,
    ) -> wasm::Result<*mut Object> {
        unsafe {
            let roller = roller_create(parent)?;
            roller_set_options(
                roller,
                two_digit_options(count).as_ptr(),
                if wrap {
                    RollerMode::Infinite
                } else {
                    RollerMode::Normal
                },
            )?;
            roller_set_visible_row_count(roller, if self.compact { 3 } else { 5 })?;
            roller_set_selected(roller, selected.min(count - 1), AnimEnable::Off)?;
            Ok(roller)
        }
    }

    unsafe fn label(
        &self,
        parent: *mut Object,
        text: &str,
        font: u32,
    ) -> wasm::Result<*mut Object> {
        unsafe {
            let label = label_create(parent)?;
            object_set_font(label, font, 0)?;
            Self::text(label, text)?;
            Ok(label)
        }
    }

    /// Full-width, centered label.
    unsafe fn centered(&self, parent: *mut Object, font: u32) -> wasm::Result<*mut Object> {
        unsafe {
            let label = self.label(parent, "", font)?;
            object_set_width(label, percentage(100)?)?;
            object_set_text_align(label, TextAlign::Center as u32, 0)?;
            Ok(label)
        }
    }

    unsafe fn text(label: *mut Object, text: &str) -> wasm::Result<()> {
        let text = CString::new(text).map_err(|_| -1)?;
        unsafe { label_set_text(label, text.as_ptr()) }
    }

    /// Transparent, centered, wrapping row.
    unsafe fn row(&self, parent: *mut Object) -> wasm::Result<*mut Object> {
        unsafe {
            let row = object_create(parent)?;
            object_set_background(row, 0, 0, 0)?;
            object_set_border(row, 0, 0, 0, 0)?;
            object_set_padding(row, 6)?;
            object_set_width(row, percentage(100)?)?;
            object_set_height(row, size_content())?;
            object_set_flex_flow(row, FlexFlow::RowWrap)?;
            object_set_flex_align(row, FlexAlign::Center, FlexAlign::Center, FlexAlign::Center)?;
            object_remove_flag(row, ObjectFlag::Scrollable)?;
            Ok(row)
        }
    }

    unsafe fn tab(&self, name: &CStr) -> wasm::Result<*mut Object> {
        unsafe {
            let tab = tabview_add_tab(self.tabs, name.as_ptr())?;
            object_set_flex_flow(tab, FlexFlow::Column)?;
            // Centered on wide screens; compact tabs overflow and must scroll
            // from the top, which centering would clip.
            let main = if self.compact {
                FlexAlign::Start
            } else {
                FlexAlign::Center
            };
            object_set_flex_align(tab, main, FlexAlign::Center, FlexAlign::Center)?;
            object_set_font(tab, FONT_MEDIUM, 0)?;
            object_set_padding(tab, 6)?;
            Ok(tab)
        }
    }

    unsafe fn button(
        &mut self,
        parent: *mut Object,
        text: &CStr,
        variant: u32,
        action: Action,
    ) -> wasm::Result<*mut Object> {
        unsafe {
            let button = button_create(parent)?;
            object_set_theme_variant(button, variant)?;
            let label = label_create(button)?;
            label_set_text(label, text.as_ptr())?;
            object_align(label, Align::Center, 0, 0)?;
            self.controls.push((button, action));
            Ok(button)
        }
    }

    unsafe fn set_hidden(object: *mut Object, hidden: bool) -> wasm::Result<()> {
        unsafe {
            if hidden {
                object_add_flag(object, ObjectFlag::Hidden)
            } else {
                object_remove_flag(object, ObjectFlag::Hidden)
            }
        }
    }

    /// Saves on the next loop turn, after the screen shows the change:
    /// a settings write can take far longer than a frame.
    fn persist(&mut self) {
        self.save_pending = true;
    }

    fn flush_save(&mut self) {
        if !core::mem::take(&mut self.save_pending) {
            return;
        }
        if let Some(store) = &mut self.store {
            self.persistence_error = match store.save(&self.settings) {
                Ok(()) => false,
                Err(error) => {
                    eprintln!("Clock settings could not be saved: {error}");
                    true
                }
            };
        }
    }

    fn tick(&mut self, now: u64, wall: Option<u64>) -> bool {
        let mut changed = self.timer.tick(now);
        if let (Some(alarm), Some(wall)) = (self.settings.alarm.as_mut(), wall)
            && alarm.tick(wall)
        {
            self.alarm_alert.ring();
            self.persist();
            changed = true;
        }
        self.alarm_alert.tick(now) || changed
    }

    unsafe fn act(&mut self, action: Action, now: u64, wall: Option<u64>) -> wasm::Result<()> {
        unsafe {
            match action {
                Action::AlarmDay(index) => {
                    self.selected_days[index] = !self.selected_days[index];
                    let variant = if self.selected_days[index] {
                        PRIMARY
                    } else {
                        OUTLINE
                    };
                    object_set_theme_variant(self.days[index], variant)?;
                }
                Action::AlarmSave => {
                    if let Some(wall) = wall {
                        let days = self
                            .selected_days
                            .iter()
                            .enumerate()
                            .filter(|(_, selected)| **selected)
                            .fold(0u8, |days, (index, _)| days | 1 << index);
                        let hour = roller_get_selected(self.alarm_hour)?.min(23) as u8;
                        let minute = roller_get_selected(self.alarm_minute)?.min(59) as u8;
                        let mut alarm = Alarm::new(hour, minute, days).unwrap();
                        alarm.arm(wall);
                        self.settings.alarm = Some(alarm);
                        self.alarm_alert.dismiss();
                        self.persist();
                    }
                }
                Action::AlarmDisable | Action::AlarmDelete => {
                    if let Some(alarm) = self.settings.alarm.as_mut() {
                        alarm.disable();
                    }
                    if matches!(action, Action::AlarmDelete) {
                        self.settings.alarm = None;
                    }
                    self.alarm_alert.dismiss();
                    self.persist();
                }
                Action::StopwatchToggle => {
                    if self.watch.is_running() {
                        self.watch.pause(now);
                    } else {
                        self.watch.start(now);
                    }
                }
                Action::Lap => {
                    self.watch.lap(now);
                }
                Action::StopwatchReset => self.watch.reset(),
                Action::TimerStart => {
                    let seconds = join_duration(
                        roller_get_selected(self.timer_rollers[0])?,
                        roller_get_selected(self.timer_rollers[1])?,
                        roller_get_selected(self.timer_rollers[2])?,
                    );
                    self.timer.start(u64::from(seconds) * 1000, now);
                }
                Action::TimerToggle => {
                    if self.timer.is_running() {
                        self.timer.pause(now);
                    } else {
                        self.timer.resume(now);
                    }
                }
                Action::TimerRestart => self.timer.restart(now),
                Action::TimerCancel => self.timer.cancel(),
                Action::Snooze => {
                    self.alarm_alert.snooze(now);
                }
                Action::Dismiss => {
                    self.alarm_alert.dismiss();
                    self.timer.acknowledge();
                }
            }
            Ok(())
        }
    }

    fn set_text(&mut self, slot: Text, text: String) -> wasm::Result<()> {
        let index = slot as usize;
        if self.rendered[index] != text {
            unsafe { Self::text(self.texts[index], &text)? };
            self.rendered[index] = text;
        }
        Ok(())
    }

    unsafe fn render(&mut self, now: u64, wall: Option<u64>) -> wasm::Result<()> {
        let mut alert = Vec::new();
        if self.alarm_alert.is_ringing() {
            alert.push(t!("alerts.alarm_ringing").trim());
        }
        if self.timer.finished() {
            alert.push(t!("alerts.timer_finished").trim());
        }
        if self.alarm_alert.is_snoozed() {
            alert.push(t!("alerts.alarm_snoozed").trim());
        }
        if self.persistence_error {
            alert.push(t!("errors.settings_save_failed").trim());
        }
        let shown = Shown {
            banner: Some(!alert.is_empty()),
            snooze: Some(self.alarm_alert.is_ringing()),
            timer_active: Some(self.timer.is_active()),
            laps: Some(!self.watch.laps().is_empty()),
        };
        self.set_text(Text::Alert, alert.join("\n"))?;
        unsafe {
            if shown.banner != self.shown.banner {
                Self::set_hidden(self.banner, alert.is_empty())?;
            }
            if shown.snooze != self.shown.snooze {
                Self::set_hidden(self.snooze, !self.alarm_alert.is_ringing())?;
            }
            if shown.laps != self.shown.laps {
                Self::set_hidden(self.laps_card, self.watch.laps().is_empty())?;
            }
            if shown.timer_active != self.shown.timer_active {
                let running = self.timer.is_active();
                Self::set_hidden(self.timer_picker, running)?;
                Self::set_hidden(self.timer_idle_actions, running)?;
                Self::set_hidden(self.timer_ring, !running)?;
                Self::set_hidden(self.timer_active_actions, !running)?;
            }
        }
        self.shown = shown;

        // Every view is refreshed; unchanged text costs no host call.
        {
            let (time, date) = wall.and_then(|wall| i64::try_from(wall).ok()).map_or_else(
                || {
                    (
                        t!("clock.errors.host_unavailable").to_string(),
                        String::new(),
                    )
                },
                |wall| {
                    let formatter = embedded_i18n::DateTimeFormatter::new(
                        embedded_i18n::date_time_locale(embedded_i18n::locale!()),
                    );
                    let pattern = if self.settings.twelve_hour {
                        "%I:%M:%S %p"
                    } else {
                        "%H:%M:%S"
                    };
                    let (mut time, mut date) = (String::new(), String::new());
                    let _ = formatter.write(&mut time, wall, pattern);
                    let _ = formatter.write(&mut date, wall, "%a %d %b %Y");
                    (time, date)
                },
            );
            self.set_text(Text::Time, time)?;
            self.set_text(Text::Date, date)?;
            if let (false, Some(wall)) = (self.clock_ring.is_null(), wall) {
                unsafe { range_set_value(self.clock_ring, (wall % 60) as i32, false)? };
            }
        }
        {
            let mut status = String::new();
            match self.settings.alarm {
                Some(saved) if saved.enabled() => {
                    status.push_str(t!("alarm.status.enabled"));
                    status.push_str(&format!("{:02}:{:02} UTC", saved.hour, saved.minute));
                    if let Some(next) = saved.next().and_then(|value| i64::try_from(value).ok()) {
                        status.push('\n');
                        let _ = embedded_i18n::DateTimeFormatter::new(
                            embedded_i18n::date_time_locale(embedded_i18n::locale!()),
                        )
                        .write(&mut status, next, "%a %d %b %H:%M");
                    }
                }
                Some(_) => status.push_str(t!("alarm.status.disabled")),
                None => status.push_str(t!("alarm.status.no_saved_alarm")),
            }
            self.set_text(Text::AlarmStatus, status)?;
        }
        {
            use core::fmt::Write;
            self.set_text(
                Text::Stopwatch,
                format_duration(self.watch.elapsed(now), true),
            )?;
            let mut laps = String::new();
            let count = self.watch.laps().len();
            if count != 0 {
                // Compiled locale forms; selection does not allocate.
                let _ = write!(
                    &mut laps,
                    "{}",
                    embedded_i18n::t!("stopwatch.laps.recorded", count = count)
                );
            }
            for (index, lap) in self.watch.laps().iter().enumerate().rev() {
                let _ = write!(laps, "\n{}: {}", index + 1, format_duration(*lap, true));
            }
            self.set_text(Text::Laps, laps)?;
        }
        if self.timer.is_active() {
            let remaining = self.timer.remaining(now);
            unsafe {
                range_set_value(
                    self.timer_ring,
                    remaining_permille(remaining, self.timer.duration()),
                    false,
                )?;
            }
            self.set_text(
                Text::TimerRemaining,
                format_duration(remaining.div_ceil(1000) * 1000, false),
            )?;
            self.set_text(
                Text::TimerDuration,
                format!(
                    "{}: {}",
                    t!("timer.labels.duration"),
                    format_duration(self.timer.duration(), false)
                ),
            )?;
        }
        Ok(())
    }
}

fn format_duration(milliseconds: u64, tenths: bool) -> String {
    let seconds = milliseconds / 1000;
    let mut text = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    if tenths {
        text.push_str(&format!(".{}", milliseconds % 1000 / 100));
    }
    text
}
