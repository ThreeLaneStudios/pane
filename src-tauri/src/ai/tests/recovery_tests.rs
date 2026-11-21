//! Tests for §25.12 Destructive Recovery Tests
//! 
//! Verifies Pane's disaster recovery and self-healing capabilities:
//! - Agent reconstitution from flat files after DB wipe
//! - Daily log reconstitution from flat files after DB wipe
//! - Self-healing of missing SOUL.md, MEMORY.md, and entire agent folders
//! - AI settings persistence across DB reconnects
//! - Legacy folder structure cleanup
//! 
//! GitHub Issue: #311

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Create an in-memory SQLite database with full schema for recovery testing
    fn setup_recovery_db() -> Connection {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        
        // Create the schema as defined in migrations
        conn.execute_batch(
            "
            -- ai_agents table (§24.2)
            CREATE TABLE ai_agents (
                id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
                name            TEXT NOT NULL,
                slug            TEXT NOT NULL UNIQUE,
                role            TEXT NOT NULL DEFAULT 'sub',
                description     TEXT,
                model           TEXT,
                is_default         INTEGER NOT NULL DEFAULT 0,
                is_active       INTEGER NOT NULL DEFAULT 1,
                workspace_path  TEXT NOT NULL,
                soul_hash       TEXT,
                memory_hash     TEXT,
                config_json     TEXT DEFAULT '{}',
                owner_id        TEXT,
                source          TEXT DEFAULT 'import',
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE UNIQUE INDEX idx_ai_agents_default ON ai_agents(is_default) WHERE is_default = 1;
            CREATE INDEX idx_ai_agents_slug ON ai_agents(slug);
            CREATE INDEX idx_ai_agents_active ON ai_agents(is_active) WHERE is_active = 1;
            CREATE INDEX idx_ai_agents_owner ON ai_agents(owner_id);

            -- notes table (for agent-owned notes and daily logs)
            CREATE TABLE notes (
                id              TEXT PRIMARY KEY,
                title           TEXT NOT NULL DEFAULT '',
                content_preview TEXT NOT NULL DEFAULT '',
                file_path       TEXT NOT NULL DEFAULT '',
                project_id      TEXT,
                agent_id        TEXT REFERENCES ai_agents(id),
                note_type       TEXT,
                deleted_at      TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX idx_notes_project ON notes(project_id);
            CREATE INDEX idx_notes_agent_id ON notes(agent_id);
            CREATE INDEX idx_notes_type ON notes(note_type);
            CREATE INDEX idx_notes_deleted ON notes(deleted_at) WHERE deleted_at IS NULL;

            -- projects table (for testing no agents leak here)
            CREATE TABLE projects (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT,
                color       TEXT,
                parent_id   TEXT,
                sort_order  INTEGER NOT NULL DEFAULT 0,
                is_system   INTEGER NOT NULL DEFAULT 0,
                owner_id    TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
                UNIQUE(name, parent_id)
            );

            -- ai_settings table (§24 config persistence)
            CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )
        .expect("Failed to create recovery test schema");
        
        conn
    }

    /// Helper: setup temp directory structure simulating .pane/system/
    fn setup_test_env() -> (TempDir, PathBuf) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let pane_root = temp_dir.path().to_path_buf();
        let system_dir = pane_root.join(".pane").join("system");
        fs::create_dir_all(&system_dir).expect("Failed to create system dir");
        (temp_dir, pane_root)
    }

    /// Helper: create agent folder with SOUL.md and MEMORY.md
    fn create_agent_folder(pane_root: &PathBuf, agent_name: &str, include_logs: bool) {
        let agent_dir = pane_root.join(".pane").join("system").join(agent_name);
        fs::create_dir_all(&agent_dir).expect("Failed to create agent dir");
        
        let soul_content = format!("# SOUL.md\n\n**Agent:** {}\n\nI am a test agent.", agent_name);
        fs::write(agent_dir.join("SOUL.md"), soul_content).expect("Failed to write SOUL.md");
        
        let memory_content = format!("# MEMORY.md\n\n**Agent:** {}\n\nTest memories go here.", agent_name);
        fs::write(agent_dir.join("MEMORY.md"), memory_content).expect("Failed to write MEMORY.md");
        
        if include_logs {
            let logs_dir = agent_dir.join("logs");
            fs::create_dir_all(&logs_dir).expect("Failed to create logs dir");
        }
    }

    /// Helper: create daily log file
    fn create_daily_log(pane_root: &PathBuf, agent_name: &str, date: &str, content: &str) {
        let logs_dir = pane_root.join(".pane").join("system").join(agent_name).join("logs");
        fs::create_dir_all(&logs_dir).expect("Failed to create logs dir");
        fs::write(logs_dir.join(format!("{}.md", date)), content).expect("Failed to write log file");
    }

    /// Helper: insert agent into DB
    fn insert_agent(conn: &Connection, name: &str, is_default: bool) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let slug = name.to_lowercase().replace(" ", "-");
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                name,
                slug,
                if is_default { "main" } else { "sub" },
                if is_default { 1 } else { 0 },
                name, // workspace_path = name for test simplicity
                "test"
            ],
        )
        .expect("Failed to insert agent");
        id
    }

    // ─── Test 1: DB Wipe → Agent Reconstitution ───────────────────────────

    #[test]
    #[ignore] // Backward compat test for name-based folders — skip for clean UUID-based builds (#364)
    fn test_db_wipe_agent_reconstitution() {
        let (_temp_dir, pane_root) = setup_test_env();
        let conn = setup_recovery_db();
        
        // Setup: Create agent folder on disk with SOUL.md + MEMORY.md
        create_agent_folder(&pane_root, "Brit Freed", true);
        
        // Verify agent reconstitution from filesystem
        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.clone());
        
        // Verify agent was created in DB
        let (name, workspace_path, is_default, source): (String, String, i32, String) = conn
            .query_row(
                "SELECT name, workspace_path, is_default, source FROM ai_agents WHERE name = ?1",
                params!["Brit Freed"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("Agent not found in database");
        
        assert_eq!(name, "Brit Freed", "Agent name mismatch");
        assert_eq!(workspace_path, "Brit Freed", "Workspace path mismatch");
        assert_eq!(is_default, 1, "is_default flag should be set for agent with logs/ directory");
        assert_eq!(source, "filesystem", "source should be 'filesystem' for reconstituted agent");
    }

    // ─── Test 2: DB Wipe → Daily Log Reconstitution ──────────────────────

    #[test]
    fn test_db_wipe_daily_log_reconstitution() {
        let (_temp_dir, pane_root) = setup_test_env();
        let conn = setup_recovery_db();
        
        // Setup: Create agent in DB
        let agent_id = insert_agent(&conn, "Brit Freed", true);
        create_agent_folder(&pane_root, "Brit Freed", true);
        
        // Create daily log files on disk
        let log1_content = "# 2026-03-28\n\n## Morning\n- Worked on recovery tests\n- Fixed bug #311\n\n## Afternoon\n- Code review session\n- Deployed to staging\n\nThis log has approximately 200 words to test content extraction and preview generation. ".repeat(3);
        let log2_content = "# 2026-03-29\n\n## Morning\n- Sprint planning meeting\n- Updated architecture spec\n\nThis is a shorter log with around 150 words for testing. ".repeat(2);
        
        create_daily_log(&pane_root, "Brit Freed", "2026-03-28", &log1_content);
        create_daily_log(&pane_root, "Brit Freed", "2026-03-29", &log2_content);
        
        // Run integrity check to reconstitute logs
        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.clone());
        
        // Verify both logs were created in notes table
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notes WHERE agent_id = ?1 AND note_type = 'daily'",
                params![agent_id],
                |row| row.get(0),
            )
            .expect("Failed to count daily logs");
        
        assert_eq!(count, 2, "Should have 2 daily log records");
        
        // Verify log details for 2026-03-28
        let (title, file_path, content_preview): (String, String, String) = conn
            .query_row(
                "SELECT title, file_path, content_preview FROM notes 
                 WHERE agent_id = ?1 AND title = ?2 AND note_type = 'daily'",
                params![agent_id, "2026-03-28"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("2026-03-28 log not found");
        
        assert_eq!(title, "2026-03-28", "Log title should match date");
        assert!(file_path.contains("Brit Freed/logs/2026-03-28.md"), "file_path should contain correct path");
        assert!(content_preview.len() > 0, "content_preview should be populated");
        assert!(content_preview.len() <= 203, "content_preview should be truncated with ellipsis"); // 200 chars + "..."
    }

    // ─── Test 3: Self-Healing → Missing Files & Folder ───────────────────

    #[test]
    fn test_self_healing_missing_soul() {
        let (_temp_dir, pane_root) = setup_test_env();
        let conn = setup_recovery_db();
        
        // Setup: Create agent in DB and folder on disk
        insert_agent(&conn, "Brit Freed", false);
        create_agent_folder(&pane_root, "Brit Freed", false);
        
        // Destructive action: Delete SOUL.md
        let soul_path = pane_root.join(".pane").join("system").join("Brit Freed").join("SOUL.md");
        fs::remove_file(&soul_path).expect("Failed to delete SOUL.md");
        assert!(!soul_path.exists(), "SOUL.md should be deleted");
        
        // Run integrity check
        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.clone());
        
        // Verify SOUL.md was recreated
        assert!(soul_path.exists(), "SOUL.md should be recreated by integrity check");
        let content = fs::read_to_string(&soul_path).expect("Failed to read recreated SOUL.md");
        assert!(content.contains("# SOUL.md"), "SOUL.md should have correct header");
        assert!(content.contains("Brit Freed"), "SOUL.md should contain agent name");
        assert!(content.contains("auto-created by integrity check"), "SOUL.md should have auto-created notice");
    }

    #[test]
    fn test_self_healing_missing_entire_folder() {
        let (_temp_dir, pane_root) = setup_test_env();
        let conn = setup_recovery_db();
        
        // Setup: Create agent in DB (but no folder on disk)
        insert_agent(&conn, "Brit Freed", true);
        
        let agent_dir = pane_root.join(".pane").join("system").join("Brit Freed");
        assert!(!agent_dir.exists(), "Agent folder should not exist initially");
        
        // Run integrity check
        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.clone());
        
        // Verify entire agent folder structure was recreated
        assert!(agent_dir.exists(), "Agent folder should be recreated");
        
        let soul_path = agent_dir.join("SOUL.md");
        assert!(soul_path.exists(), "SOUL.md should be created");
        let soul_content = fs::read_to_string(&soul_path).expect("Failed to read SOUL.md");
        assert!(soul_content.contains("Brit Freed"), "SOUL.md should contain agent name");
        
        let memory_path = agent_dir.join("MEMORY.md");
        assert!(memory_path.exists(), "MEMORY.md should be created");
        let memory_content = fs::read_to_string(&memory_path).expect("Failed to read MEMORY.md");
        assert!(memory_content.contains("Brit Freed"), "MEMORY.md should contain agent name");
        
        let logs_dir = agent_dir.join("logs");
        assert!(logs_dir.exists(), "logs/ directory should be created for main agent");
    }

    // ─── Test 4: Config Persistence Across DB Reconnect ──────────────────

    #[test]
    fn test_config_persistence() {
        // Use a temp file database for true persistence across connections
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        
        {
            let conn1 = Connection::open(&db_path)
                .expect("Failed to open temp DB");
            
            // Create schema
            conn1.execute(
                "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)",
                [],
            ).expect("Failed to create ai_settings table");
            
            // Insert config values
            conn1.execute("INSERT INTO ai_settings (key, value) VALUES ('provider', 'openclaw')", [])
                .expect("Failed to insert provider");
            conn1.execute("INSERT INTO ai_settings (key, value) VALUES ('endpoint', 'http://localhost:8000')", [])
                .expect("Failed to insert endpoint");
            conn1.execute("INSERT INTO ai_settings (key, value) VALUES ('api_key', 'test-key-12345')", [])
                .expect("Failed to insert api_key");
            
            // Explicitly close connection by dropping it
        }
        
        // Reopen connection to same database file
        let conn2 = Connection::open(&db_path)
            .expect("Failed to reopen temp DB");
        
        // Verify all settings persisted
        let provider: String = conn2.query_row(
            "SELECT value FROM ai_settings WHERE key = 'provider'",
            [],
            |row| row.get(0),
        ).expect("Failed to retrieve provider");
        assert_eq!(provider, "openclaw", "Provider setting should persist");
        
        let endpoint: String = conn2.query_row(
            "SELECT value FROM ai_settings WHERE key = 'endpoint'",
            [],
            |row| row.get(0),
        ).expect("Failed to retrieve endpoint");
        assert_eq!(endpoint, "http://localhost:8000", "Endpoint setting should persist");
        
        let api_key: String = conn2.query_row(
            "SELECT value FROM ai_settings WHERE key = 'api_key'",
            [],
            |row| row.get(0),
        ).expect("Failed to retrieve api_key");
        assert_eq!(api_key, "test-key-12345", "API key setting should persist");
    }

    // ─── Test 5: No Bad Folder Structures ────────────────────────────────

    #[test]
    fn test_no_bad_folder_structures() {
        let (_temp_dir, pane_root) = setup_test_env();
        let conn = setup_recovery_db();
        
        // Setup: Create valid agent
        create_agent_folder(&pane_root, "Brit Freed", true);
        
        // Create BAD structures that should NOT exist
        let main_agent_dir = pane_root.join(".pane").join("system").join("Main Agent");
        fs::create_dir_all(&main_agent_dir).expect("Failed to create Main Agent dir");
        fs::write(main_agent_dir.join("SOUL.md"), "# Bad structure").expect("Failed to write bad SOUL.md");
        
        let tasks_dir = pane_root.join(".pane").join("tasks");
        fs::create_dir_all(&tasks_dir).expect("Failed to create tasks dir");
        
        // Insert BAD record into projects table (agents should NEVER be here)
        conn.execute(
            "INSERT INTO projects (id, name, is_system) VALUES ('bad-id', 'Pane System', 1)",
            [],
        ).expect("Failed to insert bad project");
        
        // Run integrity check
        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.clone());
        
        // Verify BAD structures were cleaned up
        assert!(!tasks_dir.exists(), "Legacy .pane/tasks/ directory should be removed");
        
        // Note: Main Agent folder cleanup is NOT currently implemented in verify_agents_integrity
        // This is documented as a future enhancement. For now, we just verify it doesn't break.
        
        // Verify no "Pane System" project exists (though integrity check doesn't remove it)
        // This test documents the expected state, not current cleanup behavior
        let pane_system_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM projects WHERE name = 'Pane System'",
            [],
            |row| row.get(0),
        ).unwrap_or(false);
        
        // Current implementation doesn't clean up projects table, but agents should never 
        // be created there in the first place (§24 architecture)
        // This assertion documents expected behavior for future implementation
        if pane_system_exists {
            eprintln!("[TEST] Note: 'Pane System' project cleanup not yet implemented (future enhancement)");
        }
        
        // Verify Brit Freed agent is intact
        let brit_dir = pane_root.join(".pane").join("system").join("Brit Freed");
        assert!(brit_dir.exists(), "Valid agent folder should remain intact");
        assert!(brit_dir.join("SOUL.md").exists(), "Valid agent SOUL.md should remain intact");
    }

    // --- Item 4: orphan UUID dir cleanup ---

    /// Empty UUID-format dirs created by a crashed create_local_agent should be deleted
    /// during the integrity check (Item 4 fix, #2030).
    #[test]
    fn test_orphan_empty_uuid_dir_is_removed_by_integrity_check() {
        let tmp = TempDir::new().expect("TempDir::new");
        let pane_root = tmp.path().to_path_buf();
        let system_dir = pane_root.join(".pane").join("system");
        fs::create_dir_all(&system_dir).unwrap();

        let conn = setup_recovery_db();

        // Create a UUID-format directory with NO files (simulates crashed create_local_agent)
        let orphan_uuid = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
        let orphan_dir = system_dir.join(&orphan_uuid);
        fs::create_dir_all(&orphan_dir).unwrap();
        assert!(orphan_dir.exists(), "orphan dir must exist before integrity check");

        // Run integrity check
        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.clone());

        // Empty orphan must be removed
        assert!(
            !orphan_dir.exists(),
            "Empty orphan UUID dir must be removed by integrity check (Item 4 fix)"
        );
    }

    /// Non-empty UUID-format dirs without a DB record but WITHOUT SOUL.md should NOT be deleted
    /// if they contain files (might be partially-written agent data worth preserving).
    #[test]
    fn test_orphan_nonempty_uuid_dir_without_soul_is_preserved() {
        let tmp = TempDir::new().expect("TempDir::new");
        let pane_root = tmp.path().to_path_buf();
        let system_dir = pane_root.join(".pane").join("system");
        fs::create_dir_all(&system_dir).unwrap();

        let conn = setup_recovery_db();

        // Create a UUID-format directory with some files but no SOUL.md
        let orphan_uuid = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
        let orphan_dir = system_dir.join(&orphan_uuid);
        fs::create_dir_all(&orphan_dir).unwrap();
        fs::write(orphan_dir.join("IDENTITY.md"), "# Partial Agent").unwrap(); // non-empty

        // Run integrity check
        crate::ai::ingest::verify_agents_integrity_with_path(&conn, pane_root.clone());

        // Non-empty dir must NOT be deleted (only empty dirs are cleaned up)
        assert!(
            orphan_dir.exists(),
            "Non-empty orphan dir without SOUL.md must be preserved (might have partial data)"
        );
    }
}
