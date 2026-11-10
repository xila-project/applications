# Weather executable

Weather demo app for Xila WASM runtime.

## Data sources

- Geocoding: `https://geocoding-api.open-meteo.com/v1/search`
- Forecast: `https://api.open-meteo.com/v1/forecast`

## Runtime path

Uses internal HTTPS device mounted at `/devices/network/https_client`.

## UX behavior

- Enter a city name and press Refresh (or confirm the field). The first
  geocoding result is used.
- A spinner and status line appear only while loading or after an error.
- **Now:** location, temperature, condition (WMO code in words) and feels-like
  in a card, then humidity (with a bar), wind with compass direction, gusts and
  precipitation tiles, and the coordinates, time zone and elevation.
- **Hourly:** a temperature line chart and a chance-of-rain bar chart for the
  next 24 hours, with the time span and temperature range.
- **Daily:** a 10-day list with weekday, minimum and maximum temperature,
  condition and chance of rain.
- Windows smaller than 560×360 use compact tab titles, two tiles per row,
  shorter charts and shorter daily rows.
