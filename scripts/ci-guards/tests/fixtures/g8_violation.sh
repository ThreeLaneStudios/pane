#!/usr/bin/env bash
# G8 fixture: deliberately UNGUARDED patterns — no PANE_TEST_ROOT check
# anywhere above either dangerous line. Must be caught by G8.
set -euo pipefail

KEYCHAIN_SERVICE="com.threelane.pane"
for ACCT in "ai_provider_key"; do
    security delete-generic-password -s "$KEYCHAIN_SERVICE" -a "$ACCT" 2>/dev/null || true
done

rm -rf ~/Pane
rm -rf "$HOME/Pane"
