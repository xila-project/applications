#!/usr/bin/env python3
"""Stage one WASI application as the input directory of `xila-package bundle build`.

The directory holds the developer manifest as xila-package.json, the built
Wasm at the manifest's entry point and the application-owned shortcut.json.
The manifest and the shortcut are validated first with tools/validate_manifests.py,
so the checks that used to live in package_manager's package_wasm.py stay here.
"""
import argparse
import json
from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))

import validate_manifests  # noqa: E402


def stage(root: Path, application: str, wasm_directory: Path, output: Path) -> Path:
    """Validate and stage one application; return the staged directory."""
    if application not in validate_manifests.APPS:
        raise SystemExit(f"unknown application: {application}")
    validate_manifests.validate_application(root, application)
    wasm = wasm_directory / f"{application}.wasm"
    if not wasm.is_file() or wasm.is_symlink():
        raise SystemExit(f"built Wasm file missing: {wasm}")
    manifest_file = root / "packaging" / f"{application}.json"
    entrypoint = json.loads(manifest_file.read_text(encoding="utf-8"))["application"]["entrypoint"]
    if output.exists():
        raise SystemExit(f"staging directory already exists: {output}")
    destination = output.joinpath(*entrypoint.split("/"))
    destination.parent.mkdir(parents=True)
    shutil.copyfile(manifest_file, output / "xila-package.json")
    shutil.copyfile(wasm, destination)
    shutil.copyfile(root / "executables" / application / "shortcut.json", output / "shortcut.json")
    return output


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--application", required=True)
    parser.add_argument("--wasm-directory", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    staged = stage(ROOT, arguments.application, arguments.wasm_directory, arguments.output)
    print(f"Staged {arguments.application} in {staged}")


if __name__ == "__main__":
    main()
