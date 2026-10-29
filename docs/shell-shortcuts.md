# Shell shortcuts: presentation metadata (experimental v1)

The graphical Shell owns app discovery and launcher presentation. It reads
**`shortcut.json` only** and does not inspect/authorize package manifests.
A shortcut is *not* the signed package manifest, a publisher identity,
a capability grant, or a proof that its executable is trustworthy.

Editor/CI schema:
[`schemas/shell-shortcut.schema.json`](../schemas/shell-shortcut.schema.json)
(public `$schema` URI:
`https://raw.githubusercontent.com/xila-project/applications/main/schemas/shell-shortcut.schema.json`).

Example placed at `/applications/bundles/xila.calculator/shortcut.json`:

```json
{
  "$schema": "https://raw.githubusercontent.com/xila-project/applications/main/schemas/shell-shortcut.schema.json",
  "name": "Calculator",
  "command": "/applications/executables/wasm",
  "arguments": ["@bundle/executables/calculator.wasm"],
  "terminal": false,
  "icon_string": "Ca",
  "icon_color": [158, 158, 158]
}
```

The Shell expands arguments (or commands) beginning with `@bundle/` relative
to the directory containing this shortcut, and rejects `.`/`..`/empty
components and backslashes. Other arguments are passed unchanged.
The package builder may ship this JSON as an ordinary declared payload; its
SHA-256 must then appear in the **separate signed package manifest**.
The Shell does not read the manifest, nor does it interpret a self-declared
signature or privilege.

The drawer discovers bundle shortcuts lazily when opened, minimizing startup
I/O and making newly available bundles visible on refresh. Legacy
`/configuration/shared/applications/shortcuts/*.json` shortcuts remain readable
during migration, chiefly for native/system apps. They are *data files* seeded
by Platforms, not produced by a guest `__install` hook. The Dock continues
showing open windows, not every installed shortcut.

**Scope:** The initial scanner supports one active directory directly beneath
`/applications/bundles/<id>/`. Version selection / atomic activation / rollback
and trust-aware installation remain separate future work; do not display
multiple unverified versions or assume a symlink always works on LittleFS.
A Shell shortcut never bypasses the existing POSIX-style UID/GID/mode checks.
