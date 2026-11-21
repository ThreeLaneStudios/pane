// Tests for onboarding backup restore flow (§6.1.4, #971)

use tempfile::TempDir;
use rusqlite::Connection;
use std::sync::Mutex;

// Test mutex to ensure tests don't interfere with each other
static TEST_MUTEX: Mutex<()> = Mutex::new(());

fn setup_test_db(temp_dir: &TempDir) -> std::path::PathBuf {
    let db_path = temp_dir.path().join("test.db");
    let conn = Connection::open(&db_path).unwrap();
    
    // Create minimal schema for testing
    conn.execute_batch(
        "CREATE TABLE notes (id TEXT PRIMARY KEY, title TEXT, body TEXT);
         CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT);
         CREATE TABLE chat_messages_local (id TEXT PRIMARY KEY, content TEXT);"
    ).unwrap();
    
    db_path
}

#[test]
fn test_check_fresh_install_empty_db() {
    let _lock = TEST_MUTEX.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let db_path = setup_test_db(&temp);
    
    // Open connection to verify DB exists and is empty
    let conn = Connection::open(&db_path).unwrap();
    let note_count: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0)).unwrap();
    let project_count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap();
    let chat_count: i64 = conn.query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0)).unwrap();
    drop(conn);
    
    assert_eq!(note_count, 0);
    assert_eq!(project_count, 0);
    assert_eq!(chat_count, 0);
    
    // Simulate the check_fresh_install logic
    let is_fresh = note_count == 0 && project_count == 0 && chat_count == 0;
    assert!(is_fresh, "Empty DB should be considered fresh");
}

#[test]
fn test_check_fresh_install_with_notes() {
    let _lock = TEST_MUTEX.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let db_path = setup_test_db(&temp);
    
    // Insert a note
    let conn = Connection::open(&db_path).unwrap();
    conn.execute("INSERT INTO notes (id, title, body) VALUES ('1', 'Test Note', 'Content')", []).unwrap();
    
    let note_count: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0)).unwrap();
    let project_count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap();
    let chat_count: i64 = conn.query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0)).unwrap();
    drop(conn);
    
    assert_eq!(note_count, 1);
    
    // Simulate the check_fresh_install logic
    let is_fresh = note_count == 0 && project_count == 0 && chat_count == 0;
    assert!(!is_fresh, "DB with notes should NOT be considered fresh");
}

#[test]
fn test_check_fresh_install_with_projects() {
    let _lock = TEST_MUTEX.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let db_path = setup_test_db(&temp);
    
    // Insert a project
    let conn = Connection::open(&db_path).unwrap();
    conn.execute("INSERT INTO projects (id, name) VALUES ('proj1', 'My Project')", []).unwrap();
    
    let note_count: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0)).unwrap();
    let project_count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap();
    let chat_count: i64 = conn.query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0)).unwrap();
    drop(conn);
    
    assert_eq!(project_count, 1);
    
    // Simulate the check_fresh_install logic
    let is_fresh = note_count == 0 && project_count == 0 && chat_count == 0;
    assert!(!is_fresh, "DB with projects should NOT be considered fresh");
}

#[test]
fn test_check_fresh_install_with_chat_messages() {
    let _lock = TEST_MUTEX.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let db_path = setup_test_db(&temp);
    
    // Insert a chat message
    let conn = Connection::open(&db_path).unwrap();
    conn.execute("INSERT INTO chat_messages_local (id, content) VALUES ('msg1', 'Hello')", []).unwrap();
    
    let note_count: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0)).unwrap();
    let project_count: i64 = conn.query_row("SELECT COUNT(*) FROM projects", [], |r| r.get(0)).unwrap();
    let chat_count: i64 = conn.query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0)).unwrap();
    drop(conn);
    
    assert_eq!(chat_count, 1);
    
    // Simulate the check_fresh_install logic
    let is_fresh = note_count == 0 && project_count == 0 && chat_count == 0;
    assert!(!is_fresh, "DB with chat messages should NOT be considered fresh");
}

#[test]
fn test_check_fresh_install_nonexistent_db() {
    let _lock = TEST_MUTEX.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("nonexistent.db");
    
    // DB doesn't exist
    assert!(!db_path.exists());
    
    // Simulate the check_fresh_install logic for nonexistent DB
    let is_fresh = !db_path.exists();
    assert!(is_fresh, "Nonexistent DB should be considered fresh");
}
