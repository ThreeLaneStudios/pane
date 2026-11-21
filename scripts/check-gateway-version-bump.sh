#!/usr/bin/env bash
# Require a gateway semver increase whenever a PR changes src-gateway/.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./lib/cargo-lock-version.sh
source "$SCRIPT_DIR/lib/cargo-lock-version.sh"

BASE_REF="${1:-origin/${GITHUB_BASE_REF:-prod}}"
GATEWAY_MANIFEST="src-gateway/Cargo.toml"
GATEWAY_LOCK="src-gateway/Cargo.lock"

changed=$(git diff --name-only "${BASE_REF}...HEAD" -- src-gateway/)
if [[ -z "$changed" ]]; then
  echo "✅ No src-gateway changes — version bump not required"
  exit 0
fi

current_version=$(sed -n 's/^version = "\([0-9][0-9.]*\)"/\1/p' "$GATEWAY_MANIFEST" | head -1)
base_version=$(git show "${BASE_REF}:${GATEWAY_MANIFEST}" | sed -n 's/^version = "\([0-9][0-9.]*\)"/\1/p' | head -1)
lock_version=$(cargo_lock_package_version "$GATEWAY_LOCK" "pane-gateway")

if [[ ! "$current_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "❌ Current gateway version is not valid X.Y.Z semver: '$current_version'" >&2
  exit 1
fi
if [[ ! "$base_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "❌ Base gateway version is not valid X.Y.Z semver: '$base_version'" >&2
  exit 1
fi
if [[ "$lock_version" != "$current_version" ]]; then
  echo "❌ Gateway versions in Cargo.toml and Cargo.lock do not match." >&2
  echo "   Cargo.toml: $current_version" >&2
  echo "   Cargo.lock: $lock_version" >&2
  echo "   Run cargo check --manifest-path src-gateway/Cargo.toml and commit Cargo.lock." >&2
  exit 1
fi

IFS=. read -r current_major current_minor current_patch <<< "$current_version"
IFS=. read -r base_major base_minor base_patch <<< "$base_version"

if (( current_major > base_major ||
      (current_major == base_major && current_minor > base_minor) ||
      (current_major == base_major && current_minor == base_minor && current_patch > base_patch) )); then
  echo "✅ Gateway changed and version increased: $base_version → $current_version"
  exit 0
fi

echo "❌ src-gateway changed, but its version did not increase." >&2
echo "   Base:    $base_version" >&2
echo "   Current: $current_version" >&2
echo "   Increase the version in src-gateway/Cargo.toml and src-gateway/Cargo.lock." >&2
echo "" >&2
echo "Changed gateway files:" >&2
echo "$changed" >&2
exit 1
