"""Regression tests for tools/validate_manifests.py."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools"))

import validate_manifests  # noqa: E402


def write_fixture(root: Path, application: str = "clock", version: str = "0.2.0") -> tuple[Path, Path]:
    """Write a minimal valid Cargo.toml, packaging manifest, and shortcut under root."""
    package_name = f"xila-{application.replace('_', '-')}"
    crate_directory = root / "executables" / application
    crate_directory.mkdir(parents=True, exist_ok=True)
    (crate_directory / "Cargo.toml").write_text(
        f'[package]\nname = "{package_name}"\nversion = "{version}"\n',
        encoding="utf-8",
    )
    (crate_directory / "shortcut.json").write_text(
        json.dumps(
            {
                "$schema": validate_manifests.SHORTCUT_SCHEMA,
                "name": "Clock",
                "command": "/applications/executables/wasm",
                "arguments": [f"@bundle/executables/{application}.wasm"],
                "terminal": False,
                "icon_string": "C",
                "icon_color": [1, 2, 3],
            }
        ),
        encoding="utf-8",
    )
    packaging_directory = root / "packaging"
    packaging_directory.mkdir(parents=True, exist_ok=True)
    manifest_path = packaging_directory / f"{application}.json"
    manifest_path.write_text(
        json.dumps(
            {
                "format_version": 2,
                "name": f"xila.{application.replace('_', '-')}",
                "version": version,
                "target": "wasm32-wasip1",
                "xila_version": "0.1",
                "capabilities": [],
                "application": {
                    "entrypoint": f"executables/{application}.wasm",
                    "display_name": "Clock",
                    "icon": "",
                    "show_in_launcher": True,
                },
                "$schema": validate_manifests.SCHEMA,
            }
        ),
        encoding="utf-8",
    )
    return manifest_path, crate_directory / "shortcut.json"


class ValidateManifestsTests(unittest.TestCase):
    def test_repository_manifests_pass_validation(self):
        validate_manifests.validate_all(ROOT)

    def test_accepts_valid_xila_versions(self):
        for valid in ("0.0", "0.1", "1.0", "1.12", "65535.65535"):
            with self.subTest(xila_version=valid):
                with tempfile.TemporaryDirectory() as temporary_directory:
                    root = Path(temporary_directory)
                    manifest_path, _ = write_fixture(root)
                    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                    manifest["xila_version"] = valid
                    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
                    validate_manifests.validate_application(root, "clock")

    def test_rejects_legacy_format_or_invalid_xila_version(self):
        invalid_changes = [
            {"format_version": 1},
            {"minimum_xila_abi": 1},
            {"xila_version": 1},
            {"xila_version": "0.1.0"},
            {"xila_version": "01.2"},
            {"xila_version": "1.02"},
            {"xila_version": "+1.2"},
            {"xila_version": "1.2\n"},
            {"xila_version": "65536.0"},
            {"xila_version": "0.65536"},
            {"xila_version": "1."},
            {"xila_version": ".2"},
            {"xila_version": ""},
            {"unexpected": True},
        ]
        for changes in invalid_changes:
            with self.subTest(changes=changes):
                with tempfile.TemporaryDirectory() as temporary_directory:
                    root = Path(temporary_directory)
                    manifest_path, _ = write_fixture(root)
                    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
                    manifest.update(changes)
                    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
                    with self.assertRaises(SystemExit):
                        validate_manifests.validate_application(root, "clock")

    def test_rejects_missing_xila_version(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            manifest_path, _ = write_fixture(root)
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            del manifest["xila_version"]
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            with self.assertRaises(SystemExit):
                validate_manifests.validate_application(root, "clock")

    def test_sync_updates_package_version_only(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            manifest_path, _ = write_fixture(root, version="0.1.0")
            cargo_path = root / "executables" / "clock" / "Cargo.toml"
            cargo_path.write_text('[package]\nname = "xila-clock"\nversion = "0.2.0"\n', encoding="utf-8")
            with self.assertRaises(SystemExit):
                validate_manifests.validate_application(root, "clock", sync=False)
            validate_manifests.validate_application(root, "clock", sync=True)
            updated = json.loads(manifest_path.read_text(encoding="utf-8"))
            self.assertEqual(updated["version"], "0.2.0")
            self.assertEqual(updated["format_version"], 2)
            self.assertEqual(updated["xila_version"], "0.1")


if __name__ == "__main__":
    unittest.main()
