#!/usr/bin/env bash
# Verify the package the Xila package action produced with xila-package, write
# the SHA-256 sidecar that releases attach, and report the digest.
#
# Environment: CI_PACKAGE_DIRECTORY (the action's output directory),
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
echo "Verified $(basename "$archive") (SHA-256 $digest)"
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
  {
    echo "| Package | SHA-256 | Signature |"
    echo "| --- | --- | --- |"
    echo "| \`$(basename "$archive")\` | \`$digest\` | ${CI_PACKAGE_SIGNATURE:-verified} |"
  } >> "$GITHUB_STEP_SUMMARY"
fi
