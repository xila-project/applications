# Settings

The Appearance tab changes the shared Shadcn theme for all open windows in the
current session. Changes apply immediately and are saved to
`<home>/.appearance`, in the current user's home.

On screens with brightness control, the Appearance tab also has a
**Brightness** slider, from 10 % to 100 %. The level applies while the slider
moves and is saved with the appearance when it is released (or after each
keypad step), so it is restored at start-up and login with the theme. The
slider never goes below 10 %, so the screen cannot go dark by accident; screens
without brightness control, such as the native and browser examples, hide it.

The default is **Zinc dark**, with automatic primary and secondary colors from
the shared theme's zinc palette. Selecting light mode also switches automatic
colors to zinc light. Blue, green, orange, rose and violet presets keep their
colors when switching modes; the theme chooses contrasting text automatically.

The graphical shell restores each user's appearance after login, before opening
the desktop. Native and browser startup also restore root's appearance for the
login screen. Other platforms can call `graphics::appearance::restore_appearance`
after mounting the filesystem or after login; the caller must not hold the LVGL
lock.

Both shells create the private home recorded in the user's account before
dropping login's privileges and set `HOME` for the session. New accounts record
`/users/<user name>`, accounts created before homes were recorded keep
`/users/<user id>`, and root uses `/users/root`. Renaming an account keeps its
recorded home. Home directories are owned by that user and their
primary group, with mode 0700; appearance records have mode 0600. Missing
preferences use the default. Invalid or unreadable preferences are logged and
leave the startup theme intact; Settings displays a warning and allows saving a
replacement.

Preferences use a bounded, versioned record with a checksum. Settings writes and
flushes a temporary file before replacing the saved file with a filesystem
rename. If saving fails, the selected appearance still applies for this session
and Settings displays an error. Reboot persistence requires a persistent backing
disk; an in-memory or newly formatted disk starts with the default appearance.

Wi-Fi interfaces advertising station capabilities have a Wi-Fi tab in their
Network configuration panel. It supports radio control, bounded scan results,
security/signal/channel/BSSID metadata, ephemeral password entry, connect,
disconnect and progress/error status. Progress refreshes while the panel is open,
including when there is no user input. Passwords are masked and cleared after
submission or dismissal; Settings saves no network profiles or credentials.
Unsupported authentication methods are displayed but cannot be selected for
connection. See the native example's `--mock-wifi` mode for hardware-free testing.

## Time zone

The General tab shows the stored time zone, the local time it gives (updated
once per second while the tab is shown), and one editor at a time:

- **Zone name**, only in builds with a `time-zone-names-*` feature: a region,
  then a zone of that region from the IANA table that the build links. Saving
  stores both the IANA name and its POSIX TZ string. This is the default
  editor when it exists.
- **UTC offset**: a form that builds the POSIX TZ string, so that nobody has to
  type one. It takes the offset (UTC-12:00 to UTC+14:00, in steps of 15
  minutes), an optional daylight saving time with its shift (30 minutes, one
  hour or two hours) and its rules, and optional abbreviations. The rule
  presets are "European Union" (last Sunday of March to last Sunday of
  October at 01:00 UTC, written as the local times of the chosen offset) and
  "United States" (second Sunday of March to first Sunday of November at
  02:00 local time); "Custom" takes the month, week (first to fourth, or
  last), weekday and hour of the start and of the end. Without abbreviations,
  the numeric form of tzdata is used (`<+01>`). The form shows the generated
  string and stores it without a name. This is the default editor without a
  names feature.
- **TZ string (advanced)**: a raw POSIX TZ string, checked while it is typed.

The setting is stored by `xila::local_time::set` in
`/configuration/system/time_zone.json`: only root and members of the
administrator group can change it, and Settings says so otherwise. Programs
started after the change use it at once through the `TZ` environment
variable; the graphical shell clock follows it at its next update; running
WebAssembly applications keep their value until they restart.

Cargo features select the zone names, one per IANA region of
`xila-time-zone`, which combine: `time-zone-names-africa`,
`time-zone-names-america`, `time-zone-names-antarctica`,
`time-zone-names-arctic`, `time-zone-names-asia`, `time-zone-names-atlantic`,
`time-zone-names-australia`, `time-zone-names-etc`, `time-zone-names-europe`,
`time-zone-names-indian`, `time-zone-names-pacific` and `time-zone-names-all`.
`time-zone-names-legacy-aliases` adds the legacy aliases (`US/Eastern`). The
default is no names. The flash cost of each region is in the `xila-time-zone`
README; with all regions, the table costs about 9.5 KB on Cortex-M. The
selector keeps the list of regions (11 pointers with all regions) and LVGL
holds the zone list of the selected region (at most 1,359 bytes, for the
121 canonical names of America).
