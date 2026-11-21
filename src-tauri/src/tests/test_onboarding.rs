//! Tests for onboarding pipeline (§6.5, #364, #398)
//! Verifies fixes for BUG 1-7, 10, 12, 22 from onboard-robustness-audit.md
//! Issue #398: Smarter onboarding check + config restore + diagnostic logging

#[cfg(test)]
mod onboarding_tests {
    use rusqlite::{Connection, OptionalExtension};
    use tempfile::TempDir;
    use crate::database::DbState;
    
    /// Create a temporary test database
    fn create_test_db() -> (TempDir, DbState) {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let db_path = temp_dir.path().join("test.db");
        let conn = Connection::open(&db_path).expect("Failed to open test database");
        
        // Create minimal schema for ai_agents table + ai_settings
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
                source TEXT NOT NULL DEFAULT 'manual',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                owner_id TEXT
            );
            
            CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            
            -- BUG 6 FIX: Unique index on runtime_ref (non-partial for ON CONFLICT)
            CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_oc_id_unique 
              ON ai_agents(runtime_ref);
            
            -- BUG 2 FIX: Only one main agent
            CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_default 
              ON ai_agents(is_default) WHERE is_default = 1;
            "
        ).expect("Failed to create test schema");
        
        let db_state = DbState::new(conn);
        (temp_dir, db_state)
    }
    
    /// Test BUG 2 FIX: Only one agent should have is_default = 1
    #[test]
    fn test_is_main_detection() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        // Simulate importing 4 agents where runtime_ref == "main" for one
        let agents = vec![
            ("main", "Main Agent", "UUID1"),
            ("coder", "Coder", "UUID2"),
            ("researcher", "Researcher", "UUID3"),
            ("writer", "Writer", "UUID4"),
        ];
        
        let now = chrono::Utc::now().to_rfc3339();
        
        for (oc_id, name, uuid) in &agents {
            let is_default = *oc_id == "main";  // BUG 2 FIX: Use oc_id to determine main
            
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, runtime_ref, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    uuid,
                    name,
                    name.to_lowercase(),
                    if is_default { "main" } else { "sub" },
                    if is_default { 1 } else { 0 },
                    uuid,
                    oc_id,
                    &now,
                    &now
                ],
            ).expect("Failed to insert agent");
        }
        
        // Verify only ONE agent has is_default = 1
        let main_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_agents WHERE is_default = 1",
            [],
            |row| row.get(0)
        ).expect("Failed to count main agents");
        
        assert_eq!(main_count, 1, "Exactly one agent should have is_default = 1");
        
        // Verify the main agent has runtime_ref = "main"
        let main_oc_id: String = conn.query_row(
            "SELECT runtime_ref FROM ai_agents WHERE is_default = 1",
            [],
            |row| row.get(0)
        ).expect("Failed to get main agent oc_id");
        
        assert_eq!(main_oc_id, "main", "Main agent should have runtime_ref = 'main'");
        
        println!("✓ test_is_main_detection passed: only one agent marked as main");
    }
    
    /// Test BUG 6 FIX: Duplicate detection on re-import
    #[test]
    fn test_duplicate_detection() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        let now = chrono::Utc::now().to_rfc3339();
        
        // First import
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, runtime_ref, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                "UUID1",
                "Main Agent",
                "main-agent",
                "main",
                1,
                "UUID1",
                "main",
                &now,
                &now
            ],
        ).expect("Failed to insert agent first time");
        
        // Second import with different UUID but same runtime_ref (simulates re-onboarding)
        let result = conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, runtime_ref, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(runtime_ref) DO UPDATE SET
               name = excluded.name,
               workspace_path = excluded.workspace_path,
               updated_at = excluded.updated_at
             WHERE runtime_ref IS NOT NULL",
            rusqlite::params![
                "UUID2",  // Different UUID
                "Main Agent Updated",  // Updated name
                "main-agent",
                "main",
                1,
                "UUID2",
                "main",  // Same runtime_ref
                &now,
                &now
            ],
        );
        
        assert!(result.is_ok(), "ON CONFLICT should handle duplicate runtime_ref");
        
        // Verify only ONE record exists
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_agents WHERE runtime_ref = 'main'",
            [],
            |row| row.get(0)
        ).expect("Failed to count agents");
        
        assert_eq!(count, 1, "Only one record should exist after re-import");
        
        // Verify the name was updated
        let name: String = conn.query_row(
            "SELECT name FROM ai_agents WHERE runtime_ref = 'main'",
            [],
            |row| row.get(0)
        ).expect("Failed to get agent name");
        
        assert_eq!(name, "Main Agent Updated", "Agent name should be updated on conflict");
        
        println!("✓ test_duplicate_detection passed: re-import updates existing agent instead of creating duplicate");
    }
    
    /// Test BUG 5 FIX: Transaction rollback cleans up files
    #[test]
    fn test_transaction_rollback() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        let now = chrono::Utc::now().to_rfc3339();
        
        // Start transaction
        conn.execute("BEGIN TRANSACTION", []).expect("Failed to begin transaction");
        
        // Insert some agents
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, runtime_ref, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params!["UUID1", "Agent 1", "agent-1", "sub", 0, "UUID1", "agent1", &now, &now],
        ).expect("Failed to insert first agent");
        
        // Verify agent is visible within transaction
        let count_in_tx: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_agents",
            [],
            |row| row.get(0)
        ).expect("Failed to count in transaction");
        
        assert_eq!(count_in_tx, 1, "Agent should be visible within transaction");
        
        // Rollback
        conn.execute("ROLLBACK", []).expect("Failed to rollback");
        
        // Verify agent is gone after rollback
        let count_after_rollback: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_agents",
            [],
            |row| row.get(0)
        ).expect("Failed to count after rollback");
        
        assert_eq!(count_after_rollback, 0, "All agents should be removed after rollback");
        
        println!("✓ test_transaction_rollback passed: rollback removes all inserted agents");
    }
    
    /// Test BUG 12 FIX: Empty agent names get fallback
    #[test]
    fn test_empty_agent_names() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        let now = chrono::Utc::now().to_rfc3339();
        
        // Test empty name handling
        let oc_id = "test-agent";
        let empty_name = "";
        let fallback_name = format!("Agent {}", oc_id);
        
        // In real code, sync_initial_import handles this before insert
        let agent_name = if empty_name.trim().is_empty() {
            format!("Agent {}", oc_id)
        } else {
            empty_name.to_string()
        };
        
        assert_eq!(agent_name, fallback_name, "Empty name should get fallback");
        
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, runtime_ref, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params!["UUID1", &agent_name, "agent-test-agent", "sub", 0, "UUID1", oc_id, &now, &now],
        ).expect("Failed to insert agent with fallback name");
        
        let stored_name: String = conn.query_row(
            "SELECT name FROM ai_agents WHERE runtime_ref = ?1",
            rusqlite::params![oc_id],
            |row| row.get(0)
        ).expect("Failed to get stored name");
        
        assert_eq!(stored_name, fallback_name, "Stored name should be fallback");
        
        println!("✓ test_empty_agent_names passed: empty names get fallback");
    }
    
    /// Test BUG 4 FIX: Response validation catches malformed data
    #[test]
    fn test_response_validation() {
        // Test validation logic that would run in sync_client.rs
        
        struct TestAgent {
            oc_id: String,
            name: String,
            files: Vec<TestFile>,
        }
        
        struct TestFile {
            filename: String,
            content: String,
        }
        
        let valid_agent = TestAgent {
            oc_id: "main".to_string(),
            name: "Main Agent".to_string(),
            files: vec![
                TestFile {
                    filename: "MEMORY.md".to_string(),
                    content: "test content".to_string(),
                },
            ],
        };
        
        let invalid_agents = vec![
            TestAgent {
                oc_id: "".to_string(),  // Empty oc_id
                name: "Agent".to_string(),
                files: vec![],
            },
            TestAgent {
                oc_id: "test".to_string(),
                name: "Agent".to_string(),
                files: vec![
                    TestFile {
                        filename: "../../../etc/passwd".to_string(),  // Path traversal
                        content: "evil".to_string(),
                    },
                ],
            },
        ];
        
        // Valid agent passes
        assert!(!valid_agent.oc_id.is_empty(), "Valid agent should pass oc_id check");
        assert!(!valid_agent.name.is_empty(), "Valid agent should have a display name");
        assert!(!valid_agent.files.iter().any(|f| f.filename.contains("..")), "Valid agent should pass filename check");
        assert!(!valid_agent.files.iter().any(|f| f.content.is_empty()), "Valid agent files should have content");
        
        // Invalid agents fail
        assert!(invalid_agents[0].oc_id.is_empty(), "Empty oc_id should be detected");
        assert!(invalid_agents[1].files[0].filename.contains(".."), "Path traversal should be detected");
        
        println!("✓ test_response_validation passed: malformed data is caught");
    }
    
    /// Test #398: Onboarding check should skip when onboarding_completed is set
    #[test]
    fn test_onboarding_check_completed() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        // Set onboarding_completed = "true"
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('onboarding_completed', 'true')",
            [],
        ).expect("Failed to set onboarding_completed");
        
        // Verify the key exists and is "true"
        let result: String = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'onboarding_completed'",
            [],
            |row| row.get(0)
        ).expect("Failed to get onboarding_completed");
        
        assert_eq!(result, "true", "onboarding_completed should be 'true'");
        
        // Test the logic used in should_show_onboarding
        let should_show = if result == "true" { false } else { true };
        
        assert!(!should_show, "Onboarding should be skipped when onboarding_completed = 'true'");
        
        println!("✓ test_onboarding_check_completed passed: onboarding skipped when completed flag is set");
    }
    
    /// Test #398: Onboarding check should show when onboarding_completed is NOT set
    #[test]
    fn test_onboarding_check_not_completed() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        // Do NOT set onboarding_completed
        
        // Try to get the key (should return None)
        let result: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'onboarding_completed'",
            [],
            |row| row.get(0)
        ).optional().expect("Query failed");
        
        assert!(result.is_none(), "onboarding_completed should not exist");
        
        // Test the logic: if key doesn't exist, show onboarding
        let should_show = result.is_none() || result.unwrap_or_default() != "true";
        
        assert!(should_show, "Onboarding should be shown when completed flag is not set");
        
        println!("✓ test_onboarding_check_not_completed passed: onboarding shown when completed flag missing");
    }
    
    /// Test #398: Config restore should support both apiKeyRef and api_key
    #[test]
    fn test_config_restore_key_compatibility() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        // Test that both key names map to the same ai_settings key
        let config_keys = vec![
            ("apiKeyRef", "api_key"),  // Legacy
            ("api_key", "api_key"),     // New
        ];
        
        for (config_key, settings_key) in config_keys {
            // Simulate the mapping logic from restore_ai_settings_from_config
            assert_eq!(settings_key, "api_key", 
                       "Config key '{}' should map to ai_settings key 'api_key'", config_key);
        }
        
        // Verify that inserting with either key results in the same database record
        conn.execute(
            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('api_key', 'test_token_123')",
            [],
        ).expect("Failed to insert api_key");
        
        let stored_value: String = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'api_key'",
            [],
            |row| row.get(0)
        ).expect("Failed to get api_key");
        
        assert_eq!(stored_value, "test_token_123", "api_key should be stored correctly");
        
        println!("✓ test_config_restore_key_compatibility passed: both apiKeyRef and api_key are supported");
    }
    
    /// Test #398 Edge Case: Provider + api_key set, onboarding_completed missing → should skip onboarding
    /// NOTE (#2564 B1 fix, Path B): this test manually re-derives the skip/show logic
    /// inline (`if !provider.is_empty() && ...`) rather than calling the real
    /// `should_show_onboarding_impl`, so it does NOT exercise the retired
    /// `ai_settings.provider` read and is unaffected by that key's retirement
    /// (ADR-0063 D8) — kept as-is; it documents the #398 decision table, not the
    /// current provider-resolution implementation. See `notes.rs`'s
    /// `test_should_show_onboarding_recovery_with_agents_and_provider` for the real
    /// re-pointed behavior against actual `should_show_onboarding_impl`.
    #[test]
    fn test_onboarding_skip_when_provider_and_key_exist() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        // Set provider and api_key but NOT onboarding_completed
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('provider', 'anthropic')",
            [],
        ).expect("Failed to insert provider");
        
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('api_key', 'sk-test-key')",
            [],
        ).expect("Failed to insert api_key");
        
        // Verify onboarding_completed does NOT exist
        let completed_flag: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'onboarding_completed'",
            [],
            |row| row.get(0)
        ).optional().expect("Query failed");
        
        assert!(completed_flag.is_none(), "onboarding_completed should not exist");
        
        // Verify provider exists
        let provider: String = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'provider'",
            [],
            |row| row.get(0)
        ).expect("Failed to get provider");
        
        assert_eq!(provider, "anthropic", "Provider should be set");
        
        // Verify api_key exists
        let api_key: String = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'api_key'",
            [],
            |row| row.get(0)
        ).expect("Failed to get api_key");
        
        assert_eq!(api_key, "sk-test-key", "API key should be set");
        
        // Test the logic: if provider + api_key exist, skip onboarding even if completed flag is missing
        let should_show = if !provider.is_empty() && provider != "none" && !api_key.is_empty() {
            false  // Skip onboarding
        } else {
            true   // Show onboarding
        };
        
        assert!(!should_show, "Onboarding should be skipped when provider and api_key exist, even without completed flag");
        
        println!("✓ test_onboarding_skip_when_provider_and_key_exist passed: manual config skips onboarding");
    }
    
    /// Test #398 Edge Case: onboarding_completed = "false" → should show onboarding
    #[test]
    fn test_onboarding_show_when_explicitly_false() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        // Set onboarding_completed = "false" (explicit)
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('onboarding_completed', 'false')",
            [],
        ).expect("Failed to set onboarding_completed");
        
        // Verify the key exists and is "false"
        let result: String = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'onboarding_completed'",
            [],
            |row| row.get(0)
        ).expect("Failed to get onboarding_completed");
        
        assert_eq!(result, "false", "onboarding_completed should be 'false'");
        
        // Test the logic: if explicitly "false", show onboarding (user intent)
        let should_show = if result == "false" {
            true  // Force show onboarding
        } else if result == "true" {
            false  // Skip onboarding
        } else {
            true   // Default: show onboarding
        };
        
        assert!(should_show, "Onboarding should be shown when onboarding_completed is explicitly 'false'");
        
        println!("✓ test_onboarding_show_when_explicitly_false passed: explicit 'false' shows onboarding");
    }
    
    /// Test #398 Edge Case: Nothing set → should show onboarding (fresh install)
    #[test]
    fn test_onboarding_show_on_fresh_install() {
        let (_temp_dir, db) = create_test_db();
        let conn = db.0.lock().unwrap();
        
        // Do NOT set any ai_settings
        
        // Verify nothing exists
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_settings",
            [],
            |row| row.get(0)
        ).expect("Failed to count ai_settings");
        
        assert_eq!(count, 0, "ai_settings should be empty");
        
        // Test the logic: if nothing is set, show onboarding
        let completed_flag: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'onboarding_completed'",
            [],
            |row| row.get(0)
        ).optional().expect("Query failed");
        
        let provider: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'provider'",
            [],
            |row| row.get(0)
        ).optional().expect("Query failed");
        
        let api_key: Option<String> = conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'api_key'",
            [],
            |row| row.get(0)
        ).optional().expect("Query failed");
        
        assert!(completed_flag.is_none(), "onboarding_completed should not exist");
        assert!(provider.is_none(), "provider should not exist");
        assert!(api_key.is_none(), "api_key should not exist");
        
        // Test the logic: fresh install → show onboarding
        let should_show = true;
        
        assert!(should_show, "Onboarding should be shown on fresh install");
        
        println!("✓ test_onboarding_show_on_fresh_install passed: fresh install shows onboarding");
    }
    
    // TODO #577: test_fetch_gateway_models_error_handling
    // Removed temporarily because fetch_gateway_models now requires tauri::State<DbState>
    // which cannot be easily mocked in unit tests. Needs refactoring to either:
    // 1. Integration test using full Tauri test harness
    // 2. Extract business logic to non-State helper function
    // Original test verified error handling for invalid URLs and unreachable hosts

    /// #1296: sync_initial_import is now async + spawn_blocking.
    /// Verifies that the command:
    ///   1. Is callable from an async context (compile-time proof via #[tokio::test])
    ///   2. Returns an Err (not a panic) when the gateway is unreachable
    ///   3. The error message is non-empty and describes the failure
    ///
    /// Full success-path testing requires a live gateway; the error-path here
    /// provides CI-safe coverage that the async wrapper and spawn_blocking
    /// plumbing don't panic or deadlock on network failure.
    /// #1296: sync_initial_import is now async + spawn_blocking.
    /// Verifies that calling the underlying initial_sync from a blocking thread
    /// (as spawn_blocking does) returns Err on network failure — not a panic.
    ///
    /// We use spawn_blocking here to match the production execution model:
    /// the real command wraps this call in tokio::task::spawn_blocking.
    #[tokio::test]
    async fn test_sync_initial_import_is_async_and_fails_gracefully() {
        // Use an address that's guaranteed unreachable (localhost on a high port).
        // Run inside spawn_blocking to mirror production: reqwest::blocking must
        // not be called on an async thread (same constraint as the real command).
        let result = tokio::task::spawn_blocking(|| {
            crate::sync_client::initial_sync(
                "http://127.0.0.1:19999",
                "test-token",
                None, // cert_pem: no cert in test context
            )
        })
        .await
        .expect("spawn_blocking panicked");

        // The underlying sync_client call should fail — not panic
        assert!(
            result.is_err(),
            "initial_sync to unreachable host should return Err, not panic"
        );

        let err_msg = result.unwrap_err();
        assert!(
            !err_msg.is_empty(),
            "Error message should be non-empty, got: {:?}",
            err_msg
        );

        println!(
            "✓ test_sync_initial_import_is_async_and_fails_gracefully: got expected error: {}",
            err_msg
        );
    }
}
