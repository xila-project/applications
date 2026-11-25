# Dashboard

Dashboard shows panel values and small charts read from a
[Grafana](https://grafana.com/) server, for a desk screen such as the
WT32-SC01 Plus (480 × 320, Wi-Fi). It is a WebAssembly application: it is
updated through the storage image or a package, without rebuilding the
firmware.

Queries run through Grafana's query endpoint (`POST /api/ds/query`), so the
data source itself (Prometheus or a compatible one)
stays private behind Grafana. The device only needs Grafana's URL and a
token.

## Screen

- **Header:** the dashboard title, the time since the last refresh or the
  error of the last refresh (no network, token refused, timeout…), a reload
  button (only when a dashboard is imported) and the settings button.
- **Tiles:** three per row (two on narrow windows). A *value* panel shows its
  last value in big text, coloured by its thresholds; when its query returns
  several series, the first three show with their label values and "+N more".
  A *chart* panel spans two columns and shows the last value of its top
  series, a line chart of up to three series (those with the highest last
  value) and the minimum and maximum shown.
- **Refresh:** on the configured period, and when a tile or the grid is
  tapped. The reload button fetches the dashboard definition again; it does
  not change how values refresh.

## Panels: imported or manual

Panels come from one of two sources:

- **A dashboard.** When a dashboard uid is set, the application fetches
  `GET /api/dashboards/uid/<uid>` at every start (one request, 10 to 50 KB,
  parsed as a stream and dropped) and converts its panels:
  - stat, gauge and bargauge panels become value panels, and timeseries
    panels become charts over the dashboard's time range (at most 24 hours);
    panels inside collapsed rows are included; table, pie chart, logs, text
    and other panels are skipped, and the settings say how many;
  - each panel keeps its title, the expression of its first visible target,
    its unit, decimals and its first two threshold steps after the base one,
    as the warning and critical thresholds;
  - template variables (`$name`, `${name}`, `${name:format}`, `[[name]]`)
    take their current value from the dashboard: "All" becomes the
    variable's "all" value or `.*`, and several values become `a|b`.
    `$__rate_interval`, `$__interval` and `$__range` become `5m`;
  - data source references are ignored: the configured data source is used.

  The converted panels are cached in the configuration. When the import fails
  (no network, token refused…), the cached panels show with a "Dashboard not
  reloaded" notice. Imported panels are read-only in the settings, apart from
  a switch that hides each one; hidden panels are stored by their Grafana id
  and stay hidden across reloads. "Convert to manual" copies the visible
  panels to the manual list and stops following the dashboard.
- **Manual panels**, written in the settings or in the configuration file.

Grafana units map to the application's units: `percent`, `percentunit`
(shown as a percentage of 0 to 1), `bytes` and `decbytes`, `Bps` and
`binBps`, `s` and `celsius`. Every other unit shows as a plain number.

## Settings

The gear button opens the settings; they also open by themselves when there
is no configuration yet. They hold:

- **Connection:** Grafana's base URL (`https://` or `http://`, a sub-path is
  kept), the uid of the data source the queries run on, the refresh period
  in seconds (at least 10) and the uid of a dashboard to import (empty for
  manual panels). **Test connection** runs one query (the first panel, else
  `vector(1)`) and shows its value or the error.
- **Token:** a field hidden behind a mask with a show and hide button, so a
  typed token is never displayed by default. A saved token is never shown
  again: the settings show "Token set" with a **Replace** action.
- **Panels:** the imported panels with their visibility switch, **Reload
  dashboard** and **Convert to manual**; or the manual panels with **Edit**,
  **Delete** and **Add panel**. The panel editor holds the title, the PromQL
  expression, the unit, the decimals (empty for automatic), the warning and
  critical thresholds, the display (value or chart) and, for a chart, its
  range (15 minutes to 24 hours).

Thresholds normally rise: values at or above the warning threshold are
orange, at or above the critical one red, and green below. When the warning
threshold is above the critical one, lower values are worse, as for free
space.

**Save** writes the same files as the serial-shell route below.

## Files

The WebAssembly runner gives each application a private configuration
directory for each user, created with owner-only permissions (`0700`) and
passed in `XILA_APPLICATION_CONFIGURATION`. Installed from its package, the
directory is `/users/<user>/applications/xila.dashboard/configuration`. It
holds:

- `configuration.json`: the connection, the panels, the hidden panels and the
  cached dashboard;
- `token`: the Grafana token alone, on one line: printable ASCII without
  spaces, at most 512 bytes. A file that holds anything else is treated as
  having no token, since the token is sent in a request header.

A hand-written configuration only needs the connection and the panels:

```json
{
  "url": "https://grafana.example.com",
  "data_source": "prometheus",
  "refresh_seconds": 60,
  "dashboard": "servers",
  "panels": [
    {
      "title": "CPU",
      "expression": "avg(rate(node_cpu_seconds_total{mode!=\"idle\"}[5m]))",
      "unit": "fraction",
      "decimals": 1,
      "warning": 0.8,
      "critical": 0.9
    },
    {
      "title": "Network in",
      "expression": "sum(rate(node_network_receive_bytes_total[5m]))",
      "unit": "bytes_per_second",
      "kind": "chart",
      "range_seconds": 3600
    }
  ]
}
```

Units are `none`, `percent`, `fraction`, `bytes`, `bytes_per_second`,
`seconds` and `celsius`; kinds are `value` (the default) and `chart`.
Without `dashboard`, the `panels` list shows. Manual expressions must use
explicit windows such as `[5m]`: only imported panels have Grafana's interval
macros replaced.

### Grafana token

1. In Grafana, open **Administration → Users and access → Service accounts**
   and add a service account with the **Viewer** role.
2. Add a token to it and copy it. A Viewer token can read dashboards and run
   queries, nothing more.
3. Find the data source uid in **Connections → Data sources**: it is the last
   part of the data source's page URL.

### Writing the files over the serial shell

Start Dashboard once, so that the runner creates its directory. Then, with
Platforms' `tools/dev/serial_shell.py`, log in as the user who runs the
application and write both files. Single quotes keep the JSON literal:

```text
cd /users/<user>/applications/xila.dashboard/configuration
echo '{"url": "https://grafana.example.com", "data_source": "prometheus", "dashboard": "servers"}' > configuration.json
echo '<token>' > token
chmod 600 token
```

Keep the token out of shell history files and transcripts on the computer
that sends it: feed the commands from a private file.

## Security

Use a Viewer-only token: it travels in an `Authorization: Bearer` header on
every request. The token is never written to the screen once saved (the
settings show "Token set"), to logs or to error messages, and the token file
sits in the owner-only configuration directory. A URL that starts with
`http://` sends the token unencrypted: use it only on a network you trust.

HTTPS server certificate verification is not done yet on Espressif boards
(xila-project/platforms#186): the connection is encrypted, but an active
attacker on the network path could impersonate Grafana and read the token.
Until that is fixed, use the application on a trusted network, prefer a
token that can be revoked quickly, and give it the Viewer role only.

## Memory

Responses are read as a stream through a 4 KiB buffer: a dashboard is never
held whole, a response longer than 2 MiB is refused, and a chart series keeps
at most 60 averaged points. Instant
queries share one request per refresh; range queries are grouped by range,
and a request is split when its body passes 8 KiB.

## Development

```sh
cargo test -p xila-dashboard
cargo clippy --target wasm32-wasip1 -p xila-dashboard -- -D warnings
mise run build:applications
```

The library (`src/lib.rs`) holds the configuration, the Grafana requests,
their streaming parsers, the dashboard import and the formatting, and is
tested on the host with fixtures in `tests/fixtures`. The user interface
(`src/interface.rs`, `src/settings.rs`) only builds for the guest.
