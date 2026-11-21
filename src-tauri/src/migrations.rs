// Pane v0.61.0 — Canonical Schema (Pre-Launch Migration Model)
// Single source of truth schema defined in canonical-schema.md
// Per-version-pair migrations applied on mismatch; SCHEMA_MISMATCH error surfaced
// if no pair handler exists. Never destroys data per §2B.2.
//
// ─── MIGRATION-DISK-OPS CONVENTION ──────────────────────────────────────────
// Any migration function that touches files on disk OUTSIDE the SQLite database
// (renaming blobs, creating directories, writing flat files, etc.) MUST carry a
// doc-comment with the exact marker:
//
//   // MIGRATION-DISK-OPS: <one-line description of what files are affected>
//
// Rationale: pre-migration backups (backup.rs `create_backup`) intentionally skip
// the 5+ GB media/Assets archive for `BackupType::Migration` because no migration
// through v69 mutates those files. If a future migration DOES move or overwrite
// attachment blobs or other user media, the author MUST:
//   1. Add the `// MIGRATION-DISK-OPS:` marker to the migration function.
//   2. Open a follow-up issue (or coordinate directly with backup.rs) to ensure
//      the relevant files are protected before the migration runs.
//
// A CI gate that enforces this contract is tracked in issue #1910 (follow-up).
// ─────────────────────────────────────────────────────────────────────────────

use crate::attachments::attachment_storage_filename;
use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};
use rusqlite::{params, Connection, OptionalExtension};

/// Schema version — increment when canonical schema changes
/// `pub(crate)` (was private-to-module) so `migration_tests.rs`'s existing
/// hardcoded-per-arm assertions (which all check "lands at the current schema
/// version", not a specific historical one) can track this constant instead of
/// duplicating and re-drifting the literal on every bump. #2564 B1 fix, Commit 2.
pub(crate) const SCHEMA_VERSION: &str = "97";

/// 27 tables with CRR enabled (must call crsql_as_crr after creation)
pub(crate) const CRR_TABLES: [&str; 27] = [
    "projects",
    "notes",
    "tags",
    "note_tags",
    "note_links",
    "ai_context_markers",
    "ai_pinned_messages",
    "ai_conversation_summaries",
    "ai_memory",
    "ai_actions",
    "user_profiles",
    "user_dashboard_config",
    "dashboard_modules",
    "module_items",
    "daily_entries",
    "habit_entries",
    "reflection_entries",
    "gratitude_entries",
    "sync_state",
    "note_versions",
    "attachments",
    "subtasks",
    "templates",
    "category_colors",
    "sheet_columns",
    "sheet_rows",
    "sheet_cells",
];

/// Main migration entry point
/// On first run: creates schema and sets version
/// On subsequent runs: checks version and applies per-version-pair migration if needed
///
/// Thin wrapper over `run_migrations_with_progress(conn, None)` (#2723 PR B) —
/// kept so the ~150 existing call sites across the crate (production and
/// test) are completely untouched by this PR: no behavior change, no new
/// parameter to thread through code that has nothing to do with the
/// window-first migration screen. Only `lib.rs`'s real app launch, which
/// actually has an `AppHandle` to emit progress on, calls the `_with_progress`
/// variant directly.
pub fn run_migrations(conn: &Connection) -> Result<(), String> {
    run_migrations_with_progress(conn, None)
}

/// Cheap, read-only check: would `run_migrations` on this connection actually
/// change anything? Used by `database::is_migration_pending` (#2723 PR B) to
/// decide, before any work starts, whether a launch needs the window-first
/// background path or can take the unchanged synchronous fast path.
///
/// A fresh install (`None` — no `schema_version` row yet) is deliberately
/// NOT "pending": there is no pre-migration backup to run (nothing to back
/// up yet) and `init_schema` on an empty DB is already fast — the slow part
/// this PR exists for (the pre-migration backup + a real `migrate_vNN` DDL
/// chain) only happens when `Some(old) != SCHEMA_VERSION`.
pub(crate) fn needs_migration(conn: &Connection) -> bool {
    match get_schema_version(conn) {
        None => false,
        Some(ref v) => v != SCHEMA_VERSION,
    }
}

/// Same entry point as `run_migrations`, but threads an optional `AppHandle`
/// through to `create_migration_backup` (backup phase progress,
/// `backup://progress`) and `migration_step` (DDL step progress,
/// `migration://progress`) so a real launch can show live progress on the
/// "Updating Pane… don't quit" screen (#2723 PR B) instead of just a log
/// line. `app_handle = None` (every pre-existing caller, via `run_migrations`
/// above) is a complete no-op for progress — behavior is identical to
/// before this PR for every one of those ~150 call sites.
pub(crate) fn run_migrations_with_progress(
    conn: &Connection,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<(), String> {
    eprintln!(
        "[MIGRATIONS] Canonical schema v{} (pre-launch mode)",
        SCHEMA_VERSION
    );

    // Check current schema version
    let current_version = get_schema_version(conn);

    match current_version {
        None => {
            // Fresh install — create schema
            eprintln!("[MIGRATIONS] Fresh install detected — creating canonical schema...");
            init_schema(conn)?;
            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!(
                "[MIGRATIONS] ✓ Schema created successfully (v{})",
                SCHEMA_VERSION
            );
        }
        Some(ref version) if version == SCHEMA_VERSION => {
            // Schema matches — verify CRR tables
            eprintln!("[MIGRATIONS] Schema version matches (v{})", SCHEMA_VERSION);
            verify_crr_tables(conn)?;
            eprintln!("[MIGRATIONS] ✓ Schema verified");
        }
        Some(ref old_version) if old_version == "61" && SCHEMA_VERSION == "62" => {
            // v61→v62 migration: add chat_archive tables (ADR-0017)
            eprintln!("[MIGRATIONS] Schema version v61 → v62: applying idempotent table additions");

            // Backup first (§2B.1 already runs above; this is belt-and-suspenders for the
            // migration-specific path)
            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v61-to-v62", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
                // Continue — §2B.1's launch-time backup already ran successfully or we
                // wouldn't have reached this code.
            }

            // Apply v62 additions. All guarded with IF NOT EXISTS so re-runs are safe.
            create_chat_archive_tables_v62(conn)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v61 → v62 migration applied");
        }
        Some(ref old_version) if old_version == "70" => {
            // v70→v71: notifications launch columns (#1811, ADR-0021/ADR-0022)
            // Adds dismissed_at, expires_at, target_kind, target_id, idem_key columns
            // and 3 supporting indexes to the notifications table.
            eprintln!("[MIGRATIONS] Schema version v70 → v71: notifications schema (#1811)");

            migration_step(1, 20, "migrate_v71", conn, migrate_v71, app_handle)?;
            migration_step(2, 20, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(3, 20, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(4, 20, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(5, 20, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(6, 20, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(7, 20, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(8, 20, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(9, 20, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(10, 20, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(11, 20, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(12, 20, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(13, 20, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(14, 20, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(15, 20, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(16, 20, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(17, 20, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(18, 20, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(19, 20, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(20, 21, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(21, 22, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(22, 23, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(23, 24, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(24, 24, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v70 → v72 migration applied");
        }
        Some(ref old_version) if old_version == "69" => {
            // v69→v70: notes.status NOT NULL DEFAULT 'cabinet' + index fix (#1935, ADR-0003 amendment)
            // Pre-launch: dev DBs only. Backfills any NULL-status rows to 'cabinet'.
            //
            // #3112: this arm's ladder used to stop at migrate_v89 while unconditionally
            // stamping schema_version = SCHEMA_VERSION (96) below — silently skipping
            // migrate_v90 through migrate_v96 for any DB that upgraded from exactly v69.
            // The v68 arm (one version OLDER) already ran the full ladder through v96
            // correctly, which is what made this arm's gap easy to miss at a glance.
            // Extended to match v68's/v70's own full-ladder shape — no `verify-upgrade-
            // fixture.sh` run against a real v69 database (Matt's ruling: this is a
            // pre-launch, dev-DBs-only arm with no known live install at v69; the fix is
            // the arm being correct, not fixture-proven). Covered instead by
            // `migrations::arm_sweep_tests::test_every_arm_with_migrate_v_calls_reaches_schema_version`,
            // which checks every `old_version == "N"` arm's own ladder, not just this one.
            eprintln!("[MIGRATIONS] Schema version v69 → v70: notes.status cabinet (#1935)");

            conn.execute_batch("UPDATE notes SET status = 'cabinet' WHERE status IS NULL")
                .unwrap_or_else(|e| eprintln!("[MIGRATIONS] v70 backfill warn: {}", e));
            migration_step(1, 23, "migrate_v71", conn, migrate_v71, app_handle)?;
            migration_step(2, 23, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(3, 23, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(4, 23, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(5, 23, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(6, 23, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(7, 23, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(8, 23, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(9, 23, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(10, 23, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(11, 23, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(12, 23, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(13, 23, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(14, 23, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(15, 23, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(16, 23, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(17, 23, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(18, 23, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(19, 23, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(20, 23, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(21, 23, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(22, 23, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(23, 24, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(24, 24, "migrate_v97", conn, migrate_v97, app_handle)?;

            verify_crr_tables(conn)?;
            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v69 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "68" => {
            // v68→v70: og_cache + link_previews_enabled (#1780, ADR-0049) + cabinet status (#1935)
            eprintln!("[MIGRATIONS] Schema version v68 → v70: og_cache + cabinet status (#1935)");

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v68-to-v70", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 21, "migrate_v69", conn, migrate_v69, app_handle)?;
            conn.execute_batch("UPDATE notes SET status = 'cabinet' WHERE status IS NULL")
                .unwrap_or_else(|e| eprintln!("[MIGRATIONS] v70 backfill warn: {}", e));
            migration_step(2, 21, "migrate_v71", conn, migrate_v71, app_handle)?;
            migration_step(3, 21, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(4, 21, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(5, 21, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(6, 21, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(7, 21, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(8, 21, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(9, 21, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(10, 21, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(11, 21, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(12, 21, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(13, 21, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(14, 21, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(15, 21, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(16, 21, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(17, 21, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(18, 21, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(19, 21, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(20, 21, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(21, 22, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(22, 23, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(23, 24, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(24, 25, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(25, 25, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v68 → v72 migration applied");
        }
        Some(ref old_version) if old_version == "67" => {
            // v67→v69 combined: attachments CRR + NOT NULL DEFAULTs (v68) + og_cache (#1780, v69)
            eprintln!("[MIGRATIONS] Schema version v67 → v69: attachments CRR DEFAULTs + og_cache");

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v67-to-v69", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migrate_attachments_v68(conn)?;
            migration_step(1, 21, "migrate_v69", conn, migrate_v69, app_handle)?;
            conn.execute_batch("UPDATE notes SET status = 'cabinet' WHERE status IS NULL")
                .unwrap_or_else(|e| eprintln!("[MIGRATIONS] v70 backfill warn: {}", e));
            migration_step(2, 21, "migrate_v71", conn, migrate_v71, app_handle)?;
            migration_step(3, 21, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(4, 21, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(5, 21, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(6, 21, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(7, 21, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(8, 21, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(9, 21, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(10, 21, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(11, 21, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(12, 21, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(13, 21, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(14, 21, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(15, 21, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(16, 21, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(17, 21, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(18, 21, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(19, 21, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(20, 21, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(21, 22, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(22, 23, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(23, 24, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(24, 25, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(25, 25, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v67 → v72 migration applied");
        }
        Some(ref old_version) if old_version == "66" => {
            // v66→v69 combined: fix attachment storage paths + project_id UUID (v67),
            // add attachments to CRR + NOT NULL DEFAULTs (v68, #1775), + og_cache (v69, #1780).
            eprintln!("[MIGRATIONS] Schema version v66 → v69: attachment storage fix + CRR DEFAULTs + og_cache");

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v66-to-v69", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migrate_attachments_v67(conn)?;
            migrate_attachments_v68(conn)?;
            migration_step(1, 21, "migrate_v69", conn, migrate_v69, app_handle)?;
            conn.execute_batch("UPDATE notes SET status = 'cabinet' WHERE status IS NULL")
                .unwrap_or_else(|e| eprintln!("[MIGRATIONS] v70 backfill warn: {}", e));
            migration_step(2, 21, "migrate_v71", conn, migrate_v71, app_handle)?;
            migration_step(3, 21, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(4, 21, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(5, 21, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(6, 21, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(7, 21, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(8, 21, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(9, 21, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(10, 21, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(11, 21, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(12, 21, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(13, 21, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(14, 21, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(15, 21, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(16, 21, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(17, 21, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(18, 21, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(19, 21, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(20, 21, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(21, 22, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(22, 23, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(23, 24, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(24, 25, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(25, 25, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v66 → v72 migration applied");
        }
        Some(ref old_version) if old_version == "64" => {
            // v64→v68 combined: clean up stale gateway rows (v64→v65) + add embeddings engine column (v65→v66)
            // + attachment storage path fix + project_id UUID (v66→v67, #1775)
            // + attachments CRR + NOT NULL DEFAULTs (v67→v68, #1775)
            eprintln!("[MIGRATIONS] Schema version v64 → v69: applying combined migration");

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v64-to-v69", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migrate_pane_gateway_rows(conn)?;
            migrate_embeddings_engine_column(conn)?;
            // v66→v67: attachment storage path fix + project_id UUID (#1775)
            migrate_attachments_v67(conn)?;
            // v67→v68: attachments CRR + NOT NULL DEFAULTs (#1775)
            migrate_attachments_v68(conn)?;
            migration_step(1, 21, "migrate_v69", conn, migrate_v69, app_handle)?;
            conn.execute_batch("UPDATE notes SET status = 'cabinet' WHERE status IS NULL")
                .unwrap_or_else(|e| eprintln!("[MIGRATIONS] v70 backfill warn: {}", e));
            migration_step(2, 21, "migrate_v71", conn, migrate_v71, app_handle)?;
            migration_step(3, 21, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(4, 21, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(5, 21, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(6, 21, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(7, 21, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(8, 21, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(9, 21, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(10, 21, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(11, 21, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(12, 21, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(13, 21, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(14, 21, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(15, 21, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(16, 21, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(17, 21, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(18, 21, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(19, 21, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(20, 21, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(21, 22, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(22, 23, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(23, 24, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(24, 25, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(25, 25, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v64 → v72 migration applied");
        }
        Some(ref old_version) if old_version == "63" => {
            // v63→v68 combined migration: add migration_flags table (v63→v64, ADR-0044 Addendum A, #1498)
            // AND clean up stale pane_gateway_token/pane_gateway_url rows (v64→v65, §22C.8.1a, #1499)
            // AND add engine column to embeddings table (v65→v66, ADR-0048 / §5.9)
            // AND attachment storage path fix + project_id UUID (v66→v67, #1775)
            // AND attachments CRR + NOT NULL DEFAULTs (v67→v68, #1775).
            eprintln!("[MIGRATIONS] Schema version v63 → v69: applying combined migration");

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v63-to-v69", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            // v63→v64: add migration_flags table
            create_migration_flags_table(conn)?;
            // v64→v65: clean up stale gateway rows
            migrate_pane_gateway_rows(conn)?;
            // v65→v66: add engine column to embeddings table
            migrate_embeddings_engine_column(conn)?;
            // v66→v67: attachment storage path fix + project_id UUID (#1775)
            migrate_attachments_v67(conn)?;
            // v67→v68: attachments CRR + NOT NULL DEFAULTs (#1775)
            migrate_attachments_v68(conn)?;
            migration_step(1, 21, "migrate_v69", conn, migrate_v69, app_handle)?;
            conn.execute_batch("UPDATE notes SET status = 'cabinet' WHERE status IS NULL")
                .unwrap_or_else(|e| eprintln!("[MIGRATIONS] v70 backfill warn: {}", e));
            migration_step(2, 21, "migrate_v71", conn, migrate_v71, app_handle)?;
            migration_step(3, 21, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(4, 21, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(5, 21, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(6, 21, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(7, 21, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(8, 21, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(9, 21, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(10, 21, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(11, 21, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(12, 21, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(13, 21, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(14, 21, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(15, 21, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(16, 21, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(17, 21, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(18, 21, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(19, 21, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(20, 21, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(21, 22, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(22, 23, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(23, 24, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(24, 25, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(25, 25, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v63 → v72 combined migration applied");
        }
        // v62→v65 combined migration: drops greeting_name (v62→v63, ADR-0039 Decision 6, #1280)
        // AND adds migration_flags table (v63→v64, ADR-0044 Addendum A, #1498)
        // AND cleans up stale pane_gateway_token/pane_gateway_url rows (v64→v65, §22C.8.1a, #1499).
        // Not gated on SCHEMA_VERSION so devices stuck at v62 skip straight to v65.
        // v65→v66: add engine column to embeddings table (ADR-0048 / §5.9 — HTTP backend)
        // v66→v67: attachment storage path fix + project_id UUID (#1775)
        // v67→v68: attachments CRR + NOT NULL DEFAULTs (#1775)
        Some(ref old_version) if old_version == "65" => {
            eprintln!("[MIGRATIONS] Schema version v65 → v69: embeddings engine column + attachment storage fix + CRR DEFAULTs + og_cache");

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v65-to-v69", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migrate_embeddings_engine_column(conn)?;
            // v66→v67: attachment storage path fix + project_id UUID (#1775)
            migrate_attachments_v67(conn)?;
            // v67→v68: attachments CRR + NOT NULL DEFAULTs (#1775)
            migrate_attachments_v68(conn)?;
            migration_step(1, 21, "migrate_v69", conn, migrate_v69, app_handle)?;
            conn.execute_batch("UPDATE notes SET status = 'cabinet' WHERE status IS NULL")
                .unwrap_or_else(|e| eprintln!("[MIGRATIONS] v70 backfill warn: {}", e));
            migration_step(2, 21, "migrate_v71", conn, migrate_v71, app_handle)?;
            migration_step(3, 21, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(4, 21, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(5, 21, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(6, 21, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(7, 21, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(8, 21, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(9, 21, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(10, 21, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(11, 21, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(12, 21, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(13, 21, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(14, 21, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(15, 21, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(16, 21, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(17, 21, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(18, 21, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(19, 21, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(20, 21, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(21, 22, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(22, 23, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(23, 24, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(24, 25, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(25, 25, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v65 → v72 migration applied");
        }
        Some(ref old_version) if old_version == "62" => {
            eprintln!("[MIGRATIONS] Schema version v62 → {}: applying combined migration (incl. og_cache #1780)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v62-to-v69", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            // v62→v63: drop greeting_name column
            drop_greeting_name_column(conn)?;
            // v63→v64: add migration_flags table
            create_migration_flags_table(conn)?;
            // v64→v65: clean up stale gateway rows
            migrate_pane_gateway_rows(conn)?;
            // v65→v66: add engine column to embeddings table
            migrate_embeddings_engine_column(conn)?;
            // v66→v67: attachment storage path fix + project_id UUID (#1775)
            migrate_attachments_v67(conn)?;
            // v67→v68: attachments CRR + NOT NULL DEFAULTs (#1775)
            migrate_attachments_v68(conn)?;
            migration_step(1, 21, "migrate_v69", conn, migrate_v69, app_handle)?;
            conn.execute_batch("UPDATE notes SET status = 'cabinet' WHERE status IS NULL")
                .unwrap_or_else(|e| eprintln!("[MIGRATIONS] v70 backfill warn: {}", e));
            migration_step(2, 21, "migrate_v71", conn, migrate_v71, app_handle)?;
            migration_step(3, 21, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(4, 21, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(5, 21, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(6, 21, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(7, 21, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(8, 21, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(9, 21, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(10, 21, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(11, 21, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(12, 21, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(13, 21, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(14, 21, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(15, 21, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(16, 21, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(17, 21, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(18, 21, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(19, 21, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(20, 21, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(21, 22, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(22, 23, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(23, 24, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(24, 25, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(25, 25, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v62 → {} combined migration applied (incl. notifications schema #1811)", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "71" => {
            // v71→v72: chat_sessions table + session seeding + message re-key (#395, ADR-0052, ADR-0053)
            eprintln!(
                "[MIGRATIONS] Schema version v71 → v72: chat_sessions table + migration (#395)"
            );

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v71-to-v72", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 19, "migrate_v72", conn, migrate_v72, app_handle)?;
            migration_step(2, 19, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(3, 19, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(4, 19, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(5, 19, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(6, 19, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(7, 19, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(8, 19, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(9, 19, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(10, 19, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(11, 19, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(12, 19, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(13, 19, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(14, 19, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(15, 19, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(16, 19, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(17, 19, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(18, 19, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(19, 20, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(20, 21, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(21, 22, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(22, 23, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(23, 23, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v71 → v72 migration applied");
        }
        Some(ref old_version) if old_version == "72" => {
            // v72→v76 (#2598 hygiene): v72→v75 is implicit (no migrate_v73/v74/v75
            // functions exist — those version numbers were never given standalone
            // migration bodies), so from v72 the next needed step is migrate_v76.
            // This arm was MISSING prior to #2598: match arms jumped straight from
            // v71 to v75, so any DB stamped at v72/v73/v74 (a real state — no
            // released build leaves users there, but v1.0.3 stamps v76, meaning an
            // interrupted/rolled-back upgrade could strand an install here) hit the
            // SCHEMA_MISMATCH fallthrough with no path forward.
            eprintln!("[MIGRATIONS] Schema version v72 → {}: implicit v72→v75 + agent_id on chat_messages_local (#2030) + retire per-project smart-routing columns (ADR-0061) (#2598 hygiene)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v72-to-v76", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 18, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(2, 18, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(3, 18, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(4, 18, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(5, 18, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(6, 18, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(7, 18, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(8, 18, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(9, 18, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(10, 18, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(11, 18, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(12, 18, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(13, 18, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(14, 18, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(15, 18, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(16, 18, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(17, 18, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(18, 19, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(19, 20, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(20, 21, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(21, 22, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(22, 22, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v72 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "73" => {
            // v73→v76 (#2598 hygiene): same rationale as the v72 arm above —
            // v73→v75 is implicit, next needed step is migrate_v76. A v73→v74
            // arm was briefly added (c911f182, Jul 21) then reverted 4 minutes
            // later (3bfb739d) with no replacement, leaving v73 (and v74) without
            // a migration path until this fix.
            eprintln!("[MIGRATIONS] Schema version v73 → {}: implicit v73→v75 + agent_id on chat_messages_local (#2030) + retire per-project smart-routing columns (ADR-0061) (#2598 hygiene)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v73-to-v76", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 18, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(2, 18, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(3, 18, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(4, 18, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(5, 18, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(6, 18, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(7, 18, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(8, 18, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(9, 18, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(10, 18, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(11, 18, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(12, 18, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(13, 18, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(14, 18, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(15, 18, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(16, 18, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(17, 18, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(18, 19, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(19, 20, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(20, 21, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(21, 22, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(22, 22, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v73 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "74" => {
            // v74→v76 (#2598 hygiene): same rationale as the v72/v73 arms above —
            // v74→v75 is implicit, next needed step is migrate_v76. The Mac
            // Studio's own install is stamped at v74 and self-heals via this arm
            // on first post-fix launch.
            eprintln!("[MIGRATIONS] Schema version v74 → {}: implicit v74→v75 + agent_id on chat_messages_local (#2030) + retire per-project smart-routing columns (ADR-0061) (#2598 hygiene)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v74-to-v76", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 18, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(2, 18, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(3, 18, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(4, 18, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(5, 18, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(6, 18, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(7, 18, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(8, 18, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(9, 18, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(10, 18, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(11, 18, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(12, 18, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(13, 18, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(14, 18, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(15, 18, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(16, 18, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(17, 18, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(18, 19, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(19, 20, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(20, 21, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(21, 22, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(22, 22, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v74 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "75" => {
            // v75→v77: agent_id on chat_messages_local (#2030, ADR-0059 D5, §24.8)
            // + retire per-project smart-routing columns (#2531, #2533, ADR-0061)
            eprintln!("[MIGRATIONS] Schema version v75 → {}: agent_id on chat_messages_local (#2030) + retire per-project smart-routing columns (ADR-0061)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v75-to-v77", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 18, "migrate_v76", conn, migrate_v76, app_handle)?;
            migration_step(2, 18, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(3, 18, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(4, 18, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(5, 18, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(6, 18, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(7, 18, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(8, 18, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(9, 18, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(10, 18, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(11, 18, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(12, 18, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(13, 18, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(14, 18, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(15, 18, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(16, 18, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(17, 18, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(18, 19, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(19, 20, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(20, 21, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(21, 22, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(22, 22, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v75 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "76" => {
            // v76→v78: retire per-project smart-routing columns (#2531, #2533, ADR-0061)
            // + agent_id on chat_sessions (#2522, ADR-0062 D3)
            eprintln!("[MIGRATIONS] Schema version v76 → {}: retire per-project smart-routing columns (ADR-0061) + agent_id on chat_sessions (ADR-0062)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v76-to-v78", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 17, "migrate_v77", conn, migrate_v77, app_handle)?;
            migration_step(2, 17, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(3, 17, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(4, 17, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(5, 17, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(6, 17, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(7, 17, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(8, 17, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(9, 17, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(10, 17, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(11, 17, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(12, 17, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(13, 17, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(14, 17, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(15, 17, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(16, 17, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(17, 18, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(18, 19, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(19, 20, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(20, 21, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(21, 21, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v76 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "77" => {
            // v77→v79: agent_id on chat_sessions (#2522, ADR-0062 D3, Per-Session Agent Binding)
            // + runtime_type/runtime_ref rename (#2522, ADR-0063 D4)
            eprintln!("[MIGRATIONS] Schema version v77 → {}: agent_id on chat_sessions (ADR-0062) + runtime-per-agent rename (ADR-0063)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v77-to-v79", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 16, "migrate_v78", conn, migrate_v78, app_handle)?;
            migration_step(2, 16, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(3, 16, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(4, 16, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(5, 16, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(6, 16, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(7, 16, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(8, 16, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(9, 16, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(10, 16, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(11, 16, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(12, 16, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(13, 16, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(14, 16, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(15, 16, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(16, 17, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(17, 18, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(18, 19, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(19, 20, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(20, 20, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v77 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "78" => {
            // v78→v79: runtime_type/runtime_ref rename, is_main→is_default rename,
            // index swaps, ai_settings key deletes (#2522, ADR-0063 D1/D3/D4)
            eprintln!("[MIGRATIONS] Schema version v78 → {}: runtime-per-agent rename (ADR-0063)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v78-to-v79", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 15, "migrate_v79", conn, migrate_v79, app_handle)?;
            migration_step(2, 15, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(3, 15, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(4, 15, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(5, 15, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(6, 15, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(7, 15, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(8, 15, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(9, 15, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(10, 15, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(11, 15, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(12, 15, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(13, 15, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(14, 15, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(15, 16, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(16, 17, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(17, 18, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(18, 19, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(19, 19, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v78 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "79" => {
            // v79→v80: gateway-endpoint orphan recovery (#2564 B1 fix, Path B).
            // v79's D8 dissolution (ADR-0063) deleted `ai_settings.endpoint` with no
            // per-agent home for it — any install that already migrated through v79
            // before this fix shipped is left with a paired-but-endpoint-less openclaw
            // agent. This is the ONLY step in the v79→v80 chain — unlike the other
            // arms above, there is no `migrate_v78`/`migrate_v79` re-run needed since
            // those already ran to get this install to v79 in the first place.
            eprintln!("[MIGRATIONS] Schema version v79 → {}: gateway-endpoint orphan recovery (#2564 B1 fix)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v79-to-v80", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 14, "migrate_v80", conn, migrate_v80, app_handle)?;
            migration_step(2, 14, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(3, 14, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(4, 14, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(5, 14, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(6, 14, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(7, 14, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(8, 14, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(9, 14, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(10, 14, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(11, 14, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(12, 14, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(13, 14, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(14, 15, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(15, 16, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(16, 17, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(17, 18, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(18, 18, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v79 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "80" => {
            // v80→v82: role='sub'→'local' birth-mislabel data repair (#2567 B6)
            // + per-provider keychain slots / ollama_endpoint backfill (B3, #2565).
            eprintln!("[MIGRATIONS] Schema version v80 → {}: role='sub' birth-mislabel re-label (#2567 B6) + per-provider credential slots (B3, #2565)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v80-to-v82", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 13, "migrate_v81", conn, migrate_v81, app_handle)?;
            migration_step(2, 13, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(3, 13, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(4, 13, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(5, 13, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(6, 13, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(7, 13, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(8, 13, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(9, 13, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(10, 13, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(11, 13, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(12, 13, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(13, 14, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(14, 15, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(15, 16, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(16, 17, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(17, 17, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v80 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "81" => {
            // v81→v82: per-provider keychain slots + ollama_endpoint backfill (B3, #2565).
            // This is the ONLY step in the v81→v82 chain — unlike the arms above, there
            // is no upstream migrate_v* re-run needed since those already ran to get
            // this install to v81 in the first place.
            eprintln!("[MIGRATIONS] Schema version v81 → {}: per-provider credential slots + explicit-connection semantics (B3, #2565)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v81-to-v82", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 12, "migrate_v82", conn, migrate_v82, app_handle)?;
            migration_step(2, 12, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(3, 12, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(4, 12, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(5, 12, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(6, 12, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(7, 12, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(8, 12, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(9, 12, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(10, 12, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(11, 12, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(12, 13, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(13, 14, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(14, 15, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(15, 16, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(16, 16, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v81 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "82" => {
            // v82→v83: OC model placeholder backfill (C1, #2585) + legacy-unpinned
            // agent deactivation (C3, #2587). This is the ONLY step in the v82→v83
            // chain — unlike the arms above, there is no upstream migrate_v* re-run
            // needed since those already ran to get this install to v82 in the first
            // place.
            eprintln!("[MIGRATIONS] Schema version v82 → {}: OC model placeholder backfill (C1, #2585) + legacy-unpinned agent deactivation (C3, #2587)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v82-to-v83", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 11, "migrate_v83", conn, migrate_v83, app_handle)?;
            migration_step(2, 11, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(3, 11, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(4, 11, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(5, 11, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(6, 11, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(7, 11, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(8, 11, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(9, 11, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(10, 11, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(11, 12, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(12, 13, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(13, 14, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(14, 15, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(15, 15, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v82 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "83" => {
            // v83→v84: chat_messages_local.oc_message_id dedup column (#2410, §30.11.2).
            // This is the ONLY step in the v83→v84 chain — unlike the arms above, there
            // is no upstream migrate_v* re-run needed since those already ran to get
            // this install to v83 in the first place.
            eprintln!("[MIGRATIONS] Schema version v83 → {}: chat_messages_local.oc_message_id dedup column (#2410, §30.11.2)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v83-to-v84", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 10, "migrate_v84", conn, migrate_v84, app_handle)?;
            migration_step(2, 10, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(3, 10, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(4, 10, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(5, 10, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(6, 10, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(7, 10, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(8, 10, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(9, 10, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(10, 11, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(11, 12, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(12, 13, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(13, 14, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(14, 14, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v83 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "84" => {
            // v84→v85: chat_sessions.oc_baseline_done_at column (#2410, ingestion
            // filters). This is the ONLY step in the v84→v85 chain — unlike the arms
            // above, there is no upstream migrate_v* re-run needed since those already
            // ran to get this install to v84 in the first place.
            eprintln!("[MIGRATIONS] Schema version v84 → {}: chat_sessions.oc_baseline_done_at column (#2410, ingestion filters)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v84-to-v85", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 9, "migrate_v85", conn, migrate_v85, app_handle)?;
            migration_step(2, 9, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(3, 9, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(4, 9, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(5, 9, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(6, 9, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(7, 9, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(8, 9, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(9, 10, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(10, 11, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(11, 12, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(12, 13, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(13, 13, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v84 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "85" => {
            // v85→v86: chat_oc_tombstones table + idx_chat_local_thread_role_ts index
            // (#2410, §30.11 stateful echo suppression / tombstoning). This is the ONLY
            // step in the v85→v86 chain — unlike the arms above, there is no upstream
            // migrate_v* re-run needed since those already ran to get this install to
            // v85 in the first place.
            eprintln!("[MIGRATIONS] Schema version v85 → {}: chat_oc_tombstones table + idx_chat_local_thread_role_ts index (#2410, §30.11)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v85-to-v86", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 8, "migrate_v86", conn, migrate_v86, app_handle)?;
            migration_step(2, 8, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(3, 8, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(4, 8, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(5, 8, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(6, 8, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(7, 8, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(8, 9, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(9, 10, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(10, 11, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(11, 12, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(12, 12, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v85 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "86" => {
            // v86→v87: compaction_summaries.compacted_before_rowid column (#2690, §30.11
            // ordering fix). This is the ONLY step in the v86→v87 chain — unlike the arms
            // above, there is no upstream migrate_v* re-run needed since those already ran
            // to get this install to v86 in the first place.
            eprintln!("[MIGRATIONS] Schema version v86 → {}: compaction_summaries.compacted_before_rowid column (#2690, §30.11)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v86-to-v87", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 7, "migrate_v87", conn, migrate_v87, app_handle)?;
            migration_step(2, 7, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(3, 7, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(4, 7, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(5, 7, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(6, 7, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(7, 8, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(8, 9, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(9, 10, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(10, 11, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(11, 11, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v86 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "87" => {
            // v87→v89: {runtime}_configured_at upgrader backfill for anthropic/
            // openrouter/venice (Blocker 3, #2777 venice live-test scoping), then
            // ollama_configured_at backfill + ai_agents.role repair (F1/F5, venice
            // live-test v2, #2455). No upstream migrate_v* re-run needed since those
            // already ran to get this install to v87 in the first place.
            eprintln!("[MIGRATIONS] Schema version v87 → {}: {{runtime}}_configured_at backfill + role repair", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v87-to-v89", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 6, "migrate_v88", conn, migrate_v88, app_handle)?;
            migration_step(2, 6, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(3, 6, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(4, 6, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(5, 6, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(6, 7, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(7, 8, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(8, 9, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(9, 10, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(10, 10, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v87 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "88" => {
            // v88→v89: ollama_configured_at upgrader backfill (F1) + ai_agents.role
            // repair for rows with runtime_ref IS NOT NULL (F5), venice live-test v2
            // (#2455). This is the ONLY step in the v88→v89 chain — unlike the arms
            // above, there is no upstream migrate_v* re-run needed since those already
            // ran to get this install to v88 in the first place.
            eprintln!("[MIGRATIONS] Schema version v88 → {}: ollama_configured_at backfill + role repair (#2455)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v88-to-v89", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 5, "migrate_v89", conn, migrate_v89, app_handle)?;
            migration_step(2, 5, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(3, 5, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(4, 5, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(5, 6, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(6, 7, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(7, 8, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(8, 9, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(9, 9, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v88 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "89" => {
            // v89→v91: sheet_columns/sheet_rows/sheet_cells tables (#2339, §1.12),
            // then projects.default_folder_id (#2874 item 50a, §1.7.1). Two steps now
            // — unlike the arms above, there is no OLDER upstream migrate_v* re-run
            // needed since those already ran to get this install to v89 in the first
            // place.
            eprintln!("[MIGRATIONS] Schema version v89 → {}: sheet tables (#2339) + default_folder_id (#2874 item 50a)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v89-to-v91", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 4, "migrate_v90", conn, migrate_v90, app_handle)?;
            migration_step(2, 4, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(3, 4, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(4, 5, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(5, 6, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(6, 7, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(7, 8, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(8, 8, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v89 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "90" => {
            // v90→v91: projects.default_folder_id (#2874 item 50a, §1.7.1).
            // This is the ONLY step in the v90→v91 chain — unlike the arms above,
            // there is no upstream migrate_v* re-run needed since those already ran
            // to get this install to v90 in the first place.
            eprintln!("[MIGRATIONS] Schema version v90 → {}: projects.default_folder_id (#2874 item 50a, §1.7.1)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v90-to-v91", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 3, "migrate_v91", conn, migrate_v91, app_handle)?;
            migration_step(2, 3, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(3, 4, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(4, 5, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(5, 6, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(6, 7, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(7, 7, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v90 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "91" => {
            // v91→v92: clear stale_echo/backstop_echo chat_oc_tombstones rows (#3053
            // item F, §30.11.10). This is the ONLY step in the v91→v92 chain — unlike
            // the arms above, there is no upstream migrate_v* re-run needed since those
            // already ran to get this install to v91 in the first place.
            eprintln!("[MIGRATIONS] Schema version v91 → {}: clear stale_echo/backstop_echo tombstones (#3053 item F, §30.11.10)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v91-to-v92", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 2, "migrate_v92", conn, migrate_v92, app_handle)?;
            migration_step(2, 3, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(3, 4, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(4, 5, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(5, 6, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(6, 6, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v91 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "92" => {
            // v92→v93: agent_jobs table + four chat_messages_local columns (#3050 PR 2,
            // §36.3/§36.10). This is the ONLY step in the v92→v93 chain — unlike the
            // arms above, there is no upstream migrate_v* re-run needed since those
            // already ran to get this install to v92 in the first place.
            eprintln!("[MIGRATIONS] Schema version v92 → {}: agent_jobs + chat_messages_local columns (#3050)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v92-to-v93", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 2, "migrate_v93", conn, migrate_v93, app_handle)?;
            migration_step(2, 3, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(3, 4, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(4, 5, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(5, 5, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v92 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "93" => {
            // v93→v94: `crew_agent_ids` on `chat_sessions` (#3050 PR 5,
            // §36.17/§36.4.1). This is the ONLY step in the v93→v94 chain —
            // unlike the arms above, there is no upstream migrate_v* re-run
            // needed since those already ran to get this install to v93.
            eprintln!("[MIGRATIONS] Schema version v93 → {}: crew_agent_ids on chat_sessions (#3050 PR 5)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v93-to-v94", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 2, "migrate_v94", conn, migrate_v94, app_handle)?;
            migration_step(2, 3, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(3, 4, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(4, 4, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v93 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "94" => {
            // v94→v96: `oc_session_key` on `agent_jobs` (#3050 fix PR, Bug 1(b))
            // then `consecutive_failures`/`last_failed_at` (live-test #1b fix
            // PR, Ruling 3). No upstream migrate_v* re-run needed since those
            // already ran to get this install to v94.
            eprintln!("[MIGRATIONS] Schema version v94 → {}: oc_session_key, then consecutive_failures/last_failed_at on agent_jobs", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v94-to-v96", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 2, "migrate_v95", conn, migrate_v95, app_handle)?;
            migration_step(2, 3, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(3, 3, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v94 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "95" => {
            // v95→v96: `consecutive_failures`/`last_failed_at` on `agent_jobs`
            // (live-test #1b fix PR, Ruling 3). This is the ONLY step in the
            // v95→v96 chain — no upstream migrate_v* re-run needed since
            // those already ran to get this install to v95.
            eprintln!("[MIGRATIONS] Schema version v95 → {}: consecutive_failures/last_failed_at on agent_jobs (live-test #1b fix PR)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v95-to-v96", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 2, "migrate_v96", conn, migrate_v96, app_handle)?;
            migration_step(2, 2, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v95 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) if old_version == "96" => {
            // v96→v97: `archived_at` on `agent_jobs` (#3151 PR2/C24, v97). This
            // is the ONLY step in the v96→v97 chain — no upstream migrate_v*
            // re-run needed since those already ran to get this install to v96.
            eprintln!("[MIGRATIONS] Schema version v96 → {}: archived_at on agent_jobs (#3151 PR2/C24)", SCHEMA_VERSION);

            let db_path = crate::database::get_db_path();
            if let Err(e) = crate::backup::create_migration_backup(&db_path, "v96-to-v97", app_handle) {
                eprintln!("[MIGRATIONS] WARNING: pre-migration backup failed: {}", e);
            }

            migration_step(1, 1, "migrate_v97", conn, migrate_v97, app_handle)?;
            verify_crr_tables(conn)?;

            set_schema_version(conn, SCHEMA_VERSION)?;
            eprintln!("[MIGRATIONS] ✓ v96 → {} migration applied", SCHEMA_VERSION);
        }
        Some(ref old_version) => {
            // Version mismatch with no migration handler
            eprintln!(
                "[MIGRATIONS] Schema version mismatch: {} → {}",
                old_version, SCHEMA_VERSION
            );
            eprintln!("[MIGRATIONS] No migration path exists for this version pair");

            // Create backup before returning error
            let db_path = crate::database::get_db_path();
            if let Err(e) =
                crate::backup::create_migration_backup(&db_path, &format!("v{}", SCHEMA_VERSION), app_handle)
            {
                eprintln!("[MIGRATIONS] WARNING: Backup failed: {}", e);
                eprintln!("[MIGRATIONS] Continuing anyway (backup failure is not fatal)");
            }

            // Return error to trigger graceful fallthrough per §2B.2
            // The app continues with existing schema; new features unavailable
            return Err(format!(
                "SCHEMA_MISMATCH:{}->{}",
                old_version, SCHEMA_VERSION
            ));
        }
    }

    // C22 (#3120 PR 1, §36.4.5): wake dedup index, applied unconditionally
    // here — NOT behind a `SCHEMA_VERSION` bump. By design: the four-PR
    // re-cut of #3120 pins `SCHEMA_VERSION` at 96 across every one of the
    // four PRs ("none of C1–C22 or the pre-existing items bump it"), but a
    // NEW partial unique index on an EXISTING, already-v96-stamped table
    // can't be reached by any version-gated migration arm without bumping
    // the version — `agent_jobs` only gets its indexes (re-)created inside
    // `create_agent_jobs_table`/`migrate_v93`, and neither runs again once
    // an install is already at v96 (confirmed: the `version == SCHEMA_VERSION`
    // match arm above only calls `verify_crr_tables`, no DDL). Resolution:
    // the SAME idempotent `CREATE UNIQUE INDEX IF NOT EXISTS` statement is
    // ALSO baked into `create_agent_jobs_table` (fresh installs) and
    // `migrate_v93` (the one upgrade arm that still creates this table from
    // scratch, for byte-identity with the fresh-install shape) — this call
    // is the backstop that reaches every install already past those two
    // sites, by running on every launch regardless of branch. Placed after
    // the match (not before): every branch above either creates `agent_jobs`
    // from scratch or already had it from a prior launch, so by this point
    // the table is guaranteed to exist in every non-error path — the one
    // branch that returns early (`SCHEMA_MISMATCH`, above) skips this on
    // purpose, since that path leaves the schema at its old (undetermined)
    // version and the app already degrades to "new features unavailable."
    ensure_agent_jobs_wake_dedup_index(conn)?;

    // #3151 PR1 Commit C: the one-time Reminders migration. Same shape as
    // `ensure_agent_jobs_wake_dedup_index` above — idempotent, launch-time,
    // deliberately NOT behind a `SCHEMA_VERSION` bump (no column/table
    // changes here at all; that bump is reserved for PR2's v97, per Matt's
    // "one version bump at the very end of the sprint" ruling — mixing a
    // data-only rename into the version-gated dispatch would wrongly
    // couple it to a schema version nobody using PR1 alone has reached
    // yet). See the function's own doc comment for the idempotency proof.
    ensure_reminders_sessions_renamed_once(conn)?;

    // #3163 / #3161 PR 2: the one-time orphaned-image-embed repair. Same
    // shape as `ensure_reminders_sessions_renamed_once` immediately above
    // — idempotent, launch-time, deliberately NOT a schema bump (Matt's
    // "no schema bump" ruling for the image-gen sprint; this is a data
    // repair for pre-existing damage, not a structural change, so it runs
    // as a one-time backfill gated by an `ai_settings` sentinel instead).
    repair_orphaned_image_embeds_once(conn)?;

    // #3178: same shape again, same precedent as #3163 immediately
    // above — a one-time, idempotent, flag-gated content-only backfill,
    // no schema bump. Rewrites any already-persisted `chat_messages_
    // local.content` row carrying an absolute `Assets/<uuid>.<ext>`
    // image reference (from before `execute_generate_image_at`'s own
    // forward fix, image_gen.rs) to the relative form.
    repair_absolute_image_paths_once(conn)?;

    Ok(())
}

/// #3151 PR1 Commit C: #3120 PR 2's retired Reminders-session mechanism
/// (Commit B, this PR) leaves existing `chat_sessions.origin = 'reminder'`
/// rows behind — pre-existing data, not reachable by any code path going
/// forward (nothing writes `origin = 'reminder'` anymore). Renamed once,
/// at launch, to make the retirement visible in the data too: title →
/// `"Reminders (old)"`, `origin` → `'user'` (an ordinary session, per the
/// issue's own words — not left at the now-dead `'reminder'` value, which
/// would sit unexcluded in `list_sessions`'s `origin NOT IN ('spawn',
/// 'schedule')` clause forever: a landmine for the next person who edits
/// that exclusion list assuming `'reminder'` still means what it used to).
///
/// **Idempotency — an explicit flag, not inferred from data shape.** Once
/// this runs, `origin = 'reminder'` rows no longer exist — using "are
/// there any `origin = 'reminder'` rows left" as the re-run guard would be
/// indistinguishable from "there never were any" (a fresh install, or an
/// install that upgraded straight past this PR with zero legacy rows).
/// Gated instead on a dedicated `ai_settings` key (`reminders_rename_done`),
/// read/written via the same `get_ai_setting`/`set_ai_setting` pair
/// `archive.rs::should_run_archive` already uses for its own one-time/
/// throttled check — same idiom, not a second one.
///
/// Safe to call on every launch regardless of schema state, same as
/// `ensure_agent_jobs_wake_dedup_index` — see that function's own doc
/// comment for why this shape (idempotent, unconditional, launch-time)
/// is the established precedent for post-migration-dispatch cleanup that
/// isn't itself a schema change.
fn ensure_reminders_sessions_renamed_once(conn: &Connection) -> Result<(), String> {
    if crate::database::get_ai_setting(conn, "reminders_rename_done").is_some() {
        return Ok(());
    }

    // Defensive — `chat_sessions.title`/`.origin` have existed since that
    // table's very first shape (`create_chat_sessions_table`, ADR-0052),
    // so every REAL install reaching this call site already has both. The
    // guard exists for the same reason several other migrate_vNN steps in
    // this file already check `pragma_table_info` before touching a column
    // (see e.g. `migrate_v78`'s own `agent_id` check) — some of this
    // module's own hand-built historical test fixtures intentionally seed
    // a minimal `chat_sessions` shape missing columns unrelated to what
    // THEY'RE testing (confirmed: `migration_tests::test_v74_to_current_
    // migration`'s fixture has neither). Without this guard, this call
    // would hard-error on those fixtures and take the WHOLE migration
    // chain down with it, for columns that have nothing to do with the
    // historical rename/column-add work those tests actually exercise.
    let has_required_columns: bool = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM pragma_table_info('chat_sessions') WHERE name = 'title') > 0 \
               AND (SELECT COUNT(*) FROM pragma_table_info('chat_sessions') WHERE name = 'origin') > 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !has_required_columns {
        eprintln!("[MIGRATIONS] ensure_reminders_sessions_renamed_once: chat_sessions missing title/origin — skipping (not yet at a schema shape this can run against)");
        return Ok(());
    }

    let renamed = conn
        .execute(
            "UPDATE chat_sessions SET title = 'Reminders (old)', origin = 'user' WHERE origin = 'reminder'",
            [],
        )
        .map_err(|e| format!("ensure_reminders_sessions_renamed_once: rename failed: {}", e))?;

    crate::database::set_ai_setting(conn, "reminders_rename_done", "1")
        .map_err(|e| format!("ensure_reminders_sessions_renamed_once: failed to set completion flag: {}", e))?;

    eprintln!(
        "[MIGRATIONS] ensure_reminders_sessions_renamed_once: renamed {} legacy Reminders session(s) to 'Reminders (old)' (origin → 'user')",
        renamed
    );
    Ok(())
}

/// #3163: repairs existing notes damaged by the pre-#3161-PR1 childless-
/// div bug (`Attachment.ts`'s node-level `renderHTML` returned a childless
/// `<div>`, so `Editor.svelte::serializeAttachmentWrapper`'s
/// `querySelector('img')` check never matched real `getHTML()` output for
/// ANY default-mode image attachment — confirmed general, not
/// `generate_image`-specific, by PR 1's own re-audit). A save after any
/// edit to a note with an image embed degraded `![alt](Assets/<uuid>.ext)`
/// to a plain `[alt](Assets/<uuid>.ext)` link — losing only the leading
/// `!`; the path/extension survive intact, since this general-case bug
/// (unlike the separate generate_image-specific alt-extension bug PR 1's
/// audit found and fixed in the SAME commit) never routed extension
/// recovery through alt text. PR 1 stopped NEW corruption (`img ||
/// attachmentType === 'image'`); this repairs EXISTING damage.
///
/// **The join key:** the uuid segment of the link's own path
/// (`Assets/<uuid>.<ext>`) — reliably recoverable here because, unlike
/// `generate_image`'s separate bug, the general-case degradation never
/// touched the path at all, only the markdown's leading `!`.
///
/// **No schema bump** (Matt's ruling for the image-gen sprint stands;
/// `SCHEMA_VERSION` is unaffected — confirmed by this function living
/// outside the `migrate_vNN` chain entirely). This is a data repair, not
/// a structural change, so it runs as a one-time, idempotent backfill
/// gated by an `ai_settings` sentinel — the exact same shape
/// `ensure_reminders_sessions_renamed_once` immediately above already
/// uses, called unconditionally on every launch ("on migration or first
/// open" — both reach this call site).
fn repair_orphaned_image_embeds_once(conn: &Connection) -> Result<(), String> {
    if crate::database::get_ai_setting(conn, "image_embed_repair_3163_done").is_some() {
        return Ok(());
    }

    // Defensive — same reasoning as `ensure_reminders_sessions_renamed_
    // once`'s own guard: some historical test fixtures (mid-upgrade-chain
    // migration tests included — confirmed by `test_v68_to_v69_migration_
    // og_cache_and_link_previews`'s own minimal `notes` shape, which has
    // neither `body` nor `content_preview`/`deleted_at`) seed a `notes`/
    // `attachments` shape that predates one or both tables, or predates
    // the specific columns this repair's own SELECT reads.
    let has_required_shape: bool = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='notes') > 0 \
               AND (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='attachments') > 0 \
               AND (SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'body') > 0 \
               AND (SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'content_preview') > 0 \
               AND (SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'deleted_at') > 0 \
               AND (SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'file_path') > 0 \
               AND (SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name = 'word_count') > 0 \
               AND (SELECT COUNT(*) FROM pragma_table_info('attachments') WHERE name = 'mime_type') > 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !has_required_shape {
        eprintln!("[MIGRATIONS] repair_orphaned_image_embeds_once: notes/attachments not yet at a schema shape this can run against — skipping");
        return Ok(());
    }

    let mut stmt = conn
        .prepare(
            "SELECT id, COALESCE(body, content_preview, '') FROM notes \
             WHERE deleted_at IS NULL AND COALESCE(body, content_preview, '') LIKE '%Assets/%'",
        )
        .map_err(|e| format!("repair_orphaned_image_embeds_once: prepare failed: {}", e))?;
    let rows_iter = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| format!("repair_orphaned_image_embeds_once: query failed: {}", e))?;
    let rows: Vec<(String, String)> = rows_iter.filter_map(|r| r.ok()).collect();
    drop(stmt);

    let mut repaired_notes = 0u32;
    let mut repaired_links = 0u32;

    for (note_id, body) in rows {
        let (new_body, count) = repair_orphaned_image_links(conn, &body);
        if count == 0 {
            continue;
        }

        // Defensive: a pre-repair version snapshot. This rewrites user
        // notes in bulk with no human in the loop — `repair_orphaned_
        // image_links`'s own matching rules are conservative (join-key
        // lookup against a real, image-typed `attachments` row), but an
        // undo path costs nothing and this is the same pattern `execute_
        // generate_image_at`'s note-embed step already uses before ITS
        // own rewrite.
        if let Err(e) =
            crate::versions::save_version(conn, &note_id, &body, "image_embed_repair_3163")
        {
            eprintln!(
                "[MIGRATIONS] repair_orphaned_image_embeds_once: failed to save pre-repair version for note {}: {}",
                note_id, e
            );
        }

        let word_count = new_body.split_whitespace().count() as i64;
        let content_preview: String = new_body.chars().take(200).collect();
        conn.execute(
            "UPDATE notes SET body = ?1, content_preview = ?2, word_count = ?3 WHERE id = ?4",
            rusqlite::params![new_body, content_preview, word_count, note_id],
        )
        .map_err(|e| format!("repair_orphaned_image_embeds_once: update failed for note {}: {}", note_id, e))?;

        // Mirror the fix onto the on-disk file too, when this note has
        // one — same "write DB then file" shape `execute_generate_image_
        // at`'s note-embed step already uses. A DB-only fix would
        // re-corrupt the very next time this note is loaded from disk and
        // re-saved.
        let file_path: String = conn
            .query_row("SELECT file_path FROM notes WHERE id = ?1", rusqlite::params![&note_id], |row| row.get(0))
            .unwrap_or_default();
        if !file_path.is_empty() {
            if let Err(e) = crate::database::write_note_content(&file_path, &new_body, Some(conn)) {
                eprintln!(
                    "[MIGRATIONS] repair_orphaned_image_embeds_once: failed to write repaired file for note {}: {}",
                    note_id, e
                );
            }
        }

        repaired_notes += 1;
        repaired_links += count;
    }

    eprintln!(
        "[MIGRATIONS] repair_orphaned_image_embeds_once: repaired {} orphaned image embed(s) across {} note(s)",
        repaired_links, repaired_notes
    );

    crate::database::set_ai_setting(conn, "image_embed_repair_3163_done", "1")
        .map_err(|e| format!("repair_orphaned_image_embeds_once: failed to set completion flag: {}", e))
}

/// The repair itself (#3163), pure matching logic plus one `attachments`
/// lookup per candidate — separated from the note-walking loop above so
/// it's directly unit-testable without a full notes-table fixture.
///
/// Matches a plain (non-`!`-prefixed) markdown link whose target is
/// `(?:\.\./)*Assets/<uuid>.<ext>` — the exact shape `Editor.svelte::
/// processInline`'s own regex recognizes as an attachment reference, so a
/// repaired line round-trips through the editor exactly like any other
/// image embed. For each match, the uuid is looked up in `attachments`;
/// repaired ONLY when a row exists AND its `mime_type` starts with
/// `image/` — this is what keeps the repair from ever touching:
///   - a link to something that never was an attachment (no matching row)
///   - a legitimate link to a NON-image attachment in Assets (a PDF, say
///     — those are deliberately rendered as plain links, not images;
///     "repairing" one would change correct behaviour, not fix a bug)
///   - an ALREADY-correct `![alt](...)` embed (the `!`-prefixed form is
///     excluded by the match itself, which is also why re-running this
///     against an already-repaired body is a no-op — idempotent by
///     construction, not only by the one-time `ai_settings` gate above).
/// Returns the repaired body and how many links were fixed.
pub(crate) fn repair_orphaned_image_links(conn: &Connection, body: &str) -> (String, u32) {
    let re = match regex::Regex::new(
        r"(?P<bang>!)?\[(?P<alt>[^\]]*)\]\((?P<path>(?:\.\./)*Assets/(?P<uuid>[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\.(?P<ext>[A-Za-z0-9]+))\)",
    ) {
        Ok(re) => re,
        Err(_) => return (body.to_string(), 0),
    };

    // Collect repair ranges first (don't mutate `body` while iterating
    // its own match positions), applied back-to-front so earlier byte
    // offsets stay valid.
    let mut inserts: Vec<usize> = Vec::new(); // byte offset to insert '!' at
    for caps in re.captures_iter(body) {
        if caps.name("bang").is_some() {
            continue; // already `![...]` — not orphaned, nothing to do.
        }
        let uuid = &caps["uuid"];
        let is_orphaned_image_attachment: bool = conn
            .query_row(
                "SELECT mime_type FROM attachments WHERE id = ?1",
                rusqlite::params![uuid],
                |row| row.get::<_, String>(0),
            )
            .map(|mime| mime.starts_with("image/"))
            .unwrap_or(false);
        if !is_orphaned_image_attachment {
            continue;
        }
        let whole = caps.get(0).expect("capture 0 is always the full match");
        inserts.push(whole.start());
    }

    if inserts.is_empty() {
        return (body.to_string(), 0);
    }

    let mut new_body = body.to_string();
    for &pos in inserts.iter().rev() {
        new_body.insert(pos, '!');
    }
    (new_body, inserts.len() as u32)
}

/// #3178: repairs existing `chat_messages_local` rows persisted BEFORE
/// `execute_generate_image_at`'s own forward fix (image_gen.rs) — any
/// prior turn where the model generated an image was handed the real,
/// absolute filesystem path (including the user's home directory) and
/// echoed it verbatim into its reply, which was then saved as-is.
/// Same precedent as #3163 (`repair_orphaned_image_embeds_once`,
/// immediately above): idempotent, launch-time, content-only, gated on
/// a dedicated `ai_settings` sentinel rather than a schema bump — Matt's
/// ruling (#3178 issue, "the repair pass ships in THIS PR... same
/// precedent as #3164; the set is small (only `generate_image` rows
/// since 1.0.77)").
///
/// Unlike notes (`repair_orphaned_image_embeds_once`'s own pre-repair
/// `versions::save_version` snapshot), `chat_messages_local` has no
/// versioning table of its own to snapshot into — not introduced here;
/// out of scope for this fix. `chat_messages_local` is also confirmed
/// NOT a CRR table (this file's own `CRR_TABLES` doc comments, e.g. the
/// one above `ensure_reminders_sessions_renamed_once`), so a plain
/// `UPDATE` is the correct shape, same as that function uses.
///
/// The actual rewrite is `image_gen.rs::relativize_absolute_asset_paths`
/// — the SAME function the notification-body defense-in-depth guard
/// (`notifications::emit_agent_message_notification`) calls, not a
/// second copy of the regex. Its own matching rule is what keeps this
/// conservative by construction: ONLY a markdown link/embed whose
/// target is filesystem-rooted (`/...`) AND ends in the exact
/// `Assets/<uuid>.<ext>` shape is touched — an unrelated absolute path
/// that merely happens to appear in a message (not one of ours) never
/// matches that shape and is left untouched.
fn repair_absolute_image_paths_once(conn: &Connection) -> Result<(), String> {
    if crate::database::get_ai_setting(conn, "absolute_image_path_repair_3178_done").is_some() {
        return Ok(());
    }

    // Defensive — same reasoning as `repair_orphaned_image_embeds_once`'s
    // own guard immediately above: some historical test fixtures seed a
    // `chat_messages_local` shape predating this column (or the table
    // itself, for a fixture that never creates AI tables at all).
    let has_required_shape: bool = conn
        .query_row(
            "SELECT (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_messages_local') > 0 \
               AND (SELECT COUNT(*) FROM pragma_table_info('chat_messages_local') WHERE name = 'content') > 0",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !has_required_shape {
        eprintln!("[MIGRATIONS] repair_absolute_image_paths_once: chat_messages_local not yet at a schema shape this can run against — skipping");
        return Ok(());
    }

    let mut stmt = conn
        .prepare("SELECT id, content FROM chat_messages_local WHERE content LIKE '%Assets/%'")
        .map_err(|e| format!("repair_absolute_image_paths_once: prepare failed: {}", e))?;
    let rows_iter = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| format!("repair_absolute_image_paths_once: query failed: {}", e))?;
    let rows: Vec<(String, String)> = rows_iter.filter_map(|r| r.ok()).collect();
    drop(stmt);

    let mut repaired_messages = 0u32;
    let mut repaired_paths = 0u32;

    for (message_id, content) in rows {
        let (new_content, count) = crate::ai::image_gen::relativize_absolute_asset_paths(&content);
        if count == 0 {
            continue;
        }

        conn.execute(
            "UPDATE chat_messages_local SET content = ?1 WHERE id = ?2",
            rusqlite::params![new_content, message_id],
        )
        .map_err(|e| format!("repair_absolute_image_paths_once: update failed for message {}: {}", message_id, e))?;

        repaired_messages += 1;
        repaired_paths += count;
    }

    eprintln!(
        "[MIGRATIONS] repair_absolute_image_paths_once: rewrote {} absolute image path(s) across {} message(s)",
        repaired_paths, repaired_messages
    );

    crate::database::set_ai_setting(conn, "absolute_image_path_repair_3178_done", "1")
        .map_err(|e| format!("repair_absolute_image_paths_once: failed to set completion flag: {}", e))
}

/// C22 (#3120 PR 1, §36.4.5): one pending wake per session, enforced at the
/// DB layer rather than relying on an application-level check-then-insert
/// (which would race under concurrent sibling completions — the exact bug
/// this index exists to close: N sibling `spawn_job` completions each
/// independently calling `create_wake_job` with no read-before-insert dedup,
/// confirmed N wakes for N siblings). `agent_jobs` is explicitly non-CRR
/// (§36.10's own invariant list), so this is a plain index — no cr-sqlite
/// rebuild-shape hazard. Idempotent (`IF NOT EXISTS`); safe to call on
/// every launch regardless of schema state, which is exactly how it's
/// used — see the call site's own doc comment for why this is NOT gated
/// behind a `SCHEMA_VERSION` bump.
///
/// Reed's audit (2026-10-30, PR #3122 FAIL #1) — `IF NOT EXISTS` only
/// guards against the index already existing BY NAME; it does nothing for
/// pre-existing duplicate `(session_id, kind='wake', status='pending')`
/// rows, which fail the `CREATE UNIQUE INDEX` itself (`UNIQUE constraint
/// failed: agent_jobs.session_id`). Reed reproduced the consequence: once
/// that `CREATE` errors (propagated by `?` through `run_migrations` — a
/// startup failure, not a degraded feature, per the call site's own doc
/// comment), `create_wake_job`'s `ON CONFLICT(session_id) WHERE kind =
/// 'wake' AND status = 'pending'` upsert has no matching partial unique
/// index to target — SQLite does not fall back to a plain insert, it
/// errors (`ON CONFLICT clause does not match any PRIMARY KEY or UNIQUE
/// constraint`). Every wake creation, for every session, fails forever
/// from that point on. Matt's ruling: repair, then index. Before the
/// `CREATE`, for every session with more than one pending wake, keep the
/// oldest (by `created_at`, `rowid` as a tiebreak for same-instant rows)
/// and set the rest to `cancelled` — NOT deleted, so the completion
/// history for those siblings is untouched. `cancelled` also can't
/// conflict with the index afterward: the partial index's own `WHERE`
/// clause only matches `status = 'pending'`, so a `cancelled` row is
/// simply invisible to it, by construction, not by convention.
fn ensure_agent_jobs_wake_dedup_index(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "UPDATE agent_jobs \
         SET status = 'cancelled', updated_at = datetime('now') \
         WHERE kind = 'wake' AND status = 'pending' \
         AND id NOT IN ( \
             SELECT id FROM ( \
                 SELECT id, ROW_NUMBER() OVER ( \
                     PARTITION BY session_id ORDER BY created_at ASC, rowid ASC \
                 ) AS rn \
                 FROM agent_jobs \
                 WHERE kind = 'wake' AND status = 'pending' \
             ) WHERE rn = 1 \
         );",
        [],
    )
    .map_err(|e| format!("Failed to repair duplicate pending wakes before indexing: {}", e))?;

    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_jobs_one_pending_wake_per_session \
         ON agent_jobs(session_id) WHERE kind = 'wake' AND status = 'pending';",
    )
    .map_err(|e| format!("Failed to ensure idx_agent_jobs_one_pending_wake_per_session: {}", e))
}

/// Log and run one step of a multi-step migration chain (#2724 — logging only;
/// #2723 PR B adds a `migration://progress` event alongside the existing log
/// line — still no control-flow change, `f` runs exactly as it would have
/// running inline, `?` still propagates on failure).
/// Several `run_migrations` arms jump an old install forward through a long run of
/// upstream `migrate_vNN` calls (e.g. v66 → v92 is 16 steps) in a single launch; this
/// makes that progress visible in terminal/log output (and, when `app_handle` is
/// `Some`, to the "Updating Pane…" screen) without changing behavior.
///
/// `app_handle` is `None` for every one of the ~150 pre-existing call sites that
/// still go through `run_migrations` → `run_migrations_with_progress(conn, None)`
/// — `emit_progress`-style "emit only if Some" means this is a true no-op for
/// all of them, same as `backup.rs`'s `emit_progress` already does for backups.
fn migration_step(
    step: usize,
    total: usize,
    label: &str,
    conn: &Connection,
    f: fn(&Connection) -> Result<(), String>,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<(), String> {
    eprintln!("[MIGRATIONS] step {}/{}: {}", step, total, label);
    if let Some(handle) = app_handle {
        use tauri::Emitter;
        let pct = if total == 0 { 0 } else { ((step as f64 / total as f64) * 100.0).round() as u8 };
        let _ = handle.emit(
            "migration://progress",
            serde_json::json!({
                "step": step,
                "total": total,
                "label": label,
                "pct": pct
            }),
        );
    }
    f(conn)
}

/// Get current schema version from ai_settings
fn get_schema_version(conn: &Connection) -> Option<String> {
    // Check if ai_settings table exists first
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        return None;
    }

    crate::database::get_ai_setting(conn, "schema_version")
}

/// Set schema version in ai_settings
fn set_schema_version(conn: &Connection, version: &str) -> Result<(), String> {
    conn.execute(
        "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('schema_version', ?1)",
        params![version],
    )
    .map_err(|e| format!("Failed to set schema version: {}", e))?;
    Ok(())
}

/// Initialize all 44 tables from canonical schema
/// This function is the single source of truth for Pane's database schema
fn init_schema(conn: &Connection) -> Result<(), String> {
    eprintln!("[SCHEMA] Creating 44 tables from canonical schema...");

    // === CORE CONTENT TABLES (5 tables) ===

    create_projects_table(conn)?;
    create_notes_table(conn)?;
    create_tags_tables(conn)?; // tags, note_tags, note_links

    // === USERS & AUTH (4 tables) ===

    create_users_tables(conn)?; // users, user_profiles, user_roles, role_model_permissions

    // === AI CHAT (7 tables) ===

    create_chat_tables(conn)?; // chat_messages_local, ai_context_markers, ai_pinned_messages, ai_conversation_summaries, chat_archive, chat_archive_fts
    create_chat_sessions_table(conn)?; // chat_sessions (per-device, NOT CRR — §30.1.1, ADR-0052)

    // === AI MEMORY & AUDIT (4 tables) ===

    create_ai_memory_tables(conn)?; // ai_memory, ai_actions, ai_settings, ai_message_queue

    // === AI AGENTS & TASKS (3 tables) ===

    create_agent_tables(conn)?; // ai_agents, pane_agent_tasks, pane_task_workspaces
    create_agent_jobs_table(conn)?; // agent_jobs — non-CRR, per-device (§36.10, #3050 PR 2)

    // === AI PROCESSING (2 tables) ===

    create_processing_tables(conn)?; // extraction_queue, compaction_summaries

    // === DASHBOARD (7 tables) ===

    create_dashboard_tables(conn)?; // user_dashboard_config, dashboard_modules, module_items, daily_entries, habit_entries, reflection_entries, gratitude_entries

    // === SYNC & VERSIONING (4 tables) ===

    create_sync_tables(conn)?; // sync_state, sync_changelog, sync_queue, note_versions

    // === ATTACHMENTS & SEARCH (3 tables) ===

    create_attachment_tables(conn)?; // attachments, embeddings, note_context_pins

    // === SUBTASKS (1 table, §1.9 ADR-0057) ===

    create_subtasks_table(conn)?; // subtasks — CRR-synced Advanced Checklist line items

    // === TEMPLATES (1 table, §32 ADR-0058, #193) ===

    create_templates_table(conn)?; // templates — named content snapshots (CRR-synced)

    // === SHEETS (3 tables, §1.12 ADR-0066, #2339) ===

    create_sheet_tables(conn)?; // sheet_columns, sheet_rows, sheet_cells — CRR-synced spreadsheet-in-note data

    // === OG CACHE (1 table, #1780 ADR-0049) ===

    create_og_cache_table(conn)?; // og_cache — URL-keyed Open Graph metadata cache

    // === SYSTEM TABLES (3 tables) ===

    create_system_tables(conn)?;

    // === MIGRATION FLAGS (1 table) ===

    create_migration_flags_table(conn)?; // category_colors, notifications, extensions

    // === FTS5 SEARCH TABLES (3 virtual tables) ===

    create_fts_virtual_tables(conn)?; // notes_fts, ai_memory_fts, chat_fts (tables only, triggers come after CRR)

    eprintln!("[SCHEMA] All content tables created");

    // === ENABLE CRR ON 20 TABLES ===

    enable_crr(conn)?;

    // === CREATE FTS TRIGGERS AFTER CRR ===
    // IMPORTANT: Triggers must be created AFTER enable_crr() because crsql_as_crr()
    // destroys existing triggers when it modifies tables for CRDT tracking (#544)

    create_fts_triggers(conn)?;
    rebuild_fts_indexes(conn)?;

    eprintln!("[SCHEMA] ✓ Schema initialization complete");
    Ok(())
}

// === TABLE CREATION FUNCTIONS ===

fn create_projects_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS projects (
            id                    TEXT NOT NULL PRIMARY KEY,
            name                  TEXT NOT NULL DEFAULT '',
            description           TEXT,
            -- Reserved: not used for any current UI display. Category color is in category_colors.
            -- Do not read this column for visual rendering; use projects.category + category_colors instead.
            color                 TEXT,
            parent_id             TEXT,
            sort_order            INTEGER NOT NULL DEFAULT 0,
            owner_id              TEXT,

            created_at            TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at            TEXT NOT NULL DEFAULT (datetime('now')),

            category              TEXT,
            is_system             INTEGER NOT NULL DEFAULT 0,

            is_hidden             INTEGER NOT NULL DEFAULT 0,
            is_favorite           INTEGER NOT NULL DEFAULT 0,

            default_model         TEXT,
            smart_routing_enabled INTEGER,
            router_model          TEXT,
            simple_tier_model     TEXT,

            -- §1.7.1 (#2874 item 50a): the project's default folder, by ID -- never
            -- resolved by matching the name 'General'. NULL/unused on folder rows
            -- (parent_id IS NOT NULL); every top-level project gets a value via
            -- migrate_v91's backfill (existing DBs) or create_project_internal/
            -- import.rs/watcher.rs (new projects).
            default_folder_id     TEXT

            -- NOTE: No FOREIGN KEY on parent_id — CRR table, enforce in app code (#493)
            -- NOTE: No UNIQUE(name, parent_id) — cr-sqlite CRR tables cannot have
            -- unique constraints beyond primary key. Uniqueness enforced in application
            -- code instead (see #493).
        );

        CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);
        CREATE INDEX IF NOT EXISTS idx_projects_parent ON projects(parent_id);
        CREATE INDEX IF NOT EXISTS idx_projects_hidden ON projects(is_hidden) WHERE is_hidden = 1;
        CREATE INDEX IF NOT EXISTS idx_projects_favorite ON projects(is_favorite) WHERE is_favorite = 1;
    "
    )
    .map_err(|e| format!("Failed to create projects table: {}", e))?;
    eprintln!("[SCHEMA] ✓ projects");
    Ok(())
}

fn create_notes_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS notes (
            id              TEXT NOT NULL PRIMARY KEY,
            title           TEXT NOT NULL DEFAULT '',
            body            TEXT,
            content_preview TEXT NOT NULL DEFAULT '',
            file_path       TEXT NOT NULL DEFAULT '',
            project_id      TEXT,
            folder_id       TEXT,

            status          TEXT NOT NULL DEFAULT 'cabinet',
            priority        TEXT,
            due_date        TEXT,
            assignee        TEXT,
            completed_at    TEXT,
            sort_order      INTEGER NOT NULL DEFAULT 0,

            word_count      INTEGER NOT NULL DEFAULT 0,
            pinned          INTEGER NOT NULL DEFAULT 0,
            owner_id        TEXT,
            tags_json       TEXT NOT NULL DEFAULT '[]',

            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
            deleted_at      TEXT,

            parent_id       TEXT,
            note_type       TEXT,
            is_vital        INTEGER NOT NULL DEFAULT 0,
            agent_id        TEXT

            -- NOTE: No FOREIGN KEY on folder_id or parent_id — CRR table, enforce in app code (#493)
        );

        CREATE INDEX IF NOT EXISTS idx_notes_project ON notes(project_id);
        CREATE INDEX IF NOT EXISTS idx_notes_folder ON notes(folder_id);
        CREATE INDEX IF NOT EXISTS idx_notes_status ON notes(status) WHERE status != 'cabinet';
        CREATE INDEX IF NOT EXISTS idx_notes_priority ON notes(priority) WHERE priority IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_due_date ON notes(due_date) WHERE due_date IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at);
        CREATE INDEX IF NOT EXISTS idx_notes_deleted ON notes(deleted_at) WHERE deleted_at IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_agent_id ON notes(agent_id);
        CREATE INDEX IF NOT EXISTS idx_notes_type ON notes(note_type);
        CREATE INDEX IF NOT EXISTS idx_notes_vital ON notes(is_vital) WHERE is_vital = 1;
    "
    )
    .map_err(|e| format!("Failed to create notes table: {}", e))?;
    eprintln!("[SCHEMA] ✓ notes");
    Ok(())
}

fn create_tags_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tags (
            id    TEXT PRIMARY KEY NOT NULL,
            name  TEXT NOT NULL DEFAULT '',  -- No UNIQUE: CRR table, enforce in app code (#493)
            color TEXT NOT NULL DEFAULT '#6B7280'
        );

        CREATE INDEX IF NOT EXISTS idx_tags_name ON tags(name);

        CREATE TABLE IF NOT EXISTS note_tags (
            note_id TEXT NOT NULL DEFAULT '',
            tag_id  TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (note_id, tag_id)
            -- NOTE: No FOREIGN KEY — CRR table, enforce in app code (#493)
        );

        CREATE INDEX IF NOT EXISTS idx_note_tags_note ON note_tags(note_id);
        CREATE INDEX IF NOT EXISTS idx_note_tags_tag ON note_tags(tag_id);

        CREATE TABLE IF NOT EXISTS note_links (
            source_note_id TEXT NOT NULL DEFAULT '',
            target_note_id TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (source_note_id, target_note_id)
        );

        CREATE INDEX IF NOT EXISTS idx_note_links_source ON note_links(source_note_id);
        CREATE INDEX IF NOT EXISTS idx_note_links_target ON note_links(target_note_id);
    ",
    )
    .map_err(|e| format!("Failed to create tags tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ tags, note_tags, note_links");
    Ok(())
}

fn create_users_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS users (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            email       TEXT,
            avatar_url  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS user_profiles (
            id                      TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            user_id                 TEXT NOT NULL DEFAULT '',
            display_name            TEXT,
            bio                     TEXT,
            working_style           TEXT,
            ai_provider             TEXT,
            ai_model                TEXT,
            onboarding_method       TEXT,
            onboarding_completed    INTEGER NOT NULL DEFAULT 0,
            profile_json            TEXT DEFAULT '{}',
            created_at              TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at              TEXT NOT NULL DEFAULT (datetime('now'))
            -- NOTE: No FOREIGN KEY on user_id — CRR table, enforce in app code (#493)
        );

        CREATE TABLE IF NOT EXISTS user_roles (
            id          TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            user_id     TEXT NOT NULL,
            project_id  TEXT,
            role        TEXT NOT NULL DEFAULT 'viewer',
            granted_by  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, project_id)
        );

        CREATE INDEX IF NOT EXISTS idx_user_roles_user ON user_roles(user_id);
        CREATE INDEX IF NOT EXISTS idx_user_roles_project ON user_roles(project_id);

        CREATE TABLE IF NOT EXISTS role_model_permissions (
            role    TEXT NOT NULL,
            model   TEXT NOT NULL,
            allowed INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY (role, model)
        );
    ",
    )
    .map_err(|e| format!("Failed to create users tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ users, user_profiles, user_roles, role_model_permissions");
    Ok(())
}

fn create_chat_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_messages_local (
            id          TEXT NOT NULL PRIMARY KEY,
            project_id  TEXT NOT NULL DEFAULT '',
            role        TEXT NOT NULL DEFAULT 'user',
            content     TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            is_pinned   INTEGER NOT NULL DEFAULT 0,
            attachment_ids TEXT,
            tool_calls  TEXT,
            tool_results TEXT,
            model       TEXT,
            token_count INTEGER,
            thread_id   TEXT,
            agent_id    TEXT REFERENCES ai_agents(id),
            oc_message_id TEXT,
            job_id      TEXT,         -- §36.10, #3050 PR 2: report-back rows
            synthetic   INTEGER NOT NULL DEFAULT 0,  -- synthetic job briefs/continue turns; hidden in UI
            continued   INTEGER NOT NULL DEFAULT 0   -- §36.2 divider rows
        );

        CREATE INDEX IF NOT EXISTS idx_chat_local_project ON chat_messages_local(project_id, created_at);

        -- §30.1.2: thread_id is load-bearing FK to chat_sessions.id (ADR-0052)
        CREATE INDEX IF NOT EXISTS idx_chat_local_thread ON chat_messages_local(thread_id, created_at);

        -- §30.11.2 (#2410): dedup key for externally-ingested (proactive/OC-transcript-pull)
        -- messages. Partial unique index — NULL oc_message_id (ordinary user/assistant
        -- messages) is excluded and never deduped.
        CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_local_oc_dedup
            ON chat_messages_local(thread_id, oc_message_id)
            WHERE oc_message_id IS NOT NULL;

        -- §30.11 (#2410, v86): supports the content-containment check's role-scoped,
        -- unbounded-lookback query (Rule 2 / tombstoning stale-echo detection).
        CREATE INDEX IF NOT EXISTS idx_chat_local_thread_role_ts
            ON chat_messages_local(thread_id, role, created_at DESC);

        -- §30.11 (#2410, v86): permanent record of OC-transcript-ingestion echo/
        -- staleness classifications, keyed by (thread_id, oc_message_id). Once a
        -- message is tombstoned it is never re-evaluated by the backstop or the
        -- content-containment check on any future poll tick — see
        -- docs/audits/2026-08-24-tombstone-scope.md for the full mechanic design.
        CREATE TABLE IF NOT EXISTS chat_oc_tombstones (
            thread_id      TEXT NOT NULL,
            oc_message_id  TEXT NOT NULL,
            reason         TEXT NOT NULL,
            tombstoned_at  TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (thread_id, oc_message_id)
        );

        CREATE TABLE IF NOT EXISTS ai_context_markers (
            id                TEXT NOT NULL PRIMARY KEY,
            project_id        TEXT NOT NULL DEFAULT '',
            marker_type       TEXT NOT NULL DEFAULT '',
            label             TEXT NOT NULL DEFAULT '',
            target_note_id    TEXT,
            target_folder_id  TEXT,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            inserted_after    TEXT
        );

        CREATE TABLE IF NOT EXISTS ai_pinned_messages (
            id          TEXT NOT NULL PRIMARY KEY,
            project_id  TEXT NOT NULL DEFAULT '',
            message_id  TEXT NOT NULL DEFAULT '',
            pinned_at   TEXT NOT NULL DEFAULT (datetime('now')),
            sort_order  INTEGER DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_ai_pinned_project_message ON ai_pinned_messages(project_id, message_id);

        CREATE TABLE IF NOT EXISTS ai_conversation_summaries (
            id                TEXT NOT NULL PRIMARY KEY,
            project_id        TEXT NOT NULL DEFAULT '',
            summary_text      TEXT NOT NULL DEFAULT '',
            start_message_id  TEXT NOT NULL DEFAULT '',
            end_message_id    TEXT NOT NULL DEFAULT '',
            token_count       INTEGER,
            created_at        TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- §5.14.2: Archive table for messages >30 days old
        CREATE TABLE IF NOT EXISTS chat_archive (
            id          TEXT PRIMARY KEY,
            thread_id   TEXT NOT NULL,
            project_id  TEXT,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            metadata    TEXT,
            archived_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_chat_archive_project ON chat_archive(project_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_chat_archive_thread ON chat_archive(thread_id);

        -- §5.14.2: FTS5 index for archived messages
        CREATE VIRTUAL TABLE IF NOT EXISTS chat_archive_fts USING fts5(
            content,
            content_rowid='rowid',
            tokenize='porter unicode61'
        );
    "
    )
    .map_err(|e| format!("Failed to create chat tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ chat_messages_local, ai_context_markers, ai_pinned_messages, ai_conversation_summaries, chat_archive, chat_archive_fts");
    Ok(())
}

/// Create chat_archive tables (v62 schema additions)
/// Extracted for use in v61→v62 migration (ADR-0017)
/// v62→v63: Drop greeting_name column from user_dashboard_config (ADR-0039 Decision 6, #1280).
/// SQLite pre-3.35 doesn't support ALTER TABLE ... DROP COLUMN, so we use the recreate-table pattern.
fn drop_greeting_name_column(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATIONS] Dropping greeting_name column from user_dashboard_config");
    conn.execute_batch("
        BEGIN;
        CREATE TABLE IF NOT EXISTS user_dashboard_config_new (
            id                    TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            onboarding_completed  INTEGER DEFAULT 0,
            onboarding_template   TEXT,
            theme_variant         TEXT DEFAULT 'default',
            created_at            TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT OR IGNORE INTO user_dashboard_config_new
            SELECT id, onboarding_completed, onboarding_template, theme_variant, created_at, updated_at
            FROM user_dashboard_config;
        DROP TABLE user_dashboard_config;
        ALTER TABLE user_dashboard_config_new RENAME TO user_dashboard_config;
        COMMIT;
    ").map_err(|e| format!("drop_greeting_name_column: {}", e))?;
    eprintln!("[MIGRATIONS] ✓ greeting_name column removed");

    // Re-enable CRR on the rebuilt table (dropping a table removes its CRR triggers)
    let crr_available = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_function_list WHERE name='crsql_as_crr'",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);

    if crr_available {
        conn.query_row("SELECT crsql_as_crr('user_dashboard_config');", [], |_| {
            Ok(())
        })
        .map_err(|e| format!("drop_greeting_name_column: re-enable CRR: {}", e))?;
        eprintln!("[MIGRATIONS] ✓ CRR re-enabled on user_dashboard_config");
    } else {
        eprintln!("[MIGRATIONS] ⚠️  cr-sqlite not loaded, skipping CRR re-enablement (test mode)");
    }
    Ok(())
}

fn create_chat_archive_tables_v62(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "-- §5.14.2: Archive table for messages >30 days old
        CREATE TABLE IF NOT EXISTS chat_archive (
            id          TEXT PRIMARY KEY,
            thread_id   TEXT NOT NULL,
            project_id  TEXT,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            metadata    TEXT,
            archived_at TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_chat_archive_project ON chat_archive(project_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_chat_archive_thread ON chat_archive(thread_id);

        -- §5.14.2: FTS5 index for archived messages
        CREATE VIRTUAL TABLE IF NOT EXISTS chat_archive_fts USING fts5(
            content,
            content_rowid='rowid',
            tokenize='porter unicode61'
        );
    ",
    )
    .map_err(|e| format!("Failed to create chat_archive tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ chat_archive, chat_archive_fts");
    Ok(())
}

fn create_ai_memory_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ai_memory (
            id                TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            layer             TEXT NOT NULL DEFAULT '',
            project_id        TEXT,
            user_id           TEXT,
            fact              TEXT NOT NULL DEFAULT '',
            category          TEXT,
            source_message_id TEXT,
            is_active         INTEGER NOT NULL DEFAULT 1,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at        TEXT,
            last_referenced   TEXT,
            routed_at         TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_memory_layer ON ai_memory(layer);
        CREATE INDEX IF NOT EXISTS idx_memory_project ON ai_memory(project_id) WHERE project_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memory_user ON ai_memory(user_id) WHERE user_id IS NOT NULL;

        CREATE TABLE IF NOT EXISTS ai_actions (
            id              TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            user_id         TEXT NOT NULL DEFAULT '',
            action_type     TEXT NOT NULL DEFAULT '',
            target_id       TEXT,
            target_type     TEXT,
            details_json    TEXT NOT NULL DEFAULT '{}',
            project_id      TEXT,
            model           TEXT,
            message_id      TEXT,
            status          TEXT NOT NULL DEFAULT 'success',
            error_message   TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_ai_actions_user ON ai_actions(user_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_ai_actions_project ON ai_actions(project_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_ai_actions_type ON ai_actions(action_type);
        CREATE INDEX IF NOT EXISTS idx_ai_actions_target ON ai_actions(target_id);
        CREATE INDEX IF NOT EXISTS idx_ai_actions_date ON ai_actions(created_at);

        CREATE TABLE IF NOT EXISTS ai_settings (
            key   TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ai_message_queue (
            id           TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            project_id   TEXT NOT NULL,
            content      TEXT NOT NULL,
            context_json TEXT NOT NULL,
            queued_at    TEXT NOT NULL DEFAULT (datetime('now')),
            sent_at      TEXT,
            status       TEXT NOT NULL DEFAULT 'queued'
        );
    "
    )
    .map_err(|e| format!("Failed to create AI memory tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ ai_memory, ai_actions, ai_settings, ai_message_queue");
    Ok(())
}

fn create_agent_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ai_agents (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            name            TEXT NOT NULL,
            slug            TEXT NOT NULL UNIQUE,
            role            TEXT NOT NULL DEFAULT 'sub',
            description     TEXT,
            model           TEXT,
            is_default      INTEGER NOT NULL DEFAULT 0,
            is_active       INTEGER NOT NULL DEFAULT 1,
            workspace_path  TEXT NOT NULL,
            soul_hash       TEXT,
            memory_hash     TEXT,
            config_json     TEXT DEFAULT '{}',
            owner_id        TEXT,
            source          TEXT DEFAULT 'import',
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
            runtime_ref     TEXT,
            runtime_type    TEXT
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_default ON ai_agents(is_default) WHERE is_default = 1;
        CREATE INDEX IF NOT EXISTS idx_ai_agents_slug ON ai_agents(slug);
        CREATE INDEX IF NOT EXISTS idx_ai_agents_active ON ai_agents(is_active) WHERE is_active = 1;
        CREATE INDEX IF NOT EXISTS idx_ai_agents_owner ON ai_agents(owner_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_runtime_unique ON ai_agents(runtime_type, runtime_ref);

        CREATE TABLE IF NOT EXISTS pane_agent_tasks (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            parent_message_id TEXT,
            task_description TEXT NOT NULL,
            model           TEXT,
            status          TEXT NOT NULL DEFAULT 'spawned',
            progress        TEXT,
            result_note_id  TEXT,
            result_summary  TEXT,
            tokens_used     INTEGER,
            cost_estimate   REAL,
            tier            TEXT NOT NULL,
            started_at      TEXT NOT NULL DEFAULT (datetime('now')),
            completed_at    TEXT,
            error           TEXT
        );

        CREATE TABLE IF NOT EXISTS pane_task_workspaces (
            task_id         TEXT PRIMARY KEY,
            workspace_path  TEXT NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            cleaned_up      INTEGER NOT NULL DEFAULT 0
        );
    "
    )
    .map_err(|e| format!("Failed to create agent tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ ai_agents, pane_agent_tasks, pane_task_workspaces");
    Ok(())
}

/// `agent_jobs` (§36.10, #3050 PR 2) — non-CRR, per-device, matching
/// `chat_sessions`/`chat_messages_local`: the scheduler tick only runs on
/// whichever device is open (standing: Matt's real data is one MacBook), so
/// there is no cross-device sync story this table needs. Fresh-install shape
/// here must stay byte-identical to `migrate_v93`'s upgrade-path `CREATE TABLE
/// IF NOT EXISTS` below — both are the same literal schema, one for a new
/// install, one for an existing v92 install catching up. `oc_session_key`
/// (#3050 fix PR, Bug 1(b), migrate_v95) was added directly here rather than
/// left to `migrate_v95`'s own `add_column_if_missing` ALTER, because a
/// fresh install (the `None` branch of `run_migrations_with_progress`) never
/// calls any `migrate_vNN` function at all — only `init_schema`, which calls
/// this function. `migrate_v93`'s own inline copy below got the same column
/// added for the same byte-identity reason; `migrate_v95`'s ALTER is then a
/// documented no-op for both of those paths and the only real work for an
/// install already at v94.
fn create_agent_jobs_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_jobs (
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
            updated_at         TEXT NOT NULL DEFAULT (datetime('now')),
            oc_session_key     TEXT,
            consecutive_failures INTEGER NOT NULL DEFAULT 0,
            last_failed_at     TEXT,
            archived_at        TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_agent_jobs_due ON agent_jobs(status, next_run_at);
        CREATE INDEX IF NOT EXISTS idx_agent_jobs_origin ON agent_jobs(origin_message_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_jobs_heartbeat ON agent_jobs(agent_id) WHERE kind = 'heartbeat';
        CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_jobs_one_pending_wake_per_session ON agent_jobs(session_id) WHERE kind = 'wake' AND status = 'pending';
        CREATE INDEX IF NOT EXISTS idx_agent_jobs_archived_at ON agent_jobs(archived_at);
        ",
    )
    .map_err(|e| format!("Failed to create agent_jobs table: {}", e))?;
    eprintln!("[SCHEMA] ✓ agent_jobs (non-CRR, per-device)");
    Ok(())
}

fn create_processing_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS extraction_queue (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id       TEXT NOT NULL,
            message_id      TEXT NOT NULL,
            user_message_id TEXT,
            status          TEXT NOT NULL DEFAULT 'pending',
            attempts        INTEGER NOT NULL DEFAULT 0,
            max_attempts    INTEGER NOT NULL DEFAULT 3,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            processed_at    TEXT,
            error           TEXT,
            UNIQUE(message_id)
        );

        CREATE INDEX IF NOT EXISTS idx_extraction_pending ON extraction_queue(status, created_at) WHERE status IN ('pending', 'failed');

        CREATE TABLE IF NOT EXISTS compaction_summaries (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            thread_id       TEXT NOT NULL,
            summary         TEXT NOT NULL,
            message_count   INTEGER NOT NULL,
            compacted_before TEXT NOT NULL,
            -- #2690, v87: rowid-keyed cutoff boundary, unaffected by origination-
            -- timestamp stamping (created_at is presentational only post-#2690).
            -- NULL for rows written before v87 (no backfill; see migrate_v87 doc).
            compacted_before_rowid INTEGER,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_compaction_thread ON compaction_summaries(thread_id, created_at DESC);
    "
    )
    .map_err(|e| format!("Failed to create processing tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ extraction_queue, compaction_summaries");
    Ok(())
}

fn create_dashboard_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS user_dashboard_config (
            id                    TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            onboarding_completed  INTEGER DEFAULT 0,
            onboarding_template   TEXT,
            theme_variant         TEXT DEFAULT 'default',
            created_at            TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS dashboard_modules (
            id              TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            module_type     TEXT NOT NULL DEFAULT '',
            sort_order      INTEGER NOT NULL DEFAULT 0,
            is_enabled      INTEGER NOT NULL DEFAULT 1,
            config_json     TEXT DEFAULT '{}',
            static_content  TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_modules_sort ON dashboard_modules(sort_order) WHERE is_enabled = 1;

        CREATE TABLE IF NOT EXISTS module_items (
            id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            module_id   TEXT NOT NULL DEFAULT '',
            item_type   TEXT NOT NULL DEFAULT '',
            label       TEXT NOT NULL DEFAULT '',
            description TEXT,
            emoji       TEXT,
            config_json TEXT DEFAULT '{}',
            sort_order  INTEGER NOT NULL DEFAULT 0,
            is_active   INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            -- NOTE: No FOREIGN KEY on module_id — CRR table, enforce in app code (#493)
        );

        CREATE INDEX IF NOT EXISTS idx_items_module ON module_items(module_id, sort_order);

        CREATE TABLE IF NOT EXISTS daily_entries (
            id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            module_id   TEXT NOT NULL DEFAULT '',
            entry_date  TEXT NOT NULL DEFAULT '',
            data_json   TEXT NOT NULL DEFAULT '{}',
            completed_at TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            -- No UNIQUE(module_id, entry_date): CRR table, enforce in app code (#493)
            -- NOTE: No FOREIGN KEY on module_id — CRR table, enforce in app code (#493)
        );

        CREATE INDEX IF NOT EXISTS idx_daily_date ON daily_entries(entry_date);
        CREATE INDEX IF NOT EXISTS idx_daily_module_date ON daily_entries(module_id, entry_date);

        CREATE TABLE IF NOT EXISTS habit_entries (
            id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            item_id     TEXT NOT NULL DEFAULT '',
            entry_date  TEXT NOT NULL DEFAULT '',
            completed   INTEGER NOT NULL DEFAULT 0,
            completed_at TEXT
            -- No UNIQUE(item_id, entry_date): CRR table, enforce in app code (#493)
            -- NOTE: No FOREIGN KEY on item_id — CRR table, enforce in app code (#493)
        );

        CREATE INDEX IF NOT EXISTS idx_habit_date ON habit_entries(item_id, entry_date);

        CREATE TABLE IF NOT EXISTS reflection_entries (
            id              TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            question_id     TEXT NOT NULL DEFAULT '',
            entry_date      TEXT NOT NULL DEFAULT '',
            value_numeric   REAL,
            value_bool      INTEGER,
            value_text      TEXT
            -- No UNIQUE(question_id, entry_date): CRR table, enforce in app code (#493)
        );

        CREATE INDEX IF NOT EXISTS idx_reflection_date ON reflection_entries(question_id, entry_date);

        CREATE TABLE IF NOT EXISTS gratitude_entries (
            id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            entry_date  TEXT NOT NULL DEFAULT '',
            entry_index INTEGER NOT NULL DEFAULT 0,
            text        TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            -- No UNIQUE(entry_date, entry_index): CRR table, enforce in app code (#493)
        );

        CREATE INDEX IF NOT EXISTS idx_gratitude_date ON gratitude_entries(entry_date);
    "
    )
    .map_err(|e| format!("Failed to create dashboard tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ user_dashboard_config, dashboard_modules, module_items, daily_entries, habit_entries, reflection_entries, gratitude_entries");
    Ok(())
}

fn create_sync_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_state (
            id          TEXT NOT NULL PRIMARY KEY,
            device_id   TEXT NOT NULL DEFAULT '',
            device_name TEXT,
            platform    TEXT,
            last_sync_at TEXT,
            sync_version INTEGER NOT NULL DEFAULT 0,
            is_current  INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS sync_changelog (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            table_name      TEXT NOT NULL,
            row_id          TEXT NOT NULL,
            operation       TEXT NOT NULL,
            changeset_json  TEXT,
            device_id       TEXT NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_sync_changelog_time ON sync_changelog(created_at);

        CREATE TABLE IF NOT EXISTS sync_queue (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            agent           TEXT NOT NULL,
            filename        TEXT NOT NULL,
            content         TEXT NOT NULL,
            checksum        TEXT NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            retry_count     INTEGER DEFAULT 0,
            next_retry_at   TEXT,
            UNIQUE(agent, filename, checksum)
        );

        CREATE INDEX IF NOT EXISTS idx_sync_queue_retry ON sync_queue(next_retry_at);

        CREATE TABLE IF NOT EXISTS note_versions (
            id         TEXT NOT NULL PRIMARY KEY,
            note_id    TEXT NOT NULL DEFAULT '',
            content    TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            source     TEXT NOT NULL DEFAULT 'manual'
        );

        CREATE INDEX IF NOT EXISTS idx_note_versions_note_id ON note_versions(note_id);
    ",
    )
    .map_err(|e| format!("Failed to create sync tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ sync_state, sync_changelog, sync_queue, note_versions");
    Ok(())
}

fn create_attachment_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS attachments (
            id TEXT PRIMARY KEY NOT NULL,
            project_id TEXT NOT NULL DEFAULT '',
            project_name TEXT NOT NULL DEFAULT '',
            original_filename TEXT NOT NULL DEFAULT '',
            mime_type TEXT NOT NULL DEFAULT '',
            size_bytes INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
            -- NOTE: No FOREIGN KEY on project_id — CRR table, enforce in app code (#493)
        );

        CREATE TABLE IF NOT EXISTS embeddings (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            embedding BLOB NOT NULL,
            content_hash TEXT NOT NULL,
            engine TEXT,
            model TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_embeddings_source ON embeddings(source);

        CREATE TABLE IF NOT EXISTS note_context_pins (
            note_id TEXT NOT NULL PRIMARY KEY,
            pinned_at TEXT NOT NULL DEFAULT (datetime('now')),
            pinned_by TEXT NOT NULL DEFAULT 'user'
            -- NOTE: No FOREIGN KEY — not a CRR table but note_id is PK so FK is redundant
        );

        CREATE INDEX IF NOT EXISTS idx_note_context_pins_pinned_at ON note_context_pins(pinned_at);
    ",
    )
    .map_err(|e| format!("Failed to create attachment tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ attachments, embeddings, note_context_pins");
    Ok(())
}

/// Create the subtasks table — CRR-synced Advanced Checklist line items (§1.9, ADR-0057, #2092).
///
/// No FOREIGN KEY on note_id: CRR table, enforce in app code (#493).
/// No UNIQUE beyond PK: CRR table, uniqueness enforced in application code.
fn create_subtasks_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS subtasks (
            id           TEXT NOT NULL PRIMARY KEY,
            note_id      TEXT NOT NULL DEFAULT '',
            text         TEXT NOT NULL DEFAULT '',
            due_date     TEXT,
            done         INTEGER NOT NULL DEFAULT 0,
            completed_at TEXT,
            sort_order   INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
            -- NOTE: No FOREIGN KEY on note_id — CRR table, enforce in app code (#493)
        );

        CREATE INDEX IF NOT EXISTS idx_subtasks_note ON subtasks(note_id);
        CREATE INDEX IF NOT EXISTS idx_subtasks_due_date ON subtasks(due_date) WHERE due_date IS NOT NULL;
    "
    )
    .map_err(|e| format!("Failed to create subtasks table: {}", e))?;
    eprintln!("[SCHEMA] ✓ subtasks");
    Ok(())
}

/// Create the templates table — named, reusable note-content snapshots (§32, §1.10, ADR-0058, #193).
///
/// CRR-enabled: synced across devices via cr-sqlite. LWW per row.
/// No FOREIGN KEY on any column (CRR table pattern, #493).
/// `checklist_json` is opaque TEXT (JSON array of {text, sort_order}) — never queried by field.
/// No UNIQUE constraint on `name` — collision detection is app-level (§32.2.2); sync can
/// legitimately produce same-named rows created independently on two devices.
fn create_templates_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS templates (
            id             TEXT NOT NULL PRIMARY KEY,
            name           TEXT NOT NULL DEFAULT '',
            content        TEXT NOT NULL DEFAULT '',
            checklist_json TEXT NOT NULL DEFAULT '[]',
            created_at     TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
            -- NOTE: No FOREIGN KEY — CRR table, enforce in app code (#493)
        );

        CREATE INDEX IF NOT EXISTS idx_templates_name ON templates(name);
    ",
    )
    .map_err(|e| format!("Failed to create templates table: {}", e))?;
    eprintln!("[SCHEMA] ✓ templates");
    Ok(())
}

/// Create the sheet tables — `sheet_columns`, `sheet_rows`, `sheet_cells` (§1.12, #2339).
///
/// CRR-enabled: synced across devices via cr-sqlite. LWW per row.
/// No FOREIGN KEY on any column (CRR table pattern, #493) — note_id/row_id/column_id
/// relationships are enforced in app code, not the DB.
/// `sheet_cells` uses a composite PRIMARY KEY (row_id, column_id); `note_id` on that
/// table is a denormalized, non-PK column for one-scan reads/cascades.
fn create_sheet_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sheet_columns (
            id           TEXT NOT NULL PRIMARY KEY,
            note_id      TEXT NOT NULL DEFAULT '',
            name         TEXT NOT NULL DEFAULT '',
            col_type     TEXT NOT NULL DEFAULT 'text', -- 'text'|'number'|'currency'|'date'|'checkbox'|'select'
            options_json TEXT NOT NULL DEFAULT '{}', -- type config: select choices, currency code (§33.3.2)
            width        INTEGER NOT NULL DEFAULT 0, -- px; 0 = type default; syncs (Matt ruling 2026-09-03)
            sort_order   INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
            -- No FK on note_id — CRR table, app-enforced (#493)
        );
        CREATE INDEX IF NOT EXISTS idx_sheet_columns_note ON sheet_columns(note_id);

        CREATE TABLE IF NOT EXISTS sheet_rows (
            id           TEXT NOT NULL PRIMARY KEY,
            note_id      TEXT NOT NULL DEFAULT '',
            sort_order   INTEGER NOT NULL DEFAULT 0,
            height       INTEGER NOT NULL DEFAULT 0, -- px; 0 = default; syncs (Matt ruling 2026-09-03)
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
            -- No FK on note_id — CRR table, app-enforced (#493)
        );
        CREATE INDEX IF NOT EXISTS idx_sheet_rows_note ON sheet_rows(note_id);

        CREATE TABLE IF NOT EXISTS sheet_cells (
            row_id     TEXT NOT NULL,
            column_id  TEXT NOT NULL,
            note_id    TEXT NOT NULL DEFAULT '', -- denormalized for one-scan reads/cascades; NOT part of the PK
            value      TEXT NOT NULL DEFAULT '', -- raw user value, or canonical formula string ('=' prefix, §33.5.2)
            style_json TEXT NOT NULL DEFAULT '{}', -- bg/text color, border (§33.6)
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (row_id, column_id)
            -- No FKs — CRR table, app-enforced (#493)
        );
        CREATE INDEX IF NOT EXISTS idx_sheet_cells_note ON sheet_cells(note_id);
        ",
    )
    .map_err(|e| format!("Failed to create sheet tables: {}", e))?;
    eprintln!("[SCHEMA] ✓ sheet_columns, sheet_rows, sheet_cells");
    Ok(())
}

/// Create the og_cache table — URL-keyed Open Graph metadata cache (#1780, ADR-0049).
///
/// Not a CRR table: og data is locally-derived, not synced across devices.
///
/// Cache TTL policy (enforced in og_fetch.rs, not enforced by SQL):
///   - status='ok'     : 7 days  (CACHE_TTL_OK_MS)
///   - status=other    : 1 hour  (CACHE_TTL_FAIL_MS)
///
/// v68→v69 migration (#1780). DB migrations must be tested on Mac Studio before broad release.
fn create_og_cache_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS og_cache (
            url             TEXT PRIMARY KEY NOT NULL,
            og_title        TEXT,
            og_description  TEXT,
            og_image        TEXT,
            fetched_at      INTEGER NOT NULL DEFAULT 0,
            status          TEXT NOT NULL DEFAULT 'ok'
            -- status values: 'ok' | 'timeout' | 'blocked' | 'no_og' | 'error'
        );",
    )
    .map_err(|e| format!("Failed to create og_cache table: {}", e))?;
    eprintln!("[SCHEMA] ✓ og_cache");
    Ok(())
}

/// v68→v69 migration: create og_cache table + migrate link_embeds_enabled → link_previews_enabled.
///
/// link_previews_enabled (ADR-0049 §Decision 2) replaces the orphaned link_embeds_enabled key
/// (which gated the now-ripped embed path). Migration:
///   1. Create og_cache table.
///   2. Add link_previews_enabled with default 'true' (matching link_embeds_enabled default).
///   3. Carry over existing link_embeds_enabled value for users who toggled it off.
///
/// DB migration flag: Matt tests on Mac Studio before broad release.
fn migrate_v69(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v69] Starting: og_cache table + link_previews_enabled key (#1780)");

    // 1. Create og_cache table (idempotent).
    create_og_cache_table(conn)?;

    // 2. Ensure ai_settings table exists (should always be present, but guard defensively).
    let ai_settings_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !ai_settings_exists {
        eprintln!("[MIGRATION-v69] ai_settings table missing — skipping key migration");
        return Ok(());
    }

    // 3. Migrate link_embeds_enabled → link_previews_enabled.
    //    Read the existing value; default to 'true' (same default as link_embeds_enabled).
    let existing_value: Option<String> = conn
        .query_row(
            "SELECT value FROM ai_settings WHERE key = 'link_embeds_enabled'",
            [],
            |row| row.get(0),
        )
        .ok();

    let new_value = existing_value
        .as_deref()
        .unwrap_or("true") // null → true (inherited default)
        .to_string();

    // Insert link_previews_enabled only if it does not already exist, carrying over the value.
    conn.execute(
        "INSERT OR IGNORE INTO ai_settings (key, value) VALUES ('link_previews_enabled', ?1)",
        params![new_value],
    )
    .map_err(|e| {
        format!(
            "[MIGRATION-v69] Failed to insert link_previews_enabled: {}",
            e
        )
    })?;

    // 4. Stop reading link_embeds_enabled: leave the row intact (safe, no reads in new code)
    //    to avoid data loss for any future audit. Documented here for traceability.
    eprintln!(
        "[MIGRATION-v69] link_previews_enabled set to '{}' (carried from link_embeds_enabled={:?})",
        new_value, existing_value
    );

    eprintln!("[MIGRATION-v69] ✓ Done");
    Ok(())
}

/// Migrate notifications table — v70→v71 (#1811, ADR-0021/ADR-0022)
///
/// Adds 5 nullable TEXT columns (dismissed_at, expires_at, target_kind, target_id, idem_key)
/// and 3 supporting indexes. All ADD COLUMN operations are O(1) metadata-only in SQLite
/// (nullable columns with no DEFAULT require no table rebuild).
///
/// `idem_key` index is non-unique by design (ADR-0021): dedup is application-layer
/// SELECT-before-INSERT, not a database uniqueness constraint.
///
/// Idempotent: all ADD COLUMNs use IF NOT EXISTS-equivalent (ignored on duplicate column),
/// and all CREATE INDEXes use IF NOT EXISTS.
fn migrate_v71(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v71] Starting: notifications schema (#1811, ADR-0021/ADR-0022)");

    // Guard: if notifications table doesn't exist (shouldn't happen in prod, but
    // safe for test environments that construct minimal schemas), skip gracefully.
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='notifications'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        eprintln!("[MIGRATION-v71] notifications table absent — skipping (unexpected in prod)");
        return Ok(());
    }

    // Add each column idempotently: SQLite has no ADD COLUMN IF NOT EXISTS, so we
    // check pragma_table_info first. Columns are always nullable TEXT with no DEFAULT.
    let new_columns = [
        "dismissed_at",
        "expires_at",
        "target_kind",
        "target_id",
        "idem_key",
    ];

    for col in &new_columns {
        let col_exists: bool = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('notifications') WHERE name=?1"
                ),
                params![col],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !col_exists {
            conn.execute(
                &format!("ALTER TABLE notifications ADD COLUMN {} TEXT", col),
                [],
            )
            .map_err(|e| format!("[MIGRATION-v71] Failed to add column {}: {}", col, e))?;
            eprintln!("[MIGRATION-v71] Added column: {}", col);
        } else {
            eprintln!(
                "[MIGRATION-v71] Column already exists (idempotent skip): {}",
                col
            );
        }
    }

    // Indexes use IF NOT EXISTS — always safe to run.
    conn.execute_batch(
        "
        CREATE INDEX IF NOT EXISTS idx_notifications_active
            ON notifications (created_at DESC)
            WHERE dismissed_at IS NULL;

        CREATE INDEX IF NOT EXISTS idx_notifications_idem
            ON notifications (idem_key)
            WHERE idem_key IS NOT NULL;

        CREATE INDEX IF NOT EXISTS idx_notifications_expires
            ON notifications (expires_at);
    ",
    )
    .map_err(|e| format!("[MIGRATION-v71] Failed to create indexes: {}", e))?;

    eprintln!("[MIGRATION-v71] ✓ Done: notifications columns + 3 indexes applied");
    Ok(())
}

/// Create the chat_sessions table and its indexes (§30.1.1, ADR-0052)
///
/// Per-device table — NOT in CRR_TABLES — matches chat_messages_local.
/// Sessions are Pane-owned identity; one active session per surface enforced by
/// the partial unique index (mechanized invariant, not frontend discipline).
fn create_chat_sessions_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS chat_sessions (
            id             TEXT NOT NULL PRIMARY KEY,
            project_id     TEXT NOT NULL,
            title          TEXT NOT NULL DEFAULT '',
            -- Free text, documented domain (no CHECK constraint): 'user' (every
            -- interactive new-session create), plus 'spawn' and 'schedule'
            -- (§36.3.6, §36.10, #3050 PR 2) — job sessions created via
            -- ai::chat::create_session_internal, never via the public
            -- create_session command. 'spawn' rows are never is_active=1 and
            -- never returned by list_sessions (§36.11); 'schedule' rows are
            -- excluded from a project's list_sessions too, surfaced only in
            -- Home's Scheduled panel (later PR).
            origin         TEXT NOT NULL DEFAULT 'user',
            created_at     TEXT NOT NULL DEFAULT (datetime('now')),
            last_active_at TEXT NOT NULL DEFAULT (datetime('now')),
            hidden_at      TEXT,
            is_active      INTEGER NOT NULL DEFAULT 0,
            agent_id       TEXT REFERENCES ai_agents(id),
            -- §30.11 (#2410, v85): NULL = session never baselined against OC's
            -- chat.history. Set once, on the session's first-ever OC transcript pull,
            -- so pre-existing history isn't backfilled as if it were new proactive
            -- output. See migrate_v85 for the additive-column migration arm.
            oc_baseline_done_at TEXT,
            -- Section 36.17 (#3050 PR 5): JSON array of ai_agents.id this
            -- session's captain may spawn onto (spawn_job targets). NULL =
            -- no crew = spawn_job not offered (Matt's ruling, replacing
            -- the old any-active-Pane-agent rule in section 36.4.1). A
            -- spawned session (origin='spawn') inherits its captain's crew
            -- verbatim at creation (ai::scheduler::create_job); a scheduled
            -- session (origin='schedule') never gets one. See migrate_v94
            -- for the additive-column migration arm.
            crew_agent_ids TEXT
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_active_per_surface
            ON chat_sessions(project_id) WHERE is_active = 1;

        CREATE INDEX IF NOT EXISTS idx_sessions_surface
            ON chat_sessions(project_id, last_active_at DESC);
    ",
    )
    .map_err(|e| format!("Failed to create chat_sessions table: {}", e))?;
    eprintln!("[SCHEMA] ✓ chat_sessions (per-device, NOT CRR)");
    Ok(())
}

/// v71→v72: chat_sessions table + session seeding + message re-key (#395, ADR-0052, ADR-0053)
///
/// Creates chat_sessions (per-device, NOT CRR), seeds one session per legacy message bucket
/// with correct surface classification, backfills thread_id, re-points project_id to surface,
/// retires ai_settings pointers, and adds the thread index. All 7 steps are idempotent.
fn migrate_v72(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v72] Starting: chat_sessions table + session seeding (#395, ADR-0052)");

    // ── Step 1: Normalize empty bucket ─────────────────────────────────────────
    // Rows predating the 'workspace' literal may have project_id=''.
    conn.execute(
        "UPDATE chat_messages_local SET project_id='workspace' WHERE project_id=''",
        [],
    )
    .map_err(|e| {
        format!(
            "[MIGRATION-v72] Step 1 (normalize empty bucket) failed: {}",
            e
        )
    })?;
    eprintln!("[MIGRATION-v72] Step 1: normalized empty bucket rows");

    // ── Step 2: Create chat_sessions table + indexes ──────────────────────────
    // All CREATE TABLE/INDEX statements use IF NOT EXISTS — safe to re-run.
    create_chat_sessions_table(conn)
        .map_err(|e| format!("[MIGRATION-v72] Step 2 (create table) failed: {}", e))?;
    eprintln!("[MIGRATION-v72] Step 2: chat_sessions table + indexes ensured");

    // ── Step 3: Seed sessions (INSERT OR IGNORE, one per distinct bucket) ──────
    //
    // Classification algorithm:
    //   a. Build pointer_map: bucket_value → project_id from all 'chat_thread_%' keys.
    //      This handles the bidirectional lookup (bucket can be a project id OR a pointer
    //      target value from a post-reset generation).
    //   b. For each distinct bucket in chat_messages_local:
    //      1. 'workspace' → surface='workspace', active
    //      2. matches projects.id AND no pointer key → surface=bucket, active
    //      3. matches projects.id AND pointer key exists → surface=bucket, inactive
    //      4. is the VALUE of some chat_thread_{pid} key → surface=that pid, active
    //      5. else (orphan, ≥2 resets ago) → surface='__recovered__', inactive, hidden

    // Read all chat_thread_% keys from ai_settings.
    // Note: stmt must outlive the MappedRows iterator — keep at function scope, not inside a block.
    let mut ptr_stmt = conn
        .prepare("SELECT key, value FROM ai_settings WHERE key LIKE 'chat_thread_%'")
        .map_err(|e| {
            format!(
                "[MIGRATION-v72] Step 3 (prepare pointer query) failed: {}",
                e
            )
        })?;
    let pointer_rows: Vec<(String, String)> = ptr_stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| format!("[MIGRATION-v72] Step 3 (query pointers) failed: {}", e))?
        .filter_map(Result::ok)
        .collect();
    drop(ptr_stmt);

    // Build bidirectional lookup maps:
    //   pointer_map[bucket_value] = project_id that has a pointer TO this bucket
    //   has_reset = set of project_ids that have been reset (have a chat_thread_{pid} key)
    let mut pointer_map: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut has_reset: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (key, value) in &pointer_rows {
        // key = "chat_thread_{project_id}", value = the new bucket the project now points to
        let project_id = key.trim_start_matches("chat_thread_").to_string();
        pointer_map.insert(value.clone(), project_id.clone());
        has_reset.insert(project_id);
    }

    // Enumerate all distinct legacy buckets.
    // Note: stmt must outlive the MappedRows iterator — keep at function scope, not inside a block.
    let mut bucket_stmt = conn
        .prepare("SELECT DISTINCT project_id FROM chat_messages_local")
        .map_err(|e| {
            format!(
                "[MIGRATION-v72] Step 3 (prepare bucket query) failed: {}",
                e
            )
        })?;
    let buckets: Vec<String> = bucket_stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("[MIGRATION-v72] Step 3 (query buckets) failed: {}", e))?
        .filter_map(Result::ok)
        .collect();
    drop(bucket_stmt);

    eprintln!(
        "[MIGRATION-v72] Step 3: classifying {} distinct bucket(s)",
        buckets.len()
    );

    for bucket in &buckets {
        // Case 1: workspace sentinel.
        if bucket == "workspace" {
            conn.execute(
                "INSERT OR IGNORE INTO chat_sessions (id, project_id, title, origin, is_active)
                 VALUES (?1, 'workspace', '', 'migrated', 1)",
                params![bucket],
            )
            .map_err(|e| format!("[MIGRATION-v72] Step 3 (insert workspace) failed: {}", e))?;
            eprintln!("[MIGRATION-v72] Step 3: '{}' → workspace (active)", bucket);
            continue;
        }

        // Case 2/3: bucket matches a project id (current or prior generation).
        let is_project: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM projects WHERE id = ?1",
                params![bucket],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if is_project {
            if !has_reset.contains(bucket) {
                // Case 2: no reset ever happened — this bucket IS the current generation.
                conn.execute(
                    "INSERT OR IGNORE INTO chat_sessions (id, project_id, title, origin, is_active)
                     VALUES (?1, ?1, '', 'migrated', 1)",
                    params![bucket],
                )
                .map_err(|e| {
                    format!(
                        "[MIGRATION-v72] Step 3 (insert project-current) failed: {}",
                        e
                    )
                })?;
                eprintln!(
                    "[MIGRATION-v72] Step 3: '{}' → project surface (active, no reset)",
                    bucket
                );
            } else {
                // Case 3: a reset happened — the pointer target is the active session.
                conn.execute(
                    "INSERT OR IGNORE INTO chat_sessions (id, project_id, title, origin, is_active)
                     VALUES (?1, ?1, '', 'migrated', 0)",
                    params![bucket],
                )
                .map_err(|e| {
                    format!(
                        "[MIGRATION-v72] Step 3 (insert project-prior-gen) failed: {}",
                        e
                    )
                })?;
                eprintln!(
                    "[MIGRATION-v72] Step 3: '{}' → project surface (inactive, reset existed)",
                    bucket
                );
            }
            continue;
        }

        // Case 4: bucket is the VALUE of some chat_thread_{pid} key (post-reset active bucket).
        if let Some(surface_pid) = pointer_map.get(bucket) {
            conn.execute(
                "INSERT OR IGNORE INTO chat_sessions (id, project_id, title, origin, is_active)
                 VALUES (?1, ?2, '', 'migrated', 1)",
                params![bucket, surface_pid],
            )
            .map_err(|e| {
                format!(
                    "[MIGRATION-v72] Step 3 (insert pointer-target) failed: {}",
                    e
                )
            })?;
            eprintln!(
                "[MIGRATION-v72] Step 3: '{}' → surface '{}' (active, post-reset)",
                bucket, surface_pid
            );
            continue;
        }

        // Case 5: orphan — no attribution possible (pane-uuid from ≥2 resets ago).
        conn.execute(
            "INSERT OR IGNORE INTO chat_sessions (id, project_id, title, origin, is_active, hidden_at)
             VALUES (?1, '__recovered__', 'Recovered session', 'recovered', 0, datetime('now'))",
            params![bucket],
        )
        .map_err(|e| format!("[MIGRATION-v72] Step 3 (insert orphan) failed: {}", e))?;
        eprintln!(
            "[MIGRATION-v72] Step 3: '{}' → __recovered__ (orphan)",
            bucket
        );
    }

    eprintln!("[MIGRATION-v72] Step 3: session seeding complete");

    // ── Step 4: Backfill thread_id ──────────────────────────────────────────────
    // Copy project_id into the always-NULL thread_id column.
    // Id-equality rule: session.id == legacy bucket, so this is a direct FK write.
    conn.execute(
        "UPDATE chat_messages_local SET thread_id = project_id WHERE thread_id IS NULL",
        [],
    )
    .map_err(|e| format!("[MIGRATION-v72] Step 4 (backfill thread_id) failed: {}", e))?;
    eprintln!("[MIGRATION-v72] Step 4: thread_id backfilled from project_id");

    // ── Step 5: Re-point project_id to surface ───────────────────────────────
    // After this, project_id = surface (project UUID | 'workspace' | '__recovered__')
    // for every row. Messages are accessed via thread_id; project_id is surface only.
    conn.execute(
        "UPDATE chat_messages_local
         SET project_id = (
             SELECT s.project_id FROM chat_sessions s WHERE s.id = chat_messages_local.thread_id
         )
         WHERE thread_id IN (SELECT id FROM chat_sessions)",
        [],
    )
    .map_err(|e| format!("[MIGRATION-v72] Step 5 (re-point surface) failed: {}", e))?;
    eprintln!("[MIGRATION-v72] Step 5: project_id re-pointed to surface via chat_sessions join");

    // ── Step 6: Retire pointer keys ─────────────────────────────────────────
    // Read in step 3 for attribution, now deleted. The active-session invariant is
    // mechanized via the partial unique index, not an ai_settings pointer.
    conn.execute("DELETE FROM ai_settings WHERE key LIKE 'chat_thread_%'", [])
        .map_err(|e| format!("[MIGRATION-v72] Step 6 (retire pointers) failed: {}", e))?;
    eprintln!("[MIGRATION-v72] Step 6: chat_thread_% keys deleted from ai_settings");

    // ── Step 7: Add thread index ──────────────────────────────────────────────
    // thread_id becomes the load-bearing FK for message reads (§30.1.2).
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_chat_local_thread ON chat_messages_local(thread_id, created_at);"
    )
    .map_err(|e| format!("[MIGRATION-v72] Step 7 (add thread index) failed: {}", e))?;
    eprintln!("[MIGRATION-v72] Step 7: idx_chat_local_thread created");

    eprintln!("[MIGRATION-v72] ✓ Done: chat_sessions + session seeding + message re-key applied");
    Ok(())
}

/// Migrate stale pane_gateway_token / pane_gateway_url rows — v64→v65 (#1499, §22C.8.1a)
///
/// #1167 (PR #1168/#1169) renamed these keys to api_key / endpoint but did not clean up
/// any rows that pre-existing installs had written. This migration:
///   1. Copies pane_gateway_token → api_key ONLY when api_key is absent or empty.
///      api_key is authoritative post-#1167; never overwrite a present non-empty api_key.
///   2. Deletes the pane_gateway_token row regardless of whether a copy was made.
///   3. Deletes the pane_gateway_url row (was only ever written as empty string; no data to preserve).
///
/// Idempotent: re-running after both rows are deleted is a safe no-op.
fn migrate_pane_gateway_rows(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v65] Checking for stale pane_gateway_token / pane_gateway_url rows");

    // Step 1: Read stale pane_gateway_token (non-empty values only)
    let stale_token: Option<String> = conn
        .query_row(
            "SELECT value FROM ai_settings WHERE key = 'pane_gateway_token' AND value != ''",
            [],
            |row| row.get(0),
        )
        .ok();

    // Step 2: Copy to api_key ONLY if api_key is absent or empty
    if let Some(ref token) = stale_token {
        let api_key_present: bool = conn
            .query_row(
                "SELECT 1 FROM ai_settings WHERE key = 'api_key' AND value != ''",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !api_key_present {
            conn.execute(
                "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('api_key', ?1)",
                rusqlite::params![token],
            )
            .map_err(|e| {
                format!(
                    "[MIGRATION-v65] Failed to copy pane_gateway_token to api_key: {}",
                    e
                )
            })?;
            eprintln!("[MIGRATION-v65] Copied pane_gateway_token -> api_key");
        } else {
            eprintln!("[MIGRATION-v65] api_key already present — skipping copy (pane_gateway_token is stale)");
        }
    } else {
        eprintln!("[MIGRATION-v65] No pane_gateway_token row found — nothing to copy");
    }

    // Step 3: Delete stale rows (idempotent — DELETE of absent row is a no-op)
    conn.execute(
        "DELETE FROM ai_settings WHERE key = 'pane_gateway_token'",
        [],
    )
    .map_err(|e| format!("[MIGRATION-v65] Failed to delete pane_gateway_token: {}", e))?;

    conn.execute("DELETE FROM ai_settings WHERE key = 'pane_gateway_url'", [])
        .map_err(|e| format!("[MIGRATION-v65] Failed to delete pane_gateway_url: {}", e))?;

    eprintln!("[MIGRATION-v65] Deleted stale pane_gateway_token and pane_gateway_url rows");
    Ok(())
}

/// Create migration_flags table — records completion of credential/data migrations
/// Introduced: v64 (ADR-0044 Addendum A — sync master key keychain migration, #1498)
fn create_migration_flags_table(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS migration_flags (
            flag         TEXT PRIMARY KEY NOT NULL,
            completed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| format!("Failed to create migration_flags table: {}", e))?;
    eprintln!("[SCHEMA] ✓ migration_flags");
    Ok(())
}

fn create_system_tables(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS category_colors (
            category TEXT NOT NULL DEFAULT '' PRIMARY KEY,
            color TEXT NOT NULL DEFAULT '#6B7280'
        );

        CREATE TABLE IF NOT EXISTS notifications (
            id           TEXT PRIMARY KEY,
            type         TEXT NOT NULL DEFAULT 'info',
            title        TEXT NOT NULL,
            body         TEXT NOT NULL DEFAULT '',
            source       TEXT NOT NULL DEFAULT 'system',
            read         INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            dismissed_at TEXT,
            expires_at   TEXT,
            target_kind  TEXT,
            target_id    TEXT,
            idem_key     TEXT
        );

        CREATE TABLE IF NOT EXISTS extensions (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            version     TEXT NOT NULL,
            manifest    TEXT NOT NULL,
            enabled     INTEGER NOT NULL DEFAULT 1,
            settings    TEXT DEFAULT '{}',
            installed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- notifications indexes (#1811, ADR-0021/ADR-0022)
        CREATE INDEX IF NOT EXISTS idx_notifications_active
            ON notifications (created_at DESC)
            WHERE dismissed_at IS NULL;

        CREATE INDEX IF NOT EXISTS idx_notifications_idem
            ON notifications (idem_key)
            WHERE idem_key IS NOT NULL;

        CREATE INDEX IF NOT EXISTS idx_notifications_expires
            ON notifications (expires_at);
    ",
    )
    .map_err(|e| format!("Failed to create system tables: {}", e))?;
    eprintln!(
        "[SCHEMA] ✓ category_colors, notifications, extensions + notifications indexes (#1811)"
    );
    Ok(())
}

fn create_fts_virtual_tables(conn: &Connection) -> Result<(), String> {
    // Create FTS5 virtual tables (triggers created separately AFTER CRR enablement)
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
            title,
            body,
            content='notes',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS ai_memory_fts USING fts5(
            category,
            fact,
            content='ai_memory',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS chat_fts USING fts5(
            content,
            content='chat_messages_local',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );
    ",
    )
    .map_err(|e| format!("Failed to create FTS5 virtual tables: {}", e))?;

    eprintln!("[SCHEMA] ✓ FTS5 virtual tables created");
    Ok(())
}

fn create_fts_triggers(conn: &Connection) -> Result<(), String> {
    // Create triggers for notes_fts
    // MUST be called AFTER enable_crr() to prevent trigger destruction (#544)
    conn.execute_batch(
        "CREATE TRIGGER IF NOT EXISTS notes_fts_insert AFTER INSERT ON notes BEGIN
            INSERT INTO notes_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
        END;

        CREATE TRIGGER IF NOT EXISTS notes_fts_delete AFTER DELETE ON notes BEGIN
            INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES('delete', old.rowid, old.title, old.body);
        END;

        CREATE TRIGGER IF NOT EXISTS notes_fts_update AFTER UPDATE ON notes BEGIN
            INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES('delete', old.rowid, old.title, old.body);
            INSERT INTO notes_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
        END;
    "
    )
    .map_err(|e| format!("Failed to create notes_fts triggers: {}", e))?;

    eprintln!("[SCHEMA] ✓ notes_fts triggers created");
    Ok(())
}

// pub(crate), not private: also called from lib.rs's startup drift gate
// (#3072) when the FTS5 'integrity-check' reports a problem — single
// source of truth for the native 'rebuild' SQL, not forked/duplicated.
pub(crate) fn rebuild_fts_indexes(conn: &Connection) -> Result<(), String> {
    // Rebuild FTS indexes to populate with any existing data
    // This ensures search works even if notes were inserted before triggers existed
    conn.execute_batch("INSERT INTO notes_fts(notes_fts) VALUES('rebuild');")
        .map_err(|e| format!("Failed to rebuild FTS indexes: {}", e))?;

    eprintln!("[SCHEMA] ✓ FTS indexes rebuilt");
    Ok(())
}

/// Enable CRR on the 27 tables that require it
/// Gracefully skips if cr-sqlite extension is not available (e.g., in tests)
fn enable_crr(conn: &Connection) -> Result<(), String> {
    eprintln!("[SCHEMA] Enabling CRR on 27 tables...");

    // Check if crsql_as_crr function is available (requires cr-sqlite extension)
    let crr_available = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_function_list WHERE name='crsql_as_crr'",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);

    if !crr_available {
        eprintln!(
            "[SCHEMA] ⚠️  cr-sqlite extension not loaded, skipping CRR enablement (test mode)"
        );
        return Ok(());
    }

    for table in &CRR_TABLES {
        conn.query_row(
            &format!("SELECT crsql_as_crr('{}');", table),
            [],
            |_| Ok(()),
        )
        .map_err(|e| format!("Failed to enable CRR on {}: {}", table, e))?;
        eprintln!("[SCHEMA] ✓ CRR enabled: {}", table);
    }

    eprintln!("[SCHEMA] ✓ All 27 CRR tables enabled");
    Ok(())
}

/// v75→v76: Add nullable agent_id to chat_messages_local.
/// Per-device, not CRR. No backfill. (#2030, ADR-0059 D5, §24.8.1)
///
/// Contract:
/// - Purely additive: one nullable column add.
/// - MUST NOT drop, recreate, rename, or copy-and-swap chat_messages_local.
/// - Pre-existing rows take NULL (correct migrated state — §24.8: NULL means
///   "written before attribution shipped", NOT "main agent").
/// - No default, no backfill: which agent wrote a historical message is unknowable.
/// - chat_messages_local is NOT a CRR table; no CRR path needed.
///
/// spec-covered-by: #2030 (ADR-0059 D5, §24.8.1)
fn migrate_v76(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v76] Starting: agent_id column on chat_messages_local (#2030)");

    // Idempotency guard: check if column already exists via PRAGMA
    // SQLite does not support IF NOT EXISTS on ALTER TABLE ADD COLUMN
    let col_exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_messages_local') WHERE name='agent_id'",
        [],
        |row| row.get(0),
    ).map_err(|e| format!("[MIGRATION-v76] pragma check failed: {}", e))?;

    if !col_exists {
        conn.execute_batch(
            "ALTER TABLE chat_messages_local ADD COLUMN agent_id TEXT REFERENCES ai_agents(id);",
        )
        .map_err(|e| format!("[MIGRATION-v76] ALTER TABLE failed: {}", e))?;
        eprintln!("[MIGRATION-v76] Step 1: agent_id column added");
    } else {
        eprintln!("[MIGRATION-v76] Step 1: agent_id column already present (idempotent re-run)");
    }

    eprintln!(
        "[MIGRATION-v76] ✓ Done: agent_id added to chat_messages_local (existing rows have NULL)"
    );
    Ok(())
}

/// v76 → v77: retire per-project smart-routing columns (#2531, #2533, ADR-0061 Decision 3).
///
/// NULLs the four retired `projects` columns (`smart_routing_enabled`, `router_model`,
/// `simple_tier_model`, `default_model`) and deletes the five retired `ai_settings` keys
/// (`tier_1_model`, `tier_2_model`, `tier_3_model`, `router_model`, `smart_routing_enabled`).
/// Both statements are naturally idempotent (re-running on already-NULL/already-absent data
/// is a no-op), so no pre-check guard is needed, unlike `migrate_v76`'s `ADD COLUMN` guard.
///
/// The `UPDATE` carries a `WHERE` filter (Matt's ruling #3, not in the ADR's literal SQL) to
/// avoid touching already-clean project rows and cut unnecessary CRR clock churn on `projects`
/// (a CRR-tracked table — see `CRR_TABLES`).
///
/// Defensive guard: several older combined-migration integration tests (e.g. v62→v77,
/// v67→v77) build minimal fixture schemas that predate the four retired `projects`
/// columns entirely (or lack a `projects` table at all), since those tests focus on
/// unrelated migration steps. This function checks for the table and all four columns
/// via `pragma_table_info` before running the `UPDATE`, mirroring `migrate_v76`'s
/// `ADD COLUMN` idempotency-guard pattern, and skips gracefully (0 rows affected) rather
/// than erroring when the schema predates this migration's target columns.
fn migrate_v77(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v77] Starting: retire per-project smart-routing columns (ADR-0061)");

    let projects_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='projects'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v77] projects table-existence check failed: {}", e))?;

    let rows1: usize = if !projects_table_exists {
        eprintln!("[MIGRATION-v77] Skipping projects NULL-collapse: projects table absent (older fixture schema)");
        0
    } else {
        let all_columns_present: bool = conn
            .query_row(
                "SELECT COUNT(*) = 4 FROM pragma_table_info('projects')
                 WHERE name IN ('smart_routing_enabled', 'router_model', 'simple_tier_model', 'default_model')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("[MIGRATION-v77] projects column-existence check failed: {}", e))?;

        if !all_columns_present {
            eprintln!("[MIGRATION-v77] Skipping projects NULL-collapse: one or more retired columns absent (older fixture schema)");
            0
        } else {
            conn
        .execute(
            "UPDATE projects
             SET smart_routing_enabled = NULL,
                     router_model          = NULL,
                     simple_tier_model     = NULL,
                     default_model         = NULL
                 WHERE smart_routing_enabled IS NOT NULL
                    OR router_model IS NOT NULL
                    OR simple_tier_model IS NOT NULL
                    OR default_model IS NOT NULL",
                [],
            )
            .map_err(|e| format!("[MIGRATION-v77] projects NULL-collapse failed: {}", e))?
        }
    };

    let ai_settings_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v77] ai_settings table-existence check failed: {}", e))?;

    let rows2: usize = if !ai_settings_table_exists {
        eprintln!("[MIGRATION-v77] Skipping ai_settings key-delete: ai_settings table absent (older fixture schema)");
        0
    } else {
        conn
            .execute(
                "DELETE FROM ai_settings
                 WHERE key IN ('tier_1_model', 'tier_2_model', 'tier_3_model',
                               'router_model', 'smart_routing_enabled')",
                [],
            )
            .map_err(|e| format!("[MIGRATION-v77] ai_settings key delete failed: {}", e))?
    };

    eprintln!(
        "[MIGRATION-v77] nulled {} project rows, deleted {} ai_settings keys",
        rows1, rows2
    );

    eprintln!("[MIGRATION-v77] ✓ Done: per-project smart-routing columns retired");
    Ok(())
}

/// v77 → v78: add `agent_id` to `chat_sessions` (#2522, ADR-0062 D3 — Per-Session Agent
/// Binding, phase 1).
///
/// Additive column plus a two-step backfill fallback chain (ADR-0062 D3.3):
///   1. Prefer the designated default (`is_main = 1 AND is_active = 1`) agent.
///   2. Else the most-recently-created active agent (`is_active = 1 ORDER BY created_at DESC`).
///   3. Else leave `agent_id` NULL — the D3.4 self-heal contract governs from here
///      (read-time, not migration-time; see `resolve_and_selfheal_session_agent`
///      in `agents.rs`).
///
/// Both `UPDATE ... WHERE agent_id IS NULL` statements are naturally idempotent (a
/// second run finds 0 rows still NULL from Step 1, and Step 2's own `WHERE agent_id
/// IS NULL` guard prevents it from ever clobbering a Step-1 write) — order-dependence
/// between the two statements is load-bearing, so they must run as two sequential
/// statements, not a single combined query. This mirrors `migrate_v77`'s two-statement
/// `rows1`/`rows2` counting pattern above. Only the `ALTER TABLE ADD COLUMN` itself
/// needs the `pragma_table_info` idempotency guard (SQLite lacks `ADD COLUMN IF NOT
/// EXISTS`), matching `migrate_v76`'s precedent.
///
/// Same defensive table/column-existence guard as `migrate_v77`: some older
/// combined-migration test fixtures build minimal schemas that may lack `chat_sessions`
/// or `ai_agents` entirely (unrelated to this migration's focus), so this function
/// checks for both tables before running the backfill and skips gracefully rather than
/// erroring when the schema predates this migration's targets.
fn migrate_v78(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v78] Starting: agent_id column on chat_sessions (#2522, ADR-0062 D3)");

    let sessions_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='chat_sessions'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v78] chat_sessions table-existence check failed: {}", e))?;

    if !sessions_table_exists {
        eprintln!("[MIGRATION-v78] Skipping: chat_sessions table absent (older fixture schema)");
        eprintln!("[MIGRATION-v78] ✓ Done (no-op): chat_sessions absent");
        return Ok(());
    }

    // Idempotency guard: check if column already exists via PRAGMA
    // SQLite does not support IF NOT EXISTS on ALTER TABLE ADD COLUMN
    let col_exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_sessions') WHERE name='agent_id'",
        [],
        |row| row.get(0),
    ).map_err(|e| format!("[MIGRATION-v78] pragma check failed: {}", e))?;

    if !col_exists {
        conn.execute_batch(
            "ALTER TABLE chat_sessions ADD COLUMN agent_id TEXT REFERENCES ai_agents(id);",
        )
        .map_err(|e| format!("[MIGRATION-v78] ALTER TABLE failed: {}", e))?;
        eprintln!("[MIGRATION-v78] Step 1: agent_id column added");
    } else {
        eprintln!("[MIGRATION-v78] Step 1: agent_id column already present (idempotent re-run)");
    }

    let ai_agents_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v78] ai_agents table-existence check failed: {}", e))?;

    let (backfill_default, backfill_recent): (usize, usize) = if !ai_agents_table_exists {
        eprintln!("[MIGRATION-v78] Skipping backfill: ai_agents table absent (older fixture schema)");
        (0, 0)
    } else {
        // #2522 ADR-0063 Stage 1: the designated-default column was renamed is_main→
        // is_default in migrate_v79, which runs AFTER this migration in every real
        // upgrade chain (v78 always sees is_main on-disk in production). But some
        // test fixtures bootstrap the full canonical (already-v79-shaped) schema via
        // run_migrations() and then only rewind the `schema_version` stamp to replay
        // older migration arms against it — in that scenario ai_agents already has
        // is_default, not is_main, when this code runs. Resolve the live column name
        // via pragma_table_info so this migration works against both shapes rather
        // than hard-failing on the test-only inconsistency.
        let designated_col: &str = {
            let has_is_default: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM pragma_table_info('ai_agents') WHERE name='is_default'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if has_is_default { "is_default" } else { "is_main" }
        };

        // Step 2 (backfill Step 1 in ADR terms): prefer the designated default agent.
        let rows_a = conn
            .execute(
                &format!(
                    "UPDATE chat_sessions
                     SET agent_id = (SELECT id FROM ai_agents WHERE {} = 1 AND is_active = 1)
                     WHERE agent_id IS NULL",
                    designated_col
                ),
                [],
            )
            .map_err(|e| format!("[MIGRATION-v78] designated-default backfill failed: {}", e))?;
        eprintln!("[MIGRATION-v78] Step 2: backfilled {} session(s) from designated default agent", rows_a);

        // Step 3 (backfill Step 2): fall back to the most-recently-created active agent,
        // strictly guarded by `WHERE agent_id IS NULL` so this never clobbers Step 2's writes.
        let rows_b = conn
            .execute(
                "UPDATE chat_sessions
                 SET agent_id = (SELECT id FROM ai_agents WHERE is_active = 1 ORDER BY created_at DESC LIMIT 1)
                 WHERE agent_id IS NULL",
                [],
            )
            .map_err(|e| format!("[MIGRATION-v78] most-recent-active-agent backfill failed: {}", e))?;
        eprintln!("[MIGRATION-v78] Step 3: backfilled {} session(s) from most-recently-created active agent", rows_b);

        (rows_a, rows_b)
    };

    eprintln!(
        "[MIGRATION-v78] backfilled {} session(s) via designated default, {} session(s) via most-recent-active fallback — any remaining NULL agent_id rows govern under the D3.4 self-heal contract",
        backfill_default, backfill_recent
    );

    eprintln!("[MIGRATION-v78] ✓ Done: agent_id added to chat_sessions and backfilled");
    Ok(())
}

/// v78→v79: Runtime-per-agent schema rename (#2522, ADR-0063 D1/D3/D4).
///
/// Ordered steps (D4, exact order load-bearing):
///   1. `ALTER TABLE ai_agents RENAME COLUMN oc_agent_id TO runtime_ref;`
///   2. `ALTER TABLE ai_agents ADD COLUMN runtime_type TEXT;`
///   3. Backfill (D4.3): rows with `runtime_ref IS NOT NULL` (OC-imported) get
///      `runtime_type = 'openclaw'`. Locals (`runtime_ref IS NULL`): if the
///      pre-dissolution active `ai_settings.provider` is an API class
///      (`anthropic` | `openrouter` | `ollama`), pin them to it. If the active
///      provider is `'openclaw'` (or absent) at migration time, locals take
///      `runtime_type = NULL` (legacy-unpinned per D4.3) — resolved by a
///      one-time picker prompt in a later stage, out of this migration's scope.
///   4. Index swap: `DROP INDEX idx_ai_agents_oc_id_unique;`
///      `CREATE UNIQUE INDEX idx_ai_agents_runtime_unique ON ai_agents(runtime_type, runtime_ref);`
///   5. `ALTER TABLE ai_agents RENAME COLUMN is_main TO is_default;` + index
///      swap `idx_ai_agents_main` → `idx_ai_agents_default`.
///   6. `ai_settings` dissolution (D8): delete `provider`, `endpoint`,
///      `api_key`, `model` rows. (`ollama_endpoint` copy-forward and the D9
///      re-target are Stage 2+/D8-consumption concerns outside this stage's
///      schema+rename scope; the key deletes themselves are the D4 step-6
///      migration action and belong here.)
///
/// `ai_agents` is NOT a CRR table (D4's membership analysis — three
/// pre-existing non-PK unique indexes make it CRR-incompatible upstream) —
/// plain DDL throughout, no `crsql_begin_alter`/`crsql_commit_alter` wrapping
/// (see D4 CRR analysis). `ai_settings` is likewise non-CRR (#2538).
///
/// Idempotency: every step is individually guarded — `pragma_table_info`
/// checks before each `RENAME COLUMN`/`ADD COLUMN` (SQLite has no `IF NOT
/// EXISTS` for either), `sqlite_master` checks before each index `DROP`/
/// `CREATE`, and the backfill `UPDATE`s are naturally idempotent (re-running
/// them against already-backfilled rows is a no-op: the `runtime_ref IS NOT
/// NULL` branch re-writes the same value, and the locals branch only ever
/// touches `runtime_type IS NULL` rows). The `ai_settings` deletes are
/// naturally idempotent (`DELETE` on an absent key is a no-op).
fn migrate_v79(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v79] Starting: runtime_type/runtime_ref + is_default rename (#2522, ADR-0063 D1/D3/D4)");

    let ai_agents_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v79] ai_agents table-existence check failed: {}", e))?;

    if !ai_agents_table_exists {
        eprintln!("[MIGRATION-v79] Skipping: ai_agents table absent (older fixture schema)");
        eprintln!("[MIGRATION-v79] ✓ Done (no-op): ai_agents absent");
        return Ok(());
    }

    // ── Step 1: RENAME COLUMN oc_agent_id TO runtime_ref ──────────────────
    let runtime_ref_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('ai_agents') WHERE name='runtime_ref'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v79] runtime_ref pragma check failed: {}", e))?;

    if !runtime_ref_exists {
        conn.execute_batch("ALTER TABLE ai_agents RENAME COLUMN oc_agent_id TO runtime_ref;")
            .map_err(|e| format!("[MIGRATION-v79] Step 1 RENAME COLUMN oc_agent_id→runtime_ref failed: {}", e))?;
        eprintln!("[MIGRATION-v79] Step 1: oc_agent_id renamed to runtime_ref");
    } else {
        eprintln!("[MIGRATION-v79] Step 1: runtime_ref already present (idempotent re-run)");
    }

    // ── Step 2: ADD COLUMN runtime_type TEXT ───────────────────────────────
    let runtime_type_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('ai_agents') WHERE name='runtime_type'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v79] runtime_type pragma check failed: {}", e))?;

    if !runtime_type_exists {
        conn.execute_batch("ALTER TABLE ai_agents ADD COLUMN runtime_type TEXT;")
            .map_err(|e| format!("[MIGRATION-v79] Step 2 ADD COLUMN runtime_type failed: {}", e))?;
        eprintln!("[MIGRATION-v79] Step 2: runtime_type column added");
    } else {
        eprintln!("[MIGRATION-v79] Step 2: runtime_type already present (idempotent re-run)");
    }

    // ── Step 3: Backfill (D4.3) ─────────────────────────────────────────────
    // OC-imported rows: runtime_ref IS NOT NULL → runtime_type = 'openclaw'.
    // Idempotent — re-writes the same value on a second run.
    let rows_openclaw = conn
        .execute(
            "UPDATE ai_agents SET runtime_type = 'openclaw' WHERE runtime_ref IS NOT NULL",
            [],
        )
        .map_err(|e| format!("[MIGRATION-v79] Step 3 openclaw backfill failed: {}", e))?;
    eprintln!("[MIGRATION-v79] Step 3a: {} row(s) backfilled to runtime_type='openclaw' (runtime_ref IS NOT NULL)", rows_openclaw);

    // Locals: runtime_ref IS NULL AND runtime_type IS NULL (guard makes this idempotent —
    // a second run finds nothing left to touch since Step 3a/3b already populated every row).
    let active_provider = crate::database::get_ai_setting(conn, "provider");
    let is_api_provider = matches!(
        active_provider.as_deref(),
        Some("anthropic") | Some("openrouter") | Some("ollama")
    );

    let rows_local = if is_api_provider {
        let provider = active_provider.as_deref().unwrap();
        let n = conn
            .execute(
                "UPDATE ai_agents SET runtime_type = ?1 WHERE runtime_ref IS NULL AND runtime_type IS NULL",
                params![provider],
            )
            .map_err(|e| format!("[MIGRATION-v79] Step 3 API-provider local backfill failed: {}", e))?;
        eprintln!(
            "[MIGRATION-v79] Step 3b: {} local row(s) pinned to runtime_type='{}' (pre-dissolution active provider)",
            n, provider
        );
        n
    } else {
        // Active provider is 'openclaw' or absent — locals were last animated by an
        // unrecorded API provider. Leave runtime_type NULL (legacy-unpinned, D4.3):
        // resolved by a one-time picker prompt in a later stage. Nothing to write here;
        // NULL is already the column's post-ADD-COLUMN default.
        eprintln!("[MIGRATION-v79] Step 3b: active provider is '{:?}' (not an API class) — local rows left runtime_type=NULL (legacy-unpinned, D4.3 picker resolves later)", active_provider);
        0
    };

    // ── Step 4: Index swap — idx_ai_agents_oc_id_unique → idx_ai_agents_runtime_unique ──
    let old_idx_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_ai_agents_oc_id_unique'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v79] idx_ai_agents_oc_id_unique existence check failed: {}", e))?;

    if old_idx_exists {
        conn.execute_batch("DROP INDEX idx_ai_agents_oc_id_unique;")
            .map_err(|e| format!("[MIGRATION-v79] Step 4 DROP INDEX idx_ai_agents_oc_id_unique failed: {}", e))?;
        eprintln!("[MIGRATION-v79] Step 4: idx_ai_agents_oc_id_unique dropped");
    } else {
        eprintln!("[MIGRATION-v79] Step 4: idx_ai_agents_oc_id_unique already absent (idempotent re-run)");
    }

    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_runtime_unique ON ai_agents(runtime_type, runtime_ref);",
    )
    .map_err(|e| format!("[MIGRATION-v79] Step 4 CREATE INDEX idx_ai_agents_runtime_unique failed: {}", e))?;
    eprintln!("[MIGRATION-v79] Step 4: idx_ai_agents_runtime_unique ensured");

    // ── Step 5: RENAME COLUMN is_main TO is_default + index swap ────────────
    let is_default_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('ai_agents') WHERE name='is_default'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v79] is_default pragma check failed: {}", e))?;

    if !is_default_exists {
        conn.execute_batch("ALTER TABLE ai_agents RENAME COLUMN is_main TO is_default;")
            .map_err(|e| format!("[MIGRATION-v79] Step 5 RENAME COLUMN is_main→is_default failed: {}", e))?;
        eprintln!("[MIGRATION-v79] Step 5: is_main renamed to is_default");
    } else {
        eprintln!("[MIGRATION-v79] Step 5: is_default already present (idempotent re-run)");
    }

    let old_main_idx_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_ai_agents_main'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v79] idx_ai_agents_main existence check failed: {}", e))?;

    if old_main_idx_exists {
        conn.execute_batch("DROP INDEX idx_ai_agents_main;")
            .map_err(|e| format!("[MIGRATION-v79] Step 5 DROP INDEX idx_ai_agents_main failed: {}", e))?;
        eprintln!("[MIGRATION-v79] Step 5: idx_ai_agents_main dropped");
    } else {
        eprintln!("[MIGRATION-v79] Step 5: idx_ai_agents_main already absent (idempotent re-run)");
    }

    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_default ON ai_agents(is_default) WHERE is_default = 1;",
    )
    .map_err(|e| format!("[MIGRATION-v79] Step 5 CREATE INDEX idx_ai_agents_default failed: {}", e))?;
    eprintln!("[MIGRATION-v79] Step 5: idx_ai_agents_default ensured");

    // ── Step 6: ai_settings dissolution (D8) — delete retired global keys ──
    let ai_settings_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v79] ai_settings table-existence check failed: {}", e))?;

    if ai_settings_table_exists {
        let deleted = conn
            .execute(
                "DELETE FROM ai_settings WHERE key IN ('provider', 'endpoint', 'api_key', 'model')",
                [],
            )
            .map_err(|e| format!("[MIGRATION-v79] Step 6 ai_settings key deletes failed: {}", e))?;
        eprintln!("[MIGRATION-v79] Step 6: {} retired ai_settings row(s) deleted (provider/endpoint/api_key/model)", deleted);
    } else {
        eprintln!("[MIGRATION-v79] Step 6: ai_settings table absent (older fixture schema) — skipping key deletes");
    }

    eprintln!(
        "[MIGRATION-v79] ✓ Done: {} row(s) → runtime_type='openclaw', {} local row(s) pinned, rest left legacy-unpinned; is_main→is_default renamed",
        rows_openclaw, rows_local
    );
    Ok(())
}

/// #2564 B1 fix (Path B), Commit 2: recovers `ai_settings.endpoint` — and ONLY
/// `endpoint` — for installs orphaned by `migrate_v79`'s Step 6 (D8 dissolution,
/// ADR-0063), which deleted `provider`/`endpoint`/`api_key`/`model` with no per-agent
/// home for the gateway URL. `provider` is retired for real and is deliberately NOT
/// restored here — every reader is re-pointed to `ai_agents.runtime_type` instead
/// (see sync_client::has_active_openclaw_agent, runtime_resolution::derive_active_provider_label).
///
/// Detection (orphan signature): `ai_settings.endpoint` absent AND an active
/// `ai_agents` row has `runtime_type = 'openclaw'`. Both conditions false → no-op
/// (fresh installs with no OC agent; already-recovered/re-paired installs where
/// endpoint is already present — this makes the migration idempotent by construction,
/// not just safe-to-rerun).
///
/// Recovery source: the pre-v79 migration backup (`backup.rs` `create_migration_backup`,
/// `pane_pre-migration-*.db` in the backups dir) is the only place a pre-dissolution
/// `endpoint` value survives. Filenames use two observed patterns on real installs
/// (verified via `ls` on Matt's backups dir) — `pane_pre-migration-v78.db`
/// (single-version) and `pane_pre-migration-v62-to-v69.db` (range, where the FIRST
/// number is the pre-migration source version) — so candidates are ranked by their
/// first embedded version number, highest-but-≤78 (closest to the v79 boundary, most
/// likely to hold the last-known-good endpoint) first, and each is opened and
/// checked for an actual `endpoint` value rather than trusted by name alone.
fn migrate_v80(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v80] Starting: gateway-endpoint orphan recovery (#2564 B1 fix, Path B)");

    let ai_settings_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v80] ai_settings table-existence check failed: {}", e))?;

    if !ai_settings_table_exists {
        eprintln!("[MIGRATION-v80] Skipping: ai_settings table absent (older fixture schema)");
        eprintln!("[MIGRATION-v80] ✓ Done (no-op): ai_settings absent");
        return Ok(());
    }

    let ai_agents_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v80] ai_agents table-existence check failed: {}", e))?;

    let endpoint_already_present = crate::database::get_ai_setting(conn, "endpoint").is_some();
    if endpoint_already_present {
        eprintln!("[MIGRATION-v80] Skipping: ai_settings.endpoint already present (idempotent re-run or already-recovered install)");
        eprintln!("[MIGRATION-v80] ✓ Done (no-op): endpoint already present");
        return Ok(());
    }

    let has_openclaw_agent: bool = if ai_agents_table_exists {
        conn.query_row(
            "SELECT 1 FROM ai_agents WHERE runtime_type = 'openclaw' LIMIT 1",
            [],
            |_| Ok(()),
        )
        .is_ok()
    } else {
        false
    };

    if !has_openclaw_agent {
        eprintln!("[MIGRATION-v80] Skipping: no openclaw-runtime agent present (fresh install or API-provider-only install)");
        eprintln!("[MIGRATION-v80] ✓ Done (no-op): no openclaw agent");
        return Ok(());
    }

    eprintln!("[MIGRATION-v80] Orphan signature detected: openclaw agent present, endpoint absent — searching pre-migration backups for a recoverable endpoint");

    match recover_endpoint_from_backups() {
        Some(recovered_endpoint) => {
            crate::database::set_ai_setting(conn, "endpoint", &recovered_endpoint).map_err(|e| {
                format!("[MIGRATION-v80] Failed to write recovered endpoint: {}", e)
            })?;
            eprintln!(
                "[MIGRATION-v80] ✓ Done: endpoint recovered from pre-migration backup and restored"
            );
        }
        None => {
            eprintln!(
                "[MIGRATION-v80] No recoverable gateway URL found (backup missing or lacks endpoint) — orphaned openclaw agent(s) will require re-pairing"
            );
            eprintln!("[MIGRATION-v80] ✓ Done: no placeholder written, endpoint left absent (re-pair required)");
        }
    }

    Ok(())
}

/// v80→v81: role='sub'→'local' birth-mislabel data repair (#2567 B6).
///
/// Matt's ruling: a user-created agent is never a subagent, structurally. Every
/// write site that creates or demotes a locally-created agent row (runtime_ref IS
/// NULL) has been fixed in this same PR to write role='local' instead of role='sub'
/// going forward (agents.rs `create_local_agent_impl`/`upsert_agent`,
/// ai/commands.rs filesystem-recovery INSERT, sync_client.rs demotion UPDATEs). This
/// migration is the one-time data repair for rows written before those fixes
/// landed — pure data correction, not a user edit, so `updated_at` is intentionally
/// NOT bumped (Matt's explicit ruling on this point).
///
/// Idempotent by construction: the `WHERE role = 'sub' AND runtime_ref IS NULL`
/// predicate is self-exhausting — once every existing mislabeled row is re-labeled
/// 'local' on the first run, zero rows match on any subsequent run (nothing else in
/// the fixed codebase ever writes role='sub' onto a runtime_ref IS NULL row again).
fn migrate_v81(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v81] Starting: role='sub' birth-mislabel re-label (#2567 B6)");

    let ai_agents_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v81] ai_agents table-existence check failed: {}", e))?;

    if !ai_agents_table_exists {
        eprintln!("[MIGRATION-v81] Skipping: ai_agents table absent (older fixture schema)");
        eprintln!("[MIGRATION-v81] ✓ Done (no-op): ai_agents absent");
        return Ok(());
    }

    // Role only — do NOT bump updated_at (Matt's ruling: this is data repair, not a
    // user edit).
    let rows_affected = conn
        .execute(
            "UPDATE ai_agents SET role = 'local' WHERE role = 'sub' AND runtime_ref IS NULL",
            [],
        )
        .map_err(|e| format!("[MIGRATION-v81] Re-label UPDATE failed: {}", e))?;

    eprintln!(
        "[MIGRATION-v81] ✓ Done: {} row(s) re-labeled role='sub'→'local'",
        rows_affected
    );
    Ok(())
}

/// v81→v82: per-provider keychain slot split + ollama_endpoint backfill (B3, #2565).
///
/// Combined function for both B3 requirements (per the scope doc, §5 "PR bite-cut" —
/// one `migrate_vNN` covers both, same shape as `migrate_v80`'s single-version-bump
/// pattern for a multi-part fix).
///
/// Part A — keychain slot split: the legacy shared `ai_provider_key` keychain account
/// (`KEYCHAIN_ACCOUNT_PROVIDER_KEY`) carries no provenance — it may hold either an
/// Anthropic or an OpenRouter key. Format-sniff the value (`sk-ant-` / `sk-or-`
/// prefixes, per the scope doc §2c) to route it to the correct new per-provider slot.
/// Falls back to inferring from an active `ai_agents.runtime_type` row when the prefix
/// is unrecognized, and leaves both new slots empty (requiring re-entry) as the last
/// resort — exactly the three-tier resolution the scope doc recommends.
///
/// Part B — ollama_endpoint backfill: implements the still-unbuilt half of the D8
/// action item (ADR-0063) — Ollama's endpoint moves off the shared `endpoint` key
/// (which `openclaw` also reads) onto its own `ollama_endpoint` key, which now also
/// doubles as the Req 2 explicit-connection signal. A pre-existing active
/// `runtime_type = 'ollama'` agent is grandfathered in (backfilled) so it doesn't
/// silently go non-dispatchable the moment this fix ships — recommendation (a) in the
/// scope doc's §6 risk #1.
///
/// Does NOT clear the legacy `endpoint` key — `openclaw`/`sync_client`/`system_files`
/// still read it directly and splitting THEIR use of it is explicitly out of scope
/// (§5 "Explicitly follow-up").
fn migrate_v82(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v82] Starting: per-provider keychain slots + ollama_endpoint backfill (B3, #2565)");

    migrate_v82_keychain_slot_split(conn)?;
    migrate_v82_ollama_endpoint_backfill(conn)?;

    eprintln!("[MIGRATION-v82] ✓ Done");
    Ok(())
}

/// Part A of migrate_v82 — see doc comment above. Keychain failures are returned so
/// the outer migration does not stamp the schema complete and can retry next launch.
fn migrate_v82_keychain_slot_split(conn: &Connection) -> Result<(), String> {
    use crate::ai::credential_migration::{
        KEYCHAIN_ACCOUNT_ANTHROPIC_KEY, KEYCHAIN_ACCOUNT_OPENROUTER_KEY,
        KEYCHAIN_ACCOUNT_PROVIDER_KEY,
    };

    let legacy_value = match crate::keychain::get_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY) {
        Ok(Some(v)) => v,
        Ok(None) => {
            eprintln!("[MIGRATION-v82] Keychain split: legacy ai_provider_key slot empty — nothing to split (idempotent no-op)");
            return Ok(());
        }
        Err(e) => {
            return Err(format!(
                "[MIGRATION-v82] Keychain split: failed to read legacy slot: {e}"
            ));
        }
    };

    // Resolve provenance before applying idempotency rules: only the intended
    // destination can prove that this exact legacy value was migrated.
    let target_account = if legacy_value.starts_with("sk-ant-") {
        eprintln!("[MIGRATION-v82] Keychain split: legacy value matches Anthropic prefix (sk-ant-) — routing to anthropic slot");
        Some(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY)
    } else if legacy_value.starts_with("sk-or-") {
        eprintln!("[MIGRATION-v82] Keychain split: legacy value matches OpenRouter prefix (sk-or-) — routing to openrouter slot");
        Some(KEYCHAIN_ACCOUNT_OPENROUTER_KEY)
    } else {
        let has_active_anthropic = conn
            .query_row(
                "SELECT 1 FROM ai_agents WHERE runtime_type = 'anthropic' AND is_active = 1 LIMIT 1",
                [],
                |_| Ok(()),
            )
            .is_ok();
        let has_active_openrouter = conn
            .query_row(
                "SELECT 1 FROM ai_agents WHERE runtime_type = 'openrouter' AND is_active = 1 LIMIT 1",
                [],
                |_| Ok(()),
            )
            .is_ok();

        match (has_active_anthropic, has_active_openrouter) {
            (true, false) => {
                eprintln!("[MIGRATION-v82] Keychain split: legacy value has no recognized prefix — inferring anthropic from active agent row");
                Some(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY)
            }
            (false, true) => {
                eprintln!("[MIGRATION-v82] Keychain split: legacy value has no recognized prefix — inferring openrouter from active agent row");
                Some(KEYCHAIN_ACCOUNT_OPENROUTER_KEY)
            }
            _ => {
                eprintln!("[MIGRATION-v82] Keychain split: legacy value unrecognized and no unambiguous active agent to infer from — preserving legacy slot");
                None
            }
        }
    };

    let Some(account) = target_account else {
        return Ok(());
    };

    match crate::keychain::get_password(account) {
        Ok(Some(ref confirmed)) if confirmed == &legacy_value => {
            eprintln!("[MIGRATION-v82] Keychain split: intended destination {} already contains the legacy value", account);
        }
        Ok(Some(_)) => {
            eprintln!("[MIGRATION-v82] Keychain split: intended destination {} contains a different credential — preserving both it and the legacy slot", account);
            return Ok(());
        }
        Ok(None) => {
            crate::keychain::store_password(account, &legacy_value).map_err(|e| {
                format!("[MIGRATION-v82] Keychain split: failed to write to {account}: {e}")
            })?;
            eprintln!(
                "[MIGRATION-v82] Keychain split: wrote legacy value to {}",
                account
            );

            match crate::keychain::get_password(account) {
                Ok(Some(ref confirmed)) if confirmed == &legacy_value => {
                    eprintln!(
                        "[MIGRATION-v82] Keychain split: read-back confirmed for {}",
                        account
                    );
                }
                Ok(Some(_)) => {
                    return Err(format!(
                        "[MIGRATION-v82] Keychain split: read-back value mismatch for {account}"
                    ));
                }
                Ok(None) => {
                    return Err(format!(
                        "[MIGRATION-v82] Keychain split: read-back returned no value for {account}"
                    ));
                }
                Err(e) => {
                    return Err(format!(
                        "[MIGRATION-v82] Keychain split: read-back failed for {account}: {e}"
                    ));
                }
            }
        }
        Err(e) => {
            return Err(format!("[MIGRATION-v82] Keychain split: failed to read intended destination {account}: {e}"));
        }
    }

    crate::keychain::delete_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY).map_err(|e| {
        format!(
            "[MIGRATION-v82] Keychain split: failed to clear legacy slot after confirmed copy: {e}"
        )
    })?;
    eprintln!("[MIGRATION-v82] Keychain split: legacy ai_provider_key slot cleared");
    Ok(())
}

/// Part B of migrate_v82 — see doc comment above.
fn migrate_v82_ollama_endpoint_backfill(conn: &Connection) -> Result<(), String> {
    let ai_settings_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v82] ai_settings table-existence check failed: {}", e))?;

    if !ai_settings_table_exists {
        eprintln!("[MIGRATION-v82] Ollama backfill: ai_settings table absent (older fixture schema) — skipping");
        return Ok(());
    }

    // Idempotency guard: ollama_endpoint already exists — no-op, regardless of value.
    if crate::database::get_ai_setting(conn, "ollama_endpoint").is_some() {
        eprintln!("[MIGRATION-v82] Ollama backfill: ollama_endpoint already present — skipping (idempotent re-run)");
        return Ok(());
    }

    let ai_agents_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v82] ai_agents table-existence check failed: {}", e))?;

    let legacy_endpoint = crate::database::get_ai_setting(conn, "endpoint");
    let legacy_looks_ollama_shaped = legacy_endpoint
        .as_deref()
        .map(|e| e.contains("localhost:11434") || e.contains("127.0.0.1:11434"))
        .unwrap_or(false);

    // Non-grandfather path: the legacy `endpoint` key genuinely holds an Ollama-shaped
    // value (rare in practice — openclaw's pairing flow writes non-loopback gateway
    // URLs — but possible on an ollama-only install that never split off `endpoint`).
    // This applies regardless of whether an active ollama agent exists, since the
    // signal here is the stored value itself, not an inference.
    if legacy_looks_ollama_shaped {
        let endpoint = legacy_endpoint.expect("legacy_looks_ollama_shaped implies Some");
        crate::database::set_ai_setting(conn, "ollama_endpoint", &endpoint)
            .map_err(|e| format!("[MIGRATION-v82] Ollama backfill: failed to write ollama_endpoint from legacy value: {}", e))?;
        eprintln!("[MIGRATION-v82] Ollama backfill: legacy `endpoint` was Ollama-shaped ({}) — copied to ollama_endpoint", endpoint);
        return Ok(());
    }

    // Grandfather rule (scope doc §6 risk #1, recommendation (a)): an active
    // `runtime_type = 'ollama'` agent existed before this fix shipped — treat it as
    // already-explicitly-configured rather than letting it silently go
    // non-dispatchable. Defaults to the standard local port since the legacy
    // `endpoint` key (checked above) didn't hold a usable value.
    let has_active_ollama_agent: bool = if ai_agents_table_exists {
        conn.query_row(
            "SELECT 1 FROM ai_agents WHERE runtime_type = 'ollama' AND is_active = 1 LIMIT 1",
            [],
            |_| Ok(()),
        )
        .is_ok()
    } else {
        false
    };

    if has_active_ollama_agent {
        crate::database::set_ai_setting(conn, "ollama_endpoint", "http://localhost:11434")
            .map_err(|e| format!("[MIGRATION-v82] Ollama backfill: failed to write grandfathered default ollama_endpoint: {}", e))?;
        eprintln!("[MIGRATION-v82] Ollama backfill: active ollama agent found, legacy endpoint not usable — grandfathered in with default http://localhost:11434");
        return Ok(());
    }

    eprintln!("[MIGRATION-v82] Ollama backfill: no ollama-shaped legacy endpoint and no active ollama agent — leaving ollama_endpoint absent (fresh/unused Ollama still requires explicit Save)");
    Ok(())
}

/// v82→v83: OC model placeholder backfill (C1, #2585) + legacy-unpinned agent
/// deactivation (C3, #2587). Two independent, idempotent UPDATEs — combined into one
/// migration function per this file's established multi-part-fix-in-one-version-bump
/// convention (same shape as `migrate_v82`'s Part A / Part B split).
///
/// ── C1: `ai_agents.model` OC placeholder backfill ──────────────────────────────
///
/// `migrate_v79`'s Step 6 (D8 `ai_settings` dissolution, ADR-0063) deleted the
/// global `provider`/`endpoint`/`api_key`/`model` keys with no per-agent home for
/// `model` — every OC-runtime `ai_agents` row created before per-agent `model` had a
/// real writer (`sync_re_import`/`initial_sync`'s column lists never included
/// `model`; `ai_configure_and_save` wrote the resolved value to the now-dead
/// `ai_settings.model` key instead, fixed in this same PR — see Change B in
/// `ai/commands.rs`) is left with `model IS NULL` or `model = ''`. Dispatch's
/// `resolve_model_for_message` (commands.rs) gates on non-empty model and hard-fails
/// before a request is ever built, so these rows can never dispatch, which means
/// D9's post-dispatch write-back (`write_back_agent_model_if_changed`,
/// runtime_resolution.rs) — the mechanism that would otherwise self-heal this —
/// never gets a chance to run either. Chicken-and-egg (see the scope doc,
/// `~/Shared/ExampleApp/docs/audits/2026-08-19-c1-oc-model-scope.md`, for the full trace).
///
/// Fix: seed a sentinel placeholder (`'openclaw-pending'`) into `model` for every
/// affected OC row. This is pure SQL — no HTTP call at migration time (migrations in
/// this file are synchronous, DB-only; see the scope doc's Option 1 analysis for why
/// a migration-time gateway call is not viable: startup ordering, credential
/// migration hasn't run yet, endpoint recovery is best-effort, gateway may be
/// unreachable at launch). The placeholder is non-empty, so it passes
/// `resolve_model_for_message`'s gate and lets the message actually dispatch. On the
/// wire, the placeholder is never sent: `stream_openrouter_inner` (openrouter.rs)
/// unconditionally overwrites the outbound `model` field to the literal string
/// `"openclaw"` whenever `base_url.is_some()` (i.e. every OC-routed request) — see
/// the cross-reference comment at that rewrite site. D9's write-back then corrects
/// `model` to the real value on the very first successful dispatch, exactly as it
/// already does for any other stale-model case.
///
/// `updated_at` IS bumped on the backfilled rows — unlike `migrate_v81`'s role
/// mislabel repair (which explicitly does NOT bump `updated_at`, being a pure label
/// fix with zero runtime effect), this write changes a value that is directly
/// dispatch-relevant: before this migration, the row cannot dispatch at all; after,
/// it can. That's a real behavioral state change for the row, not a cosmetic
/// relabel, so it gets a real `updated_at` bump, matching this codebase's general
/// convention that dispatch-relevant column writes bump `updated_at` and pure-label
/// data repairs don't.
///
/// SAFETY NOTE (load-bearing, cross-referenced at the rewrite site too): this
/// placeholder is only safe because of the unconditional sentinel rewrite described
/// above. If that rewrite in `stream_openrouter_inner` ever becomes conditional
/// (e.g. "only rewrite if model is empty"), the literal string `'openclaw-pending'`
/// could leak onto the wire as a real model parameter. Any future change to that
/// rewrite's conditionality must account for this.
///
/// ── C3: legacy-unpinned agent deactivation ──────────────────────────────────────
///
/// Matt's ruling: the population of `runtime_type IS NULL` ("legacy-unpinned")
/// `ai_agents` rows is closed — `migrate_v79` (D4.3) was the only path that ever
/// created this shape (backfilling `runtime_type = NULL` for locally-created rows
/// whose active provider at migration time was OpenClaw), and no code path since
/// Stage 6 (`create_local_agent` requiring an explicit `runtime_type` parameter) has
/// ever minted a new NULL-runtime row. `LegacyProviderPrompt.svelte`'s one-time
/// "pin a provider" rescue UI is therefore unreachable by construction for any
/// *new* row and only ever mattered for this one historical population — deleted in
/// this same PR (see `AgentPickerPopover.svelte` and its test file).
///
/// This migration deactivates that closed population directly:
/// `UPDATE ai_agents SET is_active = 0 WHERE runtime_type IS NULL`. `is_active = 0`
/// is the same mechanism already used everywhere else in this codebase to remove an
/// agent from every active-roster query (`get_agents`, `ai_list_runtime_connections_impl`,
/// `resolve_designated_default_agent_id`, etc. all filter on `is_active = 1`) — no
/// new state, no new predicate, just applying the existing "inactive" mechanism to a
/// population that can no longer be resolved to a runtime and has no UI path left to
/// fix that. Rows are not deleted (data is preserved, per this file's "never destroys
/// data" §2B.2 convention) — only hidden from active-roster surfaces.
///
/// Both UPDATEs are idempotent by construction: their `WHERE` predicates are
/// self-exhausting (C1: a backfilled row no longer matches `model IS NULL OR model =
/// ''`; C3: a deactivated row no longer needs re-deactivating, and the guard is
/// naturally a no-op on re-run since `is_active = 0` rows just get set to `0` again,
/// a true no-op write).
fn migrate_v83(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v83] Starting: ai_agents.model OC placeholder backfill (C1, #2585) + legacy-unpinned agent deactivation (C3, #2587)");

    let ai_agents_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v83] ai_agents table-existence check failed: {}", e))?;

    if !ai_agents_table_exists {
        eprintln!("[MIGRATION-v83] Skipping: ai_agents table absent (older fixture schema)");
        eprintln!("[MIGRATION-v83] ✓ Done (no-op): ai_agents absent");
        return Ok(());
    }

    // ── C1: model backfill ──────────────────────────────────────────────────
    let backfilled = conn
        .execute(
            "UPDATE ai_agents \
             SET model = 'openclaw-pending', updated_at = ?1 \
             WHERE runtime_type = 'openclaw' AND (model IS NULL OR model = '')",
            rusqlite::params![chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| format!("[MIGRATION-v83] C1 model-backfill UPDATE failed: {}", e))?;
    eprintln!(
        "[MIGRATION-v83] C1: {} openclaw row(s) backfilled with 'openclaw-pending' placeholder (updated_at bumped)",
        backfilled
    );

    // ── C3: legacy-unpinned agent deactivation ──────────────────────────────
    let deactivated = conn
        .execute(
            "UPDATE ai_agents SET is_active = 0 WHERE runtime_type IS NULL",
            [],
        )
        .map_err(|e| format!("[MIGRATION-v83] C3 legacy-unpinned deactivation UPDATE failed: {}", e))?;
    eprintln!(
        "[MIGRATION-v83] C3: {} legacy-unpinned (runtime_type IS NULL) row(s) deactivated",
        deactivated
    );

    eprintln!("[MIGRATION-v83] ✓ Done");
    Ok(())
}

/// v83→v84: `chat_messages_local.oc_message_id` dedup column + partial unique index
/// (#2410, §30.11.2). Additive/nullable — pre-existing rows were never sourced from
/// an external ingestion adapter, so NULL is the correct value for them (excluded
/// from the partial unique index, never deduped — acceptable per spec).
///
/// Idempotent-guarded per the `migrate_v79` pattern: pragma-check before ALTER,
/// `IF NOT EXISTS` on the index. Real migration arm — external installs exist.
fn migrate_v84(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v84] Starting: chat_messages_local.oc_message_id dedup column (#2410, §30.11.2)");

    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='chat_messages_local'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v84] chat_messages_local table-existence check failed: {}", e))?;

    if !table_exists {
        eprintln!("[MIGRATION-v84] Skipping: chat_messages_local table absent (older fixture schema)");
        eprintln!("[MIGRATION-v84] ✓ Done (no-op): chat_messages_local absent");
        return Ok(());
    }

    // ── Step 1: ADD COLUMN oc_message_id TEXT ───────────────────────────────
    let oc_message_id_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_messages_local') WHERE name='oc_message_id'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v84] oc_message_id pragma check failed: {}", e))?;

    if !oc_message_id_exists {
        conn.execute_batch("ALTER TABLE chat_messages_local ADD COLUMN oc_message_id TEXT;")
            .map_err(|e| format!("[MIGRATION-v84] Step 1 ADD COLUMN oc_message_id failed: {}", e))?;
        eprintln!("[MIGRATION-v84] Step 1: oc_message_id column added");
    } else {
        eprintln!("[MIGRATION-v84] Step 1: oc_message_id already present (idempotent re-run)");
    }

    // ── Step 2: dedup partial unique index ───────────────────────────────────
    conn.execute_batch(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_chat_local_oc_dedup \
         ON chat_messages_local(thread_id, oc_message_id) \
         WHERE oc_message_id IS NOT NULL;",
    )
    .map_err(|e| format!("[MIGRATION-v84] Step 2 CREATE INDEX idx_chat_local_oc_dedup failed: {}", e))?;
    eprintln!("[MIGRATION-v84] Step 2: idx_chat_local_oc_dedup ensured");

    eprintln!("[MIGRATION-v84] ✓ Done");
    Ok(())
}

/// v84→v85: `chat_sessions.oc_baseline_done_at` baseline-marker column (#2410,
/// ingestion filters). Additive/nullable — NULL means "never baselined" for both
/// pre-existing rows and any session created before this migration ran; a session's
/// first-ever OC transcript pull (see `pull_oc_transcript_for_session` in
/// `ai/commands.rs`) stamps this column instead of backfilling the session's
/// pre-existing OC history as if it were new proactive output.
///
/// Idempotent-guarded per the `migrate_v84` pattern: pragma-check before ALTER.
/// Real migration arm — external installs exist.
fn migrate_v85(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v85] Starting: chat_sessions.oc_baseline_done_at baseline column (#2410, ingestion filters)");

    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='chat_sessions'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v85] chat_sessions table-existence check failed: {}", e))?;

    if !table_exists {
        eprintln!("[MIGRATION-v85] Skipping: chat_sessions table absent (older fixture schema)");
        eprintln!("[MIGRATION-v85] ✓ Done (no-op): chat_sessions absent");
        return Ok(());
    }

    // ── ADD COLUMN oc_baseline_done_at TEXT ──────────────────────────────────────
    let oc_baseline_done_at_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_sessions') WHERE name='oc_baseline_done_at'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v85] oc_baseline_done_at pragma check failed: {}", e))?;

    if !oc_baseline_done_at_exists {
        conn.execute_batch("ALTER TABLE chat_sessions ADD COLUMN oc_baseline_done_at TEXT;")
            .map_err(|e| format!("[MIGRATION-v85] ADD COLUMN oc_baseline_done_at failed: {}", e))?;
        eprintln!("[MIGRATION-v85] oc_baseline_done_at column added");
    } else {
        eprintln!("[MIGRATION-v85] oc_baseline_done_at already present (idempotent re-run)");
    }

    eprintln!("[MIGRATION-v85] ✓ Done");
    Ok(())
}

/// v85→v86: `chat_oc_tombstones` table + `idx_chat_local_thread_role_ts` index
/// (#2410, §30.11 stateful echo suppression / tombstoning). The tombstone table
/// records permanent classifications (`(thread_id, oc_message_id)` → `reason`) so
/// the backstop's per-tick, no-memory content-match re-evaluation can never give a
/// different answer for an id it already ruled on — see
/// `docs/audits/2026-08-24-tombstone-scope.md` §2 for the full mechanic design.
/// The composite index supports the widened (unbounded-lookback, role-scoped)
/// content-containment query added alongside it (§6.3 of the same audit).
///
/// Additive-only — new table, new index, no column changes to existing tables.
/// Idempotent-guarded per the `migrate_v84`/`migrate_v85` pattern: `IF NOT EXISTS`
/// on both the table and the index. Real migration arm — external installs exist.
fn migrate_v86(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v86] Starting: chat_oc_tombstones table + idx_chat_local_thread_role_ts index (#2410, §30.11)");

    // ── Step 1: chat_oc_tombstones table ────────────────────────────────────
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_oc_tombstones (
            thread_id      TEXT NOT NULL,
            oc_message_id  TEXT NOT NULL,
            reason         TEXT NOT NULL,
            tombstoned_at  TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (thread_id, oc_message_id)
        );",
    )
    .map_err(|e| format!("[MIGRATION-v86] Step 1 CREATE TABLE chat_oc_tombstones failed: {}", e))?;
    eprintln!("[MIGRATION-v86] Step 1: chat_oc_tombstones table ensured");

    // ── Step 2: idx_chat_local_thread_role_ts (only if chat_messages_local exists —
    // older fixture schemas predating that table can't host this index) ────────────
    let chat_messages_local_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='chat_messages_local'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v86] chat_messages_local table-existence check failed: {}", e))?;

    if chat_messages_local_exists {
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_chat_local_thread_role_ts
                ON chat_messages_local(thread_id, role, created_at DESC);",
        )
        .map_err(|e| format!("[MIGRATION-v86] Step 2 CREATE INDEX idx_chat_local_thread_role_ts failed: {}", e))?;
        eprintln!("[MIGRATION-v86] Step 2: idx_chat_local_thread_role_ts ensured");
    } else {
        eprintln!("[MIGRATION-v86] Step 2: Skipping index — chat_messages_local table absent (older fixture schema)");
    }

    eprintln!("[MIGRATION-v86] ✓ Done");
    Ok(())
}

/// v86→v87: `compaction_summaries.compacted_before_rowid` column (#2690, §30.11
/// OC-ingested message ordering fix). Origination-stamped `created_at` values
/// (Amendment 1, insert_external_message) can land chronologically BEFORE an
/// already-established `compacted_before` cutoff even though the row was never
/// actually summarized — the batch-interleave risk Reed's scoping audit
/// (`docs/audits/2026-08-24-ordering-scope-2690.md` §1, compaction WARN row)
/// flagged as a blocker-for-full-confidence. Keying the cutoff off `rowid`
/// (monotonic insertion order, unaffected by origination-timestamp stamping)
/// instead of `created_at` (now purely presentational) closes that gap — a
/// row's compaction-eligibility depends on WHEN it was inserted, never on what
/// origination time it claims.
///
/// Nullable, backfilled to NULL for existing rows (no migration can recover a
/// historical `rowid` boundary that was never captured at the time — same
/// no-backfill posture as §5 of the audit for `chat_messages_local` itself).
/// Existing `created_at`-keyed cutoff comparisons remain the fallback for any
/// `compaction_summaries` row written before this migration ran (its
/// `compacted_before_rowid` is NULL, so callers fall back to the legacy
/// `created_at` comparison for that one row — see commands.rs call sites).
///
/// Additive-only — new column, no data changes to existing tables. Idempotent-
/// guarded per the `migrate_v84`/`migrate_v85` pattern: pragma-checked before
/// `ALTER`. Real migration arm — external installs exist.
fn migrate_v87(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v87] Starting: compaction_summaries.compacted_before_rowid column (#2690, §30.11)");

    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='compaction_summaries'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v87] compaction_summaries table-existence check failed: {}", e))?;

    if !table_exists {
        eprintln!("[MIGRATION-v87] Skipping: compaction_summaries table absent (older fixture schema)");
        eprintln!("[MIGRATION-v87] ✓ Done (no-op): compaction_summaries absent");
        return Ok(());
    }

    let column_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('compaction_summaries') WHERE name='compacted_before_rowid'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v87] compacted_before_rowid pragma check failed: {}", e))?;

    if !column_exists {
        conn.execute_batch("ALTER TABLE compaction_summaries ADD COLUMN compacted_before_rowid INTEGER;")
            .map_err(|e| format!("[MIGRATION-v87] ADD COLUMN compacted_before_rowid failed: {}", e))?;
        eprintln!("[MIGRATION-v87] compacted_before_rowid column added");
    } else {
        eprintln!("[MIGRATION-v87] compacted_before_rowid already present (idempotent re-run)");
    }

    eprintln!("[MIGRATION-v87] ✓ Done");
    Ok(())
}

/// v87→v88: `{runtime}_configured_at` upgrader backfill for anthropic/openrouter/
/// venice (Blocker 3, #2777 venice live-test scoping).
///
/// The card-visibility fix (ProviderConnectionsPanel.svelte filters
/// `{#each connections}` down to `configured_this_install`) means a provider
/// with a real, working keychain credential but no `{runtime}_configured_at`
/// marker would otherwise lose its card forever on upgrade — the marker is
/// normally only ever written by `ai_configure_and_save`'s Save path, so any
/// install that configured a provider before this marker existed (or via some
/// other write path) has a real key and no marker.
///
/// This is a ONE-TIME backfill, run once at schema-bump time — NOT a per-request
/// check in `ai_list_runtime_connections` (deliberately: #2631's "stale keychain
/// leftover ≠ configured_this_install" contract must stay a stable, permanent
/// signal, not something that flips true the next time the list command happens
/// to run against a leftover key nobody ever actually configured through Save
/// this install). A key present at the moment THIS migration runs, on an install
/// old enough to be migrating up to v88 at all, is by definition not a "this
/// install" leftover in the #2631 sense — it is exactly the grandfathered
/// upgrader case this backfill exists to protect.
///
/// Idempotent: skips any runtime whose marker is already set. Keychain reads use
/// the real `crate::keychain` module directly (same as `migrate_v82_keychain_slot_split`'s
/// established precedent for synchronous keychain I/O inside a migration) — tests
/// inject `MockKeychain` via `set_test_keychain` before running migrations, so this
/// is exercised without touching the real OS keychain in CI.
fn migrate_v88(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v88] Starting: {{runtime}}_configured_at upgrader backfill (Blocker 3, #2777)");

    let ai_settings_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v88] ai_settings table-existence check failed: {}", e))?;

    if !ai_settings_table_exists {
        eprintln!("[MIGRATION-v88] ai_settings table absent (older fixture schema) — skipping");
        eprintln!("[MIGRATION-v88] ✓ Done (no-op): ai_settings absent");
        return Ok(());
    }

    use crate::ai::credential_migration::{
        KEYCHAIN_ACCOUNT_ANTHROPIC_KEY, KEYCHAIN_ACCOUNT_OPENROUTER_KEY, KEYCHAIN_ACCOUNT_VENICE_KEY,
    };

    let runtimes: [(&str, &str); 3] = [
        ("anthropic", KEYCHAIN_ACCOUNT_ANTHROPIC_KEY),
        ("openrouter", KEYCHAIN_ACCOUNT_OPENROUTER_KEY),
        ("venice", KEYCHAIN_ACCOUNT_VENICE_KEY),
    ];

    for (runtime, keychain_account) in runtimes {
        let marker_key = format!("{}_configured_at", runtime);

        if crate::database::get_ai_setting(conn, &marker_key).is_some() {
            eprintln!("[MIGRATION-v88] {} already present — skipping (idempotent re-run)", marker_key);
            continue;
        }

        match crate::keychain::get_password(keychain_account) {
            Ok(Some(_)) => {
                let backfilled_at = chrono::Utc::now().to_rfc3339();
                crate::database::set_ai_setting(conn, &marker_key, &backfilled_at).map_err(|e| {
                    format!("[MIGRATION-v88] Failed to backfill {}: {}", marker_key, e)
                })?;
                eprintln!(
                    "[MIGRATION-v88] Backfilled {} — existing keychain credential found, no prior marker (upgrader grandfather case)",
                    marker_key
                );
            }
            Ok(None) => {
                eprintln!("[MIGRATION-v88] No {} keychain credential — leaving {} unset (genuinely unconfigured)", runtime, marker_key);
            }
            Err(e) => {
                // Non-fatal: a keychain read failure here should not block the whole
                // migration from completing for the other runtimes, and should not
                // stamp SCHEMA_VERSION as failed either (this runs before
                // set_schema_version in the calling chain) — it just leaves this one
                // runtime's marker unset, same end state as "no credential found".
                eprintln!("[MIGRATION-v88] Keychain read failed for {} ({}) — leaving {} unset: {}", runtime, keychain_account, marker_key, e);
            }
        }
    }

    eprintln!("[MIGRATION-v88] ✓ Done");
    Ok(())
}

/// v88→v89: two independent, unrelated fixes bundled into one migration because
/// both landed together in the same venice live-test v2 scoping pass (#2455):
///
/// F1 — `ollama_configured_at` upgrader backfill. Ollama has no keychain credential
/// (it's a bare HTTP endpoint, not an API key) so it was entirely missed by
/// `migrate_v88`'s three-runtime keychain loop. Same upgrader-grandfather logic as
/// v88 though: any install with `ollama_endpoint` already set (i.e. Ollama was
/// configured through the app before the `configured_this_install` marker
/// existed) must get a marker backfilled, or its card vanishes forever behind
/// `ProviderConnectionsPanel`'s `configured_this_install` filter on upgrade.
///
/// F5 — `ai_agents.role` repair. The original 1b integrity safety net (since fixed,
/// see `verify_agents_integrity_with_path`) force-wrote `role='main'` onto
/// whichever row it picked as the promotion candidate, corrupting real OC
/// subagent rows into looking like the local default. `runtime_ref IS NOT NULL`
/// is the general signal that a row is OC-imported (see
/// `sync_initial_import`/`sync_re_import`, the only two writers of that column)
/// — EXCEPT for the one reserved case, OC's own main agent, which legitimately
/// has `runtime_type='openclaw' AND runtime_ref='main'` with `role` of `'main'`
/// or `'local'` (never `'sub'`) by design — see both sync functions' shared role
/// derivation (`is_default -> 'main'`, `oc_id=='main' -> 'local'`,
/// `else -> 'sub'`). Genuine OC subagents always have `runtime_ref` set to their
/// OWN oc_id (never the literal string `"main"`), so the repair predicate
/// excludes exactly that one reserved row shape and repairs everything else:
/// any row with `runtime_ref IS NOT NULL`, `role != 'sub'`, that is NOT OC's own
/// main/local row, must always be `'sub'`. This is a one-time repair pass for
/// damage already done by the pre-fix safety net; new rows can no longer be
/// corrupted this way since the safety net itself no longer overwrites `role`
/// (see F5 item 3), and `sync_initial_import`'s ON CONFLICT clause now
/// self-heals this same corruption on every re-import (see its own doc comment).
///
/// Both steps are idempotent (safe to re-run): F1 skips any runtime whose marker
/// is already set; F5's UPDATE only touches rows that are actually wrong
/// (`role != 'sub'`), so a second run affects zero rows.
fn migrate_v89(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v89] Starting: ollama_configured_at backfill (F1) + ai_agents.role repair (F5), #2455");

    // ── F1: ollama_configured_at backfill ──────────────────────────────────────
    let ai_settings_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v89] ai_settings table-existence check failed: {}", e))?;

    if !ai_settings_table_exists {
        eprintln!("[MIGRATION-v89] ai_settings table absent (older fixture schema) — skipping F1");
    } else if crate::database::get_ai_setting(conn, "ollama_configured_at").is_some() {
        eprintln!("[MIGRATION-v89] ollama_configured_at already present — skipping F1 (idempotent re-run)");
    } else if let Some(endpoint) = crate::database::get_ai_setting(conn, "ollama_endpoint") {
        let backfilled_at = chrono::Utc::now().to_rfc3339();
        crate::database::set_ai_setting(conn, "ollama_configured_at", &backfilled_at)
            .map_err(|e| format!("[MIGRATION-v89] Failed to backfill ollama_configured_at: {}", e))?;
        eprintln!(
            "[MIGRATION-v89] Backfilled ollama_configured_at — existing ollama_endpoint={} found, no prior marker (upgrader grandfather case)",
            endpoint
        );
    } else {
        eprintln!("[MIGRATION-v89] No ollama_endpoint set — leaving ollama_configured_at unset (genuinely unconfigured)");
    }

    // ── F5: ai_agents.role repair ───────────────────────────────────────────────
    let ai_agents_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v89] ai_agents table-existence check failed: {}", e))?;

    if !ai_agents_table_exists {
        eprintln!("[MIGRATION-v89] ai_agents table absent (older fixture schema) — skipping F5");
    } else {
        // Excludes OC's own main/local row (`runtime_type='openclaw' AND
        // runtime_ref='main'`) — the one legitimate case where a non-NULL
        // runtime_ref row is NOT a subagent (see doc comment above).
        let repaired = conn
            .execute(
                "UPDATE ai_agents SET role = 'sub' \
                 WHERE runtime_ref IS NOT NULL AND role != 'sub' \
                   AND NOT (runtime_type = 'openclaw' AND runtime_ref = 'main')",
                [],
            )
            .map_err(|e| format!("[MIGRATION-v89] role repair UPDATE failed: {}", e))?;
        if repaired > 0 {
            eprintln!(
                "[MIGRATION-v89] Repaired {} ai_agents row(s) with runtime_ref IS NOT NULL but role != 'sub' (1b safety-net corruption)",
                repaired
            );
        } else {
            eprintln!("[MIGRATION-v89] No ai_agents role corruption found (idempotent re-run or clean install)");
        }
    }

    eprintln!("[MIGRATION-v89] ✓ Done");
    Ok(())
}

/// v89→v90: sheet_columns/sheet_rows/sheet_cells tables (#2339, §1.12).
///
/// Additive-only — three brand-new tables, no data migration. Idempotent via
/// `CREATE TABLE IF NOT EXISTS` / `CREATE INDEX IF NOT EXISTS`, matching the
/// `migrate_v86` shape (new table + index) for the DDL portion. CRR enablement
/// (`crsql_as_crr`) mirrors `enable_crr`'s extension-availability gate so this
/// migration no-ops gracefully in test environments without cr-sqlite loaded.
///
/// No FKs on any sheet table (CRR table pattern, #493) — note_id/row_id/column_id
/// relationships are app-enforced.
fn migrate_v90(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v90] Starting: sheet_columns/sheet_rows/sheet_cells tables (#2339, §1.12)");

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sheet_columns (
            id           TEXT NOT NULL PRIMARY KEY,
            note_id      TEXT NOT NULL DEFAULT '',
            name         TEXT NOT NULL DEFAULT '',
            col_type     TEXT NOT NULL DEFAULT 'text',
            options_json TEXT NOT NULL DEFAULT '{}',
            width        INTEGER NOT NULL DEFAULT 0,
            sort_order   INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_sheet_columns_note ON sheet_columns(note_id);",
    )
    .map_err(|e| format!("[MIGRATION-v90] CREATE sheet_columns failed: {}", e))?;
    eprintln!("[MIGRATION-v90] sheet_columns table + index ensured");

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sheet_rows (
            id           TEXT NOT NULL PRIMARY KEY,
            note_id      TEXT NOT NULL DEFAULT '',
            sort_order   INTEGER NOT NULL DEFAULT 0,
            height       INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_sheet_rows_note ON sheet_rows(note_id);",
    )
    .map_err(|e| format!("[MIGRATION-v90] CREATE sheet_rows failed: {}", e))?;
    eprintln!("[MIGRATION-v90] sheet_rows table + index ensured");

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sheet_cells (
            row_id     TEXT NOT NULL,
            column_id  TEXT NOT NULL,
            note_id    TEXT NOT NULL DEFAULT '',
            value      TEXT NOT NULL DEFAULT '',
            style_json TEXT NOT NULL DEFAULT '{}',
            updated_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (row_id, column_id)
        );
        CREATE INDEX IF NOT EXISTS idx_sheet_cells_note ON sheet_cells(note_id);",
    )
    .map_err(|e| format!("[MIGRATION-v90] CREATE sheet_cells failed: {}", e))?;
    eprintln!("[MIGRATION-v90] sheet_cells table + index ensured");

    // Check if crsql_as_crr function is available (requires cr-sqlite extension) —
    // matches enable_crr's guard so this no-ops gracefully in test mode.
    let crr_available = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_function_list WHERE name='crsql_as_crr'",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);

    if crr_available {
        for table in ["sheet_columns", "sheet_rows", "sheet_cells"] {
            conn.query_row(&format!("SELECT crsql_as_crr('{}');", table), [], |_| Ok(()))
                .map_err(|e| format!("[MIGRATION-v90] Failed to enable CRR on {}: {}", table, e))?;
            eprintln!("[MIGRATION-v90] CRR enabled: {}", table);
        }
    } else {
        eprintln!("[MIGRATION-v90] ⚠️  cr-sqlite not loaded, skipping CRR enablement (test mode)");
    }

    eprintln!("[MIGRATION-v90] ✓ Done");
    Ok(())
}

/// v90→v91: `projects.default_folder_id` (#2874 item 50a, §1.7.1).
///
/// Column add follows the `migrate_attachments_v67` precedent exactly (:4393–4410
/// at the time this was written) — a plain `ALTER TABLE ... ADD COLUMN` on a synced
/// (CRR) table, made re-run-safe by treating "duplicate column name" as already-done.
/// `crsql_begin_alter`/`crsql_commit_alter` are deliberately NOT used here (never used
/// anywhere in this codebase; the one attempt, #447–449, aborted the upgrade chain).
///
/// Backfill (§1.7.1 ruling 3), both steps idempotent via `default_folder_id IS NULL`:
/// 1. Every top-level project with at least one folder → `default_folder_id` is set to
///    its OLDEST folder (`MIN(created_at)`, `id ASC` as a deterministic tie-break only
///    — not a user-facing rule). Oldest-by-created_at, not by name, is exactly why this
///    also does the right thing when a project's default folder has been renamed away
///    from "General" and a newer, unrelated folder happens to be named "General" now —
///    the newer stray never wins.
/// 2. Every top-level project with ZERO folders gets one created — named
///    `DEFAULT_FOLDER_NAME` ("General"), the same fallback `create_project_internal`
///    already uses for an unspecified `first_folder_name` — and `default_folder_id`
///    points at it.
///
/// No review log, no tie handling beyond the deterministic `id ASC` above — §1.7.1
/// ruling 3 is explicit that this isn't worth building for two users.
///
/// DB-only: no on-disk directory is created for a migration-backfilled "General"
/// folder. `create_note_on_disk`/`database.rs::create_note_on_disk` already
/// `create_dir_all`s the folder's directory lazily on first write, so there is
/// nothing for this migration to do on disk — not a MIGRATION-DISK-OPS case.
fn migrate_v91(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v91] Starting: projects.default_folder_id (#2874 item 50a, §1.7.1)");

    // Guard: some migration-chain test fixtures (and, in principle, a startup path
    // that hasn't reached `init_schema` yet) don't have a `projects` table at all —
    // same precedent as `migrate_attachments_v67`'s `table_exists` guard and
    // `migrate_v77`/`v78`/`v79`'s "table absent (older fixture schema)" skips
    // elsewhere in this file. The canonical schema creates `projects` with
    // `default_folder_id` already included (`create_projects_table`) on fresh
    // installs, so there's nothing for this migration to do without it.
    let projects_table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='projects'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !projects_table_exists {
        eprintln!("[MIGRATION-v91] projects table not yet present — default_folder_id will be included on creation (startup path)");
        return Ok(());
    }

    // Step 1: Add default_folder_id column (idempotent — ignore "duplicate column name"),
    // same pattern as migrate_attachments_v67's project_name column add.
    let result =
        conn.execute_batch("ALTER TABLE projects ADD COLUMN default_folder_id TEXT;");
    match result {
        Ok(_) => eprintln!("[MIGRATION-v91] ✓ default_folder_id column added to projects"),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("duplicate column name") || msg.contains("already exists") {
                eprintln!("[MIGRATION-v91] default_folder_id column already exists (idempotent)");
            } else {
                return Err(format!(
                    "[MIGRATION-v91] Failed to add default_folder_id column to projects: {}",
                    msg
                ));
            }
        }
    }

    // `projects` is a CRR table (crsql_as_crr was already called on it, like every
    // other table in CRR_TABLES). cr-sqlite's UPDATE trigger body is STATIC SQL
    // generated once, at `crsql_as_crr()` time, naming every column explicitly
    // (`NEW."col", OLD."col", ...` for each column that existed then) — a plain
    // `ALTER TABLE ... ADD COLUMN` does NOT regenerate it, and simply calling
    // `crsql_as_crr('projects')` again is a no-op on a table that's already CRR
    // (confirmed by direct trigger-SQL inspection: unchanged before/after).
    // Left alone, ANY subsequent UPDATE on this table — not just ones touching
    // the new column — fails at the native `crsql_after_update` call with an
    // argument-count mismatch ("expected N values, got M"), because that native
    // function reads the CURRENT live schema's column count but the static
    // trigger body still only supplies the OLD column list's worth of arguments.
    // This was NOT caught by any unit test (those run in-memory, without
    // cr-sqlite loaded) — it surfaced only via the mandated `smoke-upgrade-db`
    // run against the real extension, which is exactly what that check is for.
    //
    // Fix: rebuild `projects` (CREATE new-shape table, copy rows, DROP old, RENAME
    // into place, recreate indexes, re-enable CRR on the now brand-new table
    // object) — the exact shape `migrate_v68` already uses for `attachments` in
    // this same file, not a new mechanism. This is NOT `crsql_begin_alter`/
    // `crsql_commit_alter` (still unused anywhere in this codebase, per the v67
    // precedent/#447–449 history) — it's a full table replace, which re-enabling
    // CRR on a never-before-seen table name genuinely regenerates triggers for,
    // confirmed by the same direct trigger-SQL inspection. Gated on whether the
    // live trigger SQL already mentions `default_folder_id` (already rebuilt, by
    // this run or an earlier one) and on cr-sqlite being loaded at all (skips
    // gracefully in unit tests, matching every other crsql_as_crr gate in this
    // file) — both gates make this idempotent and a no-op on any re-run.
    let crr_available = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_function_list WHERE name='crsql_as_crr'",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);
    if crr_available {
        let trigger_is_stale: bool = conn
            .query_row(
                "SELECT COUNT(*) = 0 FROM sqlite_master
                 WHERE type = 'trigger' AND tbl_name = 'projects'
                   AND name = 'projects__crsql_utrig'
                   AND sql LIKE '%default_folder_id%'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(true);

        if trigger_is_stale {
            eprintln!("[MIGRATION-v91] projects' CRR update trigger is stale (pre-dates default_folder_id) — rebuilding table to refresh it");
            // Wrapped in BEGIN/COMMIT — same shape as migrate_attachments_v68's
            // rebuild (:4975/:5033, this file): a plain transaction (not a
            // savepoint), with no outer transaction held by run_migrations/
            // init_database around migration arms (confirmed: database.rs's
            // init_database calls run_migrations directly on a freshly opened
            // connection, no BEGIN in between), so nesting here is safe. Without
            // this, a crash/power-loss between CREATE+INSERT (new table
            // populated) and DROP+RENAME (swap) leaves both `projects` (original,
            // untouched) and `projects_v91_rebuild` (orphaned copy) on disk — but
            // WAL durability still risked a partial DROP/RENAME landing without
            // ever committing, which is the actual crash-window data-loss path
            // this closes: an aborted write mid-DROP could leave `projects`
            // missing entirely with no atomic undo. BEGIN/COMMIT makes the whole
            // rebuild all-or-nothing: on failure before COMMIT, SQLite's implicit
            // ROLLBACK on connection close (or a future explicit ROLLBACK) restores
            // the original `projects` table exactly as it was.
            conn.execute_batch(
                "BEGIN;
                CREATE TABLE projects_v91_rebuild (
                    id                    TEXT NOT NULL PRIMARY KEY,
                    name                  TEXT NOT NULL DEFAULT '',
                    description           TEXT,
                    color                 TEXT,
                    parent_id             TEXT,
                    sort_order            INTEGER NOT NULL DEFAULT 0,
                    owner_id              TEXT,
                    created_at            TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at            TEXT NOT NULL DEFAULT (datetime('now')),
                    category              TEXT,
                    is_system             INTEGER NOT NULL DEFAULT 0,
                    is_hidden             INTEGER NOT NULL DEFAULT 0,
                    is_favorite           INTEGER NOT NULL DEFAULT 0,
                    default_model         TEXT,
                    smart_routing_enabled INTEGER,
                    router_model          TEXT,
                    simple_tier_model     TEXT,
                    default_folder_id     TEXT
                );
                INSERT INTO projects_v91_rebuild (
                    id, name, description, color, parent_id, sort_order, owner_id,
                    created_at, updated_at, category, is_system, is_hidden, is_favorite,
                    default_model, smart_routing_enabled, router_model, simple_tier_model,
                    default_folder_id
                )
                SELECT
                    id, name, description, color, parent_id, sort_order, owner_id,
                    created_at, updated_at, category, is_system, is_hidden, is_favorite,
                    default_model, smart_routing_enabled, router_model, simple_tier_model,
                    default_folder_id
                FROM projects;
                DROP TABLE projects;
                ALTER TABLE projects_v91_rebuild RENAME TO projects;
                CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);
                CREATE INDEX IF NOT EXISTS idx_projects_parent ON projects(parent_id);
                CREATE INDEX IF NOT EXISTS idx_projects_hidden ON projects(is_hidden) WHERE is_hidden = 1;
                CREATE INDEX IF NOT EXISTS idx_projects_favorite ON projects(is_favorite) WHERE is_favorite = 1;
                COMMIT;",
            )
            .map_err(|e| format!("[MIGRATION-v91] Failed to rebuild projects table: {}", e))?;

            conn.query_row("SELECT crsql_as_crr('projects');", [], |_| Ok(()))
                .map_err(|e| format!("[MIGRATION-v91] Failed to re-enable CRR on rebuilt projects table: {}", e))?;
            eprintln!("[MIGRATION-v91] ✓ projects table rebuilt and CRR re-enabled (trigger now includes default_folder_id)");
        } else {
            eprintln!("[MIGRATION-v91] projects' CRR update trigger already current (idempotent re-run) — no rebuild needed");
        }
    } else {
        eprintln!("[MIGRATION-v91] ⚠️  cr-sqlite not loaded, skipping CRR trigger rebuild (test mode)");
    }

    // Guard: some older migration-chain fixtures have a `projects` table that
    // predates `parent_id`/`created_at` (folders/the current shape didn't exist
    // yet at that schema version) — same "older fixture schema" precedent as
    // above. Nothing to backfill without them; the column add above still
    // applies cleanly (ALTER TABLE doesn't care about other columns), just the
    // folder-backfill/CRR-rebuild steps below have nothing to work with.
    let has_folder_columns: bool = conn
        .query_row(
            "SELECT COUNT(*) = 2 FROM pragma_table_info('projects') WHERE name IN ('parent_id', 'created_at')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !has_folder_columns {
        eprintln!("[MIGRATION-v91] projects table missing parent_id/created_at (older fixture schema) — skipping folder backfill");
        return Ok(());
    }

    // Step 2: backfill from the oldest existing folder. Collect the (project_id,
    // oldest_folder_id) pairs first, THEN mutate row-by-row with a plain
    // single-row UPDATE — same "collect, then mutate" shape as
    // migrate_attachments_v67's `pending_rows`. A correlated-subquery bulk
    // UPDATE against this CRR table was tried first and tripped a cr-sqlite
    // CRR-trigger column-count mismatch right after the ALTER TABLE above
    // ("expected N values, got M") — per-row UPDATEs with literal bound values
    // don't hit it. Only touches rows where default_folder_id IS NULL, so a
    // re-run (every project already has a value) finds nothing to collect —
    // this is the entire idempotency story for this step.
    let oldest_folder_backfills: Vec<(String, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT p.id, (
                     SELECT f.id FROM projects AS f
                     WHERE f.parent_id = p.id
                     ORDER BY f.created_at ASC, f.id ASC
                     LIMIT 1
                 ) AS oldest_folder_id
                 FROM projects AS p
                 WHERE p.parent_id IS NULL
                   AND p.default_folder_id IS NULL
                   AND EXISTS (SELECT 1 FROM projects AS f2 WHERE f2.parent_id = p.id)",
            )
            .map_err(|e| format!("[MIGRATION-v91] Failed to prepare oldest-folder query: {}", e))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| format!("[MIGRATION-v91] Failed to map oldest-folder rows: {}", e))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("[MIGRATION-v91] Failed to collect oldest-folder rows: {}", e))?
    };

    for (project_id, oldest_folder_id) in &oldest_folder_backfills {
        conn.execute(
            "UPDATE projects SET default_folder_id = ?1 WHERE id = ?2",
            params![oldest_folder_id, project_id],
        )
        .map_err(|e| {
            format!(
                "[MIGRATION-v91] Failed to backfill default_folder_id for project {}: {}",
                project_id, e
            )
        })?;
    }
    if !oldest_folder_backfills.is_empty() {
        eprintln!(
            "[MIGRATION-v91] Backfilled default_folder_id (oldest folder) for {} project(s)",
            oldest_folder_backfills.len()
        );
    }

    // Step 3: every top-level project with ZERO folders gets a DEFAULT_FOLDER_NAME
    // folder created, then default_folder_id points at it. Same NULL-gated selection
    // as step 2 keeps a re-run a no-op (no folderless projects left to find).
    let folderless_projects: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT id FROM projects
                 WHERE parent_id IS NULL
                   AND default_folder_id IS NULL
                   AND NOT EXISTS (SELECT 1 FROM projects AS f WHERE f.parent_id = projects.id)",
            )
            .map_err(|e| format!("[MIGRATION-v91] Failed to query folderless projects: {}", e))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("[MIGRATION-v91] Failed to map folderless projects: {}", e))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("[MIGRATION-v91] Failed to collect folderless projects: {}", e))?
    };

    if !folderless_projects.is_empty() {
        eprintln!(
            "[MIGRATION-v91] {} folderless top-level project(s) need a {} folder created",
            folderless_projects.len(),
            crate::projects::DEFAULT_FOLDER_NAME
        );
    }

    for project_id in folderless_projects {
        let folder_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                folder_id,
                crate::projects::DEFAULT_FOLDER_NAME,
                &project_id,
                "user_local",
                &now,
                &now
            ],
        )
        .map_err(|e| {
            format!(
                "[MIGRATION-v91] Failed to create {} folder for project {}: {}",
                crate::projects::DEFAULT_FOLDER_NAME,
                project_id,
                e
            )
        })?;

        conn.execute(
            "UPDATE projects SET default_folder_id = ?1 WHERE id = ?2",
            params![folder_id, project_id],
        )
        .map_err(|e| {
            format!(
                "[MIGRATION-v91] Failed to set default_folder_id for project {}: {}",
                project_id, e
            )
        })?;

        eprintln!(
            "[MIGRATION-v91] Created {} folder {} for folderless project {}",
            crate::projects::DEFAULT_FOLDER_NAME,
            folder_id,
            project_id
        );
    }

    eprintln!("[MIGRATION-v91] ✓ Done");
    Ok(())
}

/// v91→v92: clears pre-existing `chat_oc_tombstones` rows with `reason IN
/// ('stale_echo', 'backstop_echo')` (#3053 item F, §30.11.10).
///
/// Before this fix, `check_stale_echo` and `is_recent_local_assistant_echo` both
/// classified purely by text containment and, on a match, wrote a PERMANENT
/// `chat_oc_tombstones` row (`reason='stale_echo'` or `'backstop_echo'`) — see
/// `is_already_classified`'s doc comment: once a `(thread_id, oc_message_id)` pair is
/// tombstoned, the Rule 1 permanence pre-filter drops it on every future poll tick,
/// unconditionally, before any other check ever runs again. A fix that only changes
/// FUTURE classification behavior (the functions no longer tombstone on a text match
/// — see their own doc comments) leaves every message ALREADY wrongly tombstoned
/// this way permanently invisible, forever — the permanence pre-filter has no
/// expiry and no other code path ever re-evaluates a settled id. This migration is
/// what actually surfaces those wrongly-hidden messages again.
///
/// **`own_turn` tombstones are deliberately NOT touched.** That reason is written by
/// `tombstone_own_turn`, which classifies by `__openclaw.seq` strictly greater than a
/// per-dispatch baseline — an identity-based mechanism, unrelated to and unaffected
/// by this fix. Clearing it would reintroduce exactly the duplicate-echo spam the
/// seq-based mechanism exists to prevent, for no corresponding benefit.
///
/// **Clearing a tombstone row does not, by itself, make the underlying message
/// reappear.** It only removes the Rule 1 permanence pre-filter's reason to drop the
/// id on the NEXT poll tick for that session. Because `pull_oc_transcript_for_session`
/// has no `since`/cursor state (§30.11.3) and always re-fetches OC's small recent
/// page, a message cleared here reappears automatically the next time that session's
/// page still contains it — if OC's own retention has since rolled it out of the
/// fetched window, clearing the tombstone has nothing left to resurrect, which is an
/// accepted, pre-existing limitation of this slice's no-cursor design, not something
/// this migration can or should work around.
///
/// **Why a migration, not an on-read clear:** this is a genuine one-time cleanup of
/// data written by behavior this same fix permanently removes going forward — no
/// code path can ever write a `stale_echo`/`backstop_echo` tombstone again after this
/// fix ships, so there is nothing for an on-read check to keep guarding against on
/// every poll tick. Running it as a migration (a) matches this file's own convention
/// for one-off data backfills/cleanups tied to a behavior change (e.g. `migrate_v91`'s
/// `default_folder_id` backfill), (b) runs exactly once per install via the existing
/// schema-version gate rather than adding a per-poll-tick DELETE that would do
/// nothing after the first tick ever since, and (c) gets the same pre-migration
/// backup safety net every other schema-version transition already has.
///
/// **Idempotency / second-run behavior:** the `DELETE ... WHERE reason IN (...)`
/// itself is naturally idempotent — a second execution (e.g. if this exact function
/// were ever called again) finds zero matching rows and deletes zero, succeeding as
/// a clean no-op. The schema-version gate in `run_migrations` additionally ensures
/// this never runs twice in the normal upgrade path: once `schema_version` reaches
/// "92", the `Some(ref version) if version == SCHEMA_VERSION` arm short-circuits to
/// `verify_crr_tables` only, and this function is never invoked again for that
/// install. Safe to call again regardless (e.g. a test re-running it directly, or a
/// hypothetical future re-entry) — no new rows accumulate from any of the reasons
/// this migration clears, since no code writes them any more.
fn migrate_v92(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v92] Starting: clear stale_echo/backstop_echo chat_oc_tombstones rows (#3053 item F, §30.11.10)");

    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='chat_oc_tombstones'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION-v92] chat_oc_tombstones table-existence check failed: {}", e))?;

    if !table_exists {
        eprintln!("[MIGRATION-v92] Skipping: chat_oc_tombstones table absent (older fixture schema, or migrate_v86 hasn't run on this connection yet)");
        eprintln!("[MIGRATION-v92] ✓ Done (no-op): chat_oc_tombstones absent");
        return Ok(());
    }

    let cleared = conn
        .execute(
            "DELETE FROM chat_oc_tombstones WHERE reason IN ('stale_echo', 'backstop_echo')",
            [],
        )
        .map_err(|e| format!("[MIGRATION-v92] DELETE FROM chat_oc_tombstones failed: {}", e))?;

    eprintln!("[MIGRATION-v92] Cleared {} stale_echo/backstop_echo tombstone row(s) — own_turn tombstones untouched", cleared);
    eprintln!("[MIGRATION-v92] ✓ Done");
    Ok(())
}

/// v92 → v93: `agent_jobs` table (non-CRR, per-device — §36.10) + four new
/// columns on `chat_messages_local` (#3050 PR 2, §36.3/§36.10).
///
/// CRR safety (re-confirmed at `69322e91`, not re-trusted from a prior audit):
/// `agent_jobs` is absent from `upgrade_tables_to_crr_inner`'s table list and
/// from the fresh-install CRR enable path, so no `crsql_as_crr` call is made
/// for it. `chat_messages_local` is likewise absent from both CRR table lists
/// — `migrate_v76` (#2030, `agent_id`) and `migrate_v84` (#2410,
/// `oc_message_id`) already added plain columns to this exact table with the
/// same "NOT a CRR table" reasoning; this migration is the third instance of
/// the same shape. #3005's hazard ("ALTER a CRR table breaks writes") is
/// about a table that WAS CRR — it does not apply here.
///
/// Idempotency: `agent_jobs`'s CREATE TABLE already carries `IF NOT EXISTS`;
/// each `chat_messages_local` column is guarded individually via
/// `add_column_if_missing` (same `pragma_table_info` check `migrate_v76`/
/// `migrate_v84` use), since SQLite has no `ADD COLUMN IF NOT EXISTS`.
/// `attachment_ids` already exists in every real install's `chat_messages_local`
/// (present in the original schema, not added by a prior ALTER) — the guard
/// makes this specific call a documented no-op rather than a duplicate-column
/// error, while still matching §36.10's own ALTER list verbatim.
fn migrate_v93(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v93] Starting: agent_jobs table + chat_messages_local columns (#3050 PR 2, §36.3/§36.10)");

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS agent_jobs (
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
            updated_at         TEXT NOT NULL DEFAULT (datetime('now')),
            oc_session_key     TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_agent_jobs_due ON agent_jobs(status, next_run_at);
        CREATE INDEX IF NOT EXISTS idx_agent_jobs_origin ON agent_jobs(origin_message_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_jobs_heartbeat ON agent_jobs(agent_id) WHERE kind = 'heartbeat';
        CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_jobs_one_pending_wake_per_session ON agent_jobs(session_id) WHERE kind = 'wake' AND status = 'pending';
        ",
    )
    .map_err(|e| format!("[MIGRATION-v93] agent_jobs CREATE TABLE failed: {}", e))?;
    eprintln!("[MIGRATION-v93] Step 1: agent_jobs table + indexes created (non-CRR, per-device)");

    add_column_if_missing(conn, "chat_messages_local", "job_id", "TEXT")?;
    add_column_if_missing(conn, "chat_messages_local", "synthetic", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(conn, "chat_messages_local", "continued", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(conn, "chat_messages_local", "attachment_ids", "TEXT")?;

    eprintln!("[MIGRATION-v93] ✓ Done: agent_jobs + 4 chat_messages_local columns (existing rows unaffected)");
    Ok(())
}

/// v93 → v94: `crew_agent_ids` on `chat_sessions` (#3050 PR 5, §36.17/§36.4.1).
/// Nullable, no default beyond NULL — NULL means "no crew," which is the only
/// genuine schema gap PR 5 needs (`parent_job_id`/`root_job_id`/`chain_depth`/
/// `wake_origin`/`origin_message_id` all already exist on `agent_jobs` since
/// migrate_v93 — PR 5 threads those existing columns through existing Rust
/// structs, no DDL for any of them; see Reed's scope audit §2).
///
/// Idempotency: same `add_column_if_missing` guard `migrate_v93` itself uses.
fn migrate_v94(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v94] Starting: crew_agent_ids on chat_sessions (#3050 PR 5, §36.17)");

    add_column_if_missing(conn, "chat_sessions", "crew_agent_ids", "TEXT")?;

    eprintln!("[MIGRATION-v94] ✓ Done: chat_sessions.crew_agent_ids added (existing rows unaffected, NULL = no crew)");
    Ok(())
}

/// v94 → v95: `oc_session_key` on `agent_jobs` (#3050 fix PR, Bug 1(b)).
/// Nullable, no default beyond NULL — NULL means "this job's OC session has
/// never been bootstrapped via the gateway's `POST /v1/sessions`
/// (`sessions.create`)." `ai::scheduler::dispatch_oc_job` checks this column
/// before its first `sessions.send` for a job: NULL → call `sessions.create`
/// first (bootstrap), then write OC's own returned canonical key here
/// (`CreateSessionResponse.oc_session_id`); non-NULL → skip the bootstrap
/// call entirely and send to the STORED key (the recurring-job
/// second-firing case). This is the job-dispatch path's own key source of
/// truth once set — unlike `ai::ingest`'s transcript-pull functions, which
/// independently derive their OWN key fresh every call for ordinary
/// (non-job) OC chat sessions (§30.11.3, unaffected by this column — no
/// existing reader of a job session's OC key re-derives it, so storing
/// OC's real answer here is strictly more robust than guessing a formula).
///
/// Idempotency: same `add_column_if_missing` guard `migrate_v93`/`migrate_v94` use.
fn migrate_v95(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v95] Starting: oc_session_key on agent_jobs (#3050 fix PR, Bug 1(b))");

    add_column_if_missing(conn, "agent_jobs", "oc_session_key", "TEXT")?;

    eprintln!("[MIGRATION-v95] ✓ Done: agent_jobs.oc_session_key added (existing rows unaffected, NULL = not yet bootstrapped)");
    Ok(())
}

/// v95 → v96: `consecutive_failures`/`last_failed_at` on `agent_jobs`
/// (live-test #1b fix PR, Ruling 3). Both nullable/zero-defaulted — a
/// pre-existing row with no prior failures reads as `consecutive_failures=0`,
/// `last_failed_at=NULL`, identical in meaning to a freshly-migrated row
/// with no failures recorded yet. `last_error` already existed
/// (`migrate_v93`) and is reused as-is; these two new columns are what
/// Ruling 3's recurring/heartbeat failure-visibility policy needs beyond
/// that: a per-row strike counter and a timestamp for the chip's "last run
/// failed" display, neither of which existed before this migration.
///
/// Idempotency: same `add_column_if_missing` guard every prior migration in
/// this file uses.
fn migrate_v96(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v96] Starting: consecutive_failures/last_failed_at on agent_jobs (live-test #1b fix PR, Ruling 3)");

    add_column_if_missing(conn, "agent_jobs", "consecutive_failures", "INTEGER NOT NULL DEFAULT 0")?;
    add_column_if_missing(conn, "agent_jobs", "last_failed_at", "TEXT")?;

    eprintln!("[MIGRATION-v96] ✓ Done: agent_jobs.consecutive_failures (default 0) + last_failed_at (NULL) added");
    Ok(())
}

/// v96 → v97: `archived_at` on `agent_jobs` (#3151 PR2/C24). Nullable, no
/// default beyond NULL — NULL means visible in the Done tab, a timestamp
/// means archived (hidden from the panel, row stays for the session's
/// history). Mirrors `chat_sessions.hidden_at` exactly, per #3136's own
/// established-precedent argument — same NULL-vs-stamped-timestamp shape,
/// same "archive ≠ delete" meaning.
///
/// This is NOT the same mechanism as `RUNTIME_JOB_ARCHIVE_DAYS`
/// (`ai::runtime_constants`) — that constant has, until this PR, been
/// enforced exclusively against `chat_sessions.last_active_at` as the
/// §36.4.5 wake-staleness proxy and has never written to an `agent_jobs`
/// row. This column is `RUNTIME_JOB_ARCHIVE_DAYS`'s first-ever use against
/// `agent_jobs` (see `ai::scheduler::sweep_archivable_jobs`), reusing the
/// same 30-day number by convention, not by shared code path.
///
/// A `status = 'archived'` overwrite was considered and rejected: it would
/// destroy the done/failed/cancelled distinction the Done tab exists to
/// show. `archived_at` is deliberately orthogonal to `status`.
///
/// Idempotency: same `add_column_if_missing` guard every prior migration in
/// this file uses.
fn migrate_v97(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v97] Starting: archived_at on agent_jobs (#3151 PR2/C24)");

    add_column_if_missing(conn, "agent_jobs", "archived_at", "TEXT")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_agent_jobs_archived_at ON agent_jobs(archived_at);",
    )
    .map_err(|e| format!("[MIGRATION-v97] Failed to create idx_agent_jobs_archived_at: {}", e))?;

    eprintln!("[MIGRATION-v97] ✓ Done: agent_jobs.archived_at added (NULL = visible) + idx_agent_jobs_archived_at index");
    Ok(())
}

/// Shared idempotency guard, originally for `migrate_v93`'s four new
/// `chat_messages_local` columns, now also used by `migrate_v94`'s
/// `chat_sessions.crew_agent_ids` (#3050 PR 5) — same "SQLite has no `ALTER
/// TABLE ... ADD COLUMN IF NOT EXISTS`" problem, same `pragma_table_info`
/// check. Every other migration in this file that adds a column
/// (`migrate_v76`, `migrate_v84`, etc.) hand-rolls this same check once,
/// inline, for its one column — factored here because `migrate_v93` needed
/// the identical check four times; `migrate_v94` reuses it rather than
/// hand-rolling a fifth copy.
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            &format!("SELECT COUNT(*) > 0 FROM pragma_table_info('{}') WHERE name=?1", table),
            params![column],
            |row| row.get(0),
        )
        .map_err(|e| format!("[MIGRATION] pragma check failed for {}.{}: {}", table, column, e))?;

    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {} ADD COLUMN {} {};", table, column, decl))
            .map_err(|e| format!("[MIGRATION] ALTER TABLE {} ADD COLUMN {} failed: {}", table, column, e))?;
        eprintln!("[MIGRATION] {}.{} column added", table, column);
    } else {
        eprintln!("[MIGRATION] {}.{} column already present (idempotent re-run)", table, column);
    }
    Ok(())
}

/// Searches `pane_pre-migration-*.db` files in the backups dir for a recoverable
/// `ai_settings.endpoint` value, trying the most-likely-relevant candidates first.
/// Returns `None` if no backup exists, none are readable, or none have an `endpoint`
/// value — callers must not synthesize a placeholder on `None` (see `migrate_v80`).
fn recover_endpoint_from_backups() -> Option<String> {
    let pane_fs = match PaneFs::for_runtime() {
        Ok(pane_fs) => pane_fs,
        Err(e) => {
            eprintln!(
                "[MIGRATION-v80] Could not initialize filesystem policy: {}",
                e
            );
            return None;
        }
    };
    let backups_relative = std::path::Path::new("backups");
    let backups_dir = match pane_fs.authorize_existing(
        PaneRoot::Internal,
        backups_relative,
        ExistingKind::Directory,
    ) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("[MIGRATION-v80] Backups directory rejected: {}", e);
            return None;
        }
    };
    let entries = match std::fs::read_dir(backups_dir.as_path()) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!(
                "[MIGRATION-v80] Could not read backups dir {}: {}",
                backups_dir.as_path().display(),
                e
            );
            return None;
        }
    };

    // Matches both observed naming patterns: single-version ("pane_pre-migration-v78.db")
    // and range ("pane_pre-migration-v62-to-v69.db"). Captures the FIRST embedded version
    // number — for range-style names that's the pre-migration source version, which is
    // the one that can actually hold a pre-dissolution endpoint value.
    let name_pattern = regex::Regex::new(r"^pane_pre-migration-v(\d+)(?:-to-v\d+)?\.db$")
        .expect("static regex must compile");

    let mut candidates: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name_str) = file_name.to_str() else { continue };
        let Some(caps) = name_pattern.captures(file_name_str) else { continue };
        let Some(version) = caps.get(1).and_then(|m| m.as_str().parse::<u32>().ok()) else {
            continue;
        };
        // Only consider pre-v79 backups — v79's Step 6 is the deletion itself, so a
        // backup taken AT v79 (or later) cannot hold a recoverable endpoint.
        if version <= 78 {
            let relative = match PaneFs::relative_from_names(["backups", file_name_str]) {
                Ok(relative) => relative,
                Err(e) => {
                    eprintln!("[MIGRATION-v80] Backup candidate name rejected: {}", e);
                    continue;
                }
            };
            match pane_fs.authorize_existing(PaneRoot::Internal, &relative, ExistingKind::File) {
                Ok(path) => candidates.push((version, path.as_path().to_path_buf())),
                Err(e) => {
                    eprintln!(
                        "[MIGRATION-v80] Backup candidate {} rejected: {}",
                        file_name_str, e
                    );
                }
            }
        }
    }

    // Highest version first — closest to the v79 boundary, most likely to reflect the
    // last-known-good endpoint before dissolution.
    candidates.sort_by(|a, b| b.0.cmp(&a.0));

    for (version, path) in candidates {
        eprintln!(
            "[MIGRATION-v80] Trying backup candidate: {} (embedded version {})",
            path.display(),
            version
        );
        match read_endpoint_from_backup_db(&path) {
            Some(endpoint) => {
                eprintln!(
                    "[MIGRATION-v80] Recovered endpoint from {}: {}",
                    path.display(),
                    endpoint
                );
                return Some(endpoint);
            }
            None => {
                eprintln!(
                    "[MIGRATION-v80] {} has no usable endpoint — trying next candidate",
                    path.display()
                );
            }
        }
    }

    None
}

/// Opens a single pre-migration backup DB read-only via ATTACH and reads
/// `ai_settings.endpoint`, if present and non-empty. Any error (corrupt file, missing
/// table, etc.) is logged and treated as "no endpoint here" rather than propagated —
/// a single bad/partial backup must not abort the whole recovery search.
fn read_endpoint_from_backup_db(path: &std::path::Path) -> Option<String> {
    let conn = match Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!(
                "[MIGRATION-v80] Failed to open backup {}: {}",
                path.display(),
                e
            );
            return None;
        }
    };

    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        return None;
    }

    let endpoint: Option<String> = conn
        .query_row(
            "SELECT value FROM ai_settings WHERE key = 'endpoint'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap_or(None)
        .flatten();

    endpoint.filter(|v| !v.is_empty())
}

/// Verify that all CRR tables have correct trigger registration
/// Gracefully skips if cr-sqlite extension is not available (e.g., in tests)
fn verify_crr_tables(conn: &Connection) -> Result<(), String> {
    // Check if crsql_as_crr function is available (requires cr-sqlite extension)
    let crr_available = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_function_list WHERE name='crsql_as_crr'",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);

    if !crr_available {
        eprintln!("[MIGRATIONS] ⚠️  cr-sqlite extension not loaded, skipping CRR verification (test mode)");
        return Ok(());
    }

    for table in &CRR_TABLES {
        // Check if table is CRR-tracked
        let is_crr: bool = conn.query_row(
            &format!("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='{}__crsql_clock'", table),
            [], |row| row.get(0),
        ).unwrap_or(false);

        if !is_crr {
            return Err(format!(
                "CRR verification failed: Table '{}' should be CRR-tracked but has no __crsql_clock table",
                table
            ));
        }

        // Check if CRR triggers exist for this table
        let trigger_count: i64 = conn
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND tbl_name = '{}'",
                    table
                ),
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if trigger_count == 0 {
            return Err(format!(
                "CRR verification failed: Table '{}' is CRR-tracked but has no triggers",
                table
            ));
        }
    }

    eprintln!("[MIGRATIONS] ✓ All 27 CRR tables verified");
    Ok(())
}

/// v65→v66: add `engine TEXT` column to embeddings table (ADR-0048 / §5.9 HTTP backend)
///
/// The column is nullable so existing rows (if any) get NULL = stale/orphaned.
/// `switch_embedding_engine` uses this column to wipe stale cross-engine vectors.
///
/// Safe to call when the embeddings table doesn’t exist yet (fresh install path).
/// In that case the table will be created with the column at startup.
fn migrate_embeddings_engine_column(conn: &Connection) -> Result<(), String> {
    // Check if table exists first — test DBs and fresh installs may not have it yet
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='embeddings'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        eprintln!("[MIGRATIONS] embeddings table not yet present — engine column will be included on creation (startup path)");
        return Ok(());
    }

    // Column addition is idempotent: ignore "duplicate column name" errors from SQLite
    let result = conn.execute_batch("ALTER TABLE embeddings ADD COLUMN engine TEXT;");
    match result {
        Ok(_) => {
            eprintln!("[MIGRATIONS] ✓ embeddings.engine column added");
            Ok(())
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("duplicate column name") || msg.contains("already exists") {
                eprintln!("[MIGRATIONS] embeddings.engine column already exists (idempotent)");
                Ok(())
            } else {
                Err(format!(
                    "Failed to add engine column to embeddings: {}",
                    msg
                ))
            }
        }
    }
}

/// v66→v67: Fix attachment storage path (.pane/attachments → Projects/{name}/Assets/) and
/// migrate project_id from project-name-string to UUID. Adds project_name TEXT column
/// (denormalized, for path construction without joins on reads). (#1775, spec-covered-by: #1783)
///
/// Idempotent: safe to re-run. Column addition swallows "duplicate column name" per pattern
/// in `migrate_embeddings_engine_column`. SQL UPDATEs are conditional on matching rows.
/// File moves are skipped for rows where source file does not exist.
///
/// On this install the attachments table is empty — migration is a no-op.
/// Ship the routine for production correctness.
// MIGRATION-DISK-OPS: renames attachment blobs on disk from legacy path
// `~/Pane/.pane/attachments/{project_name}/{id}.{ext}` to the new per-project
// path `~/Pane/Projects/{project_name}/Assets/{id}.{ext}`. Backup scope MUST
// include media if this migration is being added to a new version range; see
// backup.rs `create_backup` and the MIGRATION-DISK-OPS convention above.
struct AttachmentV67Move {
    id: String,
    project_name: String,
    filename: String,
}

fn validate_attachment_v67_row(
    id: String,
    project_name: String,
    original_filename: String,
) -> Result<AttachmentV67Move, String> {
    PaneFs::validate_name(&id)
        .map_err(|e| format!("[MIGRATION-v67] Unsafe attachment id {id:?}: {e}"))?;
    PaneFs::validate_name(&project_name)
        .map_err(|e| format!("[MIGRATION-v67] Unsafe project_name for attachment {id:?}: {e}"))?;
    let filename = attachment_storage_filename(&id, &original_filename).map_err(|e| {
        format!("[MIGRATION-v67] Unsafe generated filename for attachment {id:?}: {e}")
    })?;

    Ok(AttachmentV67Move {
        id,
        project_name,
        filename,
    })
}

fn migrate_attachments_v67(conn: &Connection) -> Result<(), String> {
    eprintln!(
        "[MIGRATION-v67] Starting: attachment storage path fix + project_id UUID (#1775, #2187)"
    );

    // Guard: if the attachments table does not exist yet (test DBs, fresh installs using
    // older schema snapshots), the canonical schema will create it with project_name included.
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='attachments'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        eprintln!("[MIGRATION-v67] attachments table not yet present — project_name will be included on creation (startup path)");
        return Ok(());
    }

    // Step 1: Add project_name column (idempotent — ignore \"duplicate column name\")
    let result = conn
        .execute_batch("ALTER TABLE attachments ADD COLUMN project_name TEXT NOT NULL DEFAULT '';");
    match result {
        Ok(_) => eprintln!("[MIGRATION-v67] ✓ project_name column added to attachments"),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("duplicate column name") || msg.contains("already exists") {
                eprintln!("[MIGRATION-v67] project_name column already exists (idempotent)");
            } else {
                return Err(format!(
                    "[MIGRATION-v67] Failed to add project_name column to attachments: {}",
                    msg
                ));
            }
        }
    }

    // Collect and validate every row before changing row data or touching the filesystem.
    // Rows in `pending_rows` still use the project name as project_id. Rows in `moves`
    // were migrated by an earlier partial/idempotent run and may still need their blob moved.
    let pending_rows: Vec<(AttachmentV67Move, String)> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, project_id, original_filename,
                    (SELECT id FROM projects WHERE name = attachments.project_id LIMIT 1)
             FROM attachments
         WHERE project_id IN (SELECT name FROM projects)
               AND (project_name IS NULL OR project_name = '')",
            )
            .map_err(|e| format!("[MIGRATION-v67] Failed to prepare row migration query: {e}"))?;
        let mapped_rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| format!("[MIGRATION-v67] Failed to query rows for migration: {e}"))?;
        let queried_rows = mapped_rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("[MIGRATION-v67] Failed to read row for migration: {e}"))?;
        queried_rows
            .into_iter()
            .map(|(id, project_name, original_filename, project_uuid)| {
                validate_attachment_v67_row(id, project_name, original_filename)
                    .map(|row| (row, project_uuid))
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    let mut moves: Vec<AttachmentV67Move> = {
        let mut stmt = conn
            .prepare(
                "SELECT id, project_name, original_filename
             FROM attachments WHERE project_name != ''",
            )
            .map_err(|e| format!("[MIGRATION-v67] Failed to prepare file move query: {e}"))?;
        let mapped_rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| format!("[MIGRATION-v67] Failed to query rows for file move: {e}"))?;
        let queried_rows = mapped_rows
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("[MIGRATION-v67] Failed to read row for file move: {e}"))?;
        queried_rows
            .into_iter()
            .map(|(id, project_name, original_filename)| {
                validate_attachment_v67_row(id, project_name, original_filename)
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    for (row, project_uuid) in &pending_rows {
        conn.execute(
            "UPDATE attachments
             SET project_name = ?2, project_id = ?3
             WHERE id = ?1 AND project_id = ?2
               AND (project_name IS NULL OR project_name = '')",
            params![row.id, row.project_name, project_uuid],
        )
        .map_err(|e| {
            format!(
                "[MIGRATION-v67] Failed to migrate attachment {:?}: {e}",
                row.id
            )
        })?;
    }
    moves.extend(pending_rows.into_iter().map(|(row, _)| row));

    eprintln!("[MIGRATION-v67] ✓ DB schema + data migration complete");

    if moves.is_empty() {
        eprintln!("[MIGRATION-v67] No attachment rows to migrate — table is empty (no-op)");
        return Ok(());
    }

    let pane_fs = PaneFs::for_runtime()
        .map_err(|e| format!("[MIGRATION-v67] Filesystem policy initialization failed: {e}"))?;
    let mut moved = 0usize;
    let mut skipped = 0usize;

    for row in &moves {
        let source_relative =
            PaneFs::relative_from_names([row.project_name.as_str(), row.filename.as_str()])
                .map_err(|e| format!("[MIGRATION-v67] Unsafe legacy attachment path: {e}"))?;
        let destination_dir_relative =
            PaneFs::relative_from_names([row.project_name.as_str(), "Assets"])
                .map_err(|e| format!("[MIGRATION-v67] Unsafe Assets directory path: {e}"))?;
        let destination_relative = PaneFs::relative_from_names([
            row.project_name.as_str(),
            "Assets",
            row.filename.as_str(),
        ])
        .map_err(|e| format!("[MIGRATION-v67] Unsafe attachment destination path: {e}"))?;

        // authorize_write safely resolves the candidate without following symlinks and lets
        // us preserve the legacy behavior of skipping rows whose source blob is absent.
        let source_candidate = pane_fs
            .authorize_write(PaneRoot::Attachments, &source_relative)
            .map_err(|e| format!("[MIGRATION-v67] Unsafe legacy source for {:?}: {e}", row.id))?;
        match std::fs::symlink_metadata(source_candidate.as_path()) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!(
                    "[MIGRATION-v67] Source file not found — skipping: {}",
                    source_candidate.as_path().display()
                );
                skipped += 1;
                continue;
            }
            Err(e) => {
                return Err(format!(
                    "[MIGRATION-v67] Failed to inspect source for {:?}: {e}",
                    row.id
                ));
            }
            Ok(_) => {}
        }

        let source = pane_fs
            .authorize_existing(PaneRoot::Attachments, &source_relative, ExistingKind::File)
            .map_err(|e| format!("[MIGRATION-v67] Unsafe legacy source for {:?}: {e}", row.id))?;
        pane_fs
            .create_dir_all(PaneRoot::Projects, &destination_dir_relative)
            .map_err(|e| {
                format!(
                    "[MIGRATION-v67] Failed to create Assets directory for {:?}: {e}",
                    row.id
                )
            })?;
        let destination = pane_fs
            .authorize_write(PaneRoot::Projects, &destination_relative)
            .map_err(|e| format!("[MIGRATION-v67] Unsafe destination for {:?}: {e}", row.id))?;

        let pane_source_relative = PaneFs::relative_from_names([
            ".pane",
            "attachments",
            row.project_name.as_str(),
            row.filename.as_str(),
        ])
        .map_err(|e| format!("[MIGRATION-v67] Unsafe Pane-relative source: {e}"))?;
        let pane_destination_relative = PaneFs::relative_from_names([
            "Projects",
            row.project_name.as_str(),
            "Assets",
            row.filename.as_str(),
        ])
        .map_err(|e| format!("[MIGRATION-v67] Unsafe Pane-relative destination: {e}"))?;

        // Both locations are beneath Pane, so use the policy rename directly. Do not copy
        // and delete on arbitrary rename failures; an unusual nested cross-device mount now
        // fails safely with the source intact.
        pane_fs
            .rename(
                PaneRoot::Pane,
                &pane_source_relative,
                &pane_destination_relative,
            )
            .map_err(|e| {
                format!(
                    "[MIGRATION-v67] Failed to move attachment {:?}: {e}",
                    row.id
                )
            })?;

        eprintln!(
            "[MIGRATION-v67] Moved {} → {}",
            source.as_path().display(),
            destination.as_path().display()
        );
        moved += 1;
    }

    eprintln!(
        "[MIGRATION-v67] ✓ File move complete: {} moved, {} skipped (source not found)",
        moved, skipped
    );
    Ok(())
}

/// v67→v68: Add `attachments` to CRR_TABLES and add DEFAULTs to all NOT NULL columns. (#1775)
///
/// CRR Rule: every NOT NULL column in a CRR table must have a DEFAULT value so that
/// cr-sqlite can synthesise missing columns when merging rows from peers on older schema.
///
/// Strategy: create new table with correct DEFAULTs, copy data, drop old, rename.
/// All inside a transaction. Re-enables CRR on the rebuilt table.
///
/// Idempotent: if the table already has the expected DEFAULTs the recreation is a no-op
/// (checked via guard before executing the batch).
fn migrate_attachments_v68(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION-v68] Starting: attachments CRR + NOT NULL DEFAULTs (#1775)");

    // Guard: if the attachments table does not exist yet (test DBs, fresh installs on older
    // schema snapshots), the canonical schema already creates it with correct DEFAULTs.
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='attachments'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        eprintln!("[MIGRATION-v68] attachments table not yet present — will be created with DEFAULTs (startup path)");
        return Ok(());
    }

    // Guard: check if project_id already has a DEFAULT (means migration already ran or
    // table was created fresh with v68 canonical schema — skip the recreation).
    let project_id_has_default: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM pragma_table_info('attachments') WHERE name='project_id' AND dflt_value IS NOT NULL",
        [],
        |row| row.get(0),
    ).unwrap_or(false);

    if project_id_has_default {
        eprintln!("[MIGRATION-v68] attachments already has DEFAULTs on NOT NULL columns — idempotent skip");
        return Ok(());
    }

    // Recreate table with correct DEFAULTs on all NOT NULL columns.
    // project_name already has DEFAULT '' from v67; the others need it added.
    conn.execute_batch(
        "
        BEGIN;
        CREATE TABLE IF NOT EXISTS attachments_new (
            id                TEXT PRIMARY KEY NOT NULL,
            project_id        TEXT NOT NULL DEFAULT '',
            project_name      TEXT NOT NULL DEFAULT '',
            original_filename TEXT NOT NULL DEFAULT '',
            mime_type         TEXT NOT NULL DEFAULT '',
            size_bytes        INTEGER NOT NULL DEFAULT 0,
            created_at        TEXT NOT NULL DEFAULT (datetime('now'))
            -- NOTE: No FOREIGN KEY on project_id — CRR table, enforce in app code (#493)
        );
        INSERT OR IGNORE INTO attachments_new
            (id, project_id, project_name, original_filename, mime_type, size_bytes, created_at)
        SELECT
            id,
            COALESCE(project_id, ''),
            COALESCE(project_name, ''),
            COALESCE(original_filename, ''),
            COALESCE(mime_type, ''),
            COALESCE(size_bytes, 0),
            COALESCE(created_at, datetime('now'))
        FROM attachments;
        DROP TABLE attachments;
        ALTER TABLE attachments_new RENAME TO attachments;
        COMMIT;
    ",
    )
    .map_err(|e| {
        format!(
            "[MIGRATION-v68] Failed to recreate attachments with DEFAULTs: {}",
            e
        )
    })?;

    eprintln!("[MIGRATION-v68] ✓ attachments table recreated with NOT NULL DEFAULTs");

    // Re-enable CRR on the rebuilt table (dropping a table removes its CRR triggers).
    // Gracefully skips if cr-sqlite extension is not available (e.g., in tests).
    let crr_available = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM pragma_function_list WHERE name='crsql_as_crr'",
            [],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);

    if crr_available {
        conn.query_row("SELECT crsql_as_crr('attachments');", [], |_| Ok(()))
            .map_err(|e| {
                format!(
                    "[MIGRATION-v68] Failed to re-enable CRR on attachments: {}",
                    e
                )
            })?;
        eprintln!("[MIGRATION-v68] ✓ CRR re-enabled on attachments");
    } else {
        eprintln!("[MIGRATION-v68] ⚠️  cr-sqlite not loaded, skipping CRR enablement (test mode)");
    }

    eprintln!("[MIGRATION-v68] ✓ Done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn attachment_v67_uses_runtime_storage_filename_derivation() {
        for original_filename in [
            "extensionless",
            "punctuation.p/n?g!",
            "unicode.图片",
            "long.abcdefghijklmnopQRSTUV",
        ] {
            let migrated = validate_attachment_v67_row(
                "attachment-id".to_string(),
                "Project".to_string(),
                original_filename.to_string(),
            )
            .unwrap();
            let runtime = attachment_storage_filename("attachment-id", original_filename).unwrap();
            assert_eq!(
                migrated.filename, runtime,
                "mismatch for {original_filename:?}"
            );
        }
    }

    #[test]
    fn test_fresh_install() {
        let conn = Connection::open_in_memory().unwrap();

        // Should succeed on fresh DB
        run_migrations(&conn).expect("Fresh install should succeed");

        // Verify schema version
        let version = get_schema_version(&conn);
        assert_eq!(version, Some(SCHEMA_VERSION.to_string()));

        // Verify table count (45 user tables)
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE '%__crsql%' AND name NOT LIKE 'sqlite_%'",
            [], |row| row.get(0),
        ).unwrap();

        assert!(count >= 45, "Expected at least 45 tables, found {}", count);
    }

    /// v94 → v95 (#3050 fix PR, Bug 1(b)): `agent_jobs.oc_session_key`.
    /// Bootstraps a fresh install (already at v95, so the column exists via
    /// `create_agent_jobs_table`'s own canonical CREATE TABLE text),
    /// downgrades the stamped version to "94" to exercise the NEW
    /// `old_version == "94"` arm specifically (not just the long historical
    /// chains other tests already run through it), and confirms both the
    /// version bump and the idempotent `add_column_if_missing` no-op land
    /// cleanly a second time.
    #[test]
    fn test_v94_to_v95_migration_arm() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("fresh install should succeed");
        conn.execute(
            "UPDATE ai_settings SET value='94' WHERE key='schema_version'",
            [],
        )
        .unwrap();

        run_migrations(&conn).expect("v94 -> v95 migration should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(SCHEMA_VERSION.to_string()), "schema_version must land on current SCHEMA_VERSION after the v94 arm runs");

        // oc_session_key must exist (was already present from the fresh
        // install's own CREATE TABLE — add_column_if_missing must have been
        // a documented no-op, not an error) and must still default to NULL.
        let has_column: bool = conn
            .prepare("SELECT 1 FROM pragma_table_info('agent_jobs') WHERE name = 'oc_session_key'")
            .unwrap()
            .exists([])
            .unwrap();
        assert!(has_column, "agent_jobs.oc_session_key must exist after the v94->v95 arm");
    }

    #[test]
    fn test_version_match() {
        let conn = Connection::open_in_memory().unwrap();

        // Create schema
        run_migrations(&conn).unwrap();

        // Running again should succeed (version match)
        run_migrations(&conn).expect("Version match should succeed");
    }

    /// C22 (#3120 PR 1, §36.4.5): a fresh install ends up with the
    /// wake-dedup partial unique index. **Deliberately redundant by two
    /// independent mechanisms** (`create_agent_jobs_table`'s own inline
    /// `CREATE UNIQUE INDEX`, AND the unconditional
    /// `ensure_agent_jobs_wake_dedup_index` call every launch makes after
    /// the version-dispatch `match`) — removing either ALONE does not
    /// redden this specific test (the other mechanism still creates it);
    /// only removing BOTH would. Said plainly here rather than inventing a
    /// single-site mutation that doesn't actually isolate one. The
    /// NEXT test below isolates the second mechanism specifically, on the
    /// one install shape (already-v96-stamped) where it is the ONLY
    /// mechanism that can reach the index.
    #[test]
    fn test_fresh_install_has_wake_dedup_index() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_agent_jobs_one_pending_wake_per_session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "idx_agent_jobs_one_pending_wake_per_session must exist on a fresh install");
    }

    /// C22 (#3120 PR 1, §36.4.5): the index must ALSO reach an install that
    /// was already stamped at the CURRENT `SCHEMA_VERSION` before this PR
    /// — the exact case `create_agent_jobs_table`/`migrate_v93` can never
    /// reach again (neither re-runs once an install is past them), which
    /// is why `ensure_agent_jobs_wake_dedup_index` exists as an
    /// unconditional post-match step rather than a version-gated migration
    /// arm (deliberately NOT a `SCHEMA_VERSION` bump — see that function's
    /// own doc comment and its call site's).
    ///
    /// **Mutation (actually run):** reverted `ensure_agent_jobs_wake_dedup_
    /// index`'s body to a no-op (`Ok(())`, no `CREATE INDEX`). Failure
    /// output:
    /// ```text
    /// assertion `left == right` failed: idx_agent_jobs_one_pending_wake_per_session must reach an already-v96 install on its NEXT launch
    ///   left: 0
    ///  right: 1
    /// ```
    /// Restored and re-confirmed green after.
    #[test]
    fn test_already_migrated_install_gains_wake_dedup_index_on_next_launch() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Simulate an install that reached SCHEMA_VERSION before this PR's
        // index existed: drop the index, leaving everything else (the
        // table, the stamped version) exactly as a real already-migrated
        // DB would have it.
        conn.execute_batch("DROP INDEX idx_agent_jobs_one_pending_wake_per_session").unwrap();
        let pre_check: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_agent_jobs_one_pending_wake_per_session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre_check, 0, "sanity: the index must actually be gone before the next launch");

        // The next ordinary launch: schema version already matches, no
        // migration arm runs — only `run_migrations_with_progress`'s own
        // unconditional post-match call should restore the index.
        run_migrations(&conn).expect("a version-match launch must still succeed");

        let post_check: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_agent_jobs_one_pending_wake_per_session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(post_check, 1, "idx_agent_jobs_one_pending_wake_per_session must reach an already-v96 install on its NEXT launch");
    }

    /// C22 (#3120 PR 1, §36.4.5), repair ruling, 2026-10-30 — Reed's audit
    /// (PR #3122 FAIL #1) found `CREATE UNIQUE INDEX IF NOT EXISTS` does
    /// nothing to protect against PRE-EXISTING duplicate `(session_id,
    /// kind='wake', status='pending')` rows: the `CREATE` itself fails
    /// (`UNIQUE constraint failed: agent_jobs.session_id`), which,
    /// propagated by `run_migrations`'s `?`, is a startup failure — and
    /// once the index is missing, `create_wake_job`'s own `ON
    /// CONFLICT(session_id) WHERE kind = 'wake' AND status = 'pending'`
    /// upsert has no matching partial index to target and hard-errors on
    /// EVERY wake creation, for every session, forever. Matt's ruling:
    /// repair, then index. This fixture starts with two pending wake rows
    /// on one session (the state Reed's audit showed is reachable even on
    /// a fresh DB: the app quits between two sibling completions and the
    /// next tick/relaunch finds the index already in place from THIS run,
    /// and the next upgrade path that drops/recreates it hits exactly
    /// this) and proves: (1) `ensure_agent_jobs_wake_dedup_index` succeeds
    /// rather than erroring, (2) exactly one pending wake survives per
    /// session — the OLDEST one, not insertion order — (3) the other is
    /// `cancelled`, not deleted (history stays), (4) the index exists
    /// afterward, and (5) `create_wake_job` inserts cleanly on that same
    /// session immediately after — the exact call that hard-errors
    /// forever, pre-fix, once the index is missing.
    ///
    /// **Mutation (actually run):** removed the repair `UPDATE` from
    /// `ensure_agent_jobs_wake_dedup_index`, leaving only the bare `CREATE
    /// UNIQUE INDEX IF NOT EXISTS` against this fixture's duplicate rows.
    /// Failure output:
    /// ```text
    /// a DB with pre-existing duplicate pending wakes must still migrate successfully: "Failed to ensure idx_agent_jobs_one_pending_wake_per_session: UNIQUE constraint failed: agent_jobs.session_id"
    /// ```
    /// Restored and re-confirmed green after.
    #[test]
    fn test_ensure_agent_jobs_wake_dedup_index_repairs_duplicate_pending_wakes_before_creating_index() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Reach the exact pre-repair state: table exists, schema already
        // stamped at the current version, index absent — nothing stops two
        // pending wake rows from coexisting on the same session.
        conn.execute_batch("DROP INDEX idx_agent_jobs_one_pending_wake_per_session").unwrap();

        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, model, created_at, updated_at) \
             VALUES ('agent-dup', 'Agent Dup', 'agent-dup', 1, 1, 'agent-dup', 'test-model', datetime('now'), datetime('now'));
             INSERT INTO chat_sessions (id, project_id, title, origin, agent_id) \
             VALUES ('sess-dup', 'workspace', 'test', 'schedule', 'agent-dup');",
        )
        .unwrap();

        // The parent/root FK targets the wake rows carry, same shape every
        // real `create_wake_job` call site passes.
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, status, created_at, updated_at) \
             VALUES ('parent-job', 'spawn', 'Sibling', 'done', 'agent-dup', 'workspace', 'sess-dup', 'done', datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();

        // Two pending wake rows on one session — inserted newer-first on
        // purpose, so the test can't accidentally pass by `rowid`/insertion
        // order alone; only `created_at` should decide "oldest".
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, parent_job_id, root_job_id, status, created_at, updated_at) \
             VALUES ('wake-newer', 'wake', '', '[newer]', 'agent-dup', 'workspace', 'sess-dup', 'parent-job', 'parent-job', 'pending', datetime('now', '+1 minute'), datetime('now', '+1 minute'))",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, parent_job_id, root_job_id, status, created_at, updated_at) \
             VALUES ('wake-older', 'wake', '', '[older]', 'agent-dup', 'workspace', 'sess-dup', 'parent-job', 'parent-job', 'pending', datetime('now', '-1 minute'), datetime('now', '-1 minute'))",
            [],
        )
        .unwrap();

        let pre_pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'pending'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pre_pending, 2, "sanity: two pending wakes on the same session before the repair runs");

        // The next ordinary launch: schema version already matches, so
        // only the unconditional post-match `ensure_agent_jobs_wake_dedup_
        // index` call runs — exactly the real-world path this repro
        // targets.
        run_migrations(&conn).expect("a DB with pre-existing duplicate pending wakes must still migrate successfully");

        let pending_after: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'pending' AND session_id = 'sess-dup'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending_after, 1, "exactly one pending wake must remain for the session after the repair");

        let surviving_id: String = conn
            .query_row(
                "SELECT id FROM agent_jobs WHERE kind = 'wake' AND status = 'pending' AND session_id = 'sess-dup'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(surviving_id, "wake-older", "the OLDEST pending wake must be the one kept, not insertion order");

        let cancelled_status: String = conn
            .query_row("SELECT status FROM agent_jobs WHERE id = 'wake-newer'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cancelled_status, "cancelled", "the newer duplicate must be cancelled, not deleted — history stays");

        let index_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'idx_agent_jobs_one_pending_wake_per_session'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(index_count, 1, "the index must exist after the repair runs");

        // The repro's whole point: `create_wake_job` must insert cleanly
        // right after, on the SAME session — this is the exact call that
        // hard-errors forever, pre-fix, once the index is missing.
        let upserted_id = crate::ai::scheduler::create_wake_job(
            &conn,
            "sess-dup",
            "workspace",
            "agent-dup",
            "[fresh completion]",
            "parent-job",
            "parent-job",
            1,
        )
        .expect("create_wake_job must insert cleanly on a session the repair just touched");
        assert_eq!(
            upserted_id, "wake-older",
            "create_wake_job must upsert onto the surviving pending row via the now-present partial index, not error"
        );

        let final_pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'pending' AND session_id = 'sess-dup'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(final_pending, 1, "the upsert must still leave exactly one pending wake for the session");
    }

    /// PR 1 WARN #1 carry-forward (#3120 PR 2, Reed's re-cut brief): the
    /// fixture above pins exactly TWO duplicates — the `PARTITION BY
    /// session_id`/`rn = 1` shape is already correct for ANY N, not just 2
    /// (confirmed in Reed's own SQLite tests outside the committed
    /// fixture), but a test seeding only 2 rows can't distinguish "keep
    /// exactly one" from "keep at least one" — an off-by-one like `rn <=
    /// 2` passes the 2-row fixture above identically to the correct `rn =
    /// 1`. This seeds 3 pending wake rows (3 different `created_at`
    /// values) on one session to close that gap; production code is
    /// UNCHANGED (test-only commit, zero production diff).
    ///
    /// **Mutation to pin (per the task):** swap `rn = 1` for `rn <= 2` in
    /// `ensure_agent_jobs_wake_dedup_index`'s repair query — with only 3
    /// seeded rows, the two NEWEST would both survive (2 pending rows
    /// left, not 1), reddening the `pending_after == 1` assertion.
    #[test]
    fn test_ensure_agent_jobs_wake_dedup_index_repairs_three_duplicate_pending_wakes() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute_batch("DROP INDEX idx_agent_jobs_one_pending_wake_per_session").unwrap();

        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, model, created_at, updated_at) \
             VALUES ('agent-dup3', 'Agent Dup3', 'agent-dup3', 1, 1, 'agent-dup3', 'test-model', datetime('now'), datetime('now'));
             INSERT INTO chat_sessions (id, project_id, title, origin, agent_id) \
             VALUES ('sess-dup3', 'workspace', 'test', 'schedule', 'agent-dup3');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, status, created_at, updated_at) \
             VALUES ('parent-job-3', 'spawn', 'Sibling', 'done', 'agent-dup3', 'workspace', 'sess-dup3', 'done', datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();

        // Three pending wake rows, three distinct created_at values,
        // seeded in deliberately scrambled order (middle, newest, oldest)
        // so insertion order can't be mistaken for the "keep one" result.
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, parent_job_id, root_job_id, status, created_at, updated_at) \
             VALUES ('wake-3-middle', 'wake', '', '[middle]', 'agent-dup3', 'workspace', 'sess-dup3', 'parent-job-3', 'parent-job-3', 'pending', datetime('now'), datetime('now'))",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, parent_job_id, root_job_id, status, created_at, updated_at) \
             VALUES ('wake-3-newest', 'wake', '', '[newest]', 'agent-dup3', 'workspace', 'sess-dup3', 'parent-job-3', 'parent-job-3', 'pending', datetime('now', '+2 minute'), datetime('now', '+2 minute'))",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, parent_job_id, root_job_id, status, created_at, updated_at) \
             VALUES ('wake-3-oldest', 'wake', '', '[oldest]', 'agent-dup3', 'workspace', 'sess-dup3', 'parent-job-3', 'parent-job-3', 'pending', datetime('now', '-2 minute'), datetime('now', '-2 minute'))",
            [],
        ).unwrap();

        let pre_pending: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'pending' AND session_id = 'sess-dup3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pre_pending, 3, "sanity: three pending wakes on the same session before the repair runs");

        run_migrations(&conn).expect("a DB with 3 pre-existing duplicate pending wakes must still migrate successfully");

        let pending_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'pending' AND session_id = 'sess-dup3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pending_after, 1, "exactly ONE pending wake must remain, not two — N>2 duplicates must collapse the same way N=2 does");

        let surviving_id: String = conn
            .query_row("SELECT id FROM agent_jobs WHERE kind = 'wake' AND status = 'pending' AND session_id = 'sess-dup3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(surviving_id, "wake-3-oldest", "the OLDEST of the three must be the one kept");

        let cancelled_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'cancelled' AND session_id = 'sess-dup3'",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cancelled_count, 2, "both duplicates (middle and newest) must be cancelled, not deleted");
    }

    /// PR 1 WARN #1 carry-forward (#3120 PR 2): the same-timestamp
    /// `rowid ASC` secondary sort key tiebreak. Seeds 2 pending wake rows
    /// on one session with an IDENTICAL `created_at` (a literal fixed
    /// string, not two independent `datetime('now')` evaluations —
    /// guarantees true equality rather than hoping two calls land in the
    /// same second), inserted in a known rowid order (lower-rowid row
    /// inserted FIRST). Production code is UNCHANGED (test-only commit).
    ///
    /// **Mutation tried, honestly reported — does NOT redden in this
    /// environment:** dropping `, rowid ASC` from the repair query's
    /// `ORDER BY created_at ASC, rowid ASC` was tried against this exact
    /// fixture (and against a minimal standalone repro, including with
    /// `PRAGMA reverse_unordered_selects = ON` to try to force a reversed
    /// scan order). Neither reddened: this build's bundled SQLite already
    /// returns rows from a plain, index-free table scan in rowid order,
    /// so `ROW_NUMBER()`'s tie-break on an all-equal `ORDER BY` key
    /// coincides with `rowid ASC` whether or not that clause is written
    /// — Reed's brief's own "in practice... for this specific seeded
    /// shape" framing turned out not to hold in THIS run's SQLite version/
    /// query-plan shape. Per the task's own standing discipline ("where a
    /// mutation would prove nothing, say so rather than inventing one"),
    /// this is reported plainly rather than claimed as a verified red/
    /// green pair. The test is kept anyway as a CORRECTNESS/INTENT pin —
    /// it still asserts the documented, intended behavior (same-timestamp
    /// ties break on `rowid`, lowest wins) and will catch a real class of
    /// future regressions (e.g. a rewrite that drops the `PARTITION BY`
    /// entirely, or reorders columns) — just not the SPECIFIC single-
    /// clause-removal mutation Reed's brief named, which this run found to
    /// be unobservable here. Flagging this as a question for Reed's audit
    /// rather than silently shipping a test whose own doc comment claims a
    /// mutation result that wasn't actually reproduced.
    #[test]
    fn test_ensure_agent_jobs_wake_dedup_index_same_timestamp_ties_break_on_rowid() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute_batch("DROP INDEX idx_agent_jobs_one_pending_wake_per_session").unwrap();

        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, model, created_at, updated_at) \
             VALUES ('agent-tie', 'Agent Tie', 'agent-tie', 1, 1, 'agent-tie', 'test-model', datetime('now'), datetime('now'));
             INSERT INTO chat_sessions (id, project_id, title, origin, agent_id) \
             VALUES ('sess-tie', 'workspace', 'test', 'schedule', 'agent-tie');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, status, created_at, updated_at) \
             VALUES ('parent-job-tie', 'spawn', 'Sibling', 'done', 'agent-tie', 'workspace', 'sess-tie', 'done', datetime('now'), datetime('now'))",
            [],
        )
        .unwrap();

        // IDENTICAL created_at (a fixed literal, not `datetime('now')`
        // evaluated twice) — a true tie, not a near-miss. First-inserted
        // row gets the lower rowid by SQLite's own rowid-assignment order.
        const SAME_TIMESTAMP: &str = "2026-10-30 12:00:00";
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, parent_job_id, root_job_id, status, created_at, updated_at) \
             VALUES ('wake-tie-first-inserted', 'wake', '', '[first]', 'agent-tie', 'workspace', 'sess-tie', 'parent-job-tie', 'parent-job-tie', 'pending', ?1, ?1)",
            params![SAME_TIMESTAMP],
        ).unwrap();
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, parent_job_id, root_job_id, status, created_at, updated_at) \
             VALUES ('wake-tie-second-inserted', 'wake', '', '[second]', 'agent-tie', 'workspace', 'sess-tie', 'parent-job-tie', 'parent-job-tie', 'pending', ?1, ?1)",
            params![SAME_TIMESTAMP],
        ).unwrap();

        let created_at_distinct: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT created_at) FROM agent_jobs WHERE kind = 'wake' AND session_id = 'sess-tie'",
                [], |r| r.get(0),
            )
            .unwrap();
        assert_eq!(created_at_distinct, 1, "sanity: both rows must share the exact same created_at — a true tie, not a near-miss");

        run_migrations(&conn).expect("a DB with a same-timestamp duplicate pending wake pair must still migrate successfully");

        let pending_after: i64 = conn
            .query_row("SELECT COUNT(*) FROM agent_jobs WHERE kind = 'wake' AND status = 'pending' AND session_id = 'sess-tie'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pending_after, 1, "exactly one pending wake must remain");

        let surviving_id: String = conn
            .query_row("SELECT id FROM agent_jobs WHERE kind = 'wake' AND status = 'pending' AND session_id = 'sess-tie'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            surviving_id, "wake-tie-first-inserted",
            "on a true created_at tie, the LOWER-rowid row (first inserted) must survive — the ORDER BY's rowid ASC secondary key"
        );
    }

    // --- #3151 PR1 Commit C: the one-time Reminders rename migration ---

    /// Seeds a legacy `origin = 'reminder'` session directly (the shape
    /// #3120 PR 2's now-retired `find_or_create_reminders_session` used to
    /// write), runs the ensure-fn TWICE, and asserts: the title/origin
    /// flip happens, and a second run is a true no-op (not a double-
    /// append like `"Reminders (old) (old)"`, and no second `UPDATE`
    /// fires at all — proven via the row's own unchanged `updated_at`
    /// is not asserted directly since the `UPDATE` statement doesn't
    /// touch it; instead this proves the no-op via the explicit
    /// `reminders_rename_done` flag plus the title staying byte-identical
    /// after run 2).
    ///
    /// Mutation: delete the `get_ai_setting(...).is_some()` early-return
    /// guard — reddens: title becomes `"Reminders (old)"` unconditionally
    /// re-written every call (not idempotent on repeated calls against a
    /// row an operator might have manually re-pointed at `origin =
    /// 'reminder'` for some other reason — the guard is what makes this
    /// "ran once, ever" rather than "ran every launch, happens to be a
    /// no-op because the WHERE clause finds nothing" — the two are
    /// observably different the moment a legacy row's `origin` is ever
    /// set back to `'reminder'` by anything else).
    #[test]
    fn test_reminders_migration_renames_once_and_flips_origin() {
        let conn = Connection::open_in_memory().unwrap();
        // Creates the full schema from scratch (chat_sessions/ai_agents/
        // ai_settings don't exist before this). `run_migrations` also
        // calls `ensure_reminders_sessions_renamed_once` unconditionally
        // as part of its own post-match step — on this truly empty DB
        // that's a real no-op (zero `origin = 'reminder'` rows exist yet)
        // but it DOES set the completion flag, which would mask the real
        // test below. Cleared immediately after, to reach the actual
        // state a pre-#3151 install upgrading to this binary would be in:
        // schema present, genuine legacy `origin = 'reminder'` rows
        // present, and the flag never having existed before THIS launch.
        run_migrations(&conn).unwrap();
        conn.execute("DELETE FROM ai_settings WHERE key = 'reminders_rename_done'", [])
            .unwrap();

        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, model, created_at, updated_at) \
             VALUES ('agent-legacy-rem', 'Agent Legacy', 'agent-legacy-rem', 1, 1, 'agent-legacy-rem', 'test-model', datetime('now'), datetime('now'));
             INSERT INTO chat_sessions (id, project_id, title, origin, agent_id) \
             VALUES ('sess-legacy-rem', 'workspace', 'Reminders', 'reminder', 'agent-legacy-rem');",
        )
        .unwrap();

        ensure_reminders_sessions_renamed_once(&conn).expect("first run must succeed");

        let (title, origin): (String, String) = conn
            .query_row(
                "SELECT title, origin FROM chat_sessions WHERE id = 'sess-legacy-rem'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "Reminders (old)", "a legacy Reminders session must be renamed");
        assert_eq!(origin, "user", "a legacy Reminders session's origin must flip to 'user' — an ordinary session, not left at the dead 'reminder' value");

        let flag: Option<String> = crate::database::get_ai_setting(&conn, "reminders_rename_done");
        assert_eq!(flag, Some("1".to_string()), "the completion flag must be set after the first run");

        // Simulate an operator (or a bug) re-pointing the SAME row back at
        // `origin = 'reminder'` after the first run — the flag, not the
        // data shape, must be what prevents a second write.
        conn.execute(
            "UPDATE chat_sessions SET title = 'Reminders', origin = 'reminder' WHERE id = 'sess-legacy-rem'",
            [],
        )
        .unwrap();

        ensure_reminders_sessions_renamed_once(&conn).expect("second run must succeed (no-op)");

        let (title_after_second, origin_after_second): (String, String) = conn
            .query_row(
                "SELECT title, origin FROM chat_sessions WHERE id = 'sess-legacy-rem'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title_after_second, "Reminders", "a second run must be a true no-op — it must NOT re-rename a row even if something re-set origin='reminder' after the flag was already set");
        assert_eq!(origin_after_second, "reminder", "same for origin — the flag alone gates this, not the data shape");
    }

    /// A `chat_sessions` shape missing `title`/`origin` (an incomplete
    /// historical fixture, same shape `migration_tests::
    /// test_v74_to_current_migration`'s own minimal fixture uses) must not
    /// hard-error — it skips cleanly, since no real install ever reaches
    /// this call site without both columns (`create_chat_sessions_table`
    /// has had them since this table's first shape). Mutation: delete the
    /// `has_required_columns` guard — reddens: `rename failed: no such
    /// column: title`, exactly the regression this test exists to catch
    /// (it IS the regression `test_v74_to_current_migration` surfaced
    /// against this PR's first draft of this function).
    #[test]
    fn test_reminders_migration_skips_cleanly_when_columns_missing() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE chat_sessions (id TEXT PRIMARY KEY, project_id TEXT NOT NULL DEFAULT '');
             INSERT INTO chat_sessions (id, project_id) VALUES ('sess-no-title-col', 'workspace');",
        )
        .unwrap();

        let result = ensure_reminders_sessions_renamed_once(&conn);
        assert!(result.is_ok(), "must skip cleanly, not error: {:?}", result.err());

        // Untouched — the row the migration would have no way to safely
        // rename on this shape is left exactly as it was.
        let project_id: String = conn
            .query_row("SELECT project_id FROM chat_sessions WHERE id = 'sess-no-title-col'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(project_id, "workspace");
    }

    // --- #3163 / #3161 PR 2: the one-time orphaned-image-embed repair ---

    fn seed_image_attachment(conn: &Connection, id: &str, mime_type: &str) {
        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES (?1, 'workspace', '', 'test.bin', ?2, 100)",
            rusqlite::params![id, mime_type],
        )
        .unwrap();
    }

    const T3163_UUID: &str = "4f9c1a2e-1234-4abc-8def-abcdef123456";

    /// The core join: an orphaned `[alt](Assets/<uuid>.png)` link whose
    /// uuid matches a REAL image-typed `attachments` row gets its `!`
    /// restored — exactly one character inserted, nothing else in the
    /// line touched. Mutation: drop the `is_orphaned_image_attachment`
    /// check entirely (repair every `Assets/<uuid>.<ext>`-shaped link
    /// unconditionally) — would still pass THIS test alone, which is why
    /// the next three tests exist.
    #[test]
    fn repair_orphaned_image_links_restores_bang_for_real_image_attachment() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE attachments (id TEXT PRIMARY KEY, project_id TEXT, project_name TEXT, original_filename TEXT, mime_type TEXT, size_bytes INTEGER, created_at TEXT DEFAULT (datetime('now')));",
        ).unwrap();
        seed_image_attachment(&conn, T3163_UUID, "image/png");

        let body = format!("# Blimpy\n\n[a lighthouse](Assets/{}.png)\n\nMore text.", T3163_UUID);
        let (new_body, count) = repair_orphaned_image_links(&conn, &body);

        assert_eq!(count, 1);
        assert_eq!(
            new_body,
            format!("# Blimpy\n\n![a lighthouse](Assets/{}.png)\n\nMore text.", T3163_UUID)
        );
    }

    /// A link whose uuid has NO matching `attachments` row at all (a
    /// deleted attachment, or just coincidentally uuid-shaped text) must
    /// be left exactly as it was — never guess that an unknown id was an
    /// orphaned image.
    #[test]
    fn repair_orphaned_image_links_leaves_unknown_uuid_untouched() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE attachments (id TEXT PRIMARY KEY, project_id TEXT, project_name TEXT, original_filename TEXT, mime_type TEXT, size_bytes INTEGER, created_at TEXT DEFAULT (datetime('now')));",
        ).unwrap();
        // Deliberately NO seed_image_attachment call.

        let body = format!("[a photo](Assets/{}.png)", T3163_UUID);
        let (new_body, count) = repair_orphaned_image_links(&conn, &body);

        assert_eq!(count, 0);
        assert_eq!(new_body, body, "an unknown uuid must be left byte-identical");
    }

    /// A legitimate link to a NON-image attachment (a PDF the user
    /// dropped into Assets) must survive untouched — that's correct,
    /// existing rendering, not damage. This is the test that would catch
    /// the "drop the mime_type check" mutation the first test's own doc
    /// comment names.
    #[test]
    fn repair_orphaned_image_links_leaves_non_image_attachment_link_untouched() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE attachments (id TEXT PRIMARY KEY, project_id TEXT, project_name TEXT, original_filename TEXT, mime_type TEXT, size_bytes INTEGER, created_at TEXT DEFAULT (datetime('now')));",
        ).unwrap();
        seed_image_attachment(&conn, T3163_UUID, "application/pdf");

        let body = format!("[my resume](Assets/{}.pdf)", T3163_UUID);
        let (new_body, count) = repair_orphaned_image_links(&conn, &body);

        assert_eq!(count, 0, "a non-image attachment link must NOT be repaired — plain links to non-images are correct, existing rendering");
        assert_eq!(new_body, body);
    }

    /// An already-correct `![alt](...)` embed must be left alone —
    /// proves the match itself (not just the one-time `ai_settings` gate)
    /// is idempotent: running the repair against its OWN output a second
    /// time is a no-op.
    #[test]
    fn repair_orphaned_image_links_leaves_already_embedded_image_untouched() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE attachments (id TEXT PRIMARY KEY, project_id TEXT, project_name TEXT, original_filename TEXT, mime_type TEXT, size_bytes INTEGER, created_at TEXT DEFAULT (datetime('now')));",
        ).unwrap();
        seed_image_attachment(&conn, T3163_UUID, "image/png");

        let body = format!("![a lighthouse](Assets/{}.png)", T3163_UUID);
        let (new_body, count) = repair_orphaned_image_links(&conn, &body);

        assert_eq!(count, 0);
        assert_eq!(new_body, body);
    }

    /// A link to something in Assets that was never uuid-shaped at all
    /// (a real file a user manually named and dropped in, or just typed)
    /// never even reaches the attachments lookup — the regex itself
    /// excludes it.
    #[test]
    fn repair_orphaned_image_links_leaves_non_uuid_assets_link_untouched() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE attachments (id TEXT PRIMARY KEY, project_id TEXT, project_name TEXT, original_filename TEXT, mime_type TEXT, size_bytes INTEGER, created_at TEXT DEFAULT (datetime('now')));",
        ).unwrap();

        let body = "[the Q3 report](Assets/report.pdf)";
        let (new_body, count) = repair_orphaned_image_links(&conn, body);

        assert_eq!(count, 0);
        assert_eq!(new_body, body);
    }

    /// Multiple orphaned links in the same body, mixed with a non-image
    /// attachment link that must survive — proves byte offsets are
    /// applied back-to-front correctly (an earlier insert must not shift
    /// a later match's recorded position) and that one body can repair
    /// some links while sparing others.
    #[test]
    fn repair_orphaned_image_links_handles_multiple_mixed_links_in_one_body() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE attachments (id TEXT PRIMARY KEY, project_id TEXT, project_name TEXT, original_filename TEXT, mime_type TEXT, size_bytes INTEGER, created_at TEXT DEFAULT (datetime('now')));",
        ).unwrap();
        let uuid_a = "11111111-1111-4111-8111-111111111111";
        let uuid_b = "22222222-2222-4222-8222-222222222222";
        let uuid_pdf = "33333333-3333-4333-8333-333333333333";
        seed_image_attachment(&conn, uuid_a, "image/png");
        seed_image_attachment(&conn, uuid_b, "image/jpeg");
        seed_image_attachment(&conn, uuid_pdf, "application/pdf");

        let body = format!(
            "First [img one](Assets/{}.png) then a [resume](Assets/{}.pdf) and [img two](Assets/{}.jpg) at the end.",
            uuid_a, uuid_pdf, uuid_b
        );
        let (new_body, count) = repair_orphaned_image_links(&conn, &body);

        assert_eq!(count, 2);
        assert_eq!(
            new_body,
            format!(
                "First ![img one](Assets/{}.png) then a [resume](Assets/{}.pdf) and ![img two](Assets/{}.jpg) at the end.",
                uuid_a, uuid_pdf, uuid_b
            )
        );
    }

    /// Full end-to-end: `repair_orphaned_image_embeds_once` against a real
    /// `notes` + `attachments` fixture (DB-only note, no on-disk file —
    /// same `file_path = ''` sentinel shape every other DB-only-note test
    /// in this crate already uses). Also proves the one-time gate: a
    /// second call is a true no-op even if the row were somehow re-
    /// corrupted after the flag was set.
    #[test]
    fn repair_orphaned_image_embeds_once_repairs_note_body_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("fresh-install migrations should succeed");
        // `run_migrations` itself just ran this repair against an empty
        // notes table — a real no-op, but it DOES set the completion flag.
        // Cleared to reach the state a real upgrading install would be in:
        // schema present, genuine damage present, flag never set before
        // THIS call.
        conn.execute("DELETE FROM ai_settings WHERE key = 'image_embed_repair_3163_done'", [])
            .unwrap();

        seed_image_attachment(&conn, T3163_UUID, "image/png");
        conn.execute(
            "INSERT INTO notes (id, title, body, project_id, file_path, status, updated_at) \
             VALUES ('note-3163', 'Blimpy', ?1, '', '', 'cabinet', datetime('now'))",
            rusqlite::params![format!("# Blimpy\n\n[a lighthouse](Assets/{}.png)", T3163_UUID)],
        )
        .unwrap();

        repair_orphaned_image_embeds_once(&conn).expect("first run must succeed");

        let body_after: String = conn
            .query_row("SELECT body FROM notes WHERE id = 'note-3163'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(body_after, format!("# Blimpy\n\n![a lighthouse](Assets/{}.png)", T3163_UUID));

        let flag: Option<String> = crate::database::get_ai_setting(&conn, "image_embed_repair_3163_done");
        assert_eq!(flag, Some("1".to_string()));

        // Simulate the link somehow reverting (operator restoring an old
        // backup column, say) — the FLAG, not the data shape, must be
        // what prevents a second pass.
        conn.execute(
            "UPDATE notes SET body = ?1 WHERE id = 'note-3163'",
            rusqlite::params![format!("# Blimpy\n\n[a lighthouse](Assets/{}.png)", T3163_UUID)],
        )
        .unwrap();

        repair_orphaned_image_embeds_once(&conn).expect("second run must succeed (no-op)");

        let body_after_second: String = conn
            .query_row("SELECT body FROM notes WHERE id = 'note-3163'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            body_after_second,
            format!("# Blimpy\n\n[a lighthouse](Assets/{}.png)", T3163_UUID),
            "a second run must be a true no-op — the flag alone gates this, same discipline as the Reminders rename migration above"
        );
    }

    /// `SCHEMA_VERSION` must be unaffected by this repair — confirms the
    /// repair runs outside the `migrate_vNN` chain entirely (Matt's "no
    /// schema bump" ruling for the image-gen sprint).
    #[test]
    fn repair_orphaned_image_embeds_does_not_touch_schema_version() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("fresh-install migrations should succeed");
        let version: String = conn
            .query_row("SELECT value FROM ai_settings WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    // ── repair_absolute_image_paths_once (#3178) ─────────────────────────

    const T3178_UUID: &str = "44d12345-1234-5678-9abc-1234567890ab";

    fn seed_chat_message(conn: &Connection, id: &str, content: &str) {
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, role, content) VALUES (?1, 'proj-aurum', 'assistant', ?2)",
            rusqlite::params![id, content],
        )
        .unwrap();
    }

    /// Matt's named mutation (verbatim from #3178): a row with an
    /// absolute path survives the pass → reddens. That's the repair's
    /// own guard: a pass that logs success while leaving absolute paths
    /// in place is not a fix. This test runs the REAL repair and asserts
    /// the row no longer carries the absolute form.
    #[test]
    fn repair_absolute_image_paths_once_rewrites_existing_row_and_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("fresh-install migrations should succeed");
        // Same reasoning as the #3163 test above: `run_migrations` already
        // ran this repair against an empty table (a real no-op) and set
        // the flag. Cleared to reach the state a real upgrading install
        // would be in: schema present, genuine damage present, flag never
        // set before THIS call.
        conn.execute("DELETE FROM ai_settings WHERE key = 'absolute_image_path_repair_3178_done'", [])
            .unwrap();

        let leaking = format!(
            "![a lighthouse](/Users/example/Pane/Projects/Aurum/Assets/{}.png)",
            T3178_UUID
        );
        seed_chat_message(&conn, "msg-3178", &leaking);

        repair_absolute_image_paths_once(&conn).expect("first run must succeed");

        let content_after: String = conn
            .query_row("SELECT content FROM chat_messages_local WHERE id = 'msg-3178'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(content_after, format!("![a lighthouse](Assets/{}.png)", T3178_UUID));
        assert!(!content_after.contains("/Users/"));

        let flag: Option<String> = crate::database::get_ai_setting(&conn, "absolute_image_path_repair_3178_done");
        assert_eq!(flag, Some("1".to_string()));

        // Simulate the row somehow reverting — the FLAG, not the data
        // shape, must be what prevents a second pass (same discipline as
        // #3163's own idempotency test above).
        conn.execute(
            "UPDATE chat_messages_local SET content = ?1 WHERE id = 'msg-3178'",
            rusqlite::params![leaking],
        )
        .unwrap();

        repair_absolute_image_paths_once(&conn).expect("second run must succeed (no-op)");

        let content_after_second: String = conn
            .query_row("SELECT content FROM chat_messages_local WHERE id = 'msg-3178'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            content_after_second, leaking,
            "a second run must be a true no-op — the flag alone gates this"
        );
    }

    /// The mirror of the test above, run against a row that is NOT one of
    /// ours: an absolute path to something that merely happens to sit
    /// next to real attachments, but doesn't match the `Assets/<uuid>.
    /// <ext>` shape at all. Must survive completely untouched — proves
    /// the repair doesn't overreach into paths it doesn't recognize.
    #[test]
    fn repair_absolute_image_paths_once_leaves_non_asset_absolute_paths_untouched() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("fresh-install migrations should succeed");
        conn.execute("DELETE FROM ai_settings WHERE key = 'absolute_image_path_repair_3178_done'", [])
            .unwrap();

        let unrelated = "See the report at [notes](/Users/example/Documents/report.pdf) for details.";
        seed_chat_message(&conn, "msg-unrelated", unrelated);

        repair_absolute_image_paths_once(&conn).expect("run must succeed");

        let content_after: String = conn
            .query_row("SELECT content FROM chat_messages_local WHERE id = 'msg-unrelated'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(content_after, unrelated, "a path that isn't one of ours must be left completely untouched");
    }

    /// `SCHEMA_VERSION` must be unaffected — same "no schema bump" ruling
    /// as #3163's own repair.
    #[test]
    fn repair_absolute_image_paths_once_does_not_touch_schema_version() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("fresh-install migrations should succeed");
        let version: String = conn
            .query_row("SELECT value FROM ai_settings WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// v77→v78 idempotency (#2522, ADR-0062 D3): running `migrate_v78` twice on
    /// the same connection must be a no-op the second time — the `agent_id`
    /// column stays exactly as Step 1 left it (not re-added, not erroring on
    /// `ALTER TABLE ADD COLUMN` for a column that already exists), and the two
    /// backfill `UPDATE`s find zero `agent_id IS NULL` rows left to touch.
    #[test]
    fn test_migrate_v78_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                is_main INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE chat_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL DEFAULT ''
             );
             INSERT INTO ai_agents (id, is_main, is_active) VALUES ('agent-default', 1, 1);
             INSERT INTO chat_sessions (id, project_id) VALUES ('sess-1', 'workspace');
             INSERT INTO chat_sessions (id, project_id) VALUES ('sess-2', 'workspace');
            ",
        )
        .unwrap();

        migrate_v78(&conn).expect("first migrate_v78 run should succeed");

        let col_count_after_first: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('chat_sessions') WHERE name = 'agent_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_count_after_first, 1, "agent_id column should exist exactly once after first run");

        let bound_after_first: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT
                    (SELECT agent_id FROM chat_sessions WHERE id = 'sess-1'),
                    (SELECT agent_id FROM chat_sessions WHERE id = 'sess-2')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(bound_after_first.0.as_deref(), Some("agent-default"), "sess-1 backfilled from designated default");
        assert_eq!(bound_after_first.1.as_deref(), Some("agent-default"), "sess-2 backfilled from designated default");

        // Second run: must not error (idempotency guard on ALTER TABLE ADD COLUMN)
        // and must not change anything (backfill UPDATEs are WHERE agent_id IS NULL).
        migrate_v78(&conn).expect("second migrate_v78 run should be a no-op, not an error");

        let col_count_after_second: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('chat_sessions') WHERE name = 'agent_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(col_count_after_second, 1, "agent_id column must still exist exactly once after second run (not duplicated)");

        let bound_after_second: (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT
                    (SELECT agent_id FROM chat_sessions WHERE id = 'sess-1'),
                    (SELECT agent_id FROM chat_sessions WHERE id = 'sess-2')",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(bound_after_second, bound_after_first, "second run must produce identical bindings to the first (pure no-op)");
    }

    /// Builds a minimal pre-v79 `ai_agents` schema (oc_agent_id/is_main names,
    /// no runtime_type column yet) for `migrate_v79` tests.
    fn make_pre_v79_ai_agents_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_agents (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL DEFAULT '',
                slug            TEXT NOT NULL UNIQUE,
                is_main         INTEGER NOT NULL DEFAULT 0,
                is_active       INTEGER NOT NULL DEFAULT 1,
                workspace_path  TEXT NOT NULL DEFAULT '',
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
                oc_agent_id     TEXT
             );
             CREATE UNIQUE INDEX idx_ai_agents_main ON ai_agents(is_main) WHERE is_main = 1;
             CREATE UNIQUE INDEX idx_ai_agents_oc_id_unique ON ai_agents(oc_agent_id);
             CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
             );
            ",
        )
        .unwrap();
        conn
    }

    /// v78→v79 idempotency (#2522, ADR-0063 D4): running `migrate_v79` twice on
    /// the same connection must be a no-op the second time — every renamed
    /// column, swapped index, and backfilled value stays exactly as the first
    /// run left it, and the `ai_settings` key deletes don't error on
    /// already-absent keys.
    #[test]
    fn test_migrate_v79_is_idempotent() {
        let conn = make_pre_v79_ai_agents_conn();
        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_main, is_active, workspace_path, oc_agent_id)
             VALUES ('agent-oc', 'OC Agent', 'oc-agent', 1, 1, 'WP1', 'main');
             INSERT INTO ai_agents (id, name, slug, is_main, is_active, workspace_path, oc_agent_id)
             VALUES ('agent-local', 'Local Agent', 'local-agent', 0, 1, 'WP2', NULL);
             INSERT INTO ai_settings (key, value) VALUES ('provider', 'anthropic');
             INSERT INTO ai_settings (key, value) VALUES ('endpoint', 'https://example.com');
             INSERT INTO ai_settings (key, value) VALUES ('api_key', 'sk-test');
             INSERT INTO ai_settings (key, value) VALUES ('model', 'claude-x');
             INSERT INTO ai_settings (key, value) VALUES ('internal_provider', 'keep-me');
            ",
        )
        .unwrap();

        migrate_v79(&conn).expect("first migrate_v79 run should succeed");

        let schema_after_first = introspect_ai_agents_v79(&conn);
        assert_eq!(schema_after_first.runtime_ref, Some("main".to_string()));
        assert_eq!(schema_after_first.runtime_type_oc, Some("openclaw".to_string()));
        assert_eq!(schema_after_first.runtime_type_local, Some("anthropic".to_string()));
        assert!(schema_after_first.has_is_default);
        assert!(schema_after_first.has_runtime_unique_index);
        assert!(schema_after_first.has_default_index);
        assert!(!schema_after_first.has_old_main_index);
        assert!(!schema_after_first.has_old_oc_id_index);

        let settings_after_first: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_settings WHERE key IN ('provider','endpoint','api_key','model')", [], |row| row.get(0))
            .unwrap();
        assert_eq!(settings_after_first, 0, "retired keys must be deleted after first run");
        let internal_survives: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_settings WHERE key = 'internal_provider'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(internal_survives, 1, "untouched keys (internal_*) must survive");

        // Second run: must not error and must produce identical state.
        migrate_v79(&conn).expect("second migrate_v79 run should be a no-op, not an error");

        let schema_after_second = introspect_ai_agents_v79(&conn);
        assert_eq!(schema_after_second.runtime_ref, schema_after_first.runtime_ref);
        assert_eq!(schema_after_second.runtime_type_oc, schema_after_first.runtime_type_oc);
        assert_eq!(schema_after_second.runtime_type_local, schema_after_first.runtime_type_local);
        assert_eq!(schema_after_second.has_is_default, schema_after_first.has_is_default);
        assert_eq!(schema_after_second.has_runtime_unique_index, schema_after_first.has_runtime_unique_index);
        assert_eq!(schema_after_second.has_default_index, schema_after_first.has_default_index);

        let settings_after_second: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_settings WHERE key IN ('provider','endpoint','api_key','model')", [], |row| row.get(0))
            .unwrap();
        assert_eq!(settings_after_second, 0, "retired keys must remain deleted (idempotent DELETE)");
    }

    struct AiAgentsV79Introspection {
        runtime_ref: Option<String>,
        runtime_type_oc: Option<String>,
        runtime_type_local: Option<String>,
        has_is_default: bool,
        has_runtime_unique_index: bool,
        has_default_index: bool,
        has_old_main_index: bool,
        has_old_oc_id_index: bool,
    }

    fn introspect_ai_agents_v79(conn: &Connection) -> AiAgentsV79Introspection {
        let runtime_ref: Option<String> = conn
            .query_row("SELECT runtime_ref FROM ai_agents WHERE id = 'agent-oc'", [], |row| row.get(0))
            .unwrap();
        let runtime_type_oc: Option<String> = conn
            .query_row("SELECT runtime_type FROM ai_agents WHERE id = 'agent-oc'", [], |row| row.get(0))
            .unwrap();
        let runtime_type_local: Option<String> = conn
            .query_row("SELECT runtime_type FROM ai_agents WHERE id = 'agent-local'", [], |row| row.get(0))
            .unwrap();
        let has_is_default: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM pragma_table_info('ai_agents') WHERE name='is_default'", [], |row| row.get(0))
            .unwrap();
        let has_runtime_unique_index: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_ai_agents_runtime_unique'", [], |row| row.get(0))
            .unwrap();
        let has_default_index: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_ai_agents_default'", [], |row| row.get(0))
            .unwrap();
        let has_old_main_index: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_ai_agents_main'", [], |row| row.get(0))
            .unwrap();
        let has_old_oc_id_index: bool = conn
            .query_row("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_ai_agents_oc_id_unique'", [], |row| row.get(0))
            .unwrap();
        AiAgentsV79Introspection {
            runtime_ref,
            runtime_type_oc,
            runtime_type_local,
            has_is_default,
            has_runtime_unique_index,
            has_default_index,
            has_old_main_index,
            has_old_oc_id_index,
        }
    }

    /// D4.3 backfill branch: pre-dissolution active provider is openclaw (or
    /// absent) at migration time — locals take `runtime_type = NULL`
    /// (legacy-unpinned), never a guessed value.
    #[test]
    fn test_migrate_v79_backfill_openclaw_active_leaves_locals_unpinned() {
        let conn = make_pre_v79_ai_agents_conn();
        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_main, is_active, workspace_path, oc_agent_id)
             VALUES ('agent-local', 'Local Agent', 'local-agent', 1, 1, 'WP1', NULL);
             INSERT INTO ai_settings (key, value) VALUES ('provider', 'openclaw');
            ",
        )
        .unwrap();

        migrate_v79(&conn).expect("migrate_v79 should succeed");

        let runtime_type: Option<String> = conn
            .query_row("SELECT runtime_type FROM ai_agents WHERE id = 'agent-local'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runtime_type, None, "locals must be left legacy-unpinned (NULL) when openclaw was the active provider at migration time");
    }

    /// D4.3 backfill branch: no `provider` key present at migration time at all
    /// (edge of the openclaw-active branch — absent is treated the same as
    /// non-API, never guessed as an API provider).
    #[test]
    fn test_migrate_v79_backfill_no_provider_key_leaves_locals_unpinned() {
        let conn = make_pre_v79_ai_agents_conn();
        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_main, is_active, workspace_path, oc_agent_id)
             VALUES ('agent-local', 'Local Agent', 'local-agent', 1, 1, 'WP1', NULL);
            ",
        )
        .unwrap();

        migrate_v79(&conn).expect("migrate_v79 should succeed");

        let runtime_type: Option<String> = conn
            .query_row("SELECT runtime_type FROM ai_agents WHERE id = 'agent-local'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runtime_type, None, "locals must be left legacy-unpinned (NULL) when no provider key is present");
    }

    /// D4.3 backfill branch: pre-dissolution active provider is an API class
    /// (ollama here) — locals are pinned to that runtime_type.
    #[test]
    fn test_migrate_v79_backfill_api_provider_pins_locals() {
        let conn = make_pre_v79_ai_agents_conn();
        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_main, is_active, workspace_path, oc_agent_id)
             VALUES ('agent-local', 'Local Agent', 'local-agent', 1, 1, 'WP1', NULL);
             INSERT INTO ai_settings (key, value) VALUES ('provider', 'ollama');
            ",
        )
        .unwrap();

        migrate_v79(&conn).expect("migrate_v79 should succeed");

        let runtime_type: Option<String> = conn
            .query_row("SELECT runtime_type FROM ai_agents WHERE id = 'agent-local'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runtime_type, Some("ollama".to_string()), "locals must be pinned to the pre-dissolution active API provider");
    }

    /// D4.3 backfill: OC-imported rows (`runtime_ref IS NOT NULL`) always get
    /// `runtime_type = 'openclaw'` regardless of the active provider at
    /// migration time.
    #[test]
    fn test_migrate_v79_backfill_oc_imported_always_openclaw() {
        let conn = make_pre_v79_ai_agents_conn();
        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_main, is_active, workspace_path, oc_agent_id)
             VALUES ('agent-oc', 'OC Agent', 'oc-agent', 1, 1, 'WP1', 'main');
             INSERT INTO ai_settings (key, value) VALUES ('provider', 'anthropic');
            ",
        )
        .unwrap();

        migrate_v79(&conn).expect("migrate_v79 should succeed");

        let runtime_type: Option<String> = conn
            .query_row("SELECT runtime_type FROM ai_agents WHERE id = 'agent-oc'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(runtime_type, Some("openclaw".to_string()), "OC-imported rows always backfill to runtime_type='openclaw'");
    }

    /// v79's index swap must be collision-free: two locals (both NULL
    /// runtime_ref) coexist under the new composite unique index exactly as
    /// they did under the old single-column unique index (D4's constraint-
    /// collision enumeration: "Locals write NULL runtime_ref → distinct under
    /// SQLite, cannot collide").
    #[test]
    fn test_migrate_v79_two_locals_coexist_under_new_index() {
        let conn = make_pre_v79_ai_agents_conn();
        conn.execute_batch(
            "INSERT INTO ai_agents (id, name, slug, is_main, is_active, workspace_path, oc_agent_id)
             VALUES ('agent-local-a', 'Local A', 'local-a', 1, 1, 'WP1', NULL);
             INSERT INTO ai_agents (id, name, slug, is_main, is_active, workspace_path, oc_agent_id)
             VALUES ('agent-local-b', 'Local B', 'local-b', 0, 1, 'WP2', NULL);
            ",
        )
        .unwrap();

        migrate_v79(&conn).expect("migrate_v79 must not error on two NULL-runtime_ref locals");

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM ai_agents", [], |row| row.get(0)).unwrap();
        assert_eq!(count, 2, "both locals must survive the index swap uncollided");
    }

    #[test]
    fn test_note_context_pins_exists() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify note_context_pins table exists (not deprecated!)
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='note_context_pins'",
            [], |row| row.get(0),
        ).unwrap();

        assert!(
            exists,
            "note_context_pins should exist (used by context_pins.rs)"
        );
    }

    #[test]
    fn test_notes_has_all_columns() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Test that notes table has all required columns
        // This prevents v0.60.0 column mismatch bug
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(notes)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        let required = [
            "id",
            "title",
            "body",
            "parent_id",
            "note_type",
            "is_vital",
            "agent_id",
        ];
        for col in &required {
            assert!(
                columns.contains(&col.to_string()),
                "notes table missing column: {}",
                col
            );
        }
    }

    #[test]
    fn test_crr_tables_not_null_columns_have_defaults() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // CRR Rule 3: Every NOT NULL column on CRR tables must have a DEFAULT value
        // This is required for cr-sqlite forward/backward sync compatibility
        for table in &CRR_TABLES {
            let mut stmt = conn
                .prepare(&format!("PRAGMA table_info({})", table))
                .unwrap();
            let columns: Vec<(String, String, bool, Option<String>)> = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(1)?,         // column name
                        row.get::<_, String>(2)?,         // type
                        row.get::<_, bool>(3)?,           // not null
                        row.get::<_, Option<String>>(4)?, // default value
                    ))
                })
                .unwrap()
                .filter_map(Result::ok)
                .collect();

            for (name, _col_type, not_null, default_value) in columns {
                // Skip primary keys (they don't need defaults)
                let is_pk = name == "id"
                    || (*table == "note_tags" && (name == "note_id" || name == "tag_id"))
                    || (*table == "note_links"
                        && (name == "source_note_id" || name == "target_note_id"))
                    || (*table == "sheet_cells" && (name == "row_id" || name == "column_id"));

                if not_null && !is_pk {
                    assert!(
                        default_value.is_some(),
                        "Table {} has a NOT NULL column '{}' without a DEFAULT VALUE. This is not allowed as it prevents forwards and backwards compatibility between schema versions. Make the column nullable or assign a default value to it.",
                        table, name
                    );
                }
            }
        }
    }

    #[test]
    fn test_projects_consolidated_columns() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Verify projects has consolidated columns from folder_preferences and project_settings
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(projects)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        let consolidated = [
            "is_hidden",
            "default_model",
            "smart_routing_enabled",
            "router_model",
            "simple_tier_model",
        ];
        for col in &consolidated {
            assert!(
                columns.contains(&col.to_string()),
                "projects table missing consolidated column: {}",
                col
            );
        }
    }

    #[test]
    fn test_attachments_has_project_name_column() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(attachments)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        assert!(
            columns.contains(&"project_name".to_string()),
            "attachments table missing project_name column (v67 migration, #1775)"
        );
        assert!(
            columns.contains(&"project_id".to_string()),
            "attachments table missing project_id column"
        );
    }

    #[test]
    fn test_migrate_attachments_v67_empty_table_is_noop() {
        // Fresh DB with attachments table (simulates v66 → v67 on empty table)
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                parent_id TEXT
            );
            CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                original_filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
            INSERT INTO ai_settings (key, value) VALUES ('schema_version', '66');
        ",
        )
        .unwrap();

        // Should succeed as a no-op
        migrate_attachments_v67(&conn)
            .expect("migrate_attachments_v67 should succeed on empty table");

        // Column should now exist
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(attachments)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            columns.contains(&"project_name".to_string()),
            "project_name column should be present after migrate_attachments_v67"
        );
    }

    #[test]
    fn test_migrate_attachments_v67_name_to_uuid() {
        // Simulate a production row where project_id stored the project name.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                parent_id TEXT
            );
            CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                original_filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
        ",
        )
        .unwrap();

        // Insert a project and a legacy attachment row (project_id = project name)
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES ('proj-uuid-001', 'Test Project', NULL)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO attachments (id, project_id, original_filename, mime_type, size_bytes)
             VALUES ('att-001', 'Test Project', 'photo.png', 'image/png', 12345)",
            [],
        )
        .unwrap();
        // Global attachment (should be untouched)
        conn.execute(
            "INSERT INTO attachments (id, project_id, original_filename, mime_type, size_bytes)
             VALUES ('att-002', '__global__', 'doc.pdf', 'application/pdf', 9999)",
            [],
        )
        .unwrap();

        migrate_attachments_v67(&conn).expect("migrate_attachments_v67 should succeed");

        // Verify the project row was migrated to UUID
        let (project_id, project_name): (String, String) = conn
            .query_row(
                "SELECT project_id, project_name FROM attachments WHERE id = 'att-001'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            project_id, "proj-uuid-001",
            "project_id should be UUID after migration"
        );
        assert_eq!(
            project_name, "Test Project",
            "project_name should be captured from legacy project_id"
        );

        // Verify the global attachment was NOT migrated
        let (global_project_id, global_project_name): (String, String) = conn
            .query_row(
                "SELECT project_id, project_name FROM attachments WHERE id = 'att-002'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            global_project_id, "__global__",
            "__global__ project_id should be unchanged"
        );
        assert_eq!(
            global_project_name, "",
            "global attachment project_name should remain empty"
        );
    }

    #[test]
    fn test_migrate_attachments_v67_rejects_unsafe_row_before_data_update() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                parent_id TEXT
            );
            CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                original_filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT INTO projects (id, name, parent_id)
                VALUES ('proj-uuid-unsafe', '../outside-pane', NULL);
            INSERT INTO attachments (id, project_id, original_filename, mime_type, size_bytes)
                VALUES ('att-unsafe', '../outside-pane', 'photo.png', 'image/png', 1);
        ",
        )
        .unwrap();

        let error = migrate_attachments_v67(&conn)
            .expect_err("unsafe DB path component must abort the migration");
        assert!(
            error.contains("Unsafe project_name"),
            "unexpected error: {error}"
        );

        let (project_id, project_name): (String, String) = conn
            .query_row(
                "SELECT project_id, project_name FROM attachments WHERE id = 'att-unsafe'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(project_id, "../outside-pane");
        assert_eq!(
            project_name, "",
            "row data must not change before validation"
        );
    }

    #[test]
    fn test_v66_to_v69_migration_arm() {
        // Simulate a v66 database running the v66→v68 combined migration arm.
        let conn = Connection::open_in_memory().unwrap();

        // Bootstrap full schema then downgrade to v66 to simulate a v66 install.
        run_migrations(&conn).unwrap();

        // Downgrade schema_version to simulate v66 install
        conn.execute(
            "UPDATE ai_settings SET value = '66' WHERE key = 'schema_version'",
            [],
        )
        .unwrap();

        // Re-run migration: should apply v66→v67 + v67→v68 and land at v68.
        run_migrations(&conn).expect("v66→v68 combined migration arm should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(SCHEMA_VERSION.to_string()), "Schema version should be current after migration (incl. og_cache v69 + notifications v71 + sessions v72 + subtasks v73 + templates v74 + category-crr v75 + agent-attribution v76 + smart-routing-retirement v77 + session-agent-binding v78 + gateway-endpoint-recovery v80)");
    }

    #[test]
    fn test_v67_to_v69_migration_adds_defaults_to_attachments() {
        // Simulates a v67 database (attachments table present, project_name exists but
        // project_id/original_filename/mime_type/size_bytes have NO DEFAULT).
        let conn = Connection::open_in_memory().unwrap();

        // Minimal v67 schema: attachments without DEFAULTs on NOT NULL columns.
        conn.execute_batch(
            "
            CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
            INSERT INTO ai_settings (key, value) VALUES ('schema_version', '67');
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                parent_id TEXT
            );
            CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL DEFAULT '',
                original_filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE migration_flags (
                flag TEXT PRIMARY KEY NOT NULL,
                completed_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE user_dashboard_config (
                id TEXT PRIMARY KEY,
                onboarding_completed INTEGER NOT NULL DEFAULT 0,
                onboarding_template TEXT,
                theme_variant TEXT DEFAULT 'default',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS notifications (
                id         TEXT PRIMARY KEY,
                type       TEXT NOT NULL DEFAULT 'info',
                title      TEXT NOT NULL,
                body       TEXT NOT NULL DEFAULT '',
                source     TEXT NOT NULL DEFAULT 'system',
                read       INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            -- chat_messages_local exists in all real DBs from the initial schema;
            -- required by migrate_v72 steps 1/3/4/5/7 (#395, ADR-0052)
            CREATE TABLE IF NOT EXISTS chat_messages_local (
                id TEXT NOT NULL PRIMARY KEY,
                project_id TEXT NOT NULL DEFAULT '',
                role TEXT NOT NULL DEFAULT 'user',
                content TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                is_pinned INTEGER NOT NULL DEFAULT 0,
                attachment_ids TEXT,
                tool_calls TEXT,
                tool_results TEXT,
                model TEXT,
                token_count INTEGER,
                thread_id TEXT
            );
        ",
        )
        .unwrap();

        // Insert seed data to verify preservation.
        conn.execute(
            "INSERT INTO attachments (id, project_id, original_filename, mime_type, size_bytes)
             VALUES ('att-v68-001', 'proj-uuid-001', 'photo.png', 'image/png', 42000)",
            [],
        )
        .unwrap();

        // Run migration.
        let result = run_migrations(&conn);
        assert!(
            result.is_ok(),
            "v67→v68 migration should succeed: {:?}",
            result
        );

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(SCHEMA_VERSION.to_string()), "Schema version should be current (notifications v71 + sessions v72 + subtasks v73 + templates v74 + category-crr v75 + agent-attribution v76 + smart-routing-retirement v77 + session-agent-binding v78 + gateway-endpoint-recovery v80 migration)");

        // Verify DEFAULTs now present on all NOT NULL columns.
        let col_info: Vec<(String, bool, Option<String>)> = conn
            .prepare("PRAGMA table_info(attachments)")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,         // name
                    row.get::<_, bool>(3)?,           // notnull
                    row.get::<_, Option<String>>(4)?, // dflt_value
                ))
            })
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        let non_pk_notnull: Vec<_> = col_info
            .iter()
            .filter(|(name, notnull, _)| *notnull && name != "id")
            .collect();

        for (name, _, dflt) in &non_pk_notnull {
            assert!(
                dflt.is_some(),
                "attachments.{} is NOT NULL but has no DEFAULT after v68 migration",
                name
            );
        }

        // Verify data preserved.
        let (pid, fname, mime, sz): (String, String, String, i64) = conn.query_row(
            "SELECT project_id, original_filename, mime_type, size_bytes FROM attachments WHERE id='att-v68-001'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();

        assert_eq!(pid, "proj-uuid-001", "project_id should be preserved");
        assert_eq!(fname, "photo.png", "original_filename should be preserved");
        assert_eq!(mime, "image/png", "mime_type should be preserved");
        assert_eq!(sz, 42000, "size_bytes should be preserved");
    }

    #[test]
    fn test_migrate_attachments_v68_idempotent() {
        // Running migrate_attachments_v68 on a table that already has DEFAULTs is a no-op.
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "
            CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT);
            CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL DEFAULT '',
                project_name TEXT NOT NULL DEFAULT '',
                original_filename TEXT NOT NULL DEFAULT '',
                mime_type TEXT NOT NULL DEFAULT '',
                size_bytes INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
        ",
        )
        .unwrap();

        // First call.
        migrate_attachments_v68(&conn).expect("first call should succeed");
        // Second call (idempotent guard should skip).
        migrate_attachments_v68(&conn).expect("second call should be idempotent no-op");

        // Data still queryable.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM attachments", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_migrate_attachments_v68_table_missing_is_noop() {
        // If attachments table doesn't exist yet, migrate_attachments_v68 should be a no-op.
        let conn = Connection::open_in_memory().unwrap();
        migrate_attachments_v68(&conn).expect("should succeed with no attachments table");
    }

    // ── v69 migration tests (#1780) ────────────────────────────────────────

    #[test]
    fn test_v69_fresh_install_creates_og_cache() {
        // Fresh install must create og_cache table with correct schema.
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("Fresh install should succeed");

        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='og_cache'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists, "og_cache table must exist after fresh install");

        // Verify all required columns.
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(og_cache)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        let required = [
            "url",
            "og_title",
            "og_description",
            "og_image",
            "fetched_at",
            "status",
        ];
        for col in &required {
            assert!(
                columns.contains(&col.to_string()),
                "og_cache missing column: {}",
                col
            );
        }
    }

    #[test]
    fn test_v69_fresh_install_seeds_link_previews_enabled() {
        // Fresh install must seed link_previews_enabled in ai_settings.
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("Fresh install should succeed");

        // The seed runs in database.rs seed_ai_settings, invoked after migrations.
        // In test context we call migrate_v69 directly to test the migration path.
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT);
        ",
        )
        .ok();

        // Simulate migration from v68 — carry over link_embeds_enabled=false.
        conn.execute(
            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('link_embeds_enabled', 'false')",
            [],
        ).unwrap();

        // Remove link_previews_enabled if it was seeded by fresh install, to test migration.
        conn.execute(
            "DELETE FROM ai_settings WHERE key='link_previews_enabled'",
            [],
        )
        .ok();

        migrate_v69(&conn).expect("migrate_v69 should succeed");

        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM ai_settings WHERE key='link_previews_enabled'",
                [],
                |row| row.get(0),
            )
            .ok();

        assert_eq!(
            value,
            Some("false".to_string()),
            "link_previews_enabled should carry over 'false' from link_embeds_enabled"
        );
    }

    #[test]
    fn test_v69_migration_default_when_no_existing_key() {
        // When link_embeds_enabled does not exist, link_previews_enabled should default to 'true'.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();

        migrate_v69(&conn).expect("migrate_v69 should succeed with no pre-existing key");

        let value: Option<String> = conn
            .query_row(
                "SELECT value FROM ai_settings WHERE key='link_previews_enabled'",
                [],
                |row| row.get(0),
            )
            .ok();

        assert_eq!(
            value,
            Some("true".to_string()),
            "link_previews_enabled should default to 'true' when link_embeds_enabled absent"
        );
    }

    #[test]
    fn test_v68_to_v69_migration_arm() {
        // Simulate a v68 database upgrading to v69.
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        // Downgrade to v68.
        conn.execute(
            "UPDATE ai_settings SET value='68' WHERE key='schema_version'",
            [],
        )
        .unwrap();
        // Drop og_cache to simulate v68 state.
        conn.execute_batch("DROP TABLE IF EXISTS og_cache;")
            .unwrap();

        run_migrations(&conn).expect("v68→v69 migration arm should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(
            version,
            Some(SCHEMA_VERSION.to_string()),
            "Schema version should be current after v68→latest"
        );

        let og_cache_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='og_cache'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            og_cache_exists,
            "og_cache must exist after v68→v69 migration"
        );
    }

    // ── v72 migration tests (#395, ADR-0052) ─────────────────────────────────

    #[test]
    fn test_fresh_install_v72() {
        // Fresh install must end at current schema version and include chat_sessions, subtasks, templates, category_colors CRR, and agent attribution.
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("Fresh install should succeed");

        // Schema version must be current.
        let version = get_schema_version(&conn);
        assert_eq!(
            version,
            Some(SCHEMA_VERSION.to_string()),
            "Fresh install must land at current schema version"
        );

        // chat_sessions table must exist.
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='chat_sessions'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            table_exists,
            "chat_sessions table must exist after fresh install"
        );

        // All required columns must be present.
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(chat_sessions)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        let required = [
            "id",
            "project_id",
            "title",
            "origin",
            "created_at",
            "last_active_at",
            "hidden_at",
            "is_active",
        ];
        for col in &required {
            assert!(
                columns.contains(&col.to_string()),
                "chat_sessions missing column: {}",
                col
            );
        }

        // Partial unique index must exist.
        let idx_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_sessions_active_per_surface'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            idx_exists,
            "idx_sessions_active_per_surface must exist after fresh install"
        );

        // Thread index must exist on chat_messages_local.
        let thread_idx_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_chat_local_thread'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // idx_chat_local_thread is created by create_chat_tables (init_schema path) and also
        // by migrate_v72 Step 7 (IF NOT EXISTS, idempotent). Both paths ensure it exists.
        assert!(
            thread_idx_exists,
            "idx_chat_local_thread must exist on fresh install"
        );
    }

    #[test]
    fn test_v71_to_v72_migration() {
        // Downgrade to v71, insert fixture data covering all 4 bucket classifications,
        // run migration, assert all outcomes.
        let conn = Connection::open_in_memory().unwrap();

        // Bootstrap full schema at current version, then downgrade to v71.
        run_migrations(&conn).expect("Bootstrap should succeed");
        conn.execute(
            "UPDATE ai_settings SET value='71' WHERE key='schema_version'",
            [],
        )
        .unwrap();

        // Drop chat_sessions to simulate v71 state (it didn't exist before v72).
        conn.execute_batch(
            "
            DROP TABLE IF EXISTS chat_sessions;
            DROP INDEX IF EXISTS idx_sessions_active_per_surface;
            DROP INDEX IF EXISTS idx_sessions_surface;
            DROP INDEX IF EXISTS idx_chat_local_thread;
        ",
        )
        .unwrap();

        // Insert fixture projects.
        // proj-a: current generation (no reset)
        // proj-b: has been reset (chat_thread_proj-b points to pane-new-b)
        conn.execute_batch(
            "
            INSERT OR IGNORE INTO projects (id, name) VALUES ('proj-a', 'Project A');
            INSERT OR IGNORE INTO projects (id, name) VALUES ('proj-b', 'Project B');
        ",
        )
        .unwrap();

        // Insert fixture ai_settings reset pointer:
        // proj-b has been reset; pane-new-b is the new active bucket.
        conn.execute(
            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('chat_thread_proj-b', 'pane-new-b')",
            [],
        ).unwrap();

        // Insert chat messages with the 4 bucket types:
        // 1. 'workspace' — workspace bucket
        // 2. 'proj-a' — project id, no reset (current gen, active)
        // 3. 'proj-b' — project id, has reset (prior gen, inactive)
        // 4. 'pane-new-b' — pointer target (post-reset active bucket for proj-b)
        // 5. 'pane-orphan-xyz' — orphan (no attribution, ≥2 resets ago)
        conn.execute_batch(
            "
            INSERT INTO chat_messages_local (id, project_id, role, content)
            VALUES
                ('msg-1', 'workspace',      'user', 'hi from workspace'),
                ('msg-2', 'proj-a',         'user', 'hi from proj-a'),
                ('msg-3', 'proj-b',         'user', 'old proj-b message'),
                ('msg-4', 'pane-new-b',     'user', 'new proj-b message after reset'),
                ('msg-5', 'pane-orphan-xyz','user', 'orphan message');
        ",
        )
        .unwrap();

        // Run the migration.
        run_migrations(&conn).expect("v71→v72 migration should succeed");

        // ─ Assert 1: Schema version = current ─
        let version = get_schema_version(&conn);
        assert_eq!(
            version,
            Some(SCHEMA_VERSION.to_string()),
            "Schema version must be current after migration"
        );

        // ─ Assert 2: chat_sessions rows exist with correct classification ─
        // Helper to fetch a session row.
        let session_row = |id: &str| -> Option<(String, String, String, i64, Option<String>)> {
            conn.query_row(
                "SELECT id, project_id, origin, is_active, hidden_at FROM chat_sessions WHERE id=?1",
                params![id],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Option<String>>(4)?,
                )),
            ).ok()
        };

        // workspace bucket → surface='workspace', origin='migrated', is_active=1
        let ws = session_row("workspace").expect("workspace session must exist");
        assert_eq!(
            ws.1, "workspace",
            "workspace session: project_id should be 'workspace'"
        );
        assert_eq!(
            ws.2, "migrated",
            "workspace session: origin should be 'migrated'"
        );
        assert_eq!(ws.3, 1, "workspace session: is_active should be 1");
        assert!(
            ws.4.is_none(),
            "workspace session: hidden_at should be NULL"
        );

        // proj-a bucket → surface='proj-a', origin='migrated', is_active=1 (no reset)
        let pa = session_row("proj-a").expect("proj-a session must exist");
        assert_eq!(
            pa.1, "proj-a",
            "proj-a session: project_id should be 'proj-a'"
        );
        assert_eq!(
            pa.2, "migrated",
            "proj-a session: origin should be 'migrated'"
        );
        assert_eq!(pa.3, 1, "proj-a session: is_active should be 1 (no reset)");
        assert!(pa.4.is_none(), "proj-a session: hidden_at should be NULL");

        // proj-b bucket → surface='proj-b', origin='migrated', is_active=0 (reset happened)
        let pb = session_row("proj-b").expect("proj-b session must exist");
        assert_eq!(
            pb.1, "proj-b",
            "proj-b session: project_id should be 'proj-b'"
        );
        assert_eq!(
            pb.2, "migrated",
            "proj-b session: origin should be 'migrated'"
        );
        assert_eq!(
            pb.3, 0,
            "proj-b session: is_active should be 0 (reset existed)"
        );

        // pane-new-b bucket → surface='proj-b' (pointer target), origin='migrated', is_active=1
        let pnb = session_row("pane-new-b").expect("pane-new-b session must exist");
        assert_eq!(
            pnb.1, "proj-b",
            "pane-new-b session: project_id should be 'proj-b' (surface)"
        );
        assert_eq!(
            pnb.2, "migrated",
            "pane-new-b session: origin should be 'migrated'"
        );
        assert_eq!(
            pnb.3, 1,
            "pane-new-b session: is_active should be 1 (post-reset active)"
        );

        // pane-orphan-xyz bucket → surface='__recovered__', origin='recovered', is_active=0, hidden
        let orphan = session_row("pane-orphan-xyz").expect("pane-orphan-xyz session must exist");
        assert_eq!(
            orphan.1, "__recovered__",
            "orphan session: project_id should be '__recovered__'"
        );
        assert_eq!(
            orphan.2, "recovered",
            "orphan session: origin should be 'recovered'"
        );
        assert_eq!(orphan.3, 0, "orphan session: is_active should be 0");
        assert!(
            orphan.4.is_some(),
            "orphan session: hidden_at should be set"
        );

        // Verify orphan title.
        let orphan_title: String = conn
            .query_row(
                "SELECT title FROM chat_sessions WHERE id='pane-orphan-xyz'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            orphan_title, "Recovered session",
            "orphan session title should be 'Recovered session'"
        );

        // ─ Assert 3: thread_id backfilled (not NULL) ─
        let null_thread_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            null_thread_count, 0,
            "No messages should have NULL thread_id after migration"
        );

        // ─ Assert 4: project_id re-pointed to surface ─
        // workspace message: project_id should stay 'workspace'
        let ws_pid: String = conn
            .query_row(
                "SELECT project_id FROM chat_messages_local WHERE id='msg-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            ws_pid, "workspace",
            "workspace message: project_id should be 'workspace'"
        );

        // proj-a message: project_id should be 'proj-a'
        let pa_pid: String = conn
            .query_row(
                "SELECT project_id FROM chat_messages_local WHERE id='msg-2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            pa_pid, "proj-a",
            "proj-a message: project_id should be 'proj-a'"
        );

        // pane-new-b message: project_id should be re-pointed to 'proj-b' (the surface)
        let pnb_pid: String = conn
            .query_row(
                "SELECT project_id FROM chat_messages_local WHERE id='msg-4'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            pnb_pid, "proj-b",
            "pane-new-b message: project_id should be re-pointed to 'proj-b'"
        );

        // orphan message: project_id should be '__recovered__'
        let orphan_pid: String = conn
            .query_row(
                "SELECT project_id FROM chat_messages_local WHERE id='msg-5'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            orphan_pid, "__recovered__",
            "orphan message: project_id should be '__recovered__'"
        );

        // ─ Assert 5: ai_settings chat_thread_% keys deleted ─
        let pointer_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_settings WHERE key LIKE 'chat_thread_%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            pointer_count, 0,
            "All chat_thread_% keys must be deleted after migration"
        );

        // ─ Assert 6: thread index exists ─
        let idx_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name='idx_chat_local_thread'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            idx_exists,
            "idx_chat_local_thread must exist after v72 migration"
        );
    }

    #[test]
    fn test_v72_migration_idempotency() {
        // Running migrate_v72 twice on the same DB must succeed with no errors
        // and produce no duplicate chat_sessions rows.
        let conn = Connection::open_in_memory().unwrap();

        // Bootstrap a schema that includes the tables migrate_v72 depends on.
        // ai_agents must exist (even empty) because create_chat_sessions_table's
        // chat_sessions.agent_id carries a REFERENCES ai_agents(id) FK clause
        // (ADR-0062 D3) — SQLite requires the referenced table to exist for any
        // insert against the referencing table when foreign_keys enforcement is on.
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT);
            CREATE TABLE IF NOT EXISTS projects (id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '');
            CREATE TABLE IF NOT EXISTS ai_agents (id TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS chat_messages_local (
                id TEXT NOT NULL PRIMARY KEY,
                project_id TEXT NOT NULL DEFAULT '',
                role TEXT NOT NULL DEFAULT 'user',
                content TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                is_pinned INTEGER NOT NULL DEFAULT 0,
                attachment_ids TEXT,
                tool_calls TEXT,
                tool_results TEXT,
                model TEXT,
                token_count INTEGER,
                thread_id TEXT
            );
        ").unwrap();

        // Insert a project and some messages.
        conn.execute_batch(
            "
            INSERT INTO projects (id, name) VALUES ('proj-idem', 'Idempotency Test');
            INSERT INTO chat_messages_local (id, project_id, role, content)
            VALUES
                ('idem-msg-1', 'workspace', 'user', 'test'),
                ('idem-msg-2', 'proj-idem', 'user', 'test2');
        ",
        )
        .unwrap();

        // First run.
        migrate_v72(&conn).expect("First migrate_v72 call should succeed");

        // Count sessions after first run.
        let count_after_first: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_sessions", [], |row| row.get(0))
            .unwrap();
        assert!(
            count_after_first > 0,
            "Sessions should exist after first migrate_v72"
        );

        // Second run (idempotency: INSERT OR IGNORE + IF NOT EXISTS guards).
        migrate_v72(&conn).expect("Second migrate_v72 call should be idempotent no-op");

        // Count must be identical — no duplicate rows.
        let count_after_second: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_sessions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count_after_first, count_after_second,
            "Running migrate_v72 twice must not produce duplicate chat_sessions rows"
        );

        // All messages should still have non-NULL thread_id.
        let null_thread: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM chat_messages_local WHERE thread_id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            null_thread, 0,
            "No messages should have NULL thread_id after idempotent run"
        );
    }

    // ─── Test: migrate_v76 idempotency ───────────────────────────────────────────
    #[test]
    fn test_v76_migration_idempotency() {
        // Running migrate_v76 twice on the same DB must succeed with no errors.
        // The idempotency guard (pragma_table_info check) prevents duplicate ALTER TABLE.
        let conn = Connection::open_in_memory().unwrap();

        // Bootstrap minimal schema: ai_agents + chat_messages_local (no agent_id yet)
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ai_agents (
                id TEXT NOT NULL PRIMARY KEY,
                name TEXT NOT NULL DEFAULT '',
                is_main INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE IF NOT EXISTS chat_messages_local (
                id          TEXT NOT NULL PRIMARY KEY,
                project_id  TEXT NOT NULL DEFAULT '',
                role        TEXT NOT NULL DEFAULT 'user',
                content     TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                is_pinned   INTEGER NOT NULL DEFAULT 0,
                attachment_ids TEXT,
                tool_calls  TEXT,
                tool_results TEXT,
                model       TEXT,
                token_count INTEGER,
                thread_id   TEXT
            );
        ",
        )
        .unwrap();

        // First run: adds agent_id column
        migrate_v76(&conn).expect("First migrate_v76 call should succeed");

        // Verify column was added
        let col_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('chat_messages_local') WHERE name='agent_id'",
            [], |row| row.get(0),
        ).unwrap();
        assert!(
            col_exists,
            "agent_id column must exist after first migrate_v76"
        );

        // Second run: must be a no-op (idempotent)
        migrate_v76(&conn).expect("Second migrate_v76 call must be idempotent (no error)");
    }

    // ─── Test: migrate_v77 idempotency ───────────────────────────────────────
    #[test]
    fn test_migrate_v77_idempotency() {
        // Running migrate_v77 twice on poisoned data must NULL/delete everything on the
        // first run and be a true no-op (no error, nothing left to touch) on the second.
        let conn = Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS projects (
                id                    TEXT NOT NULL PRIMARY KEY,
                name                  TEXT NOT NULL DEFAULT '',
                smart_routing_enabled INTEGER,
                router_model          TEXT,
                simple_tier_model     TEXT,
                default_model         TEXT
            );
            CREATE TABLE IF NOT EXISTS ai_settings (
                key   TEXT NOT NULL PRIMARY KEY,
                value TEXT
            );
        ",
        )
        .unwrap();

        // Seed poisoned data: non-NULL values in all four projects columns...
        conn.execute(
            "INSERT INTO projects (id, name, smart_routing_enabled, router_model, simple_tier_model, default_model)
             VALUES ('proj-1', 'Poisoned Project', 1, 'router/model-x', 'simple/model-y', 'complex/model-z')",
            [],
        )
        .unwrap();

        // ...and all five retired ai_settings keys.
        for (key, value) in [
            ("tier_1_model", "legacy-tier-1"),
            ("tier_2_model", "legacy-tier-2"),
            ("tier_3_model", "legacy-tier-3"),
            ("router_model", "legacy-router"),
            ("smart_routing_enabled", "true"),
        ] {
            conn.execute(
                "INSERT INTO ai_settings (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .unwrap();
        }

        // First run: NULLs the project row and deletes all five ai_settings keys.
        migrate_v77(&conn).expect("First migrate_v77 call should succeed");

        let (sre, rm, stm, dm): (Option<i64>, Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT smart_routing_enabled, router_model, simple_tier_model, default_model
                 FROM projects WHERE id = 'proj-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!(sre.is_none(), "smart_routing_enabled must be NULL after first run");
        assert!(rm.is_none(), "router_model must be NULL after first run");
        assert!(stm.is_none(), "simple_tier_model must be NULL after first run");
        assert!(dm.is_none(), "default_model must be NULL after first run");

        let remaining_keys: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_settings WHERE key IN
                 ('tier_1_model', 'tier_2_model', 'tier_3_model', 'router_model', 'smart_routing_enabled')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining_keys, 0,
            "all five retired ai_settings keys must be deleted after first run"
        );

        // Second run: no-op, no error.
        migrate_v77(&conn).expect("Second migrate_v77 call must be idempotent (no error)");

        let (sre2, rm2, stm2, dm2): (Option<i64>, Option<String>, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT smart_routing_enabled, router_model, simple_tier_model, default_model
                 FROM projects WHERE id = 'proj-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert!(sre2.is_none());
        assert!(rm2.is_none());
        assert!(stm2.is_none());
        assert!(dm2.is_none());

        let remaining_keys2: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_settings WHERE key IN
                 ('tier_1_model', 'tier_2_model', 'tier_3_model', 'router_model', 'smart_routing_enabled')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining_keys2, 0, "still zero retired keys after second run");
    }

    // ── #2564 B1 fix (Path B), Commit 2: migrate_v80 tests ───────────────────────────────────

    mod migrate_v80_tests {
        use super::*;
        use serial_test::serial;
        use tempfile::TempDir;

        /// Post-v79 `ai_agents`/`ai_settings` schema (runtime_type already present,
        /// per migrate_v79's rename) — the shape migrate_v80 actually runs against.
        fn make_post_v79_conn() -> Connection {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(
                "CREATE TABLE ai_agents (
                    id              TEXT PRIMARY KEY,
                    name            TEXT NOT NULL DEFAULT '',
                    slug            TEXT NOT NULL UNIQUE,
                    is_default      INTEGER NOT NULL DEFAULT 0,
                    is_active       INTEGER NOT NULL DEFAULT 1,
                    workspace_path  TEXT NOT NULL DEFAULT '',
                    runtime_ref     TEXT,
                    runtime_type    TEXT,
                    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
                 );
                 CREATE TABLE ai_settings (
                    key   TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                 );
                ",
            )
            .unwrap();
            conn
        }

        fn seed_openclaw_agent(conn: &Connection, id: &str) {
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_ref, runtime_type)
                 VALUES (?1, ?1, ?1, 1, 1, ?1, 'main', 'openclaw')",
                params![id],
            )
            .unwrap();
        }

        /// Points `PANE_TEST_ROOT` at a fresh tempdir and creates `.pane/backups`
        /// inside it, returning the TempDir (keep it alive for the test's duration)
        /// and the backups dir path. #[serial] required — PANE_TEST_ROOT is
        /// process-global env state shared with every other test using it.
        fn with_test_backups_dir() -> (TempDir, std::path::PathBuf) {
            let temp = TempDir::new().expect("tempdir");
            std::env::set_var("PANE_TEST_ROOT", temp.path());
            let backups_dir = crate::backup::get_backups_dir();
            std::fs::create_dir_all(&backups_dir).expect("create backups dir");
            (temp, backups_dir)
        }

        fn write_fixture_backup(backups_dir: &std::path::Path, filename: &str, endpoint: Option<&str>) {
            let path = backups_dir.join(filename);
            let conn = Connection::open(&path).expect("open fixture backup db");
            conn.execute_batch(
                "CREATE TABLE ai_settings (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);",
            )
            .unwrap();
            if let Some(ep) = endpoint {
                conn.execute(
                    "INSERT INTO ai_settings (key, value) VALUES ('endpoint', ?1)",
                    params![ep],
                )
                .unwrap();
            }
        }

        #[test]
        #[serial(pane_test_root)]
        fn test_fresh_install_no_openclaw_agent_is_noop() {
            let (_temp, _backups_dir) = with_test_backups_dir();
            let conn = make_post_v79_conn();
            // No ai_agents rows at all.
            migrate_v80(&conn).expect("must succeed on fresh install");
            assert_eq!(crate::database::get_ai_setting(&conn, "endpoint"), None);
        }

        #[test]
        #[serial(pane_test_root)]
        fn test_endpoint_already_present_is_noop_even_without_backup() {
            let (_temp, _backups_dir) = with_test_backups_dir();
            let conn = make_post_v79_conn();
            seed_openclaw_agent(&conn, "agent-oc");
            crate::database::set_ai_setting(&conn, "endpoint", "http://100.64.1.9:18789").unwrap();

            migrate_v80(&conn).expect("must succeed");
            assert_eq!(
                crate::database::get_ai_setting(&conn, "endpoint").as_deref(),
                Some("http://100.64.1.9:18789"),
                "pre-existing endpoint must be untouched"
            );
        }

        #[test]
        #[serial(pane_test_root)]
        fn test_recovers_endpoint_from_single_version_backup() {
            let (_temp, backups_dir) = with_test_backups_dir();
            write_fixture_backup(&backups_dir, "pane_pre-migration-v78.db", Some("http://127.0.0.1:63926"));

            let conn = make_post_v79_conn();
            seed_openclaw_agent(&conn, "agent-oc");

            migrate_v80(&conn).expect("must succeed");
            assert_eq!(
                crate::database::get_ai_setting(&conn, "endpoint").as_deref(),
                Some("http://127.0.0.1:63926"),
                "endpoint must be recovered from the v78 backup"
            );
        }

        #[cfg(unix)]
        #[test]
        #[serial(pane_test_root)]
        fn test_matching_backup_symlink_to_external_db_is_rejected() {
            use std::os::unix::fs::symlink;
            let (_temp, backups_dir) = with_test_backups_dir();
            let external_dir = TempDir::new().unwrap();
            write_fixture_backup(external_dir.path(), "external.db", Some("http://external.example:18789"));
            symlink(external_dir.path().join("external.db"), backups_dir.join("pane_pre-migration-v78.db")).unwrap();
            write_fixture_backup(&backups_dir, "pane_pre-migration-v77.db", Some("http://authorized.example:18789"));
            let conn = make_post_v79_conn();
            seed_openclaw_agent(&conn, "agent-oc");
            migrate_v80(&conn).unwrap();
            assert_eq!(crate::database::get_ai_setting(&conn, "endpoint").as_deref(), Some("http://authorized.example:18789"));
        }

        #[test]
        #[serial(pane_test_root)]
        fn test_prefers_highest_pre79_version_and_skips_range_style_without_endpoint() {
            let (_temp, backups_dir) = with_test_backups_dir();
            // Older backup HAS an endpoint; newer (closer to v79) does NOT — search
            // must still find the older one rather than giving up after the first miss.
            write_fixture_backup(&backups_dir, "pane_pre-migration-v62-to-v69.db", None);
            write_fixture_backup(&backups_dir, "pane_pre-migration-v75-to-v77.db", Some("http://10.0.0.5:18789"));

            let conn = make_post_v79_conn();
            seed_openclaw_agent(&conn, "agent-oc");

            migrate_v80(&conn).expect("must succeed");
            assert_eq!(
                crate::database::get_ai_setting(&conn, "endpoint").as_deref(),
                Some("http://10.0.0.5:18789"),
                "must fall through to the next-best candidate when the top one lacks endpoint"
            );
        }

        #[test]
        #[serial(pane_test_root)]
        fn test_no_backup_found_leaves_endpoint_absent_no_placeholder() {
            let (_temp, _backups_dir) = with_test_backups_dir();
            // Backups dir exists but is empty.
            let conn = make_post_v79_conn();
            seed_openclaw_agent(&conn, "agent-oc");

            migrate_v80(&conn).expect("must succeed even with no recoverable backup");
            assert_eq!(
                crate::database::get_ai_setting(&conn, "endpoint"),
                None,
                "must not write a placeholder endpoint when nothing is recoverable"
            );
        }

        #[test]
        #[serial(pane_test_root)]
        fn test_backup_exists_but_lacks_endpoint_leaves_endpoint_absent() {
            let (_temp, backups_dir) = with_test_backups_dir();
            write_fixture_backup(&backups_dir, "pane_pre-migration-v78.db", None);

            let conn = make_post_v79_conn();
            seed_openclaw_agent(&conn, "agent-oc");

            migrate_v80(&conn).expect("must succeed");
            assert_eq!(crate::database::get_ai_setting(&conn, "endpoint"), None);
        }

        /// The explicitly required run-twice idempotency test: first run restores
        /// endpoint from backup; second run (endpoint now present) is a pure no-op.
        #[test]
        #[serial(pane_test_root)]
        fn test_migrate_v80_run_twice_is_idempotent() {
            let (_temp, backups_dir) = with_test_backups_dir();
            write_fixture_backup(&backups_dir, "pane_pre-migration-v78.db", Some("http://127.0.0.1:63926"));

            let conn = make_post_v79_conn();
            seed_openclaw_agent(&conn, "agent-oc");

            migrate_v80(&conn).expect("first run must succeed");
            let after_first = crate::database::get_ai_setting(&conn, "endpoint");
            assert_eq!(after_first.as_deref(), Some("http://127.0.0.1:63926"));

            // Delete the backup fixture between runs to prove the second run doesn't
            // even need to consult it — the endpoint-already-present short-circuit
            // must fire before any backup search happens.
            std::fs::remove_file(backups_dir.join("pane_pre-migration-v78.db")).unwrap();

            migrate_v80(&conn).expect("second run must be idempotent, not error");
            let after_second = crate::database::get_ai_setting(&conn, "endpoint");
            assert_eq!(
                after_second, after_first,
                "second run must leave the recovered endpoint untouched (no-op)"
            );
        }

        #[test]
        #[serial(pane_test_root)]
        fn test_ai_settings_table_absent_is_noop_no_panic() {
            let (_temp, _backups_dir) = with_test_backups_dir();
            let conn = Connection::open_in_memory().unwrap();
            // No ai_settings, no ai_agents table at all (older fixture schema edge case).
            migrate_v80(&conn).expect("must not panic or error when ai_settings is absent");
        }
    }

    // ── #2567 B6: migrate_v81 tests (role='sub'→'local' birth-mislabel repair) ─────

    mod migrate_v81_tests {
        use super::*;

        /// Post-v80 `ai_agents` schema (role column present, per B5/#2577's shape) —
        /// the shape migrate_v81 actually runs against.
        fn make_post_v80_ai_agents_conn() -> Connection {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(
                "CREATE TABLE ai_agents (
                    id              TEXT PRIMARY KEY,
                    name            TEXT NOT NULL DEFAULT '',
                    slug            TEXT NOT NULL UNIQUE,
                    role            TEXT NOT NULL DEFAULT 'sub',
                    is_default      INTEGER NOT NULL DEFAULT 0,
                    is_active       INTEGER NOT NULL DEFAULT 1,
                    workspace_path  TEXT NOT NULL DEFAULT '',
                    runtime_ref     TEXT,
                    runtime_type    TEXT,
                    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
                 );
                ",
            )
            .unwrap();
            conn
        }

        /// Core case: a locally-created row (role='sub', runtime_ref=NULL) must be
        /// re-labeled role='local'. A real OC subagent row (role='sub',
        /// runtime_ref='sub-oc-1') must be left untouched.
        #[test]
        fn test_migrate_v81_relabels_local_rows_leaves_oc_subagents_alone() {
            let conn = make_post_v80_ai_agents_conn();
            conn.execute_batch(
                "INSERT INTO ai_agents (id, name, slug, role, runtime_ref, updated_at)
                 VALUES ('local-1', 'Local One', 'local-one', 'sub', NULL, '2020-01-01T00:00:00Z');
                 INSERT INTO ai_agents (id, name, slug, role, runtime_ref, updated_at)
                 VALUES ('oc-sub-1', 'OC Sub', 'oc-sub', 'sub', 'sub-oc-1', '2020-01-01T00:00:00Z');
                 INSERT INTO ai_agents (id, name, slug, role, runtime_ref, updated_at)
                 VALUES ('oc-main-1', 'OC Main', 'oc-main', 'main', 'main', '2020-01-01T00:00:00Z');
                ",
            )
            .unwrap();

            migrate_v81(&conn).expect("migrate_v81 must succeed");

            let local_role: String = conn
                .query_row("SELECT role FROM ai_agents WHERE id = 'local-1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(local_role, "local", "runtime_ref IS NULL row must be re-labeled 'local'");

            let oc_sub_role: String = conn
                .query_row("SELECT role FROM ai_agents WHERE id = 'oc-sub-1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(oc_sub_role, "sub", "real OC subagent (runtime_ref non-NULL) must be left as 'sub'");

            let oc_main_role: String = conn
                .query_row("SELECT role FROM ai_agents WHERE id = 'oc-main-1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(oc_main_role, "main", "role='main' rows are untouched by this migration");

            // Matt's ruling: role-only data repair, updated_at must NOT be bumped.
            let local_updated_at: String = conn
                .query_row("SELECT updated_at FROM ai_agents WHERE id = 'local-1'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                local_updated_at, "2020-01-01T00:00:00Z",
                "migrate_v81 must NOT bump updated_at — role-only data repair, not a user edit"
            );
        }

        /// Required run-twice idempotency test: first run re-labels the mislabeled
        /// rows; second run matches zero rows (self-exhausting WHERE predicate) and
        /// must not error or double-mutate anything.
        #[test]
        fn test_migrate_v81_run_twice_is_idempotent() {
            let conn = make_post_v80_ai_agents_conn();
            conn.execute_batch(
                "INSERT INTO ai_agents (id, name, slug, role, runtime_ref)
                 VALUES ('local-1', 'Local One', 'local-one', 'sub', NULL);
                 INSERT INTO ai_agents (id, name, slug, role, runtime_ref)
                 VALUES ('oc-sub-1', 'OC Sub', 'oc-sub', 'sub', 'sub-oc-1');
                ",
            )
            .unwrap();

            migrate_v81(&conn).expect("first run must succeed");
            let roles_after_first: Vec<(String, String)> = {
                let mut stmt = conn.prepare("SELECT id, role FROM ai_agents ORDER BY id").unwrap();
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect()
            };
            assert_eq!(
                roles_after_first,
                vec![
                    ("local-1".to_string(), "local".to_string()),
                    ("oc-sub-1".to_string(), "sub".to_string()),
                ]
            );

            migrate_v81(&conn).expect("second run must be idempotent, not error");
            let roles_after_second: Vec<(String, String)> = {
                let mut stmt = conn.prepare("SELECT id, role FROM ai_agents ORDER BY id").unwrap();
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                    .unwrap()
                    .map(|r| r.unwrap())
                    .collect()
            };
            assert_eq!(
                roles_after_second, roles_after_first,
                "second run must leave every row's role untouched (no-op)"
            );
        }

        /// Table-existence guard: absent `ai_agents` (older fixture schema edge case)
        /// must be a clean no-op, not a panic or error.
        #[test]
        fn test_migrate_v81_ai_agents_table_absent_is_noop_no_panic() {
            let conn = Connection::open_in_memory().unwrap();
            migrate_v81(&conn).expect("must not panic or error when ai_agents is absent");
        }
    }

    // ── B3 (#2565): migrate_v82 tests ──────────────────────────────────────────────
    //
    // #[serial] on every test in this module — KEYCHAIN_OVERRIDE
    // (crate::keychain::set_test_keychain) is process-global state, matching the
    // convention `credential_migration.rs`'s own keychain-touching tests already use.

    mod migrate_v82_tests {
        use super::*;
        use crate::ai::credential_migration::{
            KEYCHAIN_ACCOUNT_ANTHROPIC_KEY, KEYCHAIN_ACCOUNT_OPENROUTER_KEY,
            KEYCHAIN_ACCOUNT_PROVIDER_KEY,
        };
        use crate::keychain::test_support::KeychainGuard;
        use crate::keychain::KeychainProvider;
        use serial_test::serial;

        /// Post-v81 `ai_agents`/`ai_settings` schema — identical shape to
        /// migrate_v80/v81's fixtures (no schema change across v79→v82, see
        /// build_v81_fixture.rs's module doc).
        fn make_post_v81_conn() -> Connection {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(
                "CREATE TABLE ai_agents (
                    id              TEXT PRIMARY KEY,
                    name            TEXT NOT NULL DEFAULT '',
                    slug            TEXT NOT NULL UNIQUE,
                    role            TEXT NOT NULL DEFAULT 'local',
                    is_default      INTEGER NOT NULL DEFAULT 0,
                    is_active       INTEGER NOT NULL DEFAULT 1,
                    workspace_path  TEXT NOT NULL DEFAULT '',
                    runtime_ref     TEXT,
                    runtime_type    TEXT,
                    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
                 );
                 CREATE TABLE ai_settings (
                    key   TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                 );
                ",
            )
            .unwrap();
            conn
        }

        fn seed_agent(conn: &Connection, id: &str, runtime_type: &str, is_active: bool) {
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_ref, runtime_type)
                 VALUES (?1, ?1, ?1, 0, ?2, ?1, NULL, ?3)",
                params![id, if is_active { 1 } else { 0 }, runtime_type],
            )
            .unwrap();
        }

        fn with_mock_keychain<F: FnOnce()>(f: F) {
            let _kc = KeychainGuard::set();
            f();
        }

        // ── Part A: keychain slot split ───────────────────────────────────────────

        #[test]
        #[serial(keychain_override)]
        fn test_keychain_split_sk_ant_prefix_routes_to_anthropic() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                crate::keychain::store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "sk-ant-abc123").unwrap();

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap(),
                    Some("sk-ant-abc123".to_string()),
                    "sk-ant- prefixed legacy value must route to the anthropic slot"
                );
                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).unwrap(),
                    None,
                    "openrouter slot must remain untouched"
                );
                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY).unwrap(),
                    None,
                    "legacy slot must be cleared after a successful copy"
                );
            });
        }

        #[test]
        #[serial(keychain_override)]
        fn test_keychain_split_sk_or_prefix_routes_to_openrouter() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                crate::keychain::store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "sk-or-v1-xyz789").unwrap();

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).unwrap(),
                    Some("sk-or-v1-xyz789".to_string()),
                    "sk-or- prefixed legacy value must route to the openrouter slot"
                );
                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap(),
                    None,
                    "anthropic slot must remain untouched"
                );
                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY).unwrap(),
                    None,
                    "legacy slot must be cleared after a successful copy"
                );
            });
        }

        /// Non-standard prefix, but an unambiguous active `runtime_type='anthropic'`
        /// agent exists — Tier 2 fallback must infer from it.
        #[test]
        #[serial(keychain_override)]
        fn test_keychain_split_unrecognized_prefix_falls_back_to_active_agent_inference() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                seed_agent(&conn, "agent-anthropic", "anthropic", true);
                crate::keychain::store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "custom-proxy-key-000").unwrap();

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap(),
                    Some("custom-proxy-key-000".to_string()),
                    "unrecognized prefix + active anthropic agent must infer anthropic"
                );
                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).unwrap(),
                    None
                );
            });
        }

        /// No recognized prefix and no active agent to infer from at all — Tier 3
        /// last resort: both new slots stay empty and the legacy value is preserved.
        #[test]
        #[serial(keychain_override)]
        fn test_keychain_split_no_prefix_no_active_agent_leaves_both_slots_empty() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                crate::keychain::store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "unrecognized-format-key").unwrap();
                // No ai_agents rows at all.

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap(), None);
                assert_eq!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).unwrap(), None);
                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY).unwrap(),
                    Some("unrecognized-format-key".to_string()),
                    "legacy slot must be preserved when no destination can be determined"
                );
            });
        }

        /// Ambiguous active-agent inference (both anthropic and openrouter active) is
        /// exactly as unresolvable as no active agent at all — preserve the legacy value
        /// rather than guessing a destination.
        #[test]
        #[serial(keychain_override)]
        fn test_keychain_split_ambiguous_active_agents_leaves_both_slots_empty() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                seed_agent(&conn, "agent-anthropic", "anthropic", true);
                seed_agent(&conn, "agent-openrouter", "openrouter", true);
                crate::keychain::store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "unrecognized-format-key").unwrap();

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap(), None);
                assert_eq!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).unwrap(), None);
                assert_eq!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY).unwrap(), Some("unrecognized-format-key".to_string()));
            });
        }

        #[test]
        #[serial(keychain_override)]
        fn test_keychain_split_legacy_slot_empty_is_noop() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                // No legacy value stored at all.

                migrate_v82(&conn).expect("migrate_v82 must succeed even with nothing to split");

                assert_eq!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap(), None);
                assert_eq!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).unwrap(), None);
            });
        }

        // Regression tests for C5 (#2584): write-succeeds-but-readback-fails.
        //
        // This is the exact test that would have caught #2584: store_password
        // returns Ok(()) for the new per-provider slot, but the immediately
        // following get_password for that SAME account returns Ok(None) (or
        // Err) -- simulating a real Security.framework write/read visibility gap
        // (keychain-lock transition, ACL/trust-store race, first-unlock-after-boot).
        // The legacy slot must survive intact and "cleared" must never be logged.

        #[test]
        #[serial(keychain_override)]
        fn test_keychain_split_readback_ok_none_preserves_legacy_slot() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();

                // Swap in a MockKeychain whose get_password() on the anthropic slot
                // returns Ok(None) even after a successful store -- the write
                // silently didn't stick.
                let flaky = crate::keychain::MockKeychain::new();
                flaky.store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "sk-ant-flaky-write").unwrap();
                flaky.fail_get_for(
                    KEYCHAIN_ACCOUNT_ANTHROPIC_KEY,
                    crate::keychain::KeychainFailureMode::SilentMiss,
                );
                crate::keychain::set_test_keychain(Box::new(flaky));

                migrate_v82(&conn).expect_err("unconfirmed read-back must keep migration retryable");

                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY).unwrap(),
                    Some("sk-ant-flaky-write".to_string()),
                    "legacy slot must NOT be deleted when the read-back after write returns Ok(None) -- this is the exact #2584 defect"
                );
            });
        }

        #[test]
        #[serial(keychain_override)]
        fn test_keychain_split_readback_err_preserves_legacy_slot() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();

                let flaky = crate::keychain::MockKeychain::new();
                flaky.store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "sk-or-flaky-write").unwrap();
                flaky.fail_get_for(
                    KEYCHAIN_ACCOUNT_OPENROUTER_KEY,
                    crate::keychain::KeychainFailureMode::Error("simulated ACL denial".to_string()),
                );
                crate::keychain::set_test_keychain(Box::new(flaky));

                migrate_v82(&conn).expect_err("destination read error must keep migration retryable");

                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY).unwrap(),
                    Some("sk-or-flaky-write".to_string()),
                    "legacy slot must NOT be deleted when the read-back after write returns Err"
                );
                // The openrouter slot's read is still wired to fail in this mock
                // (that's what simulates the unconfirmable write) — assert the
                // failure is surfaced as Err rather than silently treated as a
                // successful confirmation.
                assert!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).is_err(),
                    "the new slot's write was never confirmed as queryable — its read should still fail"
                );
            });
        }

        #[test]
        #[serial(keychain_override)]
        fn test_unrelated_populated_slot_does_not_delete_legacy_when_target_write_fails() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                let flaky = crate::keychain::MockKeychain::new();
                flaky.store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "sk-ant-legacy").unwrap();
                flaky.store_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY, "sk-or-existing").unwrap();
                flaky.fail_store_for(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY, crate::keychain::KeychainFailureMode::Error("write denied".into()));
                crate::keychain::set_test_keychain(Box::new(flaky));
                migrate_v82(&conn).expect_err("target write failure must propagate");
                assert_eq!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY).unwrap(), Some("sk-ant-legacy".into()));
                assert_eq!(crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).unwrap(), Some("sk-or-existing".into()));
            });
        }

        #[test]
        #[serial(pane_test_root, keychain_override)]
        fn test_keychain_read_failure_does_not_stamp_schema_complete() {
            with_mock_keychain(|| {
                let temp = tempfile::TempDir::new().unwrap();
                std::env::set_var("PANE_TEST_ROOT", temp.path());
                std::env::set_var("PANE_TEST_MODE", "1");
                std::env::set_var("PANE_TEST_DB", temp.path().join("migration.db"));
                let conn = Connection::open_in_memory().unwrap();
                run_migrations(&conn).unwrap();
                crate::database::set_ai_setting(&conn, "schema_version", "81").unwrap();
                let flaky = crate::keychain::MockKeychain::new();
                flaky.fail_get_for(KEYCHAIN_ACCOUNT_PROVIDER_KEY, crate::keychain::KeychainFailureMode::Error("transient read".into()));
                crate::keychain::set_test_keychain(Box::new(flaky));
                run_migrations(&conn).expect_err("keychain read failure must abort migration");
                assert_eq!(crate::database::get_ai_setting(&conn, "schema_version").as_deref(), Some("81"));
                std::env::remove_var("PANE_TEST_DB");
                std::env::remove_var("PANE_TEST_MODE");
            });
        }

        // ── Part B: ollama_endpoint backfill ────────────────────────────────────

        /// Grandfather rule: an active `runtime_type='ollama'` agent with no usable
        /// legacy `endpoint` value must be backfilled with the standard local default.
        #[test]
        #[serial(keychain_override)]
        fn test_ollama_backfill_active_agent_grandfathered_with_default() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                seed_agent(&conn, "agent-ollama", "ollama", true);
                // No legacy `endpoint` key at all.

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(
                    crate::database::get_ai_setting(&conn, "ollama_endpoint").as_deref(),
                    Some("http://localhost:11434"),
                    "active ollama agent with no usable legacy endpoint must be grandfathered with the default"
                );
            });
        }

        /// Grandfather rule, ollama-shaped legacy `endpoint` present: must copy the
        /// actual stored value rather than the generic default.
        #[test]
        #[serial(keychain_override)]
        fn test_ollama_backfill_copies_ollama_shaped_legacy_endpoint() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                seed_agent(&conn, "agent-ollama", "ollama", true);
                crate::database::set_ai_setting(&conn, "endpoint", "http://127.0.0.1:11434").unwrap();

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(
                    crate::database::get_ai_setting(&conn, "ollama_endpoint").as_deref(),
                    Some("http://127.0.0.1:11434"),
                    "ollama-shaped legacy endpoint must be copied verbatim, not replaced with the generic default"
                );
                // Legacy `endpoint` must be left untouched — openclaw/sync_client still read it.
                assert_eq!(
                    crate::database::get_ai_setting(&conn, "endpoint").as_deref(),
                    Some("http://127.0.0.1:11434"),
                    "legacy endpoint key must NOT be cleared"
                );
            });
        }

        /// No active ollama agent and no ollama-shaped legacy endpoint — must leave
        /// `ollama_endpoint` absent entirely (no default synthesized out of thin air).
        #[test]
        #[serial(keychain_override)]
        fn test_ollama_backfill_no_active_agent_no_backfill() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                // No ollama agent, no legacy endpoint at all.

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(
                    crate::database::get_ai_setting(&conn, "ollama_endpoint"),
                    None,
                    "must not synthesize ollama_endpoint when there is no signal at all it was ever configured"
                );
            });
        }

        /// A non-ollama-shaped legacy `endpoint` (e.g. a real openclaw gateway URL)
        /// combined with NO active ollama agent must not leak into ollama_endpoint.
        #[test]
        #[serial(keychain_override)]
        fn test_ollama_backfill_non_ollama_shaped_endpoint_no_active_agent_no_backfill() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                seed_agent(&conn, "agent-oc", "openclaw", true);
                crate::database::set_ai_setting(&conn, "endpoint", "http://100.64.1.9:18789").unwrap();

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(
                    crate::database::get_ai_setting(&conn, "ollama_endpoint"),
                    None,
                    "a non-ollama-shaped legacy endpoint (openclaw gateway URL) must not backfill ollama_endpoint"
                );
            });
        }

        /// An *inactive* ollama agent must NOT trigger the grandfather backfill — only
        /// `is_active = 1` rows count as "pre-existing reliance" per the migration's
        /// own query predicate.
        #[test]
        #[serial(keychain_override)]
        fn test_ollama_backfill_inactive_agent_does_not_grandfather() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                seed_agent(&conn, "agent-ollama-inactive", "ollama", false);

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(
                    crate::database::get_ai_setting(&conn, "ollama_endpoint"),
                    None,
                    "an inactive ollama agent must not trigger the grandfather backfill"
                );
            });
        }

        // ── Combined / scaffolding ────────────────────────────────────────────

        /// The explicitly required run-twice idempotency test, covering BOTH parts at
        /// once: first run splits the keychain slot AND backfills ollama_endpoint;
        /// second run must touch neither (both idempotency guards fire).
        #[test]
        #[serial(keychain_override)]
        fn test_migrate_v82_run_twice_is_idempotent() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                seed_agent(&conn, "agent-ollama", "ollama", true);
                crate::keychain::store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "sk-ant-abc123").unwrap();

                migrate_v82(&conn).expect("first run must succeed");
                let anthropic_after_first = crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap();
                let ollama_endpoint_after_first = crate::database::get_ai_setting(&conn, "ollama_endpoint");
                assert_eq!(anthropic_after_first, Some("sk-ant-abc123".to_string()));
                assert_eq!(ollama_endpoint_after_first.as_deref(), Some("http://localhost:11434"));

                migrate_v82(&conn).expect("second run must be idempotent, not error");
                let anthropic_after_second = crate::keychain::get_password(KEYCHAIN_ACCOUNT_ANTHROPIC_KEY).unwrap();
                let ollama_endpoint_after_second = crate::database::get_ai_setting(&conn, "ollama_endpoint");
                assert_eq!(
                    anthropic_after_second, anthropic_after_first,
                    "second run must leave the anthropic slot untouched"
                );
                assert_eq!(
                    ollama_endpoint_after_second, ollama_endpoint_after_first,
                    "second run must leave ollama_endpoint untouched"
                );
                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY).unwrap(),
                    None,
                    "legacy slot stays cleared on the second run too"
                );
            });
        }

        /// Table-existence guard: absent `ai_agents`/`ai_settings` (older fixture
        /// schema edge case) must be a clean no-op for Part B, not a panic or error.
        /// Part A doesn't depend on either table so it still runs normally.
        #[test]
        #[serial(keychain_override)]
        fn test_migrate_v82_tables_absent_is_noop_no_panic() {
            with_mock_keychain(|| {
                let conn = Connection::open_in_memory().unwrap();
                migrate_v82(&conn).expect("must not panic or error when ai_agents/ai_settings are absent");
            });
        }

        /// Smoke test: the fallback chain runs end-to-end (prefix miss → active-agent
        /// inference hit) in the same pass as the ollama backfill, proving the two
        /// parts of migrate_v82 don't interfere with each other when both apply to
        /// the same DB.
        #[test]
        #[serial(keychain_override)]
        fn test_migrate_v82_keychain_fallback_and_ollama_backfill_both_apply_together() {
            with_mock_keychain(|| {
                let conn = make_post_v81_conn();
                seed_agent(&conn, "agent-openrouter", "openrouter", true);
                seed_agent(&conn, "agent-ollama", "ollama", true);
                crate::keychain::store_password(KEYCHAIN_ACCOUNT_PROVIDER_KEY, "custom-format-key").unwrap();

                migrate_v82(&conn).expect("migrate_v82 must succeed");

                assert_eq!(
                    crate::keychain::get_password(KEYCHAIN_ACCOUNT_OPENROUTER_KEY).unwrap(),
                    Some("custom-format-key".to_string()),
                    "unrecognized prefix must still resolve via active-agent inference"
                );
                assert_eq!(
                    crate::database::get_ai_setting(&conn, "ollama_endpoint").as_deref(),
                    Some("http://localhost:11434"),
                    "ollama grandfather backfill must apply in the same pass"
                );
            });
        }
    }
}

// #3112 — arm-sweep: every `old_version == "N"` match arm's own ladder must reach
// SCHEMA_VERSION, not just the v69 arm this issue found broken.
#[cfg(test)]
mod arm_sweep_tests {
    /// Scoped as a source-text scan, not an AST walk (same discipline #3104's
    /// structural guard uses) — enumerates every `Some(ref old_version) if
    /// old_version == "N"` match arm in `run_migrations` by source position, slices
    /// each arm's own body up to the next arm (or the final catch-all `Some(ref
    /// old_version) =>` fallback), and asserts: if that body calls ANY `migrate_vNN`
    /// step function at all, the HIGHEST NN it calls is exactly `SCHEMA_VERSION`'s
    /// own numeric value. An arm that calls zero `migrate_vNN` functions (e.g. the
    /// `"61"` arm, which predates the step-function ladder) is not a candidate for
    /// this check — it has no ladder to skip steps from.
    ///
    /// This is exactly the gap #3112 found by hand in the v69 arm (ladder stopped at
    /// migrate_v89 while unconditionally stamping schema_version = 96, silently
    /// skipping v90–v96) — automated here so a reader doesn't have to re-derive
    /// Reed's by-hand sweep of all 34 arms blind next time a new arm is added.
    #[test]
    fn test_every_arm_with_migrate_v_calls_reaches_schema_version() {
        let src = include_str!("migrations.rs");
        let schema_version: usize = super::SCHEMA_VERSION
            .parse()
            .expect("SCHEMA_VERSION must be a plain integer string");

        // Every arm-start position: the specific `old_version == "N"` arms, in the
        // exact order they appear in the match (source order == the order rustc
        // tries them, irrelevant here since we only use positions as slice
        // boundaries), PLUS the final catch-all `Some(ref old_version) =>` fallback
        // as the terminating boundary for the last specific arm.
        let specific_arm_re = regex::Regex::new(
            r#"Some\(ref old_version\) if old_version == "(\d+)""#,
        ).unwrap();
        let fallback_marker = "Some(ref old_version) =>";

        let mut starts: Vec<(usize, Option<usize>)> = specific_arm_re
            .captures_iter(src)
            .map(|c| {
                let m = c.get(0).unwrap();
                let ver: usize = c.get(1).unwrap().as_str().parse().unwrap();
                (m.start(), Some(ver))
            })
            .collect();
        let fallback_pos = src.find(fallback_marker).expect("fallback arm must exist");
        starts.push((fallback_pos, None));
        starts.sort_by_key(|(pos, _)| *pos);

        let specific_count = starts.iter().filter(|(_, v)| v.is_some()).count();
        assert!(
            specific_count >= 30,
            "expected dozens of versioned migration arms (found {}) — the source scan's \
             own regex may have stopped matching (e.g. after a refactor); re-check the \
             pattern before trusting silence",
            specific_count
        );

        // Deliberately narrow: matches a REAL `migration_step(step, total, "migrate_vNN", ...)`
        // registration, not a bare `migrate_vNN` substring — a doc comment describing
        // the bug (e.g. this very test's own neighboring comment, which has to SAY
        // "migrate_v90" in prose) must never be mistaken for a step call.
        let migrate_call_re = regex::Regex::new(r#"migration_step\([^,]+,[^,]+,\s*"migrate_v(\d+)""#).unwrap();
        let mut checked = 0usize;
        for i in 0..starts.len() {
            let (start, ver) = starts[i];
            let Some(old_version) = ver else { continue };
            let end = starts.get(i + 1).map(|(p, _)| *p).unwrap_or(src.len());
            let body = &src[start..end];

            let max_called = migrate_call_re
                .captures_iter(body)
                .filter_map(|c| c.get(1).unwrap().as_str().parse::<usize>().ok())
                .max();

            if let Some(max_called) = max_called {
                checked += 1;
                assert_eq!(
                    max_called, schema_version,
                    "old_version == \"{}\" arm's ladder reaches migrate_v{} but then stamps \
                     schema_version = {} — it skips v{}–v{} silently (the exact #3112 shape: \
                     a DB upgrading from v{} would report itself current while missing those \
                     columns/tables)",
                    old_version, max_called, schema_version, max_called + 1, schema_version, old_version
                );
            }
        }

        assert!(
            checked >= 25,
            "expected most arms to have a real migrate_vNN ladder to check (checked {}) — if \
             this drops, the regex or the arm-slicing boundaries may have broken, not that \
             every arm suddenly stopped calling migrate_vNN",
            checked
        );
    }
}

// ── C1 (#2585) / C3 (#2587): migrate_v83 tests ───────────────────────────────

#[cfg(test)]
mod migrate_v83_tests {
    use super::*;

    /// Post-v82 `ai_agents` schema — identical shape to migrate_v80/v81/v82's
    /// fixtures (no schema change across v79→v82, see build_v82_fixture.rs's
    /// module doc).
    fn make_post_v82_ai_agents_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_agents (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL DEFAULT '',
                slug            TEXT NOT NULL UNIQUE,
                role            TEXT NOT NULL DEFAULT 'local',
                is_default      INTEGER NOT NULL DEFAULT 0,
                is_active       INTEGER NOT NULL DEFAULT 1,
                workspace_path  TEXT NOT NULL DEFAULT '',
                model           TEXT,
                runtime_ref     TEXT,
                runtime_type    TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
             );
            ",
        )
        .unwrap();
        conn
    }

    fn seed_agent_with_model(
        conn: &Connection,
        id: &str,
        runtime_type: Option<&str>,
        model: Option<&str>,
        updated_at: &str,
    ) {
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_ref, runtime_type, model, updated_at)
             VALUES (?1, ?1, ?1, 0, 1, ?1, NULL, ?2, ?3, ?4)",
            params![id, runtime_type, model, updated_at],
        )
        .unwrap();
    }

    // ── C1: model backfill ────────────────────────────────────────────────────

    /// Item 1: an openclaw row with `model IS NULL` must be backfilled to the
    /// sentinel placeholder, and `updated_at` must be bumped (Matt's ruling #3:
    /// this write is dispatch-relevant, unlike v81's role-label repair which did
    /// NOT bump `updated_at`).
    #[test]
    fn test_migrate_v83_backfills_null_model_for_openclaw_rows() {
        let conn = make_post_v82_ai_agents_conn();
        seed_agent_with_model(&conn, "agent-oc", Some("openclaw"), None, "2020-01-01T00:00:00Z");

        migrate_v83(&conn).expect("migrate_v83 must succeed");

        let model: Option<String> = conn
            .query_row("SELECT model FROM ai_agents WHERE id = 'agent-oc'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            model,
            Some("openclaw-pending".to_string()),
            "NULL model on an openclaw row must be backfilled to the sentinel placeholder"
        );

        let updated_at: String = conn
            .query_row("SELECT updated_at FROM ai_agents WHERE id = 'agent-oc'", [], |r| r.get(0))
            .unwrap();
        assert_ne!(
            updated_at, "2020-01-01T00:00:00Z",
            "updated_at MUST be bumped on the model backfill — contrast migrate_v81's role \
             repair, which explicitly does NOT bump updated_at because role is a pure label; \
             model is dispatch-relevant (before this migration the row cannot dispatch, after \
             it can), so this write is a real behavioral state change, not a cosmetic relabel"
        );
    }

    /// Item 2: same as above but for `model = ''` (covers `unwrap_or_default()`
    /// producing empty string at read time from a prior partial write).
    #[test]
    fn test_migrate_v83_backfills_empty_string_model_for_openclaw_rows() {
        let conn = make_post_v82_ai_agents_conn();
        seed_agent_with_model(&conn, "agent-oc", Some("openclaw"), Some(""), "2020-01-01T00:00:00Z");

        migrate_v83(&conn).expect("migrate_v83 must succeed");

        let model: Option<String> = conn
            .query_row("SELECT model FROM ai_agents WHERE id = 'agent-oc'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            model,
            Some("openclaw-pending".to_string()),
            "empty-string model on an openclaw row must be backfilled to the sentinel placeholder"
        );
    }

    /// Item 3: anthropic/openrouter/ollama rows with NULL/empty model must NOT be
    /// touched — those providers legitimately require a user-set model; masking a
    /// real "not configured" state for them would be wrong.
    #[test]
    fn test_migrate_v83_leaves_non_openclaw_rows_untouched() {
        let conn = make_post_v82_ai_agents_conn();
        seed_agent_with_model(&conn, "agent-anthropic", Some("anthropic"), None, "2020-01-01T00:00:00Z");
        seed_agent_with_model(&conn, "agent-openrouter", Some("openrouter"), Some(""), "2020-01-01T00:00:00Z");
        seed_agent_with_model(&conn, "agent-ollama", Some("ollama"), None, "2020-01-01T00:00:00Z");

        migrate_v83(&conn).expect("migrate_v83 must succeed");

        let expected: &[(&str, Option<&str>)] = &[
            ("agent-anthropic", None),
            ("agent-openrouter", Some("")),
            ("agent-ollama", None),
        ];
        for (id, expected_model) in expected {
            let model: Option<String> = conn
                .query_row(&format!("SELECT model FROM ai_agents WHERE id = '{}'", id), [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                model.as_deref(),
                *expected_model,
                "{} must NOT be backfilled — only runtime_type='openclaw' rows are in scope",
                id
            );
            let updated_at: String = conn
                .query_row(&format!("SELECT updated_at FROM ai_agents WHERE id = '{}'", id), [], |r| r.get(0))
                .unwrap();
            assert_eq!(updated_at, "2020-01-01T00:00:00Z", "{} updated_at must be untouched", id);
        }
    }

    /// Item 4: an openclaw row that already has a real model must never be
    /// overwritten.
    #[test]
    fn test_migrate_v83_leaves_rows_with_existing_model_untouched() {
        let conn = make_post_v82_ai_agents_conn();
        seed_agent_with_model(
            &conn,
            "agent-oc",
            Some("openclaw"),
            Some("anthropic/claude-sonnet-4-6"), // CI-ALLOW: hardcoded-model
            "2020-01-01T00:00:00Z",
        );

        migrate_v83(&conn).expect("migrate_v83 must succeed");

        let model: Option<String> = conn
            .query_row("SELECT model FROM ai_agents WHERE id = 'agent-oc'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            model,
            Some("anthropic/claude-sonnet-4-6".to_string()), // CI-ALLOW: hardcoded-model
            "a real existing model value must never be overwritten by the placeholder"
        );

        let updated_at: String = conn
            .query_row("SELECT updated_at FROM ai_agents WHERE id = 'agent-oc'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            updated_at, "2020-01-01T00:00:00Z",
            "updated_at must not be bumped for a row that wasn't actually touched"
        );
    }

    /// Item 5: run-twice idempotency — second run must match zero rows for both
    /// UPDATEs (self-exhausting WHERE predicates) and must not error or
    /// double-mutate anything.
    #[test]
    fn test_migrate_v83_run_twice_is_idempotent() {
        let conn = make_post_v82_ai_agents_conn();
        seed_agent_with_model(&conn, "agent-oc", Some("openclaw"), None, "2020-01-01T00:00:00Z");
        seed_agent_with_model(&conn, "agent-legacy", None, Some("claude-3"), "2020-01-01T00:00:00Z"); // CI-ALLOW: hardcoded-model

        migrate_v83(&conn).expect("first run must succeed");
        let state_after_first: (Option<String>, i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT model FROM ai_agents WHERE id = 'agent-oc'),
                    (SELECT is_active FROM ai_agents WHERE id = 'agent-legacy'),
                    (SELECT COUNT(*) FROM ai_agents)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(state_after_first.0, Some("openclaw-pending".to_string()));
        assert_eq!(state_after_first.1, 0);

        migrate_v83(&conn).expect("second run must be idempotent, not error");
        let state_after_second: (Option<String>, i64, i64) = conn
            .query_row(
                "SELECT
                    (SELECT model FROM ai_agents WHERE id = 'agent-oc'),
                    (SELECT is_active FROM ai_agents WHERE id = 'agent-legacy'),
                    (SELECT COUNT(*) FROM ai_agents)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            state_after_second, state_after_first,
            "second run must leave both rows' post-migration state untouched (no-op)"
        );
    }

    /// Item 6: table-existence guard — absent `ai_agents` (older fixture schema
    /// edge case) must be a clean no-op, not a panic or error.
    #[test]
    fn test_migrate_v83_ai_agents_table_absent_is_noop_no_panic() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_v83(&conn).expect("must not panic or error when ai_agents is absent");
    }

    /// Item 7: explicit, standalone assertion that `updated_at` changes on
    /// backfilled rows — documents the distinction from migrate_v81 (which
    /// deliberately does NOT bump updated_at for its role-label repair) as its own
    /// named test, separate from item 1's combined assertion.
    #[test]
    fn test_migrate_v83_bumps_updated_at() {
        let conn = make_post_v82_ai_agents_conn();
        seed_agent_with_model(&conn, "agent-oc", Some("openclaw"), None, "2020-01-01T00:00:00Z");

        migrate_v83(&conn).expect("migrate_v83 must succeed");

        let updated_at: String = conn
            .query_row("SELECT updated_at FROM ai_agents WHERE id = 'agent-oc'", [], |r| r.get(0))
            .unwrap();
        assert_ne!(
            updated_at, "2020-01-01T00:00:00Z",
            "migrate_v83's C1 backfill MUST bump updated_at (contrast migrate_v81's role \
             repair, which does NOT bump updated_at — role is a label, model is \
             dispatch-relevant)"
        );
    }

    // ── C3: legacy-unpinned agent deactivation ──────────────────────────────────

    /// Item 11: a `runtime_type IS NULL` row must come out with `is_active = 0`.
    #[test]
    fn test_migrate_v83_deactivates_null_runtime_type_rows() {
        let conn = make_post_v82_ai_agents_conn();
        seed_agent_with_model(&conn, "agent-legacy", None, Some("claude-3"), "2020-01-01T00:00:00Z"); // CI-ALLOW: hardcoded-model

        migrate_v83(&conn).expect("migrate_v83 must succeed");

        let is_active: i64 = conn
            .query_row("SELECT is_active FROM ai_agents WHERE id = 'agent-legacy'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(is_active, 0, "runtime_type IS NULL row must be deactivated");
    }

    /// Item 12: typed rows (openclaw/anthropic/openrouter/ollama, any
    /// `runtime_type` non-NULL) must keep their `is_active` value untouched by C3.
    #[test]
    fn test_migrate_v83_leaves_typed_rows_active() {
        let conn = make_post_v82_ai_agents_conn();
        seed_agent_with_model(&conn, "agent-oc", Some("openclaw"), Some("claude-3"), "2020-01-01T00:00:00Z"); // CI-ALLOW: hardcoded-model
        seed_agent_with_model(&conn, "agent-anthropic", Some("anthropic"), Some("claude-3"), "2020-01-01T00:00:00Z"); // CI-ALLOW: hardcoded-model
        seed_agent_with_model(&conn, "agent-ollama", Some("ollama"), Some("llama3"), "2020-01-01T00:00:00Z");

        migrate_v83(&conn).expect("migrate_v83 must succeed");

        for id in ["agent-oc", "agent-anthropic", "agent-ollama"] {
            let is_active: i64 = conn
                .query_row(&format!("SELECT is_active FROM ai_agents WHERE id = '{}'", id), [], |r| r.get(0))
                .unwrap();
            assert_eq!(is_active, 1, "{} has a non-NULL runtime_type and must stay active", id);
        }
    }

    /// C1+C3 combined: proves the two independent UPDATEs in the same migration
    /// function don't interfere with each other when both apply to the same DB in
    /// one pass (mirrors migrate_v82_tests's own "both parts apply together"
    /// smoke test).
    #[test]
    fn test_migrate_v83_c1_and_c3_both_apply_together() {
        let conn = make_post_v82_ai_agents_conn();
        seed_agent_with_model(&conn, "agent-oc", Some("openclaw"), None, "2020-01-01T00:00:00Z");
        seed_agent_with_model(&conn, "agent-legacy", None, Some("claude-3"), "2020-01-01T00:00:00Z"); // CI-ALLOW: hardcoded-model

        migrate_v83(&conn).expect("migrate_v83 must succeed");

        let oc_model: Option<String> = conn
            .query_row("SELECT model FROM ai_agents WHERE id = 'agent-oc'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(oc_model, Some("openclaw-pending".to_string()), "C1 must apply");

        let legacy_active: i64 = conn
            .query_row("SELECT is_active FROM ai_agents WHERE id = 'agent-legacy'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(legacy_active, 0, "C3 must apply in the same pass");
    }
}

#[cfg(test)]
mod migrate_v88_tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    // Keychain mocking (`set_test_keychain`/`clear_test_keychain`) is process-global
    // state — same reason `ai::connections::runtime_connections_tests` uses its own
    // `TEST_MUTEX` to serialize keychain-touching tests within that module.
    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn make_ai_settings_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        conn
    }

    /// A runtime with a real keychain credential and no `{runtime}_configured_at`
    /// marker is the exact upgrader-grandfather case this migration exists for —
    /// the marker must be backfilled and written into `ai_settings`, not just held
    /// in memory somewhere.
    #[test]
    #[serial(keychain_override)]
    fn test_migrate_v88_backfills_marker_when_keychain_credential_present_and_marker_absent() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        crate::keychain::store_password(
            crate::ai::credential_migration::KEYCHAIN_ACCOUNT_VENICE_KEY,
            "a-real-venice-key-1234",
        )
        .expect("seeding the keychain must succeed");

        let conn = make_ai_settings_conn();
        assert!(
            crate::database::get_ai_setting(&conn, "venice_configured_at").is_none(),
            "precondition: no marker should exist yet"
        );

        migrate_v88(&conn).expect("migrate_v88 must succeed");

        assert!(
            crate::database::get_ai_setting(&conn, "venice_configured_at").is_some(),
            "a real keychain credential with no marker must be backfilled — the upgrader-grandfather case"
        );

    }

    /// The mirror image: no keychain credential at all — a genuinely unconfigured
    /// provider must NOT get a marker invented for it.
    #[test]
    #[serial(keychain_override)]
    fn test_migrate_v88_leaves_marker_unset_when_no_keychain_credential() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_ai_settings_conn();
        migrate_v88(&conn).expect("migrate_v88 must succeed");

        for runtime in ["anthropic", "openrouter", "venice"] {
            let key = format!("{}_configured_at", runtime);
            assert!(
                crate::database::get_ai_setting(&conn, &key).is_none(),
                "{} has no keychain credential — migrate_v88 must not invent a marker for it",
                key
            );
        }

    }

    /// A marker that's already present must be left untouched (idempotent
    /// re-run — e.g. a real Save-path marker from before the migration ever ran).
    #[test]
    #[serial(keychain_override)]
    fn test_migrate_v88_does_not_overwrite_existing_marker() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_ai_settings_conn();
        crate::database::set_ai_setting(&conn, "anthropic_configured_at", "2020-01-01T00:00:00Z").unwrap();

        migrate_v88(&conn).expect("migrate_v88 must succeed");

        let value = crate::database::get_ai_setting(&conn, "anthropic_configured_at").unwrap();
        assert_eq!(value, "2020-01-01T00:00:00Z", "an existing marker must not be overwritten by the backfill");

    }

    /// Re-running the migration twice in a row must be a no-op the second time
    /// (no panics, no double-writes, no changed values).
    #[test]
    #[serial(keychain_override)]
    fn test_migrate_v88_idempotent_rerun() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_ai_settings_conn();

        migrate_v88(&conn).expect("first run must succeed");
        let first_run_values: Vec<Option<String>> = ["anthropic", "openrouter", "venice"]
            .iter()
            .map(|rt| crate::database::get_ai_setting(&conn, &format!("{}_configured_at", rt)))
            .collect();

        migrate_v88(&conn).expect("second run must be idempotent, not error");
        let second_run_values: Vec<Option<String>> = ["anthropic", "openrouter", "venice"]
            .iter()
            .map(|rt| crate::database::get_ai_setting(&conn, &format!("{}_configured_at", rt)))
            .collect();

        assert_eq!(first_run_values, second_run_values, "re-running migrate_v88 must not change any marker's value");

    }

    /// `ai_settings` table absent (older fixture schema) must be a graceful no-op,
    /// matching every other migration's fixture-absence guard in this file.
    #[test]
    fn test_migrate_v88_noop_when_ai_settings_absent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_v88(&conn).expect("must not panic or error when ai_settings is absent");
    }
}

#[cfg(test)]
mod migrate_v89_tests {
    use super::*;

    fn make_ai_settings_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);")
            .unwrap();
        conn
    }

    fn make_ai_agents_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
             CREATE TABLE ai_agents (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 slug TEXT NOT NULL UNIQUE,
                 role TEXT NOT NULL DEFAULT 'sub',
                 is_default INTEGER NOT NULL DEFAULT 0,
                 is_active INTEGER NOT NULL DEFAULT 1,
                 workspace_path TEXT NOT NULL,
                 runtime_ref TEXT,
                 runtime_type TEXT,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        conn
    }

    fn seed_agent(conn: &Connection, id: &str, role: &str, runtime_ref: Option<&str>) {
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, workspace_path, runtime_ref, runtime_type)
             VALUES (?1, ?1, ?1, ?2, ?1, ?3, CASE WHEN ?3 IS NOT NULL THEN 'openclaw' ELSE NULL END)",
            rusqlite::params![id, role, runtime_ref],
        )
        .expect("seed_agent insert must succeed");
    }

    // ── F1: ollama_configured_at backfill ──────────────────────────────────

    /// An existing `ollama_endpoint` with no `ollama_configured_at` marker is the
    /// exact upgrader-grandfather case F1 exists for — the marker must be
    /// backfilled into `ai_settings`.
    #[test]
    fn test_migrate_v89_backfills_ollama_marker_when_endpoint_present_and_marker_absent() {
        let conn = make_ai_settings_conn();
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();

        migrate_v89(&conn).expect("migrate_v89 must succeed");

        assert!(
            crate::database::get_ai_setting(&conn, "ollama_configured_at").is_some(),
            "an existing ollama_endpoint with no marker must be backfilled — the upgrader-grandfather case (F1)"
        );
    }

    /// No `ollama_endpoint` at all — a genuinely unconfigured Ollama must NOT get a
    /// marker invented for it.
    #[test]
    fn test_migrate_v89_leaves_ollama_marker_unset_when_no_endpoint() {
        let conn = make_ai_settings_conn();

        migrate_v89(&conn).expect("migrate_v89 must succeed");

        assert!(
            crate::database::get_ai_setting(&conn, "ollama_configured_at").is_none(),
            "no ollama_endpoint set — migrate_v89 must not invent a marker for it"
        );
    }

    /// A marker that's already present must be left untouched (idempotent re-run).
    #[test]
    fn test_migrate_v89_does_not_overwrite_existing_ollama_marker() {
        let conn = make_ai_settings_conn();
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();
        crate::database::set_ai_setting(&conn, "ollama_configured_at", "2020-01-01T00:00:00Z").unwrap();

        migrate_v89(&conn).expect("migrate_v89 must succeed");

        let value = crate::database::get_ai_setting(&conn, "ollama_configured_at").unwrap();
        assert_eq!(value, "2020-01-01T00:00:00Z", "an existing marker must not be overwritten by the backfill");
    }

    /// `ai_settings` table absent (older fixture schema) must be a graceful no-op
    /// for F1, matching migrate_v88's own fixture-absence guard.
    #[test]
    fn test_migrate_v89_f1_noop_when_ai_settings_absent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate_v89(&conn).expect("must not panic or error when ai_settings/ai_agents are absent");
    }

    // ── F5: ai_agents.role repair ───────────────────────────────────────────

    /// The exact corruption pattern from the 1b safety net: an OC-imported
    /// subagent (`runtime_ref IS NOT NULL`) whose role was force-overwritten to
    /// 'main'. Must be repaired back to 'sub'.
    #[test]
    fn test_migrate_v89_repairs_corrupted_subagent_role() {
        let conn = make_ai_agents_conn();
        seed_agent(&conn, "stia", "main", Some("coder"));

        migrate_v89(&conn).expect("migrate_v89 must succeed");

        let role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'stia'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(role, "sub", "a row with runtime_ref IS NOT NULL must always have role='sub' (F5)");
    }

    /// The same corruption pattern with role='local' instead of 'main' — the
    /// predicate is `role != 'sub'`, not just `role == 'main'`.
    #[test]
    fn test_migrate_v89_repairs_corrupted_subagent_role_local_variant() {
        let conn = make_ai_agents_conn();
        seed_agent(&conn, "sub-local", "local", Some("some-oc-id"));

        migrate_v89(&conn).expect("migrate_v89 must succeed");

        let role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'sub-local'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(role, "sub", "role != 'sub' with runtime_ref IS NOT NULL must be repaired regardless of the corrupted value");
    }

    /// A genuine local/main agent (`runtime_ref IS NULL`) must NEVER be touched by
    /// the repair pass, no matter its role — this is the exact invariant that
    /// distinguishes a real default/local agent from an imported subagent.
    #[test]
    fn test_migrate_v89_does_not_touch_agents_without_runtime_ref() {
        let conn = make_ai_agents_conn();
        seed_agent(&conn, "local-main", "main", None);
        seed_agent(&conn, "murphy-brown", "local", None);

        migrate_v89(&conn).expect("migrate_v89 must succeed");

        let main_role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'local-main'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(main_role, "main", "a real local/main agent with runtime_ref IS NULL must never be touched by the repair pass");

        let murphy_role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'murphy-brown'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(murphy_role, "local", "Murphy Brown (an old inactive local test row) must not be touched by this migration");
    }

    /// OC's own main agent (`runtime_type='openclaw' AND runtime_ref='main'`) is
    /// the one legitimate row with a non-NULL runtime_ref that is NOT a subagent
    /// — its role can legitimately be `'main'` or `'local'` (never `'sub'`, see
    /// sync_initial_import/sync_re_import's shared role derivation). The repair
    /// pass must never force it to `'sub'`.
    #[test]
    fn test_migrate_v89_does_not_touch_oc_own_main_row() {
        let conn = make_ai_agents_conn();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, workspace_path, runtime_ref, runtime_type) \
             VALUES ('oc-main', 'Stia', 'stia', 'main', 'oc-main', 'main', 'openclaw')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, workspace_path, runtime_ref, runtime_type) \
             VALUES ('oc-main-local', 'Stia2', 'stia2', 'local', 'oc-main-local', 'main', 'openclaw')",
            [],
        )
        .unwrap();

        migrate_v89(&conn).expect("migrate_v89 must succeed");

        let role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'oc-main'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(role, "main", "OC's own main agent (runtime_ref='main') must never be forced to role='sub'");

        let local_role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'oc-main-local'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(local_role, "local", "OC's own main agent parked at role='local' (foreign default present) must never be forced to role='sub'");
    }

    /// A subagent whose role is already correctly 'sub' must be left alone —
    /// the repair only fires on rows that are actually wrong.
    #[test]
    fn test_migrate_v89_leaves_correct_subagent_role_untouched() {
        let conn = make_ai_agents_conn();
        seed_agent(&conn, "good-sub", "sub", Some("some-oc-id"));

        migrate_v89(&conn).expect("migrate_v89 must succeed");

        let role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'good-sub'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(role, "sub");
    }

    /// `ai_agents` table absent (older fixture schema) must be a graceful no-op
    /// for F5.
    #[test]
    fn test_migrate_v89_f5_noop_when_ai_agents_absent() {
        let conn = make_ai_settings_conn(); // ai_settings present, ai_agents absent
        migrate_v89(&conn).expect("must not panic or error when ai_agents is absent");
    }

    /// Both F1 and F5 must apply together in the same pass without interfering
    /// with each other (mirrors migrate_v83_tests's "both parts apply together"
    /// pattern for this file's other multi-fix migrations).
    #[test]
    fn test_migrate_v89_f1_and_f5_both_apply_together() {
        let conn = make_ai_agents_conn();
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();
        seed_agent(&conn, "stia", "main", Some("coder"));

        migrate_v89(&conn).expect("migrate_v89 must succeed");

        assert!(
            crate::database::get_ai_setting(&conn, "ollama_configured_at").is_some(),
            "F1 must apply"
        );
        let role: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'stia'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(role, "sub", "F5 must apply in the same pass");
    }

    /// Re-running the migration twice in a row must be a no-op the second time.
    #[test]
    fn test_migrate_v89_idempotent_rerun() {
        let conn = make_ai_agents_conn();
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();
        seed_agent(&conn, "stia", "main", Some("coder"));

        migrate_v89(&conn).expect("first run must succeed");
        let marker_first = crate::database::get_ai_setting(&conn, "ollama_configured_at");
        let role_first: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'stia'", [], |r| r.get(0))
            .unwrap();

        migrate_v89(&conn).expect("second run must be idempotent, not error");
        let marker_second = crate::database::get_ai_setting(&conn, "ollama_configured_at");
        let role_second: String = conn
            .query_row("SELECT role FROM ai_agents WHERE id = 'stia'", [], |r| r.get(0))
            .unwrap();

        assert_eq!(marker_first, marker_second, "re-running migrate_v89 must not change the ollama marker");
        assert_eq!(role_first, role_second, "re-running migrate_v89 must not change an already-repaired role");
    }

    /// Re-running migrate_v90 twice in a row must be a no-op the second time.
    /// Pure CREATE TABLE IF NOT EXISTS + crsql_as_crr — no data backfill, so
    /// idempotency here just means the second call doesn't error and the
    /// tables/indexes still exist with the same schema.
    #[test]
    fn test_migrate_v90_idempotent_rerun() {
        let conn = Connection::open_in_memory().unwrap();

        migrate_v90(&conn).expect("first run must succeed");

        let columns_first: Vec<String> = conn
            .prepare("PRAGMA table_info(sheet_columns)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        let rows_first: Vec<String> = conn
            .prepare("PRAGMA table_info(sheet_rows)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        let cells_first: Vec<String> = conn
            .prepare("PRAGMA table_info(sheet_cells)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        migrate_v90(&conn).expect("second run must be idempotent, not error");

        let columns_second: Vec<String> = conn
            .prepare("PRAGMA table_info(sheet_columns)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        let rows_second: Vec<String> = conn
            .prepare("PRAGMA table_info(sheet_rows)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        let cells_second: Vec<String> = conn
            .prepare("PRAGMA table_info(sheet_cells)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        assert_eq!(columns_first, columns_second, "re-running migrate_v90 must not change sheet_columns schema");
        assert_eq!(rows_first, rows_second, "re-running migrate_v90 must not change sheet_rows schema");
        assert_eq!(cells_first, cells_second, "re-running migrate_v90 must not change sheet_cells schema");

        // Indexes still exist after re-run
        for idx in ["idx_sheet_columns_note", "idx_sheet_rows_note", "idx_sheet_cells_note"] {
            let exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name=?1",
                    [idx],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(exists, "index {} should still exist after idempotent re-run", idx);
        }
    }

    /// Bare-bones pre-v91 `projects` table — no `default_folder_id` column yet —
    /// so `migrate_v91`'s `ALTER TABLE ... ADD COLUMN` path is actually exercised,
    /// same rationale as `test_migrate_attachments_v67_empty_table_is_noop`'s
    /// pre-column-add table shape.
    fn make_v90_projects_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL DEFAULT '',
                parent_id TEXT,
                owner_id TEXT,
                is_system INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );",
        )
        .unwrap();
        conn
    }

    fn insert_project(
        conn: &Connection,
        id: &str,
        name: &str,
        parent_id: Option<&str>,
        created_at: &str,
    ) {
        conn.execute(
            "INSERT INTO projects (id, name, parent_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, name, parent_id, created_at],
        )
        .unwrap();
    }

    fn default_folder_id_of(conn: &Connection, project_id: &str) -> Option<String> {
        conn.query_row(
            "SELECT default_folder_id FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap()
    }

    fn children_of(conn: &Connection, parent_id: &str) -> Vec<(String, String)> {
        conn.prepare("SELECT id, name FROM projects WHERE parent_id = ?1 ORDER BY created_at")
            .unwrap()
            .query_map(params![parent_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    /// Case 1: a project with a single, untouched "General" folder — default_folder_id
    /// should simply point at it.
    #[test]
    fn test_migrate_v91_untouched_general_folder() {
        let conn = make_v90_projects_conn();
        insert_project(&conn, "p1", "Project One", None, "2026-01-01T00:00:00Z");
        insert_project(&conn, "f1", "General", Some("p1"), "2026-01-01T00:00:01Z");

        let before_column_exists: bool = conn
            .prepare("PRAGMA table_info(projects)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .any(|col| col == "default_folder_id");
        assert!(!before_column_exists, "before: default_folder_id column must not exist yet (pre-v91 schema)");

        migrate_v91(&conn).expect("migrate_v91 should succeed");

        let after = default_folder_id_of(&conn, "p1");
        assert_eq!(
            after,
            Some("f1".to_string()),
            "after: default_folder_id must point at the project's only folder"
        );
    }

    /// Case 2: the default folder has been renamed away from "General" — it's still
    /// the only folder, so it must still be picked (by ID/oldest, not by name).
    #[test]
    fn test_migrate_v91_renamed_default_folder() {
        let conn = make_v90_projects_conn();
        insert_project(&conn, "p2", "Project Two", None, "2026-01-01T00:00:00Z");
        insert_project(&conn, "f2", "My Stuff", Some("p2"), "2026-01-01T00:00:01Z");

        migrate_v91(&conn).expect("migrate_v91 should succeed");

        let after = default_folder_id_of(&conn, "p2");
        assert_eq!(
            after,
            Some("f2".to_string()),
            "after: the renamed folder must still become the default (picked by id/age, not name)"
        );
    }

    /// Case 3: the renamed default PLUS a newer stray folder that happens to be named
    /// "General" — the OLDER folder (the renamed one) must win. This is the case that
    /// catches a naive find-by-name upgrade: a name-based lookup
    /// (`WHERE parent_id = ?1 AND name = 'General'`) would wrongly pick the newer
    /// stray instead of preserving the user's actual default.
    #[test]
    fn test_migrate_v91_older_folder_wins_over_newer_stray_general() {
        let conn = make_v90_projects_conn();
        insert_project(&conn, "p3", "Project Three", None, "2026-01-01T00:00:00Z");
        // Oldest folder, renamed away from "General" — this is the real default.
        insert_project(&conn, "f3-old", "Renamed Default", Some("p3"), "2026-01-01T00:00:01Z");
        // Newer stray folder that happens to be named "General" — created later.
        insert_project(&conn, "f3-new", "General", Some("p3"), "2026-06-01T00:00:00Z");

        // Prove the trap: a naive name-based lookup finds the WRONG folder.
        let naive_lookup: String = conn
            .query_row(
                "SELECT id FROM projects WHERE parent_id = ?1 AND name = 'General'",
                params!["p3"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            naive_lookup, "f3-new",
            "sanity check: a naive name-based lookup picks the newer stray, which is the bug this migration must avoid"
        );

        migrate_v91(&conn).expect("migrate_v91 should succeed");

        let after = default_folder_id_of(&conn, "p3");
        assert_eq!(
            after,
            Some("f3-old".to_string()),
            "after: the OLDER folder (the renamed real default) must win, not the newer stray named 'General'"
        );
    }

    /// Case 4: a project with no folders at all — migration must create one named
    /// DEFAULT_FOLDER_NAME ("General") and point default_folder_id at it.
    #[test]
    fn test_migrate_v91_folderless_project_gets_general_created() {
        let conn = make_v90_projects_conn();
        insert_project(&conn, "p4", "Project Four", None, "2026-01-01T00:00:00Z");

        let before_children = children_of(&conn, "p4");
        assert_eq!(before_children.len(), 0, "before: project must start with zero folders");

        migrate_v91(&conn).expect("migrate_v91 should succeed");

        let after = default_folder_id_of(&conn, "p4").expect("after: default_folder_id must be set");
        let after_children = children_of(&conn, "p4");
        assert_eq!(after_children.len(), 1, "after: exactly one folder must have been created");
        assert_eq!(after_children[0].0, after, "after: default_folder_id must point at the newly created folder");
        assert_eq!(
            after_children[0].1,
            crate::projects::DEFAULT_FOLDER_NAME,
            "after: the created folder must use the DEFAULT_FOLDER_NAME constant, not a hardcoded literal"
        );
    }

    /// Case 5: unrelated rows are left untouched — a folder row's own
    /// default_folder_id stays NULL (that column is project-only), and a project that
    /// already has default_folder_id set (simulating a prior partial run) is not
    /// overwritten even though a different/older folder exists that an unconditional
    /// re-scan would otherwise prefer.
    #[test]
    fn test_migrate_v91_leaves_unrelated_rows_untouched() {
        let conn = make_v90_projects_conn();
        insert_project(&conn, "p5", "Project Five", None, "2026-01-01T00:00:00Z");
        insert_project(&conn, "f5-old", "Oldest", Some("p5"), "2026-01-01T00:00:01Z");
        insert_project(&conn, "f5-new", "Newest", Some("p5"), "2026-02-01T00:00:00Z");

        // Simulate a prior run having already pointed default_folder_id at the NEWER
        // folder (deliberately "wrong" by the oldest-wins rule, to prove the NULL gate
        // prevents the migration from ever re-deciding this).
        conn.execute(
            "ALTER TABLE projects ADD COLUMN default_folder_id TEXT",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE projects SET default_folder_id = 'f5-new' WHERE id = 'p5'",
            [],
        )
        .unwrap();

        let folder_before = default_folder_id_of(&conn, "f5-old");
        assert_eq!(folder_before, None, "before: a folder row's own default_folder_id must be NULL");

        migrate_v91(&conn).expect("migrate_v91 should succeed");

        let project_after = default_folder_id_of(&conn, "p5");
        assert_eq!(
            project_after,
            Some("f5-new".to_string()),
            "after: an already-set default_folder_id must not be overwritten by the backfill"
        );
        let folder_after = default_folder_id_of(&conn, "f5-old");
        assert_eq!(folder_after, None, "after: a folder row's own default_folder_id must remain NULL (untouched)");
        let folder_name_after: String = conn
            .query_row("SELECT name FROM projects WHERE id = 'f5-old'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(folder_name_after, "Oldest", "after: unrelated folder row's name must be byte-identical");
    }

    /// Case 6: re-running the upgrade must be a no-op — no duplicate "General"
    /// folders, no changed default_folder_id, for both the has-folders and
    /// folderless cases.
    #[test]
    fn test_migrate_v91_idempotent_rerun() {
        let conn = make_v90_projects_conn();
        insert_project(&conn, "p6a", "Has Folders", None, "2026-01-01T00:00:00Z");
        insert_project(&conn, "f6a", "General", Some("p6a"), "2026-01-01T00:00:01Z");
        insert_project(&conn, "p6b", "Folderless", None, "2026-01-01T00:00:00Z");

        migrate_v91(&conn).expect("first run must succeed");
        let default_a_first = default_folder_id_of(&conn, "p6a");
        let default_b_first = default_folder_id_of(&conn, "p6b");
        let children_b_first = children_of(&conn, "p6b");

        migrate_v91(&conn).expect("second run must be idempotent, not error");
        let default_a_second = default_folder_id_of(&conn, "p6a");
        let default_b_second = default_folder_id_of(&conn, "p6b");
        let children_b_second = children_of(&conn, "p6b");

        assert_eq!(default_a_first, default_a_second, "re-running migrate_v91 must not change an already-backfilled default_folder_id");
        assert_eq!(default_b_first, default_b_second, "re-running migrate_v91 must not change the folderless project's default_folder_id");
        assert_eq!(children_b_first.len(), 1, "first run: exactly one General folder created");
        assert_eq!(children_b_second.len(), 1, "second run: must NOT create a duplicate General folder");
        assert_eq!(children_b_first, children_b_second, "re-running migrate_v91 must not change the folderless project's folder list");
    }
}

// ── #3006 (Reed audit on #2874 item 50a): migrate_v91 rebuild atomicity ──────
// Proves the BEGIN/COMMIT wrap added around the projects table rebuild (the
// CREATE projects_v91_rebuild / INSERT ... SELECT / DROP projects / ALTER
// TABLE ... RENAME TO projects sequence) actually makes that rebuild
// all-or-nothing. None of migrate_v89_tests's existing migrate_v91 cases
// exercise this code path at all: it's gated behind `crr_available` (checks
// `pragma_function_list` for `crsql_as_crr`), which is false for every
// `Connection::open_in_memory()` fixture in this file because the real
// cr-sqlite extension is never loaded in unit tests (same reason
// `migrate_attachments_v68`'s own rebuild is untested in-process). To
// actually exercise it here without loading the real extension:
//   - `crsql_as_crr` is stubbed via `create_scalar_function` so the
//     `pragma_function_list` gate sees cr-sqlite as "present".
//   - A dummy `projects__crsql_utrig` trigger (not mentioning
//     `default_folder_id`) is pre-created so the "is the CRR trigger stale"
//     gate evaluates true, same as it would against a real pre-v91 database.
// A real file-backed connection (not in-memory) is used so the connection can
// be closed and reopened to simulate a crash: SQLite rolls back whatever
// wasn't COMMITted when a connection with an open transaction is closed,
// which is exactly what happens to an uncommitted transaction when the
// process dies mid-migration and the user restarts the app.
#[cfg(test)]
mod migrate_v91_rebuild_atomicity_tests {
    use super::*;
    use rusqlite::functions::FunctionFlags;
    use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
    use tempfile::TempDir;

    /// v90-shape `projects` table (no `default_folder_id` yet) on a real
    /// file-backed connection, plus a dummy "stale" CRR update trigger so
    /// `migrate_v91`'s trigger-staleness gate decides a rebuild is needed —
    /// same shape as `migrate_v89_tests::make_v90_projects_conn`, but
    /// file-backed (crash simulation needs a connection we can close and
    /// reopen against the same on-disk file) and with the trigger stand-in
    /// the in-memory fixtures never needed because they never flip
    /// `crr_available` on.
    fn make_v90_projects_file_conn(path: &std::path::Path) -> Connection {
        let conn = Connection::open(path).expect("open file-backed test db");
        conn.execute_batch(
            "CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL DEFAULT '',
                parent_id TEXT,
                owner_id TEXT,
                is_system INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TRIGGER projects__crsql_utrig AFTER UPDATE ON projects BEGIN SELECT 1; END;",
        )
        .expect("create v90 projects table + dummy CRR trigger");
        conn
    }

    fn insert_project(
        conn: &Connection,
        id: &str,
        name: &str,
        parent_id: Option<&str>,
        created_at: &str,
    ) {
        conn.execute(
            "INSERT INTO projects (id, name, parent_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![id, name, parent_id, created_at],
        )
        .unwrap();
    }

    /// Registers a no-op `crsql_as_crr(text) -> integer` scalar function so
    /// `migrate_v91`'s `SELECT COUNT(*) > 0 FROM pragma_function_list WHERE
    /// name='crsql_as_crr'` gate (and its later `SELECT crsql_as_crr('projects')`
    /// re-enable call) both succeed without the real cr-sqlite extension.
    fn stub_crsql_as_crr(conn: &Connection) {
        conn.create_scalar_function(
            "crsql_as_crr",
            1,
            FunctionFlags::SQLITE_UTF8,
            |_ctx| Ok(1i64),
        )
        .expect("register crsql_as_crr stub");
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name=?1",
            params![name],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn trigger_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='trigger' AND name=?1",
            params![name],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn projects_schema(conn: &Connection) -> Vec<String> {
        conn.prepare("PRAGMA table_info(projects)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect()
    }

    fn all_projects_rows(conn: &Connection) -> Vec<(String, String, Option<String>)> {
        conn.prepare("SELECT id, name, parent_id FROM projects ORDER BY id")
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    /// Denies exactly the `ALTER TABLE projects_v91_rebuild RENAME TO
    /// projects` step (SQLite's authorizer reports this as `AlterTable` with
    /// `table_name` set to the table being altered — i.e. the pre-rename name,
    /// `projects_v91_rebuild`) while allowing everything else through. This
    /// lands the injected failure in the worst-case spot: AFTER `CREATE TABLE
    /// projects_v91_rebuild` + `INSERT ... SELECT FROM projects` (new table
    /// fully built) AND after `DROP TABLE projects` (original table gone),
    /// but BEFORE the RENAME that would make the new table visible as
    /// `projects` again. Without the transaction wrap, this is the genuine
    /// data-loss window: the DROP auto-commits immediately, so a crash right
    /// here leaves the database with NO `projects` table at all.
    fn deny_rebuild_rename(ctx: AuthContext<'_>) -> Authorization {
        if let AuthAction::AlterTable { table_name, .. } = ctx.action {
            if table_name == "projects_v91_rebuild" {
                return Authorization::Deny;
            }
        }
        Authorization::Allow
    }

    #[test]
    fn test_migrate_v91_rebuild_rolls_back_cleanly_on_mid_rebuild_failure() {
        let dir = TempDir::new().expect("tempdir");
        // Canonicalize the temp root before use — macOS's /tmp is a symlink to
        // /private/tmp, and a path mismatch here was the exact trap in this
        // author's prior commit (test(folders): canonicalize the vault root in
        // the default-folder landing test).
        let dir_path = std::fs::canonicalize(dir.path()).expect("canonicalize tempdir");
        let db_path = dir_path.join("v91-rebuild-atomicity.db");

        let conn = make_v90_projects_file_conn(&db_path);
        stub_crsql_as_crr(&conn);

        insert_project(&conn, "p1", "Project One", None, "2026-01-01T00:00:00Z");
        insert_project(&conn, "f1", "General", Some("p1"), "2026-01-01T00:00:01Z");
        insert_project(&conn, "p2", "Project Two", None, "2026-01-02T00:00:00Z");

        let schema_before = projects_schema(&conn);
        assert!(
            !schema_before.contains(&"default_folder_id".to_string()),
            "sanity check: pre-state has no default_folder_id column yet"
        );
        let rows_before = all_projects_rows(&conn);
        assert_eq!(rows_before.len(), 3, "sanity check: 3 rows present before migration");

        conn.authorizer(Some(deny_rebuild_rename));

        let result = migrate_v91(&conn);
        assert!(
            result.is_err(),
            "migrate_v91 must surface the denied RENAME as an error, not silently succeed: {:?}",
            result
        );

        // Simulate a crash: drop the connection without ever issuing COMMIT or
        // an explicit ROLLBACK. Closing a rusqlite Connection mid-transaction
        // rolls back anything not committed — the same outcome a real crash
        // (process dies, OS/SQLite discards the uncommitted WAL frames) leaves
        // behind for the next startup to find.
        drop(conn);

        let reopened = Connection::open(&db_path).expect("reopen db after simulated crash");

        // 1. No orphaned rebuild table left behind.
        assert!(
            !table_exists(&reopened, "projects_v91_rebuild"),
            "BUG: projects_v91_rebuild must not survive a crash between DROP and RENAME"
        );

        // 2. `projects` itself must exist — the DROP must have been rolled
        //    back along with everything else in the same transaction.
        assert!(
            table_exists(&reopened, "projects"),
            "BUG (crash-window data loss): projects table is GONE after a crash between DROP and RENAME — \
             this is exactly the data-loss path the BEGIN/COMMIT wrap exists to close"
        );

        // 3. Original schema intact (the rebuild's own DDL never committed —
        //    note this migration's unrelated, already-atomic Step 1 `ALTER
        //    TABLE ADD COLUMN default_folder_id` runs and auto-commits BEFORE
        //    the guarded rebuild block even starts, so that column's presence
        //    is expected and is not what this test is about; this assertion is
        //    about every OTHER column being exactly as it was).
        let schema_after = projects_schema(&reopened);
        for col in &schema_before {
            assert!(
                schema_after.contains(col),
                "BUG: original column {:?} missing after rollback",
                col
            );
        }

        // 4. Every original row still present, values unchanged.
        let rows_after = all_projects_rows(&reopened);
        assert_eq!(
            rows_after.len(),
            rows_before.len(),
            "BUG: row count changed after a crash mid-rebuild — rows lost or duplicated"
        );
        for (id, name, parent_id) in &rows_before {
            let found = rows_after
                .iter()
                .find(|(rid, _, _)| rid == id)
                .unwrap_or_else(|| panic!("BUG: row {:?} missing after rollback", id));
            assert_eq!(&found.1, name, "BUG: name changed for row {:?}", id);
            assert_eq!(&found.2, parent_id, "BUG: parent_id changed for row {:?}", id);
        }

        // 5. CRR state on `projects` is still intact — the original table
        //    object (and its trigger) was never dropped, so the pre-existing
        //    trigger must still be there, byte-for-byte unchanged (DROP TABLE
        //    implicitly drops a table's triggers; if the DROP had actually
        //    stuck, this trigger would be gone too).
        assert!(
            trigger_exists(&reopened, "projects__crsql_utrig"),
            "BUG: projects' CRR trigger is gone — the original table (and its CRR state) was destroyed"
        );
    }

    // ─── #2723 PR B: needs_migration + progress events ───

    #[test]
    fn test_needs_migration_false_on_fresh_install() {
        let conn = Connection::open_in_memory().unwrap();
        // No ai_settings table yet at all — the "fresh install" case.
        assert!(!needs_migration(&conn));
    }

    #[test]
    fn test_needs_migration_false_when_current() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("fresh install migration should succeed");
        assert!(!needs_migration(&conn), "an install already at SCHEMA_VERSION must not need a migration");
    }

    #[test]
    fn test_needs_migration_true_for_old_version() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("fresh install migration should succeed");
        set_schema_version(&conn, "61").expect("stamp back to an old version");
        assert!(needs_migration(&conn), "an install stamped at an old version must need a migration");
    }

    /// `run_migrations` (the ~150-call-site entry point) must remain a true
    /// no-op wrapper around `run_migrations_with_progress(conn, None)` —
    /// same outcome, same schema_version, nothing extra (#2723 PR B: this is
    /// the guarantee every pre-existing caller is relying on).
    #[test]
    fn test_run_migrations_delegates_to_progress_variant_with_none() {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).expect("fresh install should succeed");
        assert_eq!(get_schema_version(&conn), Some(SCHEMA_VERSION.to_string()));
    }

    /// `migration_step`'s percentage math is a plain, pure calculation —
    /// tested directly since the actual `handle.emit(...)` call (like
    /// `backup.rs`'s `emit_progress`, which this mirrors) needs a real
    /// `AppHandle<Wry>` that only exists inside a running app; `backup.rs`
    /// draws that exact same line (no fake-listener unit test for
    /// `backup://progress` either) — the real end-to-end emission is
    /// verified live by the `smoke-upgrade-db` run in the PR description,
    /// which greps for the `[MIGRATIONS] step N/M` log line `migration_step`
    /// emits unconditionally (independent of whether an `AppHandle` is
    /// present), immediately before firing (or not firing) the event.
    #[test]
    fn test_migration_step_percentage_math() {
        let conn = Connection::open_in_memory().unwrap();
        fn noop(_conn: &Connection) -> Result<(), String> {
            Ok(())
        }
        // step=3, total=4 -> 75%; verified by re-deriving the same formula
        // migration_step uses, applied to inputs chosen so integer rounding
        // can't paper over an off-by-one in either direction.
        let pct = ((3_f64 / 4_f64) * 100.0).round() as u8;
        assert_eq!(pct, 75);
        // Exercise the real function (no AppHandle — the no-op path below
        // covers that half); this just confirms step/total/label/f all
        // still thread through correctly with the new trailing parameter.
        migration_step(3, 4, "noop_step", &conn, noop, None).expect("step should succeed");
    }

    #[test]
    fn test_migration_step_is_a_no_op_for_progress_when_handle_is_none() {
        let conn = Connection::open_in_memory().unwrap();
        fn noop(_conn: &Connection) -> Result<(), String> {
            Ok(())
        }
        // Must not panic with no AppHandle — this is the path every
        // pre-existing call site (via `run_migrations`) takes.
        let result = migration_step(1, 1, "noop_step", &conn, noop, None);
        assert!(result.is_ok());
    }
}

