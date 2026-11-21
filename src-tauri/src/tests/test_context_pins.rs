// Tests for context_pins module (#471)

#[cfg(test)]
mod tests {
    use crate::context_pins::*;
    use crate::database::DbState;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn create_test_db() -> (TempDir, DbState) {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let db_path = temp_dir.path().join("test.db");
        let conn = Connection::open(&db_path).expect("Failed to open test database");
        
        // Create minimal schema
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT,
                updated_at TEXT
            );
            
            CREATE TABLE IF NOT EXISTS folders (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                name TEXT NOT NULL
            );
            
            CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                folder_id TEXT,
                title TEXT NOT NULL,
                file_path TEXT NOT NULL,
                body TEXT,
                note_type TEXT,
                deleted_at TEXT
            );
            
            CREATE TABLE IF NOT EXISTS note_context_pins (
                note_id TEXT PRIMARY KEY,
                pinned_at TEXT NOT NULL,
                pinned_by TEXT NOT NULL
            );
            "
        ).expect("Failed to create test schema");
        
        (temp_dir, DbState::new(conn))
    }

    fn setup_test_project() -> (TempDir, DbState, String, String) {
        let (_temp_dir, db) = create_test_db();
        let temp_dir = TempDir::new().expect("Failed to create temp directory for files");

        // Create a test project
        let project_id = "test-proj-001".to_string();
        
        // Create test file
        let test_file_path = temp_dir.path().join("test-note.md");
        std::fs::write(&test_file_path, "Test note content for pinning.").unwrap();
        
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO projects (id, name) VALUES (?1, 'Test Project')",
                rusqlite::params![&project_id],
            )
            .unwrap();

            // Create a test folder
            conn.execute(
                "INSERT INTO folders (id, project_id, name) VALUES ('folder-1', ?1, 'General')",
                rusqlite::params![&project_id],
            )
            .unwrap();

            // Create a test note
            conn.execute(
                "INSERT INTO notes (id, project_id, folder_id, title, file_path) 
                 VALUES ('note-1', ?1, 'folder-1', 'Test Note', ?2)",
                rusqlite::params![&project_id, test_file_path.to_str().unwrap()],
            )
            .unwrap();
        }

        (_temp_dir, db, project_id, "note-1".to_string())
    }

    #[test]
    fn test_pin_and_list_notes() {
        let (_temp_dir, db, project_id, note_id) = setup_test_project();

        // Pin the note
        let result = pin_note_to_context_internal(&note_id, &db);
        assert!(result.is_ok(), "Should pin note successfully");

        // List pinned notes
        let pins = list_context_pins_internal(&project_id, &db).unwrap();
        assert_eq!(pins.len(), 1, "Should have 1 pinned note");
        assert_eq!(pins[0].id, note_id);
        assert_eq!(pins[0].title, "Test Note");
    }

    #[test]
    fn test_unpin_note() {
        let (_temp_dir, db, project_id, note_id) = setup_test_project();

        // Pin then unpin
        pin_note_to_context_internal(&note_id, &db).unwrap();
        unpin_note_from_context_internal(&note_id, &db).unwrap();

        // Should be empty
        let pins = list_context_pins_internal(&project_id, &db).unwrap();
        assert_eq!(pins.len(), 0, "Should have no pinned notes after unpin");
    }

    #[test]
    fn test_is_note_pinned() {
        let (_temp_dir, db, _project_id, note_id) = setup_test_project();

        // Initially not pinned
        let pinned = is_note_pinned_internal(&note_id, &db).unwrap();
        assert!(!pinned, "Note should not be pinned initially");

        // Pin it
        pin_note_to_context_internal(&note_id, &db).unwrap();

        // Should be pinned now
        let pinned = is_note_pinned_internal(&note_id, &db).unwrap();
        assert!(pinned, "Note should be pinned");
    }

    #[test]
    fn test_budget_usage() {
        let (_temp_dir, db, project_id, note_id) = setup_test_project();

        // Pin a note
        pin_note_to_context_internal(&note_id, &db).unwrap();

        // Get budget usage
        let usage = get_context_budget_usage_internal(&project_id, &db).unwrap();

        assert_eq!(usage.pinned_notes_count, 1);
        assert!(usage.total_used > 0);
        assert_eq!(usage.total_budget, 32000);
    }
}
