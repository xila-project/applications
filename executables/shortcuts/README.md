# Shortcuts

Shortcuts turns a Xila device into a remote for the computer it is connected
to: media keys, screenshots, keyboard shortcuts and an optional mouse pad.
It is a WebAssembly application: it is updated through the storage image or a
package, without rebuilding the firmware.

The application writes 8-byte messages to the device node
`/devices/human_interface/emulation`. The device acts as a USB or Bluetooth
keyboard, media remote and mouse toward the connected computer. The
connection state comes from the `GET_STATUS` control command of the same
node.

## Screen

- **Header:** the application title and the connection state of the device:
  connected, not connected, with the caps lock indicator when the computer
  reports it. When the device has no `/devices/human_interface/emulation`
  node at all, a message says so and every control stays disabled.
- **Media tab:** play/pause, previous, next, stop, volume down, volume up and
  mute, sent as consumer control usages.
- **Shortcuts tab:** a grid of the buttons of the active profile, each with
  its label and its shortcut text, with the modifier names of the profile
  (`Win` on Windows, `Cmd` on macOS, `Super` on GNOME). A tap presses and
  releases the keys, so no key stays pressed on the computer. The
  application also sends an all-zero release for keyboard, consumer control
  and mouse at start and at exit, as a safety net.
- **Notice lines:** a start-up line holds the problems found at start, such
  as an invalid `settings.json` or `profiles.json` or a missing device node,
  and stays visible; the line under it shows the result of the last action.
- **Mouse tab (optional):** a touch pad that sends relative pointer movement,
  a scroll strip that sends wheel steps, and left and right click buttons.
- **Settings tab:** the profile of the computer (Windows, macOS, Linux with
  GNOME), the keyboard layout (QWERTY, AZERTY, QWERTZ) and whether the mouse
  pad page shows.

While no computer listens, the header says "Not connected" and the buttons
are disabled.

## Files

The WebAssembly runner gives each application a private configuration
directory for each user, created with owner-only permissions (`0700`) and
passed in `XILA_APPLICATION_CONFIGURATION`. Installed from its package, the
directory is `/users/<user>/applications/shortcuts/configuration`. It holds:

- `settings.json` (optional): the active profile, the keyboard layout and
  the mouse pad switch;
- `profiles.json` (optional): the shortcut buttons of each profile, to
  replace the built-in ones and add custom buttons.

A hand-written settings file looks like this:

```json
{
  "profile": "macos",
  "layout": "azerty",
  "mouse_pad": true
}
```

The fields are:

- `profile` (optional, `windows` by default): `windows`, `macos` or `gnome`;
- `layout` (optional, `qwerty` by default): `qwerty`, `azerty` or `qwertz`;
- `mouse_pad` (optional, `true` by default): whether the mouse pad page
  shows.

### Profiles

Each profile holds the buttons of one computer system. The built-in
defaults are:

| Button            | Windows      | macOS         | Linux (GNOME)      |
|-------------------|--------------|---------------|--------------------|
| Screenshot        | `Win+Shift+S`| `Cmd+Shift+4` | `Print Screen`     |
| Full screenshot   | `Win+Print Screen`| `Cmd+Shift+3`| `Shift+Print Screen`|
| Copy              | `Ctrl+C`     | `Cmd+C`       | `Ctrl+C`           |
| Paste             | `Ctrl+V`     | `Cmd+V`       | `Ctrl+V`           |
| Undo              | `Ctrl+Z`     | `Cmd+Z`       | `Ctrl+Z`           |
| Lock screen       | `Win+L`      | `Ctrl+Cmd+Q`  | `Super+L`          |
| Switch window     | `Alt+Tab`    | `Cmd+Tab`     | `Alt+Tab`          |
| Show desktop      | `Win+D`      | `Cmd+F3`      | `Super+D`          |
| Mute microphone   | `Win+Alt+K`  | none          | none               |

Notes on the defaults:

- `Win+Shift+S` on Windows and `Cmd+Shift+4` on macOS open an area
  selection that needs a mouse drag on the computer, with the mouse pad of
  this application or the computer mouse. On Windows 11, `Print Screen`
  opens the Snipping Tool by default, so the full screenshot uses
  `Win+Print Screen`, which saves the whole screen to a file. On GNOME 42
  and later, `Print Screen` opens the interactive screenshot UI and
  `Shift+Print Screen` takes the full screenshot directly, following
  GNOME Shell's own keybinding defaults in
  `data/org.gnome.shell.gschema.xml.in` (`screenshot` is `<Shift>Print`,
  `show-screenshot-ui` is `Print`, `screenshot-window` is `<Alt>Print`);
  GNOME before 42 behaves differently.
- `Win+Alt+K` mutes the microphone only in the applications that support
  it, for example Microsoft Teams.
- `Cmd+F3` shows the desktop on a Mac only when the function keys behave
  as F1 to F12 in the Mac keyboard settings, or with the `Fn` key held.
- Vanilla GNOME has no default shortcut for `Super+D`; Ubuntu sets one to
  show the desktop.
- `Alt+Tab` switches applications on GNOME, not windows; use `Alt` above
  the `Tab` key for windows.

To change the buttons or add custom ones, write `profiles.json`. A profile
entry replaces the whole button list of that profile:

```json
{
  "profiles": [
    {
      "id": "windows",
      "buttons": [
        { "id": "screenshot", "keys": "Win+Shift+S" },
        { "id": "my_tool", "keys": "Ctrl+Alt+T", "label": "My tool" }
      ]
    }
  ]
}
```

Each button has an `id`, a `keys` shortcut text, and an optional `label` for
custom buttons (the built-in identifiers are translated). A shortcut text is
made of tokens joined by `+`: modifier tokens (`Ctrl`, `Shift`, `Alt`,
`AltGr`, `Win` or `Super`, `Cmd`, with optional `Left`/`Right`) followed by
one or more key tokens: a letter, a digit, `Enter`, `Escape`, `Tab`, `Space`,
`Backspace`, `Delete`, `Insert`, `Home`, `End`, `Page Up`, `Page Down`,
`Print Screen`, an arrow (`Left`, `Right`, `Up`, `Down`) or `F1` to `F24`.
The `+` key itself cannot be written, because `+` joins the tokens: a
trailing `+` or a doubled `++` is rejected, and so is a text with only
modifiers and no key. A button whose shortcut text does not parse is not
shown silently: the notice line reports how many buttons were ignored, with
the identifier and the profile of the first one, and a profile with no
buttons gets its own message.

HID sends key positions, not characters. A letter in a shortcut text means
the key that shows that letter on the keyboard of the user, so it is mapped
through the `layout` setting: `Ctrl+Z` presses the Z key on QWERTY, the W
position on AZERTY and the Y position on QWERTZ. The digit row keeps its
positions on all three layouts, but on AZERTY it produces `&é"'(-è_çà`
without Shift, so a shortcut that uses a digit gets Shift added when it is
missing and the computer receives the digit itself: `Ctrl+1` on AZERTY is
sent as Ctrl+Shift+1.

### Writing the files over the serial shell

Start Shortcuts once, so that the runner creates its directory. Then, with
Platforms' `tools/dev/serial_shell.py`, log in as the user who runs the
application and write the files. Single quotes keep the JSON literal:

```text
cd /users/<user>/applications/shortcuts/configuration
echo '{"profile": "macos", "layout": "qwerty", "mouse_pad": true}' > settings.json
```

## Memory

Xila targets devices with 256 KB of RAM. Every message is 8 bytes, a tap
sends the press and the release in one 16-byte write, and the shortcut grid
holds at most 24 buttons. The connection state is polled once per second.

## Development

```sh
cargo test -p xila-shortcuts
cargo clippy --target wasm32-wasip1 -p xila-shortcuts -- -D warnings
mise run build:applications
```

The library (`src/lib.rs`) holds the mirrored device contract, the write
protocol with its retries, the layouts, the shortcut parser, the profiles,
the settings and the storage, and is tested on the host. The user interface
(`src/interface.rs`) and the device transport (`src/device.rs`) only build
for the guest.
