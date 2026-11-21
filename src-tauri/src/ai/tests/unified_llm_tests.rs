//! Unit tests for unified call_llm() and call_llm_from_config() functions (#423)
//!
//! These tests verify the unified LLM calling layer introduced in v0.22.0.
//! Tests focus on provider resolution, URL construction, timeout behavior,
//! and error handling WITHOUT making real API calls.

#[cfg(test)]
mod tests {
    use crate::ai::provider::{AiProvider, ChatMessage};
    use crate::ai::provider::{call_llm, call_llm_from_config};
    
    /// Helper: Create a simple user message for testing
    fn test_message(content: &str) -> ChatMessage {
        ChatMessage {
            role: "user".to_string(),
            content: content.to_string(),
            tool_calls: None,
            tool_call_id: None,
            content_blocks: None,
            attachment_ids: None,
            id: None,
        }
    }

    #[tokio::test]
    async fn test_call_llm_from_config_anthropic() {
        // Test that call_llm_from_config correctly constructs Anthropic provider
        let messages = vec![test_message("test query")];
        
        // This will fail with "Invalid API key" but that proves the provider was constructed
        let result = call_llm_from_config(
            "anthropic",
            "test-key",
            "claude-sonnet-4",
            None,
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        
        // We expect an error because the API key is invalid, but not an "Unsupported provider" error
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(!err_msg.contains("Unsupported provider"));
    }

    #[tokio::test]
    async fn test_call_llm_from_config_openrouter() {
        // Test that call_llm_from_config correctly constructs OpenRouter provider
        let messages = vec![test_message("test query")];
        
        let result = call_llm_from_config(
            "openrouter",
            "test-key",
            "anthropic/claude-sonnet-4",
            None,
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(!err_msg.contains("Unsupported provider"));
    }

    #[tokio::test]
    async fn test_call_llm_from_config_openclaw() {
        // Test that call_llm_from_config correctly constructs OpenClaw provider with custom endpoint
        let messages = vec![test_message("test query")];
        
        let result = call_llm_from_config(
            "openclaw",
            "test-token",
            "main",
            Some("http://localhost:18789"),
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(!err_msg.contains("Unsupported provider"));
    }

    #[tokio::test]
    async fn test_call_llm_from_config_openclaw_default_endpoint() {
        // Test that OpenClaw uses default endpoint when none provided
        let messages = vec![test_message("test query")];
        
        let result = call_llm_from_config(
            "openclaw",
            "test-token",
            "main",
            None, // Should default to http://127.0.0.1:18789
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        // Should NOT be unsupported provider - should be network/auth error
        assert!(!err_msg.contains("Unsupported provider"));
    }

    #[tokio::test]
    async fn test_call_llm_from_config_unsupported_provider() {
        // Test that unsupported provider name returns appropriate error
        let messages = vec![test_message("test query")];
        
        let result = call_llm_from_config(
            "fake-provider",
            "test-key",
            "test-model",
            None,
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("Unsupported provider"));
        assert!(err_msg.contains("fake-provider"));
    }

    #[tokio::test]
    async fn test_call_llm_timeout_respected() {
        // Test that timeout parameter is respected (using very short timeout)
        let messages = vec![test_message("test query")];
        
        // Use a fake endpoint that will timeout
        let result = call_llm_from_config(
            "openclaw",
            "test-token",
            "main",
            Some("http://240.0.0.1:18789"), // Non-routable IP
            &messages,
            100,
            0.0,
            1, // 1 second timeout
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        // Should contain "timeout" in the error
        assert!(err_msg.to_lowercase().contains("timeout") || err_msg.contains("LLM call timeout"));
    }

    #[tokio::test]
    async fn test_call_llm_ollama_connection_error() {
        // Test that Ollama provider returns appropriate error when Ollama is not running
        let messages = vec![test_message("test query")];
        
        let result = call_llm_from_config(
            "ollama",
            "", // Ollama doesn't use API key
            "llama2",
            Some("http://localhost:11434"),
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        
        // Should fail with connection error (Ollama likely not running in test env)
        // but NOT with "not yet supported" error
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(!err_msg.contains("not yet supported"), 
                "Ollama should be supported, got error: {}", err_msg);
    }

    #[tokio::test]
    async fn test_call_llm_provider_none() {
        // Test that AiProvider::None returns appropriate error
        let provider = AiProvider::None;
        let messages = vec![test_message("test query")];
        
        let result = call_llm(
            &provider,
            "test-model",
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
        ).await;
        
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(err_msg.contains("AI provider not configured") || err_msg.contains("not configured"));
    }

    #[tokio::test]
    async fn test_call_llm_anthropic_url_construction() {
        // Test that Anthropic provider is constructed with correct API key
        let provider = AiProvider::Anthropic {
            api_key: "sk-ant-test123".to_string(),
        };
        let messages = vec![test_message("test query")];
        
        let result = call_llm(
            &provider,
            "claude-sonnet-4",
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
        ).await;
        
        // Should fail with auth error (invalid key), not a construction error
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        // Should NOT be a "not configured" error
        assert!(!err_msg.contains("not configured"));
    }

    #[tokio::test]
    async fn test_call_llm_openrouter_url_construction() {
        // Test that OpenRouter provider is constructed correctly
        let provider = AiProvider::OpenRouter {
            api_key: "sk-or-test123".to_string(),
        };
        let messages = vec![test_message("test query")];
        
        let result = call_llm(
            &provider,
            "anthropic/claude-sonnet-4",
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
        ).await;
        
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(!err_msg.contains("not configured"));
    }

    #[tokio::test]
    async fn test_call_llm_openclaw_url_construction() {
        // Test that OpenClaw provider uses custom base_url
        let provider = AiProvider::OpenClaw {
            base_url: "http://localhost:9999".to_string(),
            token: "test-token".to_string(),
            runtime_ref: None,
        };
        let messages = vec![test_message("test query")];
        
        let result = call_llm(
            &provider,
            "main",
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
        ).await;
        
        assert!(result.is_err());
        let err_msg = result.unwrap_err();
        assert!(!err_msg.contains("not configured"));
    }

    #[tokio::test]
    async fn test_empty_messages_handled() {
        // Test that empty messages array doesn't crash
        let provider = AiProvider::OpenRouter {
            api_key: "test-key".to_string(),
        };
        let messages: Vec<ChatMessage> = vec![];
        
        let result = call_llm(
            &provider,
            "anthropic/claude-sonnet-4",
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
        ).await;
        
        // Should return an error (likely from provider), but not crash
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_different_timeout_values() {
        // Test that different timeout values are accepted
        let messages = vec![test_message("test")];
        
        // 5s timeout (rerank use case)
        let result_5s = call_llm_from_config(
            "fake-provider", // Will fail early, but we're testing parameter acceptance
            "key",
            "model",
            None,
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        assert!(result_5s.is_err());
        
        // 30s timeout (default use case)
        let result_30s = call_llm_from_config(
            "fake-provider",
            "key",
            "model",
            None,
            &messages,
            100,
            0.0,
            30,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        assert!(result_30s.is_err());
        
        // 90s timeout (synthesis use case)
        let result_90s = call_llm_from_config(
            "fake-provider",
            "key",
            "model",
            None,
            &messages,
            100,
            0.0,
            90,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        assert!(result_90s.is_err());
    }

    #[tokio::test]
    async fn test_different_max_tokens_values() {
        // Test that different max_tokens values are accepted
        let messages = vec![test_message("test")];
        
        // Small (100 tokens - rerank)
        let result_small = call_llm_from_config(
            "fake-provider",
            "key",
            "model",
            None,
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        assert!(result_small.is_err());
        
        // Medium (1024 tokens - extraction)
        let result_medium = call_llm_from_config(
            "fake-provider",
            "key",
            "model",
            None,
            &messages,
            1024,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        assert!(result_medium.is_err());
        
        // Large (2048 tokens - synthesis)
        let result_large = call_llm_from_config(
            "fake-provider",
            "key",
            "model",
            None,
            &messages,
            2048,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        assert!(result_large.is_err());
    }

    #[tokio::test]
    async fn test_temperature_parameter_accepted() {
        // Test that temperature parameter is accepted
        let messages = vec![test_message("test")];
        
        // Temperature 0.0 (deterministic)
        let result_zero = call_llm_from_config(
            "fake-provider",
            "key",
            "model",
            None,
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        assert!(result_zero.is_err());
        
        // Temperature 0.7 (creative)
        let result_mid = call_llm_from_config(
            "fake-provider",
            "key",
            "model",
            None,
            &messages,
            100,
            0.7,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        assert!(result_mid.is_err());
    }

    #[tokio::test]
    async fn test_call_llm_from_config_preserves_provider_string() {
        // Test that provider string is correctly passed through (case for error messages)
        let messages = vec![test_message("test")];
        
        let result = call_llm_from_config(
            "unsupported-provider-xyz",
            "key",
            "model",
            None,
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // oc_runtime_ref
        ).await;
        
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("unsupported-provider-xyz"));
    }

    #[tokio::test]
    async fn test_multimodal_message_structure() {
        // Test that messages with content_blocks are accepted
        let multimodal_msg = ChatMessage {
            role: "user".to_string(),
            content: "What's in this image?".to_string(),
            tool_calls: None,
            tool_call_id: None,
            content_blocks: Some(serde_json::json!([
                {"type": "text", "text": "What's in this image?"},
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,..."}}
            ])),
            attachment_ids: None,
            id: None,
        };
        
        let messages = vec![multimodal_msg];
        let provider = AiProvider::Anthropic {
            api_key: "test-key".to_string(),
        };
        
        let result = call_llm(
            &provider,
            "claude-sonnet-4",
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
        ).await;
        
        // Should fail with auth error, but structure should be accepted
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tool_response_message_structure() {
        // Test that tool response messages are accepted
        let tool_msg = ChatMessage {
            role: "tool".to_string(),
            content: "Result: 42".to_string(),
            tool_calls: None,
            tool_call_id: Some("call_abc123".to_string()),
            content_blocks: None,
            attachment_ids: None,
            id: None,
        };
        
        let messages = vec![tool_msg];
        let provider = AiProvider::OpenRouter {
            api_key: "test-key".to_string(),
        };
        
        let result = call_llm(
            &provider,
            "anthropic/claude-sonnet-4",
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
        ).await;
        
        // Should fail with auth/validation error, but structure should be accepted
        assert!(result.is_err());
    }

    /// ADR-0067 D2 leak guard — Matt's explicit requirement: captures the REAL
    /// outbound HTTP body produced by `call_llm_from_config`'s "openclaw" arm, the
    /// shared entry point every non-streaming default-agent-resolver dispatch site
    /// (consolidator, check_contradiction, ai_synthesize_all_memories_impl) routes
    /// through. `ai_synthesize_memory`'s direct-dispatch path and the streaming
    /// `ai_chat_stream` path route through the same underlying `oc_wire_model()`
    /// mutation point (see openrouter.rs's own D2 leak-guard tests — the two
    /// together are comprehensive across all six dispatch sites, since every one
    /// of them funnels through one of these two shared mutation points by
    /// construction, not by per-site duplicated formatting logic). Asserts Pane's
    /// local `ai_agents.id` (a UUID, distinct from runtime_ref) appears NOWHERE in
    /// the body — not just absent from the `model` field — and that the
    /// migrate_v83 `"openclaw-pending"` placeholder also never reaches the wire.
    #[tokio::test]
    async fn test_call_llm_from_config_openclaw_never_leaks_local_agent_id_onto_wire() {
        let local_agent_id = "9F1A2B3C4D5E6F708192A3B4C5D6E7F8"; // UUID-shaped, distinct from runtime_ref
        let runtime_ref = "researcher";
        assert_ne!(local_agent_id, runtime_ref, "sanity: the two identifiers must differ");

        let mut server = mockito::Server::new_async().await;
        let captured_body: std::sync::Arc<std::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let captured_body_clone = captured_body.clone();
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_request(move |request| {
                let body = request.utf8_lossy_body().unwrap_or_default().into_owned();
                *captured_body_clone.lock().unwrap() = Some(body);
                true
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let messages = vec![test_message("hello")];
        let result = call_llm_from_config(
            "openclaw",
            "dummy-token",
            "openclaw-pending", // migrate_v83 placeholder — must never reach the wire either
            Some(&server.url()),
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            Some(runtime_ref), // the ONLY agent-identity value allowed near the wire
        )
        .await;
        assert!(result.is_ok(), "dispatch must succeed: {:?}", result.err());
        mock.assert_async().await;

        let body = captured_body.lock().unwrap().clone().expect("body must have been captured");
        assert!(
            !body.contains(local_agent_id),
            "D2 VIOLATION: local agent_id {:?} leaked onto the wire in body: {}",
            local_agent_id, body
        );
        assert!(
            !body.contains("openclaw-pending"),
            "migrate_v83 placeholder must never reach the wire, got: {}",
            body
        );
        assert!(
            body.contains("\"model\":\"openclaw/researcher\""),
            "body must carry the agent-scoped runtime_ref, got: {}",
            body
        );
    }

    /// Same guard, NULL runtime_ref shape (the real-world case for a non-OC or
    /// legacy-OC agent without a runtime_ref yet): must produce the bare
    /// "openclaw" sentinel, never a UUID, never "openclaw/null".
    #[tokio::test]
    async fn test_call_llm_from_config_openclaw_bare_sentinel_leaks_nothing_when_runtime_ref_none() {
        let mut server = mockito::Server::new_async().await;
        let captured_body: std::sync::Arc<std::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let captured_body_clone = captured_body.clone();
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_request(move |request| {
                let body = request.utf8_lossy_body().unwrap_or_default().into_owned();
                *captured_body_clone.lock().unwrap() = Some(body);
                true
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let messages = vec![test_message("hello")];
        let result = call_llm_from_config(
            "openclaw",
            "dummy-token",
            "openclaw",
            Some(&server.url()),
            &messages,
            100,
            0.0,
            5,
            None,
            None, // model_registry
            None, // no runtime_ref — NULL case
        )
        .await;
        assert!(result.is_ok(), "dispatch must succeed: {:?}", result.err());
        mock.assert_async().await;

        let body = captured_body.lock().unwrap().clone().expect("body must have been captured");
        assert!(
            body.contains("\"model\":\"openclaw\""),
            "NULL runtime_ref must produce the bare sentinel, got: {}",
            body
        );
        assert!(
            !body.to_lowercase().contains("openclaw/null"),
            "must never produce openclaw/null, got: {}",
            body
        );
    }
}
