use crate::model::{self, MemorySample};
use alloc::{collections::VecDeque, ffi::CString, format};
use core::{ffi::c_char, num::NonZeroUsize};
use xila::graphics::{self, EventKind, OwnedWindow, fonts, lvgl, theme};
use xila::internationalization::t;

pub enum Action {
    Close,
    Workload(bool),
    Interval(u64),
}

/// Refresh periods offered by the segmented control, in milliseconds.
pub const INTERVALS: [u64; 3] = [250, 1000, 2000];

struct ButtonMap([*const c_char; 4]);
// The map only points at immutable string literals.
unsafe impl Sync for ButtonMap {}
static INTERVAL_MAP: ButtonMap = ButtonMap([
    c"250 ms".as_ptr(),
    c"1 s".as_ptr(),
    c"2 s".as_ptr(),
    c"".as_ptr(),
]);

/// Samples kept for the memory chart.
const HISTORY: usize = 60;
/// Windows smaller than this use the dense layout.
const COMPACT: (i32, i32) = (560, 360);
const MUTED: u8 = 150;

pub struct View {
    graphics: &'static graphics::Manager,
    window: OwnedWindow,
    uptime: *mut lvgl::lv_obj_t,
    tasks: *mut lvgl::lv_obj_t,
    memory: *mut lvgl::lv_obj_t,
    share: *mut lvgl::lv_obj_t,
    bar: *mut lvgl::lv_obj_t,
    chart: *mut lvgl::lv_obj_t,
    series: *mut lvgl::lv_chart_series_t,
    range: *mut lvgl::lv_obj_t,
    interval: *mut lvgl::lv_obj_t,
    toggle: *mut lvgl::lv_obj_t,
    status: *mut lvgl::lv_obj_t,
    history: VecDeque<usize>,
}

impl View {
    pub async fn new(graphics: &'static graphics::Manager) -> Result<Self, NonZeroUsize> {
        // Drop the lock before OwnedWindow on every construction error.
        let mut window;
        let _lock = graphics.lock().await;
        window = graphics.create_window().await.map_err(|_| crate::code(1))?;
        window.set_icon("Ac", graphics::Color::BLACK);
        let root = window.as_object_mutable();
        unsafe {
            lvgl::lv_obj_update_layout(root);
            let compact = lvgl::lv_obj_get_width(root) < COMPACT.0
                || lvgl::lv_obj_get_height(root) < COMPACT.1;
            let gap = if compact { 8 } else { 12 };
            lvgl::lv_obj_set_layout(root, lvgl::lv_layout_t_LV_LAYOUT_FLEX);
            lvgl::lv_obj_set_flex_flow(root, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
            lvgl::lv_obj_set_style_pad_all(root, if compact { 8 } else { 16 }, 0);
            lvgl::lv_obj_set_style_pad_row(root, gap, 0);

            // Title on the left, refresh period on the right.
            let header = plain(root)?;
            lvgl::lv_obj_set_flex_flow(header, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
            lvgl::lv_obj_set_flex_align(
                header,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_START,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            );
            lvgl::lv_obj_set_style_pad_column(header, gap, 0);
            let titles = plain(header)?;
            lvgl::lv_obj_set_flex_grow(titles, 1);
            lvgl::lv_obj_set_flex_flow(titles, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
            lvgl::lv_obj_set_style_pad_row(titles, 2, 0);
            let title = label(titles, t!("app.title"))?;
            lvgl::lv_obj_set_style_text_font(title, fonts::get_font_large(), 0);
            if !compact {
                caption(titles, t!("app.sources"))?;
            }
            let interval = lvgl::lv_buttonmatrix_create(header);
            if interval.is_null() {
                return Err(crate::code(1));
            }
            lvgl::lv_buttonmatrix_set_map(interval, INTERVAL_MAP.0.as_ptr());
            lvgl::lv_buttonmatrix_set_button_ctrl_all(
                interval,
                lvgl::lv_buttonmatrix_ctrl_t_LV_BUTTONMATRIX_CTRL_CHECKABLE,
            );
            lvgl::lv_buttonmatrix_set_one_checked(interval, true);
            lvgl::lv_obj_set_size(
                interval,
                if compact { 150 } else { 228 },
                if compact { 32 } else { 40 },
            );
            lvgl::lv_obj_set_style_pad_all(interval, 3, 0);
            lvgl::lv_obj_set_style_pad_column(interval, 3, 0);
            lvgl::lv_obj_set_style_text_font(interval, fonts::get_font_small(), 0);
            lvgl::lv_obj_add_flag(interval, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_EVENT_BUBBLE);

            // One row of metric cards; the memory card is wider when there is room.
            let metrics = plain(root)?;
            lvgl::lv_obj_set_flex_flow(metrics, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
            lvgl::lv_obj_set_style_pad_column(metrics, gap, 0);
            let value_font = if compact {
                fonts::get_font_large()
            } else {
                fonts::get_font_extra_large()
            };
            let mut cards = [core::ptr::null_mut(); 3];
            let mut values = [core::ptr::null_mut(); 3];
            for (index, title) in [t!("cards.uptime"), t!("cards.tasks"), t!("cards.memory")]
                .into_iter()
                .enumerate()
            {
                let card = card(metrics, compact)?;
                lvgl::lv_obj_set_flex_grow(card, if index == 2 && !compact { 2 } else { 1 });
                caption(card, title)?;
                let value = label(card, "")?;
                lvgl::lv_obj_set_style_text_font(value, value_font, 0);
                cards[index] = card;
                values[index] = value;
            }
            let share = caption(cards[2], "")?;
            let bar = lvgl::lv_bar_create(cards[2]);
            if bar.is_null() {
                return Err(crate::code(1));
            }
            lvgl::lv_obj_set_size(bar, lvgl::lv_pct(100), 6);
            lvgl::lv_bar_set_range(bar, 0, 1000);
            // Flex rows do not stretch their children: match the tallest card.
            lvgl::lv_obj_update_layout(metrics);
            let height = cards
                .map(|card| lvgl::lv_obj_get_height(card))
                .into_iter()
                .max();
            for card in cards {
                lvgl::lv_obj_set_height(card, height.unwrap_or(lvgl::LV_SIZE_CONTENT));
            }

            // Recent allocator usage, scaled to the visible samples.
            let history = card(root, compact)?;
            lvgl::lv_obj_set_flex_grow(history, 1);
            lvgl::lv_obj_set_style_min_height(history, if compact { 96 } else { 140 }, 0);
            let heading = plain(history)?;
            lvgl::lv_obj_set_flex_flow(heading, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
            let name = caption(heading, t!("cards.history"))?;
            lvgl::lv_obj_set_flex_grow(name, 1);
            let range = caption(heading, "")?;
            lvgl::lv_obj_set_width(range, lvgl::LV_SIZE_CONTENT);
            let chart = lvgl::lv_chart_create(history);
            if chart.is_null() {
                return Err(crate::code(1));
            }
            lvgl::lv_obj_set_width(chart, lvgl::lv_pct(100));
            lvgl::lv_obj_set_flex_grow(chart, 1);
            lvgl::lv_obj_set_style_border_width(chart, 0, 0);
            lvgl::lv_obj_set_style_pad_all(chart, 0, 0);
            lvgl::lv_obj_set_style_bg_opa(chart, lvgl::LV_OPA_TRANSP, 0);
            lvgl::lv_obj_set_style_size(chart, 0, 0, lvgl::LV_PART_INDICATOR);
            lvgl::lv_obj_set_style_line_width(chart, 2, lvgl::LV_PART_ITEMS);
            lvgl::lv_chart_set_type(chart, lvgl::lv_chart_type_t_LV_CHART_TYPE_LINE);
            lvgl::lv_chart_set_point_count(chart, HISTORY as u32);
            lvgl::lv_chart_set_div_line_count(chart, 3, 0);
            lvgl::lv_chart_set_update_mode(
                chart,
                lvgl::lv_chart_update_mode_t_LV_CHART_UPDATE_MODE_SHIFT,
            );
            let series = lvgl::lv_chart_add_series(
                chart,
                primary(chart),
                lvgl::lv_chart_axis_t_LV_CHART_AXIS_PRIMARY_Y,
            );
            if series.is_null() {
                return Err(crate::code(1));
            }

            // The bounded demo workload is a state, so it gets a switch.
            let demo = card(root, compact)?;
            lvgl::lv_obj_set_flex_flow(demo, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
            lvgl::lv_obj_set_flex_align(
                demo,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_START,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            );
            lvgl::lv_obj_set_style_pad_column(demo, gap, 0);
            let texts = plain(demo)?;
            lvgl::lv_obj_set_flex_grow(texts, 1);
            lvgl::lv_obj_set_flex_flow(texts, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
            lvgl::lv_obj_set_style_pad_row(texts, 2, 0);
            label(texts, t!("cards.demo"))?;
            let status = caption(texts, "")?;
            let toggle = lvgl::lv_switch_create(demo);
            if toggle.is_null() {
                return Err(crate::code(1));
            }
            lvgl::lv_obj_add_flag(toggle, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_EVENT_BUBBLE);

            if !compact {
                caption(root, t!("metrics.unsupported"))?;
            }
            Ok(Self {
                graphics,
                window,
                uptime: values[0],
                tasks: values[1],
                memory: values[2],
                share,
                bar,
                chart,
                series,
                range,
                interval,
                toggle,
                status,
                history: VecDeque::with_capacity(HISTORY),
            })
        }
    }

    pub async fn action(&mut self) -> Option<Action> {
        let _lock = self.graphics.lock().await;
        while let Some(event) = self.window.pop_event() {
            if matches!(event.code, EventKind::Delete | EventKind::CloseRequested)
                && event.target == self.window.as_object_mutable()
            {
                return Some(Action::Close);
            }
            if event.code == EventKind::ValueChanged {
                if event.target == self.toggle {
                    let on = unsafe { lvgl::lv_obj_has_state(self.toggle, lvgl::LV_STATE_CHECKED) };
                    return Some(Action::Workload(on));
                }
                if event.target == self.interval {
                    let index = unsafe { lvgl::lv_buttonmatrix_get_selected_button(self.interval) };
                    if let Some(interval) = INTERVALS.get(index as usize) {
                        return Some(Action::Interval(*interval));
                    }
                }
            }
        }
        None
    }

    pub async fn refresh(&mut self, uptime: u64, tasks: usize, memory: MemorySample) {
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(memory.used);
        let (low, high) = model::chart_range(self.history.iter().copied());
        let _lock = self.graphics.lock().await;
        unsafe {
            text(self.uptime, &model::format_uptime(uptime));
            text(self.tasks, &format!("{tasks}"));
            text(self.memory, &model::format_bytes(memory.used));
            match memory.capacity {
                Some(capacity) => {
                    let permille = model::permille(memory.used, capacity);
                    text(
                        self.share,
                        &format!(
                            "{}",
                            t!(
                                "cards.memory_share",
                                arg0 = format!("{}.{}", permille / 10, permille % 10),
                                arg1 = model::format_bytes(capacity)
                            )
                        ),
                    );
                    lvgl::lv_bar_set_value(self.bar, permille as i32, false);
                    lvgl::lv_obj_remove_flag(self.bar, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                }
                None => {
                    text(self.share, t!("metrics.capacity_unavailable"));
                    lvgl::lv_obj_add_flag(self.bar, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                }
            }
            text(
                self.range,
                &format!(
                    "{} - {}",
                    model::format_bytes(low),
                    model::format_bytes(high)
                ),
            );
            // The series follows the palette when the appearance changes.
            lvgl::lv_chart_set_series_color(self.chart, self.series, primary(self.chart));
            lvgl::lv_chart_set_axis_range(
                self.chart,
                lvgl::lv_chart_axis_t_LV_CHART_AXIS_PRIMARY_Y,
                model::chart_point(low),
                model::chart_point(high),
            );
            lvgl::lv_chart_set_next_value(self.chart, self.series, model::chart_point(memory.used));
        }
    }

    pub async fn controls(&mut self, interval: u64, running: bool, failed: bool) {
        let _lock = self.graphics.lock().await;
        unsafe {
            for (index, value) in INTERVALS.iter().enumerate() {
                let control = lvgl::lv_buttonmatrix_ctrl_t_LV_BUTTONMATRIX_CTRL_CHECKED;
                if *value == interval {
                    lvgl::lv_buttonmatrix_set_button_ctrl(self.interval, index as u32, control);
                } else {
                    lvgl::lv_buttonmatrix_clear_button_ctrl(self.interval, index as u32, control);
                }
            }
            lvgl::lv_obj_set_state(self.toggle, lvgl::LV_STATE_CHECKED, running);
            text(
                self.status,
                if failed {
                    t!("workload.failed")
                } else if running {
                    t!("workload.running")
                } else {
                    t!("cards.demo_detail")
                },
            );
            lvgl::lv_theme_shadcn_set_error(self.status, failed);
        }
    }

    pub async fn wait(&mut self, duration: core::time::Duration) {
        embassy_futures::select::select(
            xila::task::suspend(|context| self.window.register_waker(context.waker())),
            xila::task::sleep(duration),
        )
        .await;
    }
}

// All helpers are called under the graphics lock, with live widget pointers.
unsafe fn primary(object: *mut lvgl::lv_obj_t) -> lvgl::lv_color_t {
    unsafe {
        theme::get_palette_color(object, theme::PaletteColor::Primary)
            .unwrap_or(graphics::Color::WHITE)
            .into_lvgl_color()
    }
}

/// A layout-only container: full width, content height, no chrome.
unsafe fn plain(parent: *mut lvgl::lv_obj_t) -> Result<*mut lvgl::lv_obj_t, NonZeroUsize> {
    unsafe {
        let object = lvgl::lv_obj_create(parent);
        if object.is_null() {
            return Err(crate::code(1));
        }
        lvgl::lv_obj_set_size(object, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
        lvgl::lv_obj_set_style_bg_opa(object, lvgl::LV_OPA_TRANSP, 0);
        lvgl::lv_obj_set_style_border_width(object, 0, 0);
        lvgl::lv_obj_set_style_pad_all(object, 0, 0);
        lvgl::lv_obj_remove_flag(object, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
        Ok(object)
    }
}

unsafe fn card(
    parent: *mut lvgl::lv_obj_t,
    compact: bool,
) -> Result<*mut lvgl::lv_obj_t, NonZeroUsize> {
    unsafe {
        let card = lvgl::lv_obj_create(parent);
        if card.is_null() {
            return Err(crate::code(1));
        }
        theme::set_surface(card, theme::Surface::Card);
        lvgl::lv_obj_set_size(card, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
        lvgl::lv_obj_set_flex_flow(card, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
        lvgl::lv_obj_set_style_pad_all(card, if compact { 8 } else { 14 }, 0);
        lvgl::lv_obj_set_style_pad_row(card, if compact { 2 } else { 6 }, 0);
        lvgl::lv_obj_remove_flag(card, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
        Ok(card)
    }
}

unsafe fn label(
    parent: *mut lvgl::lv_obj_t,
    value: &str,
) -> Result<*mut lvgl::lv_obj_t, NonZeroUsize> {
    unsafe {
        let label = lvgl::lv_label_create(parent);
        if label.is_null() {
            return Err(crate::code(1));
        }
        lvgl::lv_obj_set_width(label, lvgl::lv_pct(100));
        text(label, value);
        Ok(label)
    }
}

/// Secondary text: small and dimmed through opacity, so it follows the palette.
unsafe fn caption(
    parent: *mut lvgl::lv_obj_t,
    value: &str,
) -> Result<*mut lvgl::lv_obj_t, NonZeroUsize> {
    unsafe {
        let label = label(parent, value)?;
        lvgl::lv_obj_set_style_text_font(label, fonts::get_font_small(), 0);
        lvgl::lv_obj_set_style_text_opa(label, MUTED, 0);
        Ok(label)
    }
}

unsafe fn text(label: *mut lvgl::lv_obj_t, value: &str) {
    // Catalog text and numeric substitutions contain no interior NULs.
    if let Ok(value) = CString::new(value) {
        unsafe {
            lvgl::lv_label_set_text(label, value.as_ptr());
        }
    }
}
