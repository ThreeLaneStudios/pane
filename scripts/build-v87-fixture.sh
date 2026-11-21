#!/usr/bin/env bash
# Build tests/fixtures/upgrade-from-v87.db for the v87→v88 migration smoke test
# (Blocker 3, #2777 venice live-test scoping — migrate_v88, {runtime}_configured_at
# upgrader backfill for anthropic/openrouter/venice).
#
# Approach: start from the already-verified upgrade-from-v86.db fixture (which
# has the full canonical table set, CRR enabled, and seed data: 2 projects,
# 1 note, 3 agents, 2 sessions, 2 chat messages) and apply EXACTLY the schema
# delta that `migrate_v87()` in src-tauri/src/migrations.rs applies:
#   1. ALTER TABLE compaction_summaries ADD COLUMN compacted_before_rowid INTEGER
# then re-stamp ai_settings.schema_version to '87'.
#
# This produces the exact POST-migrate_v87 / PRE-migrate_v88 state: a v87-
# stamped fixture is the correct starting point to exercise the v87→v88
# migration (the {runtime}_configured_at upgrader backfill).
#
# migrate_v88 seeds an `openrouter_configured_at` row directly (see below)
# rather than relying on a real macOS Keychain credential — a committed .db
# fixture cannot carry a Keychain entry, and the smoke-launch.sh harness does
# not seed the ai_openrouter_key/ai_anthropic_key/ai_venice_key Keychain slots
# for CI runs. Seeding `openrouter_configured_at` directly means migrate_v88
# finds the marker already present and takes its idempotent "already present —
# skipping" branch (see migrate_v88_tests::test_migrate_v88_does_not_overwrite_existing_marker)
# — a deterministic, CI-safe no-op that still exercises the migration arm
# end-to-end without depending on real OS Keychain state.
#
# Run from repo root: ./scripts/build-v87-fixture.sh

set -euo pipefail

SRC="tests/fixtures/upgrade-from-v86.db"
DST="tests/fixtures/upgrade-from-v87.db"

if [ ! -f "$SRC" ]; then
    echo "❌ Source fixture not found: $SRC"
    exit 1
fi

rm -f "$DST"
cp "$SRC" "$DST"
echo "✓ Copied $SRC → $DST"

# ── Apply migrate_v87's exact schema delta ──────────────────────────────────
sqlite3 "$DST" <<'SQL'
ALTER TABLE compaction_summaries ADD COLUMN compacted_before_rowid INTEGER;

-- Seed data: an OpenRouter connection configured at some point in the past,
-- with no {runtime}_configured_this_install-style follow-up needed — the
-- codebase computes `configured_this_install` at read time purely from the
-- presence of this `_configured_at` marker (ai/commands.rs list_runtime_connections),
-- so seeding this row is sufficient to represent "OpenRouter was configured
-- before this upgrade ran".
INSERT OR REPLACE INTO ai_settings (key, value)
    VALUES ('openrouter_configured_at', '2026-01-15T09:00:00Z');

UPDATE ai_settings SET value = '87' WHERE key = 'schema_version';
SQL
echo "✓ Applied migrate_v87 schema delta (compaction_summaries.compacted_before_rowid)"
echo "✓ Seeded openrouter_configured_at ai_settings row"
echo "✓ Re-stamped schema_version = 87"

# ── Verify ───────────────────────────────────────────────────────────────────
STAMPED=$(sqlite3 "$DST" "SELECT value FROM ai_settings WHERE key='schema_version'")
PROJECTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM projects")
NOTES=$(sqlite3 "$DST" "SELECT COUNT(*) FROM notes")
ROWID_COL_PRESENT=$(sqlite3 "$DST" "SELECT COUNT(*)>0 FROM pragma_table_info('compaction_summaries') WHERE name='compacted_before_rowid'")
OPENROUTER_MARKER=$(sqlite3 "$DST" "SELECT COUNT(*) FROM ai_settings WHERE key='openrouter_configured_at'")

echo ""
echo "Verification:"
echo "  schema_version:                 $STAMPED (expect 87)"
echo "  projects:                       $PROJECTS (expect >0)"
echo "  notes:                          $NOTES (expect >0)"
echo "  compacted_before_rowid present: $ROWID_COL_PRESENT (expect 1)"
echo "  openrouter_configured_at seeded: $OPENROUTER_MARKER (expect 1)"

if [ "$STAMPED" != "87" ] || [ "$PROJECTS" -eq 0 ] || [ "$NOTES" -eq 0 ] || [ "$ROWID_COL_PRESENT" != "1" ] || [ "$OPENROUTER_MARKER" != "1" ]; then
    echo "❌ Fixture verification failed"
    exit 1
fi

echo ""
echo "✓ tests/fixtures/upgrade-from-v87.db built and verified"
