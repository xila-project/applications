# Home Assistant

Home Assistant shows and controls
[Home Assistant](https://www.home-assistant.io/) entities on a Xila device,
for a wall screen in a smart home. It is a WebAssembly application: it is
updated through the storage image or a package, without rebuilding the
firmware.

The device talks to the Home Assistant REST API over the network with a
long-lived access token. It is set up from its own settings screen: the URL
of the server, the token, and the entities to show.

## Screen

- **Header:** the application title, the time since the last refresh or the
  error of the last refresh (no network, token refused, entity missing), a
  reload button, and a settings button.
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

## Settings

The settings button in the header opens the settings screen. It also opens by
itself when the configuration or the token is missing. While it is open, the
header and the cards hide, so the screen gets the whole window. The screen
holds:

- **Connection:** the base URL of the Home Assistant server and the refresh
  period in seconds;
- **Advanced:** the long-lived access token in a field hidden behind a mask,
  with a Show button to reveal it (the token is never logged), an "Import
  from file" action, and a help text. The import reads the first non-empty
  line of a file the user points at (by default
  `/media/sd_card/home_assistant_token.txt`, the SD card mount of the boards
  that have one, such as the WT32-SC01 Plus), trims it, checks it with the
  same rules as the token file, and stores it. The source file is only read,
  never changed. A missing, empty, invalid or too long file shows its own
  error;
- **Entities:** the ordered list of entity identifiers, with buttons to add
  an identifier by hand, remove an entry, and move an entry up or down. An
  "add from the server" button opens a picker that lists the areas of the
  Home Assistant installation, then the entities of a chosen area (and the
  entities without an area), each with a switch to add it. The picker reads
  the list through the `POST /api/template` endpoint with the token, so the
  token must belong to an administrator account. The picker does not
  download all the states of the installation.

Saving writes `configuration.json` and the `token` file atomically (a
temporary file, then a rename) and returns to the cards.

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

### Creating the token

1. In Home Assistant, open **Profile**, then **Security**, then **Long-Lived
   Access Tokens** (at the bottom of the page) and choose **Create Token**.
2. Give the token a name, such as `xila`, and copy it. The token shows once:
   keep it in a safe place.

To revoke it later, delete it under **Profile -> Security -> Long-Lived
Access Tokens**; the application then gets 401 answers and shows the token
error.

### Writing the files over the serial shell

Start Home Assistant once, so that the runner creates its directory. Then,
with Platforms' `tools/dev/serial_shell.py`, log in as the user who runs the
application and write the files. The shell supports `>` redirection, which
creates or truncates the file. Single quotes keep the JSON literal:

```text
cd /users/<user>/applications/xila.home-assistant/configuration
echo '{"url": "http://homeassistant.local:8123", "refresh_seconds": 30, "entities": ["light.living_room", "sensor.outdoor_temperature"]}' > configuration.json
echo '<token>' > token
chmod 600 token
```

Keep the token out of shell history files and transcripts on the computer
that sends it: feed the command from a private file.

## Security

The token travels in an `Authorization: Bearer` header on every request.
It is never written to logs or error messages; the settings screen can show
it through the Show button, but it is hidden by default, and the file sits in
the owner-only configuration directory. A URL that starts with `http://`
sends it unencrypted: use it only on a network you trust.

The HTTPS network device does not validate server certificates yet: the
connection is encrypted, but an active attacker on the network path could
impersonate Home Assistant and read the token. Until that is fixed, use the
application on a trusted network, and prefer a token that you can revoke
quickly.

## Memory

Xila targets devices with 256 KB of RAM. Each state answer is read as a
stream through a small buffer: only the shown attributes are kept, a
response longer than 2 MiB is refused, and at most 24 entities show. One
request is sent per entity, at each refresh and after each action. The
answers of the template endpoint are bounded to 64 KiB as well.

## Development

```sh
cargo test -p xila-home-assistant
cargo clippy --target wasm32-wasip1 -p xila-home-assistant -- -D warnings
mise run build:applications
```

The library (`src/lib.rs`) holds the configuration, the Home Assistant
requests and their streaming parsers, the entity states, the service calls,
the entity picker and the storage, and is tested on the host with fixtures
in `tests/fixtures`. The user interface (`src/interface.rs` and
`src/settings.rs`) only builds for the guest, and is exercised by the Wasm
host test `executables/wasm/tests/home_assistant.rs` against a scripted
device.
