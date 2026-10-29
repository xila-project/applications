#!/usr/bin/env python3
"""Validate each editable v1 package manifest against its Rust application."""
import json
import os
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parent.parent
APPS = ("calculator", "clock", "weather", "pixel_painter", "text_editor", "synth")
SCHEMA = "https://raw.githubusercontent.com/xila-project/package_manager/main/schemas/developer-manifest.schema.json"

for app in APPS:
    cargo = tomllib.loads((ROOT / "executables" / app / "Cargo.toml").read_text())
    metadata = json.loads((ROOT / "packaging" / f"{app}.json").read_text())
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
    if not isinstance(app_metadata, dict) or app_metadata.get("entrypoint") != f"bin/{app}.wasm":
        raise SystemExit(f"invalid bundle entrypoint for {app}")
    if not isinstance(app_metadata.get("display_name"), str) or not app_metadata["display_name"]:
        raise SystemExit(f"missing display name for {app}")
    if app_metadata.get("icon") != "" or app_metadata.get("show_in_launcher") is not True:
        raise SystemExit(f"unexpected application presentation metadata for {app}")
    if os.environ.get("XILA_RELEASE_TAG") and os.environ["XILA_RELEASE_TAG"] != "v" + metadata["version"]:
        raise SystemExit(f"release tag does not match {app} version")
print("All six editable v1 application manifests match Cargo identities, versions, ABI and entrypoints.")
