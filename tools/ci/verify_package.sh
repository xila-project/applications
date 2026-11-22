#!/usr/bin/env bash
# Verify the package the Xila package action produced with xila-package, write
# the SHA-256 sidecar that releases attach, and report the digest.
#
# Environment: CI_PACKAGE_DIRECTORY (the action's output directory),
# EXPECTED_SHA256 (optional; the digest the action reported),
# XILA_PACKAGE_EXECUTABLE (optional; found under RUNNER_TEMP when unset),
# GITHUB_STEP_SUMMARY (optional).
set -euo pipefail

directory="${CI_PACKAGE_DIRECTORY:?}"
executable="${XILA_PACKAGE_EXECUTABLE:-}"
if [[ -z "$executable" ]]; then
  # The action installs the pinned, digest-checked release below RUNNER_TEMP.
  mapfile -t found < <(find "${RUNNER_TEMP:?}" -path '*/xila-package-install.*/xila-package' -type f)
  [[ ${#found[@]} -eq 1 ]] || { echo "::error::expected one installed xila-package, found ${#found[@]}"; exit 1; }
  executable="${found[0]}"
fi

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
