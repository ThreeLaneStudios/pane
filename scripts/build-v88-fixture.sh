#!/usr/bin/env bash
# Build tests/fixtures/upgrade-from-v88.db for the v88→v89 migration smoke test
# (#2455 venice live-test v2 scoping — migrate_v89, ollama_configured_at
# upgrader backfill (F1) + ai_agents.role repair (F5)).
#
# Approach: start from the already-verified upgrade-from-v87.db fixture and
# apply EXACTLY the schema delta that `migrate_v88()` in
# src-tauri/src/migrations.rs applies (none — v88 is settings-only, no ALTER
# TABLE), plus seed the data needed to exercise BOTH halves of migrate_v89:
#   1. An `ollama_endpoint` setting with NO matching `ollama_configured_at`
#      marker — the exact upgrader-grandfather shape F1's backfill targets.
#   2. A corrupted `ai_agents` row: `runtime_ref` set (non-'main') but
#      `role != 'sub'` — the exact 1b-safety-net corruption shape F5's repair
#      targets. Also seeds a control row shaped like OC's own main/local agent
#      (`runtime_type='openclaw', runtime_ref='main', role='local'`) that the
#      repair predicate must NOT touch, so the migration test fixture alone
#      can assert both the positive and negative case.
# then re-stamp ai_settings.schema_version to '88'.
#
# This produces the exact POST-migrate_v88 / PRE-migrate_v89 state: a v88-
# stamped fixture is the correct starting point to exercise the v88→v89
# migration.
#
# Run from repo root: ./scripts/build-v88-fixture.sh

set -euo pipefail

SRC="tests/fixtures/upgrade-from-v87.db"
DST="tests/fixtures/upgrade-from-v88.db"

if [ ! -f "$SRC" ]; then
    echo "❌ Source fixture not found: $SRC"
    exit 1
fi

rm -f "$DST"
cp "$SRC" "$DST"
echo "✓ Copied $SRC → $DST"

# ── migrate_v88 has no schema delta (settings-only backfill) — just seed data
#    and re-stamp ───────────────────────────────────────────────────────────
sqlite3 "$DST" <<'SQL'
-- F1 fixture data: ollama configured before the *_configured_at marker existed
-- (upgrader-grandfather case) — no ollama_configured_at row present yet.
INSERT OR REPLACE INTO ai_settings (key, value)
    VALUES ('ollama_endpoint', 'http://localhost:11434');

-- F5 fixture data: a corrupted subagent row (Stia-shaped damage from the old
-- 1b safety net — runtime_ref set, role force-written to 'main' instead of
-- 'sub'). id is a fixed UUID so the migration test can assert on it directly.
INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_type, runtime_ref, role, is_default, is_active, created_at, updated_at)
    VALUES ('f1a5c000-0000-0000-0000-000000000089', 'stia-corrupted', 'stia-corrupted', '/tmp/stia', 'openclaw', 'coder', 'main', 0, 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');

-- Control row: shaped like OC's own main/local agent — legitimately has
-- runtime_ref='main' and role != 'sub'. The repair predicate MUST NOT touch
-- this row. is_default=0 here since `agent-main` (the fixture's pre-existing
-- OC main row) already holds the is_default=1 seat (idx_ai_agents_default is
-- a partial UNIQUE index) — is_default is irrelevant to what F5's repair
-- predicate checks (runtime_ref/role only), so this control row exercises the
-- same exclusion without a seed-data conflict.
INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_type, runtime_ref, role, is_default, is_active, created_at, updated_at)
    VALUES ('f1a5c000-0000-0000-0000-000000000090', 'oc-main-control', 'oc-main-control', '/tmp/oc-main', 'openclaw', 'main', 'local', 0, 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');

UPDATE ai_settings SET value = '88' WHERE key = 'schema_version';
SQL
echo "✓ Seeded ollama_endpoint (no ollama_configured_at marker) for F1"
echo "✓ Seeded corrupted subagent row + OC-main control row for F5"
echo "✓ Re-stamped schema_version = 88"

# ── Verify ───────────────────────────────────────────────────────────────────
STAMPED=$(sqlite3 "$DST" "SELECT value FROM ai_settings WHERE key='schema_version'")
PROJECTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM projects")
NOTES=$(sqlite3 "$DST" "SELECT COUNT(*) FROM notes")
OLLAMA_ENDPOINT=$(sqlite3 "$DST" "SELECT COUNT(*) FROM ai_settings WHERE key='ollama_endpoint'")
OLLAMA_MARKER=$(sqlite3 "$DST" "SELECT COUNT(*) FROM ai_settings WHERE key='ollama_configured_at'")
CORRUPTED_ROW=$(sqlite3 "$DST" "SELECT role FROM ai_agents WHERE id='f1a5c000-0000-0000-0000-000000000089'")
CONTROL_ROW=$(sqlite3 "$DST" "SELECT role FROM ai_agents WHERE id='f1a5c000-0000-0000-0000-000000000090'")

echo ""
echo "Verification:"
echo "  schema_version:          $STAMPED (expect 88)"
echo "  projects:                $PROJECTS (expect >0)"
echo "  notes:                   $NOTES (expect >0)"
echo "  ollama_endpoint seeded:  $OLLAMA_ENDPOINT (expect 1)"
echo "  ollama_configured_at:    $OLLAMA_MARKER (expect 0 — not yet backfilled)"
echo "  corrupted row role:      $CORRUPTED_ROW (expect main)"
echo "  OC-main control role:    $CONTROL_ROW (expect local)"

if [ "$STAMPED" != "88" ] || [ "$PROJECTS" -eq 0 ] || [ "$NOTES" -eq 0 ] || \
   [ "$OLLAMA_ENDPOINT" != "1" ] || [ "$OLLAMA_MARKER" != "0" ] || \
   [ "$CORRUPTED_ROW" != "main" ] || [ "$CONTROL_ROW" != "local" ]; then
    echo "❌ Fixture verification failed"
    exit 1
fi

echo ""
echo "✓ tests/fixtures/upgrade-from-v88.db built and verified"
