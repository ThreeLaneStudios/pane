#!/usr/bin/env bash
# Build tests/fixtures/upgrade-from-v92.db for the v92→v93 migration smoke test
# (#3050 PR 2 — migrate_v93, agent_jobs + chat_messages_local columns).
#
# This fixture was missing before this PR (the v91→v92 migration, #3053 item F,
# shipped in 1.0.65 without its own fixture — a pre-existing gap, not
# introduced here). Filled in now because migrate_v93's own smoke test needs a
# v92-stamped starting point, same template as every other build-vNN-fixture.sh.
#
# Approach: start from the already-verified upgrade-from-v91.db fixture and
# apply EXACTLY the delta migrate_v92() applies in src-tauri/src/migrations.rs
# (DELETE FROM chat_oc_tombstones WHERE reason IN ('stale_echo',
# 'backstop_echo') — a one-time cleanup, no schema change), then re-stamp
# ai_settings.schema_version to '92'.
#
# Run from repo root: ./scripts/build-v92-fixture.sh

set -euo pipefail

SRC="tests/fixtures/upgrade-from-v91.db"
DST="tests/fixtures/upgrade-from-v92.db"

if [ ! -f "$SRC" ]; then
    echo "❌ Source fixture not found: $SRC"
    exit 1
fi

rm -f "$DST"
cp "$SRC" "$DST"
echo "✓ Copied $SRC → $DST"

# ── migrate_v92's delta: DELETE stale_echo/backstop_echo tombstone rows (if
#    the table even exists on this fixture — guarded, same as the real
#    migration function's own table-existence check) — then re-stamp ───────
TABLE_EXISTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_oc_tombstones';")
if [ "$TABLE_EXISTS" = "1" ]; then
    sqlite3 "$DST" "DELETE FROM chat_oc_tombstones WHERE reason IN ('stale_echo', 'backstop_echo');"
    echo "✓ Cleared stale_echo/backstop_echo tombstone rows"
else
    echo "✓ chat_oc_tombstones absent on this fixture — migrate_v92's delta is a no-op here (same guard the real migration function uses)"
fi
sqlite3 "$DST" "UPDATE ai_settings SET value = '92' WHERE key = 'schema_version';"
echo "✓ Re-stamped schema_version = 92"

# ── Verify ───────────────────────────────────────────────────────────────────
STAMPED=$(sqlite3 "$DST" "SELECT value FROM ai_settings WHERE key='schema_version'")
PROJECTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM projects")
NOTES=$(sqlite3 "$DST" "SELECT COUNT(*) FROM notes")

echo ""
echo "Verification:"
echo "  schema_version:  $STAMPED (expect 92)"
echo "  projects:        $PROJECTS (expect >0)"
echo "  notes:           $NOTES (expect >0)"

if [ "$STAMPED" != "92" ] || [ "$PROJECTS" -eq 0 ] || [ "$NOTES" -eq 0 ]; then
    echo "❌ Fixture verification failed"
    exit 1
fi

echo ""
echo "✓ tests/fixtures/upgrade-from-v92.db built and verified"
