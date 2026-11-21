#!/usr/bin/env bash
# Verify upgrade fixture is current with schema version
# Exits 0 if fixture matches expected version (SCHEMA_VERSION - 1)
# Exits 1 if fixture is missing, stale, or mismatched

set -euo pipefail

# Read current schema version from migrations.rs
MIGRATIONS_FILE="src-tauri/src/migrations.rs"

if [ ! -f "$MIGRATIONS_FILE" ]; then
    echo "❌ migrations.rs not found at: $MIGRATIONS_FILE"
    echo "Run this script from the Pane repo root."
    exit 1
fi

# Extract SCHEMA_VERSION constant (e.g., const SCHEMA_VERSION: &str = "62";)
CURRENT_VERSION=$(grep -oE 'const SCHEMA_VERSION: &str = "[0-9]+"' "$MIGRATIONS_FILE" \
  | grep -oE '[0-9]+')

if [ -z "$CURRENT_VERSION" ]; then
    echo "❌ Could not parse SCHEMA_VERSION from $MIGRATIONS_FILE"
    exit 1
fi

EXPECTED_FIXTURE_VERSION=$((CURRENT_VERSION - 1))
FIXTURE_FILE="tests/fixtures/upgrade-from-v${EXPECTED_FIXTURE_VERSION}.db"

echo "[VERIFY-FIXTURE] Current schema version: v${CURRENT_VERSION}"
echo "[VERIFY-FIXTURE] Expected fixture version: v${EXPECTED_FIXTURE_VERSION}"
echo "[VERIFY-FIXTURE] Fixture path: $FIXTURE_FILE"

# Check if fixture exists
if [ ! -f "$FIXTURE_FILE" ]; then
    echo ""
    echo "❌ Upgrade fixture missing: $FIXTURE_FILE"
    echo ""
    echo "Schema is at v${CURRENT_VERSION}; expected fixture for v${EXPECTED_FIXTURE_VERSION}."
    echo "When bumping SCHEMA_VERSION, create a new fixture:"
    echo "  1. Build a v${EXPECTED_FIXTURE_VERSION}-stamped database"
    echo "  2. Save it as tests/fixtures/upgrade-from-v${EXPECTED_FIXTURE_VERSION}.db"
    echo "  3. Add seed data (project, note, settings)"
    echo ""
    echo "See: docs/audits/2026-04-30-adr0018-workflow-stubs.md"
    exit 1
fi

# Read stamped version from fixture
STAMPED_VERSION=$(sqlite3 "$FIXTURE_FILE" "SELECT value FROM ai_settings WHERE key = 'schema_version'" 2>/dev/null || echo "")

if [ -z "$STAMPED_VERSION" ]; then
    echo "❌ Could not read schema_version from fixture"
    echo "The fixture may be corrupted or missing the ai_settings table."
    exit 1
fi

# Verify stamped version matches expected.
# Two valid cases:
#   1. Pre-launch bump (canonical): fixture is stamped at CURRENT_VERSION-1.
#      No migration arm exists (§1.10); app emits SCHEMA_MISMATCH, which is
#      the correct and expected outcome asserted by smoke-launch.sh.
#   2. Post-launch / migration arm shipped: fixture is also at CURRENT_VERSION-1,
#      but a migration arm now exists.  App applies it and emits a ready signal.
#      (SCHEMA_POLICY=post-launch in smoke-launch.sh asserts the ready signal.)
#
# NOTE: A fixture stamped at CURRENT_VERSION would mean the app sees "Schema
# verified" and skips migration entirely — that fixture does NOT exercise the
# upgrade path.  It is NOT a valid upgrade fixture and is rejected below.
if [ "$STAMPED_VERSION" != "$EXPECTED_FIXTURE_VERSION" ]; then
    echo ""
    echo "❌ Fixture stamped version mismatch!"
    echo "   Expected: v${EXPECTED_FIXTURE_VERSION} (prior schema — triggers SCHEMA_MISMATCH pre-launch)"
    echo "   Found:    v${STAMPED_VERSION}"
    echo ""
    if [ "$STAMPED_VERSION" = "$CURRENT_VERSION" ]; then
        echo "The fixture is stamped at the CURRENT version (v${CURRENT_VERSION})."
        echo "This means the app would see 'Schema verified' and skip migration entirely —"
        echo "the upgrade path is not exercised at all."
        echo ""
        echo "Fix: regenerate the fixture stamped at v${EXPECTED_FIXTURE_VERSION}:"
        echo "  sqlite3 tests/fixtures/upgrade-from-v${EXPECTED_FIXTURE_VERSION}.db \\"
        echo "    \"UPDATE ai_settings SET value = '${EXPECTED_FIXTURE_VERSION}' WHERE key = 'schema_version'\""
    else
        echo "The fixture is stale. Regenerate it stamped at v${EXPECTED_FIXTURE_VERSION}."
    fi
    echo ""
    echo "See: docs/dev-hygiene-playbook.md § Schema PR Checklist"
    exit 1
fi

echo "[VERIFY-FIXTURE] ✓ Fixture stamped version matches expected: v${EXPECTED_FIXTURE_VERSION}"

# Verify fixture has seed data
PROJECT_COUNT=$(sqlite3 "$FIXTURE_FILE" "SELECT COUNT(*) FROM projects" 2>/dev/null || echo "0")
NOTE_COUNT=$(sqlite3 "$FIXTURE_FILE" "SELECT COUNT(*) FROM notes" 2>/dev/null || echo "0")

if [ "$PROJECT_COUNT" -eq 0 ] || [ "$NOTE_COUNT" -eq 0 ]; then
    echo "⚠️  Warning: Fixture has no seed data (projects: $PROJECT_COUNT, notes: $NOTE_COUNT)"
    echo "Acceptance criterion #10 requires seed data for migration verification."
fi

echo "[VERIFY-FIXTURE] ✓ Upgrade fixture is current (v${EXPECTED_FIXTURE_VERSION} → v${CURRENT_VERSION})"
exit 0
