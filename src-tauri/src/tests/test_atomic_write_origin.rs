// Integration tests for atomic_write WriteOrigin behavior (#1298)
//
// Verifies that:
//   - WriteOrigin::Local on a system file queues a push to sync_queue
//   - WriteOrigin::Remote on a system file does NOT queue a push (ping-pong suppression)
//   - WriteOrigin::Remote with db_conn=Some still suppresses trigger_sync
//
// These tests exercise the real atomic_write code path against an in-memory-like SQLite DB.

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};
    use std::path::PathBuf;
    use tempfile::TempDir;
    use crate::file_ops::{atomic_write, WriteOrigin};

    /// Create a minimal DB with sync_queue, ai_agents, and ai_settings tables.
    /// Inserts a test agent with a known workspace_path UUID and runtime_ref so that
    /// is_system_path() + trigger_sync() can look it up.
    fn setup_db_with_agent(temp_dir: &TempDir) -> (Connection, String) {
        let db_path = temp_dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ai_agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'sub',
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                workspace_path TEXT NOT NULL,
                runtime_ref TEXT,
                runtime_type TEXT,
                source TEXT NOT NULL DEFAULT 'manual',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                owner_id TEXT
            );

            CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS sync_queue (
                id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
                agent           TEXT NOT NULL,
                filename        TEXT NOT NULL,
                content         TEXT NOT NULL,
                checksum        TEXT NOT NULL,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                retry_count     INTEGER DEFAULT 0,
                next_retry_at   TEXT,
                UNIQUE(agent, filename, checksum)
            );
            ",
        )
        .unwrap();

        // Insert a test agent with a fixed UUID workspace_path and runtime_ref
        let agent_uuid = "test-agent-uuid-1298";
        let runtime_ref = "oc-agent-test-1298";
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_ref, runtime_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'openclaw', datetime('now'), datetime('now'))",
            params!["agent-id-1298", "Test Agent", "test-agent", agent_uuid, runtime_ref],
        )
        .unwrap();

        // Insert the keys that trigger_sync reads via get_gateway_url / get_gateway_token:
        //   get_gateway_url:   #2564 B1 fix (Path B) — requires an active
        //                      ai_agents.runtime_type='openclaw' row (set on the INSERT
        //                      above), replacing the retired ai_settings.provider check.
        //                      get_ai_setting(conn, "endpoint")  → the gateway URL
        //   get_gateway_token: get_ai_setting(conn, "api_key")   → the bearer token
        // (See sync_client.rs:207-220 — NOT "pane_gateway_url" / "pane_gateway_token")
        //
        // With correct keys, trigger_sync reaches queue_sync_blocking for Local writes
        // (synchronous DB insert → queue entry exists before test assertion runs).
        // The background network push will fail against the fake URL, but the queue
        // entry is already committed — test assertions are stable without mocking.
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
            params!["endpoint", "https://gateway.test.example"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
            params!["api_key", "test-token-1298"],
        )
        .unwrap();

        (conn, agent_uuid.to_string())
    }

    /// Build a system file path that is_system_path() will recognize.
    /// System paths follow: <pane_root>/.pane/system/{agent_uuid}/{filename}
    fn make_system_path(temp_dir: &TempDir, agent_uuid: &str, filename: &str) -> PathBuf {
        // Simulate Pane's system path structure inside a temp dir
        let system_dir = temp_dir.path()
            .join(".pane")
            .join("system")
            .join(agent_uuid);
        std::fs::create_dir_all(&system_dir).unwrap();
        system_dir.join(filename)
    }

    /// Count sync_queue entries for a given agent + filename.
    fn queue_count(conn: &Connection, runtime_ref: &str, filename: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM sync_queue WHERE agent = ?1 AND filename = ?2",
            params![runtime_ref, filename],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 1 (positive): WriteOrigin::Local on a system file DOES queue a push
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_local_write_queues_sync() {
        let temp_dir = TempDir::new().unwrap();
        let (conn, agent_uuid) = setup_db_with_agent(&temp_dir);

        let file_path = make_system_path(&temp_dir, &agent_uuid, "SOUL.md");
        let content = "# Soul\n\nLocal write test content.";

        let result = atomic_write(&file_path, content, Some(&conn), WriteOrigin::Local);
        assert!(result.is_ok(), "atomic_write with Local should succeed: {:?}", result);

        // File should exist on disk
        assert!(file_path.exists(), "File should exist after atomic_write");
        let written = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, content, "File content should match what was written");

        // With correct settings keys (provider=openclaw, endpoint, api_key), trigger_sync
        // reaches queue_sync_blocking which does a synchronous DB insert — queue entry
        // exists before this assertion runs. The background thread that attempts the
        // actual network push will fail against the fake URL, but that's after the insert.
        let count = queue_count(&conn, "oc-agent-test-1298", "SOUL.md");
        eprintln!("[test_local_write_queues_sync] sync_queue count after Local write: {}", count);
        assert!(
            count > 0,
            "WriteOrigin::Local on a system file with valid settings must queue a sync entry; got {}",
            count
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 2 (negative): WriteOrigin::Remote on a system file does NOT queue
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_remote_write_does_not_queue_sync() {
        let temp_dir = TempDir::new().unwrap();
        let (conn, agent_uuid) = setup_db_with_agent(&temp_dir);

        let file_path = make_system_path(&temp_dir, &agent_uuid, "MEMORY.md");
        let content = "# Memory\n\nRemote ingest write — should not be re-queued.";

        // Pre-condition: sync_queue is empty for this file
        assert_eq!(
            queue_count(&conn, "oc-agent-test-1298", "MEMORY.md"),
            0,
            "sync_queue must be empty before ingest write"
        );

        let result = atomic_write(&file_path, content, Some(&conn), WriteOrigin::Remote);
        assert!(result.is_ok(), "atomic_write with Remote should succeed: {:?}", result);

        // File should exist and contain the ingested content
        assert!(file_path.exists(), "File should exist after Remote atomic_write");
        let written = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, content, "File content should match ingested content");

        // Critical assertion: sync_queue must remain empty.
        // WriteOrigin::Remote suppresses trigger_sync — no ping-pong.
        assert_eq!(
            queue_count(&conn, "oc-agent-test-1298", "MEMORY.md"),
            0,
            "sync_queue must be empty after Remote ingest write — WriteOrigin::Remote must suppress trigger_sync"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 3: Remote with db_conn=Some still suppresses (not just db_conn=None)
    // This is the core regression guard — prior to #1298 any Some(conn) triggered sync.
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_remote_with_some_conn_still_suppresses_sync() {
        let temp_dir = TempDir::new().unwrap();
        let (conn, agent_uuid) = setup_db_with_agent(&temp_dir);

        let file_path = make_system_path(&temp_dir, &agent_uuid, "USER.md");
        let content = "# User\n\nIngest content — db_conn=Some, origin=Remote.";

        // 224-file scenario simulation: write the same file 5 times as Remote
        for i in 1..=5 {
            let versioned_content = format!("{} (v{})", content, i);
            let result = atomic_write(&file_path, &versioned_content, Some(&conn), WriteOrigin::Remote);
            assert!(result.is_ok(), "Remote write {} should succeed: {:?}", i, result);
        }

        // Even with 5 writes and Some(conn) provided, zero queue entries
        let total = queue_count(&conn, "oc-agent-test-1298", "USER.md");
        assert_eq!(
            total, 0,
            "WriteOrigin::Remote with Some(conn) must NOT add any sync_queue entries; got {}",
            total
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 4: WriteOrigin::Remote with db_conn=None (bootstrap path) still works
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_remote_with_none_conn_writes_file() {
        let temp_dir = TempDir::new().unwrap();
        let (_conn, agent_uuid) = setup_db_with_agent(&temp_dir);

        let file_path = make_system_path(&temp_dir, &agent_uuid, "TOOLS.md");
        let content = "# Tools\n\nBootstrap ingest — no DB connection.";

        let result = atomic_write(&file_path, content, None, WriteOrigin::Remote);
        assert!(result.is_ok(), "Remote write with None conn should succeed: {:?}", result);
        assert!(file_path.exists(), "File should exist after Remote write with None conn");

        let written = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, content, "Content should match despite None conn");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 5: WriteOrigin::Local with db_conn=None (pre-DB bootstrap path)
    // Should write the file but not crash (no conn = no sync attempt)
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_local_with_none_conn_writes_file_no_panic() {
        let temp_dir = TempDir::new().unwrap();
        let (_conn, agent_uuid) = setup_db_with_agent(&temp_dir);

        let file_path = make_system_path(&temp_dir, &agent_uuid, "RULES.md");
        let content = "# Rules\n\nPre-DB bootstrap local write.";

        let result = atomic_write(&file_path, content, None, WriteOrigin::Local);
        assert!(result.is_ok(), "Local write with None conn should succeed (no sync attempt): {:?}", result);
        assert!(file_path.exists(), "File should exist");

        let written = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(written, content);
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 6: Non-system path with Remote — file writes, no queue touched
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_non_system_path_remote_write() {
        let temp_dir = TempDir::new().unwrap();
        let (conn, _agent_uuid) = setup_db_with_agent(&temp_dir);

        // Non-system path: not under .pane/system/{uuid}/
        let note_path = temp_dir.path().join("notebooks").join("General").join("test-note.md");
        std::fs::create_dir_all(note_path.parent().unwrap()).unwrap();

        let content = "# Test\n\nA regular note, not a system file.";

        let result = atomic_write(&note_path, content, Some(&conn), WriteOrigin::Remote);
        assert!(result.is_ok(), "Remote write on non-system path should succeed: {:?}", result);
        assert!(note_path.exists(), "File should exist");

        // No sync queue entries for a non-system file
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_queue", [], |row| row.get(0))
            .unwrap_or(0);
        assert_eq!(total, 0, "Non-system path must not queue any sync entries");
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 7: pull_pending_changes_blocking ingest path — Remote suppresses queue
    // Simulates what happens when pull_pending_changes_blocking calls atomic_write for AcceptRemote
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_pull_pending_changes_ingest_does_not_queue() {
        let temp_dir = TempDir::new().unwrap();
        let (conn, agent_uuid) = setup_db_with_agent(&temp_dir);

        // Simulate ingesting 5 files (as pull_pending_changes_blocking would for AcceptRemote)
        let files = ["SOUL.md", "MEMORY.md", "USER.md", "TOOLS.md", "RULES.md"];
        for filename in &files {
            let file_path = make_system_path(&temp_dir, &agent_uuid, filename);
            let content = format!("# {}\n\nGateway-pulled content.", filename);
            let result = atomic_write(&file_path, &content, Some(&conn), WriteOrigin::Remote);
            assert!(result.is_ok(), "Ingest of {} should succeed: {:?}", filename, result);
        }

        // After ingesting 5 files, the sync_queue must be empty — no ping-pong
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_queue", [], |row| row.get(0))
            .unwrap_or(0);
        assert_eq!(
            total, 0,
            "After ingesting {} files via pull_pending_changes_blocking path, sync_queue must be empty; got {} entries",
            files.len(),
            total
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 8: initial_sync ingest path — Remote suppresses queue
    // Simulates what happens when initial_sync calls atomic_write during onboarding
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_initial_sync_ingest_does_not_queue() {
        let temp_dir = TempDir::new().unwrap();
        let (conn, agent_uuid) = setup_db_with_agent(&temp_dir);

        // Simulate a larger batch (as initial_sync would for 224 files)
        let files = [
            "SOUL.md", "MEMORY.md", "USER.md", "TOOLS.md", "RULES.md",
            "logs/2026-05-30.md", "logs/2026-05-31.md",
        ];

        for filename in &files {
            let file_path = make_system_path(&temp_dir, &agent_uuid, filename);
            // Create parent dir in case it's nested (e.g. logs/)
            std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
            let content = format!("# {}\n\nInitial sync content.", filename);
            let result = atomic_write(&file_path, &content, Some(&conn), WriteOrigin::Remote);
            assert!(result.is_ok(), "initial_sync ingest of {} should succeed: {:?}", filename, result);
        }

        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM sync_queue", [], |row| row.get(0))
            .unwrap_or(0);
        assert_eq!(
            total, 0,
            "After initial_sync ingest of {} files, sync_queue must be empty; got {} entries",
            files.len(),
            total
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test 9: WriteOrigin enum has correct variants (compile-time check)
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_write_origin_enum_variants() {
        // These must compile and be distinct
        let local = WriteOrigin::Local;
        let remote = WriteOrigin::Remote;
        assert_ne!(local, remote, "Local and Remote must be distinct variants");

        // Clone and Copy must work
        let local2 = local;
        let remote2 = remote;
        assert_eq!(local, local2);
        assert_eq!(remote, remote2);
    }
}
