// §29 Dual-Provider Routing Tests

#[cfg(test)]
mod tests {
    use crate::ai::provider::get_internal_provider;
    use crate::ai::provider::AiProvider;
    use crate::database::{DbState, set_ai_setting};
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    /// Helper to create a test database with ai_settings + ai_agents tables.
    /// Each call returns a brand-new in-memory DB — no shared singleton.
    ///
    /// `ai_agents` is required post-#2586 (C2): `get_internal_provider`'s Step 2
    /// fallback now resolves via `runtime_resolution::derive_active_provider_label`,
    /// which queries `ai_agents.runtime_type` (not the retired `ai_settings.provider`
    /// scalar) — see `test_get_internal_provider_fallback_to_primary` and
    /// `test_get_internal_provider_gateway_primary_no_internal` below.
    fn setup_test_db() -> DbState {
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ai_agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                workspace_path TEXT NOT NULL,
                runtime_type TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );",
        )
        .unwrap();

        DbState::new(conn)
    }

    /// Seed a designated-default `ai_agents` row with the given `runtime_type` —
    /// what `derive_active_provider_label` actually resolves against post-#2564/
    /// #2586, replacing the old `set_ai_setting(&conn, "provider", ...)` seed.
    fn seed_default_agent(conn: &rusqlite::Connection, runtime_type: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, created_at, updated_at)
             VALUES ('agent-primary', 'Primary', 'primary', 1, 1, 'primary', ?1, ?2, ?2)",
            rusqlite::params![runtime_type, now],
        )
        .unwrap();
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_with_internal_config() {
        // Fresh empty keychain — nothing from credential_migration tests leaks in.
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        // C50 (#3171 PR 1): the openrouter key now lives in Provider
        // Connections' own keychain slot (`ai_openrouter_key`), not the
        // retired shared `internal_api_key` slot — this seeds the slot
        // `get_internal_provider`'s real resolution path (`read_provider_key`)
        // actually reads.
        set_ai_setting(&conn, "internal_provider", "openrouter").unwrap();
        crate::keychain::store_password("ai_openrouter_key", "sk-or-test-key")
            .expect("store_password must succeed against MockKeychain");

        drop(conn); // Release lock before async call

        // Should return internal provider
        let result = get_internal_provider(&db).await;
        assert!(result.is_ok(), "Should return internal provider");

        match result.unwrap() {
            AiProvider::OpenRouter { api_key } => {
                assert_eq!(api_key, "sk-or-test-key");
            }
            _ => panic!("Expected OpenRouter provider"),
        }
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_fallback_to_primary() {
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        // Set only primary provider (no internal provider).
        // #2586 C2 fix: was `set_ai_setting(&conn, "provider", "openrouter")` — the
        // retired `ai_settings.provider` scalar (deleted for real by #2564/B1).
        // `get_internal_provider`'s Step 2 fallback now resolves via
        // `derive_active_provider_label`, which reads `ai_agents.runtime_type` —
        // seed a designated-default agent row instead.
        seed_default_agent(&conn, "openrouter");
        set_ai_setting(&conn, "api_key", "sk-or-primary-key").unwrap();

        drop(conn);

        // Should fallback to primary provider
        let result = get_internal_provider(&db).await;
        assert!(result.is_ok(), "Should fallback to primary provider");

        match result.unwrap() {
            AiProvider::OpenRouter { api_key } => {
                assert_eq!(api_key, "sk-or-primary-key");
            }
            _ => panic!("Expected OpenRouter provider"),
        }
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_gateway_primary_no_internal() {
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        // Set primary provider as gateway (openclaw).
        // #2586 C2 fix: was `set_ai_setting(&conn, "provider", "openclaw")` — the
        // retired `ai_settings.provider` scalar. `derive_active_provider_label`
        // resolves "openclaw" via `has_active_openclaw_agent` (any active
        // `runtime_type='openclaw'` row), so seed that instead.
        seed_default_agent(&conn, "openclaw");
        set_ai_setting(&conn, "api_key", "gateway-token").unwrap();
        set_ai_setting(&conn, "endpoint", "http://localhost:18789").unwrap();

        // No internal provider set

        drop(conn);

        // Gateway CAN be used for internal calls (Phase 2 fix)
        let result = get_internal_provider(&db).await;
        assert!(result.is_ok(), "Gateway can now be used as internal provider");

        match result.unwrap() {
            AiProvider::OpenClaw { base_url, token, .. } => {
                assert_eq!(base_url, "http://localhost:18789/v1");
                assert_eq!(token, "gateway-token");
            }
            _ => panic!("Expected OpenClaw provider"),
        }
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_anthropic() {
        // Isolated keychain — ensures "sk-ant-correct-internal" written by the
        // credential_migration State C test never leaks into read_provider_key here.
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        // C50 (#3171 PR 1): seed the anthropic Provider Connection's own
        // keychain slot, not the retired shared `internal_api_key` slot.
        set_ai_setting(&conn, "internal_provider", "anthropic").unwrap();
        crate::keychain::store_password("ai_anthropic_key", "sk-ant-test-key")
            .expect("store_password must succeed against MockKeychain");

        drop(conn);

        let result = get_internal_provider(&db).await;
        assert!(result.is_ok());

        match result.unwrap() {
            AiProvider::Anthropic { api_key } => {
                assert_eq!(api_key, "sk-ant-test-key");
            }
            _ => panic!("Expected Anthropic provider"),
        }
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_ollama() {
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        set_ai_setting(&conn, "internal_provider", "ollama").unwrap();
        set_ai_setting(&conn, "internal_endpoint", "http://localhost:11434").unwrap();

        drop(conn);

        let result = get_internal_provider(&db).await;
        assert!(result.is_ok());

        match result.unwrap() {
            AiProvider::Ollama { endpoint } => {
                assert_eq!(endpoint, "http://localhost:11434");
            }
            _ => panic!("Expected Ollama provider"),
        }
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_priority() {
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        // Set both internal provider AND primary provider. C50 (#3171 PR 1):
        // the internal openrouter key now lives in Provider Connections' own
        // keychain slot, not the retired shared `internal_api_key` slot.
        set_ai_setting(&conn, "internal_provider", "openrouter").unwrap();
        crate::keychain::store_password("ai_openrouter_key", "sk-or-internal")
            .expect("store_password must succeed against MockKeychain");
        set_ai_setting(&conn, "provider", "anthropic").unwrap();
        set_ai_setting(&conn, "api_key", "sk-ant-primary").unwrap();

        drop(conn);

        // Should prioritize internal provider over primary
        let result = get_internal_provider(&db).await;
        assert!(result.is_ok());

        match result.unwrap() {
            AiProvider::OpenRouter { api_key } => {
                assert_eq!(api_key, "sk-or-internal", "Should use internal provider, not primary");
            }
            _ => panic!("Expected OpenRouter (internal) provider"),
        }
    }

    /// Matt's named mutation (C50 / #3171 PR 1): "remove a feature's key reuse
    /// → it must FAIL, not fall back to an empty key." `internal_provider` is
    /// set to openrouter, but NO Provider Connection key exists for it — the
    /// real resolution path must return `Err`, never `Ok(AiProvider::OpenRouter
    /// { api_key: "" })`. A silent empty-key fallback would be indistinguishable
    /// from a provider outage once a downstream caller sent it as a Bearer
    /// token and got a 401.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_missing_connection_key_errors_not_empty_key() {
        let _kc = KeychainGuard::set(); // empty mock keychain — no ai_openrouter_key

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();
        set_ai_setting(&conn, "internal_provider", "openrouter").unwrap();
        drop(conn);

        let result = get_internal_provider(&db).await;
        let err = result.expect_err(
            "missing openrouter connection key must error — never resolve to an empty-key provider",
        );
        assert!(
            err.contains("not connected"),
            "error must name the real problem (not connected), got: {}",
            err
        );
    }

    /// Same mutation, anthropic arm.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_anthropic_missing_connection_key_errors() {
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();
        set_ai_setting(&conn, "internal_provider", "anthropic").unwrap();
        drop(conn);

        let result = get_internal_provider(&db).await;
        let err = result.expect_err(
            "missing anthropic connection key must error — never resolve to an empty-key provider",
        );
        assert!(
            err.contains("not connected"),
            "error must name the real problem (not connected), got: {}",
            err
        );
    }

    /// Closes the exact conflation bug C50 fixed: switching `internal_provider`
    /// from openrouter to anthropic and back must read each provider's OWN
    /// connection key, never a value left over from the other provider's slot
    /// (the old `ai_internal_key` shared-account bug this migration retires).
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_switching_providers_reads_distinct_connection_keys() {
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        {
            let conn = db.0.lock().unwrap();
            crate::keychain::store_password("ai_openrouter_key", "sk-or-DISTINCT-key")
                .expect("store_password must succeed against MockKeychain");
            crate::keychain::store_password("ai_anthropic_key", "sk-ant-DISTINCT-key")
                .expect("store_password must succeed against MockKeychain");
            set_ai_setting(&conn, "internal_provider", "openrouter").unwrap();
        }

        match get_internal_provider(&db).await.expect("openrouter must resolve") {
            AiProvider::OpenRouter { api_key } => assert_eq!(api_key, "sk-or-DISTINCT-key"),
            _ => panic!("Expected OpenRouter provider"),
        }

        {
            let conn = db.0.lock().unwrap();
            set_ai_setting(&conn, "internal_provider", "anthropic").unwrap();
        }

        match get_internal_provider(&db).await.expect("anthropic must resolve") {
            AiProvider::Anthropic { api_key } => assert_eq!(
                api_key, "sk-ant-DISTINCT-key",
                "switching providers must read the NEW provider's own connection key, \
                 never the previous provider's value"
            ),
            _ => panic!("Expected Anthropic provider"),
        }

        // Switch back — must read openrouter's key again, not anthropic's.
        {
            let conn = db.0.lock().unwrap();
            set_ai_setting(&conn, "internal_provider", "openrouter").unwrap();
        }
        match get_internal_provider(&db).await.expect("openrouter must resolve again") {
            AiProvider::OpenRouter { api_key } => assert_eq!(api_key, "sk-or-DISTINCT-key"),
            _ => panic!("Expected OpenRouter provider"),
        }
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_no_config() {
        let _kc = KeychainGuard::set();

        let db = setup_test_db();

        // No provider configured at all
        let result = get_internal_provider(&db).await;
        assert!(result.is_err(), "Should error when no provider configured");
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_ollama_default_endpoint() {
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        // Set internal provider to Ollama but DON'T set internal_endpoint
        // Should fall back to default endpoint
        set_ai_setting(&conn, "internal_provider", "ollama").unwrap();
        // Intentionally NOT setting internal_endpoint

        drop(conn);

        let result = get_internal_provider(&db).await;
        assert!(result.is_ok(), "Should succeed with default endpoint");

        match result.unwrap() {
            AiProvider::Ollama { endpoint } => {
                assert_eq!(endpoint, "http://127.0.0.1:11434", "Should use default endpoint when not specified");
            }
            _ => panic!("Expected Ollama provider"),
        }
    }

    // ==================== #2795: Venice as Smart Features internal provider ====================

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_venice() {
        // Fresh empty keychain — nothing from other tests leaks in.
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        set_ai_setting(&conn, "internal_provider", "venice").unwrap();

        // #2795: Venice keys live in their own keychain account (`ai_venice_key`,
        // D4) — NOT the shared `internal_api_key` slot used by OR/Anthropic.
        // Write via the same production function `get_internal_provider` reads
        // through (`read_venice_key`), so this test exercises the real key path.
        crate::ai::credential_migration::write_venice_key(&conn, "VEN-test-key-1234567890")
            .expect("write_venice_key must succeed against MockKeychain");

        drop(conn); // Release lock before async call

        let result = get_internal_provider(&db).await;
        assert!(result.is_ok(), "Should resolve Venice as internal provider");

        match result.unwrap() {
            AiProvider::Venice { api_key, include_system_prompt } => {
                assert_eq!(api_key, "VEN-test-key-1234567890");
                // #2795 R1: include_venice_system_prompt is unconditionally false
                // on the internal path — extraction/classification/rerank are
                // background machinery, not agent conversation. The toggle (D3)
                // is chat-scoped. See smart-routing.md §27.6.2.
                assert_eq!(
                    include_system_prompt, false,
                    "Internal Venice calls must never include the Venice system prompt (R1)"
                );
            }
            _ => panic!("Expected Venice provider"),
        }
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_venice_missing_key() {
        // Fresh empty keychain — no Venice key written.
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        set_ai_setting(&conn, "internal_provider", "venice").unwrap();
        // Intentionally NOT writing a Venice key to the keychain.

        drop(conn);

        let result = get_internal_provider(&db).await;
        assert!(
            result.is_err(),
            "Should error when Venice is selected but no key is configured"
        );
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_venice_fallback_from_primary() {
        // #2795 R2: Venice-primary with `internal_provider` unset must resolve
        // via Step 2 fallback, not Err. This broken state is what motivated
        // #2795 — mirrors test_get_internal_provider_fallback_to_primary but
        // with a Venice-runtime default agent.
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        // Primary agent runtime is Venice; internal_provider is intentionally
        // left unset so resolution falls through to Step 2.
        seed_default_agent(&conn, "venice");
        crate::ai::credential_migration::write_venice_key(&conn, "VEN-primary-fallback-key")
            .expect("write_venice_key must succeed against MockKeychain");

        drop(conn);

        let result = get_internal_provider(&db).await;
        assert!(
            result.is_ok(),
            "Venice-primary users must get a working Smart Features fallback (R2)"
        );

        match result.unwrap() {
            AiProvider::Venice { api_key, include_system_prompt } => {
                assert_eq!(api_key, "VEN-primary-fallback-key");
                assert_eq!(
                    include_system_prompt, false,
                    "Step 2 fallback must also send include_system_prompt: false (R1)"
                );
            }
            _ => panic!("Expected Venice provider via Step 2 fallback"),
        }
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_venice_fallback_missing_key() {
        // Step 2's Venice arm must also fail cleanly (not panic) when no key
        // is configured — mirrors the other Step 2 arms' `.ok_or(...)?` shape.
        let _kc = KeychainGuard::set();

        let db = setup_test_db();
        let conn = db.0.lock().unwrap();

        seed_default_agent(&conn, "venice");
        // Intentionally NOT writing a Venice key.

        drop(conn);

        let result = get_internal_provider(&db).await;
        assert!(
            result.is_err(),
            "Should error, not panic, when Venice-primary fallback has no key"
        );
    }
}

/// #2795: Integration test — round-trip through the real Tauri command entry
/// points (`set_ai_setting` → DB write → `get_ai_setting` read-back →
/// `get_internal_provider` resolves to `AiProvider::Venice`).
///
/// Unlike the unit tests above (which call `get_internal_provider` directly
/// against a hand-built in-memory DB), this exercises the actual
/// `#[tauri::command]` handlers (`notes::set_ai_setting`, `notes::get_ai_setting`)
/// through a real `tauri::test::mock_app`, against a DB created by the real
/// migration runner — the same call path Settings.svelte and Onboarding.svelte
/// drive in production via `invoke('set_ai_setting', ...)`.
#[cfg(test)]
mod venice_integration_tests {
    use crate::ai::provider::get_internal_provider;
    use crate::ai::provider::AiProvider;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;

    fn setup() -> tauri::App<tauri::test::MockRuntime> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_set_ai_setting_then_get_internal_provider_resolves_venice() {
        let _kc = KeychainGuard::set();

        let app = setup();
        let db_state = app.state::<DbState>();

        // Production entry point #1: the real Tauri command handler, exactly
        // as invoked by Settings.svelte / Onboarding.svelte via
        // `invoke('set_ai_setting', { key: 'internal_provider', value: 'venice' })`.
        crate::notes::set_ai_setting(
            db_state.clone(),
            "internal_provider".to_string(),
            "venice".to_string(),
        )
        .expect("set_ai_setting('internal_provider', 'venice') should succeed");

        // Venice's key goes through the keychain-only write path (D4), not
        // `set_ai_setting` (which has no internal_api_key-style intercept for
        // Venice — see credential_migration::write_venice_key).
        {
            let conn = db_state.0.lock().unwrap();
            crate::ai::credential_migration::write_venice_key(&conn, "VEN-roundtrip-key")
                .expect("write_venice_key must succeed");
        }

        // Production entry point #2: read the value back through the real
        // get_ai_setting command handler, confirming the DB write actually
        // persisted (not just held in the connection that wrote it).
        let read_back = crate::notes::get_ai_setting(
            db_state.clone(),
            "internal_provider".to_string(),
        )
        .expect("get_ai_setting should succeed");
        assert_eq!(
            read_back,
            Some("venice".to_string()),
            "internal_provider should read back as 'venice' after the round-trip"
        );

        // Production entry point #3: get_internal_provider resolves the
        // freshly-written setting to a working AiProvider::Venice, end to end.
        let resolved = get_internal_provider(&db_state).await;

        assert!(
            resolved.is_ok(),
            "get_internal_provider should resolve Venice after the set_ai_setting round-trip"
        );
        match resolved.unwrap() {
            AiProvider::Venice { api_key, include_system_prompt } => {
                assert_eq!(api_key, "VEN-roundtrip-key");
                assert_eq!(include_system_prompt, false);
            }
            _ => panic!("Expected AiProvider::Venice after round-trip resolution"),
        }
    }
}

/// #2795 follow-up (Reed's 2026-09-01 Venice-unsupported sweep) — 4 provider-dispatch
/// gaps where a second, local `AiProvider -> String` conversion match (downstream of
/// `get_internal_provider`/`resolve_runtime_config_for_default_agent`, both of which
/// already build `AiProvider::Venice` correctly) was missing its Venice arm, silently
/// dropping Venice into that match's `_` catch-all.
///
/// Each site here lives inside machinery that cannot be invoked directly in a unit
/// test without a live `AppHandle` (a spawned background worker loop, a Tauri
/// `app.listen` event handler, or an `.and_then` closure buried inside
/// `ai_chat_stream`'s multi-thousand-line streaming loop) — the same shape as the
/// existing `tests_2604_stream_done_agent_attribution` source-scan gate in
/// `chat.rs`. These tests scan the real source files and mechanically assert
/// the `AiProvider::Venice` arm is present in each match block, so a future
/// refactor that drops the arm again fails `cargo test` instead of silently
/// regressing. The task's dispatch-shape correctness (routes to `"venice".to_string()`,
/// clones `api_key`, passes `None` for endpoint) is verified identically at 8 other
/// sites already covered by `test_get_internal_provider_venice*` and the Step-2
/// fallback tests above — these 4 tests close the "arm exists at all" gap specific
/// to the second, local conversion matches Reed's sweep found.
#[cfg(test)]
mod venice_dispatch_gap_tests {
    /// Shared helper: read a source file relative to the crate manifest dir,
    /// the same pattern used by `tests_2604_stream_done_agent_attribution`
    /// (`chat.rs`) and `agents.rs`'s existing `CARGO_MANIFEST_DIR` tests.
    fn read_source(rel_path: &str) -> String {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set during `cargo test`");
        let path = std::path::Path::new(&manifest_dir).join(rel_path);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", rel_path, e))
    }

    /// Gap #1 (CRITICAL) — `extraction_worker.rs`'s internal-provider dispatch match
    /// (`internal_provider: AiProvider` -> `(provider, api_key, endpoint)` for
    /// `call_llm_from_config`) must carry a `Venice` arm before its `_` catch-all.
    /// This is the exact match the v1.0.41 build log confirmed broken
    /// (`[EXTRACTION] Unsupported internal provider variant for extraction`).
    #[test]
    fn test_extraction_worker_venice_dispatch() {
        let content = read_source("src/ai/extraction_worker.rs");

        let marker = "Unsupported internal provider variant for extraction";
        let marker_idx = content
            .find(marker)
            .expect("extraction_worker.rs must still contain its catch-all error marker");

        // The match block containing the catch-all starts a bounded window before
        // the marker (covers the full `match &internal_provider { ... }` arm list).
        let window_start = marker_idx.saturating_sub(1200);
        let scan_window = &content[window_start..marker_idx];

        assert!(
            scan_window.contains("AiProvider::Venice"),
            "extraction_worker.rs's internal_provider match must have an \
             AiProvider::Venice arm before the catch-all that emits '{}' \
             (Reed's 2026-09-01 sweep, Gap #1)",
            marker
        );
        assert!(
            scan_window.contains("\"venice\".to_string()"),
            "the Venice arm must route to the \"venice\" provider-name string, matching \
             the fix pattern proven at chat.rs's other Venice arms"
        );
    }

    /// Gap #2 (HIGH) — `ai_chat_stream`'s internal-provider single-retry-on-
    /// transient-error block (`chat.rs`) must have an `AiProvider::Venice` arm
    /// calling `stream_openrouter` with `VENICE_API_BASE` / `RequestOrigin::Venice`,
    /// identical to the sibling arms in the primary-dispatch block above it and the
    /// fallback-retry block below it. Without it, a transient error on a Venice
    /// stream falls into `_ => Err(("Streaming not supported"...))`, discarding the
    /// real error and masking it with a misleading message.
    #[test]
    fn test_streaming_retry_venice_dispatch() {
        let content = read_source("src/ai/chat.rs");

        // There are two literal `"[SMART-ROUTE] Internal provider failed, retrying..."`
        // markers in the file: none — there's exactly one, immediately preceding the
        // retry match this test targets. Anchor on it, then scan forward to the first
        // `_ => Err(("Streaming not supported"` after it (this retry block's own
        // catch-all, not the primary-dispatch or fallback-retry blocks' catch-alls).
        let anchor = "Internal provider failed, retrying...";
        let anchor_idx = content
            .find(anchor)
            .expect("chat.rs must still contain the internal-provider retry log line");

        let catch_all = "_ => Err((\"Streaming not supported\".to_string(), String::new())),";
        let catch_all_idx = content[anchor_idx..]
            .find(catch_all)
            .map(|rel| anchor_idx + rel)
            .expect("the retry block's catch-all arm must still exist after the anchor");

        let scan_window = &content[anchor_idx..catch_all_idx];

        assert!(
            scan_window.contains("AiProvider::Venice { api_key, include_system_prompt }"),
            "ai_chat_stream's internal-provider retry block must have an \
             AiProvider::Venice arm before its 'Streaming not supported' catch-all \
             (Reed's 2026-09-01 sweep, Gap #2 / row 16b)"
        );
        assert!(
            scan_window.contains("VENICE_API_BASE") && scan_window.contains("RequestOrigin::Venice"),
            "the retry block's Venice arm must call stream_openrouter with \
             VENICE_API_BASE + RequestOrigin::Venice, matching the primary-dispatch \
             and fallback-retry arms exactly"
        );
    }

    /// Gap #3 (MEDIUM) — both consolidator trigger paths in `lib.rs` (router-
    /// triggered `trigger-consolidator` event listener, and the one-time startup
    /// staleness-check pass) must have an `AiProvider::Venice` arm before their
    /// respective `"[CONSOLIDATOR] Unsupported provider..."` catch-alls. Without it,
    /// Venice-primary installs silently never get background note consolidation.
    #[test]
    fn test_consolidator_venice_dispatch() {
        let content = read_source("src/lib.rs");

        // Site A: router-triggered consolidation.
        let marker_a = "[CONSOLIDATOR] Unsupported provider\"";
        let marker_a_idx = content
            .find(marker_a)
            .expect("lib.rs must still contain the router-trigger consolidator catch-all");
        let window_a_start = marker_a_idx.saturating_sub(800);
        let window_a = &content[window_a_start..marker_a_idx];
        assert!(
            window_a.contains("AiProvider::Venice"),
            "lib.rs's router-triggered consolidator dispatch (trigger-consolidator \
             event listener) must have an AiProvider::Venice arm before its \
             'Unsupported provider' catch-all (Reed's 2026-09-01 sweep, Gap #3 / #2)"
        );

        // Site B: startup consolidation pass — distinct marker string, must not
        // double-count against Site A's window.
        let marker_b = "[CONSOLIDATOR] Unsupported provider for startup consolidation";
        let marker_b_idx = content
            .find(marker_b)
            .expect("lib.rs must still contain the startup consolidator catch-all");
        assert!(
            marker_b_idx > marker_a_idx,
            "sanity: startup consolidator marker expected after router-trigger marker in lib.rs"
        );
        let window_b_start = marker_b_idx.saturating_sub(800);
        let window_b = &content[window_b_start..marker_b_idx];
        assert!(
            window_b.contains("AiProvider::Venice"),
            "lib.rs's startup consolidation dispatch must have an AiProvider::Venice \
             arm before its 'Unsupported provider for startup consolidation' catch-all \
             (Reed's 2026-09-01 sweep, Gap #3 / #3)"
        );
    }

    /// Gap #4 (LOW) — `ai_chat_stream`'s daily-log narrative generation trigger
    /// (`chat.rs`, inside the `.and_then` closure resolving the daily log's
    /// agent runtime) must have an `AiProvider::Venice` arm so Venice-primary
    /// installs still get auto-generated daily narratives instead of silently
    /// no-oping via `_ => return None`.
    #[test]
    fn test_daily_narrative_venice_dispatch() {
        let content = read_source("src/ai/chat.rs");

        let anchor = "Yesterday's log needs narrative, spawning generation";
        let anchor_idx = content
            .find(anchor)
            .expect("chat.rs must still contain the daily-narrative trigger log line");

        // Scan forward to this block's own `Some((name, key, cfg.model.clone(), ep, ...))`
        // close, which immediately follows its match's arm list. ADR-0067 D2/D5 widened
        // this tuple with a 5th `cfg.runtime_ref.clone()` element (the wire selector) —
        // the anchor is deliberately just the stable prefix, not the full literal, so it
        // survives that kind of field-count-only widening.
        let block_close = "Some((name, key, cfg.model.clone(), ep";
        let block_close_idx = content[anchor_idx..]
            .find(block_close)
            .map(|rel| anchor_idx + rel)
            .expect("the daily-narrative provider-resolution block must still exist after the anchor");

        let scan_window = &content[anchor_idx..block_close_idx];

        assert!(
            scan_window.contains("AiProvider::Venice"),
            "ai_chat_stream's daily-log narrative provider-resolution match must have \
             an AiProvider::Venice arm before its '_ => return None' catch-all \
             (Reed's 2026-09-01 sweep, Gap #4)"
        );
        assert!(
            scan_window.contains("\"venice\".to_string()"),
            "the daily-narrative Venice arm must route to the \"venice\" provider-name \
             string, matching the fix pattern proven at chat.rs's other Venice arms"
        );
    }
}
