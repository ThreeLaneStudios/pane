#!/usr/bin/env bash
# Build tests/fixtures/upgrade-from-v95.db for the v95→v96 migration smoke test
# (live-test #1b fix PR — migrate_v96, agent_jobs.consecutive_failures +
# agent_jobs.last_failed_at).
#
# Approach: start from the already-verified upgrade-from-v94.db fixture and
# apply EXACTLY the delta migrate_v95() applies in src-tauri/src/migrations.rs
# (agent_jobs.oc_session_key TEXT, nullable), then re-stamp
# ai_settings.schema_version to '95'. Same template as every other
# build-vNN-fixture.sh.
#
# Run from repo root: ./scripts/build-v95-fixture.sh

set -euo pipefail

SRC="tests/fixtures/upgrade-from-v94.db"
DST="tests/fixtures/upgrade-from-v95.db"

if [ ! -f "$SRC" ]; then
    echo "❌ Source fixture not found: $SRC"
    exit 1
fi

rm -f "$DST"
cp "$SRC" "$DST"
echo "✓ Copied $SRC → $DST"

# ── migrate_v95's delta: agent_jobs.oc_session_key (nullable, no DDL beyond
#    the single ADD COLUMN; NULL = never bootstrapped) ──────────────────────
col="oc_session_key"
exists=$(sqlite3 "$DST" "SELECT COUNT(*) FROM pragma_table_info('agent_jobs') WHERE name='$col';")
if [ "$exists" = "0" ]; then
    sqlite3 "$DST" "ALTER TABLE agent_jobs ADD COLUMN $col TEXT;"
    echo "✓ agent_jobs.$col added"
else
    echo "✓ agent_jobs.$col already present (idempotent)"
fi

sqlite3 "$DST" "UPDATE ai_settings SET value = '95' WHERE key = 'schema_version';"
echo "✓ Re-stamped schema_version = 95"

# ── Verify ───────────────────────────────────────────────────────────────────
STAMPED=$(sqlite3 "$DST" "SELECT value FROM ai_settings WHERE key='schema_version'")
PROJECTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM projects")
NOTES=$(sqlite3 "$DST" "SELECT COUNT(*) FROM notes")

echo ""
echo "Verification:"
echo "  schema_version:  $STAMPED (expect 95)"
echo "  projects:        $PROJECTS (expect >0)"
echo "  notes:           $NOTES (expect >0)"

if [ "$STAMPED" != "95" ] || [ "$PROJECTS" -eq 0 ] || [ "$NOTES" -eq 0 ]; then
    echo "❌ Fixture verification failed"
    exit 1
fi

echo ""
echo "✓ tests/fixtures/upgrade-from-v95.db built and verified"
