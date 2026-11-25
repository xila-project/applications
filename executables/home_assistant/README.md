# Home Assistant

Home Assistant shows and controls
[Home Assistant](https://www.home-assistant.io/) entities on a Xila device,
for a wall screen in a smart home. It is a WebAssembly application: it is
updated through the storage image or a package, without rebuilding the
firmware.

The device talks to the Home Assistant REST API over the network. It is set
up from its own settings screen: the URL of the server, a sign-in with the
Home Assistant username and password, and the entities to show.

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
itself when the configuration or the credentials are missing. The screen
holds:

- **Connection:** the base URL of the Home Assistant server and the refresh
  period in seconds;
- **Account:** the sign-in state (signed in as which user, or not signed in),
  a sign-in form with username and password (the password field is masked and
  the password is not kept in memory after the request), a two-factor code
  field when Home Assistant asks for one, and a sign-out button that revokes
  the stored credentials on the server and deletes the credentials file. The
  form errors of Home Assistant show under the form, for example an invalid
  username or password;
- **Advanced:** a masked field to paste a long-lived access token instead of
  signing in, with a show or hide button and a replace button;
- **Entities:** the ordered list of entity identifiers, with buttons to add
  an identifier by hand, remove an entry, and move an entry up or down. An
  "add from the server" button opens a picker that lists the areas of the
  Home Assistant installation, then the entities of a chosen area (and the
  entities without an area), each with a switch to add it. The picker reads
  the list through the `POST /api/template` endpoint, so the signed-in user
  must be an administrator. The picker does not download all the states of
  the installation.

Saving writes `configuration.json` and the credentials file atomically (a
temporary file, then a rename) and returns to the cards.

## Sign-in

The sign-in uses the Home Assistant authentication API, so that no token
needs to be typed:

1. `POST /auth/login_flow` starts a login flow and returns a flow identifier;
2. `POST /auth/login_flow/<flow_id>` receives the username and password, or
   the two-factor code when the flow asks for one. A `create_entry` result
   gives an authorization code; a `form` result can carry errors such as
   `invalid_auth`;
3. `POST /auth/token` exchanges the authorization code, as
   `application/x-www-form-urlencoded`, for an access token (valid 1800
   seconds) and a refresh token.

The application stores only the refresh token and the user name in
`credentials.json`. The password is never stored and is cleared from memory
after each request. The access token stays in memory only: before each
request, when it is missing or expires within 60 seconds, the application
gets a new one with `grant_type=refresh_token`. A 400 answer on that refresh
means the device was revoked in Home Assistant: the application shows "signed
out" and opens the settings.

Both the client identifier and the redirect URI of the flow are the fixed URL
`https://xila.dev/applications/home-assistant/`. Home Assistant requires the
redirect URI to be on the same host as the client identifier, which this URL
satisfies; no redirect actually happens in this flow.

If a long-lived access token is set in the advanced field instead, it is used
directly with no refresh.

### Revoking the device

In Home Assistant, open **Profile → Security**. The signed-in application
appears under the sessions or the devices of the account: delete it there to
revoke its refresh token. Long-lived access tokens are revoked under
**Profile → Security → Long-Lived Access Tokens**.

## Files

The WebAssembly runner gives each application a private configuration
directory for each user, created with owner-only permissions (`0700`) and
passed in `XILA_APPLICATION_CONFIGURATION`. Installed from its package, the
directory is `/users/<user>/applications/xila.home-assistant/configuration`.
It holds:

- `configuration.json`: the URL of the Home Assistant server, the refresh
  period in seconds, and the ordered list of the entity identifiers to show;
- `credentials.json`: written by the settings screen after a sign-in, it
  holds the refresh token and the user name: `{"refresh_token": "...",
  "name": "..."}`;
- `token`: the long-lived access token alone, on one line, written by hand
  or by the advanced field of the settings: printable ASCII without spaces,
  at most 512 bytes. A file that holds anything else is treated as having no
  token, since the token is sent in a request header. When both files exist,
  the long-lived token wins.

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
application and write the files. Single quotes keep the JSON literal:

```text
cd /users/<user>/applications/xila.home-assistant/configuration
echo '{"url": "http://homeassistant.local:8123", "refresh_seconds": 30, "entities": ["light.living_room", "sensor.outdoor_temperature"]}' > configuration.json
echo '<token>' > token
chmod 600 token
```

To sign in over the serial shell instead, write the credentials file left by
a sign-in on another device:

```text
echo '{"refresh_token": "<refresh token>", "name": "<user name>"}' > credentials.json
chmod 600 credentials.json
```

Keep the tokens out of shell history files and transcripts on the computer
that sends them: feed the commands from a private file.

## Security

The access token travels in an `Authorization: Bearer` header on every
request, and the refresh token in a request body of `/auth/token`. Neither
token nor the password is ever written to the screen, to logs or to error
messages, and the files sit in the owner-only configuration directory. A URL
that starts with `http://` sends them unencrypted: use it only on a network
you trust.

The HTTPS network device does not validate server certificates yet: the
connection is encrypted, but an active attacker on the network path could
impersonate Home Assistant and read the tokens. Until that is fixed, use the
application on a trusted network, and prefer credentials that you can revoke
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
requests and their streaming parsers, the sign-in flow and the token refresh,
the entity states, the service calls, the entity picker and the storage, and
is tested on the host with fixtures in `tests/fixtures`. The user interface
(`src/interface.rs` and `src/settings.rs`) only builds for the guest, and is
exercised by the Wasm host test `executables/wasm/tests/home_assistant.rs`
against a scripted device.
