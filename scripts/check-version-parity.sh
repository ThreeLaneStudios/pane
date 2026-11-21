#!/usr/bin/env bash
# Version parity check: ensure package.json, Cargo.toml, tauri.conf.json,
# and Cargo.lock all declare the same version string.
#
# The Cargo.lock leg was added by #3056 after #3048's bump raised the three
# manifests but the standing no-lock-changes-in-PRs rule (no documented
# carve-out at the time, see docs/REVIEW-CHECKLIST.md §8) stripped the lock
# back out, and nothing here noticed — the lock sat one version behind on
# main until this check existed. Reuses the gateway's own
# name-keyed-awk parsing (scripts/lib/cargo-lock-version.sh) rather than a
# second hand-rolled parser, so the two lock-parsing paths can't diverge.
#
# Run: bash scripts/check-version-parity.sh
# CI: .github/workflows/ci.yml (job `version-parity`, every PR to main,
#     no path filter — the three manifests can drift from the lock on ANY
#     PR that bumps a version, not just ones that touch src-tauri/; a path
#     filter here would reproduce the exact gap #3056 closed).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# shellcheck source=./lib/cargo-lock-version.sh
source "$SCRIPT_DIR/lib/cargo-lock-version.sh"

PKG_JSON="$REPO_ROOT/package.json"
CARGO_TOML="$REPO_ROOT/src-tauri/Cargo.toml"
TAURI_CONF="$REPO_ROOT/src-tauri/tauri.conf.json"
CARGO_LOCK="$REPO_ROOT/src-tauri/Cargo.lock"

errors=0

if [[ ! -f "$PKG_JSON" ]]; then
  echo "ERROR: package.json not found at $PKG_JSON" >&2
  exit 1
fi
if [[ ! -f "$CARGO_TOML" ]]; then
  echo "ERROR: src-tauri/Cargo.toml not found at $CARGO_TOML" >&2
  exit 1
fi
if [[ ! -f "$TAURI_CONF" ]]; then
  echo "ERROR: src-tauri/tauri.conf.json not found at $TAURI_CONF" >&2
  exit 1
fi
if [[ ! -f "$CARGO_LOCK" ]]; then
  echo "ERROR: src-tauri/Cargo.lock not found at $CARGO_LOCK" >&2
  exit 1
fi

# Extract versions
PKG_VERSION=$(grep -m1 '"version"' "$PKG_JSON" | sed 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
CARGO_VERSION=$(grep -m1 '^version' "$CARGO_TOML" | sed 's/.*"\([^"]*\)".*/\1/')
TAURI_VERSION=$(grep -m1 '"version"' "$TAURI_CONF" | sed 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')
LOCK_VERSION=$(cargo_lock_package_version "$CARGO_LOCK" "pane")

echo "package.json:              $PKG_VERSION"
echo "src-tauri/Cargo.toml:      $CARGO_VERSION"
echo "src-tauri/tauri.conf.json: $TAURI_VERSION"
echo "src-tauri/Cargo.lock:      $LOCK_VERSION"

if [[ "$PKG_VERSION" != "$CARGO_VERSION" ]]; then
  echo "❌ Version mismatch: package.json ($PKG_VERSION) ≠ Cargo.toml ($CARGO_VERSION)" >&2
  errors=$((errors + 1))
fi

if [[ "$PKG_VERSION" != "$TAURI_VERSION" ]]; then
  echo "❌ Version mismatch: package.json ($PKG_VERSION) ≠ tauri.conf.json ($TAURI_VERSION)" >&2
  errors=$((errors + 1))
fi

if [[ "$CARGO_VERSION" != "$TAURI_VERSION" ]]; then
  echo "❌ Version mismatch: Cargo.toml ($CARGO_VERSION) ≠ tauri.conf.json ($TAURI_VERSION)" >&2
  errors=$((errors + 1))
fi

if [[ "$CARGO_VERSION" != "$LOCK_VERSION" ]]; then
  echo "❌ Version mismatch: Cargo.toml ($CARGO_VERSION) ≠ Cargo.lock ($LOCK_VERSION)" >&2
  echo "   Run cargo check --manifest-path src-tauri/Cargo.toml and commit Cargo.lock." >&2
  errors=$((errors + 1))
fi

if [[ $errors -gt 0 ]]; then
  echo ""
  echo "❌ Version parity check FAILED — package.json, Cargo.toml, tauri.conf.json, and Cargo.lock must all declare the same version."
  exit 1
fi

echo "✅ Version parity check passed: $PKG_VERSION"
