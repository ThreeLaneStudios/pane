// Integration tests for `sync_now_blocking` (#2522, ADR-0063 Stage 7a)
//
// `sync_now_blocking` is the core logic behind the `sync_now` Tauri command
// (lib.rs). It wraps `pull_pending_changes_blocking`, which internally
// handles both the inbound pull and the outbound flush (via its own tail
// call to `flush_sync_queue_blocking`) — the same primitive the periodic
// sync thread uses. `sync_now_blocking` does NOT make a second, separate
// flush call (#2560 WARN fast-follow: the old explicit second call was dead
// weight since pull's internal flush already covers it).
//
// `tauri::AppHandle` cannot be constructed in unit tests, so — following the
// convention in test_pull_pending_changes_blocking.rs — these tests call
// `sync_now_blocking` directly with `app_handle: None` and assert on the
// same observable side effects (files written, mock server hits) that the
// live command path produces.
//
// Run with: cargo test -- --test-threads=1
// (std::env::set_var is not thread-safe; --test-threads=1 is required per AGENTS.md §9)

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};
    use serial_test::serial;
    use tempfile::TempDir;
    use crate::database::DbState;
    use crate::sync_client::sync_now_blocking;

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Same schema shape as test_pull_pending_changes_blocking.rs's
    /// create_pull_test_db — kept local (not shared) so this test file has no
    /// cross-file coupling and can evolve independently.
    fn create_sync_now_test_db(
        temp_dir: &TempDir,
        gateway_url: &str,
        agent_uuid: &str,
        runtime_ref: &str,
    ) -> Connection {
        let db_path = temp_dir.path().join("sync-now-test.db");
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
                id TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
                agent TEXT NOT NULL,
                filename TEXT NOT NULL,
                content TEXT NOT NULL,
                checksum TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                retry_count INTEGER DEFAULT 0,
                next_retry_at TEXT,
                UNIQUE(agent, filename, checksum)
            );
            ",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_ref, runtime_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'openclaw', datetime('now'), datetime('now'))",
            params![
                "agent-id-sync-now",
                "Sync Now Test Agent",
                "sync-now-test-agent",
                agent_uuid,
                runtime_ref
            ],
        )
        .unwrap();

        // #2564 B1 fix (Path B): get_gateway_url() now requires an active
        // ai_agents.runtime_type='openclaw' row (set above via the INSERT) instead of
        // the retired ai_settings.provider scalar.
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('endpoint', ?1)",
            params![gateway_url],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('api_key', 'test-token-sync-now')",
            [],
        )
        .unwrap();

        conn
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test: happy path — sync_now_blocking wraps pull_pending_changes_blocking
    // (which internally covers both pull and its own tail-call flush), and
    // returns a SyncNowResult with success = true and a parseable timestamp.
    //
    // Verifies the full pull+flush contract still holds after the #2560
    // simplification: the mock pull endpoint is hit (pull ran) and the
    // function makes it to the final Ok(...) return only if pull's internal
    // flush_sync_queue_blocking call also succeeded (its Err would
    // short-circuit via `?` inside pull_pending_changes_blocking itself).
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    #[serial(pane_test_root)]
    fn test_sync_now_blocking_calls_pull_then_flush_happy_path() {
        let mut server = mockito::Server::new();

        let temp_dir = TempDir::new().unwrap();
        let fake_home = temp_dir.path().join("fake-home-sync-now");
        std::fs::create_dir_all(&fake_home).unwrap();

        let agent_uuid = "test-uuid-sync-now-happy";
        let runtime_ref = "test-oc-agent-sync-now-happy";

        // Pre-create the agent's local system dir so pull can write into it.
        let system_dir = fake_home.join("Pane").join(".pane").join("system");
        let agent_dir = system_dir.join(agent_uuid);
        std::fs::create_dir_all(&agent_dir).unwrap();

        // No pending changes — simplest happy path that still exercises the
        // real HTTP round trip (pull hits the mock server, gets an empty
        // list, then flush runs against an empty sync_queue).
        let _mock_pull = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"pending_changes":[]}"#)
            .create();

        let conn = create_sync_now_test_db(&temp_dir, &server.url(), agent_uuid, runtime_ref);
        let db_state = DbState::new(Connection::open_in_memory().unwrap());

        let original_home = std::env::var("HOME").unwrap_or_default();
        std::env::set_var("HOME", &fake_home);

        let before = chrono::Utc::now();
        let result = sync_now_blocking(&conn, None, &db_state);
        let after = chrono::Utc::now();

        std::env::set_var("HOME", &original_home);

        let sync_result = result.expect("sync_now_blocking should succeed on happy path");
        assert!(sync_result.success, "success must be true on Ok(...)");

        // Timestamp must be a valid RFC3339 string, and must fall within the
        // window the test executed in (proves it's a live timestamp, not a
        // hardcoded placeholder).
        let parsed = chrono::DateTime::parse_from_rfc3339(&sync_result.timestamp)
            .unwrap_or_else(|e| panic!("timestamp must be valid RFC3339, got {:?}: {}", sync_result.timestamp, e));
        let parsed_utc: chrono::DateTime<chrono::Utc> = parsed.into();
        assert!(
            parsed_utc >= before && parsed_utc <= after,
            "timestamp {:?} must fall within test execution window [{:?}, {:?}]",
            sync_result.timestamp, before, after
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test: pull failure propagates as an Err from sync_now_blocking. Because
    // pull_pending_changes_blocking's own tail-call flush only runs after its
    // main pull logic succeeds, a malformed pull response fails before that
    // internal flush is ever reached — sync_now_blocking has no separate
    // flush call of its own to reach either.
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    #[serial(pane_test_root)]
    fn test_sync_now_blocking_propagates_pull_error() {
        let mut server = mockito::Server::new();

        let temp_dir = TempDir::new().unwrap();
        let fake_home = temp_dir.path().join("fake-home-sync-now-err");
        std::fs::create_dir_all(&fake_home).unwrap();

        let agent_uuid = "test-uuid-sync-now-err";
        let runtime_ref = "test-oc-agent-sync-now-err";

        let system_dir = fake_home.join("Pane").join(".pane").join("system");
        let agent_dir = system_dir.join(agent_uuid);
        std::fs::create_dir_all(&agent_dir).unwrap();

        // Pull endpoint returns malformed JSON — pull_pending_changes_blocking
        // must fail to deserialize the PullResponse and return Err.
        let _mock_pull = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("not valid json")
            .create();

        let conn = create_sync_now_test_db(&temp_dir, &server.url(), agent_uuid, runtime_ref);
        let db_state = DbState::new(Connection::open_in_memory().unwrap());

        let original_home = std::env::var("HOME").unwrap_or_default();
        std::env::set_var("HOME", &fake_home);

        let result = sync_now_blocking(&conn, None, &db_state);

        std::env::set_var("HOME", &original_home);

        assert!(
            result.is_err(),
            "sync_now_blocking must return Err when pull_pending_changes_blocking fails, got {:?}",
            result
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("pull_pending_changes_blocking"),
            "error must include context identifying which stage failed (per SOUL.md §4 — no bare `?` propagation), got: {}",
            err
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test: a failed push during flush does not fail sync_now_blocking.
    //
    // Seeds sync_queue with one entry (bypassing pull, which finds no
    // pending changes) and points the gateway push endpoint at a 500 —
    // flush_sync_queue_blocking's underlying sync_push_blocking call fails,
    // updates retry bookkeeping, and returns Ok(()) itself (flush never
    // hard-fails on a single push error — see sync_client.rs). This flush
    // runs via pull_pending_changes_blocking's own internal tail call, NOT a
    // second explicit call from sync_now_blocking (removed per #2560's WARN
    // fast-follow — it was dead weight since pull's internal flush already
    // covers it). This test verifies flush actually ran (retry_count
    // incremented) and the retry-count scenario still resolves correctly on
    // the simplified code — proof the second flush was never needed.
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    #[serial(pane_test_root)]
    fn test_sync_now_blocking_runs_flush_after_pull_and_retries_failed_push() {
        let mut server = mockito::Server::new();

        let temp_dir = TempDir::new().unwrap();
        let fake_home = temp_dir.path().join("fake-home-sync-now-flush");
        std::fs::create_dir_all(&fake_home).unwrap();

        let agent_uuid = "test-uuid-sync-now-flush";
        let runtime_ref = "test-oc-agent-sync-now-flush";

        let system_dir = fake_home.join("Pane").join(".pane").join("system");
        let agent_dir = system_dir.join(agent_uuid);
        std::fs::create_dir_all(&agent_dir).unwrap();

        let _mock_pull = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"pending_changes":[]}"#)
            .create();

        // Push endpoint fails — flush_sync_queue_blocking (called internally by
        // pull_pending_changes_blocking's own tail, NOT as a separate explicit
        // call from sync_now_blocking) must record a retry, not crash, and
        // sync_now_blocking must still see Ok(()) from that flush (flush only
        // errors on a hard, non-recoverable failure — a single push 500 is
        // handled internally via retry bookkeeping).
        let _mock_push = server
            .mock("POST", "/v1/sync/push")
            .with_status(500)
            .with_body("internal error")
            .create();

        let conn = create_sync_now_test_db(&temp_dir, &server.url(), agent_uuid, runtime_ref);

        // Seed one entry directly into sync_queue so flush has something to push.
        conn.execute(
            "INSERT INTO sync_queue (id, agent, filename, content, checksum, retry_count)
             VALUES ('queued-1', ?1, 'SOUL.md', 'queued content', 'checksum123', 0)",
            params![runtime_ref],
        )
        .unwrap();

        let db_state = DbState::new(Connection::open_in_memory().unwrap());

        let original_home = std::env::var("HOME").unwrap_or_default();
        std::env::set_var("HOME", &fake_home);

        let result = sync_now_blocking(&conn, None, &db_state);

        std::env::set_var("HOME", &original_home);

        assert!(
            result.is_ok(),
            "sync_now_blocking must still succeed when a single push fails (retried, not fatal): {:?}",
            result
        );

        // Verify flush actually ran and recorded the failed push as a retry —
        // proof that pull_pending_changes_blocking's internal flush executed
        // as part of sync_now_blocking, even though sync_now_blocking itself
        // makes no separate/explicit flush call.
        let retry_count: i64 = conn
            .query_row(
                "SELECT retry_count FROM sync_queue WHERE id = 'queued-1'",
                [],
                |row| row.get(0),
            )
            .expect("queued entry must still exist with an updated retry_count");
        assert_eq!(
            retry_count, 1,
            "flush_sync_queue_blocking must have incremented retry_count after the mocked 500, proving flush ran as part of sync_now_blocking"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test: no gateway configured — both pull and flush treat this as a
    // no-op success (matches the existing contract of both underlying
    // functions), so sync_now_blocking must also succeed rather than error.
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    #[serial(pane_test_root)]
    fn test_sync_now_blocking_no_gateway_configured_is_a_noop_success() {
        let temp_dir = TempDir::new().unwrap();
        let fake_home = temp_dir.path().join("fake-home-sync-now-noop");
        std::fs::create_dir_all(&fake_home).unwrap();

        let db_path = temp_dir.path().join("sync-now-noop-test.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS ai_agents (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, slug TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'sub', is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1, workspace_path TEXT NOT NULL,
                runtime_ref TEXT, runtime_type TEXT, source TEXT NOT NULL DEFAULT 'manual',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now')), owner_id TEXT
            );
            CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY, value TEXT NOT NULL,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS sync_queue (
                id TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
                agent TEXT NOT NULL, filename TEXT NOT NULL,
                content TEXT NOT NULL, checksum TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                retry_count INTEGER DEFAULT 0, next_retry_at TEXT,
                UNIQUE(agent, filename, checksum)
            );
            ",
        )
        .unwrap();
        // Deliberately no ai_settings rows — no provider/endpoint/api_key configured.

        let db_state = DbState::new(Connection::open_in_memory().unwrap());

        let original_home = std::env::var("HOME").unwrap_or_default();
        std::env::set_var("HOME", &fake_home);

        let result = sync_now_blocking(&conn, None, &db_state);

        std::env::set_var("HOME", &original_home);

        let sync_result = result.expect(
            "sync_now_blocking must succeed (no-op) when no gateway is configured, matching \
             pull_pending_changes_blocking's and flush_sync_queue_blocking's existing no-gateway contract",
        );
        assert!(sync_result.success);
    }
}
