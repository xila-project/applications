# Home Assistant

Home Assistant shows and controls
[Home Assistant](https://www.home-assistant.io/) entities on a Xila device,
for a wall screen in a smart home. It is a WebAssembly application: it is
updated through the storage image or a package, without rebuilding the
firmware.

The device talks to the Home Assistant REST API over the network. It only
needs the URL of the server and a long-lived access token.

## Screen

- **Header:** the application title, the time since the last refresh or the
  error of the last refresh (no network, token refused, entity missing), and
  a reload button.
- **Cards:** one per configured entity, in the configured order: three per
  row on wide windows, two on narrow ones. A media player card spans the
  whole row, because it holds five controls. Each card shows the friendly
  name of the entity, its value, and its controls.
- **Controls, by domain:**
  - `light`, `switch` and `input_boolean`: a toggle button, which calls the
    `toggle` service;
  - `scene` and `script`: a turn on button, which calls `turn_on`;
  - `sensor` and `binary_sensor`: the value with its unit
    (`attributes.unit_of_measurement`), without controls;
  - `media_player`: the current title and artist
    (`attributes.media_title`, `attributes.media_artist`), the state with the
    volume in percent, and the previous, play or pause, next, volume down and
    volume up buttons, which call `media_previous_track`, `media_play_pause`,
    `media_next_track`, `volume_down` and `volume_up`.
  - Any other domain shows its value only.
- **Refresh:** on the configured period, after each action, and when the
  reload button is tapped.
- **Errors:** the header shows the error of the last refresh, and each card
  shows its own error. A server that cannot be reached shows a network error,
  a refused token shows the 401 error, and an entity that does not exist
  shows the 404 error on its card. The application keeps running.

## Files

The WebAssembly runner gives each application a private configuration
directory for each user, created with owner-only permissions (`0700`) and
passed in `XILA_APPLICATION_CONFIGURATION`. Installed from its package, the
directory is `/users/<user>/applications/xila.home-assistant/configuration`.
It holds:

- `configuration.json`: the URL of the Home Assistant server, the refresh
  period in seconds, and the ordered list of the entity identifiers to show;
- `token`: the long-lived access token alone, on one line: printable ASCII
  without spaces, at most 512 bytes. A file that holds anything else is
  treated as having no token, since the token is sent in a request header.

A hand-written configuration looks like this:

```json
{
  "url": "http://homeassistant.local:8123",
  "refresh_seconds": 30,
  "entities": [
    "light.living_room",
    "switch.pump",
    "input_boolean.guest_mode",
    "scene.movie_night",
    "script.morning",
    "sensor.outdoor_temperature",
    "binary_sensor.front_door",
    "media_player.television"
  ]
}
```

The fields are:

- `url` (required): the base URL of the server, starting with `http://` or
  `https://`, with an optional port and sub-path, such as
  `https://homeassistant.example.com:8123`;
- `refresh_seconds` (optional, 30 by default, at least 10): the period
  between two refreshes;
- `entities` (required): the entity identifiers to show, in order, such as
  `light.living_room`. Each identifier holds a domain and an object
  identifier, in lowercase, separated by a dot. At most 24 entities show.

### Home Assistant token

1. In Home Assistant, open **Profile → Long-Lived Access Tokens** (at the
   bottom of the profile page) and choose **Create Token**.
2. Give the token a name, such as `xila`, and copy it. The token shows once:
   keep it in a safe place.

### Writing the files over the serial shell

Start Home Assistant once, so that the runner creates its directory. Then,
with Platforms' `tools/dev/serial_shell.py`, log in as the user who runs the
application and write both files. Single quotes keep the JSON literal:

```text
cd /users/<user>/applications/xila.home-assistant/configuration
echo '{"url": "http://homeassistant.local:8123", "refresh_seconds": 30, "entities": ["light.living_room", "sensor.outdoor_temperature"]}' > configuration.json
echo '<token>' > token
chmod 600 token
```

Keep the token out of shell history files and transcripts on the computer
that sends it: feed the commands from a private file.

## Security

The token travels in an `Authorization: Bearer` header on every request. The
token is never written to the screen, to logs or to error messages, and the
token file sits in the owner-only configuration directory. A URL that starts
with `http://` sends the token unencrypted: use it only on a network you
trust.

The HTTPS network device does not validate server certificates yet: the
connection is encrypted, but an active attacker on the network path could
impersonate Home Assistant and read the token. Until that is fixed, use the
application on a trusted network, and prefer a token that you can revoke
quickly.

## Memory

Xila targets devices with 256 KB of RAM. Each state answer is read as a
stream through a small buffer: only the shown attributes are kept, a
response longer than 2 MiB is refused, and at most 24 entities show. One
request is sent per entity, at each refresh and after each action.

## Development

```sh
cargo test -p xila-home-assistant
cargo clippy --target wasm32-wasip1 -p xila-home-assistant -- -D warnings
mise run build:applications
```

The library (`src/lib.rs`) holds the configuration, the Home Assistant
requests and their streaming parsers, the entity states, the service calls
and the storage, and is tested on the host with fixtures in
`tests/fixtures`. The user interface (`src/interface.rs`) only builds for
the guest.
