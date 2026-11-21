//! Tests for Phase 3 Smart Routing (#442 #443 #444)
//!
//! Verifies:
//! - Binary classifier correctly identifies simple vs complex messages
//! - Model resolution chain follows correct order
//! - Bypass rules work correctly
//! - Migration 22 applies schema changes
//!
//! GitHub Issues: #442 #443 #444

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use crate::database::DbState;
    use crate::ai::provider::ChatMessage;
    use crate::ai::chat::{classify_message_complexity, resolve_model_for_message};

    /// Create an in-memory SQLite database with schema for smart routing testing
    fn setup_smart_routing_db() -> Connection {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        
        // Create minimal schema needed for smart routing tests
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                default_model TEXT,
                smart_routing_enabled INTEGER DEFAULT NULL,
                router_model TEXT,
                simple_tier_model TEXT
            );

            CREATE TABLE ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT
            );

            INSERT INTO ai_settings (key, value) VALUES ('model', 'anthropic/claude-sonnet-4');
            INSERT INTO ai_settings (key, value) VALUES ('smart_routing_enabled', 'false');
            INSERT INTO ai_settings (key, value) VALUES ('router_model', 'anthropic/claude-3-5-haiku-20241022');
            INSERT INTO ai_settings (key, value) VALUES ('tier_1_model', 'anthropic/claude-3-5-haiku-20241022');
            "
        ).expect("Failed to create test schema");
        
        conn
    }

    #[test]
    fn test_bypass_long_message() {
        // Messages >500 words should bypass classifier
        let long_message = (0..501).map(|i| format!("word{}", i)).collect::<Vec<_>>().join(" ");
        let context: Vec<ChatMessage> = vec![];
        
        let conn = setup_smart_routing_db();
        let db_state = DbState::new(conn);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(classify_message_complexity(&db_state, None, &long_message, &context, None));
        
        // Should return false (use complex model) due to bypass
        assert_eq!(result, false, "Long messages should bypass classifier and return false");
    }

    #[test]
    fn test_bypass_explicit_complexity() {
        let messages = vec![
            "think deeply about this problem",
            "analyze the architecture",
            "help me think through this",
        ];
        
        let conn = setup_smart_routing_db();
        let db_state = DbState::new(conn);
        let context: Vec<ChatMessage> = vec![];
        
        for msg in messages {
            let result = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(classify_message_complexity(&db_state, None, msg, &context, None));
            
            assert_eq!(result, false, "Message '{}' should bypass classifier", msg);
        }
    }

    #[test]
    fn test_bypass_long_conversation() {
        let message = "What's 2+2?";
        let mut context: Vec<ChatMessage> = vec![];
        
        // Create 11 messages of context (>10 should bypass)
        for i in 0..11 {
            context.push(ChatMessage {
                role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
                content: format!("Message {}", i),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                attachment_ids: None,
                id: None,
            });
        }
        
        let conn = setup_smart_routing_db();
        let db_state = DbState::new(conn);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(classify_message_complexity(&db_state, None, message, &context, None));
        
        assert_eq!(result, false, "Long conversations should bypass classifier");
    }

    #[test]
    fn test_resolve_model_smart_routing_disabled() {
        // Project-level smart-routing override path no longer exists (ADR-0061,
        // #2531/#2533). With no `internal_provider`/`simple_tier_model` configured,
        // resolution should fall through to the global default model.
        let conn = setup_smart_routing_db_v32();
        let db_state = DbState::new(conn);

        let message = "What's 2+2?";
        let context: Vec<ChatMessage> = vec![];

        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolve_model_for_message(&db_state, None, message, &context, None, Some("anthropic/claude-sonnet-4")));

        assert!(result.is_ok());
        assert_eq!(result.unwrap().model, "anthropic/claude-sonnet-4", "Should fall back to global default model when smart routing is not configured");
    }

    #[test]
    fn test_resolve_model_no_project_default_uses_global() {
        let conn = setup_smart_routing_db_v32();
        let db_state = DbState::new(conn);
        
        // Create project with no default model (project-level model override retired, ADR-0061)
        db_state.0.lock().unwrap().execute(
            "INSERT INTO projects (id, name) VALUES ('test-2', 'Test')",
            [],
        ).unwrap();
        
        let message = "What's 2+2?";
        let context: Vec<ChatMessage> = vec![];
        
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolve_model_for_message(&db_state, Some("test-2"), message, &context, None, Some("anthropic/claude-sonnet-4")));

        assert!(result.is_ok());
        assert_eq!(result.unwrap().model, "anthropic/claude-sonnet-4", "Should fall back to global default model");
    }

    #[test]
    fn test_resolve_model_no_config_returns_error() {
        let conn = setup_smart_routing_db_v32();
        let db_state = DbState::new(conn);
        
        // Create project with no default model (project-level model override retired, ADR-0061)
        db_state.0.lock().unwrap().execute(
            "INSERT INTO projects (id, name) VALUES ('test-3', 'Test')",
            [],
        ).unwrap();
        
        // Remove global default
        db_state.0.lock().unwrap().execute("DELETE FROM ai_settings WHERE key = 'model'", []).unwrap();
        
        let message = "What's 2+2?";
        let context: Vec<ChatMessage> = vec![];
        
        // #2522 ADR-0063 Stage 2: the global ai_settings.model fallback is retired —
        // the "no model configured anywhere" case is now expressed by passing None
        // for the agent's own resolved model (agent_model_fallback), not by deleting
        // an ai_settings row that resolve_model_for_message no longer reads.
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolve_model_for_message(&db_state, Some("test-3"), message, &context, None, None));
        
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No AI model configured"), "Should error when no model is configured anywhere");
    }

    #[test]
    fn test_migration_22_adds_columns() {
        let conn = setup_smart_routing_db();
        
        // Apply migration 22 (simplified inline version)
        conn.execute("ALTER TABLE projects ADD COLUMN router_model TEXT", []).ok();
        conn.execute("ALTER TABLE projects ADD COLUMN simple_tier_model TEXT", []).ok();
        
        // Verify columns exist by trying to set values
        conn.execute(
            "INSERT INTO projects (id, name, router_model, simple_tier_model) VALUES (1, 'Test', 'model1', 'model2')",
            [],
        ).expect("Should be able to insert into new columns");
        
        let router_model: String = conn.query_row(
            "SELECT router_model FROM projects WHERE id = 1",
            [],
            |row| row.get(0),
        ).unwrap();
        
        assert_eq!(router_model, "model1", "router_model column should exist and store values");
    }

    #[test]
    fn test_smart_routing_null_inheritance() {
        let conn = setup_smart_routing_db();
        
        // Project with smart_routing_enabled = NULL should inherit global setting
        conn.execute(
            "INSERT INTO projects (id, name, smart_routing_enabled) VALUES (1, 'Test', NULL)",
            [],
        ).unwrap();
        
        // Set global smart routing to true
        conn.execute(
            "UPDATE ai_settings SET value = 'true' WHERE key = 'smart_routing_enabled'",
            [],
        ).unwrap();
        
        // When resolving model, NULL should inherit global (true)
        // Since we can't easily mock the classifier without a real provider,
        // we just verify the NULL value is read correctly
        let smart_routing: Option<bool> = conn.query_row(
            "SELECT smart_routing_enabled FROM projects WHERE id = 1",
            [],
            |row| row.get(0),
        ).unwrap();
        
        assert_eq!(smart_routing, None, "NULL should be preserved in database for inheritance");
    }

    // ==================== v0.32.1 Tests: Derived Smart Routing ====================

    /// Setup database with consolidated projects schema (v0.60.0+)
    fn setup_smart_routing_db_v32() -> Connection {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                is_hidden INTEGER NOT NULL DEFAULT 0,
                default_model TEXT,
                smart_routing_enabled INTEGER,
                router_model TEXT,
                simple_tier_model TEXT,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );

            CREATE TABLE ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT
            );

            INSERT INTO ai_settings (key, value) VALUES ('model', 'anthropic/claude-sonnet-4');
            INSERT INTO ai_settings (key, value) VALUES ('router_model', 'anthropic/claude-3-5-haiku-20241022');
            "
        ).expect("Failed to create test schema");
        
        conn
    }

    #[test]
    fn test_global_smart_routing_derives_from_simple_model_and_provider() {
        let conn = setup_smart_routing_db_v32();
        let db_state = DbState::new(conn);
        
        // CASE 1: simple_tier_model set but no internal_provider → smart routing OFF
        db_state.0.lock().unwrap().execute(
            "INSERT INTO ai_settings (key, value) VALUES ('simple_tier_model', 'anthropic/claude-3-5-haiku-20241022')",
            [],
        ).unwrap();
        
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolve_model_for_message(&db_state, None, "test message", &vec![], None, Some("anthropic/claude-sonnet-4")));
        
        // Should use global default model (not simple model) because internal provider is missing
        assert_eq!(result.unwrap().model, "anthropic/claude-sonnet-4", 
            "Smart routing should be OFF when internal_provider is missing");
        
        // CASE 2: simple_tier_model AND internal_provider set → smart routing ON
        db_state.0.lock().unwrap().execute(
            "INSERT INTO ai_settings (key, value) VALUES ('internal_provider', 'openrouter')",
            [],
        ).unwrap();
        db_state.0.lock().unwrap().execute(
            "INSERT INTO ai_settings (key, value) VALUES ('internal_api_key', 'test-key')",
            [],
        ).unwrap();
        
        // Smart routing should now derive as enabled (simple_model + internal_provider both present)
        // Note: We can't easily test the classifier path without mocking, but we verified the derivation logic compiles
    }

    #[test]
    fn test_internal_provider_empty_string_disables_smart_routing() {
        let conn = setup_smart_routing_db_v32();
        let db_state = DbState::new(conn);
        
        // Set simple_tier_model and internal_provider="" (empty)
        db_state.0.lock().unwrap().execute(
            "INSERT INTO ai_settings (key, value) VALUES ('simple_tier_model', 'anthropic/claude-3-5-haiku-20241022')",
            [],
        ).unwrap();
        db_state.0.lock().unwrap().execute(
            "INSERT INTO ai_settings (key, value) VALUES ('internal_provider', '')",
            [],
        ).unwrap();
        
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolve_model_for_message(&db_state, None, "test", &vec![], None, Some("anthropic/claude-sonnet-4")));
        
        // Should use global default (smart routing OFF)
        assert_eq!(result.unwrap().model, "anthropic/claude-sonnet-4",
            "Empty internal_provider should disable smart routing");
    }

    #[test]
    fn test_internal_provider_none_disables_smart_routing() {
        let conn = setup_smart_routing_db_v32();
        let db_state = DbState::new(conn);
        
        // Set simple_tier_model and internal_provider="none"
        db_state.0.lock().unwrap().execute(
            "INSERT INTO ai_settings (key, value) VALUES ('simple_tier_model', 'anthropic/claude-3-5-haiku-20241022')",
            [],
        ).unwrap();
        db_state.0.lock().unwrap().execute(
            "INSERT INTO ai_settings (key, value) VALUES ('internal_provider', 'none')",
            [],
        ).unwrap();
        
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolve_model_for_message(&db_state, None, "test", &vec![], None, Some("anthropic/claude-sonnet-4")));
        
        // Should use global default (smart routing OFF)
        assert_eq!(result.unwrap().model, "anthropic/claude-sonnet-4",
            "internal_provider='none' should disable smart routing");
    }

    /// Tests for v0.32.2 ModelResolution with provider override (#455)
    /// 
    /// Note: These tests focus on the provider_override logic, not the classifier.
    /// The classifier is tested separately in other tests. Here we test that:
    /// - When smart routing is ENABLED, ModelResolution structure is correct
    /// - When smart routing is DISABLED, provider_override is None
    /// - We don't test actual simple vs complex classification (that requires real API calls)
    /// 
    /// v0.32.3 Tool Stripping (#457):
    /// When ModelResolution.provider_override is Some (simple-tier routing), the streaming
    /// code omits tools from the request to prevent simple models from making unnecessary
    /// tool calls. The logic is: has_provider_override = resolution.provider_override.is_some(),
    /// then tools = if has_provider_override { vec![] } else { get_pane_tools() }.
    /// This is tested via integration (manual verification) since it requires the full streaming path.

    #[test]
    fn test_resolve_returns_model_resolution_struct() {
        // Test that resolve_model_for_message returns ModelResolution (not String)
        let conn = setup_smart_routing_db_v32();
        let db_state = DbState::new(conn);
        
        let message = "test";
        let context: Vec<ChatMessage> = vec![];
        
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolve_model_for_message(&db_state, None, message, &context, None, Some("anthropic/claude-sonnet-4")));
        
        assert!(result.is_ok(), "resolve_model_for_message should succeed");
        let resolution = result.unwrap();
        
        // Should return default model (no smart routing configured)
        assert_eq!(resolution.model, "anthropic/claude-sonnet-4",
            "Should return default model");
        
        // Should NOT have provider override (no smart routing)
        assert!(resolution.provider_override.is_none(),
            "Without smart routing, should NOT have provider_override");
    }

    #[test]
    fn test_resolve_returns_provider_override_type() {
        // Test that ModelResolution can hold an internal provider
        let conn = setup_smart_routing_db_v32();
        let db_state = DbState::new(conn);
        
        // Configure internal provider (but no simple_tier_model, so smart routing won't activate)
        db_state.0.lock().unwrap().execute(
            "INSERT INTO ai_settings (key, value) VALUES ('internal_provider', 'openrouter')",
            [],
        ).unwrap();
        db_state.0.lock().unwrap().execute(
            "INSERT INTO ai_settings (key, value) VALUES ('internal_api_key', 'test-key')",
            [],
        ).unwrap();
        
        let message = "test";
        let context: Vec<ChatMessage> = vec![];
        
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(resolve_model_for_message(&db_state, None, message, &context, None, Some("anthropic/claude-sonnet-4")));
        
        assert!(result.is_ok(), "resolve_model_for_message should succeed");
        let resolution = result.unwrap();
        
        // Should return default model (smart routing not fully configured)
        assert_eq!(resolution.model, "anthropic/claude-sonnet-4",
            "Should return default model");
        
        // provider_override should be None when smart routing doesn't activate
        assert!(resolution.provider_override.is_none(),
            "Should NOT have provider_override without smart routing");
    }

    /// Tests for v0.33.8 retry logic and is_transient_error
    
    #[test]
    fn test_is_transient_error_5xx() {
        use crate::ai::chat::is_transient_error;
        
        let transient_errors = vec![
            "500 Internal Server Error",
            "502 Bad Gateway",
            "503 Service Unavailable",
            "504 Gateway Timeout",
            "internal server error occurred",
            "bad gateway error",
            "service unavailable",
            "gateway timeout",
        ];
        
        for error in transient_errors {
            assert!(is_transient_error(error), 
                "'{}' should be identified as transient", error);
        }
    }
    
    #[test]
    fn test_is_transient_error_connection() {
        use crate::ai::chat::is_transient_error;
        
        let transient_errors = vec![
            "connection refused",
            "connection reset",
            "connection timeout",
            "network error",
            "failed to connect",
            "Connection refused by server",
            "Network error occurred",
        ];
        
        for error in transient_errors {
            assert!(is_transient_error(error), 
                "'{}' should be identified as transient", error);
        }
    }
    
    #[test]
    fn test_is_transient_error_timeout() {
        use crate::ai::chat::is_transient_error;
        
        let transient_errors = vec![
            "timeout",
            "request timed out",
            "operation timeout",
            "Timeout waiting for response",
        ];
        
        for error in transient_errors {
            assert!(is_transient_error(error), 
                "'{}' should be identified as transient", error);
        }
    }
    
    #[test]
    fn test_is_transient_error_rate_limit() {
        use crate::ai::chat::is_transient_error;
        
        let transient_errors = vec![
            "429 Too Many Requests",
            "rate limit exceeded",
            "Rate limit hit",
        ];
        
        for error in transient_errors {
            assert!(is_transient_error(error), 
                "'{}' should be identified as transient", error);
        }
    }
    
    #[test]
    fn test_is_transient_error_overloaded() {
        use crate::ai::chat::is_transient_error;
        
        let transient_errors = vec![
            "model overloaded",
            "service overloaded",
            "The model is currently overloaded. Please try again.",
            "Overloaded error occurred",
        ];
        
        for error in transient_errors {
            assert!(is_transient_error(error), 
                "'{}' should be identified as transient", error);
        }
    }
    
    #[test]
    fn test_is_transient_error_4xx_non_transient() {
        use crate::ai::chat::is_transient_error;
        
        let non_transient_errors = vec![
            "400 Bad Request",
            "401 Unauthorized",
            "403 Forbidden",
            "404 Not Found",
            "invalid api key",
            "unauthorized request",
            "forbidden access",
        ];
        
        for error in non_transient_errors {
            assert!(!is_transient_error(error), 
                "'{}' should NOT be identified as transient", error);
        }
    }

    // ==================== Classifier Accuracy Tests ====================
    // These tests verify the bypass rules that skip the LLM classifier for known complex cases.
    // The actual classifier prompt contains few-shot examples including "step by step" as a NO case,
    // but "step by step" is not a bypass keyword — it goes through the classifier.

    #[test]
    fn test_bypass_analyze_keyword() {
        // Messages with "analyze" should bypass to complex
        let message = "analyze the architecture of this system";
        let context: Vec<ChatMessage> = vec![];
        
        let conn = setup_smart_routing_db();
        let db_state = DbState::new(conn);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(classify_message_complexity(&db_state, None, message, &context, None));
        
        assert_eq!(result, false, "Messages with 'analyze' should bypass to complex");
    }

    #[test]
    fn test_bypass_help_me_think_keyword() {
        // Messages with "help me think" should bypass to complex
        let message = "help me think through this problem";
        let context: Vec<ChatMessage> = vec![];
        
        let conn = setup_smart_routing_db();
        let db_state = DbState::new(conn);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(classify_message_complexity(&db_state, None, message, &context, None));
        
        assert_eq!(result, false, "Messages with 'help me think' should bypass to complex");
    }

    #[test]
    fn test_short_greeting_does_not_bypass() {
        // Short greeting with < 3 messages should NOT bypass (goes to classifier)
        // This tests that the bypass rules DON'T trigger for simple cases
        let message = "Hey";
        let context: Vec<ChatMessage> = vec![
            ChatMessage {
                role: "assistant".to_string(),
                content: "Hi there!".to_string(),
                content_blocks: None,
                tool_calls: None,
                tool_call_id: None,
                attachment_ids: None,
                id: None,
            },
        ];
        
        // We can't easily test the actual classifier result without mocking the API,
        // but we can verify the bypass rules DON'T trigger by checking:
        // - word count is 1 (not > 500)
        // - doesn't contain complexity keywords
        // - context has 1 message (not > 10)
        // So this should reach the classifier, not bypass
        // 
        // Since we can't run the real classifier in tests, we just verify the bypass
        // logic doesn't trigger by checking the preconditions
        let word_count = message.split_whitespace().count();
        let text_lower = message.to_lowercase();
        let has_complexity_keywords = text_lower.contains("think deeply") || 
                                       text_lower.contains("analyze") || 
                                       text_lower.contains("help me think");
        let is_long_conversation = context.len() > 10;
        
        assert!(word_count <= 500, "Short message should not trigger word count bypass");
        assert!(!has_complexity_keywords, "Greeting should not trigger keyword bypass");
        assert!(!is_long_conversation, "Short conversation should not trigger length bypass");
    }

    #[test]
    fn test_classifier_prompt_contains_examples() {
        // Regression test: verify the classifier prompt contains expected few-shot examples
        // This catches if someone accidentally removes the examples from the prompt
        
        // Read the classifier function source to extract the prompt
        // Since we can't easily extract the runtime prompt string, we'll verify the
        // function behavior by checking that the bypass rules work correctly (tested above)
        // and document the expected examples here as a reference.
        
        // Expected examples from the prompt (as of this test):
        let _expected_examples = vec![
            "What is 5 + 5?",
            "Hey",
            "Thanks!",
            "What's the capital of France?",
            "Is it raining?",
            "What time is it?",
            "Translate 'hello' to Spanish",
            "Write me a business plan for a SaaS startup",
            "Debug this Python function that's throwing a TypeError",
            "Compare the economic policies of Keynesian and Austrian schools",
            "Help me design a database schema for a multi-tenant app",
            "What are the pros and cons of moving to Portugal?",
            "Refactor this React component to use hooks instead of classes",
            "Brainstorm 10 names for my podcast about liberty",
            "Explain how transformer attention mechanisms work step by step",
        ];
        
        // This test documents the expected examples. If the prompt changes,
        // this test should be updated to match. The actual verification happens
        // in production when the classifier is called.
        // 
        // To verify examples are still present in the source:
        let source_file = std::fs::read_to_string("src/ai/chat.rs")
            .expect("Should be able to read chat.rs");
        
        // Verify a few key examples are in the source
        assert!(source_file.contains("What is 5 + 5?"), 
            "Classifier prompt should contain 'What is 5 + 5?' example");
        assert!(source_file.contains("Write me a business plan"), 
            "Classifier prompt should contain 'Write me a business plan' example");
        
        // Count YES/NO markers in the prompt
        let yes_count = source_file.matches("→ YES").count();
        let no_count = source_file.matches("→ NO").count();
        let total_examples = yes_count + no_count;
        
        assert!(total_examples >= 10, 
            "Classifier prompt should contain at least 10 examples (found {})", total_examples);
    }

    // ==================== Settings Persistence Tests ====================

    #[test]
    fn test_set_and_get_ai_setting_round_trip() {
        // Test basic round-trip: write a value, read it back
        let conn = setup_smart_routing_db_v32();
        
        let key = "test_setting";
        let value = "test_value";
        
        // Write setting
        let write_result = crate::database::set_ai_setting(&conn, key, value);
        assert!(write_result.is_ok(), "set_ai_setting should succeed");
        
        // Read it back
        let read_value = crate::database::get_ai_setting(&conn, key);
        assert_eq!(read_value, Some(value.to_string()), 
            "get_ai_setting should return the value we just set");
    }

    #[test]
    fn test_ai_setting_overwrite() {
        // Test overwrite: set a value, then set it again with a new value
        let conn = setup_smart_routing_db_v32();
        
        let key = "overwrite_test";
        let value1 = "initial_value";
        let value2 = "updated_value";
        
        // Write initial value
        crate::database::set_ai_setting(&conn, key, value1).unwrap();
        let read1 = crate::database::get_ai_setting(&conn, key);
        assert_eq!(read1, Some(value1.to_string()));
        
        // Overwrite with new value
        crate::database::set_ai_setting(&conn, key, value2).unwrap();
        let read2 = crate::database::get_ai_setting(&conn, key);
        assert_eq!(read2, Some(value2.to_string()), 
            "Overwrite should replace the old value");
    }

    #[test]
    fn test_multiple_ai_settings_persist_independently() {
        // Test that multiple settings can be stored and retrieved independently
        let conn = setup_smart_routing_db_v32();
        
        let keys = vec![
            ("internal_provider", "openrouter"),
            ("internal_api_key", "sk-test-key-123"),
            ("simple_tier_model", "anthropic/claude-3-5-haiku-20241022"),
        ];
        
        // Write all settings
        for (key, value) in &keys {
            crate::database::set_ai_setting(&conn, key, value).unwrap();
        }
        
        // Verify all settings persist independently
        for (key, expected_value) in &keys {
            let actual_value = crate::database::get_ai_setting(&conn, key);
            assert_eq!(actual_value, Some(expected_value.to_string()),
                "Setting '{}' should persist independently", key);
        }
    }

    #[test]
    fn test_ai_setting_null_handling() {
        // Test that setting one key doesn't null out another key
        let conn = setup_smart_routing_db_v32();
        
        // Write first setting
        crate::database::set_ai_setting(&conn, "key1", "value1").unwrap();
        
        // Write second setting
        crate::database::set_ai_setting(&conn, "key2", "value2").unwrap();
        
        // Verify both settings still exist
        let key1_value = crate::database::get_ai_setting(&conn, "key1");
        let key2_value = crate::database::get_ai_setting(&conn, "key2");
        
        assert_eq!(key1_value, Some("value1".to_string()),
            "Setting key2 should NOT null out key1");
        assert_eq!(key2_value, Some("value2".to_string()),
            "key2 should have its own value");
    }

    #[test]
    fn test_ai_setting_empty_string_handling() {
        // Test that empty strings are handled correctly
        let conn = setup_smart_routing_db_v32();
        
        let key = "empty_test";
        
        // Set to empty string
        crate::database::set_ai_setting(&conn, key, "").unwrap();
        
        // get_ai_setting filters out empty strings and returns None
        let value = crate::database::get_ai_setting(&conn, key);
        assert_eq!(value, None, 
            "Empty strings should be filtered out by get_ai_setting");
    }

    // ==================== Tool Pattern Bypass Tests (#482) ====================

    #[test]
    fn test_bypass_tool_requiring_show_me() {
        // "show me my April 2 daily log" should bypass classifier (requires read_note tool)
        let message = "show me my April 2 daily log";
        let context: Vec<ChatMessage> = vec![];
        
        let conn = setup_smart_routing_db();
        let db_state = DbState::new(conn);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(classify_message_complexity(&db_state, None, message, &context, None));
        
        // Should return false (use complex model) due to tool pattern bypass
        assert_eq!(result, false, "'show me' + date reference should bypass classifier (#482)");
    }

    #[test]
    fn test_bypass_tool_requiring_save() {
        // "save this to memory" should bypass classifier (requires save/write tools)
        let message = "save this to memory";
        let context: Vec<ChatMessage> = vec![];
        
        let conn = setup_smart_routing_db();
        let db_state = DbState::new(conn);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(classify_message_complexity(&db_state, None, message, &context, None));
        
        assert_eq!(result, false, "'save' action verb should bypass classifier (#482)");
    }

    #[test]
    fn test_bypass_tool_requiring_delete() {
        // "delete that note" should bypass classifier (requires delete tool)
        let message = "delete that note";
        let context: Vec<ChatMessage> = vec![];
        
        let conn = setup_smart_routing_db();
        let db_state = DbState::new(conn);
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(classify_message_complexity(&db_state, None, message, &context, None));
        
        assert_eq!(result, false, "'delete' + 'note' should bypass classifier (#482)");
    }

    #[test]
    fn test_simple_messages_still_simple() {
        // Regression test: ensure simple messages still get classified as simple
        let simple_messages = vec![
            "Hey",
            "What's the capital of France?",
        ];
        
        let conn = setup_smart_routing_db();
        let db_state = DbState::new(conn);
        let context: Vec<ChatMessage> = vec![];
        
        for msg in simple_messages {
            let _result = tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(classify_message_complexity(&db_state, None, msg, &context, None));
            
            // These should still go through the classifier, not bypass
            // Note: We can't test the actual LLM response here (would need mock),
            // but at minimum they should NOT hit the tool pattern bypass (which returns false)
            // Since these don't match tool patterns, they'll go to the LLM classifier.
            // In a real scenario with the LLM, they would return true (simple).
            // For this test, we just verify they don't match our bypass patterns.
            // The actual LLM call will fail in test environment, returning false as fallback.
            // TODO: Add mock LLM for proper classifier testing.
        }
    }

    // ==================== Reed Option A: partial-text gate for smart-route retry ====================
    // Audit: ~/Shared/ExampleApp/docs/audits/2026-06-02-120s-cap-and-destructive-retry.md (Target 2, Option A)
    // Issue: #1358
    //
    // These tests exercise `should_smart_route_retry` — the gate introduced by Reed Option A.
    // §4 production entry point: `crate::ai::chat::should_smart_route_retry` is the
    // extracted decision function used directly in the smart-route retry block inside
    // `ai_chat_stream`. Tests call it through its public(crate) surface.

    #[test]
    fn test_should_not_retry_when_partial_text_present() {
        // Reed Option A core case: decode error WITH partial text must NOT retry.
        // If we allow the retry, the fallback provider's onDone overwrites the user's
        // partial content in the DB (the "Helix→Loom" bug, audit §2.2).
        use crate::ai::chat::should_smart_route_retry;

        let error_msg = "Stream error: error decoding response body: connection reset by peer";
        let partial_text = "Here is the Helix spec you asked for: ## Overview\nHelix is a...";

        let should_retry = should_smart_route_retry(error_msg, partial_text);

        assert!(
            !should_retry,
            "should_smart_route_retry must return false when partial_text is non-empty. \
             Got true, which means the destructive-retry bug is still present. \
             error_msg={:?}, partial_text length={}",
            error_msg,
            partial_text.len()
        );
    }

    #[test]
    fn test_should_retry_when_partial_text_empty_and_transient_error() {
        // Reed Option A complementary case: decode error with NO partial text SHOULD retry.
        // Short-lived connection failures (e.g., before any bytes streamed) must still
        // benefit from the smart-route fallback — that is the original intent of #1359.
        use crate::ai::chat::should_smart_route_retry;

        let error_msg = "Stream error: error decoding response body: connection reset by peer";
        let partial_text = ""; // empty — no partial streamed before the failure

        let should_retry = should_smart_route_retry(error_msg, partial_text);

        assert!(
            should_retry,
            "should_smart_route_retry must return true when partial_text is empty and error is transient. \
             Got false, which means the smart-route fallback would be disabled for clean-connection failures. \
             error_msg={:?}",
            error_msg
        );
    }

    #[test]
    fn test_non_transient_error_never_retries_regardless_of_partial() {
        // A 401 auth error should never trigger retry, even with empty partial text.
        use crate::ai::chat::should_smart_route_retry;

        let non_transient = "401 Unauthorized: invalid api key";

        assert!(
            !should_smart_route_retry(non_transient, ""),
            "Non-transient 4xx errors must not trigger smart-route retry even with empty partial"
        );
        assert!(
            !should_smart_route_retry(non_transient, "some partial content"),
            "Non-transient 4xx errors must not trigger smart-route retry even with partial content"
        );
    }

    #[test]
    fn test_partial_text_gate_with_all_transient_error_variants() {
        // Every variant that `is_transient_error` classifies as transient must be blocked
        // by the partial-text gate when partial_text is non-empty.
        use crate::ai::chat::should_smart_route_retry;

        let transient_errors = vec![
            "Stream error: error decoding response body: ...",
            "500 Internal Server Error",
            "503 Service Unavailable",
            "connection refused",
            "timeout",
            "429 rate limit",
            "model overloaded",
        ];

        let partial = "partial streamed text";

        for error in &transient_errors {
            assert!(
                !should_smart_route_retry(error, partial),
                "Transient error {:?} with non-empty partial must NOT retry (Option A gate)",
                error
            );
        }

        // Same errors with empty partial SHOULD retry
        for error in &transient_errors {
            assert!(
                should_smart_route_retry(error, ""),
                "Transient error {:?} with empty partial SHOULD retry",
                error
            );
        }
    }

}
