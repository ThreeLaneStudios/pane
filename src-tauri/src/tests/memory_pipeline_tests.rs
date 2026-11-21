//! Tests for Memory.md backfill functionality (Issue #803)

#[cfg(test)]
mod tests {
    use crate::database::{DbState, seed_defaults, get_pane_root};
    use crate::projects::{CreateProjectRequest, create_project_internal};
    use rusqlite::params;
    use serial_test::serial;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_db() -> (TempDir, DbState, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let db_path = temp_dir.path().join("test.db");
        
        let conn = rusqlite::Connection::open(&db_path)
            .expect("Failed to open test database");
        
        // Create minimal schema for projects and notes
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS projects (
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
            
            CREATE TABLE IF NOT EXISTS notes (
                id              TEXT PRIMARY KEY,
                title           TEXT NOT NULL,
                content_preview TEXT NOT NULL DEFAULT '',
                file_path       TEXT NOT NULL DEFAULT '',
                project_id      TEXT,
                folder_id       TEXT,
                note_type       TEXT,
                word_count      INTEGER NOT NULL DEFAULT 0,
                pinned          INTEGER NOT NULL DEFAULT 0,
                owner_id        TEXT,
                tags_json       TEXT NOT NULL DEFAULT '[]',
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL,
                deleted_at      TEXT
            );
            
            CREATE TABLE IF NOT EXISTS user_dashboard_config (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            
            CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_sessions (
                id             TEXT NOT NULL PRIMARY KEY,
                project_id     TEXT NOT NULL,
                title          TEXT NOT NULL DEFAULT '',
                origin         TEXT NOT NULL DEFAULT 'user',
                created_at     TEXT NOT NULL DEFAULT (datetime('now')),
                last_active_at TEXT NOT NULL DEFAULT (datetime('now')),
                hidden_at      TEXT,
                is_active      INTEGER NOT NULL DEFAULT 0,
                agent_id       TEXT
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_active_per_surface
                ON chat_sessions(project_id) WHERE is_active = 1;
            "
        ).expect("Failed to create test schema");
        
        // Set up temp filesystem root
        let temp_fs_dir = TempDir::new().expect("Failed to create temp filesystem directory");
        std::env::set_var("PANE_TEST_ROOT", temp_fs_dir.path());
        
        let db_state = DbState::new(conn);
        (temp_dir, db_state, temp_fs_dir)
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_backfill_creates_both_file_and_notes_row() {
        eprintln!("\n[TEST] test_backfill_creates_both_file_and_notes_row");
        
        let (_temp_dir, db_state, temp_fs_dir) = setup_test_db();
        let conn = db_state.0.lock().unwrap();
        
        // Ensure Projects/ directory exists before creating project
        let projects_dir = temp_fs_dir.path().join("Projects");
        std::fs::create_dir_all(&projects_dir).expect("Failed to create Projects dir");
        
        // Create project using create_project_internal (auto-creates Memory.md)
        let request = CreateProjectRequest {
            name: "Test Project Alpha".to_string(),
            description: Some("Test project for backfill".to_string()),
            color: Some("#FF0000".to_string()),
            category: None,
            first_folder_name: None,
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        
        drop(conn);
        let project = create_project_internal(&request, &db_state)
            .expect("Failed to create project");
        let conn = db_state.0.lock().unwrap();
        
        eprintln!("[TEST] Created project: {} ({})", project.name, project.id);
        
        // Verify Memory note was created
        let note_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE project_id = ?1 AND note_type = 'memory' AND deleted_at IS NULL",
            params![&project.id],
            |row| row.get(0),
        ).expect("Failed to count notes");
        assert_eq!(note_count, 1, "Memory note should exist after project creation");
        
        // Verify Memory.md file exists
        let memory_path = get_pane_root().join("Projects").join(&project.name).join("Memory.md");
        assert!(memory_path.exists(), "Memory.md file should exist after project creation");
        
        // Simulate pre-#802 state: delete both notes row AND file
        conn.execute(
            "DELETE FROM notes WHERE project_id = ?1 AND note_type = 'memory'",
            params![&project.id],
        ).expect("Failed to delete notes row");
        
        fs::remove_file(&memory_path).expect("Failed to delete Memory.md file");
        
        eprintln!("[TEST] Deleted both notes row and Memory.md file");
        
        // Verify both are gone
        let note_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE project_id = ?1 AND note_type = 'memory'",
            params![&project.id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(note_count, 0, "Memory note should be deleted");
        assert!(!memory_path.exists(), "Memory.md file should be deleted");
        
        // Run seed_defaults (which includes backfill logic)
        drop(conn);
        seed_defaults(&db_state.0.lock().unwrap())
            .expect("seed_defaults should succeed");
        let conn = db_state.0.lock().unwrap();
        
        eprintln!("[TEST] Ran seed_defaults with backfill logic");
        
        // Verify notes row was recreated
        let note_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE project_id = ?1 AND note_type = 'memory' AND deleted_at IS NULL",
            params![&project.id],
            |row| row.get(0),
        ).expect("Failed to count notes after backfill");
        assert_eq!(note_count, 1, "Memory note should be backfilled");
        
        // Verify file was recreated
        assert!(memory_path.exists(), "Memory.md file should be backfilled");
        
        // Verify file content
        let content = fs::read_to_string(&memory_path)
            .expect("Failed to read Memory.md");
        assert!(content.contains(&format!("# Memory — {}", project.name)), 
                "Memory.md should have correct header");
        
        eprintln!("[TEST] ✓ Backfill successfully created both file and notes row");
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_backfill_creates_notes_row_for_existing_file() {
        eprintln!("\n[TEST] test_backfill_creates_notes_row_for_existing_file");
        
        let (_temp_dir, db_state, temp_fs_dir) = setup_test_db();
        let conn = db_state.0.lock().unwrap();
        
        // Ensure Projects/ directory exists before creating project
        let projects_dir = temp_fs_dir.path().join("Projects");
        std::fs::create_dir_all(&projects_dir).expect("Failed to create Projects dir");
        
        // Create project using create_project_internal
        let request = CreateProjectRequest {
            name: "Test Project Beta".to_string(),
            description: Some("Test project for notes-only backfill".to_string()),
            color: Some("#00FF00".to_string()),
            category: None,
            first_folder_name: None,
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        
        drop(conn);
        let project = create_project_internal(&request, &db_state)
            .expect("Failed to create project");
        let conn = db_state.0.lock().unwrap();
        
        eprintln!("[TEST] Created project: {} ({})", project.name, project.id);
        
        // Verify Memory.md file exists
        let memory_path = get_pane_root().join("Projects").join(&project.name).join("Memory.md");
        assert!(memory_path.exists(), "Memory.md file should exist after project creation");
        
        // Read original content
        let original_content = fs::read_to_string(&memory_path)
            .expect("Failed to read original Memory.md");
        
        // Simulate Pane Marketing's case: delete notes row only, leave file
        conn.execute(
            "DELETE FROM notes WHERE project_id = ?1 AND note_type = 'memory'",
            params![&project.id],
        ).expect("Failed to delete notes row");
        
        eprintln!("[TEST] Deleted notes row only (file remains)");
        
        // Verify notes row is gone but file remains
        let note_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE project_id = ?1 AND note_type = 'memory'",
            params![&project.id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(note_count, 0, "Memory note should be deleted");
        assert!(memory_path.exists(), "Memory.md file should still exist");
        
        // Run seed_defaults (which includes backfill logic)
        drop(conn);
        seed_defaults(&db_state.0.lock().unwrap())
            .expect("seed_defaults should succeed");
        let conn = db_state.0.lock().unwrap();
        
        eprintln!("[TEST] Ran seed_defaults with backfill logic");
        
        // Verify notes row was recreated
        let note_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE project_id = ?1 AND note_type = 'memory' AND deleted_at IS NULL",
            params![&project.id],
            |row| row.get(0),
        ).expect("Failed to count notes after backfill");
        assert_eq!(note_count, 1, "Memory note should be backfilled");
        
        // Verify file still exists and content unchanged
        assert!(memory_path.exists(), "Memory.md file should still exist");
        let new_content = fs::read_to_string(&memory_path)
            .expect("Failed to read Memory.md after backfill");
        assert_eq!(original_content, new_content, "File content should be unchanged");
        
        eprintln!("[TEST] ✓ Backfill successfully created notes row for existing file");
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_backfill_idempotent() {
        eprintln!("\n[TEST] test_backfill_idempotent");
        
        let (_temp_dir, db_state, temp_fs_dir) = setup_test_db();
        let conn = db_state.0.lock().unwrap();
        
        // Ensure Projects/ directory exists before creating project
        let projects_dir = temp_fs_dir.path().join("Projects");
        std::fs::create_dir_all(&projects_dir).expect("Failed to create Projects dir");
        
        // Create project
        let request = CreateProjectRequest {
            name: "Test Project Gamma".to_string(),
            description: Some("Test idempotency".to_string()),
            color: Some("#0000FF".to_string()),
            category: None,
            first_folder_name: None,
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        
        drop(conn);
        let project = create_project_internal(&request, &db_state)
            .expect("Failed to create project");
        let conn = db_state.0.lock().unwrap();
        
        eprintln!("[TEST] Created project: {} ({})", project.name, project.id);
        
        // Run seed_defaults multiple times
        drop(conn);
        seed_defaults(&db_state.0.lock().unwrap())
            .expect("First seed_defaults should succeed");
        seed_defaults(&db_state.0.lock().unwrap())
            .expect("Second seed_defaults should succeed");
        seed_defaults(&db_state.0.lock().unwrap())
            .expect("Third seed_defaults should succeed");
        let conn = db_state.0.lock().unwrap();
        
        eprintln!("[TEST] Ran seed_defaults 3 times");
        
        // Verify only one Memory note exists
        let note_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes WHERE project_id = ?1 AND note_type = 'memory' AND deleted_at IS NULL",
            params![&project.id],
            |row| row.get(0),
        ).expect("Failed to count notes");
        assert_eq!(note_count, 1, "Should only have one Memory note after multiple runs");
        
        eprintln!("[TEST] ✓ Backfill is idempotent");
    }
}
