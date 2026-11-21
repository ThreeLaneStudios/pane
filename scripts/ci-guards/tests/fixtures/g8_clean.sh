#!/usr/bin/env bash
# G8 fixture: same dangerous command shapes, but each is textually guarded
# by a PANE_TEST_ROOT check above it in this file — must NOT be caught by
# G8. Also includes the known-safe non-matches (scratch-dir rm -rf and a
# Pane.app path) that must never trigger regardless of guarding.
set -euo pipefail

: "${PANE_TEST_ROOT:?PANE_TEST_ROOT must be set before this script runs}"

KEYCHAIN_SERVICE="com.threelane.pane.test"
for ACCT in "ai_provider_key"; do
    security delete-generic-password -s "$KEYCHAIN_SERVICE" -a "$ACCT" 2>/dev/null || true
done

[ -z "${PANE_TEST_ROOT:-}" ] && exit 1
rm -rf ~/Pane
rm -rf "$HOME/Pane"

# Never the dangerous shape at all — scratch dir / app bundle, not the real
# user data root. Deliberately kept on its own, unambiguous line so a test
# asserting the anchor's path-shape safety doesn't also see a legitimate
# guarded ~/Pane line a few lines up (see g8_path_shape_safe.sh for that).
MOUNT_POINT=$(mktemp -d)
rm -rf "$MOUNT_POINT/Pane.app"
