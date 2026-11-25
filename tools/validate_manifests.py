#!/usr/bin/env python3
"""Validate each editable version 2 package manifest against its Rust application.

With --sync, first copy each application's Cargo.toml version into its
packaging/<app>.json (used on release-plz version pull requests).
"""
import json
from pathlib import Path
import sys
import tomllib
import unicodedata

ROOT = Path(__file__).resolve().parent.parent
APPS = ("calculator", "clock", "weather", "pixel_painter", "text_editor", "synth", "digits", "dashboard", "home_assistant")
# Package names use dashes, except xila.home_assistant, which keeps its underscore.
PACKAGE_NAMES = {"home_assistant": "xila.home_assistant"}
SCHEMA = "https://raw.githubusercontent.com/xila-project/package_manager/main/schemas/developer-manifest.schema.json"
SOURCE_PREFIX = "https://github.com/xila-project/applications/tree/main/executables/"
SHORTCUT_SCHEMA = "https://raw.githubusercontent.com/xila-project/applications/main/schemas/shell-shortcut.schema.json"
ALLOWED_MANIFEST_KEYS = {
    "$schema",
    "format_version",
    "name",
    "version",
    "target",
    "xila_version",
    "capabilities",
    "application",
    "metadata",
}


def is_valid_xila_version(value: object) -> bool:
    """Return whether value is a major.minor string with 0..=65535 components and no leading zeros."""
    if not isinstance(value, str) or value.count(".") != 1:
        return False
    major, minor = value.split(".", 1)
    for component in (major, minor):
        if not 1 <= len(component) <= 5 or not component.isascii() or not component.isdigit():
            return False
        if len(component) > 1 and component.startswith("0"):
            return False
        if int(component) > 65535:
            return False
    return True


def is_control(character: str) -> bool:
    """Return whether `character` is a Unicode control character, like Rust `char::is_control`."""
    return unicodedata.category(character) == "Cc"


def crate_license(root: Path, cargo: dict, application: str) -> str:
    """Return the license declared by the crate of an application, resolving workspace inheritance."""
    license_field = cargo["package"].get("license")
    if isinstance(license_field, dict):
        if license_field.get("workspace") is not True:
            raise SystemExit(f"unsupported license inheritance in the crate of {application}")
        workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
        license_field = workspace.get("workspace", {}).get("package", {}).get("license")
        if not isinstance(license_field, str) or not license_field:
            raise SystemExit(
                f"the crate of {application} inherits the license, but [workspace.package] of the root Cargo.toml declares none"
            )
    if not isinstance(license_field, str) or not license_field:
        raise SystemExit(f"the crate of {application} declares no license")
    return license_field


def validate_catalog_metadata(application: str, metadata: object, expected_license: str) -> None:
    """Require the catalog metadata that the registry entry of a release copies.

    Every published application needs a summary, the licence of its crate
    and the source URL of its own directory. The homepage is optional.
    """
    if not isinstance(metadata, dict) or not {"summary", "license", "source"} <= metadata.keys():
        raise SystemExit(f"missing catalog metadata (summary, license, source) for {application}")
    if not metadata.keys() <= {"summary", "license", "source", "homepage"}:
        raise SystemExit(f"unexpected catalog metadata field for {application}")
    summary = metadata["summary"]
    # Apply the rules of xila-package (Metadata::validate): at most 256 UTF-8
    # bytes and no Unicode control character (general category Cc).
    if (
        not isinstance(summary, str)
        or not summary.strip()
        or len(summary.encode("utf-8")) > 256
        or any(is_control(character) for character in summary)
    ):
        raise SystemExit(f"invalid catalog summary (one line, 1 to 256 UTF-8 bytes) for {application}")
    if metadata["license"] != expected_license:
        raise SystemExit(
            f"catalog license {metadata['license']} does not match the license {expected_license} of the {application} crate"
        )
    if metadata["source"] != f"{SOURCE_PREFIX}{application}":
        raise SystemExit(f"catalog source must be {SOURCE_PREFIX}{application}")
    homepage = metadata.get("homepage")
    if homepage is not None and (
        not isinstance(homepage, str)
        or len(homepage.encode("utf-8")) > 256
        or not homepage.startswith("https://")
        or homepage == "https://"
        or any(character.isspace() or is_control(character) for character in homepage)
    ):
        raise SystemExit(f"invalid catalog homepage for {application}")


def validate_application(root: Path, application: str, sync: bool = False) -> None:
    """Validate one application's packaging manifest and graphical Shell shortcut."""
    cargo = tomllib.loads((root / "executables" / application / "Cargo.toml").read_text(encoding="utf-8"))
    manifest_file = root / "packaging" / f"{application}.json"
    metadata = json.loads(manifest_file.read_text(encoding="utf-8"))
    if not isinstance(metadata, dict):
        raise SystemExit(f"developer manifest must be a JSON object for {application}")
    if sync and metadata.get("version") != cargo["package"]["version"]:
        metadata["version"] = cargo["package"]["version"]
        manifest_file.write_text(json.dumps(metadata, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    if "files" in metadata or "sha256" in metadata:
        raise SystemExit(f"generated file inventory belongs in the built runtime manifest: {application}")
    if not metadata.keys() <= ALLOWED_MANIFEST_KEYS:
        raise SystemExit(f"unexpected field in developer manifest for {application}")
    format_version = metadata.get("format_version")
    if type(format_version) is not int or format_version != 2 or metadata.get("$schema") != SCHEMA:
        raise SystemExit(f"unexpected developer manifest version/schema for {application}")
    expected = (PACKAGE_NAMES.get(application, f"xila.{application.replace('_', '-')}"), cargo["package"]["version"], "wasm32-wasip1")
    actual = tuple(metadata.get(key) for key in ("name", "version", "target"))
    if actual != expected:
        raise SystemExit(f"invalid package identity/version/target for {application}")
    validate_catalog_metadata(application, metadata.get("metadata"), crate_license(root, cargo, application))
    if not is_valid_xila_version(metadata.get("xila_version")):
        raise SystemExit(f"invalid xila_version for {application}")
    if metadata.get("capabilities") != []:
        raise SystemExit(f"review package capability request for {application}")
    application_metadata = metadata.get("application")
    if (
        not isinstance(application_metadata, dict)
        or application_metadata.get("entrypoint") != f"executables/{application}.wasm"
    ):
        raise SystemExit(f"invalid bundle entrypoint for {application}")
    if not isinstance(application_metadata.get("display_name"), str) or not application_metadata["display_name"]:
        raise SystemExit(f"missing display name for {application}")
    if application_metadata.get("icon") != "" or application_metadata.get("show_in_launcher") is not True:
        raise SystemExit(f"unexpected application presentation metadata for {application}")

    # The application owns its graphical Shell shortcut. It travels in the
    # signed bundle as shortcut.json, never as a field in packaging/*.json.
    shortcut_file = root / "executables" / application / "shortcut.json"
    shortcut = json.loads(shortcut_file.read_text(encoding="utf-8"))
    required = {"$schema", "name", "command", "arguments", "terminal", "icon_string", "icon_color"}
    if not isinstance(shortcut, dict) or set(shortcut) != required:
        raise SystemExit(f"invalid shortcut fields for {application}")
    if shortcut.get("$schema") != SHORTCUT_SCHEMA:
        raise SystemExit(f"invalid graphical Shell shortcut schema for {application}")
    if shortcut.get("name") != application_metadata["display_name"]:
        raise SystemExit(f"shortcut name disagrees with package display name for {application}")
    if shortcut.get("command") != "/applications/executables/wasm":
        raise SystemExit(f"shortcut must invoke Xila's Wasm runtime for {application}")
    if shortcut.get("arguments") != [f"@bundle/executables/{application}.wasm"]:
        raise SystemExit(f"shortcut must refer to its own bundle entrypoint for {application}")
    if type(shortcut.get("terminal")) is not bool:
        raise SystemExit(f"invalid terminal shortcut flag for {application}")
    icon = shortcut.get("icon_string")
    color = shortcut.get("icon_color")
    if not isinstance(icon, str) or not icon or len(icon) > 8:
        raise SystemExit(f"invalid shortcut icon for {application}")
    if not isinstance(color, list) or len(color) != 3 or any(
        type(component) is not int or component < 0 or component > 255 for component in color
    ):
        raise SystemExit(f"invalid shortcut RGB color for {application}")
    if shortcut_file.stat().st_size > 2048:
        raise SystemExit(f"shortcut exceeds graphical Shell read limit for {application}")


def validate_all(root: Path = ROOT, sync: bool = False) -> None:
    """Validate every official WASI application manifest and shortcut."""
    for application in APPS:
        validate_application(root, application, sync=sync)


def main(arguments: list[str] | None = None) -> None:
    arguments = sys.argv[1:] if arguments is None else arguments
    sync = arguments == ["--sync"]
    if arguments and not sync:
        raise SystemExit("usage: validate_manifests.py [--sync]")
    validate_all(ROOT, sync=sync)
    print(
        f"All {len(APPS)} version 2 package manifests and application-owned graphical Shell shortcuts "
        "match Cargo identities, versions, Xila version declarations and entrypoints."
    )


if __name__ == "__main__":
    main()
