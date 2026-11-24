#!/usr/bin/env bash
# Verify the package the Xila package action produced with xila-package, write
# the SHA-256 sidecar that releases attach, and report the digest.
#
# Environment: CI_PACKAGE_DIRECTORY (the action's output directory),
# CI_PACKAGE_MANIFEST (the developer manifest, packaging/<application>.json),
# EXPECTED_SHA256 (optional; the digest the action reported),
# XILA_PACKAGE_EXECUTABLE (the installed xila-package, the action's `xila-package-path` output),
# GITHUB_STEP_SUMMARY (optional).
set -euo pipefail

directory="${CI_PACKAGE_DIRECTORY:?}"
executable="${XILA_PACKAGE_EXECUTABLE:?}"
[[ -x "$executable" ]] || { echo "::error::$executable is not an executable"; exit 1; }

shopt -s nullglob
archives=("$directory"/*.xpk)
[[ ${#archives[@]} -eq 1 ]] || { echo "::error::expected one .xpk in $directory, found ${#archives[@]}"; exit 1; }
archive="${archives[0]}"
public_key="$archive.pub"
[[ -f "$public_key" ]] || { echo "::error::missing $public_key"; exit 1; }

"$executable" verify "$archive" "$public_key"
(cd "$directory" && sha256sum "$(basename "$archive")" > "$(basename "$archive").sha256")
digest="$(cut -d ' ' -f 1 < "$archive.sha256")"
if [[ -n "${EXPECTED_SHA256:-}" && "$digest" != "$EXPECTED_SHA256" ]]; then
  echo "::error::package digest $digest differs from the action's $EXPECTED_SHA256"
  exit 1
fi

# The registry entry of a release copies the catalog metadata from the signed
# manifest. Generate an entry from the archive and require that it carries the
# summary, licence and source of the developer manifest (packaging/<name>.json).
manifest="${CI_PACKAGE_MANIFEST:?}"
entry_directory="$(mktemp -d)"
trap 'rm -rf -- "$entry_directory"' EXIT
entry="$entry_directory/entry.json"
# The archive URL is a placeholder: this check compares only the metadata.
"$executable" registry entry "$archive" "$public_key" xila \
  "https://github.com/xila-project/applications/releases/download/package/$(basename "$archive")" "$entry"
# The homepage is optional: a missing and a null homepage are equal.
normalize='.metadata | if . == null then null else {summary, license, source, homepage: (.homepage // null)} end'
expected="$(jq -cS "$normalize" "$manifest")"
actual="$(jq -cS "$normalize" "$entry")"
if [[ "$expected" == "null" ]]; then
  echo "::error::$manifest has no catalog metadata (summary, license, source)"
  exit 1
fi
if [[ "$actual" != "$expected" ]]; then
  echo "::error::registry entry metadata ($actual) differs from the developer manifest ($expected)"
  exit 1
fi
echo "Registry entry carries the catalog metadata of $(basename "$manifest")"
echo "Verified $(basename "$archive") (SHA-256 $digest)"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    echo "| Package | SHA-256 | Signature |"
    echo "| --- | --- | --- |"
    echo "| \`$(basename "$archive")\` | \`$digest\` | ${CI_PACKAGE_SIGNATURE:-verified} |"
  } >> "$GITHUB_STEP_SUMMARY"
fi
