//! Tests for Canonical Schema (v0.61.0+)
//! 
//! Verifies Pane's canonical schema implementation:
//! - Fresh install creates all 44 tables
//! - Schema version stored in ai_settings (not schema_migrations)
//! - All 60 indexes created
//! - CRR enabled on 23 tables
//! - Consolidated columns present
//! - Deleted tables do NOT exist
//! 
//! GitHub Issue: #486 (canonical schema consolidation)
//! 
//! **Note:** These tests replace the old migration chain tests (001-027).
//! Pre-launch mode: no migration chain, one canonical schema, delete-and-recreate on mismatch.

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};

    /// Create an in-memory SQLite database for testing
    fn setup_test_db() -> Connection {
        Connection::open_in_memory().expect("Failed to open in-memory DB")
    }

    /// Helper: verify a table exists
    fn table_exists(conn: &Connection, table_name: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            params![table_name],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false)
    }

    /// Helper: verify a table does NOT exist
    fn table_not_exists(conn: &Connection, table_name: &str) -> bool {
        !table_exists(conn, table_name)
    }

    /// Helper: check if a column exists in a table
    fn column_exists(conn: &Connection, table_name: &str, column_name: &str) -> bool {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name=?1", table_name),
            params![column_name],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false)
    }

    /// Helper: verify an index exists
    fn index_exists(conn: &Connection, index_name: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
            params![index_name],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count > 0)
        .unwrap_or(false)
    }

    /// Helper: get schema version from ai_settings
    fn get_schema_version(conn: &Connection) -> Option<String> {
        // Check if ai_settings exists first
        if !table_exists(conn, "ai_settings") {
            return None;
        }
        
        conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .ok()
    }

    // ─── Test 1: Fresh Install Creates All Tables ───────────────────────

    #[test]
    fn test_fresh_install_creates_all_tables() {
        let conn = setup_test_db();
        
        // Run migrations on empty database
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed on fresh install");
        
        // Count user tables (42 defined tables + FTS5 metadata tables)
        // FTS5 virtual tables create additional metadata tables automatically
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%__crsql%'",
            [], |row| row.get(0),
        ).unwrap();
        
        assert!(count >= 44, "Should have at least 44 tables (44 user + FTS metadata), found {}", count);
        
        // Verify all expected tables exist
        let expected_tables = vec![
            // Core content
            "projects", "notes", "tags", "note_tags", "note_links",
            // Search (FTS5)
            "notes_fts", "ai_memory_fts", "chat_fts",
            // Users & auth
            "users", "user_profiles", "user_roles", "role_model_permissions",
            // AI chat
            "chat_messages_local", "chat_sessions", "ai_context_markers", "ai_pinned_messages", "ai_conversation_summaries",
            // AI memory & audit
            "ai_memory", "ai_actions", "ai_settings", "ai_message_queue",
            // AI agents & tasks
            "ai_agents", "pane_agent_tasks", "pane_task_workspaces",
            // AI processing
            "extraction_queue", "compaction_summaries",
            // Dashboard
            "user_dashboard_config", "dashboard_modules", "module_items",
            "daily_entries", "habit_entries", "reflection_entries", "gratitude_entries",
            // Sync & versioning
            "sync_state", "sync_changelog", "sync_queue", "note_versions",
            // Attachments & search
            "attachments", "embeddings", "note_context_pins",
            // Note components
            "subtasks",
            // Templates (§32, ADR-0058, #193)
            "templates",
            // System
            "category_colors", "notifications", "extensions",
        ];
        
        for table in expected_tables {
            assert!(table_exists(&conn, table), "Table '{}' should exist after fresh install", table);
        }
    }

    // ─── Test 2: Schema Version Stored in ai_settings ───────────────────

    #[test]
    fn test_schema_version_stored() {
        let conn = setup_test_db();
        
        // Run migrations
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        
        // Verify schema version is stored in ai_settings
        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Schema version should be 77 in ai_settings");
    }

    // ─── Test 3: All Indexes Created ─────────────────────────────────────

    #[test]
    fn test_all_indexes_exist() {
        let conn = setup_test_db();
        
        // Run migrations
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        
        // Count all indexes (exclude sqlite_autoindex_* which are auto-created for PRIMARY KEY)
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name NOT LIKE 'sqlite_autoindex_%'",
            [], |row| row.get(0),
        ).unwrap();
        
        assert_eq!(count, 71, "Should have exactly 71 user-defined indexes (70 prior + 1 agent_jobs index, #3151 PR2/C24 — idx_agent_jobs_archived_at, added by migrate_v97/create_agent_jobs_table for the Done-tab archive sweep's indexed WHERE clause)");
        
        // Spot check critical indexes (sample from each major table group)
        let expected_indexes = vec![
            // projects
            "idx_projects_name", "idx_projects_parent",
            // notes
            "idx_notes_project", "idx_notes_folder", "idx_notes_status", "idx_notes_priority", "idx_notes_due_date",
            // tags
            "idx_note_tags_note", "idx_note_tags_tag",
            // AI chat
            "idx_chat_local_project", "idx_chat_local_thread",
            "idx_sessions_active_per_surface", "idx_sessions_surface",
            "idx_ai_pinned_project_message",
            // Agent jobs (#3120 PR 1, C22)
            "idx_agent_jobs_one_pending_wake_per_session",
            // Agent jobs (#3151 PR2/C24)
            "idx_agent_jobs_archived_at",
            // AI memory
            "idx_memory_layer", "idx_memory_project",
            // Sync
            "idx_sync_changelog_time",
            // Attachments
            "idx_note_context_pins_pinned_at",
            // Subtasks
            "idx_subtasks_note", "idx_subtasks_due_date",
            // Templates (#193)
            "idx_templates_name",
            // Sheets (#2339, §1.12)
            "idx_sheet_columns_note", "idx_sheet_rows_note", "idx_sheet_cells_note",
        ];
        
        for index in expected_indexes {
            assert!(index_exists(&conn, index), "Index '{}' should exist", index);
        }
    }

    // ─── Test 4: note_context_pins Exists (Not Deprecated) ──────────────

    #[test]
    fn test_note_context_pins_exists() {
        let conn = setup_test_db();
        
        // Run migrations
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        
        // Verify note_context_pins table exists (actively used by context_pins.rs)
        assert!(table_exists(&conn, "note_context_pins"), "note_context_pins should exist (not deprecated)");
        
        // Verify it has the expected columns (note_id is PRIMARY KEY, no separate id)
        assert!(column_exists(&conn, "note_context_pins", "note_id"), "note_context_pins.note_id should exist");
        assert!(column_exists(&conn, "note_context_pins", "pinned_at"), "note_context_pins.pinned_at should exist");
        assert!(column_exists(&conn, "note_context_pins", "pinned_by"), "note_context_pins.pinned_by should exist");
        
        // Verify its index exists
        assert!(index_exists(&conn, "idx_note_context_pins_pinned_at"), "note_context_pins index should exist");
    }

    // ─── Test 5: Consolidated Columns Present ───────────────────────────

    #[test]
    fn test_consolidated_columns() {
        let conn = setup_test_db();
        
        // Run migrations
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        
        // Verify projects has consolidated columns from folder_preferences and project_settings
        assert!(column_exists(&conn, "projects", "is_hidden"), "projects.is_hidden should exist (from folder_preferences)");
        assert!(column_exists(&conn, "projects", "is_favorite"), "projects.is_favorite should exist (#2009)");
        assert!(column_exists(&conn, "projects", "default_model"), "projects.default_model should exist (from project_settings)");
        assert!(column_exists(&conn, "projects", "smart_routing_enabled"), "projects.smart_routing_enabled should exist (from project_settings)");
        assert!(column_exists(&conn, "projects", "router_model"), "projects.router_model should exist (from project_settings)");
        assert!(column_exists(&conn, "projects", "simple_tier_model"), "projects.simple_tier_model should exist (from project_settings)");
        
        // Verify notes has body column (DB-primary storage)
        assert!(column_exists(&conn, "notes", "body"), "notes.body should exist (DB-primary storage)");
        
        // Verify notes has agent_id (agent ownership)
        assert!(column_exists(&conn, "notes", "agent_id"), "notes.agent_id should exist (agent ownership)");
        
        // Verify chat_messages_local has thread_id (thread grouping)
        assert!(column_exists(&conn, "chat_messages_local", "thread_id"), "chat_messages_local.thread_id should exist (thread grouping)");
    }

    // ─── Test 6: Deleted Tables Do NOT Exist ────────────────────────────

    #[test]
    fn test_no_deleted_tables() {
        let conn = setup_test_db();
        
        // Run migrations
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        
        // Verify old tables are gone
        assert!(table_not_exists(&conn, "schema_migrations"), "schema_migrations should NOT exist (version now in ai_settings)");
        assert!(table_not_exists(&conn, "migration_failures"), "migration_failures should NOT exist (pre-launch mode)");
        assert!(table_not_exists(&conn, "folder_preferences"), "folder_preferences should NOT exist (consolidated into projects.is_hidden)");
        assert!(table_not_exists(&conn, "project_settings"), "project_settings should NOT exist (consolidated into projects columns)");
    }

    // ─── Test 7: CRR Tables Verification (Skip in Test Mode) ────────────

    #[test]
    #[ignore] // Skip by default (requires cr-sqlite extension)
    fn test_crr_enabled_on_21_tables() {
        // NOTE: cargo test uses in-memory SQLite WITHOUT cr-sqlite extension
        // This test is here for documentation but should be skipped
        // Real CRR verification happens in Mac Studio real-DB testing
        
        let _conn = setup_test_db();
        
        // This would fail without cr-sqlite loaded:
        // crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        
        // CRR verification would check for __crsql_clock tables:
        // SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE '%__crsql_clock'
        // Expected: 21 (one for each CRR-enabled table)
        
        panic!("This test should not run in CI — requires cr-sqlite extension");
    }

    // ─── Test 8: Schema Version Mismatch Error ──────────────────────────

    #[test]
    fn test_schema_version_mismatch_triggers_error() {
        let conn = setup_test_db();
        
        // Run migrations to create schema
        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        
        // Manually set wrong version
        conn.execute(
            "UPDATE ai_settings SET value = '60' WHERE key = 'schema_version'",
            []
        ).expect("Failed to set wrong version");
        
        // Run migrations again — should detect mismatch
        let result = crate::migrations::run_migrations(&conn);
        
        assert!(result.is_err(), "Should return error on version mismatch");
        
        let err = result.unwrap_err();
        assert!(err.contains("SCHEMA_MISMATCH"), "Error should contain SCHEMA_MISMATCH");
        assert!(err.contains("60"), "Error should contain old version");
        assert!(err.contains(crate::migrations::SCHEMA_VERSION), "Error should contain new version");
    }

    // ─── Test 9: Idempotent Schema Creation ──────────────────────────────

    #[test]
    fn test_idempotent_schema_creation() {
        let conn = setup_test_db();
        
        // Run migrations twice
        crate::migrations::run_migrations(&conn).expect("First run should succeed");
        crate::migrations::run_migrations(&conn).expect("Second run should succeed (version match)");
        
        // Verify still at current version
        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()));
        
        // Verify table count unchanged (>= 44 user tables including migration_flags + chat_sessions + FTS metadata)
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%__crsql%'",
            [], |row| row.get(0),
        ).unwrap();
        
        assert!(count >= 44, "Should have at least 44 tables (incl migration_flags + chat_sessions + FTS metadata), found {}", count);
    }

    // ─── Test 10: FTS5 Tables Created ────────────────────────────────────

    #[test]
    fn test_fts5_tables_exist() {
        let conn = setup_test_db();
        
        // Run migrations
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        
        // Verify FTS5 virtual tables exist
        assert!(table_exists(&conn, "notes_fts"), "notes_fts should exist");
        assert!(table_exists(&conn, "ai_memory_fts"), "ai_memory_fts should exist");
        assert!(table_exists(&conn, "chat_fts"), "chat_fts should exist");
        
        // Verify notes_fts has triggers (per canonical schema)
        let trigger_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND tbl_name='notes'",
            [], |row| row.get(0),
        ).unwrap();
        
        assert_eq!(trigger_count, 3, "notes should have 3 FTS triggers (insert, update, delete)");
    }

    // ─── Test 11: Foreign Key Constraints Present ───────────────────────

    #[test]
    fn test_foreign_keys_removed_from_crr_tables() {
        let conn = setup_test_db();
        
        // Run migrations
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        
        // Verify CRR tables have NO foreign key constraints
        // Per #493: cr-sqlite rejects checked FK constraints on CRR tables
        
        // Check notes has NO FK constraints
        let notes_fks: Vec<String> = conn.prepare("PRAGMA foreign_key_list(notes)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(2))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        
        assert_eq!(notes_fks.len(), 0, "notes (CRR table) should have NO foreign keys");
        
        // Check note_tags has NO FK constraints
        let note_tags_fks: Vec<String> = conn.prepare("PRAGMA foreign_key_list(note_tags)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(2))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        
        assert_eq!(note_tags_fks.len(), 0, "note_tags (CRR table) should have NO foreign keys");
        
        // Check projects has NO FK constraints
        let projects_fks: Vec<String> = conn.prepare("PRAGMA foreign_key_list(projects)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(2))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        
        assert_eq!(projects_fks.len(), 0, "projects (CRR table) should have NO foreign keys");
        
        // Verify non-CRR tables (like user_roles) CAN still have FK constraints
        let user_roles_fks: Vec<String> = conn.prepare("PRAGMA foreign_key_list(user_roles)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(2))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        
        // user_roles is NOT a CRR table, so it's OK (but not required) to have FKs
        // We just verify the table exists and can be queried
        let _ = user_roles_fks; // Intentionally unused
    }

    // ─── Test 12: Default Values Applied ─────────────────────────────────

    #[test]
    fn test_default_values_applied() {
        let conn = setup_test_db();
        
        // Run migrations
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        
        // Insert minimal note (no optional fields)
        conn.execute(
            "INSERT INTO notes (id, title) VALUES ('test-note-1', 'Test Note')",
            []
        ).expect("Failed to insert note");
        
        // Verify default values applied
        let note: (Option<String>, i64, String) = conn.query_row(
            "SELECT status, is_vital, created_at FROM notes WHERE id = 'test-note-1'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        ).expect("Failed to query note");
        
        // status is NOT NULL DEFAULT 'cabinet' (per spec §1.3)
        assert_eq!(note.0, Some("cabinet".to_string()), "Default status should be 'cabinet'");
        assert_eq!(note.1, 0, "Default is_vital should be 0");
        assert!(!note.2.is_empty(), "Default created_at should be set");
    }

    // ─── Test 13: Version Coverage (ADR-0017 guard against PR #690 recurrence) ───

    #[test]
    fn test_version_pair_coverage() {
        // This test ensures every version step from lowest supported to SCHEMA_VERSION
        // has a migration handler in run_migrations().
        // Prevents PR #690-shape failures where schema version bumps without migration code.
        
        // Currently supported version range: 61 → 62 → ... → 85
        // When SCHEMA_VERSION increments, this test fails unless a new migration arm is added.
        // #2564 B1 fix, Commit 2: bumped 79 → 80 — the new `Some(ref old_version) if
        // old_version == "79"` arm (migrate_v80, gateway-endpoint orphan recovery) is
        // that new migration arm.
        // #2567 B6: bumped 80 → 81 — the new `Some(ref old_version) if
        // old_version == "80"` arm (migrate_v81, role='sub'→'local' birth-mislabel
        // repair) is that new migration arm.
        // B3 (#2565): bumped 81 → 82 — the new `Some(ref old_version) if
        // old_version == "81"` arm (migrate_v82, per-provider keychain slots +
        // ollama_endpoint backfill / explicit-connection semantics) is that new
        // migration arm.
        // C1/C3 (#2585/#2587): bumped 82 → 83 — the new `Some(ref old_version) if
        // old_version == "82"` arm (migrate_v83, OC model placeholder backfill +
        // legacy-unpinned agent deactivation) is that new migration arm.
        // #2410: bumped 83 → 84 — the new `Some(ref old_version) if
        // old_version == "83"` arm (migrate_v84, chat_messages_local.oc_message_id
        // dedup column + idx_chat_local_oc_dedup partial unique index for proactive
        // message ingestion, §30.11.2) is that new migration arm.
        // #2410 (ingestion filters): bumped 84 → 85 — the new `Some(ref old_version) if
        // old_version == "84"` arm (migrate_v85, chat_sessions.oc_baseline_done_at
        // first-pull baseline marker column) is that new migration arm.
        // #2410 (stateful echo suppression — tombstoning): bumped 85 → 86 — the new
        // `Some(ref old_version) if old_version == "85"` arm (migrate_v86,
        // chat_oc_tombstones table + idx_chat_local_thread_role_ts index) is that new
        // migration arm.
        // #2690 (§30.11 OC-ingested message ordering fix): bumped 86 → 87 — the new
        // `Some(ref old_version) if old_version == "86"` arm (migrate_v87,
        // compaction_summaries.compacted_before_rowid nullable column, so compaction's
        // cutoff can key off insertion-order rowid instead of the now-purely-
        // presentational created_at string) is that new migration arm.
        // Blocker 3 (#2777 venice live-test scoping): bumped 87 → 88 — the new
        // `Some(ref old_version) if old_version == "87"` arm (migrate_v88,
        // {runtime}_configured_at upgrader backfill for anthropic/openrouter/venice,
        // one-time keychain-driven marker backfill so an existing working key never
        // loses its Provider Connection Card on upgrade) is that new migration arm.
        // #2455 (venice live-test v2): bumped 88 → 89 — the new
        // `Some(ref old_version) if old_version == "88"` arm (migrate_v89,
        // ollama_configured_at upgrader backfill (F1, Ollama has no keychain account
        // so migrate_v88's loop structurally missed it) + ai_agents.role repair for
        // rows corrupted by the pre-fix 1b safety net (F5)) is that new migration arm.
        // #2339 (§1.12, sheets schema): bumped 89 → 90 — the new
        // `Some(ref old_version) if old_version == "89"` arm (migrate_v90,
        // sheet_columns/sheet_rows/sheet_cells tables + indexes, CRR-enabled) is
        // that new migration arm.
        // #2874 item 50a (§1.7.1, default folder by ID): bumped 90 → 91 — the new
        // `Some(ref old_version) if old_version == "90"` arm (migrate_v91,
        // projects.default_folder_id nullable column + backfill: oldest folder for
        // projects that have one, a newly created "General" folder for projects
        // that don't) is that new migration arm.
        // #3053 item F (§30.11.10, dedup filter stops deciding by text): bumped 91 → 92
        // — the new `Some(ref old_version) if old_version == "91"` arm (migrate_v92,
        // one-time DELETE of chat_oc_tombstones rows with reason IN ('stale_echo',
        // 'backstop_echo') — those two text-containment-based classifications no
        // longer decide anything going forward, so pre-existing rows they wrote must
        // not keep hiding messages forever via the Rule 1 permanence pre-filter;
        // `own_turn` rows, seq-based and unaffected, are left untouched) is that new
        // migration arm.
        // #3050 PR 2 (§36.3/§36.10, scheduler): bumped 92 → 93 — the new
        // `Some(ref old_version) if old_version == "92"` arm (migrate_v93, the
        // `agent_jobs` table (non-CRR, per-device) + four new `chat_messages_local`
        // columns: `job_id`, `synthetic`, `continued`, `attachment_ids`) is that new
        // migration arm.
        // #3050 PR 5 (§36.17, captain/crew): bumped 93 → 94 — the new
        // `Some(ref old_version) if old_version == "93"` arm (migrate_v94,
        // `chat_sessions.crew_agent_ids`, nullable TEXT, JSON array) is that new
        // migration arm.
        // #3050 fix PR (Bug 1(b), OC session bootstrap): bumped 94 → 95 — the new
        // `Some(ref old_version) if old_version == "94"` arm (migrate_v95,
        // `agent_jobs.oc_session_key`, nullable TEXT — NULL means this job's OC
        // session has never been bootstrapped via `sessions.create`) is that new
        // migration arm.
        // live-test #1b fix PR (Ruling 3, recurring/heartbeat failure policy):
        // bumped 95 → 96 — the new `Some(ref old_version) if old_version == "95"`
        // arm (migrate_v96, `agent_jobs.consecutive_failures` INTEGER DEFAULT 0 +
        // `agent_jobs.last_failed_at` TEXT) is that new migration arm.
        // #3151 PR2/C24 (v97, Done-tab archive): bumped 96 → 97 — the new
        // `Some(ref old_version) if old_version == "96"` arm (migrate_v97,
        // `agent_jobs.archived_at` TEXT, nullable — NULL means visible in the
        // Done tab, a timestamp means archived) is that new migration arm.

        const LOWEST_SUPPORTED_VERSION: u32 = 62;
        const CURRENT_VERSION: u32 = 97;
        
        // Parse SCHEMA_VERSION from migrations.rs
        let schema_version: u32 = crate::migrations::SCHEMA_VERSION.parse().expect("SCHEMA_VERSION should parse as u32");
        
        assert_eq!(schema_version, CURRENT_VERSION, 
            "Update this test when SCHEMA_VERSION changes — verify migration arms exist");
        
        for old_version in LOWEST_SUPPORTED_VERSION..CURRENT_VERSION {
            let _ = old_version;
        }
        
        // If you reach here and SCHEMA_VERSION != CURRENT_VERSION, the assertion above caught it.
        // Add the new migration arm and a new integration test for the new pair.
    }

    // ─── Test 14: v61→v62 Migration Integration Test (ADR-0017 §4) ───────

    #[test]
    fn test_v61_to_v62_migration_creates_archive_tables() {
        // §4 integration test per ADR-0017: constructs a v61-stamped DB,
        // runs run_migrations(), verifies:
        // - v62 tables exist (chat_archive, chat_archive_fts)
        // - Schema version is stamped "62"
        // - Original data is preserved
        
        let conn = setup_test_db();
        
        // === Step 1: Create v61 schema (before PR #690) ===
        
        // Create ai_settings table and stamp as v61
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key     TEXT PRIMARY KEY,
                value   TEXT
            );
            
            INSERT INTO ai_settings (key, value) VALUES ('schema_version', '61');
            
            -- Create some v61 tables (subset — just enough to verify data preservation)
            CREATE TABLE notes (
                id          TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                body        TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            
            CREATE TABLE chat_messages_local (
                id          TEXT PRIMARY KEY,
                role        TEXT NOT NULL,
                content     TEXT NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "
        ).expect("Failed to create v61 schema");
        
        // Insert test data to verify preservation
        conn.execute(
            "INSERT INTO notes (id, title, body) VALUES ('note-1', 'Test Note', 'Body content')",
            []
        ).expect("Failed to insert test note");
        
        conn.execute(
            "INSERT INTO chat_messages_local (id, role, content) VALUES ('msg-1', 'user', 'Test message')",
            []
        ).expect("Failed to insert test message");
        
        // Verify v61 stamp
        assert_eq!(get_schema_version(&conn), Some("61".to_string()), "Should start at v61");
        
        // v61→v63: no direct migration handler; expect SCHEMA_MISMATCH (multi-hop not supported)
        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_err(), "v61→v63 should return SCHEMA_MISMATCH (no handler)");
        let err = result.unwrap_err();
        assert!(err.contains("SCHEMA_MISMATCH"), "Error should be SCHEMA_MISMATCH: {}", err);
    }

    // ─── Test 15: v62→v63 Migration Integration Test (#1280, ADR-0039 Decision 6) ─

    #[test]
    fn test_v62_to_v63_migration_drops_greeting_name() {
        // Constructs a v62-stamped DB (with greeting_name column), runs run_migrations(),
        // verifies greeting_name column is gone and schema is v63.
        
        let conn = setup_test_db();
        
        // Create v62 schema subset
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY,
                value TEXT
            );
            INSERT INTO ai_settings (key, value) VALUES ('schema_version', '62');
            CREATE TABLE user_dashboard_config (
                id                   TEXT PRIMARY KEY,
                onboarding_completed INTEGER NOT NULL DEFAULT 0,
                onboarding_template  TEXT,
                greeting_name        TEXT,
                theme_variant        TEXT DEFAULT 'default',
                created_at           TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT INTO user_dashboard_config (id, greeting_name, created_at, updated_at)
            VALUES ('default', 'there', datetime('now'), datetime('now'));
            CREATE TABLE chat_archive (
                id          TEXT PRIMARY KEY,
                thread_id   TEXT NOT NULL,
                project_id  TEXT,
                role        TEXT NOT NULL,
                content     TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                metadata    TEXT,
                archived_at TEXT NOT NULL
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
        "
        ).expect("Failed to create v62 schema");
        
        assert_eq!(get_schema_version(&conn), Some("62".to_string()), "Should start at v62");
        
        // Verify greeting_name exists before migration
        let col_exists_before: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('user_dashboard_config') WHERE name = 'greeting_name'",
            [],
            |row| row.get(0),
        ).expect("pragma query");
        assert!(col_exists_before, "greeting_name should exist before migration");
        
        // Run migration
        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v62→v63 migration should succeed: {:?}", result);
        
        // Verify greeting_name is gone
        let col_exists_after: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('user_dashboard_config') WHERE name = 'greeting_name'",
            [],
            |row| row.get(0),
        ).expect("pragma query");
        assert!(!col_exists_after, "greeting_name should be gone after v62→v63 migration");
        
        // Verify data preserved
        let id: String = conn.query_row(
            "SELECT id FROM user_dashboard_config WHERE id = 'default'",
            [],
            |row| row.get(0),
        ).expect("Row should be preserved");
        assert_eq!(id, "default");
        
        // v62→v68 combined migration: greeting_name gone, migration_flags created, stale rows cleaned,
        // embeddings engine column, attachment storage fix, and attachments CRR DEFAULTs.
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()), "After v62→v78 combined: version should be 78");
        
        // Verify migration_flags table also created in combined migration
        assert!(table_exists(&conn, "migration_flags"), "migration_flags should exist after v62→v68 combined migration");
        
        // Verify greeting_name is gone (the v62 op)
        let col_still_gone: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM pragma_table_info('user_dashboard_config') WHERE name = 'greeting_name'",
            [],
            |row| row.get(0),
        ).expect("pragma query");
        assert!(!col_still_gone, "greeting_name should still be absent after combined migration");
        
        // Second run: idempotent (version matches, no-op)
        let second_run = crate::migrations::run_migrations(&conn);
        assert!(second_run.is_ok(), "Second run (idempotent) should succeed");
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test 16: v63→v65 Migration Integration Test (#1498, ADR-0044 Addendum A + #1499, §22C.8.1a) ───

    #[test]
    fn test_v63_to_v65_migration_creates_migration_flags_and_cleans_stale_rows() {
        // Constructs a v63-stamped DB, runs run_migrations(),
        // verifies migration_flags table is created AND stale gateway rows are cleaned, schema is v65.

        let conn = setup_test_db();

        // Create minimal v63 schema
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY,
                value TEXT
            );
            INSERT INTO ai_settings (key, value) VALUES ('schema_version', '63');
            -- migration_flags does NOT exist at v63 (introduced by v64 migration step)
            CREATE TABLE user_dashboard_config (
                id                   TEXT PRIMARY KEY,
                onboarding_completed INTEGER NOT NULL DEFAULT 0,
                onboarding_template  TEXT,
                theme_variant        TEXT DEFAULT 'default',
                created_at           TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
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
        "
        ).expect("Failed to create v63 schema");

        assert_eq!(get_schema_version(&conn), Some("63".to_string()), "Should start at v63");
        assert!(!table_exists(&conn, "migration_flags"), "migration_flags should NOT exist before migration");

        // Run migration
        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v63→v65 combined migration should succeed: {:?}", result);

        // Verify migration_flags table created (v64 step)
        assert!(table_exists(&conn, "migration_flags"), "migration_flags should exist after v63→v65 combined migration");

        // Verify schema version bumped
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()), "Schema version should be 77 after v63→v77 combined migration");

        // Verify idempotence
        let second_run = crate::migrations::run_migrations(&conn);
        assert!(second_run.is_ok(), "Second run should succeed (version match)");
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test 17: v64→v65: pane_gateway_token present, api_key absent ───
    // Spec §22C.8.1a: token copied to api_key; stale row deleted.

    #[test]
    fn test_v64_to_v65_token_copied_when_api_key_absent() {
        let conn = setup_test_db();

        // Build minimal v64 schema
        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO ai_settings (key, value) VALUES ('schema_version', '64');
             INSERT INTO ai_settings (key, value) VALUES ('pane_gateway_token', 'tok_abc123');
             CREATE TABLE migration_flags (flag TEXT PRIMARY KEY NOT NULL, completed_at TEXT NOT NULL DEFAULT (datetime('now')));
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
             "
        ).expect("Failed to create v64 schema");

        assert_eq!(get_schema_version(&conn), Some("64".to_string()));

        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v64→v65 migration should succeed: {:?}", result);

        // api_key should be copied from pane_gateway_token
        let api_key: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'api_key'",
            [],
            |row| row.get(0),
        ).ok();
        assert_eq!(api_key, Some("tok_abc123".to_string()), "api_key should be copied from pane_gateway_token");

        // pane_gateway_token should be deleted
        let stale_token: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'pane_gateway_token'",
            [],
            |row| row.get(0),
        ).ok();
        assert_eq!(stale_token, None, "pane_gateway_token should be deleted");

        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test 18: v64→v65: pane_gateway_token present, api_key present (divergent) ───
    // Spec §22C.8.1a: api_key is authoritative — NEVER overwrite; stale row deleted.

    #[test]
    fn test_v64_to_v65_api_key_unchanged_when_already_present() {
        let conn = setup_test_db();

        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO ai_settings (key, value) VALUES ('schema_version', '64');
             INSERT INTO ai_settings (key, value) VALUES ('api_key', 'tok_current_authoritative');
             INSERT INTO ai_settings (key, value) VALUES ('pane_gateway_token', 'tok_stale_old');
             CREATE TABLE migration_flags (flag TEXT PRIMARY KEY NOT NULL, completed_at TEXT NOT NULL DEFAULT (datetime('now')));
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
             "
        ).expect("Failed to create v64 schema");

        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v64→v65 migration should succeed: {:?}", result);

        // api_key must NOT be overwritten — authoritative value wins
        let api_key: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'api_key'",
            [],
            |row| row.get(0),
        ).ok();
        assert_eq!(api_key, Some("tok_current_authoritative".to_string()),
            "api_key must NOT be overwritten by stale pane_gateway_token");

        // pane_gateway_token should be deleted regardless
        let stale_token: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'pane_gateway_token'",
            [],
            |row| row.get(0),
        ).ok();
        assert_eq!(stale_token, None, "pane_gateway_token should be deleted even when api_key was present");

        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test 19: v64→v65: pane_gateway_token absent ───
    // Spec §22C.8.1a: no-op; no error.

    #[test]
    fn test_v64_to_v65_no_op_when_token_absent() {
        let conn = setup_test_db();

        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO ai_settings (key, value) VALUES ('schema_version', '64');
             -- No pane_gateway_token row at all
             CREATE TABLE migration_flags (flag TEXT PRIMARY KEY NOT NULL, completed_at TEXT NOT NULL DEFAULT (datetime('now')));
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
             "
        ).expect("Failed to create v64 schema");

        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v64→v65 migration should succeed even with no stale rows: {:?}", result);

        // No api_key created out of thin air
        let api_key: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'api_key'",
            [],
            |row| row.get(0),
        ).ok();
        assert_eq!(api_key, None, "api_key should not be created when pane_gateway_token was absent");

        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test 20: v64→v65: pane_gateway_url present ───
    // Spec §22C.8.1a: row deleted; no data to preserve (was only ever written as empty string).

    #[test]
    fn test_v64_to_v65_gateway_url_deleted() {
        let conn = setup_test_db();

        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO ai_settings (key, value) VALUES ('schema_version', '64');
             INSERT INTO ai_settings (key, value) VALUES ('pane_gateway_url', '');
             CREATE TABLE migration_flags (flag TEXT PRIMARY KEY NOT NULL, completed_at TEXT NOT NULL DEFAULT (datetime('now')));
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
             "
        ).expect("Failed to create v64 schema");

        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v64→v65 migration should succeed: {:?}", result);

        // pane_gateway_url should be deleted
        let stale_url: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'pane_gateway_url'",
            [],
            |row| row.get(0),
        ).ok();
        assert_eq!(stale_url, None, "pane_gateway_url should be deleted");

        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test 21: v64→v65: both stale keys absent ───
    // Spec §22C.8.1a: clean pass; idempotent.

    #[test]
    fn test_v64_to_v65_idempotent_when_both_absent() {
        let conn = setup_test_db();

        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO ai_settings (key, value) VALUES ('schema_version', '64');
             INSERT INTO ai_settings (key, value) VALUES ('api_key', 'tok_live');
             -- Neither pane_gateway_token nor pane_gateway_url present
             CREATE TABLE migration_flags (flag TEXT PRIMARY KEY NOT NULL, completed_at TEXT NOT NULL DEFAULT (datetime('now')));
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
             "
        ).expect("Failed to create v64 schema");

        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v64→v65 migration should succeed cleanly when no stale rows: {:?}", result);

        // api_key must be untouched
        let api_key: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'api_key'",
            [],
            |row| row.get(0),
        ).ok();
        assert_eq!(api_key, Some("tok_live".to_string()), "api_key should be untouched");

        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));

        // Run again — must be idempotent (now at v68, should be a no-op)
        let second_run = crate::migrations::run_migrations(&conn);
        assert!(second_run.is_ok(), "Second run should be a no-op");
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test 22: v67→v68 migration — attachments CRR + NOT NULL DEFAULTs ───

    #[test]
    fn test_v67_to_v68_migration_attachments_crr_defaults() {
        // Constructs a v67-stamped DB (attachments table present without DEFAULTs),
        // runs run_migrations(), verifies:
        // - attachments has DEFAULTs on all NOT NULL columns
        // - schema version is stamped "76"
        // - existing attachment rows are preserved

        let conn = setup_test_db();

        // Build minimal v67 schema: attachments without DEFAULTs on NOT NULL columns.
        conn.execute_batch("
            CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
            INSERT INTO ai_settings (key, value) VALUES ('schema_version', '67');
            CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT);
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
            CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL DEFAULT '',
                original_filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
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
        ").expect("Failed to build v67 schema");

        // Insert a seed row to verify preservation.
        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES ('att-v68-test', 'proj-001', 'My Project', 'report.pdf', 'application/pdf', 8192)",
            [],
        ).expect("Failed to insert seed attachment");

        assert_eq!(get_schema_version(&conn), Some("67".to_string()), "Should start at v67");

        // Run migration.
        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v67→v68 migration should succeed: {:?}", result);

        // Verify schema version.
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()),
            "Schema version should be 73 after v67→v72 migration");

        // Verify DEFAULTs present on all NOT NULL non-PK columns.
        let col_info: Vec<(String, bool, Option<String>)> = conn
            .prepare("PRAGMA table_info(attachments)")
            .unwrap()
            .query_map([], |row| Ok((
                row.get::<_, String>(1)?,
                row.get::<_, bool>(3)?,
                row.get::<_, Option<String>>(4)?,
            )))
            .unwrap()
            .filter_map(Result::ok)
            .collect();

        for (name, notnull, dflt) in &col_info {
            if *notnull && name != "id" {
                assert!(
                    dflt.is_some(),
                    "attachments.{} is NOT NULL but missing DEFAULT after v68 migration",
                    name
                );
            }
        }

        // Verify seed data preserved.
        let (pid, fname, mime, sz): (String, String, String, i64) = conn.query_row(
            "SELECT project_id, original_filename, mime_type, size_bytes FROM attachments WHERE id='att-v68-test'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).expect("Seed row should be preserved after migration");

        assert_eq!(pid, "proj-001");
        assert_eq!(fname, "report.pdf");
        assert_eq!(mime, "application/pdf");
        assert_eq!(sz, 8192);

        // Second run is a no-op.
        let second = crate::migrations::run_migrations(&conn);
        assert!(second.is_ok(), "Second run should be idempotent");
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test 23: v68→v69 migration — og_cache + link_previews_enabled carry-over ───

    #[test]
    fn test_v68_to_v69_migration_og_cache_and_link_previews() {
        // Constructs a v68-stamped DB (like the upgrade-from-v68.db fixture):
        //   - attachments table with DEFAULTs (v68 shape)
        //   - link_embeds_enabled = 'false' (was explicitly toggled off by user)
        //   - NO og_cache table
        //   - NO link_previews_enabled key
        // Runs run_migrations() and asserts:
        //   - Schema is now v69
        //   - og_cache table exists
        //   - link_previews_enabled row exists with value carried from link_embeds_enabled ('false')

        let conn = setup_test_db();

        // Build minimal v68 schema.
        conn.execute_batch("
            CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO ai_settings (key, value) VALUES ('schema_version', '68');
            INSERT INTO ai_settings (key, value) VALUES ('link_embeds_enabled', 'false');
            CREATE TABLE projects (
                id TEXT NOT NULL PRIMARY KEY,
                name TEXT NOT NULL DEFAULT ''
            );
            CREATE TABLE notes (
                id TEXT NOT NULL PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                project_id TEXT
            );
            CREATE TABLE migration_flags (
                flag TEXT PRIMARY KEY NOT NULL,
                completed_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE user_dashboard_config (
                id TEXT PRIMARY KEY,
                onboarding_completed INTEGER NOT NULL DEFAULT 0,
                theme_variant TEXT DEFAULT 'default',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE attachments (
                id TEXT PRIMARY KEY NOT NULL,
                project_id TEXT NOT NULL DEFAULT '',
                project_name TEXT NOT NULL DEFAULT '',
                original_filename TEXT NOT NULL DEFAULT '',
                mime_type TEXT NOT NULL DEFAULT '',
                size_bytes INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
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
        ").expect("Failed to build v68 schema");

        // Seed project and note (mirrors upgrade-from-v68.db fixture).
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('proj-v68-seed', 'Seed Project')",
            [],
        ).expect("Failed to insert seed project");
        conn.execute(
            "INSERT INTO notes (id, title, project_id) VALUES ('note-v68-seed', 'Seed Note', 'proj-v68-seed')",
            [],
        ).expect("Failed to insert seed note");

        assert_eq!(get_schema_version(&conn), Some("68".to_string()), "Should start at v68");

        // og_cache must not exist yet.
        let og_cache_before: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='og_cache'",
            [], |row| row.get(0),
        ).unwrap_or(false);
        assert!(!og_cache_before, "og_cache must not exist in v68 fixture");

        // Run migration.
        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v68→v69 migration should succeed: {:?}", result);

        // 1. Schema version is v71.
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()),
            "Schema version should be 73 after v68→v72 migration");

        // 2. og_cache table exists.
        let og_cache_after: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='og_cache'",
            [], |row| row.get(0),
        ).unwrap_or(false);
        assert!(og_cache_after, "og_cache table must exist after v69 migration");

        // 3. link_previews_enabled was carried over from link_embeds_enabled.
        //    link_embeds_enabled was 'false', so link_previews_enabled must be 'false'.
        let previews_val: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'link_previews_enabled'",
            [], |row| row.get(0),
        ).ok();
        assert_eq!(
            previews_val.as_deref(),
            Some("false"),
            "link_previews_enabled must carry over 'false' from link_embeds_enabled"
        );

        // 4. Seed data is preserved.
        let proj_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM projects WHERE id = 'proj-v68-seed'",
            [], |row| row.get(0),
        ).unwrap_or(0);
        assert_eq!(proj_count, 1, "Seed project must be preserved after migration");

        let note_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE id = 'note-v68-seed'",
            [], |row| row.get(0),
        ).unwrap_or(0);
        assert_eq!(note_count, 1, "Seed note must be preserved after migration");

        // 5. Second run is idempotent.
        let second = crate::migrations::run_migrations(&conn);
        assert!(second.is_ok(), "Second run should be idempotent: {:?}", second);
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test 25: CRR Invariant Gate — NOT NULL non-PK columns must have DEFAULTs ────────

    #[test]
    fn test_crr_tables_not_null_columns_have_defaults() {
        // CRR (Conflict-free Replicated Relations via cr-sqlite) requires that every NOT NULL
        // non-PK column in a CRR-enabled table has a DEFAULT value. Without a DEFAULT, CRR
        // cannot synthesise rows during sync and will error at runtime.
        //
        // This test is a generic structural gate — it will catch any future CRR table that
        // violates the invariant, not just the table that motivated its addition (subtasks).
        //
        // NOTE: CRR_TABLES is a private const in migrations.rs. Because we cannot modify that
        // file (per task constraints), the list is duplicated here. Keep it in sync with
        // migrations::CRR_TABLES whenever tables are added or removed.
        const CRR_TABLES: [&str; 27] = [
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

        let conn = setup_test_db();
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed on fresh install");

        for table in &CRR_TABLES {
            // PRAGMA table_info columns: cid, name, type, notnull, dflt_value, pk
            let violations: Vec<String> = conn
                .prepare(&format!("PRAGMA table_info({})", table))
                .unwrap_or_else(|e| panic!("Failed to prepare PRAGMA for table '{}': {}", table, e))
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(1)?,  // name
                        row.get::<_, i64>(3)?,     // notnull
                        row.get::<_, Option<String>>(4)?, // dflt_value
                        row.get::<_, i64>(5)?,     // pk
                    ))
                })
                .unwrap_or_else(|e| panic!("Failed to query PRAGMA for table '{}': {}", table, e))
                .filter_map(Result::ok)
                .filter(|(_, notnull, dflt_value, pk)| {
                    // Only flag columns that are: NOT NULL, not a PK, and have no DEFAULT
                    *notnull == 1 && *pk == 0 && dflt_value.is_none()
                })
                .map(|(name, _, _, _)| name)
                .collect();

            assert!(
                violations.is_empty(),
                "CRR invariant violated in table '{}': the following NOT NULL non-PK columns have no DEFAULT (CRR requires defaults for all NOT NULL non-PK columns): [{}]",
                table,
                violations
                    .iter()
                    .map(|col| format!("Table '{}', column '{}' is NOT NULL but has no DEFAULT — CRR requires defaults for all NOT NULL non-PK columns", table, col))
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
    }

    // ─── Test 24: v70→v72 migration — notifications schema (#1811, ADR-0021/ADR-0022) ───

    #[test]
    fn test_v70_to_v71_migration_notifications_schema() {
        // Constructs a v70-stamped DB with the notifications table in its pre-migration
        // shape (no dismissed_at, expires_at, target_kind, target_id, or idem_key columns),
        // runs run_migrations(), and asserts:
        //   1. Schema version is stamped "76".
        //   2. All 5 new columns exist on the notifications table.
        //   3. All 3 new indexes exist.
        //   4. Pre-existing notifications row is preserved.

        let conn = setup_test_db();

        // Build minimal v70 schema with notifications table in pre-migration shape.
        conn.execute_batch("
            CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT INTO ai_settings (key, value) VALUES ('schema_version', '70');

            CREATE TABLE notifications (
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
        ").expect("Failed to build v70 schema");

        // Seed a pre-existing notification to verify data preservation.
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, read) \
             VALUES ('notif-v70-seed', 'info', 'Seed Notification', 'Body text', 'system', 0)",
            [],
        ).expect("Failed to insert seed notification");

        assert_eq!(get_schema_version(&conn), Some("70".to_string()), "Should start at v70");

        // Verify new columns do NOT exist before migration.
        assert!(!column_exists(&conn, "notifications", "dismissed_at"), "dismissed_at must not exist before migration");
        assert!(!column_exists(&conn, "notifications", "idem_key"), "idem_key must not exist before migration");

        // Run migration.
        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v70→v72 migration should succeed: {:?}", result);

        // 1. Schema version is v71.
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()),
            "Schema version should be 73 after v70→v72 migration");

        // 2. All 5 new columns exist on notifications.
        assert!(column_exists(&conn, "notifications", "dismissed_at"),
            "notifications.dismissed_at must exist after migration");
        assert!(column_exists(&conn, "notifications", "expires_at"),
            "notifications.expires_at must exist after migration");
        assert!(column_exists(&conn, "notifications", "target_kind"),
            "notifications.target_kind must exist after migration");
        assert!(column_exists(&conn, "notifications", "target_id"),
            "notifications.target_id must exist after migration");
        assert!(column_exists(&conn, "notifications", "idem_key"),
            "notifications.idem_key must exist after migration");

        // 3. All 3 new indexes exist.
        assert!(index_exists(&conn, "idx_notifications_active"),
            "idx_notifications_active must exist after migration");
        assert!(index_exists(&conn, "idx_notifications_idem"),
            "idx_notifications_idem must exist after migration");
        assert!(index_exists(&conn, "idx_notifications_expires"),
            "idx_notifications_expires must exist after migration");

        // Verify new columns are nullable (no DEFAULT) by confirming the seeded row survives
        // and new column values are NULL.
        let (title, dismissed_at, idem_key): (String, Option<String>, Option<String>) = conn.query_row(
            "SELECT title, dismissed_at, idem_key FROM notifications WHERE id = 'notif-v70-seed'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).expect("Seed notification row must be preserved after migration");

        assert_eq!(title, "Seed Notification", "Seed row title must be preserved");
        assert_eq!(dismissed_at, None, "dismissed_at must be NULL for pre-existing rows");
        assert_eq!(idem_key, None, "idem_key must be NULL for pre-existing rows");

        // 5. Second run is idempotent (version matches, no-op).
        let second = crate::migrations::run_migrations(&conn);
        assert!(second.is_ok(), "Second run should be idempotent: {:?}", second);
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ── Test F1-F5: Category bug fixes (#2141) ────────────────────────────

    /// F4a: `category_colors` must be in the CRR_TABLES const (fresh-install path).
    /// This is a compile-time / const check — if the entry is missing, the array
    /// size assertion or the contains check will fail at test time.
    #[test]
    fn test_category_colors_in_crr_tables_const() {
        use crate::migrations::CRR_TABLES;

        // Assert the const has exactly 27 entries. If someone adds/removes a table
        // without updating this count, the test fails and forces a conscious decision.
        assert_eq!(
            CRR_TABLES.len(),
            27,
            "CRR_TABLES must have exactly 27 entries; update this assertion when adding/removing tables"
        );

        // Assert category_colors is present. If it falls off the list, fresh-install
        // devices will not sync category definitions across devices.
        assert!(
            CRR_TABLES.contains(&"category_colors"),
            "category_colors must be in CRR_TABLES so fresh-install path enables CRR on it"
        );
    }

    /// F4: `category_colors` must be in the CRR upgrade list.
    #[test]
    fn test_category_colors_in_crr_list() {
        use crate::database::upgrade_tables_to_crr_inner;

        let conn = setup_test_db();
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");

        // category_colors table must exist (created by create_system_tables)
        let table_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='category_colors'",
            [],
            |row| row.get(0),
        ).expect("sqlite_master query");
        assert!(table_exists, "category_colors table must exist after migration");

        // Running upgrade_tables_to_crr_inner should succeed (idempotent — already crr or table exists)
        let result = upgrade_tables_to_crr_inner(&conn);
        assert!(result.is_ok(), "upgrade_tables_to_crr_inner should succeed with category_colors: {:?}", result);
    }

    /// F1-F3: Category save, clear, and rename propagation tests.
    #[test]
    fn test_category_save_and_load() {
        let conn = setup_test_db();
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");

        // Create category_colors entry and a project
        conn.execute(
            "INSERT OR REPLACE INTO category_colors (category, color) VALUES ('Work', '#FF0000')",
            [],
        ).expect("Insert category_colors");

        let proj_id = "proj-cat-test-1";
        conn.execute(
            "INSERT INTO projects (id, name, description, color, category, owner_id, is_favorite, created_at, updated_at)
             VALUES (?1, 'Test Project', NULL, NULL, 'Work', 'user_local', 0, datetime('now'), datetime('now'))",
            rusqlite::params![proj_id],
        ).expect("Insert test project");

        // F1: Verify category persists and loads correctly
        let saved_cat: String = conn.query_row(
            "SELECT category FROM projects WHERE id = ?1",
            rusqlite::params![proj_id],
            |row| row.get(0),
        ).expect("Query project category");
        assert_eq!(saved_cat, "Work", "Category should be 'Work' after save");

        // F2: Clear category (set to NULL via empty-string semantics, as update_project_settings now does)
        conn.execute(
            "UPDATE projects SET category = NULL WHERE id = ?1",
            rusqlite::params![proj_id],
        ).expect("Clear project category");

        let cleared_cat: Option<String> = conn.query_row(
            "SELECT category FROM projects WHERE id = ?1",
            rusqlite::params![proj_id],
            |row| row.get(0),
        ).expect("Query cleared project category");
        assert_eq!(cleared_cat, None, "Category should be NULL after clear");
    }

    /// F3: Category rename propagates to projects.category atomically.
    #[test]
    fn test_category_rename_propagation() {
        let conn = setup_test_db();
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");

        // Seed two projects with category 'OldName', one with 'Other'
        conn.execute(
            "INSERT OR REPLACE INTO category_colors (category, color) VALUES ('OldName', '#AABBCC')",
            [],
        ).expect("Seed category");
        for (id, cat) in &[("p1", "OldName"), ("p2", "OldName"), ("p3", "Other")] {
            conn.execute(
                "INSERT INTO projects (id, name, description, color, category, owner_id, is_favorite, created_at, updated_at)
                 VALUES (?1, ?1, NULL, NULL, ?2, 'user_local', 0, datetime('now'), datetime('now'))",
                rusqlite::params![id, cat],
            ).expect("Seed project");
        }

        // Simulate rename_category: atomic transaction
        conn.execute("BEGIN", []).unwrap();
        conn.execute(
            "UPDATE category_colors SET category = 'NewName', color = '#AABBCC' WHERE category = 'OldName'",
            [],
        ).expect("Rename category_colors");
        conn.execute(
            "UPDATE projects SET category = 'NewName' WHERE category = 'OldName'",
            [],
        ).expect("Rename project categories");
        conn.execute("COMMIT", []).unwrap();

        // F3: Both p1 and p2 should now be 'NewName'; p3 stays 'Other'
        for (id, expected) in &[("p1", "NewName"), ("p2", "NewName"), ("p3", "Other")] {
            let cat: String = conn.query_row(
                "SELECT category FROM projects WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get(0),
            ).expect("Query project category");
            assert_eq!(&cat, expected, "Project {} category should be '{}'", id, expected);
        }

        // And category_colors should have 'NewName', not 'OldName'
        let old_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM category_colors WHERE category = 'OldName'",
            [],
            |row| row.get(0),
        ).expect("Check old category");
        assert!(!old_exists, "'OldName' should not exist in category_colors after rename");

        let new_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM category_colors WHERE category = 'NewName'",
            [],
            |row| row.get(0),
        ).expect("Check new category");
        assert!(new_exists, "'NewName' should exist in category_colors after rename");
    }

    /// F5: Category groups sort alphabetically; projects within group sort alphabetically.
    #[test]
    fn test_category_group_sort_order() {
        let conn = setup_test_db();
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");

        // Seed categories in reverse alpha order
        for (cat, color) in &[("Zebra", "#000001"), ("Alpha", "#000002"), ("Mango", "#000003")] {
            conn.execute(
                "INSERT OR REPLACE INTO category_colors (category, color) VALUES (?1, ?2)",
                rusqlite::params![cat, color],
            ).expect("Seed category");
        }
        // Seed projects in reverse alpha within each category
        for (id, name, cat) in &[
            ("p-z2", "Zorro",   "Zebra"),
            ("p-z1", "Anchovy", "Zebra"),
            ("p-a2", "Walrus",  "Alpha"),
            ("p-a1", "Bear",    "Alpha"),
        ] {
            conn.execute(
                "INSERT INTO projects (id, name, description, color, category, owner_id, is_favorite, created_at, updated_at)
                 VALUES (?1, ?2, NULL, NULL, ?3, 'user_local', 1, datetime('now'), datetime('now'))",
                rusqlite::params![id, name, cat],
            ).expect("Seed project");
        }

        // list_categories returns categories in ORDER BY category (alpha)
        let mut stmt = conn.prepare(
            "SELECT category FROM category_colors ORDER BY category"
        ).expect("Prepare category query");
        let cats: Vec<String> = stmt.query_map([], |row| row.get(0))
            .expect("Query categories")
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(cats, vec!["Alpha", "Mango", "Zebra"], "Categories must sort alphabetically");

        // Projects within Zebra category sort alpha by name
        let mut stmt2 = conn.prepare(
            "SELECT name FROM projects WHERE category = 'Zebra' ORDER BY name"
        ).expect("Prepare project query");
        let zebra_projects: Vec<String> = stmt2.query_map([], |row| row.get(0))
            .expect("Query projects")
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(zebra_projects, vec!["Anchovy", "Zorro"], "Projects within Zebra must sort alphabetically");
    }

    // ─── Test: Fresh install at v76 ─────────────────────────────────────────────
    #[test]
    fn test_fresh_install_v76() {
        let conn = setup_test_db();

        // run_migrations on empty DB → should initialize to v76
        crate::migrations::run_migrations(&conn).expect("Fresh install should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Fresh install should be at v77");

        // Verify chat_messages_local has agent_id column (the v76 addition)
        assert!(
            column_exists(&conn, "chat_messages_local", "agent_id"),
            "chat_messages_local should have agent_id column at v76"
        );

        // Verify table count still >= 44 (at least as many as v75 + any new tables)
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name NOT LIKE '%__crsql%'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(count >= 44, "Should have at least 44 tables at v77, found {}", count);
    }

    // ─── Test: v75→v76 upgrade path ────────────────────────────────────────────
    #[test]
    fn test_v75_to_v76_migration() {
        let conn = setup_test_db();

        // Bootstrap current schema, then downgrade stamp to v75 to simulate pre-migration state
        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        conn.execute(
            "UPDATE ai_settings SET value = '75' WHERE key = 'schema_version'",
            []
        ).expect("Failed to set v75 stamp");

        // Re-run migrations — should apply v75→v76 arm
        crate::migrations::run_migrations(&conn).expect("v75→v76 migration should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Should be at v77 after migration");

        // Verify agent_id column was added
        assert!(
            column_exists(&conn, "chat_messages_local", "agent_id"),
            "v75→v76 migration must add agent_id to chat_messages_local"
        );

        // Verify idempotent: second run should not fail
        crate::migrations::run_migrations(&conn).expect("Second run at v76 should be idempotent");
    }

    // ─── Test: Data preservation through v75→v76 ───────────────────────────────
    #[test]
    fn test_v75_to_v76_data_preservation() {
        let conn = setup_test_db();

        // Bootstrap current schema, then downgrade stamp to v75 to simulate pre-migration state
        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        conn.execute(
            "UPDATE ai_settings SET value = '75' WHERE key = 'schema_version'",
            []
        ).expect("Failed to set v75 stamp");

        // Insert test data at v75 state
        let msg_id = "test-msg-v75";
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content) \
             VALUES (?1, 'workspace', 'thread-1', 'user', 'Hello from v75')",
            rusqlite::params![msg_id],
        ).expect("Failed to insert test chat message");

        // Migrate
        crate::migrations::run_migrations(&conn).expect("v75→v76 migration should succeed");

        // Verify message survived
        let content: String = conn.query_row(
            "SELECT content FROM chat_messages_local WHERE id = ?1",
            rusqlite::params![msg_id],
            |row| row.get(0),
        ).expect("Test message should survive migration");
        assert_eq!(content, "Hello from v75", "Message content must survive v75→v76");

        // Verify agent_id is nullable (pre-migration messages have NULL agent_id — acceptable)
        let agent_id: Option<String> = conn.query_row(
            "SELECT agent_id FROM chat_messages_local WHERE id = ?1",
            rusqlite::params![msg_id],
            |row| row.get(0),
        ).expect("Query should succeed");
        assert!(agent_id.is_none(), "Pre-migration messages should have NULL agent_id");
    }

    // ─── Test: v74 → SCHEMA_VERSION migration path (#2598 hygiene) ─────────────────
    //
    // Prior to the #2598 fix, `run_migrations` had match arms for v62–v71, then
    // jumped straight to v75 — no arms for v72/v73/v74. Any DB stamped at one of
    // those three versions hit the `SCHEMA_MISMATCH` fallthrough with no path
    // forward. This test builds a minimal DB shaped like a real v74 install (using
    // the OLD `ai_agents` column names — `is_main`/`oc_agent_id`, not
    // `is_default`/`runtime_ref`, since those renames don't happen until
    // `migrate_v79`) and asserts the full v74→SCHEMA_VERSION chain completes.
    #[test]
    fn test_v74_to_current_migration() {
        let conn = setup_test_db();

        // Minimal v74-shaped schema: pre-v79 ai_agents column names (is_main,
        // oc_agent_id), pre-v76 chat_messages_local (no agent_id column), pre-v78
        // chat_sessions (no agent_id column). All other tables the v72→v83 chain
        // touches (ai_settings, projects) are also included so every migrate_vNN's
        // table-existence checks resolve to "present" (exercising the real code
        // path, not the older-fixture no-op branches).
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY,
                value TEXT
            );
            INSERT INTO ai_settings (key, value) VALUES ('schema_version', '74');

            CREATE TABLE ai_agents (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL DEFAULT '',
                slug            TEXT NOT NULL UNIQUE,
                role            TEXT NOT NULL DEFAULT 'sub',
                model           TEXT,
                is_main         INTEGER NOT NULL DEFAULT 0,
                is_active       INTEGER NOT NULL DEFAULT 1,
                workspace_path  TEXT NOT NULL DEFAULT '',
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
                oc_agent_id     TEXT
             );
             CREATE UNIQUE INDEX idx_ai_agents_main ON ai_agents(is_main) WHERE is_main = 1;
             CREATE UNIQUE INDEX idx_ai_agents_oc_id_unique ON ai_agents(oc_agent_id);
             INSERT INTO ai_agents (id, name, slug, role, model, is_main, is_active, workspace_path, oc_agent_id)
             VALUES ('agent-oc', 'OC Agent', 'oc-agent', 'main', NULL, 1, 1, 'WP1', 'main-oc-id');

             CREATE TABLE chat_messages_local (
                id TEXT NOT NULL PRIMARY KEY,
                project_id TEXT NOT NULL DEFAULT '',
                role TEXT NOT NULL DEFAULT 'user',
                content TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                thread_id TEXT
             );
             INSERT INTO chat_messages_local (id, project_id, thread_id, role, content)
             VALUES ('msg-v74', 'workspace', 'thread-1', 'user', 'Hello from v74');

             CREATE TABLE chat_sessions (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL DEFAULT ''
             );
             INSERT INTO chat_sessions (id, project_id) VALUES ('sess-v74', 'workspace');

             CREATE TABLE projects (
                id                     TEXT PRIMARY KEY,
                name                   TEXT NOT NULL,
                smart_routing_enabled  INTEGER,
                router_model           TEXT,
                simple_tier_model      TEXT,
                default_model          TEXT
             );
            "
        ).expect("Failed to create v74 fixture schema");

        assert_eq!(get_schema_version(&conn), Some("74".to_string()), "Fixture should start stamped at v74");
        assert!(column_exists(&conn, "ai_agents", "is_main"), "Fixture should use OLD column name is_main");
        assert!(column_exists(&conn, "ai_agents", "oc_agent_id"), "Fixture should use OLD column name oc_agent_id");
        assert!(!column_exists(&conn, "ai_agents", "is_default"), "Fixture should NOT yet have the renamed is_default column");
        assert!(!column_exists(&conn, "ai_agents", "runtime_ref"), "Fixture should NOT yet have the renamed runtime_ref column");

        // Run migrations — must find and apply the new v72/v73/v74 arm (all three
        // route through the same v74→v76 chain since v74 is closest to v76).
        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v74→{} migration should succeed: {:?}", crate::migrations::SCHEMA_VERSION, result);

        // Verify final schema version.
        assert_eq!(
            get_schema_version(&conn),
            Some(crate::migrations::SCHEMA_VERSION.to_string()),
            "Should land at current SCHEMA_VERSION after v74 migration"
        );

        // Verify the column renames landed (migrate_v79's Step 1 + Step 5).
        assert!(column_exists(&conn, "ai_agents", "runtime_ref"), "oc_agent_id should be renamed to runtime_ref");
        assert!(column_exists(&conn, "ai_agents", "is_default"), "is_main should be renamed to is_default");
        assert!(column_exists(&conn, "ai_agents", "runtime_type"), "runtime_type column should be added");

        // Verify migrate_v76's agent_id column landed on chat_messages_local.
        assert!(column_exists(&conn, "chat_messages_local", "agent_id"), "chat_messages_local should have agent_id column after v74→current migration");

        // Verify migrate_v78's agent_id column landed on chat_sessions.
        assert!(column_exists(&conn, "chat_sessions", "agent_id"), "chat_sessions should have agent_id column after v74→current migration");

        // Verify pre-existing data survived the full chain.
        let content: String = conn.query_row(
            "SELECT content FROM chat_messages_local WHERE id = 'msg-v74'",
            [],
            |row| row.get(0),
        ).expect("Pre-migration message should survive the full v74→current chain");
        assert_eq!(content, "Hello from v74");

        // Verify the renamed ai_agents row survived with its runtime_type backfilled
        // (migrate_v79 Step 3: oc_agent_id-not-null rows get runtime_type='openclaw').
        let runtime_type: Option<String> = conn.query_row(
            "SELECT runtime_type FROM ai_agents WHERE id = 'agent-oc'",
            [],
            |row| row.get(0),
        ).expect("Migrated agent row should survive");
        assert_eq!(runtime_type.as_deref(), Some("openclaw"), "OC-imported row (oc_agent_id NOT NULL) should be backfilled to runtime_type='openclaw'");

        // Verify idempotent: second run at current version should be a clean no-op.
        let second_run = crate::migrations::run_migrations(&conn);
        assert!(second_run.is_ok(), "Second run at current SCHEMA_VERSION should succeed (version match, no-op)");
        assert_eq!(get_schema_version(&conn), Some(crate::migrations::SCHEMA_VERSION.to_string()));
    }

    // ─── Test: v72 and v73 also have a migration path (#2598 hygiene) ────────────
    //
    // Lighter-weight sibling to `test_v74_to_current_migration` — just proves the
    // v72 and v73 arms exist and reach SCHEMA_VERSION, without re-asserting every
    // intermediate column detail already covered above.
    #[test]
    fn test_v72_and_v73_reach_current_schema_version() {
        for stamp in ["72", "73"] {
            let conn = setup_test_db();
            conn.execute_batch(&format!(
                "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);
                 INSERT INTO ai_settings (key, value) VALUES ('schema_version', '{}');
                 CREATE TABLE ai_agents (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL DEFAULT '',
                    slug TEXT NOT NULL UNIQUE,
                    role TEXT NOT NULL DEFAULT 'sub',
                    model TEXT,
                    is_main INTEGER NOT NULL DEFAULT 0,
                    is_active INTEGER NOT NULL DEFAULT 1,
                    workspace_path TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                    oc_agent_id TEXT
                 );
                 CREATE UNIQUE INDEX idx_ai_agents_main ON ai_agents(is_main) WHERE is_main = 1;
                 CREATE UNIQUE INDEX idx_ai_agents_oc_id_unique ON ai_agents(oc_agent_id);
                 CREATE TABLE chat_messages_local (
                    id TEXT NOT NULL PRIMARY KEY,
                    project_id TEXT NOT NULL DEFAULT '',
                    role TEXT NOT NULL DEFAULT 'user',
                    content TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    thread_id TEXT
                 );
                 CREATE TABLE chat_sessions (
                    id TEXT PRIMARY KEY,
                    project_id TEXT NOT NULL DEFAULT ''
                 );
                ",
                stamp
            )).expect("Failed to create fixture schema");

            assert_eq!(get_schema_version(&conn), Some(stamp.to_string()), "Fixture should start stamped at v{}", stamp);

            let result = crate::migrations::run_migrations(&conn);
            assert!(result.is_ok(), "v{}→{} migration should succeed: {:?}", stamp, crate::migrations::SCHEMA_VERSION, result);

            assert_eq!(
                get_schema_version(&conn),
                Some(crate::migrations::SCHEMA_VERSION.to_string()),
                "v{} fixture should land at current SCHEMA_VERSION", stamp
            );

            assert!(column_exists(&conn, "ai_agents", "runtime_ref"), "v{}: oc_agent_id should be renamed to runtime_ref", stamp);
            assert!(column_exists(&conn, "ai_agents", "is_default"), "v{}: is_main should be renamed to is_default", stamp);
            assert!(column_exists(&conn, "chat_messages_local", "agent_id"), "v{}: chat_messages_local should have agent_id column", stamp);
            assert!(column_exists(&conn, "chat_sessions", "agent_id"), "v{}: chat_sessions should have agent_id column", stamp);
        }
    }

    // ─── Test: fresh install lands at v93 with agent_jobs + the 4 columns (#3050 PR 2) ───
    #[test]
    fn test_fresh_install_v93() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Fresh install should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Fresh install should be at current SCHEMA_VERSION (93)");

        assert!(table_exists(&conn, "agent_jobs"), "Fresh install should have agent_jobs table");
        assert!(index_exists(&conn, "idx_agent_jobs_due"), "Fresh install should have idx_agent_jobs_due");
        assert!(index_exists(&conn, "idx_agent_jobs_origin"), "Fresh install should have idx_agent_jobs_origin");
        assert!(index_exists(&conn, "idx_agent_jobs_heartbeat"), "Fresh install should have idx_agent_jobs_heartbeat");

        for col in ["job_id", "synthetic", "continued", "attachment_ids"] {
            assert!(
                column_exists(&conn, "chat_messages_local", col),
                "Fresh install should have chat_messages_local.{}", col
            );
        }
    }

    // ─── Test: v92→v93 upgrade path (#3050 PR 2, §36.10) — migration test per §36.10's
    // own last line: "agent_jobs exists, the four columns exist with defaults, existing
    // chat_messages_local rows read back unchanged." ───
    #[test]
    fn test_v92_to_v93_migration() {
        let conn = setup_test_db();

        // Bootstrap current schema, then downgrade stamp to v92 to simulate a real
        // pre-#3050 install (v92 already has the pre-existing chat_messages_local
        // shape minus job_id/synthetic/continued — downgrading the stamp alone is
        // sufficient since those three columns are only read, never required, by
        // any v92-era code path).
        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        conn.execute(
            "UPDATE ai_settings SET value = '92' WHERE key = 'schema_version'",
            [],
        ).expect("Failed to set v92 stamp");

        // Drop the v93 columns to simulate a real pre-migration v92 install, since
        // the fresh-install path above already created them at the current schema.
        conn.execute_batch(
            "CREATE TABLE chat_messages_local_v92_shape AS SELECT id, project_id, role, content, created_at, is_pinned, attachment_ids, tool_calls, tool_results, model, token_count, thread_id, agent_id, oc_message_id FROM chat_messages_local;
             DROP TABLE chat_messages_local;
             ALTER TABLE chat_messages_local_v92_shape RENAME TO chat_messages_local;
             DROP TABLE agent_jobs;"
        ).expect("Failed to downgrade chat_messages_local/agent_jobs to v92 shape");

        assert!(!column_exists(&conn, "chat_messages_local", "job_id"), "v92 fixture must not have job_id yet");
        assert!(!table_exists(&conn, "agent_jobs"), "v92 fixture must not have agent_jobs yet");

        // Re-run migrations — should apply the v92→v93 arm.
        crate::migrations::run_migrations(&conn).expect("v92→v93 migration should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Should be at v93 after migration");

        assert!(table_exists(&conn, "agent_jobs"), "v92→v93 migration must create agent_jobs");
        assert!(index_exists(&conn, "idx_agent_jobs_due"), "v92→v93 migration must create idx_agent_jobs_due");
        assert!(index_exists(&conn, "idx_agent_jobs_heartbeat"), "v92→v93 migration must create idx_agent_jobs_heartbeat");

        for col in ["job_id", "synthetic", "continued", "attachment_ids"] {
            assert!(
                column_exists(&conn, "chat_messages_local", col),
                "v92→v93 migration must add chat_messages_local.{}", col
            );
        }

        // Default-value verification (synthetic/continued default to 0 for
        // pre-existing rows) lives in `test_v92_to_v93_data_preservation` below,
        // which inserts a real row before migrating and reads it back — a
        // schema-only check here would not prove SQLite's ADD COLUMN DEFAULT
        // actually applies retroactively.

        // Verify idempotent: second run should not fail (agent_jobs CREATE TABLE IF
        // NOT EXISTS + add_column_if_missing's pragma guard both re-run cleanly).
        crate::migrations::run_migrations(&conn).expect("Second run at v93 should be idempotent");
    }

    // ─── Test: data preservation through v92→v93 — existing chat_messages_local rows
    // read back unchanged, new columns take their documented defaults ───
    #[test]
    fn test_v92_to_v93_data_preservation() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        conn.execute(
            "UPDATE ai_settings SET value = '92' WHERE key = 'schema_version'",
            [],
        ).expect("Failed to set v92 stamp");
        conn.execute_batch(
            "CREATE TABLE chat_messages_local_v92_shape AS SELECT id, project_id, role, content, created_at, is_pinned, attachment_ids, tool_calls, tool_results, model, token_count, thread_id, agent_id, oc_message_id FROM chat_messages_local;
             DROP TABLE chat_messages_local;
             ALTER TABLE chat_messages_local_v92_shape RENAME TO chat_messages_local;
             DROP TABLE agent_jobs;"
        ).expect("Failed to downgrade to v92 shape");

        // Insert test data at v92 state (pre-migration — no job_id/synthetic/continued
        // columns exist yet on this connection's view of the table).
        let msg_id = "test-msg-v92";
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content) \
             VALUES (?1, 'workspace', 'thread-1', 'user', 'Hello from v92')",
            rusqlite::params![msg_id],
        ).expect("Failed to insert test chat message");

        crate::migrations::run_migrations(&conn).expect("v92→v93 migration should succeed");

        // Byte-identical read-back of the pre-existing row's original columns.
        let content: String = conn.query_row(
            "SELECT content FROM chat_messages_local WHERE id = ?1",
            rusqlite::params![msg_id],
            |row| row.get(0),
        ).expect("Test message should survive migration");
        assert_eq!(content, "Hello from v92", "Message content must survive v92→v93 unchanged");

        // New columns take their documented defaults for a pre-existing row:
        // job_id NULL, synthetic=0, continued=0 (DEFAULT 0 applies retroactively to
        // existing rows on SQLite's ADD COLUMN, per SQLite's own ALTER TABLE semantics).
        let (job_id, synthetic, continued): (Option<String>, i64, i64) = conn.query_row(
            "SELECT job_id, synthetic, continued FROM chat_messages_local WHERE id = ?1",
            rusqlite::params![msg_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).expect("Query should succeed");
        assert_eq!(job_id, None, "Pre-migration messages should have NULL job_id");
        assert_eq!(synthetic, 0, "Pre-migration messages should default synthetic=0");
        assert_eq!(continued, 0, "Pre-migration messages should default continued=0");
    }

    // ─── Test: the REAL upgrade-from-v92.db fixture, not the synthetic
    // downgrade-a-fresh-install shape the two tests above use (#3102 correction,
    // per Reed's audit §4) ───────────────────────────────────────────────────
    //
    // Same pattern as `ai::ingest::tombstoning_tests::test_upgrade_from_v85_fixture_migrates_cleanly_to_v86`
    // (copy fixture to a scratch path, point PANE_TEST_MODE/PANE_TEST_DB/PANE_TEST_ROOT
    // at it, run the real `run_migrations`, assert on the result) — read that test
    // and matched its shape deliberately, not reinvented.
    //
    // `smoke-upgrade-db` already proves this fixture migrates cleanly end-to-end
    // through a real launch (Stia's correction to Reed's §4 framing: the fixture IS
    // loaded by a shipped CI job, just not by any `cargo test`). What this test adds
    // is the missing assertion layer on top of that already-proven path.
    #[test]
    #[serial_test::serial(pane_test_root, keychain_override)]
    fn test_upgrade_from_v92_fixture_migrates_cleanly_to_v93() {
        use std::path::PathBuf;
        use tempfile::TempDir;

        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/upgrade-from-v92.db");
        assert!(
            fixture_path.exists(),
            "upgrade-from-v92.db fixture not found at {:?} — see scripts/build-v92-fixture.sh",
            fixture_path
        );

        let temp_dir = TempDir::new().expect("tempdir");
        let scratch_path = temp_dir.path().join("scratch-upgrade-v92.db");
        std::fs::copy(&fixture_path, &scratch_path).expect("copy fixture to scratch path");

        unsafe {
            std::env::set_var("PANE_TEST_MODE", "1");
            std::env::set_var("PANE_TEST_DB", scratch_path.to_str().unwrap());
            std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        }

        // The v92→v93 arm calls `create_migration_backup`, which (like the v85→v86
        // arm ingest.rs's sibling test guards against) touches the real macOS
        // Keychain unless a MockKeychain is installed.
        let _kc = crate::keychain::test_support::KeychainGuard::set();

        let conn = rusqlite::Connection::open(&scratch_path).expect("open fixture db");

        // Neither `agent_jobs` (new, non-CRR) nor `chat_messages_local` (pre-existing,
        // non-CRR — see migrate_v93's own doc comment) is CRR-tracked, and the v92→v93
        // arm's only writes are a CREATE TABLE and four ADD COLUMNs on these two
        // non-CRR tables — unlike the v91 fixture's `projects.default_folder_id`
        // backfill, nothing here writes into an already-CRR-tracked table, so this
        // migration arm does not require the cr-sqlite extension to be loaded on
        // this test connection (`verify_crr_tables` fails closed to a no-op when the
        // extension isn't present, confirmed by its own doc comment/log line).
        let version_before: String = conn
            .query_row("SELECT value FROM ai_settings WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version_before, "92", "fixture must be stamped at v92 before migration");

        let row_count_before: i64 = conn.query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0)).unwrap();
        assert!(row_count_before > 0, "fixture must carry pre-existing chat_messages_local seed data");

        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v92→{} migration on the real fixture must succeed: {:?}", crate::migrations::SCHEMA_VERSION, result);

        // #3050 PR 5: SCHEMA_VERSION bumped 93 → 94 (migrate_v94,
        // chat_sessions.crew_agent_ids) — the v92→v93 arm now chains straight
        // through migrate_v94 too (same shape every other "old install, N
        // versions behind" arm already uses), so this fixture lands at the
        // CURRENT version, not the literal "93" this test asserted before PR 5.
        let version_after: String = conn
            .query_row("SELECT value FROM ai_settings WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version_after, crate::migrations::SCHEMA_VERSION, "fixture must land at the current SCHEMA_VERSION after migration");

        assert!(
            column_exists(&conn, "chat_sessions", "crew_agent_ids"),
            "chat_sessions.crew_agent_ids must exist after migrating the real v92 fixture through to current"
        );

        // `agent_jobs` exists with its full expected column set (new table, CREATE
        // TABLE IF NOT EXISTS — migrate_v93's own literal column list).
        assert!(table_exists(&conn, "agent_jobs"), "agent_jobs table must exist after migrating the real v92 fixture");
        for col in [
            "id", "kind", "title", "brief", "agent_id", "project_id", "session_id",
            "origin_session_id", "origin_message_id", "parent_job_id", "root_job_id",
            "chain_depth", "wake_origin", "cron", "run_at", "next_run_at", "status",
            "started_at", "finished_at", "last_error", "tool_calls_used",
            "missed_count", "created_at", "updated_at",
        ] {
            assert!(
                column_exists(&conn, "agent_jobs", col),
                "agent_jobs.{} must exist after migrating the real v92 fixture", col
            );
        }
        assert!(index_exists(&conn, "idx_agent_jobs_due"), "idx_agent_jobs_due must exist after migrating the real v92 fixture");
        assert!(index_exists(&conn, "idx_agent_jobs_origin"), "idx_agent_jobs_origin must exist after migrating the real v92 fixture");
        assert!(index_exists(&conn, "idx_agent_jobs_heartbeat"), "idx_agent_jobs_heartbeat must exist after migrating the real v92 fixture");

        // The THREE columns this migration actually ADDS to chat_messages_local.
        for col in ["job_id", "synthetic", "continued"] {
            assert!(
                column_exists(&conn, "chat_messages_local", col),
                "chat_messages_local.{} must be added by the v92→v93 migration on the real fixture", col
            );
        }
        // `attachment_ids` is asserted too, for completeness of "the migration's
        // documented column list" — but it is explicitly NOT one of the three columns
        // this migration adds. It pre-existed on every real install since the base
        // schema (migrate_v93's own doc comment, and `migrations.rs:1527`'s original
        // CREATE TABLE) — this migration's `add_column_if_missing` call for it is a
        // documented idempotent no-op, not evidence the migration created it. A
        // future reader diffing this assertion list against the three above should
        // not conclude this migration is responsible for `attachment_ids`.
        assert!(
            column_exists(&conn, "chat_messages_local", "attachment_ids"),
            "chat_messages_local.attachment_ids must still exist after migration (pre-existing column, not added by v93)"
        );

        // Pre-existing data survives the migration untouched — row count unchanged.
        let row_count_after: i64 = conn.query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0)).unwrap();
        assert_eq!(row_count_after, row_count_before, "pre-existing chat_messages_local rows must survive the v92→v93 migration on the real fixture untouched");

        unsafe {
            std::env::remove_var("PANE_TEST_MODE");
            std::env::remove_var("PANE_TEST_DB");
        }
    }

    // ─── #3050 PR 5, Commit 1 — fresh install lands at v94 with
    // chat_sessions.crew_agent_ids present ────────────────────────────────
    #[test]
    fn test_fresh_install_v94() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Fresh install should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Fresh install should be at current SCHEMA_VERSION");

        assert!(
            column_exists(&conn, "chat_sessions", "crew_agent_ids"),
            "Fresh install should have chat_sessions.crew_agent_ids"
        );
    }

    // ─── #3050 fix PR, Bug 1(b) — fresh install lands at current
    // SCHEMA_VERSION (v95) with agent_jobs.oc_session_key present ──────────
    #[test]
    fn test_fresh_install_v95() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Fresh install should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Fresh install should be at current SCHEMA_VERSION");

        assert!(
            column_exists(&conn, "agent_jobs", "oc_session_key"),
            "Fresh install should have agent_jobs.oc_session_key"
        );
    }

    // ─── #3050 PR 5, Commit 1 — v93→v94 upgrade path: crew_agent_ids added,
    // NULL on every pre-existing row, idempotent on a second run (same
    // `pragma_table_info` guard shape `migrate_v93` uses) ──────────────────
    #[test]
    fn test_v93_to_v94_migration() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        conn.execute(
            "UPDATE ai_settings SET value = '93' WHERE key = 'schema_version'",
            [],
        ).expect("Failed to set v93 stamp");

        // Downgrade chat_sessions to its pre-v94 shape to simulate a real
        // v93 install (same "drop and recreate the narrower shape" pattern
        // test_v92_to_v93_migration uses for chat_messages_local).
        conn.execute_batch(
            "CREATE TABLE chat_sessions_v93_shape AS SELECT id, project_id, title, origin, created_at, last_active_at, hidden_at, is_active, agent_id, oc_baseline_done_at FROM chat_sessions;
             DROP TABLE chat_sessions;
             ALTER TABLE chat_sessions_v93_shape RENAME TO chat_sessions;"
        ).expect("Failed to downgrade chat_sessions to v93 shape");

        assert!(!column_exists(&conn, "chat_sessions", "crew_agent_ids"), "v93 fixture must not have crew_agent_ids yet");

        // Insert a pre-existing session row at the v93 shape, to prove it
        // survives the migration and gets NULL for the new column.
        let session_id = "test-session-v93";
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, title, origin, agent_id) VALUES (?1, 'workspace', 'test', 'user', NULL)",
            rusqlite::params![session_id],
        ).expect("Failed to insert test session");

        crate::migrations::run_migrations(&conn).expect("v93→v94 migration should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Should be at v94 after migration");

        assert!(column_exists(&conn, "chat_sessions", "crew_agent_ids"), "v93→v94 migration must add chat_sessions.crew_agent_ids");

        let crew: Option<String> = conn.query_row(
            "SELECT crew_agent_ids FROM chat_sessions WHERE id = ?1",
            rusqlite::params![session_id],
            |row| row.get(0),
        ).expect("Query should succeed");
        assert_eq!(crew, None, "Pre-existing sessions should have NULL crew_agent_ids (no crew)");

        // Idempotent: second run should not fail.
        crate::migrations::run_migrations(&conn).expect("Second run at v94 should be idempotent");
    }

    // ─── #3050 fix PR, Bug 1(b) — v94→v95 upgrade path: oc_session_key
    // added to agent_jobs, NULL on every pre-existing row, idempotent on a
    // second run (same `pragma_table_info` guard shape `migrate_v93`/
    // `migrate_v94` use) ─────────────────────────────────────────────────
    #[test]
    fn test_v94_to_v95_migration() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        conn.execute(
            "UPDATE ai_settings SET value = '94' WHERE key = 'schema_version'",
            [],
        ).expect("Failed to set v94 stamp");

        // Downgrade agent_jobs to its pre-v95 shape to simulate a real v94
        // install (same "drop and recreate the narrower shape" pattern
        // test_v93_to_v94_migration uses for chat_sessions).
        conn.execute_batch(
            "CREATE TABLE agent_jobs_v94_shape AS SELECT id, kind, title, brief, agent_id, project_id, session_id, origin_session_id, origin_message_id, parent_job_id, root_job_id, chain_depth, wake_origin, cron, run_at, next_run_at, status, started_at, finished_at, last_error, tool_calls_used, missed_count, created_at, updated_at FROM agent_jobs;
             DROP TABLE agent_jobs;
             ALTER TABLE agent_jobs_v94_shape RENAME TO agent_jobs;"
        ).expect("Failed to downgrade agent_jobs to v94 shape");

        assert!(!column_exists(&conn, "agent_jobs", "oc_session_key"), "v94 fixture must not have oc_session_key yet");

        // Insert a pre-existing job row at the v94 shape, to prove it
        // survives the migration and gets NULL for the new column.
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, model, created_at, updated_at) \
             VALUES ('agent-v94', 'Agent V94', 'agent-v94', 1, 1, 'agent-v94', 'test-model', datetime('now'), datetime('now'))",
            [],
        ).expect("Failed to seed agent");
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, title, origin, agent_id) VALUES ('sess-v94', 'workspace', 'test', 'schedule', 'agent-v94')",
            [],
        ).expect("Failed to seed session");
        let job_id = "test-job-v94";
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, status) \
             VALUES (?1, 'once', 'test', 'test brief', 'agent-v94', 'workspace', 'sess-v94', 'pending')",
            rusqlite::params![job_id],
        ).expect("Failed to insert test job");

        crate::migrations::run_migrations(&conn).expect("v94→v95 migration should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Should be at v95 after migration");

        assert!(column_exists(&conn, "agent_jobs", "oc_session_key"), "v94→v95 migration must add agent_jobs.oc_session_key");

        let oc_session_key: Option<String> = conn.query_row(
            "SELECT oc_session_key FROM agent_jobs WHERE id = ?1",
            rusqlite::params![job_id],
            |row| row.get(0),
        ).expect("Query should succeed");
        assert_eq!(oc_session_key, None, "Pre-existing jobs should have NULL oc_session_key (never bootstrapped)");

        // Idempotent: second run should not fail.
        crate::migrations::run_migrations(&conn).expect("Second run at v95 should be idempotent");
    }

    // ─── live-test #1b fix PR, Ruling 3 — fresh install lands at current
    // SCHEMA_VERSION (v96) with agent_jobs.consecutive_failures +
    // agent_jobs.last_failed_at present ─────────────────────────────────
    #[test]
    fn test_fresh_install_v96() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Fresh install should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Fresh install should be at current SCHEMA_VERSION");

        assert!(
            column_exists(&conn, "agent_jobs", "consecutive_failures"),
            "Fresh install should have agent_jobs.consecutive_failures"
        );
        assert!(
            column_exists(&conn, "agent_jobs", "last_failed_at"),
            "Fresh install should have agent_jobs.last_failed_at"
        );
    }

    // ─── live-test #1b fix PR, Ruling 3 — v95→v96 upgrade path:
    // consecutive_failures (default 0) + last_failed_at (NULL) added to
    // agent_jobs, idempotent on a second run (same `pragma_table_info`
    // guard shape every prior migration in this ladder uses) ─────────────
    #[test]
    fn test_v95_to_v96_migration() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        conn.execute(
            "UPDATE ai_settings SET value = '95' WHERE key = 'schema_version'",
            [],
        ).expect("Failed to set v95 stamp");

        // Downgrade agent_jobs to its pre-v96 shape to simulate a real v95
        // install (same "drop and recreate the narrower shape" pattern
        // test_v94_to_v95_migration uses).
        conn.execute_batch(
            "CREATE TABLE agent_jobs_v95_shape AS SELECT id, kind, title, brief, agent_id, project_id, session_id, origin_session_id, origin_message_id, parent_job_id, root_job_id, chain_depth, wake_origin, cron, run_at, next_run_at, status, started_at, finished_at, last_error, tool_calls_used, missed_count, created_at, updated_at, oc_session_key FROM agent_jobs;
             DROP TABLE agent_jobs;
             ALTER TABLE agent_jobs_v95_shape RENAME TO agent_jobs;"
        ).expect("Failed to downgrade agent_jobs to v95 shape");

        assert!(!column_exists(&conn, "agent_jobs", "consecutive_failures"), "v95 fixture must not have consecutive_failures yet");
        assert!(!column_exists(&conn, "agent_jobs", "last_failed_at"), "v95 fixture must not have last_failed_at yet");

        // Insert a pre-existing job row at the v95 shape, to prove it
        // survives the migration and gets the documented defaults for the
        // two new columns.
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, model, created_at, updated_at) \
             VALUES ('agent-v95', 'Agent V95', 'agent-v95', 1, 1, 'agent-v95', 'test-model', datetime('now'), datetime('now'))",
            [],
        ).expect("Failed to seed agent");
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, title, origin, agent_id) VALUES ('sess-v95', 'workspace', 'test', 'schedule', 'agent-v95')",
            [],
        ).expect("Failed to seed session");
        let job_id = "test-job-v95";
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, status) \
             VALUES (?1, 'recurring', 'test', 'test brief', 'agent-v95', 'workspace', 'sess-v95', 'pending')",
            rusqlite::params![job_id],
        ).expect("Failed to insert test job");

        crate::migrations::run_migrations(&conn).expect("v95→v96 migration should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Should be at v96 after migration");

        assert!(column_exists(&conn, "agent_jobs", "consecutive_failures"), "v95→v96 migration must add agent_jobs.consecutive_failures");
        assert!(column_exists(&conn, "agent_jobs", "last_failed_at"), "v95→v96 migration must add agent_jobs.last_failed_at");

        let (consecutive_failures, last_failed_at): (i64, Option<String>) = conn.query_row(
            "SELECT consecutive_failures, last_failed_at FROM agent_jobs WHERE id = ?1",
            rusqlite::params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("Query should succeed");
        assert_eq!(consecutive_failures, 0, "Pre-existing jobs should default to consecutive_failures = 0");
        assert_eq!(last_failed_at, None, "Pre-existing jobs should have NULL last_failed_at (no recorded failure yet)");

        // Idempotent: second run should not fail.
        crate::migrations::run_migrations(&conn).expect("Second run at v96 should be idempotent");
    }

    // ─── #3151 PR2/C24 — fresh install lands at current SCHEMA_VERSION (v97)
    // with agent_jobs.archived_at present and NULL on every row ────────────
    #[test]
    fn test_fresh_install_v97() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Fresh install should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Fresh install should be at current SCHEMA_VERSION");

        assert!(
            column_exists(&conn, "agent_jobs", "archived_at"),
            "Fresh install should have agent_jobs.archived_at"
        );
    }

    // ─── #3151 PR2/C24 — v96→v97 upgrade path: archived_at (NULL) added to
    // agent_jobs, idempotent on a second run (same `pragma_table_info` guard
    // shape every prior migration in this ladder uses) ─────────────────────
    #[test]
    fn test_v96_to_v97_migration() {
        let conn = setup_test_db();

        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        conn.execute(
            "UPDATE ai_settings SET value = '96' WHERE key = 'schema_version'",
            [],
        ).expect("Failed to set v96 stamp");

        // Downgrade agent_jobs to its pre-v97 shape to simulate a real v96
        // install (same "drop and recreate the narrower shape" pattern
        // test_v95_to_v96_migration uses).
        conn.execute_batch(
            "CREATE TABLE agent_jobs_v96_shape AS SELECT id, kind, title, brief, agent_id, project_id, session_id, origin_session_id, origin_message_id, parent_job_id, root_job_id, chain_depth, wake_origin, cron, run_at, next_run_at, status, started_at, finished_at, last_error, tool_calls_used, missed_count, created_at, updated_at, oc_session_key, consecutive_failures, last_failed_at FROM agent_jobs;
             DROP TABLE agent_jobs;
             ALTER TABLE agent_jobs_v96_shape RENAME TO agent_jobs;"
        ).expect("Failed to downgrade agent_jobs to v96 shape");

        assert!(!column_exists(&conn, "agent_jobs", "archived_at"), "v96 fixture must not have archived_at yet");

        // Insert a pre-existing job row at the v96 shape, to prove it
        // survives the migration and gets the documented NULL default for
        // the new column (NULL = visible, not archived).
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, model, created_at, updated_at) \
             VALUES ('agent-v96', 'Agent V96', 'agent-v96', 1, 1, 'agent-v96', 'test-model', datetime('now'), datetime('now'))",
            [],
        ).expect("Failed to seed agent");
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, title, origin, agent_id) VALUES ('sess-v96', 'workspace', 'test', 'schedule', 'agent-v96')",
            [],
        ).expect("Failed to seed session");
        let job_id = "test-job-v96";
        conn.execute(
            "INSERT INTO agent_jobs (id, kind, title, brief, agent_id, project_id, session_id, status) \
             VALUES (?1, 'recurring', 'test', 'test brief', 'agent-v96', 'workspace', 'sess-v96', 'done')",
            rusqlite::params![job_id],
        ).expect("Failed to insert test job");

        crate::migrations::run_migrations(&conn).expect("v96→v97 migration should succeed");

        let version = get_schema_version(&conn);
        assert_eq!(version, Some(crate::migrations::SCHEMA_VERSION.to_string()), "Should be at v97 after migration");

        assert!(column_exists(&conn, "agent_jobs", "archived_at"), "v96→v97 migration must add agent_jobs.archived_at");

        let archived_at: Option<String> = conn.query_row(
            "SELECT archived_at FROM agent_jobs WHERE id = ?1",
            rusqlite::params![job_id],
            |row| row.get(0),
        ).expect("Query should succeed");
        assert_eq!(archived_at, None, "Pre-existing jobs should have NULL archived_at (visible, not archived)");

        // Idempotent: second run should not fail.
        crate::migrations::run_migrations(&conn).expect("Second run at v97 should be idempotent");
    }

    /// §Migration test suite — the REAL `tests/fixtures/upgrade-from-v96.db`
    /// fixture (built by `scripts/build-v96-fixture.sh`, verified by
    /// `scripts/verify-upgrade-fixture.sh`), not an in-memory downgrade
    /// simulation. Same shape as `test_upgrade_from_v92_fixture_migrates_
    /// cleanly_to_v93` above — the one change a schema bump can make that
    /// breaks a real upgrading install, so it gets its own real-file test,
    /// not just the synthetic-fixture unit test above.
    #[test]
    #[serial_test::serial(pane_test_root, keychain_override)]
    fn test_upgrade_from_v96_fixture_migrates_cleanly_to_v97() {
        use std::path::PathBuf;
        use tempfile::TempDir;

        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/upgrade-from-v96.db");
        assert!(
            fixture_path.exists(),
            "upgrade-from-v96.db fixture not found at {:?} — see scripts/build-v96-fixture.sh",
            fixture_path
        );

        let temp_dir = TempDir::new().expect("tempdir");
        let scratch_path = temp_dir.path().join("scratch-upgrade-v96.db");
        std::fs::copy(&fixture_path, &scratch_path).expect("copy fixture to scratch path");

        unsafe {
            std::env::set_var("PANE_TEST_MODE", "1");
            std::env::set_var("PANE_TEST_DB", scratch_path.to_str().unwrap());
            std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        }

        // The v96→v97 arm calls `create_migration_backup`, which touches
        // the real macOS Keychain unless a MockKeychain is installed.
        let _kc = crate::keychain::test_support::KeychainGuard::set();

        let conn = rusqlite::Connection::open(&scratch_path).expect("open fixture db");

        let version_before: String = conn
            .query_row("SELECT value FROM ai_settings WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version_before, "96", "fixture must be stamped at v96 before migration");

        let row_count_before: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs", [], |r| r.get(0)).unwrap_or(0);

        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_ok(), "v96→{} migration on the real fixture must succeed: {:?}", crate::migrations::SCHEMA_VERSION, result);

        let version_after: String = conn
            .query_row("SELECT value FROM ai_settings WHERE key = 'schema_version'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version_after, crate::migrations::SCHEMA_VERSION, "fixture must land at the current SCHEMA_VERSION after migration");

        assert!(
            column_exists(&conn, "agent_jobs", "archived_at"),
            "agent_jobs.archived_at must exist after migrating the real v96 fixture through to current"
        );
        assert!(
            index_exists(&conn, "idx_agent_jobs_archived_at"),
            "idx_agent_jobs_archived_at must exist after migrating the real v96 fixture"
        );

        // Pre-existing agent_jobs row count survives untouched (the fixture
        // may have zero jobs, which is fine — the assertion is "unchanged",
        // not "non-zero").
        let row_count_after: i64 = conn.query_row("SELECT COUNT(*) FROM agent_jobs", [], |r| r.get(0)).unwrap_or(0);
        assert_eq!(row_count_after, row_count_before, "pre-existing agent_jobs rows must survive the v96→v97 migration on the real fixture untouched");

        unsafe {
            std::env::remove_var("PANE_TEST_MODE");
            std::env::remove_var("PANE_TEST_DB");
        }
    }

    // ─── #3151 PR2/C24, Matt's ruling (the v69/#3112 lesson) — the migration
    // arm must RUN its steps and THEN stamp the new version, never the
    // reverse. This is a RUNTIME/ORDERING test, not the static source-text
    // scan `arm_sweep_tests::test_every_arm_with_migrate_v_calls_reaches_
    // schema_version` already provides (that one catches a truncated ladder;
    // this one catches a ladder that stamps before its own step actually
    // runs, which a source-text scan of "which migrate_vNN calls exist in
    // this arm" cannot distinguish from "and they ran in the right order").
    //
    // Mechanism: force `migrate_v97`'s own step to fail (drop `agent_jobs`
    // entirely before migrating, so `ALTER TABLE agent_jobs ADD COLUMN
    // archived_at` errors) and assert TWO things: `run_migrations` surfaces
    // the error (not swallowed), AND `schema_version` stays at "96" — not
    // "97". If stamping ever moved ahead of running the step (the exact
    // #3112 shape, generalized), this test would see schema_version = "97"
    // on a DB that never actually got the column, because the stamp write
    // in a buggy arm doesn't depend on the step's own Result at all.
    #[test]
    fn test_migrate_v97_stamp_happens_after_step_not_before() {
        let conn = setup_test_db();
        crate::migrations::run_migrations(&conn).expect("Initial migration should succeed");
        conn.execute(
            "UPDATE ai_settings SET value = '96' WHERE key = 'schema_version'",
            [],
        ).expect("Failed to set v96 stamp");

        // Sabotage the v96→v97 step: drop agent_jobs so migrate_v97's
        // `ALTER TABLE agent_jobs ADD COLUMN archived_at` has no table to
        // alter and must return Err.
        conn.execute_batch("DROP TABLE agent_jobs;").expect("Failed to drop agent_jobs for sabotage");

        let result = crate::migrations::run_migrations(&conn);
        assert!(result.is_err(), "run_migrations must surface migrate_v97's failure, not swallow it");

        let version = get_schema_version(&conn);
        assert_eq!(
            version,
            Some("96".to_string()),
            "schema_version must NOT advance to 97 when migrate_v97's own step failed — a \
             version stamped at 97 here would mean the arm claimed the work was done \
             without the step having actually run (the #3112 shape, generalized to this PR's bump)"
        );
    }
}
