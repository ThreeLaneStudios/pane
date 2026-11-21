/// Integration tests for the param-compat stripping layer in the chat route.
///
/// These tests verify the end-to-end behavior: a request comes in to the gateway's
/// `/v1/chat/completions`, the gateway strips deprecated params for models that
/// reject them, and the forwarded request to the OC HTTP backend is correctly modified.
///
/// Architecture:
///   - A mock HTTP server captures what the gateway forwards to "OC"
///   - The `TestGateway` is configured with a compat table that has rules for claude-opus-4-7
///   - We assert on the captured body, not on any downstream response
use axum::routing::post;
use axum::{Router, Json, extract::State};
use chrono::Utc;
use pane_gateway::{
    auth::AuthState,
    oc_client::OcClient,
    param_compat::CompatTable,
    server::SharedState,
    agents::{AgentInfo, AgentRegistry},
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio::sync::{RwLock, Mutex};

// ── Mock HTTP backend ─────────────────────────────────────────────────────────

/// A minimal HTTP server that captures the body of every POST request.
/// Used to verify what the gateway forwards to OC's HTTP API.
struct MockHttpBackend {
    addr: SocketAddr,
    captured_bodies: Arc<Mutex<Vec<Value>>>,
}

impl MockHttpBackend {
    async fn start() -> Self {
        let captured_bodies: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured_bodies.clone();

        let app = Router::new()
            .route("/v1/chat/completions", post(capture_body))
            .with_state(captured_clone);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Give the server a moment to be ready
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        MockHttpBackend { addr, captured_bodies }
    }

    fn http_base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn take_captured_bodies(&self) -> Vec<Value> {
        std::mem::take(&mut *self.captured_bodies.lock().await)
    }
}

/// Axum handler for the mock backend — captures the request body
async fn capture_body(
    State(bodies): State<Arc<Mutex<Vec<Value>>>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    eprintln!("[MOCK HTTP BACKEND] Captured body: {}", body);
    bodies.lock().await.push(body);
    // Return a minimal chat completion response so the gateway doesn't error
    Json(json!({
        "id": "test-completion",
        "object": "chat.completion",
        "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]
    }))
}

// ── Test gateway with configurable compat table + HTTP backend ────────────────

/// Construct a seeded `AgentInfo` for the "main" agent with the given model string.
/// Used to pre-populate the agent_registry cache so tests don't require a live OC
/// WebSocket connection after the #1323 fix (agent_registry is consulted unconditionally).
fn make_main_agent(model: &str) -> AgentInfo {
    AgentInfo {
        oc_agent_id: "main".to_string(),
        name: "main".to_string(),
        model: Some(model.to_string()),
        role: None,
        workspace: None,
        registered_at: Utc::now(),
    }
}

/// Build a gateway connected to a real compat table and a mock HTTP backend.
async fn build_chat_test_gateway(
    compat_table: Arc<CompatTable>,
    mock_backend: &MockHttpBackend,
) -> (Arc<SharedState>, String, String, reqwest::Client) {
    use pane_gateway::idempotency::IdempotencyStore;
    use pane_gateway::metrics::Metrics;
    use pane_gateway::ring_buffer::RingBuffer;
    use pane_gateway::routes;
    use pane_gateway::sync_queue::SyncQueue;
    use pane_gateway::auth::auth_middleware;
    use axum::middleware;

    let tokens_path = std::env::temp_dir().join(format!(
        "test-tokens-chat-compat-{}.json",
        uuid::Uuid::new_v4()
    ));
    let auth_state = Arc::new(AuthState::new(tokens_path));
    let pairing_code = auth_state.generate_pairing_code().await;
    let token_info = auth_state
        .exchange_code_for_token(&pairing_code)
        .await
        .unwrap();
    let auth_token = token_info.token;

    let oc_client = Arc::new(
        OcClient::new(
            "ws://127.0.0.1:1".to_string(), // will never be called for chat tests
            "test-token".to_string(),
        )
        .unwrap(),
    );

    let shared_state = Arc::new(SharedState {
        start_time: OffsetDateTime::now_utc(),
        tls_fingerprint: "test-fp".to_string(),
        tls_cert_pem: "test-cert".to_string(),
        auth_state: auth_state.clone(),
        oc_client: oc_client.clone(),
        agent_registry: Arc::new(AgentRegistry::new(oc_client.clone())),
        models_cache: Arc::new(routes::models::ModelsCache::new()),
        sync_queue: Arc::new(SyncQueue::new()),
        ring_buffer: Arc::new(RingBuffer::new()),
        idempotency_store: IdempotencyStore::new(),
        metrics: Arc::new(Metrics::new()),
        session_mappings: Arc::new(RwLock::new(HashMap::new())),
        oc_http_base_url: mock_backend.http_base_url(),
        oc_bearer_token: "test-token".to_string(),
        tls_hostname: None,
        param_compat_table: compat_table,
        streaming_idle_timeout_secs: 60,
    });

    // Minimal router — only what we need for chat tests
    let protected = Router::new()
        .route(
            "/v1/chat/completions",
            post(routes::chat::chat_completions)
                .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)),
        )
        .route_layer(middleware::from_fn_with_state(
            shared_state.auth_state.clone(),
            auth_middleware,
        ));

    let app = Router::new()
        .merge(protected)
        .with_state(shared_state.clone());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap();

    (shared_state, base_url, auth_token, client)
}

/// Build the production compat table (the bundled default rules).
fn production_compat_table() -> Arc<CompatTable> {
    let json = include_str!("../../config/param-compat-default.json");
    Arc::new(serde_json::from_str(json).expect("bundled default must parse"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// POST with temperature: 0.7 and model = claude-opus-4-7 → upstream body has NO temperature.
/// §4-helper-test: Test invokes production `chat_completions` route via a real HTTP client
/// against a real gateway server (via `build_chat_test_gateway` helper). The call crosses
/// an HTTP boundary so the §4 AST analyzer doesn't see the production call directly. The
/// helper starts the actual gateway router and the test posts to its `/v1/chat/completions`
/// endpoint; assertions verify the body the gateway forwards to OC.
///
/// NOTE: agent_registry is seeded with the real model (post-#1323 fix, the registry is
/// consulted unconditionally — Pane's body["model"] string is a debug hint only).
#[tokio::test]
async fn test_opus_47_temperature_is_stripped() {
    let mock = MockHttpBackend::start().await;
    let compat = production_compat_table();
    let (state, base_url, token, client) = build_chat_test_gateway(compat, &mock).await;

    // Seed agent_registry: real OC model is claude-opus-4-7.
    state.agent_registry.seed_agents("default", vec![make_main_agent("claude-opus-4-7")]).await;

    let resp = client
        .post(format!("{}/v1/chat/completions", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "model": "claude-opus-4-7",
            "messages": [{"role": "user", "content": "hello"}],
            "temperature": 0.7
        }))
        .send()
        .await
        .expect("request should succeed");

    // The gateway might fail to connect to the (real) OC RPC side, but the HTTP body
    // capture happens in our mock at the HTTP layer. As long as the status is not 500 due
    // to body-processing error, we can check what was captured.
    eprintln!("[TEST] Response status: {}", resp.status());

    let captured = mock.take_captured_bodies().await;
    assert!(!captured.is_empty(), "mock backend should have received the forwarded request");

    let forwarded = &captured[0];
    eprintln!("[TEST] Forwarded body: {}", forwarded);

    assert!(
        forwarded.get("temperature").is_none(),
        "temperature must be stripped for claude-opus-4-7, got body: {}",
        forwarded
    );
    // model must have been overwritten to "openclaw"
    assert_eq!(forwarded["model"].as_str(), Some("openclaw"));
}

/// POST with temperature: 0.7 and model = claude-sonnet-4-6 → upstream body retains temperature.
/// §4-helper-test: invokes production `chat_completions` via real HTTP boundary; see
/// `test_opus_47_temperature_is_stripped` for the rationale.
///
/// NOTE: agent_registry is seeded with claude-sonnet-4-6 (no strip rules for this model).
#[tokio::test]
async fn test_sonnet_46_temperature_is_not_stripped() {
    let mock = MockHttpBackend::start().await;
    let compat = production_compat_table();
    let (state, base_url, token, client) = build_chat_test_gateway(compat, &mock).await;

    // Seed agent_registry: real OC model is claude-sonnet-4-6 (no strip rules).
    state.agent_registry.seed_agents("default", vec![make_main_agent("claude-sonnet-4-6")]).await;

    let resp = client
        .post(format!("{}/v1/chat/completions", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "model": "claude-sonnet-4-6",
            "messages": [{"role": "user", "content": "hello"}],
            "temperature": 0.7
        }))
        .send()
        .await
        .expect("request should succeed");

    eprintln!("[TEST] Response status: {}", resp.status());

    let captured = mock.take_captured_bodies().await;
    assert!(!captured.is_empty(), "mock backend should have received the forwarded request");

    let forwarded = &captured[0];
    eprintln!("[TEST] Forwarded body: {}", forwarded);

    assert_eq!(
        forwarded["temperature"].as_f64(),
        Some(0.7),
        "temperature must NOT be stripped for claude-sonnet-4-6, got body: {}",
        forwarded
    );
}

/// POST with top_p: 0.9 and model = claude-opus-4-7 → upstream body has NO top_p.
/// §4-helper-test: invokes production `chat_completions` via real HTTP boundary; see
/// `test_opus_47_temperature_is_stripped` for the rationale.
///
/// NOTE: agent_registry is seeded with claude-opus-4-7 (real model from registry).
#[tokio::test]
async fn test_opus_47_top_p_is_stripped() {
    let mock = MockHttpBackend::start().await;
    let compat = production_compat_table();
    let (state, base_url, token, client) = build_chat_test_gateway(compat, &mock).await;

    // Seed agent_registry: real OC model is claude-opus-4-7.
    state.agent_registry.seed_agents("default", vec![make_main_agent("claude-opus-4-7")]).await;

    let resp = client
        .post(format!("{}/v1/chat/completions", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "model": "claude-opus-4-7",
            "messages": [{"role": "user", "content": "hello"}],
            "top_p": 0.9
        }))
        .send()
        .await
        .expect("request should succeed");

    eprintln!("[TEST] Response status: {}", resp.status());

    let captured = mock.take_captured_bodies().await;
    assert!(!captured.is_empty(), "mock backend should have received the forwarded request");

    let forwarded = &captured[0];
    eprintln!("[TEST] Forwarded body: {}", forwarded);

    assert!(
        forwarded.get("top_p").is_none(),
        "top_p must be stripped for claude-opus-4-7, got body: {}",
        forwarded
    );
}

/// POST with temperature: 0.7 and model = anthropic/claude-opus-4-7 → stripped.
/// §4-helper-test: invokes production `chat_completions` via real HTTP boundary; see
/// `test_opus_47_temperature_is_stripped` for the rationale.
///
/// NOTE: agent_registry is seeded with the prefixed form (anthropic/claude-opus-4-7).
#[tokio::test]
async fn test_anthropic_prefixed_opus_47_temperature_is_stripped() {
    let mock = MockHttpBackend::start().await;
    let compat = production_compat_table();
    let (state, base_url, token, client) = build_chat_test_gateway(compat, &mock).await;

    // Seed agent_registry: real OC model uses the prefixed form.
    state.agent_registry.seed_agents("default", vec![make_main_agent("anthropic/claude-opus-4-7")]).await;

    let _resp = client
        .post(format!("{}/v1/chat/completions", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "model": "anthropic/claude-opus-4-7",
            "messages": [{"role": "user", "content": "hello"}],
            "temperature": 0.7,
            "top_p": 0.9
        }))
        .send()
        .await
        .expect("request should succeed");

    let captured = mock.take_captured_bodies().await;
    assert!(!captured.is_empty());
    let forwarded = &captured[0];

    assert!(
        forwarded.get("temperature").is_none(),
        "temperature must be stripped for anthropic/claude-opus-4-7"
    );
    assert!(
        forwarded.get("top_p").is_none(),
        "top_p must be stripped for anthropic/claude-opus-4-7"
    );
}

/// #1323 regression guard: Pane sends stale body["model"] (cache mismatch), but
/// agent_registry returns the real downstream model → strip rules fire on the real model.
///
/// Scenario:
///   - Pane's local cache says `anthropic/claude-opus-4-6` (stale — model was bumped in OC)
///   - agent_registry mock (seeded cache) returns `claude-opus-4-7` for tenant's main agent
///   - Strip table has rules for `claude-opus-4-7` (strips temperature, top_p)
///   - Expected: temperature and top_p are stripped from the forwarded body
///   - Without the #1323 fix: common path trusts Pane's string → no rule for `claude-opus-4-6`
///     → temperature and top_p NOT stripped → Anthropic 422
///   - With the fix: registry is always consulted → `claude-opus-4-7` resolved → strip fires
///
/// §4-helper-test: invokes production `chat_completions` via real HTTP boundary; see
/// `test_opus_47_temperature_is_stripped` for the rationale.
#[tokio::test]
async fn test_stale_pane_model_uses_registry_model_for_strip() {
    let mock = MockHttpBackend::start().await;
    let compat = production_compat_table();
    let (state, base_url, token, client) = build_chat_test_gateway(compat, &mock).await;

    // Seed agent_registry: OC is actually running claude-opus-4-7 (the real model).
    // Strip rules exist for claude-opus-4-7 but NOT for claude-opus-4-6.
    state.agent_registry.seed_agents("default", vec![make_main_agent("claude-opus-4-7")]).await;

    // Pane sends stale model string from its cache (claude-opus-4-6 — no strip rule).
    // Without the fix, this would be trusted directly and temperature/top_p would slip through.
    let _resp = client
        .post(format!("{}/v1/chat/completions", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .json(&json!({
            "model": "anthropic/claude-opus-4-6",
            "messages": [{"role": "user", "content": "hello"}],
            "temperature": 0.7,
            "top_p": 0.9
        }))
        .send()
        .await
        .expect("request should succeed");

    let captured = mock.take_captured_bodies().await;
    assert!(
        !captured.is_empty(),
        "mock backend should have received the forwarded request"
    );
    let forwarded = &captured[0];
    eprintln!("[TEST] Forwarded body (stale-model scenario): {}", forwarded);

    // The strip table was consulted using claude-opus-4-7 (from registry), NOT
    // anthropic/claude-opus-4-6 (Pane's stale string). Both params must be absent.
    assert!(
        forwarded.get("temperature").is_none(),
        "temperature must be stripped: registry returned claude-opus-4-7 which has a strip rule, \
         but got body: {}",
        forwarded
    );
    assert!(
        forwarded.get("top_p").is_none(),
        "top_p must be stripped: registry returned claude-opus-4-7 which has a strip rule, \
         but got body: {}",
        forwarded
    );
    // model must have been overwritten to "openclaw" (§22C)
    assert_eq!(
        forwarded["model"].as_str(),
        Some("openclaw"),
        "model must be rewritten to openclaw for §22C agent routing"
    );
}

/// Config-file test: missing file writes default on first run.
/// §4-helper-test: Calls `CompatTable::load_or_default` which is an impl-method on
/// the production type. The §4 AST analyzer's symbol collector only walks `ItemFn`
/// (free functions), so impl methods are not in the production-symbols set. See #1333
/// for the analyzer follow-on. The test exercises real production code (load_or_default
/// reads/writes the on-disk config), not a stub.
#[tokio::test]
async fn test_missing_config_file_falls_back_to_default() {
    let dir = tempfile::tempdir().expect("tempdir");
    let result = CompatTable::load_or_default(dir.path());
    assert!(result.is_ok(), "missing file should not error: {:?}", result.err());

    let config_path = dir.path().join("param-compat.json");
    assert!(config_path.exists(), "default must be written to disk on first run");
}

/// Config-file test: malformed JSON returns a clear error (not a panic).
/// §4-helper-test: Calls `CompatTable::load_or_default` (impl method on production
/// type). See `test_missing_config_file_falls_back_to_default` for the rationale and
/// #1333 for the §4 analyzer follow-on that will recognize impl methods.
#[tokio::test]
async fn test_malformed_config_returns_clear_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("param-compat.json"), b"{bad json").unwrap();

    let result = CompatTable::load_or_default(dir.path());
    assert!(result.is_err(), "malformed config should return an error");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("param-compat") || msg.contains("Malformed") || msg.contains("malformed"),
        "error should reference the config file: {}",
        msg
    );
}
