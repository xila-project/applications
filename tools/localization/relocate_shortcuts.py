#!/usr/bin/env python3
"""Place Tolgee's legacy metadata namespace in its application-owned crate."""
from pathlib import Path
import shutil

root = Path(__file__).resolve().parents[2]
source = root / "libraries/application_metadata/locales"
destination = root / "libraries/application_shortcuts/locales"
if source.is_dir():
    destination.mkdir(parents=True, exist_ok=True)
    for catalog in source.glob("*.json"):
        shutil.move(str(catalog), str(destination / catalog.name))
    if not any(source.iterdir()):
        source.rmdir()
        if not any(source.parent.iterdir()):
            source.parent.rmdir()
