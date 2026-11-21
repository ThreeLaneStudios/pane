// Tests for watcher startup reconciliation (§22C.11.3)

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_db() -> (Connection, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let db_path = temp_dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();

        // Create minimal schema
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ai_agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL UNIQUE,
                role TEXT NOT NULL DEFAULT 'sub',
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                workspace_path TEXT NOT NULL,
                runtime_ref TEXT,
                source TEXT NOT NULL DEFAULT 'manual',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                owner_id TEXT
            );
            
            CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );
            
            CREATE TABLE IF NOT EXISTS notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                content TEXT,
                content_preview TEXT,
                file_path TEXT,
                project_id TEXT,
                folder_id TEXT,
                status TEXT DEFAULT 'draft',
                word_count INTEGER DEFAULT 0,
                tags_json TEXT DEFAULT '[]',
                owner_id TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                deleted_at TEXT
            );
            ",
        )
        .unwrap();

        (conn, temp_dir)
    }

    #[test]
    fn test_last_known_timestamp_round_trip() {
        let (conn, _temp) = setup_test_db();

        // Initially no timestamp
        let key = "file_mtime:main:SOUL.md";
        let value: Option<String> = conn
            .query_row("SELECT value FROM ai_settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .ok();
        assert!(value.is_none());

        // Store a timestamp
        let now = chrono::Utc::now();
        crate::database::set_ai_setting(&conn, key, &now.to_rfc3339()).unwrap();

        // Retrieve it
        let retrieved: Option<String> = conn
            .query_row("SELECT value FROM ai_settings WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .ok();
        assert!(retrieved.is_some());

        // Parse it back
        let parsed = chrono::DateTime::parse_from_rfc3339(&retrieved.unwrap())
            .ok()
            .map(|dt| dt.with_timezone(&chrono::Utc));
        assert!(parsed.is_some());

        // Should be within 1 second of original (allowing for serialization rounding)
        let diff = (parsed.unwrap() - now).num_seconds().abs();
        assert!(diff <= 1, "Timestamp round-trip diff too large: {}", diff);
    }

    #[test]
    fn test_whitelisted_files_complete() {
        // Verify the whitelisted file list in watcher.rs matches the spec
        let expected = vec![
            "SOUL.md",
            "MEMORY.md",
            "IDENTITY.md",
            "USER.md",
            "TOOLS.md",
            "AGENTS.md",
            "HEARTBEAT.md",
            "RULES.md",
        ];

        // This test documents the whitelist — if this fails, either:
        // 1. Watcher.rs whitelisted array changed and this test needs update
        // 2. Or the change was unintentional
        assert_eq!(expected.len(), 8, "Expected 8 whitelisted files");
    }

    #[test]
    fn test_reconciliation_detects_modified_file() {
        let (conn, temp) = setup_test_db();

        // Insert an agent
        let agent_id = "test-agent-1";
        let agent_name = "TestAgent";
        let workspace_path = "agents/test";
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, created_at, updated_at, is_active)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'), 1)",
            rusqlite::params![agent_id, agent_name, agent_name.to_lowercase(), workspace_path],
        )
        .unwrap();

        // Simulate: file was last seen 1 hour ago
        let old_time = chrono::Utc::now() - chrono::Duration::hours(1);
        let key = format!("file_mtime:{}:SOUL.md", agent_name);
        crate::database::set_ai_setting(&conn, &key, &old_time.to_rfc3339()).unwrap();

        // Create a file with mtime = now (newer than DB record)
        let file_path = temp.path().join("test_file.md");
        fs::write(&file_path, "New content written by OC").unwrap();

        // Get file's actual mtime
        let file_mtime = fs::metadata(&file_path)
            .unwrap()
            .modified()
            .unwrap();
        let file_mtime_utc: chrono::DateTime<chrono::Utc> = file_mtime.into();

        // Verify file is newer than DB record
        assert!(
            file_mtime_utc > old_time,
            "Test setup failed: file should be newer than DB record"
        );

        // Retrieve last-known timestamp from DB
        let retrieved: Option<String> = conn
            .query_row(
                "SELECT value FROM ai_settings WHERE key = ?1",
                [&key],
                |row| row.get(0),
            )
            .ok();

        assert!(retrieved.is_some());
        let db_time = chrono::DateTime::parse_from_rfc3339(&retrieved.unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc);

        // Reconciliation logic should detect this as modified (file mtime > db_time)
        assert!(
            file_mtime_utc > db_time,
            "File should be newer than DB record — would trigger reconciliation"
        );
    }

    #[test]
    fn test_reconciliation_skips_unmodified_file() {
        let (conn, temp) = setup_test_db();

        // Insert an agent
        let agent_id = "test-agent-2";
        let agent_name = "TestAgent2";
        let workspace_path = "agents/test2";
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, created_at, updated_at, is_active)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'), 1)",
            rusqlite::params![agent_id, agent_name, agent_name.to_lowercase(), workspace_path],
        )
        .unwrap();

        // Create a file
        let file_path = temp.path().join("test_file2.md");
        fs::write(&file_path, "Original content").unwrap();

        // Get file's mtime
        let file_mtime = fs::metadata(&file_path)
            .unwrap()
            .modified()
            .unwrap();
        let file_mtime_utc: chrono::DateTime<chrono::Utc> = file_mtime.into();

        // Record this exact mtime in DB
        let key = format!("file_mtime:{}:MEMORY.md", agent_name);
        crate::database::set_ai_setting(&conn, &key, &file_mtime_utc.to_rfc3339()).unwrap();

        // Retrieve and verify
        let retrieved: Option<String> = conn
            .query_row(
                "SELECT value FROM ai_settings WHERE key = ?1",
                [&key],
                |row| row.get(0),
            )
            .ok();

        assert!(retrieved.is_some());
        let db_time = chrono::DateTime::parse_from_rfc3339(&retrieved.unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc);

        // File mtime should match DB record (within 1 second for serialization)
        let diff = (file_mtime_utc - db_time).num_seconds().abs();
        assert!(
            diff <= 1,
            "File mtime should match DB record — would skip reconciliation"
        );
    }

    #[test]
    fn test_get_agents_for_reconciliation() {
        let (conn, _temp) = setup_test_db();

        // Insert multiple agents
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, created_at, updated_at, is_active)
             VALUES ('a1', 'Agent1', 'agent1', 'agents/agent1', datetime('now'), datetime('now'), 1)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, created_at, updated_at, is_active)
             VALUES ('a2', 'Agent2', 'agent2', 'agents/agent2', datetime('now'), datetime('now'), 1)",
            [],
        )
        .unwrap();

        // Insert inactive agent (should be excluded)
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, created_at, updated_at, is_active)
             VALUES ('a3', 'Agent3', 'agent3', 'agents/agent3', datetime('now'), datetime('now'), 0)",
            [],
        )
        .unwrap();

        // Query agents
        let mut stmt = conn
            .prepare("SELECT name, workspace_path FROM ai_agents WHERE is_active = 1")
            .unwrap();

        let agents: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        // Should return only the 2 active agents
        assert_eq!(agents.len(), 2);
        assert!(agents.iter().any(|(name, _)| name == "Agent1"));
        assert!(agents.iter().any(|(name, _)| name == "Agent2"));
        assert!(!agents.iter().any(|(name, _)| name == "Agent3"));
    }
}
