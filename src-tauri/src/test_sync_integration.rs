#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};
    use crate::sync::SyncChange;
    use tempfile::tempdir;

    fn load_crsqlite_extension(conn: &Connection) -> Result<(), String> {
        unsafe {
            conn.load_extension_enable()
                .map_err(|e| format!("Failed to enable extension loading: {}", e))?;

            // Path relative to where cargo test runs (src-tauri/)
            conn.load_extension("resources/crsqlite.dylib", Some("sqlite3_crsqlite_init"))
                .map_err(|e| format!("Failed to load cr-sqlite extension: {}", e))?;
        }

        conn.load_extension_disable()
            .map_err(|e| format!("Failed to disable extension loading: {}", e))?;

        Ok(())
    }

    fn init_test_database(db_path: &str) -> Result<Connection, String> {
        let conn = Connection::open(db_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        // Load cr-sqlite extension
        load_crsqlite_extension(&conn)?;

        // Create schema (same as database.rs)
        conn.execute_batch(
            "
            CREATE TABLE notes (
                id          TEXT PRIMARY KEY NOT NULL,
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

            CREATE TABLE tasks (
                id              TEXT PRIMARY KEY NOT NULL,
                title           TEXT NOT NULL DEFAULT '',
                description     TEXT,
                status          TEXT NOT NULL DEFAULT 'todo',
                priority        TEXT NOT NULL DEFAULT 'medium',
                due_date        TEXT,
                project         TEXT,
                project_id      TEXT,
                folder_id       TEXT,
                tags_json       TEXT NOT NULL DEFAULT '[]',
                assignee        TEXT,
                parent_task_id  TEXT,
                sort_order      INTEGER NOT NULL DEFAULT 0,
                created_at      TEXT NOT NULL DEFAULT '',
                updated_at      TEXT NOT NULL DEFAULT '',
                completed_at    TEXT
            );

            CREATE TABLE projects (
                id          TEXT PRIMARY KEY NOT NULL,
                name        TEXT NOT NULL DEFAULT '',
                description TEXT,
                color       TEXT,
                parent_id   TEXT,
                sort_order  INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT NOT NULL DEFAULT '',
                updated_at  TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE users (
                id          TEXT PRIMARY KEY NOT NULL,
                name        TEXT NOT NULL DEFAULT '',
                email       TEXT,
                avatar_url  TEXT,
                created_at  TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE test_sync (
                id TEXT PRIMARY KEY NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                content TEXT,
                updated_at TEXT NOT NULL DEFAULT ''
            );
            "
        ).map_err(|e| format!("Failed to create tables: {}", e))?;

        // Mark tables as CRRs (these return results, so use query_row)
        conn.query_row("SELECT crsql_as_crr('notes')", [], |_| Ok(()))
            .map_err(|e| format!("Failed to mark notes as CRR: {}", e))?;
        conn.query_row("SELECT crsql_as_crr('tasks')", [], |_| Ok(()))
            .map_err(|e| format!("Failed to mark tasks as CRR: {}", e))?;
        conn.query_row("SELECT crsql_as_crr('projects')", [], |_| Ok(()))
            .map_err(|e| format!("Failed to mark projects as CRR: {}", e))?;
        conn.query_row("SELECT crsql_as_crr('users')", [], |_| Ok(()))
            .map_err(|e| format!("Failed to mark users as CRR: {}", e))?;
        conn.query_row("SELECT crsql_as_crr('test_sync')", [], |_| Ok(()))
            .map_err(|e| format!("Failed to mark test_sync as CRR: {}", e))?;

        Ok(conn)
    }

    fn extract_changes(conn: &Connection, since_version: i64) -> Result<Vec<SyncChange>, String> {
        let mut stmt = conn.prepare(
            "SELECT \"table\", pk, cid, val, col_version, db_version, site_id, cl, seq 
             FROM crsql_changes WHERE db_version > ?"
        ).map_err(|e| format!("Failed to prepare changes query: {}", e))?;

        let changes = stmt.query_map([since_version], |row| {
            // Handle the pk column which might be stored as a blob in cr-sqlite
            let pk_value: rusqlite::types::Value = row.get(1)?;
            let pk = match pk_value {
                rusqlite::types::Value::Text(s) => s,
                rusqlite::types::Value::Blob(b) => {
                    String::from_utf8(b.clone()).unwrap_or_else(|_| {
                        // If it's not valid UTF-8, encode as hex
                        b.iter().map(|byte| format!("{:02x}", byte)).collect::<String>()
                    })
                },
                _ => format!("{:?}", pk_value), // Fallback for other types
            };
            
            // Handle the val column which can be of various types
            let val_value: rusqlite::types::Value = row.get(3)?;
            let val = match val_value {
                rusqlite::types::Value::Text(s) => Some(s),
                rusqlite::types::Value::Integer(i) => Some(i.to_string()),
                rusqlite::types::Value::Real(f) => Some(f.to_string()),
                rusqlite::types::Value::Blob(b) => {
                    Some(String::from_utf8(b.clone()).unwrap_or_else(|_| {
                        b.iter().map(|byte| format!("{:02x}", byte)).collect::<String>()
                    }))
                },
                rusqlite::types::Value::Null => None,
            };

            Ok(SyncChange {
                table_name: row.get(0)?,
                pk: pk.into(),
                cid: row.get(2)?,
                val,
                col_version: row.get(4)?,
                db_version: row.get(5)?,
                site_id: row.get(6)?,
                cl: row.get(7)?,
                seq: row.get(8)?,
            })
        }).map_err(|e| format!("Failed to execute changes query: {}", e))?;

        let mut result = Vec::new();
        for change in changes {
            result.push(change.map_err(|e| format!("Failed to read change: {}", e))?);
        }

        Ok(result)
    }

    fn apply_changes(conn: &Connection, changes: Vec<SyncChange>) -> Result<(), String> {
        for change in changes {
            conn.execute(
                "INSERT INTO crsql_changes (\"table\", pk, cid, val, col_version, db_version, site_id, cl, seq) 
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                [
                    &change.table_name as &dyn rusqlite::ToSql,
                    &change.pk,
                    &change.cid,
                    &change.val,
                    &change.col_version,
                    &change.db_version,
                    &change.site_id,
                    &change.cl,
                    &change.seq,
                ],
            ).map_err(|e| format!("Failed to insert change: {}", e))?;
        }

        Ok(())
    }

    #[test]
    fn test_sync_integration() {
        let temp_dir = tempdir().expect("Failed to create temp directory");
        
        let db_a_path = temp_dir.path().join("db_a.db");
        let db_b_path = temp_dir.path().join("db_b.db");

        // Create two databases
        let db_a = init_test_database(db_a_path.to_str().unwrap()).expect("Failed to init db_a");
        let db_b = init_test_database(db_b_path.to_str().unwrap()).expect("Failed to init db_b");

        let now = chrono::Utc::now().to_rfc3339();

        // Insert a note in db_a
        db_a.execute(
            "INSERT INTO notes (id, title, notebook, content_preview, word_count, created_at, updated_at, file_path) 
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                "note_a",
                "Note from A", 
                "General",
                "This is note A content",
                4,
                now,
                now,
                "/path/to/note_a.md"
            ]
        ).expect("Failed to insert note in db_a");

        // Insert a different note in db_b
        db_b.execute(
            "INSERT INTO notes (id, title, notebook, content_preview, word_count, created_at, updated_at, file_path) 
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                "note_b",
                "Note from B",
                "General", 
                "This is note B content",
                4,
                now,
                now,
                "/path/to/note_b.md"
            ]
        ).expect("Failed to insert note in db_b");

        // Extract changes from db_a
        let changes_a = extract_changes(&db_a, 0).expect("Failed to extract changes from db_a");
        println!("Changes from db_a: {} changes", changes_a.len());
        assert!(!changes_a.is_empty(), "db_a should have changes");

        // Apply changes to db_b
        apply_changes(&db_b, changes_a).expect("Failed to apply changes to db_b");

        // Extract changes from db_b
        let changes_b = extract_changes(&db_b, 0).expect("Failed to extract changes from db_b");
        println!("Changes from db_b: {} changes", changes_b.len());

        // Apply changes to db_a
        apply_changes(&db_a, changes_b).expect("Failed to apply changes to db_a");

        // Verify both databases now have both notes
        let count_a: i64 = db_a.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
            .expect("Failed to count notes in db_a");
        let count_b: i64 = db_b.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))
            .expect("Failed to count notes in db_b");

        println!("Notes in db_a: {}, Notes in db_b: {}", count_a, count_b);
        assert_eq!(count_a, 2, "db_a should have 2 notes");
        assert_eq!(count_b, 2, "db_b should have 2 notes");

        // Verify the specific notes exist
        let note_a_in_a: bool = db_a.query_row("SELECT COUNT(*) > 0 FROM notes WHERE id = 'note_a'", [], |row| row.get(0))
            .expect("Failed to check note_a in db_a");
        let note_b_in_a: bool = db_a.query_row("SELECT COUNT(*) > 0 FROM notes WHERE id = 'note_b'", [], |row| row.get(0))
            .expect("Failed to check note_b in db_a");
        let note_a_in_b: bool = db_b.query_row("SELECT COUNT(*) > 0 FROM notes WHERE id = 'note_a'", [], |row| row.get(0))
            .expect("Failed to check note_a in db_b");
        let note_b_in_b: bool = db_b.query_row("SELECT COUNT(*) > 0 FROM notes WHERE id = 'note_b'", [], |row| row.get(0))
            .expect("Failed to check note_b in db_b");

        assert!(note_a_in_a, "note_a should exist in db_a");
        assert!(note_b_in_a, "note_b should exist in db_a");
        assert!(note_a_in_b, "note_a should exist in db_b");
        assert!(note_b_in_b, "note_b should exist in db_b");

        println!("✅ Basic sync test passed");

        // Test conflict resolution: update the SAME note in both databases
        let later_time = chrono::Utc::now().to_rfc3339();
        
        // Update note_a in both databases with different content
        db_a.execute(
            "UPDATE notes SET title = ?, updated_at = ? WHERE id = ?",
            params!["Note A updated from A", later_time, "note_a"]
        ).expect("Failed to update note_a in db_a");

        std::thread::sleep(std::time::Duration::from_millis(10)); // Ensure different timestamp
        let later_time_2 = chrono::Utc::now().to_rfc3339();

        db_b.execute(
            "UPDATE notes SET title = ?, updated_at = ? WHERE id = ?",
            params!["Note A updated from B", later_time_2, "note_a"]
        ).expect("Failed to update note_a in db_b");

        // Extract and apply changes (conflict resolution)
        let changes_a_conflict = extract_changes(&db_a, count_a).expect("Failed to extract conflict changes from db_a");
        apply_changes(&db_b, changes_a_conflict).expect("Failed to apply conflict changes to db_b");

        let changes_b_conflict = extract_changes(&db_b, count_b).expect("Failed to extract conflict changes from db_b");
        apply_changes(&db_a, changes_b_conflict).expect("Failed to apply conflict changes to db_a");

        // Check which version won (should be last-write-wins based on timestamp)
        let title_in_a: String = db_a.query_row("SELECT title FROM notes WHERE id = 'note_a'", [], |row| row.get(0))
            .expect("Failed to get title from db_a");
        let title_in_b: String = db_b.query_row("SELECT title FROM notes WHERE id = 'note_b'", [], |row| row.get(0))
            .expect("Failed to get title from db_b");

        println!("Conflict resolution - title in db_a: '{}', title in db_b: '{}'", title_in_a, title_in_b);

        // Both databases should have the same title for note_a after conflict resolution
        let final_title_a: String = db_a.query_row("SELECT title FROM notes WHERE id = 'note_a'", [], |row| row.get(0))
            .expect("Failed to get final title from db_a");
        let final_title_b: String = db_b.query_row("SELECT title FROM notes WHERE id = 'note_a'", [], |row| row.get(0))
            .expect("Failed to get final title from db_b");

        assert_eq!(final_title_a, final_title_b, "Both databases should have the same title for note_a after sync");
        println!("✅ Conflict resolution test passed - final title: '{}'", final_title_a);

        println!("✅ All sync integration tests passed!");
    }
}