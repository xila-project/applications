#!/usr/bin/env python3
"""Validate package identities against the application Cargo manifests."""
import json
import os
from pathlib import Path
import tomllib

ROOT = Path(__file__).resolve().parent.parent
APPS = ("calculator", "clock", "weather", "pixel_painter", "text_editor", "synth")

for app in APPS:
    cargo = tomllib.loads((ROOT / "executables" / app / "Cargo.toml").read_text())
    manifest = json.loads((ROOT / "packaging" / f"{app}.json").read_text())
    expected = (f"xila.{app.replace('_', '-')}", cargo["package"]["version"], "wasm32-wasip1", 1)
    actual = (manifest["name"], manifest["version"], manifest["target"], manifest["minimum_xila_abi"])
    if actual != expected or len(manifest["files"]) != 1 or manifest["files"][0]["path"] != f"bin/{app}.wasm":
        raise SystemExit(f"invalid package metadata for {app}")
    if os.environ.get("XILA_RELEASE_TAG") and os.environ["XILA_RELEASE_TAG"] != "v" + manifest["version"]:
        raise SystemExit(f"release tag does not match {app} version")
print("Application package identities, versions and ABI requirements are consistent.")
