#!/usr/bin/env bash
# Build v61 fixture with CRR enabled using cr-sqlite extension
set -euo pipefail

FIXTURE="tests/fixtures/upgrade-from-v61.db"
SCHEMA_SQL="tests/fixtures/build-v61-fixture.sql"
CRSQLITE_EXT="src-tauri/target/debug/resources/crsqlite.dylib"

if [ ! -f "$CRSQLITE_EXT" ]; then
    echo "❌ cr-sqlite extension not found: $CRSQLITE_EXT"
    echo "Build the app first: cd src-tauri && cargo build --bin pane"
    exit 1
fi

if [ ! -f "$SCHEMA_SQL" ]; then
    echo "❌ Schema SQL not found: $SCHEMA_SQL"
    exit 1
fi

# Remove existing fixture
rm -f "$FIXTURE"

echo "[BUILD-V61] Creating v61 database..."
sqlite3 "$FIXTURE" < "$SCHEMA_SQL"

echo "[BUILD-V61] Loading cr-sqlite extension and enabling CRR on 20 tables..."

# Create a combined SQL file with extension load + CRR commands
COMBINED_SQL=$(mktemp)
trap "rm -f '$COMBINED_SQL'" EXIT

cat > "$COMBINED_SQL" <<EOSQL
SELECT crsql_as_crr('projects');
SELECT crsql_as_crr('notes');
SELECT crsql_as_crr('tags');
SELECT crsql_as_crr('note_tags');
SELECT crsql_as_crr('note_links');
SELECT crsql_as_crr('ai_context_markers');
SELECT crsql_as_crr('ai_pinned_messages');
SELECT crsql_as_crr('ai_conversation_summaries');
SELECT crsql_as_crr('ai_memory');
SELECT crsql_as_crr('ai_actions');
SELECT crsql_as_crr('user_profiles');
SELECT crsql_as_crr('user_dashboard_config');
SELECT crsql_as_crr('dashboard_modules');
SELECT crsql_as_crr('module_items');
SELECT crsql_as_crr('daily_entries');
SELECT crsql_as_crr('habit_entries');
SELECT crsql_as_crr('reflection_entries');
SELECT crsql_as_crr('gratitude_entries');
SELECT crsql_as_crr('sync_state');
SELECT crsql_as_crr('note_versions');
EOSQL

# Use -cmd to load extension before executing SQL
sqlite3 "$FIXTURE" -cmd ".load $CRSQLITE_EXT" < "$COMBINED_SQL"

echo "[BUILD-V61] ✓ CRR enabled on 20 tables"

# Verify
VERSION=$(sqlite3 "$FIXTURE" "SELECT value FROM ai_settings WHERE key = 'schema_version'")
PROJECT_COUNT=$(sqlite3 "$FIXTURE" "SELECT COUNT(*) FROM projects")
NOTE_COUNT=$(sqlite3 "$FIXTURE" "SELECT COUNT(*) FROM notes")

echo "[BUILD-V61] ✓ Schema version: $VERSION"
echo "[BUILD-V61] ✓ Seed data: $PROJECT_COUNT project(s), $NOTE_COUNT note(s)"
echo "[BUILD-V61] ✓ Fixture created: $FIXTURE"

# Verify CRR tables exist
CRR_TABLE_COUNT=$(sqlite3 "$FIXTURE" "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE '%__crsql_clock'")
echo "[BUILD-V61] ✓ CRR clock tables: $CRR_TABLE_COUNT"

if [ "$CRR_TABLE_COUNT" -lt 20 ]; then
    echo "⚠️  Warning: Expected 20 CRR clock tables, found $CRR_TABLE_COUNT"
    exit 1
fi

echo "[BUILD-V61] ✓ v61 fixture is ready for testing"
