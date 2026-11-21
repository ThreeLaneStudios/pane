//! Tests for project creation and management (Issue #412)

#[cfg(test)]
mod tests {
    use crate::database::DbState;
    use crate::projects::{CreateProjectRequest, create_project_internal};
    // use crate::reference_files::{CreateReferenceFileRequest, UpdateReferenceFileRequest};
    use rusqlite::params;
    use tempfile::TempDir;

    fn setup_test_db() -> (TempDir, DbState, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let db_path = temp_dir.path().join("test.db");
        
        let conn = rusqlite::Connection::open(&db_path)
            .expect("Failed to open test database");
        
        // Create minimal schema for projects and notes with flat file support
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS projects (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT,
                color       TEXT,
                category    TEXT,
                parent_id   TEXT,
                sort_order  INTEGER NOT NULL DEFAULT 0,
                owner_id    TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL,
                is_system   INTEGER NOT NULL DEFAULT 0,
                is_favorite INTEGER NOT NULL DEFAULT 0,
                default_folder_id TEXT
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

            -- Required by born-active hook (§30.2.6, #395)
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
    
    fn cleanup_test_env() {
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_create_project_with_custom_first_folder() {
        let (_temp_dir, db, temp_fs_dir) = setup_test_db();
        
        let request = CreateProjectRequest {
            name: "Test Project".to_string(),
            description: Some("Test description".to_string()),
            color: None,
            category: None,
            first_folder_name: Some("Inbox".to_string()),
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        
        let result = create_project_internal(&request, &db);
        assert!(result.is_ok(), "Failed to create project: {:?}", result.err());
        
        let project = result.unwrap();
        assert_eq!(project.name, "Test Project");
        assert_eq!(project.description, Some("Test description".to_string()));
        
        // Verify folder was created with custom name
        let conn = db.0.lock().unwrap();
        let folder_name: String = conn.query_row(
            "SELECT name FROM projects WHERE parent_id = ?1",
            params![project.id],
            |row| row.get(0)
        ).expect("Failed to query folder");
        
        assert_eq!(folder_name, "Inbox");
        
        // Verify filesystem creation
        let pane_root = temp_fs_dir.path().to_str().unwrap();
        let project_dir = std::path::PathBuf::from(pane_root).join("Projects").join(&request.name);
        assert!(project_dir.exists(), "Project directory should exist");
        assert!(project_dir.join("Assets").exists(), "Assets directory should exist");
        assert!(project_dir.join("Inbox").exists(), "First folder directory should exist");
        
        cleanup_test_env();
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_create_project_with_default_folder() {
        let (_temp_dir, db, temp_fs_dir) = setup_test_db();
        
        let request = CreateProjectRequest {
            name: "Test Project".to_string(),
            description: None,
            color: None,
            category: None,
            first_folder_name: None, // Should default to "General"
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        
        let result = create_project_internal(&request, &db);
        assert!(result.is_ok(), "Failed to create project: {:?}", result.err());
        
        let project = result.unwrap();
        
        // Verify folder was created with default name
        let conn = db.0.lock().unwrap();
        let folder_name: String = conn.query_row(
            "SELECT name FROM projects WHERE parent_id = ?1",
            params![project.id],
            |row| row.get(0)
        ).expect("Failed to query folder");
        
        assert_eq!(folder_name, "General");
        
        // Verify filesystem creation
        let pane_root = temp_fs_dir.path().to_str().unwrap();
        let project_dir = std::path::PathBuf::from(pane_root).join("Projects").join(&request.name);
        assert!(project_dir.exists(), "Project directory should exist");
        assert!(project_dir.join("Assets").exists(), "Assets directory should exist");
        assert!(project_dir.join("General").exists(), "First folder directory should exist");
        
        cleanup_test_env();
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_create_project_with_category() {
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();
        
        let request = CreateProjectRequest {
            name: "Test Project".to_string(),
            description: None,
            color: None,
            category: Some("Work".to_string()),
            first_folder_name: None,
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        
        let result = create_project_internal(&request, &db);
        assert!(result.is_ok(), "Failed to create project: {:?}", result.err());
        
        let project = result.unwrap();
        assert_eq!(project.category, Some("Work".to_string()));
        
        cleanup_test_env();
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_create_project_with_color_and_category() {
        let (_temp_dir, db, temp_fs_dir) = setup_test_db();
        
        let request = CreateProjectRequest {
            name: "Test Project".to_string(),
            description: None,
            color: Some("#FF5733".to_string()),
            category: Some("Personal".to_string()),
            first_folder_name: Some("Tasks".to_string()),
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        
        let result = create_project_internal(&request, &db);
        assert!(result.is_ok(), "Project creation should succeed");
        
        let project = result.unwrap();
        
        // Verify all fields were saved correctly
        assert_eq!(project.color, Some("#FF5733".to_string()));
        assert_eq!(project.category, Some("Personal".to_string()));
        
        // Verify filesystem creation
        let pane_root = temp_fs_dir.path().to_str().unwrap();
        let project_dir = std::path::PathBuf::from(pane_root).join("Projects").join(&request.name);
        assert!(project_dir.exists(), "Project directory should exist");
        assert!(project_dir.join("Assets").exists(), "Assets directory should exist");
        assert!(project_dir.join("Tasks").exists(), "First folder directory should exist");
        
        cleanup_test_env();
    }
    
    // === is_favorite Tests (#2009 — DB-backed favorites) ===

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_create_project_with_is_favorite_true_persists() {
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();

        let request = CreateProjectRequest {
            name: "Fav Project".to_string(),
            description: None,
            color: None,
            category: None,
            first_folder_name: None,
            is_favorite: Some(true),
        agent_id: "test-agent".to_string(),
        };

        let project = create_project_internal(&request, &db).unwrap();

        // Returned struct should have is_favorite = true
        assert!(project.is_favorite, "create_project with is_favorite=true should return project.is_favorite=true");

        // DB row should persist is_favorite = 1
        let conn = db.0.lock().unwrap();
        let is_fav: i32 = conn.query_row(
            "SELECT is_favorite FROM projects WHERE id = ?1 AND parent_id IS NULL",
            rusqlite::params![project.id],
            |row| row.get(0),
        ).expect("Should find project row");
        assert_eq!(is_fav, 1, "is_favorite column should be 1 in DB");

        cleanup_test_env();
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_create_project_with_is_favorite_false_default() {
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();

        let request = CreateProjectRequest {
            name: "Unfav Project".to_string(),
            description: None,
            color: None,
            category: None,
            first_folder_name: None,
            is_favorite: Some(false),
        agent_id: "test-agent".to_string(),
        };

        let project = create_project_internal(&request, &db).unwrap();

        assert!(!project.is_favorite, "create_project with is_favorite=false should return is_favorite=false");

        let conn = db.0.lock().unwrap();
        let is_fav: i32 = conn.query_row(
            "SELECT is_favorite FROM projects WHERE id = ?1 AND parent_id IS NULL",
            rusqlite::params![project.id],
            |row| row.get(0),
        ).expect("Should find project row");
        assert_eq!(is_fav, 0, "is_favorite column should be 0 in DB");

        cleanup_test_env();
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_is_favorite_round_trip_toggle() {
        // Tests: favorite on → DB = 1 → favorite off → DB = 0
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();

        let request = CreateProjectRequest {
            name: "Toggle Project".to_string(),
            description: None,
            color: None,
            category: None,
            first_folder_name: None,
            is_favorite: Some(false),
        agent_id: "test-agent".to_string(),
        };

        let project = create_project_internal(&request, &db).unwrap();
        assert!(!project.is_favorite, "Should start unfavorited");

        let conn = db.0.lock().unwrap();

        // Toggle ON
        conn.execute(
            "UPDATE projects SET is_favorite = 1, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![project.id],
        ).unwrap();

        let is_fav_on: i32 = conn.query_row(
            "SELECT is_favorite FROM projects WHERE id = ?1",
            rusqlite::params![project.id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(is_fav_on, 1, "After favorite-on: DB should be 1");

        // Toggle OFF
        conn.execute(
            "UPDATE projects SET is_favorite = 0, updated_at = datetime('now') WHERE id = ?1",
            rusqlite::params![project.id],
        ).unwrap();

        let is_fav_off: i32 = conn.query_row(
            "SELECT is_favorite FROM projects WHERE id = ?1",
            rusqlite::params![project.id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(is_fav_off, 0, "After favorite-off: DB should be 0");

        cleanup_test_env();
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_execute_star_project_sets_is_favorite_in_db() {
        // Tests execute_star_project directly: parameter parsing, name→ID lookup, unstar routing
        use crate::ai::tools::execute_star_project;
        use serde_json::json;

        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();

        let request = CreateProjectRequest {
            name: "Star Me".to_string(),
            description: None,
            color: None,
            category: None,
            first_folder_name: None,
            is_favorite: Some(false),
        agent_id: "test-agent".to_string(),
        };

        let project = create_project_internal(&request, &db).unwrap();
        let project_id = project.id.clone();

        // Star by project_id (UUID path)
        let star_params = json!({ "project_id": project_id, "unstar": false });
        let star_result = execute_star_project(&star_params, &db);
        assert!(star_result.is_ok(), "execute_star_project star should succeed: {:?}", star_result.err());

        let is_fav: i32 = db.0.lock().unwrap().query_row(
            "SELECT is_favorite FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(is_fav, 1, "execute_star_project: is_favorite should be 1 after star");

        // Unstar by project name (name→ID lookup path)
        let unstar_params = json!({ "project_id": "Star Me", "unstar": true });
        let unstar_result = execute_star_project(&unstar_params, &db);
        assert!(unstar_result.is_ok(), "execute_star_project unstar should succeed: {:?}", unstar_result.err());

        let is_fav_after: i32 = db.0.lock().unwrap().query_row(
            "SELECT is_favorite FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(is_fav_after, 0, "execute_star_project: is_favorite should be 0 after unstar");

        cleanup_test_env();
    }

    // === Reference File Tests (§412) — DISABLED (replaced by context_pins) ===
    
    /* DISABLED - replaced by context_pins (#471)
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_create_reference_file() {
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();
        let conn = db.0.lock().unwrap();
        
        // Create a test project
        let project_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, owner_id, created_at, updated_at) VALUES (?1, 'TestProject', 'user_local', ?2, ?3)",
            params![project_id, now, now],
        ).unwrap();
        drop(conn);
        
        // Create reference file
        let request = CreateReferenceFileRequest {
            project_id: project_id.clone(),
            title: "API Documentation".to_string(),
            content: "This is the API documentation for our project. It contains all endpoints and examples.".to_string(),
        };
        
        let result = crate::reference_files::create_reference_file_internal(&request, &db);
        assert!(result.is_ok(), "Failed to create reference file: {:?}", result.err());
        
        let ref_file = result.unwrap();
        assert_eq!(ref_file.title, "API Documentation");
        assert!(ref_file.content.contains("API documentation"));
        assert_eq!(ref_file.content_preview.len(), 86); // Content is 86 chars (< 200, so not truncated)
        
        // Verify flat file was created
        let pane_root = std::env::var("PANE_TEST_ROOT").unwrap();
        let expected_path = std::path::PathBuf::from(&pane_root).join(&ref_file.file_path);
        assert!(expected_path.exists(), "Flat file should exist at: {}", expected_path.display());
        
        let file_content = fs::read_to_string(&expected_path).unwrap();
        assert_eq!(file_content, request.content);
        
        // Verify in database (should have content_preview, NOT content column)
        let conn = db.0.lock().unwrap();
        let (note_type, content_preview, file_path): (String, String, String) = conn.query_row(
            "SELECT note_type, content_preview, file_path FROM notes WHERE id = ?1",
            params![ref_file.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        ).unwrap();
        assert_eq!(note_type, "reference");
        assert_eq!(content_preview, ref_file.content_preview);
        assert!(!file_path.is_empty());
        
        cleanup_test_env();
    }
    
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_list_reference_files() {
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();
        let conn = db.0.lock().unwrap();
        
        // Create a test project
        let project_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, owner_id, created_at, updated_at) VALUES (?1, 'TestProject', 'user_local', ?2, ?3)",
            params![project_id, now, now],
        ).unwrap();
        drop(conn);
        
        // Create two reference files
        let request1 = CreateReferenceFileRequest {
            project_id: project_id.clone(),
            title: "Architecture Notes".to_string(),
            content: "System architecture details".to_string(),
        };
        crate::reference_files::create_reference_file_internal(&request1, &db).unwrap();
        
        let request2 = CreateReferenceFileRequest {
            project_id: project_id.clone(),
            title: "API Docs".to_string(),
            content: "API endpoint documentation".to_string(),
        };
        crate::reference_files::create_reference_file_internal(&request2, &db).unwrap();
        
        // List reference files
        let result = crate::reference_files::list_reference_files_internal(&project_id, &db);
        assert!(result.is_ok());
        
        let files = result.unwrap();
        assert_eq!(files.len(), 2);
        
        // Verify alphabetical order
        assert_eq!(files[0].title, "API Docs");
        assert_eq!(files[1].title, "Architecture Notes");
        
        // Verify content is NOT loaded (should be empty string)
        assert_eq!(files[0].content, "");
        assert_eq!(files[1].content, "");
        
        // But content_preview should be populated
        assert!(!files[0].content_preview.is_empty());
        assert!(!files[1].content_preview.is_empty());
        
        cleanup_test_env();
    }
    
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_get_reference_file() {
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();
        let conn = db.0.lock().unwrap();
        
        // Create a test project
        let project_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, owner_id, created_at, updated_at) VALUES (?1, 'TestProject', 'user_local', ?2, ?3)",
            params![project_id, now, now],
        ).unwrap();
        drop(conn);
        
        // Create reference file
        let request = CreateReferenceFileRequest {
            project_id: project_id.clone(),
            title: "Test Doc".to_string(),
            content: "Full content that should be loaded from flat file.".to_string(),
        };
        let created = crate::reference_files::create_reference_file_internal(&request, &db).unwrap();
        
        // Get reference file (should read from flat file)
        let result = crate::reference_files::get_reference_file_internal(&created.id, &db);
        assert!(result.is_ok(), "Failed to get reference file: {:?}", result.err());
        
        let ref_file = result.unwrap();
        assert_eq!(ref_file.id, created.id);
        assert_eq!(ref_file.title, "Test Doc");
        assert_eq!(ref_file.content, request.content);
        assert!(!ref_file.content_preview.is_empty());
        
        cleanup_test_env();
    }
    
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_update_reference_file_content() {
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();
        let conn = db.0.lock().unwrap();
        
        // Create a test project
        let project_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, owner_id, created_at, updated_at) VALUES (?1, 'TestProject', 'user_local', ?2, ?3)",
            params![project_id, now, now],
        ).unwrap();
        drop(conn);
        
        // Create reference file
        let create_req = CreateReferenceFileRequest {
            project_id: project_id.clone(),
            title: "Original Title".to_string(),
            content: "Original content".to_string(),
        };
        let ref_file = crate::reference_files::create_reference_file_internal(&create_req, &db).unwrap();
        let original_file_path = ref_file.file_path.clone();
        
        // Update content only
        let update_req = UpdateReferenceFileRequest {
            id: ref_file.id.clone(),
            title: None,
            content: Some("Updated content with new text".to_string()),
        };
        let result = crate::reference_files::update_reference_file_internal(&update_req, &db);
        assert!(result.is_ok(), "Update failed: {:?}", result.err());
        
        // Verify content was written to flat file
        let pane_root = std::env::var("PANE_TEST_ROOT").unwrap();
        let file_path = std::path::PathBuf::from(&pane_root).join(&original_file_path);
        let file_content = fs::read_to_string(&file_path).unwrap();
        assert_eq!(file_content, "Updated content with new text");
        
        // Verify content_preview was updated in DB
        let conn = db.0.lock().unwrap();
        let (content_preview, word_count): (String, i64) = conn.query_row(
            "SELECT content_preview, word_count FROM notes WHERE id = ?1",
            params![ref_file.id],
            |row| Ok((row.get(0)?, row.get(1)?))
        ).unwrap();
        assert!(content_preview.starts_with("Updated content"));
        assert_eq!(word_count, 5); // "Updated content with new text" = 5 words
        
        cleanup_test_env();
    }
    
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_update_reference_file_title() {
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();
        let conn = db.0.lock().unwrap();
        
        // Create a test project
        let project_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, owner_id, created_at, updated_at) VALUES (?1, 'TestProject', 'user_local', ?2, ?3)",
            params![project_id, now, now],
        ).unwrap();
        drop(conn);
        
        // Create reference file
        let create_req = CreateReferenceFileRequest {
            project_id: project_id.clone(),
            title: "Original Title".to_string(),
            content: "Some content".to_string(),
        };
        let ref_file = crate::reference_files::create_reference_file_internal(&create_req, &db).unwrap();
        let original_file_path = ref_file.file_path.clone();
        
        // Update title (should rename file)
        let update_req = UpdateReferenceFileRequest {
            id: ref_file.id.clone(),
            title: Some("New Title".to_string()),
            content: None,
        };
        let result = crate::reference_files::update_reference_file_internal(&update_req, &db);
        assert!(result.is_ok(), "Update failed: {:?}", result.err());
        
        // Verify old file no longer exists
        let pane_root = std::env::var("PANE_TEST_ROOT").unwrap();
        let old_file_path = std::path::PathBuf::from(&pane_root).join(&original_file_path);
        assert!(!old_file_path.exists(), "Old file should have been renamed");
        
        // Verify new file exists with new slug
        let conn = db.0.lock().unwrap();
        let (title, new_file_path): (String, String) = conn.query_row(
            "SELECT title, file_path FROM notes WHERE id = ?1",
            params![ref_file.id],
            |row| Ok((row.get(0)?, row.get(1)?))
        ).unwrap();
        assert_eq!(title, "New Title");
        assert!(new_file_path.contains("new-title"));
        
        let new_full_path = std::path::PathBuf::from(&pane_root).join(&new_file_path);
        assert!(new_full_path.exists(), "New file should exist");
        
        let file_content = fs::read_to_string(&new_full_path).unwrap();
        assert_eq!(file_content, "Some content");
        
        cleanup_test_env();
    }
    
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_delete_reference_file() {
        let (_temp_dir, db, _temp_fs_dir) = setup_test_db();
        let conn = db.0.lock().unwrap();
        
        // Create a test project
        let project_id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, owner_id, created_at, updated_at) VALUES (?1, 'TestProject', 'user_local', ?2, ?3)",
            params![project_id, now, now],
        ).unwrap();
        drop(conn);
        
        // Create reference file
        let create_req = CreateReferenceFileRequest {
            project_id: project_id.clone(),
            title: "To Be Deleted".to_string(),
            content: "This will be deleted".to_string(),
        };
        let ref_file = crate::reference_files::create_reference_file_internal(&create_req, &db).unwrap();
        let file_path = ref_file.file_path.clone();
        
        // Verify file exists before deletion
        let pane_root = std::env::var("PANE_TEST_ROOT").unwrap();
        let full_path = std::path::PathBuf::from(&pane_root).join(&file_path);
        assert!(full_path.exists(), "File should exist before deletion");
        
        // Delete it
        let result = crate::reference_files::delete_reference_file_internal(&ref_file.id, &db);
        assert!(result.is_ok());
        
        // Verify soft delete in DB
        let conn = db.0.lock().unwrap();
        let deleted_at: Option<String> = conn.query_row(
            "SELECT deleted_at FROM notes WHERE id = ?1",
            params![ref_file.id],
            |row| row.get(0)
        ).unwrap();
        assert!(deleted_at.is_some(), "deleted_at should be set");
        
        // Verify flat file was deleted
        assert!(!full_path.exists(), "Flat file should be deleted");
        
        // Verify it doesn't appear in list
        drop(conn);
        let files = crate::reference_files::list_reference_files_internal(&project_id, &db).unwrap();
        assert_eq!(files.len(), 0, "Deleted file should not appear in list");
        
        cleanup_test_env();
    }
    */
}
