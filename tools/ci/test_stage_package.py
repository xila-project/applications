"""Regression tests for tools/ci/stage_package.py."""
import json
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "ci"))

import stage_package  # noqa: E402
from test_validate_manifests import write_fixture  # noqa: E402


class StagePackageTest(unittest.TestCase):
    def test_stages_manifest_wasm_and_shortcut(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "source"
            wasm_directory = Path(temporary) / "wasm"
            wasm_directory.mkdir()
            (wasm_directory / "clock.wasm").write_bytes(b"\0asm")
            manifest_path, _ = write_fixture(root)
            output = Path(temporary) / "staged"
            stage_package.stage(root, "clock", wasm_directory, output)
            self.assertEqual((output / "xila-package.json").read_bytes(), manifest_path.read_bytes())
            self.assertEqual((output / "executables" / "clock.wasm").read_bytes(), b"\0asm")
            self.assertEqual(json.loads((output / "shortcut.json").read_text())["name"], "Clock")

    def test_rejects_invalid_shortcut(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "source"
            wasm_directory = Path(temporary) / "wasm"
            wasm_directory.mkdir()
            (wasm_directory / "clock.wasm").write_bytes(b"\0asm")
            _, shortcut_file = write_fixture(root)
            shortcut = json.loads(shortcut_file.read_text())
            shortcut["command"] = "/bin/sh"
            shortcut_file.write_text(json.dumps(shortcut))
            with self.assertRaises(SystemExit):
                stage_package.stage(root, "clock", wasm_directory, Path(temporary) / "staged")

    def test_requires_built_wasm(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "source"
            write_fixture(root)
            with self.assertRaises(SystemExit):
                stage_package.stage(root, "clock", Path(temporary), Path(temporary) / "staged")


if __name__ == "__main__":
    unittest.main()
