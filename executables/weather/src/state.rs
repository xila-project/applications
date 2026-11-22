use crate::model::{ForecastResponse, GeocodingResponse, GeocodingResult};
use alloc::string::{String, ToString};
use embedded_i18n::t;

#[cfg(target_arch = "wasm32")]
use std::eprintln;

#[derive(Debug, Clone)]
pub enum AppError {
    NoLocationFound,
    Network,
    Parse,
    ApiError(u16),
}

pub fn map_status_message(error: &AppError) -> String {
    match error {
        AppError::NoLocationFound => String::from(t!("status.no_location")),
        AppError::Network => String::from(t!("status.network_error")),
        AppError::Parse => String::from(t!("status.invalid_response")),
        AppError::ApiError(code) => t!("status.api_error", code = code).to_string(),
    }
}

pub struct WeatherData {
    pub city_label: String,
    pub latitude: f64,
    pub longitude: f64,
    pub forecast: ForecastResponse,
}

#[cfg(target_arch = "wasm32")]
type FetchFn = fn(&str) -> Result<alloc::vec::Vec<u8>, alloc::string::String>;

/// Diagnostic for a response that could not be parsed. It holds the size only:
/// a response body is never printed.
#[cfg(any(target_arch = "wasm32", test))]
fn parse_failure_message(label: &str, length: usize) -> String {
    alloc::format!("Could not parse the {label} response ({length} bytes)")
}

#[allow(dead_code)]
fn pick_best_location(response: GeocodingResponse) -> Option<GeocodingResult> {
    response
        .results
        .and_then(|results| results.into_iter().next())
}

#[cfg(target_arch = "wasm32")]
pub fn fetch_weather(city: &str) -> Result<WeatherData, AppError> {
    fetch_weather_with(city, crate::net::https_get)
}

#[cfg(target_arch = "wasm32")]
fn fetch_weather_with(city: &str, https_get: FetchFn) -> Result<WeatherData, AppError> {
    use miniserde::json;

    use crate::api::{build_forecast_url, build_geocoding_url};

    let geocoding_url = build_geocoding_url(city);
    let geo_bytes = https_get(&geocoding_url).map_err(|error| {
        if let Some(status) = parse_api_error_status(&error) {
            AppError::ApiError(status)
        } else {
            AppError::Network
        }
    })?;
    drop(geocoding_url);
    let geo_text = core::str::from_utf8(&geo_bytes).map_err(|_| AppError::Parse)?;
    let geo: GeocodingResponse = json::from_str(geo_text).map_err(|_| {
        #[cfg(target_arch = "wasm32")]
        eprintln!("{}", parse_failure_message("geocoding", geo_bytes.len()));
        AppError::Parse
    })?;
    // Geocoding fields are owned after parsing; release its JSON before the
    // larger forecast download starts.
    drop(geo_bytes);
    let best = pick_best_location(geo).ok_or(AppError::NoLocationFound)?;

    let forecast_url = build_forecast_url(best.latitude, best.longitude);

    let mut forecast_bytes = https_get(&forecast_url).map_err(|error| {
        if let Some(status) = parse_api_error_status(&error) {
            AppError::ApiError(status)
        } else {
            AppError::Network
        }
    });

    if forecast_bytes.is_err() {
        forecast_bytes = https_get(&forecast_url).map_err(|error| {
            if let Some(status) = parse_api_error_status(&error) {
                AppError::ApiError(status)
            } else {
                AppError::Network
            }
        });
    }

    let forecast_bytes = forecast_bytes?;
    let forecast_text = core::str::from_utf8(&forecast_bytes).map_err(|_| AppError::Parse)?;
    let forecast: ForecastResponse = json::from_str(forecast_text).map_err(|_| {
        #[cfg(target_arch = "wasm32")]
        eprintln!(
            "{}",
            parse_failure_message("forecast", forecast_bytes.len())
        );
        AppError::Parse
    })?;

    drop(forecast_bytes);
    drop(forecast_url);
    let city_label = if let Some(country) = best.country {
        alloc::format!("{}, {}", best.name, country)
    } else {
        best.name
    };

    Ok(WeatherData {
        city_label,
        latitude: best.latitude,
        longitude: best.longitude,
        forecast,
    })
}

#[allow(dead_code)]
fn parse_api_error_status(message: &str) -> Option<u16> {
    message
        .strip_prefix("api error: ")
        .and_then(|value| value.parse::<u16>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_failure_message_reports_the_size_only() {
        assert_eq!(
            parse_failure_message("forecast", 1234),
            "Could not parse the forecast response (1234 bytes)"
        );
    }

    #[test]
    fn picks_first_geocoding_result() {
        let response = GeocodingResponse {
            results: Some(alloc::vec![
                GeocodingResult {
                    name: String::from("First"),
                    country: None,
                    timezone: None,
                    latitude: 1.0,
                    longitude: 2.0,
                },
                GeocodingResult {
                    name: String::from("Second"),
                    country: None,
                    timezone: None,
                    latitude: 3.0,
                    longitude: 4.0,
                },
            ]),
            generationtime_ms: None,
        };

        let selected = pick_best_location(response).unwrap();
        assert_eq!(selected.name, "First");
    }

    #[test]
    fn status_messages_come_from_the_catalog() {
        assert_eq!(map_status_message(&AppError::Network), "Network error");
        assert_eq!(
            map_status_message(&AppError::ApiError(503)),
            "API error: 503"
        );
    }

    #[test]
    fn parses_api_error_status_code() {
        assert_eq!(parse_api_error_status("api error: 503"), Some(503));
    }

    #[cfg(target_arch = "wasm32")]
    #[test]
    fn maps_api_error_from_transport_in_fetch_flow() {
        fn failing_get(_: &str) -> Result<alloc::vec::Vec<u8>, alloc::string::String> {
            Err(alloc::string::String::from("api error: 429"))
        }

        let error = fetch_weather_with("Paris", failing_get).unwrap_err();
        match error {
            AppError::ApiError(429) => {}
            _ => panic!("expected API error 429"),
        }
    }
}
