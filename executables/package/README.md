# Xila `package` (native CLI)

This is a **native Xila executable**, not the host-side `xila-package`
builder. It deliberately has no graphical interface, host filesystem access
or launcher registration.

```text
package install dev.xila.clock
package install dev.xila.clock@0.1.0 --repository xila
package install /temporary/clock.xpk
package install /temporary/clock.xpk --public-key /temporary/developer.pub
package install /temporary/clock.xpk --allow-untrusted
package upgrade dev.xila.clock
package upgrade /temporary/clock-0.2.0.xpk
package remove dev.xila.clock
package recover
package list
package info dev.xila.clock
package info dev.xila.clock@0.2.0 --repository xila
package search clock
package repository add community https://registry.xila.example
package repository list
package repository remove community
```

## Trust

The device trusts the **root keys of its configured repositories**, never a
developer key on its own. Installing checks, in order: the repository's root
key, then its repository key, then the registry entry and the developer key
that entry authorizes, then the package's manifest signature, then every file
hash.

- `package install NAME` resolves the package in the configured repositories
  and downloads the archive the registry entry pins (SHA-256 and size).
- `package install ARCHIVE` reads the archive's manifest, finds the registry
  entry of that exact name and version in the configured repositories, and
  requires the entry to pin this archive. Only registry metadata is fetched;
  the archive never leaves the device.
- `--public-key PATH` instead verifies a local archive with an explicitly
  trusted raw 32-byte Ed25519 developer key, for offline sideloading. It never
  imports the key into system trust.
- `--allow-untrusted` is an explicit temporary override that accepts an
  unsigned or unknown publisher. It still verifies the complete manifest,
  SHA-256 inventory, file order, TAR headers, end marker, compatibility and
  sizes. It must not be used for unattended deployment.

No signature verification happens at launch. The OS protects installed
package contents and manifests from unprivileged writes.

## Installation, upgrade and removal

The installer reads the archive in 512-byte blocks through the Xila VFS and
`tar-no-std`, using the shared incremental verification of
`xila-project/package_manager`, into a newly created private quarantine
directory (`/applications/bundles/.incoming-*`), then seals it read-only.

It then hands it to package_manager's crash-consistent installation
transactions (`xila_package_manager::installation`). A journal at
`/configuration/system/packages/transaction.json` describes each operation
before its first step, and only single renames inside
`/applications/bundles/` change what is installed:

- **install** makes the sealed directory `/applications/bundles/<package-id>/`
  and records its origin in `/configuration/system/packages/installed/`;
- **upgrade** needs a strictly higher version from the repository the package
  was installed from (or another archive for a locally installed one). It moves
  the old bundle aside, activates the new one, then deletes the old one, so
  files the new version no longer ships disappear;
- **remove** moves the bundle aside, deletes its origin, then deletes it.

Upgrades and removals never touch user data, which lives outside the bundles
directory. Every command that changes packages first finishes or undoes an
operation interrupted by a crash or power loss, and deletes abandoned
temporary entries (`.incoming-*`, `.previous-*`, `.removing-*`,
`.download-*`); an interrupted upgrade always leaves the old or the new
version. `package recover` does only that, and `package list` warns when an
interrupted operation is pending.

## Information and search

`package info NAME` describes an installed package offline, from the manifest
verified at installation and kept in its sealed bundle, and from its recorded
origin. When the package is not installed, or when a version
(`NAME@VERSION`) or `--repository` is given, it describes the newest
compatible registry entry instead (or that version's entry), authenticated
from each repository's root key like an installation, and shows the installed
version next to it.

`package search [QUERY]` lists the packages whose name contains `QUERY`
(ignoring case) in each repository's signed index, with their newest listed
version, and marks the installed ones. It reads only the authenticated index
and fetches no registry entry, so it does not tell whether a version suits
this device: `info` and `install` check that. A repository that cannot be
reached is reported and skipped.

Both commands are read-only and need no privilege. They refuse a root or index
older than the one the device already accepted, but only commands that change
packages record a newer one.

`pkg` is not a separate program: a platform that wants the short name mounts
this same executable under it. Dependency installation will be planned by
`package install` itself, through package_manager's planner, once registry
entries declare dependencies.

## Who may change packages

Root and the members of the `administrator` group may install, upgrade and
remove packages, and add or remove repositories. The `package` task checks the
invoking user (and the parent task, so a set-user-ID mount cannot let other
users in), becomes root for the operation, then gets its identity back. The
system package directories stay writable by root only; do not make them
world-writable. `package list` and `package repository list` need no
privilege.

Application entrypoints are installed at e.g.
`/applications/bundles/dev.xila.clock/executables/clock.wasm` and can be run
through the existing Xila Wasm executable. There is no dependency solver,
capability grant prompt or launcher shortcut registration yet.

The application must be explicitly registered in each native platform's
`mount_executables!` invocation (see xila-project/platforms).

## Packages with legacy identifiers

The `dev.xila.<name>` package has a separate installation and upgrade
origin from `xila.<name>`. An upgrade of `xila.<name>` still looks for
versions of that identifier in its original repository. Install the new
identifier separately once the device and registry support per-package keys. The device keeps existing data under
`<home>/applications/xila.<name>` and uses a separate data directory
for the new installation. Save or export data you need before you remove
the old package.
