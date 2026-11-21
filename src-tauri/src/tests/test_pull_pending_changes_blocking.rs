// Integration test: pull_pending_changes_blocking LWW tie-break (ADR-0042)
//
// Verifies the LIVE path: lib.rs:945 → pull_pending_changes_blocking → conflict_resolution::resolve()
// On a tie (queued_at == local mtime), Pane wins (KeepLocal) per ADR-0042.
// Before PR #1372, the blocking function had an inline "gateway wins" tie-break
// that bypassed the shared module entirely. This test would have FAILED on 0.65.81.
//
// Run with: cargo test -- --test-threads=1
// (std::env::set_var is not thread-safe; --test-threads=1 is required per AGENTS.md §9)

#[cfg(test)]
mod tests {
    use rusqlite::{Connection, params};
    use serial_test::serial;
    use tempfile::TempDir;
    use crate::database::DbState;
    use crate::sync_client::pull_pending_changes_blocking;

    // ─────────────────────────────────────────────────────────────────────────
    // Helpers
    // ─────────────────────────────────────────────────────────────────────────

    fn create_pull_test_db(temp_dir: &TempDir, gateway_url: &str) -> (Connection, String) {
        let db_path = temp_dir.path().join("pull-test.db");
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

        let agent_uuid = "test-uuid-1299-tie-break";
        let runtime_ref = "test-oc-agent-1299";

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_ref, runtime_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'openclaw', datetime('now'), datetime('now'))",
            params![
                "agent-id-1299-tie",
                "Tie Test Agent",
                "tie-test-agent",
                agent_uuid,
                runtime_ref
            ],
        )
        .unwrap();

        // #2564 B1 fix (Path B): get_gateway_url() now requires an active
        // ai_agents.runtime_type='openclaw' row (set above via the INSERT) instead of
        // the retired ai_settings.provider scalar. "endpoint" is still read from
        // ai_settings directly.
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('endpoint', ?1)",
            params![gateway_url],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('api_key', 'test-token-tie-1299')",
            [],
        )
        .unwrap();

        (conn, agent_uuid.to_string())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test: tie-break resolves to KeepLocal (Pane wins) — ADR-0042
    //
    // This is the real fix for #1299. Before PR #1372, the inline block at
    // sync_client.rs:939-970 would set should_ingest=true on tie ("gateway wins").
    // After the fix, the shared conflict_resolution::resolve() returns KeepLocal.
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    #[serial(pane_test_root)]
    fn test_pull_pending_changes_blocking_tie_keeps_local() {
        // NOTE: std::env::set_var is not thread-safe. Run with --test-threads=1.
        let mut server = mockito::Server::new();

        let temp_dir = TempDir::new().unwrap();
        let fake_home = temp_dir.path().join("fake-home");
        std::fs::create_dir_all(&fake_home).unwrap();

        let agent_uuid = "test-uuid-1299-tie-break";
        let runtime_ref = "test-oc-agent-1299";
        let filename = "SOUL.md"; // must be on the ADR-0041 allowlist

        let local_content = "# Local SOUL.md — Pane wins on tie";
        let remote_content = "# Remote SOUL.md — gateway should NOT overwrite on tie";

        // Create local file under the fake home's system directory
        let system_dir = fake_home
            .join("Pane")
            .join(".pane")
            .join("system");
        let agent_dir = system_dir.join(agent_uuid);
        std::fs::create_dir_all(&agent_dir).unwrap();
        let file_path = agent_dir.join(filename);
        std::fs::write(&file_path, local_content).unwrap();

        // Read the file's actual mtime — this is the tie timestamp
        let mtime = std::fs::metadata(&file_path).unwrap().modified().unwrap();
        let mtime_utc: chrono::DateTime<chrono::Utc> = mtime.into();
        // Use nanosecond precision so parse_from_rfc3339 → DateTime<Utc> round-trips exactly.
        let queued_at = mtime_utc
            .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);

        // Build the mock pull response: one pending change with queued_at == local mtime (tie)
        let change_id = "change-tie-1299";
        let pull_body = format!(
            r#"{{"pending_changes":[{{"id":"{change_id}","oc_agent_id":"{runtime_ref}","filename":"{filename}","content":"{remote_escaped}","checksum":"deadbeef","queued_at":"{queued_at}"}}]}}"#,
            change_id = change_id,
            runtime_ref = runtime_ref,
            filename = filename,
            remote_escaped = remote_content.replace('"', "\\\""),
            queued_at = queued_at,
        );

        // Mock GET /v1/sync/pull?... → return the tie scenario
        let _mock_pull = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(pull_body)
            .create();

        // Mock DELETE /v1/sync/pull/confirm → OK
        // (even for KeepLocal, the change is marked processed so gateway doesn't re-send)
        let _mock_confirm = server
            .mock("DELETE", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true}"#)
            .create();

        // Set up test DB with mock server URL
        let (conn, _) = create_pull_test_db(&temp_dir, &server.url());

        // DbState is required by the signature but ignored inside flush_sync_queue_blocking
        // (sync_queue is empty after KeepLocal — no write, no queue entry, no push request)
        let db_state = DbState::new(Connection::open_in_memory().unwrap());

        // Redirect HOME so pull_pending_changes_blocking finds the file under fake_home.
        // #3057: get_pane_root() now refuses to resolve via dirs::home_dir() at all
        // inside a test binary — PANE_TEST_ROOT must be set explicitly too, pointed
        // at the exact path the OLD home.join("Pane") fallback would have produced
        // with HOME redirected, so this test's on-disk fixture layout (already built
        // under fake_home.join("Pane")...) resolves identically.
        let original_home = std::env::var("HOME").unwrap_or_default();
        std::env::set_var("HOME", &fake_home);
        std::env::set_var("PANE_TEST_ROOT", fake_home.join("Pane"));

        // ── INVOKE THE LIVE PATH ──────────────────────────────────────────────
        // lib.rs:945 calls this function. After PR #1372 it routes through
        // conflict_resolution::resolve() which returns KeepLocal on tie.
        // None for app_handle — no banner emission needed in this tie-break test.
        let result = pull_pending_changes_blocking(&conn, None, &db_state);

        // Restore HOME before any assertions (ensures cleanup even on panic)
        std::env::set_var("HOME", &original_home);

        // ── ASSERTIONS ───────────────────────────────────────────────────────
        assert!(
            result.is_ok(),
            "pull_pending_changes_blocking should succeed: {:?}",
            result
        );

        let actual = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            actual, local_content,
            "ADR-0042: on LWW tie, Pane wins (KeepLocal). \
             Local content must NOT be overwritten by gateway. \
             Got: {:?}",
            actual
        );

        // Verify the remote content was NOT written (belt-and-suspenders)
        assert_ne!(
            actual, remote_content,
            "Remote content must not have been ingested on tie"
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test: remote-newer scenario still works (AcceptRemote) — regression guard
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    #[serial(pane_test_root)]
    fn test_pull_pending_changes_blocking_remote_newer_accepts_remote() {
        let mut server = mockito::Server::new();

        let temp_dir = TempDir::new().unwrap();
        let fake_home = temp_dir.path().join("fake-home-newer");
        std::fs::create_dir_all(&fake_home).unwrap();

        let agent_uuid = "test-uuid-1299-newer";
        let runtime_ref = "test-oc-agent-1299-newer";
        let filename = "SOUL.md";
        let local_content = "# Old local SOUL.md";
        let remote_content = "# Newer remote SOUL.md — should overwrite";

        // Create local file with an old mtime (just written = now)
        let system_dir = fake_home.join("Pane").join(".pane").join("system");
        let agent_dir = system_dir.join(agent_uuid);
        std::fs::create_dir_all(&agent_dir).unwrap();
        let file_path = agent_dir.join(filename);
        std::fs::write(&file_path, local_content).unwrap();

        // Remote timestamp is clearly in the future relative to local mtime
        let remote_ts = chrono::Utc::now() + chrono::Duration::hours(1);
        let queued_at = remote_ts.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);

        let change_id = "change-newer-1299";
        let pull_body = format!(
            r#"{{"pending_changes":[{{"id":"{change_id}","oc_agent_id":"{runtime_ref}","filename":"{filename}","content":"{remote_escaped}","checksum":"deadbeef","queued_at":"{queued_at}"}}]}}"#,
            change_id = change_id,
            runtime_ref = runtime_ref,
            filename = filename,
            remote_escaped = remote_content.replace('"', "\\\""),
            queued_at = queued_at,
        );

        let _mock_pull = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(pull_body)
            .create();

        // We also need to mock the push endpoint since after AcceptRemote the
        // file is written (WriteOrigin::Remote suppresses sync_queue, but
        // flush_sync_queue_blocking is still called and checks sync_queue).
        // sync_queue is empty (Remote write doesn't queue), so no push is made.
        let _mock_confirm = server
            .mock("DELETE", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true}"#)
            .create();

        let (conn, _) = {
            // reuse create_pull_test_db but with the new agent UUID
            let db_path = temp_dir.path().join("pull-newer-test.db");
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
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_ref, runtime_type, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'openclaw', datetime('now'), datetime('now'))",
                params!["agent-id-1299-newer", "Newer Agent", "newer-agent", agent_uuid, runtime_ref],
            )
            .unwrap();
            // #2564 B1 fix (Path B): runtime_type='openclaw' set on the INSERT above,
            // replacing the retired ai_settings.provider write.
            conn.execute("INSERT INTO ai_settings (key, value) VALUES ('endpoint', ?1)", params![server.url()]).unwrap();
            conn.execute("INSERT INTO ai_settings (key, value) VALUES ('api_key', 'test-token-newer')", []).unwrap();
            (conn, agent_uuid.to_string())
        };

        let db_state = DbState::new(Connection::open_in_memory().unwrap());

        // #3057: see the tie_keeps_local test above for why both HOME and
        // PANE_TEST_ROOT must be set here now.
        let original_home = std::env::var("HOME").unwrap_or_default();
        std::env::set_var("HOME", &fake_home);
        std::env::set_var("PANE_TEST_ROOT", fake_home.join("Pane"));

        let result = pull_pending_changes_blocking(&conn, None, &db_state);

        std::env::set_var("HOME", &original_home);

        assert!(result.is_ok(), "pull_pending_changes_blocking should succeed: {:?}", result);

        let actual = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(
            actual, remote_content,
            "When remote is newer, AcceptRemote should write the remote content. Got: {:?}",
            actual
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test: app_handle forwarding — source-level contract guard for #1370
    //
    // Asserts that pull_pending_changes_blocking forwards its app_handle parameter
    // to the internal flush_sync_queue_blocking call, so the sync-failure banner
    // fires when retry_count >= 5. Source-grep catches this regression with zero
    // production footprint (no #[cfg(test)] instrumentation needed in prod code).
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    #[serial(pane_test_root)]
    fn test_pull_pending_changes_blocking_forwards_app_handle_to_flush_source_contract() {
        // Source-level contract test (#1370). Asserts that the internal call to
        // flush_sync_queue_blocking inside pull_pending_changes_blocking forwards
        // the caller's app_handle parameter rather than passing a hardcoded None.
        //
        // Why source-grep instead of runtime probe: Tauri's AppHandle cannot be
        // constructed in unit tests, and adding #[cfg(test)] instrumentation to
        // production code is scaffolding-rot we explicitly reject. A source-grep
        // test catches the regression with zero production footprint.

        // Resolve the path relative to CARGO_MANIFEST_DIR so the test runs from
        // any workdir (including CI).
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set during `cargo test`");
        let source_path = std::path::Path::new(&manifest_dir).join("src").join("sync_client.rs");
        let source = std::fs::read_to_string(&source_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {}", source_path.display(), e));

        // Locate the function body of pull_pending_changes_blocking.
        let fn_marker = "pub fn pull_pending_changes_blocking(";
        let fn_start = source.find(fn_marker).unwrap_or_else(|| {
            panic!(
                "pull_pending_changes_blocking signature not found in {}; \
                 did the function rename? Update this test to match.",
                source_path.display()
            )
        });

        // The function body extends until the matching closing brace at column 0.
        // Approximate by searching forward from fn_start until we hit "\n}\n"
        // (top-level closing brace followed by newline). This is a heuristic but
        // sufficient for a flat-formatted file like sync_client.rs.
        let body_end_rel = source[fn_start..]
            .find("\n}\n")
            .expect("could not locate end of pull_pending_changes_blocking; reformat?");
        let fn_body = &source[fn_start..fn_start + body_end_rel];

        // Find every call to flush_sync_queue_blocking within the function body.
        // Strip line comments before scanning so inline // notes don't skew results.
        let body_no_line_comments: String = fn_body
            .lines()
            .map(|line| {
                if let Some(idx) = line.find("//") {
                    &line[..idx]
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let call_marker = "flush_sync_queue_blocking(";
        let call_sites: Vec<&str> = body_no_line_comments
            .match_indices(call_marker)
            .map(|(idx, _)| {
                // Capture from the call start through the matching close paren.
                let after = &body_no_line_comments[idx..];
                let close_rel = after
                    .find(')')
                    .expect("flush_sync_queue_blocking call site missing close paren");
                &after[..=close_rel]
            })
            .collect();

        assert!(
            !call_sites.is_empty(),
            "pull_pending_changes_blocking must call flush_sync_queue_blocking internally \
             (per ADR-0042 and #1370 — pull subsumes flush). No call site found in function body."
        );

        // Contract: every call site must forward `app_handle`, never pass a hardcoded
        // `None`. Hardcoded `None` would silently break the sync-failure banner
        // (src/routes/+layout.svelte:14) — exactly the regression #1370 prevented.
        for (i, call) in call_sites.iter().enumerate() {
            let normalized = call.replace(char::is_whitespace, "");
            assert!(
                !normalized.contains(",None,"),
                "#1370 regression: pull_pending_changes_blocking call #{} to \
                 flush_sync_queue_blocking passes hardcoded `None` for app_handle. \
                 Must forward the caller's `app_handle` parameter so the sync-failure \
                 banner reaches the frontend (src/routes/+layout.svelte:14). \
                 Offending call: {}",
                i,
                call.trim()
            );
            assert!(
                normalized.contains("app_handle"),
                "#1370 regression: pull_pending_changes_blocking call #{} to \
                 flush_sync_queue_blocking does not appear to forward `app_handle`. \
                 The internal flush call must pass the function's `app_handle` \
                 parameter. Offending call: {}",
                i,
                call.trim()
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Test: path-traversal filenames are rejected by safe_path_join (#1300)
    //
    // Verifies INVARIANT §22C.18 #15: sync ingest MUST NOT write outside
    // `$HOME/Pane/.pane/system/{uuid}/`. The centralized `file_ops::safe_path_join`
    // helper is the single enforcement point.
    //
    // Mock gateway returns four pending changes:
    //   c1: ../../etc/passwd   (parent traversal)
    //   c2: ../other-agent/SOUL.md (relative escape)
    //   c3: /abs/evil.md       (absolute path)
    //   c4: SOUL.md            (legitimate, on IDENTITY_WHITELIST)
    //
    // Expected:
    //   c1, c2, c3 — skipped (no file written, traversal rejected by safe_path_join)
    //   c4          — written (happy path unbroken)
    //   return value — Ok(()) (escapes cause continue, not fatal Err)
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    #[serial(pane_test_root)]
    fn test_pull_pending_changes_blocking_rejects_path_traversal() {
        // NOTE: std::env::set_var is not thread-safe. Run with --test-threads=1 (#serial).
        let mut server = mockito::Server::new();

        let temp_dir = TempDir::new().unwrap();
        let fake_home = temp_dir.path().join("fake-home-1300");
        std::fs::create_dir_all(&fake_home).unwrap();

        let agent_uuid = "test-uuid-1300-traversal";
        let runtime_ref = "test-oc-agent-1300";

        // Set up the agent's workspace dir under the fake HOME
        let system_dir = fake_home.join("Pane").join(".pane").join("system");
        let agent_dir = system_dir.join(agent_uuid);
        std::fs::create_dir_all(&agent_dir).unwrap();

        // Timestamp in the past so any traversal-borne file would be "newer" (AcceptRemote)
        // This eliminates LWW as a reason for skipping — only safe_path_join guards matter.
        let future_ts = chrono::Utc::now() + chrono::Duration::hours(2);
        let queued_at = future_ts.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);

        // Four pending changes: 3 malicious + 1 legitimate
        let pull_body = format!(
            r#"{{"pending_changes":[
              {{"id":"c1","oc_agent_id":"{oc}","filename":"../../etc/passwd","content":"pwned","checksum":"aa","queued_at":"{ts}"}},
              {{"id":"c2","oc_agent_id":"{oc}","filename":"../other-agent/SOUL.md","content":"overwritten","checksum":"bb","queued_at":"{ts}"}},
              {{"id":"c3","oc_agent_id":"{oc}","filename":"/abs/evil.md","content":"abs","checksum":"cc","queued_at":"{ts}"}},
              {{"id":"c4","oc_agent_id":"{oc}","filename":"SOUL.md","content":"Legit SOUL content","checksum":"dd","queued_at":"{ts}"}}
            ]}}"#,
            oc = runtime_ref,
            ts = queued_at,
        );

        let _mock_pull = server
            .mock("GET", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(pull_body)
            .create();

        let _mock_confirm = server
            .mock("DELETE", mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true}"#)
            .create();

        // Build the test DB
        let db_path = temp_dir.path().join("pull-traversal-test.db");
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

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_ref, runtime_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'openclaw', datetime('now'), datetime('now'))",
            params![
                "agent-id-1300",
                "Traversal Test Agent",
                "traversal-test-agent",
                agent_uuid,
                runtime_ref
            ],
        )
        .unwrap();
        // #2564 B1 fix (Path B): runtime_type='openclaw' set on the INSERT above,
        // replacing the retired ai_settings.provider write.
        conn.execute("INSERT INTO ai_settings (key, value) VALUES ('endpoint', ?1)", params![server.url()]).unwrap();
        conn.execute("INSERT INTO ai_settings (key, value) VALUES ('api_key', 'test-token-1300')", []).unwrap();

        let db_state = DbState::new(Connection::open_in_memory().unwrap());

        // #3057: see the tie_keeps_local test above for why both HOME and
        // PANE_TEST_ROOT must be set here now.
        let original_home = std::env::var("HOME").unwrap_or_default();
        std::env::set_var("HOME", &fake_home);
        std::env::set_var("PANE_TEST_ROOT", fake_home.join("Pane"));

        // ── INVOKE THE LIVE PATH ───────────────────────────────────────────────────────────
        let result = pull_pending_changes_blocking(&conn, None, &db_state);

        std::env::set_var("HOME", &original_home);

        // ── ASSERTIONS ───────────────────────────────────────────────────────────────

        // 1. Function returns Ok(()) — traversal filenames cause `continue`, not fatal Err
        assert!(
            result.is_ok(),
            "pull_pending_changes_blocking must return Ok(()) even when traversal filenames are present — they should be skipped, not crash the pull: {:?}",
            result
        );

        // 2. No file written outside system_dir/{agent_uuid}/
        //    Check that the traversal targets do NOT exist anywhere under fake_home.
        let fake_etc_passwd = fake_home.join("etc").join("passwd");
        assert!(
            !fake_etc_passwd.exists(),
            "SECURITY: ../../etc/passwd traversal must not write outside system_dir. \
             File unexpectedly found at: {}",
            fake_etc_passwd.display()
        );

        // c2: ../other-agent/SOUL.md — would land at system_dir/other-agent/SOUL.md
        let sibling_soul = system_dir.join("other-agent").join("SOUL.md");
        assert!(
            !sibling_soul.exists(),
            "SECURITY: ../other-agent/SOUL.md traversal must not write a sibling agent directory. \
             File unexpectedly found at: {}",
            sibling_soul.display()
        );

        // c3: /abs/evil.md — absolute path; safe_path_join rejects at is_absolute() check
        // (On macOS/Linux, /abs/evil.md lives at the real root, not under fake_home.)
        // We verify it was NOT written into the agent_dir (the only safe write target).
        let abs_in_agent = agent_dir.join("abs").join("evil.md");
        assert!(
            !abs_in_agent.exists(),
            "SECURITY: /abs/evil.md (absolute path) must not be written into agent_dir: {}",
            abs_in_agent.display()
        );

        // 3. The legitimate c4 (SOUL.md) IS written — happy path unbroken
        let soul_path = agent_dir.join("SOUL.md");
        assert!(
            soul_path.exists(),
            "Legitimate SOUL.md must be written to the agent_dir after traversal-rejection loop. \
             Path not found: {}",
            soul_path.display()
        );
        let soul_content = std::fs::read_to_string(&soul_path).unwrap();
        assert_eq!(
            soul_content, "Legit SOUL content",
            "SOUL.md content must match the legitimate change payload. Got: {:?}",
            soul_content
        );
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Unit tests for file_ops::safe_path_join (#1300)
    //
    // Verifies the helper itself under all documented cases:
    //   - empty segment
    //   - absolute path (/etc/passwd)
    //   - parent-dir traversal (../foo)
    //   - nested parent-dir (a/b/../../etc)
    //   - Windows separator on Unix (..\.\foo — single component, not ParentDir)
    //   - legitimate segment (SOUL.md)
    //   - legitimate with dots (file..bak)
    //   - legitimate subdirectory (memory/2026-06-03.md)
    // ─────────────────────────────────────────────────────────────────────────
    #[test]
    fn test_safe_path_join_empty_segment() {
        let base = std::path::PathBuf::from("/home/user/pane/system/uuid123");
        let result = crate::file_ops::safe_path_join(&base, "");
        assert!(
            result.is_err(),
            "Empty segment must return Err, got Ok({:?})",
            result
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("empty"),
            "Error must mention 'empty', got: {}",
            err
        );
    }

    #[test]
    fn test_safe_path_join_absolute_path() {
        let base = std::path::PathBuf::from("/home/user/pane/system/uuid123");
        let result = crate::file_ops::safe_path_join(&base, "/etc/passwd");
        assert!(
            result.is_err(),
            "Absolute segment must return Err, got Ok({:?})",
            result
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("absolute"),
            "Error must mention 'absolute', got: {}",
            err
        );
    }

    #[test]
    fn test_safe_path_join_parent_dir_traversal() {
        let base = std::path::PathBuf::from("/home/user/pane/system/uuid123");
        let result = crate::file_ops::safe_path_join(&base, "../foo");
        assert!(
            result.is_err(),
            "Parent-dir traversal must return Err, got Ok({:?})",
            result
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("parent-dir"),
            "Error must mention 'parent-dir', got: {}",
            err
        );
    }

    #[test]
    fn test_safe_path_join_nested_parent_dir() {
        let base = std::path::PathBuf::from("/home/user/pane/system/uuid123");
        // a/b/../../etc normalizes to ../etc which escapes base; ParentDir component present
        let result = crate::file_ops::safe_path_join(&base, "a/b/../../etc");
        assert!(
            result.is_err(),
            "Nested parent-dir traversal (a/b/../../etc) must return Err, got Ok({:?})",
            result
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("parent-dir"),
            "Error must mention 'parent-dir', got: {}",
            err
        );
    }

    #[test]
    fn test_safe_path_join_windows_separator_unix_behavior() {
        // On Unix, `..\foo` is a single path component (the literal string `..\foo`),
        // NOT a ParentDir component. The function treats it as a normal filename segment.
        // The starts_with belt-and-suspenders check ensures it stays within base.
        // This test documents the OS-native behavior — the allowlist (ADR-0041 \u00a722C.8.5)
        // provides the complementary defense by rejecting backslash-containing filenames.
        let base = std::path::PathBuf::from("/home/user/pane/system/uuid123");
        let result = crate::file_ops::safe_path_join(&base, "..\\foo");
        // On Unix: this is allowed by safe_path_join (backslash is not a separator).
        // The resulting path is base/"..\\foo" — a literal filename with a backslash.
        // We only assert that it doesn't escape base (starts_with check passes).
        if let Ok(ref joined) = result {
            assert!(
                joined.starts_with(&base),
                "Windows-separator segment must not escape base. Got: {}",
                joined.display()
            );
        }
        // We don't assert Ok vs Err here — both are acceptable; what matters is
        // that the joined path, if Ok, stays inside base.
    }

    #[test]
    fn test_safe_path_join_legitimate_segment() {
        let base = std::path::PathBuf::from("/home/user/pane/system/uuid123");
        let result = crate::file_ops::safe_path_join(&base, "SOUL.md");
        assert!(
            result.is_ok(),
            "Legitimate segment 'SOUL.md' must return Ok, got Err({:?})",
            result
        );
        let joined = result.unwrap();
        assert_eq!(
            joined,
            std::path::PathBuf::from("/home/user/pane/system/uuid123/SOUL.md")
        );
    }

    #[test]
    fn test_safe_path_join_legitimate_with_dots() {
        // `file..bak` is a valid filename (two dots, no slash); safe_path_join must allow it.
        // The old `contains("..")` check incorrectly rejected this (false positive).
        let base = std::path::PathBuf::from("/home/user/pane/system/uuid123");
        let result = crate::file_ops::safe_path_join(&base, "file..bak");
        assert!(
            result.is_ok(),
            "'file..bak' is a legitimate filename (two dots, no slash, no ParentDir component). \
             Must return Ok. Old contains('..') check falsely rejected this; safe_path_join must not. \
             Got Err({:?})",
            result
        );
        let joined = result.unwrap();
        assert_eq!(
            joined,
            std::path::PathBuf::from("/home/user/pane/system/uuid123/file..bak")
        );
    }

    #[test]
    fn test_safe_path_join_subdirectory_segment() {
        let base = std::path::PathBuf::from("/home/user/pane/system/uuid123");
        let result = crate::file_ops::safe_path_join(&base, "memory/2026-06-03.md");
        assert!(
            result.is_ok(),
            "Legitimate subdirectory segment 'memory/2026-06-03.md' must return Ok, got Err({:?})",
            result
        );
        let joined = result.unwrap();
        assert_eq!(
            joined,
            std::path::PathBuf::from("/home/user/pane/system/uuid123/memory/2026-06-03.md")
        );
    }
}
