#!/usr/bin/env bash
# Build tests/fixtures/upgrade-from-v94.db for the v94→v95 migration smoke test
# (#3050 fix PR — migrate_v95, agent_jobs.oc_session_key).
#
# Approach: start from the already-verified upgrade-from-v93.db fixture and
# apply EXACTLY the delta migrate_v94() applies in src-tauri/src/migrations.rs
# (chat_sessions.crew_agent_ids TEXT, nullable), then re-stamp
# ai_settings.schema_version to '94'. Same template as every other
# build-vNN-fixture.sh.
#
# Run from repo root: ./scripts/build-v94-fixture.sh

set -euo pipefail

SRC="tests/fixtures/upgrade-from-v93.db"
DST="tests/fixtures/upgrade-from-v94.db"

if [ ! -f "$SRC" ]; then
    echo "❌ Source fixture not found: $SRC"
    exit 1
fi

rm -f "$DST"
cp "$SRC" "$DST"
echo "✓ Copied $SRC → $DST"

# ── migrate_v94's delta: chat_sessions.crew_agent_ids (nullable, no DDL
#    beyond the single ADD COLUMN; NULL = no crew) ──────────────────────────
col="crew_agent_ids"
exists=$(sqlite3 "$DST" "SELECT COUNT(*) FROM pragma_table_info('chat_sessions') WHERE name='$col';")
if [ "$exists" = "0" ]; then
    sqlite3 "$DST" "ALTER TABLE chat_sessions ADD COLUMN $col TEXT;"
    echo "✓ chat_sessions.$col added"
else
    echo "✓ chat_sessions.$col already present (idempotent)"
fi

sqlite3 "$DST" "UPDATE ai_settings SET value = '94' WHERE key = 'schema_version';"
echo "✓ Re-stamped schema_version = 94"

# ── Verify ───────────────────────────────────────────────────────────────────
STAMPED=$(sqlite3 "$DST" "SELECT value FROM ai_settings WHERE key='schema_version'")
PROJECTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM projects")
NOTES=$(sqlite3 "$DST" "SELECT COUNT(*) FROM notes")

echo ""
echo "Verification:"
echo "  schema_version:  $STAMPED (expect 94)"
echo "  projects:        $PROJECTS (expect >0)"
echo "  notes:           $NOTES (expect >0)"

if [ "$STAMPED" != "94" ] || [ "$PROJECTS" -eq 0 ] || [ "$NOTES" -eq 0 ]; then
    echo "❌ Fixture verification failed"
    exit 1
fi

echo ""
echo "✓ tests/fixtures/upgrade-from-v94.db built and verified"
