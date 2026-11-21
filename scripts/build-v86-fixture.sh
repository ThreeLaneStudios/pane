#!/usr/bin/env bash
# Build tests/fixtures/upgrade-from-v86.db for the v86→v87 migration smoke test
# (schema bump #2690, §30.11 — compaction_summaries.compacted_before_rowid).
#
# Approach: start from the already-verified upgrade-from-v85.db fixture (which
# has the full canonical table set, CRR enabled on 24 tables, and seed data:
# 2 projects, 1 note, 3 agents, 2 sessions, 2 chat messages) and apply EXACTLY
# the schema delta that `migrate_v86()` in src-tauri/src/migrations.rs applies:
#   1. CREATE TABLE chat_oc_tombstones (thread_id, oc_message_id, reason,
#      tombstoned_at, PK(thread_id, oc_message_id))
#   2. CREATE INDEX idx_chat_local_thread_role_ts ON
#      chat_messages_local(thread_id, role, created_at DESC)
# then re-stamp ai_settings.schema_version to '86'.
#
# This produces the exact POST-migrate_v86 / PRE-migrate_v87 state: a v86-
# stamped fixture is the correct starting point to exercise the v86→v87
# migration (compaction_summaries.compacted_before_rowid ADD COLUMN), since
# that fixture must NOT already have compacted_before_rowid (verified below).
#
# Run from repo root: ./scripts/build-v86-fixture.sh

set -euo pipefail

SRC="tests/fixtures/upgrade-from-v85.db"
DST="tests/fixtures/upgrade-from-v86.db"

if [ ! -f "$SRC" ]; then
    echo "❌ Source fixture not found: $SRC"
    exit 1
fi

rm -f "$DST"
cp "$SRC" "$DST"
echo "✓ Copied $SRC → $DST"

# ── Apply migrate_v86's exact schema delta ──────────────────────────────────
sqlite3 "$DST" <<'SQL'
CREATE TABLE IF NOT EXISTS chat_oc_tombstones (
    thread_id      TEXT NOT NULL,
    oc_message_id  TEXT NOT NULL,
    reason         TEXT NOT NULL,
    tombstoned_at  TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (thread_id, oc_message_id)
);

CREATE INDEX IF NOT EXISTS idx_chat_local_thread_role_ts
    ON chat_messages_local(thread_id, role, created_at DESC);

UPDATE ai_settings SET value = '86' WHERE key = 'schema_version';
SQL
echo "✓ Applied migrate_v86 schema delta (chat_oc_tombstones + idx_chat_local_thread_role_ts)"
echo "✓ Re-stamped schema_version = 86"

# ── Verify ───────────────────────────────────────────────────────────────────
STAMPED=$(sqlite3 "$DST" "SELECT value FROM ai_settings WHERE key='schema_version'")
PROJECTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM projects")
NOTES=$(sqlite3 "$DST" "SELECT COUNT(*) FROM notes")
TOMBSTONE_TABLE=$(sqlite3 "$DST" "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_oc_tombstones'")
ROWID_COL_ABSENT=$(sqlite3 "$DST" "SELECT COUNT(*)=0 FROM pragma_table_info('compaction_summaries') WHERE name='compacted_before_rowid'")

echo ""
echo "Verification:"
echo "  schema_version:              $STAMPED (expect 86)"
echo "  projects:                    $PROJECTS (expect >0)"
echo "  notes:                       $NOTES (expect >0)"
echo "  chat_oc_tombstones present:  $TOMBSTONE_TABLE (expect 1)"
echo "  compacted_before_rowid absent (pre-v87 shape): $ROWID_COL_ABSENT (expect 1)"

if [ "$STAMPED" != "86" ] || [ "$PROJECTS" -eq 0 ] || [ "$NOTES" -eq 0 ] || [ "$TOMBSTONE_TABLE" != "1" ] || [ "$ROWID_COL_ABSENT" != "1" ]; then
    echo "❌ Fixture verification failed"
    exit 1
fi

echo ""
echo "✓ tests/fixtures/upgrade-from-v86.db built and verified"
