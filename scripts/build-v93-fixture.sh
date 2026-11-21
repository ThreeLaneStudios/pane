#!/usr/bin/env bash
# Build tests/fixtures/upgrade-from-v93.db for the v93→v94 migration smoke test
# (#3050 PR 5 — migrate_v94, chat_sessions.crew_agent_ids).
#
# Approach: start from the already-verified upgrade-from-v92.db fixture and
# apply EXACTLY the delta migrate_v93() applies in src-tauri/src/migrations.rs
# (CREATE TABLE agent_jobs + 3 indexes, plus the 4 chat_messages_local ADD
# COLUMNs — job_id/synthetic/continued/attachment_ids, the last a documented
# idempotent no-op since it already exists), then re-stamp
# ai_settings.schema_version to '93'. Same template as every other
# build-vNN-fixture.sh.
#
# Run from repo root: ./scripts/build-v93-fixture.sh

set -euo pipefail

SRC="tests/fixtures/upgrade-from-v92.db"
DST="tests/fixtures/upgrade-from-v93.db"

if [ ! -f "$SRC" ]; then
    echo "❌ Source fixture not found: $SRC"
    exit 1
fi

rm -f "$DST"
cp "$SRC" "$DST"
echo "✓ Copied $SRC → $DST"

# ── migrate_v93's delta: agent_jobs table + indexes (non-CRR, per-device) ──
sqlite3 "$DST" "
CREATE TABLE IF NOT EXISTS agent_jobs (
    id                 TEXT PRIMARY KEY,
    kind               TEXT NOT NULL,
    title              TEXT NOT NULL DEFAULT '',
    brief              TEXT NOT NULL,
    agent_id           TEXT NOT NULL REFERENCES ai_agents(id),
    project_id         TEXT NOT NULL,
    session_id         TEXT NOT NULL REFERENCES chat_sessions(id),
    origin_session_id  TEXT REFERENCES chat_sessions(id),
    origin_message_id  TEXT,
    parent_job_id      TEXT REFERENCES agent_jobs(id),
    root_job_id        TEXT REFERENCES agent_jobs(id),
    chain_depth        INTEGER NOT NULL DEFAULT 0,
    wake_origin        INTEGER NOT NULL DEFAULT 1,
    cron               TEXT,
    run_at             TEXT,
    next_run_at        TEXT,
    status             TEXT NOT NULL DEFAULT 'pending',
    started_at         TEXT,
    finished_at        TEXT,
    last_error         TEXT,
    tool_calls_used    INTEGER NOT NULL DEFAULT 0,
    missed_count       INTEGER NOT NULL DEFAULT 0,
    created_at         TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at         TEXT NOT NULL DEFAULT (datetime('now'))
);
CREATE INDEX IF NOT EXISTS idx_agent_jobs_due ON agent_jobs(status, next_run_at);
CREATE INDEX IF NOT EXISTS idx_agent_jobs_origin ON agent_jobs(origin_message_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_jobs_heartbeat ON agent_jobs(agent_id) WHERE kind = 'heartbeat';
"
echo "✓ agent_jobs table + indexes created"

# ── migrate_v93's delta: 4 chat_messages_local columns (attachment_ids
#    already exists on every real fixture — this ALTER is a documented
#    idempotent no-op for it, matching the real migration's own guard) ─────
for col_decl in "job_id TEXT" "synthetic INTEGER NOT NULL DEFAULT 0" "continued INTEGER NOT NULL DEFAULT 0"; do
    col="${col_decl%% *}"
    exists=$(sqlite3 "$DST" "SELECT COUNT(*) FROM pragma_table_info('chat_messages_local') WHERE name='$col';")
    if [ "$exists" = "0" ]; then
        sqlite3 "$DST" "ALTER TABLE chat_messages_local ADD COLUMN $col_decl;"
        echo "✓ chat_messages_local.$col added"
    else
        echo "✓ chat_messages_local.$col already present (idempotent)"
    fi
done

sqlite3 "$DST" "UPDATE ai_settings SET value = '93' WHERE key = 'schema_version';"
echo "✓ Re-stamped schema_version = 93"

# ── Verify ───────────────────────────────────────────────────────────────────
STAMPED=$(sqlite3 "$DST" "SELECT value FROM ai_settings WHERE key='schema_version'")
PROJECTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM projects")
NOTES=$(sqlite3 "$DST" "SELECT COUNT(*) FROM notes")

echo ""
echo "Verification:"
echo "  schema_version:  $STAMPED (expect 93)"
echo "  projects:        $PROJECTS (expect >0)"
echo "  notes:           $NOTES (expect >0)"

if [ "$STAMPED" != "93" ] || [ "$PROJECTS" -eq 0 ] || [ "$NOTES" -eq 0 ]; then
    echo "❌ Fixture verification failed"
    exit 1
fi

echo ""
echo "✓ tests/fixtures/upgrade-from-v93.db built and verified"
