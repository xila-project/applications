#!/usr/bin/env python3
"""Validate each editable v1 package manifest against its Rust application.

With --sync, first copy each application's Cargo.toml version into its
packaging/<app>.json (used on release-plz version pull requests).
"""
import json
from pathlib import Path
import sys
import tomllib

ROOT = Path(__file__).resolve().parent.parent
APPS = ("calculator", "clock", "weather", "pixel_painter", "text_editor", "synth", "digits")
SCHEMA = "https://raw.githubusercontent.com/xila-project/package_manager/main/schemas/developer-manifest.schema.json"
SHORTCUT_SCHEMA = "https://raw.githubusercontent.com/xila-project/applications/main/schemas/shell-shortcut.schema.json"

SYNC = sys.argv[1:] == ["--sync"]
if sys.argv[1:] and not SYNC:
    raise SystemExit("usage: validate_manifests.py [--sync]")

for app in APPS:
    cargo = tomllib.loads((ROOT / "executables" / app / "Cargo.toml").read_text())
    manifest_file = ROOT / "packaging" / f"{app}.json"
    metadata = json.loads(manifest_file.read_text())
    if SYNC and metadata.get("version") != cargo["package"]["version"]:
        metadata["version"] = cargo["package"]["version"]
        manifest_file.write_text(json.dumps(metadata, indent=2, ensure_ascii=False) + "\n")
    expected = (f"xila.{app.replace('_', '-')}", cargo["package"]["version"], "wasm32-wasip1", 1)
    actual = tuple(metadata.get(k) for k in ("name", "version", "target", "minimum_xila_abi"))
    if actual != expected:
        raise SystemExit(f"invalid package identity/version/ABI for {app}")
    if metadata.get("format_version") != 1 or metadata.get("$schema") != SCHEMA:
        raise SystemExit(f"unexpected developer manifest version/schema for {app}")
    if "files" in metadata or "sha256" in metadata:
        raise SystemExit(f"generated file inventory belongs in the built runtime manifest: {app}")
    if metadata.get("capabilities") != []:
        raise SystemExit(f"review package capability request for {app}")
    app_metadata = metadata.get("application")
    if not isinstance(app_metadata, dict) or app_metadata.get("entrypoint") != f"executables/{app}.wasm":
        raise SystemExit(f"invalid bundle entrypoint for {app}")
    if not isinstance(app_metadata.get("display_name"), str) or not app_metadata["display_name"]:
        raise SystemExit(f"missing display name for {app}")
    if app_metadata.get("icon") != "" or app_metadata.get("show_in_launcher") is not True:
        raise SystemExit(f"unexpected application presentation metadata for {app}")

    # The application owns its graphical Shell shortcut. It travels in the
    # signed bundle as shortcut.json, never as a field in packaging/*.json.
    shortcut_file = ROOT / "executables" / app / "shortcut.json"
    shortcut = json.loads(shortcut_file.read_text(encoding="utf-8"))
    allowed = {"$schema", "name", "command", "arguments", "terminal", "icon_string", "icon_color"}
    required = allowed
    if not isinstance(shortcut, dict) or set(shortcut) != required:
        raise SystemExit(f"invalid shortcut fields for {app}")
    if shortcut.get("$schema") != SHORTCUT_SCHEMA:
        raise SystemExit(f"invalid graphical Shell shortcut schema for {app}")
    if shortcut.get("name") != app_metadata["display_name"]:
        raise SystemExit(f"shortcut name disagrees with package display name for {app}")
    if shortcut.get("command") != "/applications/executables/wasm":
        raise SystemExit(f"shortcut must invoke Xila's Wasm runtime for {app}")
    if shortcut.get("arguments") != [f"@bundle/executables/{app}.wasm"]:
        raise SystemExit(f"shortcut must refer to its own bundle entrypoint for {app}")
    if type(shortcut.get("terminal")) is not bool:
        raise SystemExit(f"invalid terminal shortcut flag for {app}")
    icon = shortcut.get("icon_string")
    color = shortcut.get("icon_color")
    if not isinstance(icon, str) or not icon or len(icon) > 8:
        raise SystemExit(f"invalid shortcut icon for {app}")
    if not isinstance(color, list) or len(color) != 3 or any(
        type(component) is not int or component < 0 or component > 255 for component in color
    ):
        raise SystemExit(f"invalid shortcut RGB color for {app}")
    if shortcut_file.stat().st_size > 2048:
        raise SystemExit(f"shortcut exceeds graphical Shell read limit for {app}")
print("All seven v1 package manifests and application-owned graphical Shell shortcuts match Cargo identities, versions, ABI and entrypoints.")
