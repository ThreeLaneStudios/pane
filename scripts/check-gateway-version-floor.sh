#!/usr/bin/env bash
# §22C.11A.5 — CI guard: gateway version vs MIN_GATEWAY_VERSION floor
#
# PASS: gateway_version >= MIN_GATEWAY_VERSION
# FAIL: gateway_version <  MIN_GATEWAY_VERSION (the compatibility floor is ahead of the release)
#
# Run: bash scripts/check-gateway-version-floor.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

GATEWAY_CARGO="$REPO_ROOT/src-gateway/Cargo.toml"
NOTES_RS="$REPO_ROOT/src-tauri/src/notes.rs"

if [[ ! -f "$GATEWAY_CARGO" ]]; then
  echo "ERROR: src-gateway/Cargo.toml not found at $GATEWAY_CARGO" >&2
  exit 1
fi

if [[ ! -f "$NOTES_RS" ]]; then
  echo "ERROR: src-tauri/src/notes.rs not found at $NOTES_RS" >&2
  exit 1
fi

# Extract gateway version (first `version = "X.Y.Z"` line in its Cargo.toml)
GATEWAY_VERSION=$(grep -m1 '^version\s*=' "$GATEWAY_CARGO" | sed 's/.*"\(.*\)".*/\1/')
# Extract MIN_GATEWAY_VERSION constant from notes.rs
MIN_GATEWAY_VERSION=$(grep 'MIN_GATEWAY_VERSION' "$NOTES_RS" | grep 'const\|let' | sed 's/.*"\(.*\)".*/\1/' | head -1)

if [[ -z "$GATEWAY_VERSION" ]]; then
  echo "ERROR: Could not extract version from $GATEWAY_CARGO" >&2
  exit 1
fi

if [[ -z "$MIN_GATEWAY_VERSION" ]]; then
  echo "ERROR: Could not extract MIN_GATEWAY_VERSION from $NOTES_RS" >&2
  exit 1
fi

echo "Gateway version:     $GATEWAY_VERSION"
echo "MIN_GATEWAY_VERSION: $MIN_GATEWAY_VERSION"

# Parse semver into major/minor/patch — split on dots
parse_semver() {
  local ver="$1"
  # Strip any leading 'v'
  ver="${ver#v}"
  IFS='.' read -r major minor patch <<< "$ver"
  # Default to 0 if not numeric
  major="${major//[^0-9]/}"
  minor="${minor//[^0-9]/}"
  patch="${patch//[^0-9]/}"
  echo "${major:-0} ${minor:-0} ${patch:-0}"
}

# semver_cmp <a> <b>
# Returns: 0 if a == b, 1 if a > b, 2 if a < b
semver_cmp() {
  read -r a_maj a_min a_pat <<< "$(parse_semver "$1")"
  read -r b_maj b_min b_pat <<< "$(parse_semver "$2")"

  if (( a_maj != b_maj )); then
    (( a_maj > b_maj )) && echo 1 || echo 2
    return
  fi
  if (( a_min != b_min )); then
    (( a_min > b_min )) && echo 1 || echo 2
    return
  fi
  if (( a_pat != b_pat )); then
    (( a_pat > b_pat )) && echo 1 || echo 2
    return
  fi
  echo 0
}

CMP=$(semver_cmp "$GATEWAY_VERSION" "$MIN_GATEWAY_VERSION")

case "$CMP" in
  0)
    echo "✅ PASS: gateway version ($GATEWAY_VERSION) matches MIN_GATEWAY_VERSION ($MIN_GATEWAY_VERSION)"
    exit 0
    ;;
  1)
    echo "✅ PASS: gateway version ($GATEWAY_VERSION) is at or above compatibility floor ($MIN_GATEWAY_VERSION)"
    exit 0
    ;;
  2)
    echo "" >&2
    echo "❌ FAIL: gateway version ($GATEWAY_VERSION) is BELOW MIN_GATEWAY_VERSION ($MIN_GATEWAY_VERSION)." >&2
    echo "   The floor cannot be ahead of the actual gateway — something is wrong." >&2
    echo "   Check src-gateway/Cargo.toml and src-tauri/src/notes.rs." >&2
    echo "   (spec: §22C.11A.5, relates to #1508)" >&2
    exit 1
    ;;
  *)
    echo "ERROR: unexpected semver_cmp result: $CMP" >&2
    exit 1
    ;;
esac
