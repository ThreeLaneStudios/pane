#!/usr/bin/env bash
# Build tests/fixtures/upgrade-from-v96.db for the v96→v97 migration smoke test
# (#3151 PR2/C24 — migrate_v97, agent_jobs.archived_at).
#
# Approach: start from the already-verified upgrade-from-v95.db fixture and
# apply EXACTLY the delta migrate_v96() applies in src-tauri/src/migrations.rs
# (agent_jobs.consecutive_failures INTEGER NOT NULL DEFAULT 0 +
# agent_jobs.last_failed_at TEXT, nullable), then re-stamp
# ai_settings.schema_version to '96'. Same template as every other
# build-vNN-fixture.sh.
#
# Run from repo root: ./scripts/build-v96-fixture.sh

set -euo pipefail

SRC="tests/fixtures/upgrade-from-v95.db"
DST="tests/fixtures/upgrade-from-v96.db"

if [ ! -f "$SRC" ]; then
    echo "❌ Source fixture not found: $SRC"
    exit 1
fi

rm -f "$DST"
cp "$SRC" "$DST"
echo "✓ Copied $SRC → $DST"

# ── migrate_v96's delta: agent_jobs.consecutive_failures (NOT NULL DEFAULT
#    0) + agent_jobs.last_failed_at (nullable) ──────────────────────────────
col1="consecutive_failures"
exists1=$(sqlite3 "$DST" "SELECT COUNT(*) FROM pragma_table_info('agent_jobs') WHERE name='$col1';")
if [ "$exists1" = "0" ]; then
    sqlite3 "$DST" "ALTER TABLE agent_jobs ADD COLUMN $col1 INTEGER NOT NULL DEFAULT 0;"
    echo "✓ agent_jobs.$col1 added"
else
    echo "✓ agent_jobs.$col1 already present (idempotent)"
fi

col2="last_failed_at"
exists2=$(sqlite3 "$DST" "SELECT COUNT(*) FROM pragma_table_info('agent_jobs') WHERE name='$col2';")
if [ "$exists2" = "0" ]; then
    sqlite3 "$DST" "ALTER TABLE agent_jobs ADD COLUMN $col2 TEXT;"
    echo "✓ agent_jobs.$col2 added"
else
    echo "✓ agent_jobs.$col2 already present (idempotent)"
fi

sqlite3 "$DST" "UPDATE ai_settings SET value = '96' WHERE key = 'schema_version';"
echo "✓ Re-stamped schema_version = 96"

# ── Verify ───────────────────────────────────────────────────────────────────
STAMPED=$(sqlite3 "$DST" "SELECT value FROM ai_settings WHERE key='schema_version'")
PROJECTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM projects")
NOTES=$(sqlite3 "$DST" "SELECT COUNT(*) FROM notes")
# #3151 PR2/C24: archived_at must NOT exist yet in a v96 fixture — the
# whole point of this fixture is to exercise migrate_v97 adding it.
ARCHIVED_AT_ABSENT=$(sqlite3 "$DST" "SELECT COUNT(*) FROM pragma_table_info('agent_jobs') WHERE name='archived_at';")

echo ""
echo "Verification:"
echo "  schema_version:  $STAMPED (expect 96)"
echo "  projects:        $PROJECTS (expect >0)"
echo "  notes:           $NOTES (expect >0)"
echo "  archived_at col: $ARCHIVED_AT_ABSENT (expect 0 — v97's migrate_v97 must add it, not find it already there)"

if [ "$STAMPED" != "96" ] || [ "$PROJECTS" -eq 0 ] || [ "$NOTES" -eq 0 ] || [ "$ARCHIVED_AT_ABSENT" != "0" ]; then
    echo "❌ Fixture verification failed"
    exit 1
fi

echo ""
echo "✓ tests/fixtures/upgrade-from-v96.db built and verified"
