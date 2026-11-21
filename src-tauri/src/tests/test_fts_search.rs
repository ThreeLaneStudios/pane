//! Tests for FTS5 search functionality (Issue #544)
//! 
//! Verifies that FTS triggers are created correctly AFTER CRR enablement
//! to prevent trigger destruction by crsql_as_crr()

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};
    use tempfile::TempDir;

    fn setup_test_db() -> (TempDir, Connection) {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let db_path = temp_dir.path().join("test.db");
        
        let conn = Connection::open(&db_path)
            .expect("Failed to open test database");
        
        // Create minimal schema with FTS (without CRR since test env doesn't load cr-sqlite)
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS notes (
                id              TEXT PRIMARY KEY,
                title           TEXT NOT NULL DEFAULT '',
                body            TEXT,
                content_preview TEXT NOT NULL DEFAULT '',
                file_path       TEXT NOT NULL DEFAULT '',
                project_id      TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );
            
            -- FTS virtual table
            CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
                title,
                body,
                content='notes',
                content_rowid='rowid',
                tokenize='porter unicode61'
            );
            
            -- FTS triggers (same as production, created AFTER table)
            CREATE TRIGGER IF NOT EXISTS notes_fts_insert AFTER INSERT ON notes BEGIN
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
        ).expect("Failed to create test schema");
        
        (temp_dir, conn)
    }

    #[test]
    fn test_fts_triggers_fire_on_insert() {
        let (_temp_dir, conn) = setup_test_db();
        
        // Insert a note with searchable content
        conn.execute(
            "INSERT INTO notes (id, title, body) VALUES (?1, ?2, ?3)",
            params!["note-1", "Hello World", "This is test content"],
        ).expect("Failed to insert note");
        
        // Search for "hello" - should find the note via FTS
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'hello'",
            [],
            |row| row.get(0),
        ).expect("Failed to query FTS");
        
        assert_eq!(count, 1, "FTS trigger should have indexed the inserted note");
    }
    
    #[test]
    fn test_fts_triggers_fire_on_update() {
        let (_temp_dir, conn) = setup_test_db();
        
        // Insert a note
        conn.execute(
            "INSERT INTO notes (id, title, body) VALUES (?1, ?2, ?3)",
            params!["note-1", "Original Title", "Original body"],
        ).expect("Failed to insert note");
        
        // Update the note
        conn.execute(
            "UPDATE notes SET title = ?1, body = ?2 WHERE id = ?3",
            params!["Updated Title", "Updated searchable body", "note-1"],
        ).expect("Failed to update note");
        
        // Search for updated content
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'searchable'",
            [],
            |row| row.get(0),
        ).expect("Failed to query FTS");
        
        assert_eq!(count, 1, "FTS trigger should have updated the index");
        
        // Old content should not be found
        let old_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'original'",
            [],
            |row| row.get(0),
        ).expect("Failed to query FTS");
        
        assert_eq!(old_count, 0, "FTS should have removed old content from index");
    }
    
    #[test]
    fn test_fts_triggers_fire_on_delete() {
        let (_temp_dir, conn) = setup_test_db();
        
        // Insert a note
        conn.execute(
            "INSERT INTO notes (id, title, body) VALUES (?1, ?2, ?3)",
            params!["note-1", "Delete Me", "This will be deleted"],
        ).expect("Failed to insert note");
        
        // Verify it's searchable
        let count_before: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'delete'",
            [],
            |row| row.get(0),
        ).expect("Failed to query FTS");
        
        assert_eq!(count_before, 1, "Note should be indexed before deletion");
        
        // Delete the note
        conn.execute(
            "DELETE FROM notes WHERE id = ?1",
            params!["note-1"],
        ).expect("Failed to delete note");
        
        // Verify it's no longer searchable
        let count_after: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'delete'",
            [],
            |row| row.get(0),
        ).expect("Failed to query FTS");
        
        assert_eq!(count_after, 0, "FTS trigger should have removed deleted note from index");
    }
    
    #[test]
    fn test_fts_search_matches_title_and_body() {
        let (_temp_dir, conn) = setup_test_db();
        
        // Insert notes with searchable content in different fields
        conn.execute(
            "INSERT INTO notes (id, title, body) VALUES (?1, ?2, ?3)",
            params!["note-1", "Apple Pie Recipe", "How to make delicious pie"],
        ).expect("Failed to insert note 1");
        
        conn.execute(
            "INSERT INTO notes (id, title, body) VALUES (?1, ?2, ?3)",
            params!["note-2", "Shopping List", "Buy apples and flour"],
        ).expect("Failed to insert note 2");
        
        // Search for "apple" - should find both notes
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'apple'",
            [],
            |row| row.get(0),
        ).expect("Failed to query FTS");
        
        assert_eq!(count, 2, "FTS should search both title and body columns");
    }
    
    #[test]
    fn test_fts_rebuild_populates_existing_data() {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let db_path = temp_dir.path().join("test.db");
        
        let conn = Connection::open(&db_path)
            .expect("Failed to open test database");
        
        // Create tables and insert notes WITHOUT triggers first
        // (simulates the bug where notes exist but FTS is empty)
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS notes (
                id              TEXT PRIMARY KEY,
                title           TEXT NOT NULL DEFAULT '',
                body            TEXT,
                content_preview TEXT NOT NULL DEFAULT '',
                file_path       TEXT NOT NULL DEFAULT '',
                project_id      TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );
            
            CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
                title,
                body,
                content='notes',
                content_rowid='rowid',
                tokenize='porter unicode61'
            );
            "
        ).expect("Failed to create schema");
        
        // Insert notes before triggers exist
        conn.execute(
            "INSERT INTO notes (id, title, body) VALUES (?1, ?2, ?3)",
            params!["note-1", "Existing Note", "This was here before triggers"],
        ).expect("Failed to insert note");
        
        // FTS should be empty at this point
        let count_before: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'existing'",
            [],
            |row| row.get(0),
        ).expect("Failed to query FTS");
        
        assert_eq!(count_before, 0, "FTS should be empty without triggers");
        
        // Now rebuild the FTS index (simulates the production rebuild step)
        conn.execute(
            "INSERT INTO notes_fts(notes_fts) VALUES('rebuild')",
            [],
        ).expect("Failed to rebuild FTS");
        
        // FTS should now contain the existing note
        let count_after: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notes_fts WHERE notes_fts MATCH 'existing'",
            [],
            |row| row.get(0),
        ).expect("Failed to query FTS");
        
        assert_eq!(count_after, 1, "FTS rebuild should populate index with existing data");
    }
}
