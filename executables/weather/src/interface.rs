use core::ffi::CStr;
use core::ptr::null_mut;
use std::{ffi::CString, thread::sleep};

use wasm::*;

use weather::{
    model::{CurrentWeather, DailyWeather, HourlyWeather},
    state::{WeatherData, fetch_weather, map_status_message},
    trigger::should_refresh,
    view::{chart_series, compass, condition, day_label, hour_label},
};

/// shadcn button variant, numbered as `lv_theme_shadcn_variant_t`.
const PRIMARY: u32 = 0;

const FONT_SMALL: u32 = 0;
const FONT_MEDIUM: u32 = 1;
const FONT_LARGE: u32 = 2;
const FONT_EXTRA_LARGE: u32 = 3;
const SURFACE_MUTED: u32 = 3;

/// Windows narrower or shorter than this use the compact arrangement.
const WIDE_MINIMUM_WIDTH: i32 = 560;
const WIDE_MINIMUM_HEIGHT: i32 = 360;

const HOURS: usize = 24;
const DAYS: usize = 10;

/// Chart colours: temperature and rain chance.
const TEMPERATURE_RGB: u32 = 0xF97316;
const RAIN_RGB: u32 = 0x3B82F6;

/// One stat tile: caption and value.
struct Tile {
    value: *mut Object,
}

pub struct Interface {
    window: *mut Object,
    city_input: *mut Object,
    refresh_button: *mut Object,
    spinner: *mut Object,
    status_row: *mut Object,
    status_label: *mut Object,
    compact: bool,
    // Now
    empty_state: *mut Object,
    hero: *mut Object,
    location: *mut Object,
    temperature: *mut Object,
    condition: *mut Object,
    feels_like: *mut Object,
    tiles: *mut Object,
    humidity: Tile,
    humidity_bar: *mut Object,
    wind: Tile,
    gusts: Tile,
    precipitation: Tile,
    details: *mut Object,
    // Hourly
    hourly_empty: *mut Object,
    hourly_content: *mut Object,
    hourly_range: *mut Object,
    temperature_chart: *mut Object,
    temperature_series: u32,
    rain_chart: *mut Object,
    rain_series: u32,
    // Daily
    daily_empty: *mut Object,
    daily_list: *mut Object,
    daily_rows: Vec<*mut Object>,
    in_flight: bool,
    buffer: Vec<i8>,
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

unsafe fn label(parent: *mut Object, text: &str, font: u32) -> wasm::Result<*mut Object> {
    unsafe {
        let label = label_create(parent)?;
        object_set_font(label, font, 0)?;
        set_text(label, text)?;
        Ok(label)
    }
}

unsafe fn set_text(label: *mut Object, text: &str) -> wasm::Result<()> {
    let text = CString::new(text).map_err(|_| -1)?;
    unsafe { label_set_text(label, text.as_ptr()) }
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

impl Interface {
    pub fn new() -> wasm::Result<Self> {
        unsafe {
            let window = window_create()?;
            window_set_icon(
                window,
                c"We".as_ptr(),
                Color {
                    red: 14,
                    green: 165,
                    blue: 233,
                },
            )?;
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_remove_flag(window, ObjectFlag::Scrollable)?;
            object_update_layout(window)?;
            let compact = object_get_width(window)? < WIDE_MINIMUM_WIDTH
                || object_get_height(window)? < WIDE_MINIMUM_HEIGHT;
            object_set_padding(window, if compact { 2 } else { 6 })?;

            // Search: city field and the primary action.
            let search = container(window)?;
            object_set_size(search, percentage(100)?, size_content())?;
            object_set_flex_flow(search, FlexFlow::Row)?;
            object_set_flex_align(
                search,
                FlexAlign::Start,
                FlexAlign::Center,
                FlexAlign::Center,
            )?;
            object_set_padding(search, 6)?;
            let city_input = textarea_create(search)?;
            textarea_set_one_line(city_input, true)?;
            object_set_width(city_input, 0)?;
            object_set_flex_grow(city_input, 1)?;
            let refresh_button = button_create(search)?;
            object_set_theme_variant(refresh_button, PRIMARY)?;
            let refresh_label = label_create(refresh_button)?;
            label_set_text(refresh_label, c"Refresh".as_ptr())?;
            object_align(refresh_label, Align::Center, 0, 0)?;

            // Status: spinner while a request runs, then the outcome.
            let status_row = container(window)?;
            object_set_size(status_row, percentage(100)?, size_content())?;
            object_set_flex_flow(status_row, FlexFlow::Row)?;
            object_set_flex_align(
                status_row,
                FlexAlign::Start,
                FlexAlign::Center,
                FlexAlign::Center,
            )?;
            object_set_padding(status_row, 4)?;
            let spinner = spinner_create(status_row)?;
            object_set_size(spinner, 16, 16)?;
            object_add_flag(spinner, ObjectFlag::Hidden)?;
            let status_label = label(status_row, "", FONT_SMALL)?;
            // Only loading and errors need a line; the empty state explains the rest.
            object_add_flag(status_row, ObjectFlag::Hidden)?;

            let tabs = tabview_create(window)?;
            object_set_width(tabs, percentage(100)?)?;
            object_set_flex_grow(tabs, 1)?;
            if compact {
                object_set_font(tabs, FONT_SMALL, 0)?;
            }
            let tab = |name: &CStr| -> wasm::Result<*mut Object> {
                let page = tabview_add_tab(tabs, name.as_ptr())?;
                object_set_flex_flow(page, FlexFlow::Column)?;
                object_set_flex_align(
                    page,
                    FlexAlign::Start,
                    FlexAlign::Center,
                    FlexAlign::Center,
                )?;
                object_set_font(page, FONT_MEDIUM, 0)?;
                object_set_padding(page, 6)?;
                Ok(page)
            };
            let now_tab = tab(c"Now")?;
            let hourly_tab = tab(c"Hourly")?;
            let daily_tab = tab(c"Daily")?;

            let empty = |page: *mut Object| -> wasm::Result<*mut Object> {
                let text = label(page, "Search for a city to see its weather.", FONT_MEDIUM)?;
                object_set_width(text, percentage(100)?)?;
                object_set_text_align(text, TextAlign::Center as u32, 0)?;
                Ok(text)
            };

            // Now: hero card, stat tiles, location details.
            let empty_state = empty(now_tab)?;
            let hero = card_create(now_tab)?;
            object_set_size(hero, percentage(100)?, size_content())?;
            object_set_flex_flow(hero, FlexFlow::Column)?;
            object_set_flex_align(hero, FlexAlign::Start, FlexAlign::Center, FlexAlign::Center)?;
            object_set_padding(hero, if compact { 6 } else { 12 })?;
            object_remove_flag(hero, ObjectFlag::Scrollable)?;
            let location = label(hero, "", FONT_MEDIUM)?;
            let temperature = label(hero, "", FONT_EXTRA_LARGE)?;
            let condition_label = label(hero, "", FONT_LARGE)?;
            let feels_like = label(hero, "", FONT_SMALL)?;

            let tiles = container(now_tab)?;
            object_set_size(tiles, percentage(100)?, size_content())?;
            object_set_flex_flow(tiles, FlexFlow::RowWrap)?;
            object_set_flex_align(
                tiles,
                FlexAlign::Center,
                FlexAlign::Center,
                FlexAlign::Center,
            )?;
            object_set_padding(tiles, 4)?;
            // Wide: four tiles in a row; compact: two per row.
            let tile_width = percentage(if compact { 48 } else { 24 })?;
            let tile = |caption: &str| -> wasm::Result<(*mut Object, Tile)> {
                let card = card_create(tiles)?;
                object_set_size(card, tile_width, size_content())?;
                object_set_flex_flow(card, FlexFlow::Column)?;
                object_set_padding(card, 8)?;
                object_remove_flag(card, ObjectFlag::Scrollable)?;
                // Equal heights even though only the humidity tile has a bar.
                object_set_min_max_size(card, 0, 16_384, 70, 16_384, 0)?;
                label(card, caption, FONT_SMALL)?;
                let value = label(card, "", FONT_LARGE)?;
                Ok((card, Tile { value }))
            };
            let (humidity_card, humidity) = tile("Humidity")?;
            let humidity_bar = bar_create(humidity_card)?;
            object_set_size(humidity_bar, percentage(100)?, 6)?;
            range_set_range(humidity_bar, 0, 100)?;
            let (_, wind) = tile("Wind")?;
            let (_, gusts) = tile("Gusts")?;
            let (_, precipitation) = tile("Precipitation")?;
            let details = label(now_tab, "", FONT_SMALL)?;
            object_set_width(details, percentage(100)?)?;
            object_set_text_align(details, TextAlign::Center as u32, 0)?;

            // Hourly: temperature line and rain-chance bars for the next day.
            let hourly_empty = empty(hourly_tab)?;
            let hourly_content = container(hourly_tab)?;
            object_set_size(hourly_content, percentage(100)?, size_content())?;
            object_set_flex_flow(hourly_content, FlexFlow::Column)?;
            object_set_padding(hourly_content, 4)?;
            let chart_height = if compact { 90 } else { 170 };
            let chart_card = |title: &str| -> wasm::Result<*mut Object> {
                let card = card_create(hourly_content)?;
                object_set_size(card, percentage(100)?, size_content())?;
                object_set_flex_flow(card, FlexFlow::Column)?;
                object_set_padding(card, 8)?;
                object_remove_flag(card, ObjectFlag::Scrollable)?;
                label(card, title, FONT_SMALL)?;
                let chart = chart_create(card)?;
                object_set_size(chart, percentage(100)?, chart_height)?;
                chart_set_point_count(chart, HOURS as u32)?;
                Ok(chart)
            };
            let temperature_chart = chart_card("Temperature, next 24 hours (°C)")?;
            chart_set_type(temperature_chart, ChartType::Line)?;
            let temperature_series = chart_add_series_slot(temperature_chart, TEMPERATURE_RGB)?;
            let rain_chart = chart_card("Chance of rain (%)")?;
            chart_set_type(rain_chart, ChartType::Bar)?;
            chart_set_axis_range(rain_chart, ChartAxis::PrimaryY, 0, 100)?;
            let rain_series = chart_add_series_slot(rain_chart, RAIN_RGB)?;
            let hourly_range = label(hourly_content, "", FONT_SMALL)?;
            object_set_width(hourly_range, percentage(100)?)?;
            object_set_text_align(hourly_range, TextAlign::Center as u32, 0)?;

            // Daily: one list row per day, created once and relabelled.
            let daily_empty = empty(daily_tab)?;
            let daily_list = list_create(daily_tab)?;
            object_set_size(daily_list, percentage(100)?, size_content())?;
            object_set_surface(daily_list, SURFACE_MUTED)?;
            if compact {
                object_set_font(daily_list, FONT_SMALL, 0)?;
            }
            let mut daily_rows = Vec::with_capacity(DAYS);
            for _ in 0..DAYS {
                daily_rows.push(list_add_text_button(daily_list, c" ".as_ptr())?);
            }

            for object in [hero, tiles, details, hourly_content, daily_list] {
                object_add_flag(object, ObjectFlag::Hidden)?;
            }

            Ok(Self {
                window,
                city_input,
                refresh_button,
                spinner,
                status_row,
                status_label,
                compact,
                empty_state,
                hero,
                location,
                temperature,
                condition: condition_label,
                feels_like,
                tiles,
                humidity,
                humidity_bar,
                wind,
                gusts,
                precipitation,
                details,
                hourly_empty,
                hourly_content,
                hourly_range,
                temperature_chart,
                temperature_series,
                rain_chart,
                rain_series,
                daily_empty,
                daily_list,
                daily_rows,
                in_flight: false,
                buffer: Vec::new(),
            })
        }
    }

    /// Shows a loading or error line; `None` hides the status row.
    fn set_status(&self, text: Option<&str>, loading: bool) {
        unsafe {
            let _ = set_text(self.status_label, text.unwrap_or(""));
            let _ = set_hidden(self.spinner, !loading);
            let _ = set_hidden(self.status_row, text.is_none());
        }
    }

    unsafe fn show_current(&self, data: &WeatherData) -> wasm::Result<()> {
        let current: Option<&CurrentWeather> = data.forecast.current.as_ref();
        unsafe {
            set_text(self.location, &data.city_label)?;
            match current {
                Some(now) => {
                    set_text(
                        self.temperature,
                        &format!("{:.1}°C", now.temperature_2m.unwrap_or_default()),
                    )?;
                    set_text(
                        self.condition,
                        condition(now.weather_code.unwrap_or(u16::MAX)),
                    )?;
                    set_text(
                        self.feels_like,
                        &format!(
                            "Feels like {:.1}°C",
                            now.apparent_temperature.unwrap_or_default()
                        ),
                    )?;
                    let humidity = now.relative_humidity_2m.unwrap_or_default();
                    set_text(self.humidity.value, &format!("{humidity}%"))?;
                    range_set_value(self.humidity_bar, i32::from(humidity.min(100)), false)?;
                    set_text(
                        self.wind.value,
                        &format!(
                            "{:.0} km/h {}",
                            now.wind_speed_10m.unwrap_or_default(),
                            compass(now.wind_direction_10m.unwrap_or_default())
                        ),
                    )?;
                    set_text(
                        self.gusts.value,
                        &format!("{:.0} km/h", now.wind_gusts_10m.unwrap_or_default()),
                    )?;
                    set_text(
                        self.precipitation.value,
                        &format!("{:.1} mm", now.precipitation.unwrap_or_default()),
                    )?;
                }
                None => {
                    set_text(self.temperature, "--")?;
                    set_text(self.condition, "No current data")?;
                    set_text(self.feels_like, "")?;
                }
            }
            set_hidden(self.tiles, current.is_none())?;
            let forecast = &data.forecast;
            let separator = if self.compact { "\n" } else { "  •  " };
            set_text(
                self.details,
                &format!(
                    "{:.4}, {:.4}{separator}{} ({}){separator}{:.0} m",
                    data.latitude,
                    data.longitude,
                    forecast.timezone.as_deref().unwrap_or("?"),
                    forecast.timezone_abbreviation.as_deref().unwrap_or("?"),
                    forecast.elevation.unwrap_or_default()
                ),
            )?;
            set_hidden(self.empty_state, true)?;
            set_hidden(self.hero, false)?;
            set_hidden(self.details, false)?;
        }
        Ok(())
    }

    unsafe fn show_hourly(&self, hourly: Option<&HourlyWeather>) -> wasm::Result<()> {
        let times = hourly.and_then(|h| h.time.as_deref()).unwrap_or(&[]);
        let count = times.len().min(HOURS);
        unsafe {
            set_hidden(self.hourly_empty, count != 0)?;
            set_hidden(self.hourly_content, count == 0)?;
            if count == 0 {
                set_text(self.hourly_empty, "No hourly forecast.")?;
                return Ok(());
            }
            let hourly = hourly.expect("times come from hourly data");
            let temperatures = hourly.temperature_2m.as_deref().unwrap_or(&[]);
            let (points, low, high) = chart_series(&temperatures[..temperatures.len().min(count)]);
            if !points.is_empty() {
                chart_set_axis_range(self.temperature_chart, ChartAxis::PrimaryY, low, high)?;
                chart_set_values(
                    self.temperature_chart,
                    self.temperature_series,
                    points.as_ptr(),
                    points.len(),
                )?;
            }
            let rain: Vec<i32> = hourly
                .precipitation_probability
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .take(count)
                .map(|value| i32::from(*value))
                .collect();
            if !rain.is_empty() {
                chart_set_values(self.rain_chart, self.rain_series, rain.as_ptr(), rain.len())?;
            }
            let (minimum, maximum) = temperatures
                .iter()
                .take(count)
                .fold((f64::MAX, f64::MIN), |(lo, hi), t| (lo.min(*t), hi.max(*t)));
            set_text(
                self.hourly_range,
                &format!(
                    "{} to {}  •  {:.0}° to {:.0}°",
                    hour_label(&times[0]),
                    hour_label(&times[count - 1]),
                    minimum,
                    maximum
                ),
            )?;
        }
        Ok(())
    }

    unsafe fn show_daily(&self, daily: Option<&DailyWeather>) -> wasm::Result<()> {
        let times = daily.and_then(|d| d.time.as_deref()).unwrap_or(&[]);
        unsafe {
            set_hidden(self.daily_empty, !times.is_empty())?;
            set_hidden(self.daily_list, times.is_empty())?;
            if times.is_empty() {
                set_text(self.daily_empty, "No daily forecast.")?;
                return Ok(());
            }
            let daily = daily.expect("times come from daily data");
            let at = |values: Option<&Vec<f64>>, index: usize| {
                values
                    .and_then(|v| v.get(index))
                    .copied()
                    .unwrap_or_default()
            };
            for (index, row) in self.daily_rows.iter().enumerate() {
                let Some(date) = times.get(index) else {
                    set_hidden(*row, true)?;
                    continue;
                };
                let code = daily
                    .weather_code
                    .as_ref()
                    .and_then(|v| v.get(index))
                    .copied()
                    .unwrap_or(u16::MAX);
                let rain = daily
                    .precipitation_probability_max
                    .as_ref()
                    .and_then(|v| v.get(index))
                    .copied()
                    .unwrap_or_default();
                let (low, high) = (
                    at(daily.temperature_2m_min.as_ref(), index),
                    at(daily.temperature_2m_max.as_ref(), index),
                );
                // Compact rows put the condition last so a narrow screen
                // loses the least important part first.
                let text = if self.compact {
                    format!(
                        "{}  {low:.0}°/{high:.0}°  {rain}%  {}",
                        day_label(date),
                        condition(code)
                    )
                } else {
                    format!(
                        "{}   {low:.0}° / {high:.0}°   {}   rain {rain}%",
                        day_label(date),
                        condition(code)
                    )
                };
                let text = CString::new(text).map_err(|_| -1)?;
                list_set_button_text(self.daily_list, *row, text.as_ptr())?;
                set_hidden(*row, false)?;
            }
        }
        Ok(())
    }

    unsafe fn on_refresh(&mut self) {
        if self.in_flight {
            return;
        }
        self.in_flight = true;

        let text_length = unsafe {
            textarea_get_text_length(self.city_input).expect("Failed to get textarea text length")
        };

        self.buffer.clear();
        self.buffer.reserve(text_length as usize + 1); // +1 for null terminator

        unsafe {
            textarea_get_text(
                self.city_input,
                self.buffer.as_mut_ptr(),
                self.buffer.capacity(),
            )
            .expect("Failed to get textarea text");
        }

        let city = unsafe { CStr::from_ptr(self.buffer.as_ptr()).to_string_lossy() };

        let city = city.trim();
        if city.is_empty() {
            self.set_status(Some("Enter a city name"), false);
            self.in_flight = false;
            return;
        }

        self.set_status(Some("Loading..."), true);

        match fetch_weather(city) {
            Ok(data) => {
                let shown = unsafe {
                    self.show_current(&data)
                        .and_then(|()| self.show_hourly(data.forecast.hourly.as_ref()))
                        .and_then(|()| self.show_daily(data.forecast.daily.as_ref()))
                };
                match shown {
                    Ok(()) => self.set_status(None, false),
                    Err(error) => self.set_status(Some(&format!("Display error {error}")), false),
                }
            }
            Err(error) => {
                let message = map_status_message(&error);
                self.set_status(Some(&message), false);
            }
        }

        self.in_flight = false;
    }

    pub unsafe fn run(&mut self) {
        loop {
            let mut code = EventCode::All;
            let mut target: *mut Object = null_mut();
            let _ = unsafe {
                window_pop_event(
                    self.window,
                    &mut code as *mut _ as *mut _,
                    &mut target as *mut _ as *mut _,
                )
            };

            if code as u32 == EVENT_CLOSE_REQUESTED {
                return;
            }
            if should_refresh(
                code as u32,
                target == self.refresh_button,
                target == self.city_input,
            ) {
                unsafe {
                    self.on_refresh();
                }
            } else if code == EventCode::All {
                sleep(core::time::Duration::from_millis(10));
            }
        }
    }
}
