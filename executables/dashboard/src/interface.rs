//! Main screen: a header with the title, the time since the last refresh and
//! the errors, then a grid of panel tiles.

use core::ptr::null_mut;
use std::{
    thread::sleep,
    time::{Duration, Instant},
};

use dashboard::{
    configuration::{Configuration, Kind, Panel},
    error::Error,
    format::{format_value, level},
    grafana,
    model::Reload,
    net::device::Devices,
    query::CHART_POINTS,
    response::{PanelData, Series},
    storage::Storage,
    text::{age, error_message},
};
use embedded_i18n::t;
use wasm::*;

use crate::settings::{Context, Outcome, Settings};
use crate::widgets::*;

include!(concat!(env!("OUT_DIR"), "/shortcut_icon.rs"));

/// Windows narrower than this show two tiles per row instead of three.
const WIDE_MINIMUM_WIDTH: i32 = 400;
/// Series colours of a chart, in order.
const CHART_COLORS: [u32; 3] = [0x3B82F6, 0xF97316, 0xA855F7];
/// Chart values are scaled to this range, with a margin at both ends.
const CHART_TOP: i32 = 1_000;
const CHART_MARGIN: i32 = 60;
/// LVGL's marker for a missing chart point.
const POINT_NONE: i32 = i32::MAX;
const ERROR_COLOR: u32 = 0xEF4444;
const MUTED_COLOR: u32 = 0x71717A;
/// A tap within this time of the end of a refresh does not start another
/// one: taps made while the guest waited on the network are queued, and would
/// otherwise repeat the refresh back to back.
const TAP_REFRESH_INTERVAL: Duration = Duration::from_secs(3);

/// What a request needs of the configuration, without the cached panels.
fn connection(configuration: &Configuration) -> Configuration {
    Configuration {
        url: configuration.url.clone(),
        data_source: configuration.data_source.clone(),
        dashboard: configuration.dashboard.clone(),
        ..Configuration::default()
    }
}

/// A label pair for one series of a value panel with several series.
struct Row {
    container: *mut Object,
    name: *mut Object,
    value: *mut Object,
}

struct Chart {
    object: *mut Object,
    slots: [u32; 3],
    legend: [*mut Object; 3],
}

struct Tile {
    value: *mut Object,
    /// Holds a row per series when a value panel has several.
    rows_container: *mut Object,
    rows: Vec<Row>,
    chart: Option<Chart>,
    footer: *mut Object,
    error: *mut Object,
}

pub struct Interface {
    window: *mut Object,
    columns: i32,
    storage: Option<Storage>,
    configuration: Option<Configuration>,
    token: Option<String>,
    title: *mut Object,
    status: *mut Object,
    reload_button: *mut Object,
    settings_button: *mut Object,
    notice: *mut Object,
    grid: *mut Object,
    tiles: Vec<Tile>,
    panels: Vec<Panel>,
    last_refresh: Option<Instant>,
    /// End of the last refresh attempt, successful or not.
    refresh_ended: Option<Instant>,
    next_refresh: Option<Instant>,
    /// Error of the last refresh, or of reading the configuration.
    error: Option<Error>,
    /// Why the dashboard could not be reloaded; its cached panels show.
    reload_error: Option<Error>,
    shown_status: String,
    settings: Option<Settings>,
}

impl Interface {
    pub fn new() -> wasm::Result<Self> {
        unsafe {
            let window = window_create()?;
            window_set_icon(window, ICON_TEXT.as_ptr(), ICON_COLOR)?;
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_remove_flag(window, ObjectFlag::Scrollable)?;
            object_update_layout(window)?;
            let columns = if object_get_width(window)? < WIDE_MINIMUM_WIDTH {
                2
            } else {
                3
            };
            object_set_padding(window, 4)?;

            let header = flex(window, FlexFlow::Row)?;
            object_set_padding(header, 2)?;
            let title = label(header, t!("header.title"), FONT_LARGE)?;
            object_set_width(title, 0)?;
            object_set_flex_grow(title, 1)?;
            let status = label(header, "", FONT_SMALL)?;
            object_set_text_color(status, MUTED_COLOR, 0)?;
            let reload_button = button(header, SYMBOL_REFRESH, GHOST)?;
            let settings_button = button(header, SYMBOL_SETTINGS, GHOST)?;

            let notice = paragraph(window, "", FONT_SMALL)?;
            object_set_text_color(notice, ERROR_COLOR, 0)?;
            set_hidden(notice, true)?;

            let grid = Self::create_grid(window)?;

            Ok(Self {
                window,
                columns,
                storage: Storage::from_environment(),
                configuration: None,
                token: None,
                title,
                status,
                reload_button,
                settings_button,
                notice,
                grid,
                tiles: Vec::new(),
                panels: Vec::new(),
                last_refresh: None,
                refresh_ended: None,
                next_refresh: None,
                error: None,
                reload_error: None,
                shown_status: String::new(),
                settings: None,
            })
        }
    }

    unsafe fn create_grid(window: *mut Object) -> wasm::Result<*mut Object> {
        unsafe {
            let grid = container(window)?;
            object_set_width(grid, percentage(100)?)?;
            object_set_flex_grow(grid, 1)?;
            object_set_flex_flow(grid, FlexFlow::RowWrap)?;
            object_set_flex_align(grid, FlexAlign::Start, FlexAlign::Start, FlexAlign::Start)?;
            object_add_flag(grid, ObjectFlag::Scrollable)?;
            Ok(grid)
        }
    }

    /// Reads the files, imports the dashboard and shows the first values;
    /// opens the settings when there is no configuration yet.
    fn start(&mut self) -> wasm::Result<()> {
        match &self.storage {
            None => self.error = Some(Error::Storage),
            Some(storage) => {
                match storage.load() {
                    Ok(configuration) => self.configuration = configuration,
                    Err(error) => self.error = Some(error),
                }
                self.token = storage.token().ok().flatten();
            }
        }
        if self.configuration.is_none() {
            self.show_notice()?;
            if self.error.is_none() {
                return self.open_settings();
            }
            return Ok(());
        }
        if self
            .configuration
            .as_ref()
            .is_some_and(|configuration| configuration.dashboard.is_some())
        {
            self.reload_dashboard()?;
        }
        self.rebuild()?;
        self.refresh()
    }

    /// Shows `text` in the status line right away, before a blocking call.
    fn show_busy(&mut self, text: &str) -> wasm::Result<()> {
        unsafe {
            set_text(self.status, text)?;
            object_set_text_color(self.status, MUTED_COLOR, 0)?;
        }
        self.shown_status = String::from(text);
        // Let the display catch up before the guest blocks on the network.
        sleep(Duration::from_millis(30));
        Ok(())
    }

    fn reload_dashboard(&mut self) -> wasm::Result<()> {
        let Some(configuration) = self.configuration.as_mut() else {
            return Ok(());
        };
        if configuration.dashboard.is_none() {
            return Ok(());
        }
        let Some(token) = self.token.clone() else {
            self.reload_error = Some(Error::MissingToken);
            return Ok(());
        };
        let configuration_snapshot = connection(configuration);
        self.show_busy(t!("header.importing"))?;
        let result = grafana::import(&mut Devices, &configuration_snapshot, &token);
        let configuration = self.configuration.as_mut().expect("checked above");
        match configuration.apply_import(result) {
            Reload::Imported => {
                self.reload_error = None;
                if let Some(storage) = &self.storage
                    && let Err(error) = storage.save(configuration)
                {
                    self.error = Some(error);
                }
            }
            Reload::Failed(error) => self.reload_error = Some(error),
        }
        Ok(())
    }

    /// Recreates the header title and the tiles from the configuration.
    fn rebuild(&mut self) -> wasm::Result<()> {
        unsafe {
            delete(self.grid)?;
            self.grid = Self::create_grid(self.window)?;
        }
        self.tiles.clear();
        self.panels.clear();
        let title = self
            .configuration
            .as_ref()
            .and_then(|configuration| configuration.current_import())
            .map(|imported| imported.title.clone())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| String::from(t!("header.title")));
        unsafe {
            set_text(self.title, &title)?;
            set_hidden(
                self.reload_button,
                self.configuration
                    .as_ref()
                    .is_none_or(|configuration| configuration.dashboard.is_none()),
            )?;
        }
        if let Some(configuration) = &self.configuration {
            self.panels = configuration
                .visible_panels()
                .into_iter()
                .cloned()
                .collect();
        }
        for index in 0..self.panels.len() {
            let tile = unsafe { self.create_tile(index)? };
            self.tiles.push(tile);
        }
        self.show_notice()
    }

    unsafe fn create_tile(&self, index: usize) -> wasm::Result<Tile> {
        let panel = &self.panels[index];
        let chart = panel.kind() == Kind::Chart;
        unsafe {
            let card = card_create(self.grid)?;
            let width = match (self.columns, chart) {
                (3, false) => 31,
                (3, true) => 63,
                (_, false) => 48,
                (_, true) => 97,
            };
            object_set_size(card, percentage(width)?, size_content())?;
            object_set_flex_flow(card, FlexFlow::Column)?;
            object_set_padding(card, 6)?;
            object_remove_flag(card, ObjectFlag::Scrollable)?;
            object_set_min_max_size(card, 0, 16_384, 84, 16_384, 0)?;
            paragraph(card, &panel.title, FONT_SMALL)?;
            let value = paragraph(card, "-", if chart { FONT_LARGE } else { FONT_EXTRA_LARGE })?;
            let rows_container = flex(card, FlexFlow::Column)?;
            let chart = if chart {
                let legend = flex(card, FlexFlow::Row)?;
                let mut labels = [null_mut(); 3];
                for (label_slot, color) in labels.iter_mut().zip(CHART_COLORS) {
                    *label_slot = label(legend, "", FONT_SMALL)?;
                    object_set_text_color(*label_slot, color, 0)?;
                    set_hidden(*label_slot, true)?;
                }
                let object = chart_create(card)?;
                object_set_size(
                    object,
                    percentage(100)?,
                    if self.columns == 3 { 72 } else { 56 },
                )?;
                chart_set_type(object, ChartType::Line)?;
                chart_set_point_count(object, CHART_POINTS)?;
                chart_set_axis_range(object, ChartAxis::PrimaryY, 0, CHART_TOP)?;
                let mut slots = [0; 3];
                for (slot, color) in slots.iter_mut().zip(CHART_COLORS) {
                    *slot = chart_add_series_slot(object, color)?;
                }
                Some(Chart {
                    object,
                    slots,
                    legend: labels,
                })
            } else {
                None
            };
            let footer = paragraph(card, "", FONT_SMALL)?;
            object_set_text_color(footer, MUTED_COLOR, 0)?;
            set_hidden(footer, true)?;
            let error = paragraph(card, "", FONT_SMALL)?;
            object_set_text_color(error, ERROR_COLOR, 0)?;
            set_hidden(error, true)?;
            Ok(Tile {
                value,
                rows_container,
                rows: Vec::new(),
                chart,
                footer,
                error,
            })
        }
    }

    /// Runs every panel query and updates the tiles.
    fn refresh(&mut self) -> wasm::Result<()> {
        let period = self
            .configuration
            .as_ref()
            .map_or(60, |configuration| configuration.refresh_seconds());
        self.next_refresh = Some(Instant::now() + Duration::from_secs(u64::from(period)));
        let Some(configuration) = &self.configuration else {
            return Ok(());
        };
        if self.panels.is_empty() {
            return Ok(());
        }
        let Some(token) = self.token.clone() else {
            self.error = Some(Error::MissingToken);
            return Ok(());
        };
        let configuration = connection(configuration);
        self.show_busy(t!("header.loading"))?;
        let panels: Vec<&Panel> = self.panels.iter().collect();
        match grafana::query(&mut Devices, &configuration, &token, &panels) {
            Ok(data) => {
                drop(panels);
                for index in 0..self.tiles.len() {
                    let data = data.get(index).cloned().unwrap_or_default();
                    unsafe { self.update_tile(index, &data)? };
                }
                self.error = None;
                self.last_refresh = Some(Instant::now());
            }
            Err(error) => self.error = Some(error),
        }
        // The period counts from the end of the refresh.
        let ended = Instant::now();
        self.refresh_ended = Some(ended);
        self.next_refresh = Some(ended + Duration::from_secs(u64::from(period)));
        self.shown_status.clear();
        Ok(())
    }

    unsafe fn set_value(label: *mut Object, value: Option<f64>, panel: &Panel) -> wasm::Result<()> {
        unsafe {
            match value {
                Some(value) => {
                    set_text(label, &format_value(value, panel.unit(), panel.decimals))?;
                    if let Some(color) = level(value, panel.warning, panel.critical).color() {
                        object_set_text_color(label, color, 0)?;
                    }
                }
                None => set_text(label, "-")?,
            }
        }
        Ok(())
    }

    unsafe fn update_tile(&mut self, index: usize, data: &PanelData) -> wasm::Result<()> {
        let panel = self.panels[index].clone();
        let tile = &mut self.tiles[index];
        unsafe {
            set_hidden(tile.error, data.error.is_none())?;
            if let Some(error) = &data.error {
                set_text(tile.error, error)?;
            }
            let mut footer = String::new();
            if data.series.is_empty() && data.error.is_none() {
                footer.push_str(t!("tile.no_data"));
            }
            match &tile.chart {
                None if data.series.len() > 1 => {
                    set_hidden(tile.value, true)?;
                    while tile.rows.len() < data.series.len() {
                        let container = flex(tile.rows_container, FlexFlow::Row)?;
                        let name = label(container, "", FONT_SMALL)?;
                        object_set_width(name, 0)?;
                        object_set_flex_grow(name, 1)?;
                        let value = label(container, "", FONT_LARGE)?;
                        tile.rows.push(Row {
                            container,
                            name,
                            value,
                        });
                    }
                    for (position, row) in tile.rows.iter().enumerate() {
                        let series = data.series.get(position);
                        set_hidden(row.container, series.is_none())?;
                        if let Some(series) = series {
                            set_text(row.name, &series.name)?;
                            Self::set_value(row.value, series.last, &panel)?;
                        }
                    }
                }
                None => {
                    set_hidden(tile.value, false)?;
                    for row in &tile.rows {
                        set_hidden(row.container, true)?;
                    }
                    Self::set_value(
                        tile.value,
                        data.series.first().and_then(|series| series.last),
                        &panel,
                    )?;
                }
                Some(chart) => {
                    Self::set_value(
                        tile.value,
                        data.series.first().and_then(|series| series.last),
                        &panel,
                    )?;
                    let named = data.series.len() > 1
                        || data.series.iter().any(|series| !series.name.is_empty());
                    for (position, legend) in chart.legend.iter().enumerate() {
                        let series = data.series.get(position).filter(|_| named);
                        set_hidden(*legend, series.is_none())?;
                        if let Some(series) = series {
                            set_text(*legend, &series.name)?;
                        }
                    }
                    if let Some((minimum, maximum)) = Self::draw_chart(chart, &data.series)? {
                        footer = t!(
                            "tile.range",
                            minimum = format_value(minimum, panel.unit(), panel.decimals),
                            maximum = format_value(maximum, panel.unit(), panel.decimals)
                        )
                        .to_string();
                    }
                }
            }
            if data.more > 0 {
                if !footer.is_empty() {
                    footer.push_str("  ");
                }
                footer.push_str(&t!("tile.more", count = data.more).to_string());
            }
            set_hidden(tile.footer, footer.is_empty())?;
            set_text(tile.footer, &footer)?;
        }
        Ok(())
    }

    /// Draws the series scaled to their common range, newest points on the
    /// right, and returns that range.
    unsafe fn draw_chart(chart: &Chart, series: &[Series]) -> wasm::Result<Option<(f64, f64)>> {
        let points = series
            .iter()
            .map(|series| series.points.len())
            .max()
            .unwrap_or(0)
            .max(2);
        let range = series.iter().flat_map(|series| series.points.iter()).fold(
            None,
            |range: Option<(f32, f32)>, value| match range {
                None => Some((*value, *value)),
                Some((low, high)) => Some((low.min(*value), high.max(*value))),
            },
        );
        unsafe {
            chart_set_point_count(chart.object, points as u32)?;
            let mut values = vec![POINT_NONE; points];
            for (position, slot) in chart.slots.iter().enumerate() {
                values.fill(POINT_NONE);
                if let (Some(series), Some((low, high))) = (series.get(position), range) {
                    let offset = points - series.points.len();
                    let span = high - low;
                    for (index, value) in series.points.iter().enumerate() {
                        let fraction = if span > 0.0 {
                            (value - low) / span
                        } else {
                            0.5
                        };
                        values[offset + index] = CHART_MARGIN
                            + (fraction * (CHART_TOP - 2 * CHART_MARGIN) as f32) as i32;
                    }
                }
                chart_set_values(chart.object, *slot, values.as_ptr(), values.len())?;
            }
        }
        Ok(range.map(|(low, high)| (f64::from(low), f64::from(high))))
    }

    fn show_notice(&mut self) -> wasm::Result<()> {
        let text = if let Some(error) = &self.reload_error {
            Some(t!("main.not_reloaded", error = error_message(error)).to_string())
        } else if self.configuration.is_none() {
            Some(String::from(t!("main.not_configured")))
        } else if self.panels.is_empty() {
            Some(String::from(t!("main.no_panels")))
        } else {
            None
        };
        unsafe {
            set_hidden(self.notice, text.is_none())?;
            set_text(self.notice, text.as_deref().unwrap_or(""))?;
        }
        Ok(())
    }

    /// Status line: the error of the last refresh, else the time since it.
    fn update_status(&mut self) -> wasm::Result<()> {
        let (text, color) = match (&self.error, self.last_refresh) {
            (Some(error), _) => (error_message(error), ERROR_COLOR),
            (None, Some(last)) => (
                t!("header.updated", age = age(last.elapsed().as_secs())).to_string(),
                MUTED_COLOR,
            ),
            (None, None) => (String::new(), MUTED_COLOR),
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

    fn open_settings(&mut self) -> wasm::Result<()> {
        let token_saved = self.token.is_some();
        let settings =
            unsafe { Settings::open(self.window, self.configuration.as_ref(), token_saved)? };
        self.settings = Some(settings);
        Ok(())
    }

    fn close_settings(&mut self) -> wasm::Result<()> {
        if let Some(settings) = self.settings.take() {
            unsafe { settings.close()? };
        }
        Ok(())
    }

    /// Applies a configuration saved by the settings.
    fn apply_saved(&mut self, configuration: Configuration) -> wasm::Result<()> {
        let previous_dashboard = self
            .configuration
            .as_ref()
            .and_then(|configuration| configuration.dashboard.clone());
        self.token = self
            .storage
            .as_ref()
            .and_then(|storage| storage.token().ok().flatten());
        let reload = configuration.dashboard.is_some()
            && (configuration.dashboard != previous_dashboard
                || configuration.current_import().is_none());
        self.configuration = Some(configuration);
        self.error = None;
        self.reload_error = None;
        self.last_refresh = None;
        if reload {
            self.reload_dashboard()?;
        }
        self.rebuild()?;
        self.refresh()
    }

    fn handle(&mut self, code: u32, target: *mut Object) -> wasm::Result<()> {
        if let Some(settings) = &mut self.settings {
            let mut context = Context {
                storage: self.storage.as_ref(),
                token: self.token.as_deref(),
            };
            match unsafe { settings.handle(code, target, &mut context)? } {
                Outcome::Stay => (),
                Outcome::Close => {
                    self.close_settings()?;
                    self.show_notice()?;
                }
                Outcome::Saved(configuration) => {
                    self.close_settings()?;
                    self.apply_saved(*configuration)?;
                }
            }
            return Ok(());
        }
        if code != EventCode::Clicked as u32 {
            return Ok(());
        }
        if target == self.settings_button {
            self.open_settings()
        } else if target == self.reload_button {
            self.reload_dashboard()?;
            self.rebuild()?;
            self.refresh()
        } else if self
            .refresh_ended
            .is_some_and(|ended| ended.elapsed() < TAP_REFRESH_INTERVAL)
        {
            Ok(())
        } else {
            // A tap anywhere else refreshes the values.
            self.refresh()
        }
    }

    pub fn run(&mut self) -> wasm::Result<()> {
        self.start()?;
        loop {
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
                    unsafe { delete(self.window)? };
                    return Ok(());
                }
                if code == EventCode::All as u32 {
                    break;
                }
                self.handle(code, target)?;
            }
            if self.settings.is_none()
                && self.next_refresh.is_some_and(|next| Instant::now() >= next)
            {
                self.refresh()?;
            }
            if self.settings.is_none() {
                self.update_status()?;
            }
            sleep(Duration::from_millis(50));
        }
    }
}
