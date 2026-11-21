// Pane v0.61.1 — Transcript Archive Module
// Implements §5.14.2 — move messages >30 days from active table to archive table

use rusqlite::{params, Connection};

/// Archive old messages from chat_messages_local to chat_archive
/// Returns: (success: bool, count_archived: usize, error_message: Option<String>)
pub fn archive_old_messages(conn: &Connection) -> (bool, usize, Option<String>) {
    eprintln!("[ARCHIVE] Starting transcript archive migration...");
    
    // Calculate cutoff date (30 days ago)
    let cutoff_date = chrono::Utc::now() - chrono::Duration::days(30);
    let cutoff_str = cutoff_date.format("%Y-%m-%d %H:%M:%S").to_string();
    
    eprintln!("[ARCHIVE] Cutoff date: {} (messages older than this will be archived)", cutoff_str);
    
    // Start transaction
    if let Err(e) = conn.execute("BEGIN TRANSACTION", []) {
        let error = format!("Failed to begin transaction: {}", e);
        eprintln!("[ARCHIVE] ERROR: {}", error);
        return (false, 0, Some(error));
    }
    
    // Count messages to be archived (for logging)
    let count_to_archive: usize = match conn.query_row(
        "SELECT COUNT(*) FROM chat_messages_local WHERE created_at < ?1",
        params![cutoff_str],
        |row| row.get(0),
    ) {
        Ok(count) => count,
        Err(e) => {
            let error = format!("Failed to count messages to archive: {}", e);
            eprintln!("[ARCHIVE] ERROR: {}", error);
            conn.execute("ROLLBACK", []).ok();
            return (false, 0, Some(error));
        }
    };
    
    if count_to_archive == 0 {
        eprintln!("[ARCHIVE] No messages to archive (all messages are <30 days old)");
        conn.execute("ROLLBACK", []).ok();
        return (true, 0, None);
    }
    
    eprintln!("[ARCHIVE] Found {} messages to archive", count_to_archive);
    
    // Get current timestamp for archived_at
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    
    // Insert old messages into archive table
    let insert_result = conn.execute(
        "INSERT INTO chat_archive (id, thread_id, project_id, role, content, created_at, metadata, archived_at)
         SELECT id, COALESCE(thread_id, ''), COALESCE(project_id, ''), role, content, created_at, 
                json_object('tool_calls', tool_calls, 'tool_results', tool_results, 'model', model, 
                           'token_count', token_count, 'attachment_ids', attachment_ids, 'is_pinned', is_pinned),
                ?1
         FROM chat_messages_local 
         WHERE created_at < ?2",
        params![now, cutoff_str],
    );
    
    if let Err(e) = insert_result {
        let error = format!("Failed to insert into archive: {}", e);
        eprintln!("[ARCHIVE] ERROR: {}", error);
        conn.execute("ROLLBACK", []).ok();
        return (false, 0, Some(error));
    }
    
    let inserted_count = insert_result.unwrap();
    eprintln!("[ARCHIVE] Inserted {} messages into chat_archive", inserted_count);
    
    // Insert content into FTS index
    let fts_result = conn.execute(
        "INSERT INTO chat_archive_fts (rowid, content)
         SELECT rowid, content FROM chat_archive WHERE archived_at = ?1",
        params![now],
    );
    
    if let Err(e) = fts_result {
        let error = format!("Failed to update FTS index: {}", e);
        eprintln!("[ARCHIVE] ERROR: {}", error);
        conn.execute("ROLLBACK", []).ok();
        return (false, 0, Some(error));
    }
    
    eprintln!("[ARCHIVE] Updated FTS index for {} messages", fts_result.unwrap());
    
    // Delete from active table
    let delete_result = conn.execute(
        "DELETE FROM chat_messages_local WHERE created_at < ?1",
        params![cutoff_str],
    );
    
    if let Err(e) = delete_result {
        let error = format!("Failed to delete from active table: {}", e);
        eprintln!("[ARCHIVE] ERROR: {}", error);
        conn.execute("ROLLBACK", []).ok();
        return (false, 0, Some(error));
    }
    
    let deleted_count = delete_result.unwrap();
    eprintln!("[ARCHIVE] Deleted {} messages from chat_messages_local", deleted_count);
    
    // Verify counts match
    if inserted_count != deleted_count {
        let error = format!(
            "Archive count mismatch: inserted {} but deleted {}",
            inserted_count, deleted_count
        );
        eprintln!("[ARCHIVE] ERROR: {}", error);
        conn.execute("ROLLBACK", []).ok();
        return (false, 0, Some(error));
    }
    
    // Commit transaction
    if let Err(e) = conn.execute("COMMIT", []) {
        let error = format!("Failed to commit transaction: {}", e);
        eprintln!("[ARCHIVE] ERROR: {}", error);
        conn.execute("ROLLBACK", []).ok();
        return (false, 0, Some(error));
    }
    
    eprintln!("[ARCHIVE] ✓ Successfully archived {} messages", inserted_count);
    (true, inserted_count, None)
}

/// Check if archive migration should run (≥7 days since last run or never run)
pub fn should_run_archive(conn: &Connection) -> bool {
    let last_archived: Option<String> = crate::database::get_ai_setting(conn, "last_archived_at");
    
    match last_archived {
        None => {
            eprintln!("[ARCHIVE] No previous archive run detected");
            true
        }
        Some(ref timestamp) => {
            // Try to parse the timestamp
            let parsed = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.fZ")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f%:z"))
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S"));
            
            match parsed {
                Ok(dt) => {
                    let days_since_last = (chrono::Utc::now().naive_utc() - dt).num_days();
                    eprintln!("[ARCHIVE] Last archive run was {} days ago", days_since_last);
                    days_since_last >= 7
                }
                Err(_) => {
                    eprintln!("[ARCHIVE] WARNING: Could not parse last_archived_at timestamp: {}", timestamp);
                    true  // If we can't parse, assume it's time to run
                }
            }
        }
    }
}

/// Run the archive migration and update last_archived_at timestamp
pub fn run_archive_migration(conn: &Connection) {
    eprintln!("[ARCHIVE] Running archive migration check...");
    
    if !should_run_archive(conn) {
        eprintln!("[ARCHIVE] Skipping archive (last run was <7 days ago)");
        return;
    }
    
    let (success, count, error) = archive_old_messages(conn);
    
    if success {
        // Update last_archived_at timestamp
        let now = chrono::Utc::now().to_rfc3339();
        if let Err(e) = crate::database::set_ai_setting(conn, "last_archived_at", &now) {
            eprintln!("[ARCHIVE] ERROR: Failed to update last_archived_at: {}", e);
        } else {
            eprintln!("[ARCHIVE] Updated last_archived_at to {}", now);
        }
        
        if count > 0 {
            eprintln!("[ARCHIVE] ✓ Archive migration complete: {} messages archived", count);
        } else {
            eprintln!("[ARCHIVE] ✓ Archive check complete: no messages to archive");
        }
    } else {
        eprintln!("[ARCHIVE] ✗ Archive migration failed: {}", error.unwrap_or_else(|| "Unknown error".to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;
    
    fn create_test_db() -> (Connection, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");
        
        let conn = Connection::open(&db_path).expect("Failed to open test database");
        
        // Create schema
        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
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
            
            CREATE VIRTUAL TABLE IF NOT EXISTS chat_archive_fts USING fts5(
                content,
                content_rowid='rowid',
                tokenize='porter unicode61'
            );
        ").expect("Failed to create test schema");
        
        (conn, temp_dir)
    }
    
    #[test]
    fn test_archive_moves_old_messages() {
        eprintln!("[TEST] test_archive_moves_old_messages");
        
        let (conn, _temp_dir) = create_test_db();
        
        // Insert old message (35 days ago)
        let old_date = (chrono::Utc::now() - chrono::Duration::days(35))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, role, content, created_at, thread_id) 
             VALUES ('old1', 'proj1', 'user', 'Old message', ?1, 'thread1')",
            params![old_date],
        ).expect("Failed to insert old message");
        
        // Insert recent message (15 days ago)
        let recent_date = (chrono::Utc::now() - chrono::Duration::days(15))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, role, content, created_at, thread_id) 
             VALUES ('recent1', 'proj1', 'user', 'Recent message', ?1, 'thread1')",
            params![recent_date],
        ).expect("Failed to insert recent message");
        
        // Run archive
        let (success, count, error) = archive_old_messages(&conn);
        
        assert!(success, "Archive should succeed: {:?}", error);
        assert_eq!(count, 1, "Should archive exactly 1 message");
        
        // Verify old message is in archive
        let archive_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM chat_archive WHERE id = 'old1'",
            [],
            |row| row.get(0),
        ).expect("Failed to count archive");
        assert_eq!(archive_count, 1, "Old message should be in archive");
        
        // Verify recent message is still in active table
        let active_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM chat_messages_local WHERE id = 'recent1'",
            [],
            |row| row.get(0),
        ).expect("Failed to count active");
        assert_eq!(active_count, 1, "Recent message should still be in active table");
        
        // Verify old message is NOT in active table
        let old_in_active: usize = conn.query_row(
            "SELECT COUNT(*) FROM chat_messages_local WHERE id = 'old1'",
            [],
            |row| row.get(0),
        ).expect("Failed to count active");
        assert_eq!(old_in_active, 0, "Old message should be removed from active table");
        
        eprintln!("[TEST] ✓ Archive correctly moves old messages");
    }
    
    #[test]
    fn test_archive_does_not_move_recent_messages() {
        eprintln!("[TEST] test_archive_does_not_move_recent_messages");
        
        let (conn, _temp_dir) = create_test_db();
        
        // Insert only recent messages
        let recent_date = (chrono::Utc::now() - chrono::Duration::days(15))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, role, content, created_at, thread_id) 
             VALUES ('recent1', 'proj1', 'user', 'Recent message 1', ?1, 'thread1')",
            params![recent_date],
        ).expect("Failed to insert message 1");
        
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, role, content, created_at, thread_id) 
             VALUES ('recent2', 'proj1', 'user', 'Recent message 2', ?1, 'thread1')",
            params![recent_date],
        ).expect("Failed to insert message 2");
        
        // Run archive
        let (success, count, error) = archive_old_messages(&conn);
        
        assert!(success, "Archive should succeed: {:?}", error);
        assert_eq!(count, 0, "Should archive 0 messages");
        
        // Verify messages are still in active table
        let active_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM chat_messages_local",
            [],
            |row| row.get(0),
        ).expect("Failed to count active");
        assert_eq!(active_count, 2, "Both messages should still be in active table");
        
        // Verify archive is empty
        let archive_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM chat_archive",
            [],
            |row| row.get(0),
        ).expect("Failed to count archive");
        assert_eq!(archive_count, 0, "Archive should be empty");
        
        eprintln!("[TEST] ✓ Archive does not move recent messages");
    }
    
    #[test]
    fn test_archive_transaction_rollback_on_failure() {
        eprintln!("[TEST] test_archive_transaction_rollback_on_failure");
        
        let (conn, _temp_dir) = create_test_db();
        
        // Insert old message
        let old_date = (chrono::Utc::now() - chrono::Duration::days(35))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, role, content, created_at, thread_id) 
             VALUES ('old1', 'proj1', 'user', 'Old message', ?1, 'thread1')",
            params![old_date],
        ).expect("Failed to insert old message");
        
        // Drop the archive table to force a failure
        conn.execute("DROP TABLE chat_archive", []).expect("Failed to drop archive table");
        
        // Run archive (should fail)
        let (success, count, _error) = archive_old_messages(&conn);
        
        assert!(!success, "Archive should fail when archive table is missing");
        assert_eq!(count, 0, "No messages should be counted as archived");
        
        // Recreate archive table
        conn.execute_batch("
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
        ").expect("Failed to recreate archive table");
        
        // Verify original message is still in active table (rollback worked)
        let active_count: usize = conn.query_row(
            "SELECT COUNT(*) FROM chat_messages_local WHERE id = 'old1'",
            [],
            |row| row.get(0),
        ).expect("Failed to count active");
        assert_eq!(active_count, 1, "Message should still be in active table after rollback");
        
        eprintln!("[TEST] ✓ Transaction rollback preserves data on failure");
    }
    
    #[test]
    fn test_fts_index_populated_for_archived_messages() {
        eprintln!("[TEST] test_fts_index_populated_for_archived_messages");
        
        let (conn, _temp_dir) = create_test_db();
        
        // Insert old message with searchable content
        let old_date = (chrono::Utc::now() - chrono::Duration::days(35))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, role, content, created_at, thread_id) 
             VALUES ('old1', 'proj1', 'user', 'floating therapy business plan', ?1, 'thread1')",
            params![old_date],
        ).expect("Failed to insert old message");
        
        // Run archive
        let (success, count, error) = archive_old_messages(&conn);
        
        assert!(success, "Archive should succeed: {:?}", error);
        assert_eq!(count, 1, "Should archive exactly 1 message");
        
        // Search FTS index
        let fts_result: Option<String> = conn.query_row(
            "SELECT content FROM chat_archive_fts WHERE content MATCH 'therapy'",
            [],
            |row| row.get(0),
        ).ok();
        
        assert!(fts_result.is_some(), "FTS search should find archived message");
        assert!(fts_result.unwrap().contains("floating therapy"), "FTS should return correct content");
        
        eprintln!("[TEST] ✓ FTS index populated for archived messages");
    }
    
    #[test]
    fn test_should_run_archive_respects_interval() {
        eprintln!("[TEST] test_should_run_archive_respects_interval");
        
        let (conn, _temp_dir) = create_test_db();
        
        // Test 1: No last_archived_at → should run
        assert!(should_run_archive(&conn), "Should run when never run before");
        
        // Test 2: Set last_archived_at to 3 days ago → should NOT run
        let three_days_ago = (chrono::Utc::now() - chrono::Duration::days(3)).to_rfc3339();
        crate::database::set_ai_setting(&conn, "last_archived_at", &three_days_ago)
            .expect("Failed to set last_archived_at");
        assert!(!should_run_archive(&conn), "Should NOT run when last run was 3 days ago");
        
        // Test 3: Set last_archived_at to 8 days ago → should run
        let eight_days_ago = (chrono::Utc::now() - chrono::Duration::days(8)).to_rfc3339();
        crate::database::set_ai_setting(&conn, "last_archived_at", &eight_days_ago)
            .expect("Failed to set last_archived_at");
        assert!(should_run_archive(&conn), "Should run when last run was 8 days ago");
        
        eprintln!("[TEST] ✓ Archive respects 7-day interval");
    }
    
    #[test]
    fn test_run_archive_migration_updates_timestamp() {
        eprintln!("[TEST] test_run_archive_migration_updates_timestamp");
        
        let (conn, _temp_dir) = create_test_db();
        
        // Insert old message
        let old_date = (chrono::Utc::now() - chrono::Duration::days(35))
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, role, content, created_at, thread_id) 
             VALUES ('old1', 'proj1', 'user', 'Old message', ?1, 'thread1')",
            params![old_date],
        ).expect("Failed to insert old message");
        
        // Verify no timestamp exists
        assert!(crate::database::get_ai_setting(&conn, "last_archived_at").is_none());
        
        // Run migration
        run_archive_migration(&conn);
        
        // Verify timestamp was set
        let timestamp = crate::database::get_ai_setting(&conn, "last_archived_at");
        assert!(timestamp.is_some(), "last_archived_at should be set after successful run");
        
        eprintln!("[TEST] ✓ Archive migration updates last_archived_at timestamp");
    }
}
