//! Unit tests for Pane core functionality
//! Tests the note CRUD operations, indexing, and file system operations
//! to ensure the content mixing bug stays fixed.

mod test_atomic_write_origin;
mod test_pull_pending_changes_blocking;
mod test_sync_now;
// test_auth_integration deleted per ADR-0070 Decision 4 (#3195 PR C): its
// remaining 7 sign_in tests asserted the ecom wrapper's /v1/auth/signin
// path, response shape, and install_uuid/machine_name params — all removed
// by the GoTrue re-point. Equivalent coverage (success + 400 + 429 + network
// cases) now lives in sync/auth.rs's own #[cfg(test)] mod tests, co-located
// with sign_in_with_url the same way check_entitlement/take_over's tests
// used to be. The file's email_check tests were already deleted in PR B.
mod test_auth_session;
mod test_backup;
mod test_conflict_resolution;
mod test_context_pins;
mod test_fts_search;
mod test_onboarding;
mod test_projects;
mod test_reconciliation;
// test_same_machine_no_bridge deleted per #1108 (SameMachine topology scrapped)
mod test_topology;
mod memory_pipeline_tests;
mod test_identity_sot;
mod test_sessions;
mod test_rekey;
mod test_context_assembly_2156;

// NOTE: Legacy tests below use functions that were refactored/removed.
// The old tests (test_create_note, test_save_and_read_note, etc.) reference
// NoteMeta, slugify, calculate_word_count, calculate_reading_time, sync_note_to_index_raw
// which no longer exist in their original form. These tests are disabled until
// they can be updated to use the current API.
// TODO: Update or remove legacy tests (#375)

/*
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use uuid::Uuid;

use crate::{NoteMeta, slugify, calculate_word_count, calculate_reading_time, sync_note_to_index_raw};

/// Create a temporary test vault directory
fn create_test_vault() -> TempDir {
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    
    // Create notebooks structure
    let notebooks_path = temp_dir.path().join("notebooks");
    fs::create_dir_all(&notebooks_path).expect("Failed to create notebooks directory");
    
    // Create default test notebook
    let test_notebook = notebooks_path.join("TestNotebook");
    fs::create_dir_all(&test_notebook).expect("Failed to create test notebook");
    
    // Create .pane directory for metadata
    let pane_dir = temp_dir.path().join(".pane");
    fs::create_dir_all(&pane_dir).expect("Failed to create .pane directory");
    
    temp_dir
}

/// Initialize test database with custom path
fn init_test_db(vault_path: &PathBuf) -> std::sync::Mutex<rusqlite::Connection> {
    let db_path = vault_path.join(".pane").join("test.db");
    let conn = rusqlite::Connection::open(&db_path)
        .expect("Failed to open test database");
    
    // Run basic schema creation (without cr-sqlite for tests)
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS notes (
            id          TEXT PRIMARY KEY,
            title       TEXT NOT NULL DEFAULT '',
            notebook    TEXT NOT NULL DEFAULT '',
            content_preview TEXT NOT NULL DEFAULT '',
            tags_json   TEXT NOT NULL DEFAULT '[]',
            word_count  INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT '',
            updated_at  TEXT NOT NULL DEFAULT '',
            file_path   TEXT NOT NULL DEFAULT '',
            project_id  TEXT,
            folder_id   TEXT
        );

        CREATE TABLE IF NOT EXISTS projects (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            description TEXT,
            color       TEXT,
            parent_id   TEXT,
            sort_order  INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_notes_notebook ON notes(notebook);
        CREATE INDEX IF NOT EXISTS idx_notes_updated  ON notes(updated_at);
        "
    ).expect("Failed to create test schema");
    
    std::sync::Mutex::new(conn)
}

/// Create a test note and return its content and file path
fn create_test_note_direct(vault_path: &PathBuf, notebook: &str, title: &str, content: &str) -> (String, PathBuf) {
    let slug = slugify(title);
    let notebook_path = vault_path.join("notebooks").join(notebook);
    fs::create_dir_all(&notebook_path).expect("Failed to create notebook directory");
    
    let file_path = notebook_path.join(format!("{}.md", slug));
    let full_content = if content.is_empty() { String::from("\n") } else { format!("{}\n", content) };
    fs::write(&file_path, &full_content).expect("Failed to write test note");
    
    (slug, file_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use rusqlite::OptionalExtension;

    #[test]
    fn test_create_note() {
        let temp_vault = create_test_vault();
        let vault_path = temp_vault.path().to_path_buf();
        let _db = init_test_db(&vault_path);

        let notebook = "TestNotebook";
        let title = "Test Note Title";
        
        // Simulate create_note logic
        let slug = if title.is_empty() {
            format!("untitled-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"))
        } else {
            slugify(title)
        };
        
        let display_title = if title.is_empty() { "Untitled".to_string() } else { title.to_string() };
        let content = String::from("\n");
        let now = chrono::Utc::now().to_rfc3339();
        let note_id = format!("{}/{}", notebook, slug);
        
        // Verify slug generation
        assert!(!slug.is_empty(), "Slug should not be empty");
        assert_eq!(slug, "test-note-title", "Slug should be properly formatted");
        
        // Verify note ID format
        assert_eq!(note_id, "TestNotebook/test-note-title", "Note ID should follow notebook/slug format");
        
        // Create the note on filesystem
        let notebook_path = vault_path.join("notebooks").join(notebook);
        fs::create_dir_all(&notebook_path).expect("Failed to create notebook directory");
        let file_path = notebook_path.join(format!("{}.md", slug));
        fs::write(&file_path, &content).expect("Failed to write note file");
        
        // Verify file exists on disk
        assert!(file_path.exists(), "Note file should exist on disk");
        
        // Verify file content
        let written_content = fs::read_to_string(&file_path).expect("Failed to read note file");
        assert_eq!(written_content, content, "File content should match expected content");
        
        println!("✓ test_create_note passed: slug={}, id={}, file exists", slug, note_id);
    }

    #[test]
    fn test_save_and_read_note() {
        let temp_vault = create_test_vault();
        let vault_path = temp_vault.path().to_path_buf();
        
        let notebook = "TestNotebook";
        let title = "Save Read Test";
        let original_content = "This is test content.\n";
        let updated_content = "This is updated test content with more details.\n";
        
        // Create initial note
        let (slug, file_path) = create_test_note_direct(&vault_path, notebook, title, "This is test content.");
        
        // Verify initial read
        let read_content = fs::read_to_string(&file_path).expect("Failed to read initial note");
        assert_eq!(read_content, original_content, "Initial content should match");
        
        // Update content
        fs::write(&file_path, updated_content).expect("Failed to update note");
        
        // Verify updated read
        let updated_read = fs::read_to_string(&file_path).expect("Failed to read updated note");
        assert_eq!(updated_read, updated_content, "Updated content should match");
        
        // Verify content persistence
        let slug2 = slugify("Another Note");
        let file_path2 = vault_path.join("notebooks").join(notebook).join(format!("{}.md", slug2));
        fs::write(&file_path2, "Different content\n").expect("Failed to write second note");
        
        // Verify first note still has correct content
        let final_read = fs::read_to_string(&file_path).expect("Failed to read original note after creating second");
        assert_eq!(final_read, updated_content, "Original note content should be preserved");
        
        println!("✓ test_save_and_read_note passed: content persists correctly");
    }

    #[test]
    fn test_save_and_read_json() {
        let temp_vault = create_test_vault();
        let vault_path = temp_vault.path().to_path_buf();
        
        let notebook = "TestNotebook";
        let title = "JSON Test";
        let slug = slugify(title);
        
        // Create markdown file
        let (_, md_path) = create_test_note_direct(&vault_path, notebook, title, "This is markdown content.");
        
        // Create JSON sidecar file
        let json_path = vault_path.join("notebooks").join(notebook).join(format!("{}.json", slug));
        let test_json = serde_json::json!({
            "tiptap": {
                "type": "doc",
                "content": [
                    {
                        "type": "heading",
                        "attrs": { "level": 1 },
                        "content": [{ "type": "text", "text": "JSON Test" }]
                    },
                    {
                        "type": "paragraph",
                        "content": [{ "type": "text", "text": "This is TipTap JSON content." }]
                    }
                ]
            },
            "lastModified": "2026-02-16T19:30:00.000Z"
        });
        
        fs::write(&json_path, serde_json::to_string_pretty(&test_json).unwrap())
            .expect("Failed to write JSON file");
        
        // Verify JSON file exists and content matches
        assert!(json_path.exists(), "JSON file should exist");
        
        let read_json: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&json_path).expect("Failed to read JSON file")
        ).expect("Failed to parse JSON");
        
        assert_eq!(read_json, test_json, "JSON content should match exactly");
        
        // Verify both files exist together
        assert!(md_path.exists(), "Markdown file should still exist");
        assert!(json_path.exists(), "JSON file should exist");
        
        println!("✓ test_save_and_read_json passed: JSON sidecar files work correctly");
    }

    #[test]
    fn test_sync_note_to_index() {
        let temp_vault = create_test_vault();
        let vault_path = temp_vault.path().to_path_buf();
        let db = init_test_db(&vault_path);
        
        let notebook = "TestNotebook";
        let title = "Index Test Note";
        let content = "This is content for indexing test.";
        let slug = slugify(title);
        
        // Create test note
        let (_, file_path) = create_test_note_direct(&vault_path, notebook, title, content);
        
        // Create note metadata
        let now = chrono::Utc::now().to_rfc3339();
        let note_id = format!("{}/{}", notebook, slug);
        let preview = "This is content for indexing test.".chars().take(120).collect::<String>();
        let word_count = calculate_word_count(content);
        let reading_time = calculate_reading_time(word_count);
        
        let meta = NoteMeta {
            id: note_id.clone(),
            title: title.to_string(),
            notebook: notebook.to_string(),
            slug: slug.clone(),
            created_at: now.clone(),
            updated_at: now,
            preview,
            pinned: Some(false),
            word_count: Some(word_count),
            reading_time: Some(reading_time),
            task_status: None,
            task_priority: None,
            task_due_date: None,
            task_assignee: None,
        };
        
        let full_content = format!("{}\n", content);
        
        // Sync to index
        sync_note_to_index_raw(&db, &meta, &full_content);
        
        // Verify note is in index
        {
            let conn = db.lock().unwrap();
            let mut stmt = conn.prepare("SELECT id, title, notebook FROM notes WHERE id = ?1")
                .expect("Failed to prepare query");
            
            let note_exists: Option<(String, String, String)> = stmt.query_row(
                rusqlite::params![&note_id],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?
                ))
            ).optional().expect("Failed to query note");
            
            assert!(note_exists.is_some(), "Note should be indexed in database");
            let (indexed_id, indexed_title, indexed_notebook) = note_exists.unwrap();
            assert_eq!(indexed_id, note_id, "Indexed ID should match");
            assert_eq!(indexed_title, title, "Indexed title should match");
            assert_eq!(indexed_notebook, notebook, "Indexed notebook should match");
        }
        
        println!("✓ test_sync_note_to_index passed: note properly indexed");
    }

    #[test]
    fn test_subfolder_path_parsing() {
        let temp_vault = create_test_vault();
        let vault_path = temp_vault.path().to_path_buf();
        let db = init_test_db(&vault_path);
        
        // Create test projects in database
        {
            let conn = db.lock().unwrap();
            let now = chrono::Utc::now().to_rfc3339();
            
            // Create parent project
            let parent_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO projects (id, name, parent_id, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, NULL, 0, ?3, ?4)",
                rusqlite::params![&parent_id, "ParentProject", &now, &now]
            ).expect("Failed to create parent project");
            
            // Create child folder
            let folder_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO projects (id, name, parent_id, sort_order, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5)",
                rusqlite::params![&folder_id, "ChildFolder", &parent_id, &now, &now]
            ).expect("Failed to create child folder");
        }
        
        // Test subfolder notebook path: "ParentProject/ChildFolder"
        let notebook = "ParentProject/ChildFolder";
        let title = "Subfolder Test Note";
        let content = "This note should be in a subfolder.";
        let slug = slugify(title);
        
        // Create notebook directory structure
        let notebook_path = vault_path.join("notebooks").join(notebook);
        fs::create_dir_all(&notebook_path).expect("Failed to create subfolder notebook");
        
        let file_path = notebook_path.join(format!("{}.md", slug));
        let full_content = format!("{}\n", content);
        fs::write(&file_path, &full_content).expect("Failed to write subfolder note");
        
        // Create metadata and sync
        let now = chrono::Utc::now().to_rfc3339();
        let note_id = format!("{}/{}", notebook, slug);
        
        let meta = NoteMeta {
            id: note_id.clone(),
            title: title.to_string(),
            notebook: notebook.to_string(),
            slug: slug.clone(),
            created_at: now.clone(),
            updated_at: now,
            preview: content.chars().take(120).collect(),
            pinned: Some(false),
            word_count: Some(calculate_word_count(&full_content)),
            reading_time: Some(calculate_reading_time(calculate_word_count(&full_content))),
            task_status: None,
            task_priority: None,
            task_due_date: None,
            task_assignee: None,
        };
        
        // Sync to index
        sync_note_to_index_raw(&db, &meta, &full_content);
        
        // Verify project_id and folder_id are set correctly
        {
            let conn = db.lock().unwrap();
            let result: Option<(String, Option<String>, Option<String>)> = conn.query_row(
                "SELECT id, project_id, folder_id FROM notes WHERE id = ?1",
                rusqlite::params![&note_id],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?
                ))
            ).optional().expect("Failed to query indexed note");
            
            assert!(result.is_some(), "Note should be indexed");
            let (indexed_id, project_id, folder_id) = result.unwrap();
            assert_eq!(indexed_id, note_id, "Note ID should match");
            // Note: project_id and folder_id may be None if projects aren't properly linked
            // The important thing is that the note gets indexed without errors
            println!("Project ID: {:?}, Folder ID: {:?}", project_id, folder_id);
        }
        
        println!("✓ test_subfolder_path_parsing passed: Parent/Child notebook correctly maps to project + folder");
    }

    #[test]
    fn test_move_to_trash() {
        let temp_vault = create_test_vault();
        let vault_path = temp_vault.path().to_path_buf();
        let db = init_test_db(&vault_path);
        
        let notebook = "TestNotebook";
        let title = "Trash Test Note";
        let content = "This note will be moved to trash.";
        let slug = slugify(title);
        let note_id = format!("{}/{}", notebook, slug);
        
        // Create and index the note
        let (_, file_path) = create_test_note_direct(&vault_path, notebook, title, content);
        
        let now = chrono::Utc::now().to_rfc3339();
        let meta = NoteMeta {
            id: note_id.clone(),
            title: title.to_string(),
            notebook: notebook.to_string(),
            slug: slug.clone(),
            created_at: now.clone(),
            updated_at: now,
            preview: content.chars().take(120).collect(),
            pinned: Some(false),
            word_count: Some(calculate_word_count(content)),
            reading_time: Some(1),
            task_status: None,
            task_priority: None,
            task_due_date: None,
            task_assignee: None,
        };
        
        let full_content = format!("{}\n", content);
        sync_note_to_index_raw(&db, &meta, &full_content);
        
        // Verify note is indexed
        {
            let conn = db.lock().unwrap();
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM notes WHERE id = ?1",
                rusqlite::params![&note_id],
                |row| row.get(0)
            ).expect("Failed to count notes");
            assert_eq!(count, 1, "Note should be in index before trash");
        }
        
        // Create trash directory
        let trash_path = vault_path.join(".pane").join("trash");
        fs::create_dir_all(&trash_path).expect("Failed to create trash directory");
        
        // Move to trash (simulate move_to_trash logic)
        let safe_trash_name = slug.replace('/', "_");
        let trash_file_path = trash_path.join(format!("{}_{}.md", notebook, safe_trash_name));
        
        // Move file
        if file_path.exists() {
            fs::copy(&file_path, &trash_file_path).expect("Failed to copy to trash");
            fs::remove_file(&file_path).expect("Failed to remove original");
        }
        
        // Remove from index
        {
            let conn = db.lock().unwrap();
            conn.execute(
                "DELETE FROM notes WHERE id = ?1",
                rusqlite::params![&note_id]
            ).expect("Failed to remove from index");
        }
        
        // Verify note is removed from index
        {
            let conn = db.lock().unwrap();
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM notes WHERE id = ?1",
                rusqlite::params![&note_id],
                |row| row.get(0)
            ).expect("Failed to count notes after trash");
            assert_eq!(count, 0, "Note should be removed from index");
        }
        
        // Verify file is in trash
        assert!(trash_file_path.exists(), "Note should be moved to trash directory");
        assert!(!file_path.exists(), "Original file should be removed");
        
        let trash_content = fs::read_to_string(&trash_file_path).expect("Failed to read trash file");
        assert_eq!(trash_content, full_content, "Trash file content should match original");
        
        println!("✓ test_move_to_trash passed: note removed from index and moved to trash");
    }

    #[test]
    fn test_create_note_background_write_guard() {
        let temp_vault = create_test_vault();
        let vault_path = temp_vault.path().to_path_buf();
        
        let notebook = "TestNotebook";
        let title = "Background Write Test";
        let slug = slugify(title);
        let file_path = vault_path.join("notebooks").join(notebook).join(format!("{}.md", slug));
        
        // First, create the file as if save_note() was called immediately
        let user_content = "User typed this content quickly.\n";
        fs::write(&file_path, user_content).expect("Failed to write user content");
        
        // Now simulate the background write from create_note() trying to write default content
        let background_content = "\n";
        
        // The guard: only write if file doesn't already exist
        if !file_path.exists() {
            fs::write(&file_path, background_content).expect("Failed to write background content");
        }
        
        // Verify the file still contains user content, not background content
        let final_content = fs::read_to_string(&file_path).expect("Failed to read final content");
        assert_eq!(final_content, user_content, "User content should be preserved");
        assert_ne!(final_content, background_content, "Background content should not overwrite user content");
        
        // Test the opposite case: background write should work when file doesn't exist
        let slug2 = "test-background-write-2";
        let file_path2 = vault_path.join("notebooks").join(notebook).join(format!("{}.md", slug2));
        
        // Ensure file doesn't exist
        assert!(!file_path2.exists(), "Second test file should not exist initially");
        
        // Background write should work
        if !file_path2.exists() {
            fs::write(&file_path2, background_content).expect("Failed to write background content to non-existing file");
        }
        
        let bg_written_content = fs::read_to_string(&file_path2).expect("Failed to read background written content");
        assert_eq!(bg_written_content, background_content, "Background content should be written when file doesn't exist");
        
        println!("✓ test_create_note_background_write_guard passed: background writes don't overwrite existing files");
    }
}*/
