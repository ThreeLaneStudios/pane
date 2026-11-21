#!/usr/bin/env bash
# Build tests/fixtures/upgrade-from-v89.db for the v89→v90 migration smoke test
# (#2339 §1.12 — migrate_v90, sheet_columns/sheet_rows/sheet_cells tables).
#
# Approach: start from the already-verified upgrade-from-v88.db fixture and
# apply EXACTLY the schema delta that `migrate_v89()` in
# src-tauri/src/migrations.rs applies (none — v89 is settings-only, no ALTER
# TABLE, no new tables; F1/F5 backfill logic operates on existing rows), then
# re-stamp ai_settings.schema_version to '89'.
#
# No sheet-shaped seed data is needed: sheet_columns/sheet_rows/sheet_cells
# are net-new tables introduced by migrate_v90 — no pre-v90 rows can exist for
# them. The fixture only needs the standard projects/notes seed data that
# verify-upgrade-fixture.sh checks for, inherited unchanged from
# upgrade-from-v88.db.
#
# This produces the exact POST-migrate_v89 / PRE-migrate_v90 state: a v89-
# stamped fixture is the correct starting point to exercise the v89→v90
# migration.
#
# Run from repo root: ./scripts/build-v89-fixture.sh

set -euo pipefail

SRC="tests/fixtures/upgrade-from-v88.db"
DST="tests/fixtures/upgrade-from-v89.db"

if [ ! -f "$SRC" ]; then
    echo "❌ Source fixture not found: $SRC"
    exit 1
fi

rm -f "$DST"
cp "$SRC" "$DST"
echo "✓ Copied $SRC → $DST"

# ── migrate_v89 has no schema delta relevant to this fixture (settings-only
#    backfill on ai_settings/ai_agents, already exercised by the v88 fixture's
#    seed data via migrate_v88_tests/migrate_v89_tests) — just re-stamp ─────
sqlite3 "$DST" <<'SQL'
UPDATE ai_settings SET value = '89' WHERE key = 'schema_version';
SQL
echo "✓ Re-stamped schema_version = 89"

# ── Verify ───────────────────────────────────────────────────────────────────
STAMPED=$(sqlite3 "$DST" "SELECT value FROM ai_settings WHERE key='schema_version'")
PROJECTS=$(sqlite3 "$DST" "SELECT COUNT(*) FROM projects")
NOTES=$(sqlite3 "$DST" "SELECT COUNT(*) FROM notes")

echo ""
echo "Verification:"
echo "  schema_version:  $STAMPED (expect 89)"
echo "  projects:        $PROJECTS (expect >0)"
echo "  notes:           $NOTES (expect >0)"

if [ "$STAMPED" != "89" ] || [ "$PROJECTS" -eq 0 ] || [ "$NOTES" -eq 0 ]; then
    echo "❌ Fixture verification failed"
    exit 1
fi

echo ""
echo "✓ tests/fixtures/upgrade-from-v89.db built and verified"
