use axum::Router;
use futures_util::{SinkExt, StreamExt};
use pane_gateway::{
    auth::AuthState, oc_client::OcClient, param_compat::CompatTable, server::SharedState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, RwLock};
use tokio_tungstenite::{accept_async, tungstenite::Message};

/// OC protocol request
#[derive(Debug, Serialize, Deserialize, Clone)]
struct OcRequest {
    #[serde(rename = "type")]
    frame_type: String,
    id: Value,
    method: String,
    params: Value,
}

/// OC protocol response
#[derive(Debug, Serialize, Deserialize)]
struct OcResponse {
    #[serde(rename = "type")]
    frame_type: String,
    id: Value,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<OcError>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OcError {
    code: String,
    message: String,
}

/// Mock OC WebSocket server that speaks JSON-RPC 2.0
pub struct MockOcServer {
    addr: SocketAddr,
    responses: Arc<RwLock<HashMap<String, Value>>>,
    calls: Arc<RwLock<Vec<(String, Value)>>>,
    shutdown_tx: Option<mpsc::UnboundedSender<()>>,
}

impl MockOcServer {
    /// Start the mock server on a random port
    pub async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let responses = Arc::new(RwLock::new(HashMap::new()));
        let calls = Arc::new(RwLock::new(Vec::new()));
        let (shutdown_tx, mut shutdown_rx) = mpsc::unbounded_channel();

        let responses_clone = responses.clone();
        let calls_clone = calls.clone();

        // Spawn server task
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.recv() => {
                        eprintln!("[MOCK OC] Shutting down");
                        break;
                    }
                    accept_result = listener.accept() => {
                        match accept_result {
                            Ok((stream, addr)) => {
                                eprintln!("[MOCK OC] New connection from {}", addr);
                                let responses = responses_clone.clone();
                                let calls = calls_clone.clone();
                                tokio::spawn(handle_connection(stream, responses, calls));
                            }
                            Err(e) => {
                                eprintln!("[MOCK OC] Accept error: {}", e);
                            }
                        }
                    }
                }
            }
        });

        Ok(Self {
            addr,
            responses,
            calls,
            shutdown_tx: Some(shutdown_tx),
        })
    }

    /// Configure a response for a specific method
    pub async fn configure_response(&self, method: &str, response: Value) {
        self.responses.write().await.insert(method.to_string(), response);
    }

    /// Get all recorded calls (method, params)
    pub async fn get_calls(&self) -> Vec<(String, Value)> {
        self.calls.read().await.clone()
    }

    /// Get the WebSocket URL for this mock server
    pub fn ws_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// Clear recorded calls
    pub async fn clear_calls(&self) {
        self.calls.write().await.clear();
    }
}

impl Drop for MockOcServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Handle a single WebSocket connection
async fn handle_connection(
    stream: TcpStream,
    responses: Arc<RwLock<HashMap<String, Value>>>,
    calls: Arc<RwLock<Vec<(String, Value)>>>,
) {
    eprintln!("[MOCK OC] Handling WebSocket connection");
    
    let ws_stream = match accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("[MOCK OC] WebSocket handshake error: {}", e);
            return;
        }
    };

    let (mut write, mut read) = ws_stream.split();
    
    // Send connect.challenge event immediately after connection
    let challenge_event = json!({
        "type": "event",
        "event": "connect.challenge",
        "payload": {
            "nonce": "mock-nonce-12345"
        }
    });
    eprintln!("[MOCK OC] Sending connect.challenge");
    if let Err(e) = write.send(Message::Text(serde_json::to_string(&challenge_event).unwrap())).await {
        eprintln!("[MOCK OC] Failed to send challenge: {}", e);
        return;
    }

    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                eprintln!("[MOCK OC] Received: {}", text);
                
                // Parse OC request
                let request: OcRequest = match serde_json::from_str(&text) {
                    Ok(req) => req,
                    Err(e) => {
                        eprintln!("[MOCK OC] Failed to parse request: {}", e);
                        continue;
                    }
                };

                // Record the call
                calls.write().await.push((request.method.clone(), request.params.clone()));

                // Build OC response
                let response = if request.method == "connect" {
                    // Return hello-ok for connect requests
                    OcResponse {
                        frame_type: "res".to_string(),
                        id: request.id,
                        ok: true,
                        payload: Some(json!({
                            "type": "hello-ok",
                            "protocol": 3,
                            "server": {
                                "connId": "mock-conn-123",
                                "version": "1.0.0"
                            },
                            "features": {},
                            "snapshot": {},
                            "policy": {}
                        })),
                        error: None,
                    }
                } else {
                    // Look up configured response
                    let responses_guard = responses.read().await;
                    let response_value = responses_guard.get(&request.method).cloned();
                    drop(responses_guard);
                    
                    if let Some(result) = response_value {
                        OcResponse {
                            frame_type: "res".to_string(),
                            id: request.id,
                            ok: true,
                            payload: Some(result),
                            error: None,
                        }
                    } else {
                        // Default empty success response
                        OcResponse {
                            frame_type: "res".to_string(),
                            id: request.id,
                            ok: true,
                            payload: Some(json!({})),
                            error: None,
                        }
                    }
                };

                let response_text = serde_json::to_string(&response).unwrap();
                eprintln!("[MOCK OC] Sending: {}", response_text);
                
                if let Err(e) = write.send(Message::Text(response_text)).await {
                    eprintln!("[MOCK OC] Failed to send response: {}", e);
                    break;
                }
            }
            Ok(Message::Close(_)) => {
                eprintln!("[MOCK OC] Connection closed");
                break;
            }
            Ok(Message::Ping(data)) => {
                let _ = write.send(Message::Pong(data)).await;
            }
            Err(e) => {
                eprintln!("[MOCK OC] WebSocket error: {}", e);
                break;
            }
            _ => {}
        }
    }
}

/// Test gateway builder
pub struct TestGateway {
    mock: MockOcServer,
    shared_state: Arc<SharedState>,
    base_url: String,
    client: reqwest::Client,
    auth_token: String,
}

impl TestGateway {
    /// Create a new test gateway
    pub async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        eprintln!("[TEST GATEWAY] Starting mock OC server...");
        let mock = MockOcServer::start().await?;
        
        eprintln!("[TEST GATEWAY] Mock OC server started at {}", mock.ws_url());

        // Create auth state and generate token
        let tokens_path = std::env::temp_dir().join("test-tokens-integration.json");
        let auth_state = Arc::new(AuthState::new(tokens_path));
        let pairing_code = auth_state.generate_pairing_code().await;
        let token_info = auth_state.exchange_code_for_token(&pairing_code).await?;
        let auth_token = token_info.token;
        eprintln!("[TEST GATEWAY] Generated auth token: {}", auth_token);

        // Create OC client pointing at mock server
        eprintln!("[TEST GATEWAY] Creating OC client...");
        let oc_client = Arc::new(OcClient::new(
            mock.ws_url(),
            "test-token".to_string(),
        )?);

        // Build shared state
        let shared_state = Arc::new(SharedState {
            start_time: OffsetDateTime::now_utc(),
            tls_fingerprint: "test-fingerprint".to_string(),
            tls_cert_pem: "test_cert_pem".to_string(),
            auth_state: auth_state.clone(),
            oc_client: oc_client.clone(),
            agent_registry: Arc::new(pane_gateway::agents::AgentRegistry::new(oc_client.clone())),
            models_cache: Arc::new(pane_gateway::routes::models::ModelsCache::new()),
            sync_queue: Arc::new(pane_gateway::sync_queue::SyncQueue::new()),
            ring_buffer: Arc::new(pane_gateway::ring_buffer::RingBuffer::new()),
            idempotency_store: pane_gateway::idempotency::IdempotencyStore::new(),
            metrics: Arc::new(pane_gateway::metrics::Metrics::new()),
            session_mappings: Arc::new(RwLock::new(HashMap::new())),
            oc_http_base_url: "http://localhost:8080".to_string(),
            oc_bearer_token: "test-token".to_string(),
            tls_hostname: None,
            param_compat_table: Arc::new(CompatTable::default()),
            streaming_idle_timeout_secs: 60,
        });

        // Build the router (same as production, but without TLS)
        eprintln!("[TEST GATEWAY] Building router...");
        let app = build_test_router(shared_state.clone());

        // Start HTTP server on random port
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let base_url = format!("http://{}", addr);
        
        eprintln!("[TEST GATEWAY] Starting HTTP server at {}", base_url);

        // Spawn server
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Give server time to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Create HTTP client
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;

        Ok(Self {
            mock,
            shared_state,
            base_url,
            client,
            auth_token,
        })
    }

    /// Get the HTTP client
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Get the base URL
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get the mock OC server
    pub fn mock(&self) -> &MockOcServer {
        &self.mock
    }

    /// Get the shared state
    pub fn shared_state(&self) -> &Arc<SharedState> {
        &self.shared_state
    }

    /// Get the auth token
    pub fn auth_token(&self) -> &str {
        &self.auth_token
    }
}

/// Build the test router (same structure as production)
fn build_test_router(state: Arc<SharedState>) -> Router {
    use axum::{
        extract::DefaultBodyLimit,
        middleware,
        routing::{delete, get, post},
    };
    use pane_gateway::routes;
    use pane_gateway::auth::auth_middleware;

    // Protected routes
    let protected = Router::new()
        .route("/v1/agents", get(routes::get_agents))
        .route("/v1/models", get(routes::models::get_models))
        .route("/v1/metrics", get(routes::metrics_endpoint))
        .route("/v1/chat/completions", post(routes::chat::chat_completions).layer(DefaultBodyLimit::max(1024 * 1024)))
        .route("/v1/sync/push", post(routes::sync::push).layer(DefaultBodyLimit::max(10 * 1024 * 1024)))
        .route("/v1/sync/pull", get(routes::sync::pull))
        .route("/v1/sync/pull/confirm", delete(routes::sync::confirm))
        .route("/v1/sync/initial", get(routes::sync::initial))
        .route("/v1/sessions", post(routes::sessions::create).get(routes::sessions::list))
        .route("/v1/sessions/:session_id", delete(routes::sessions::delete_session))
        .route("/v1/sessions/:session_id/messages", get(routes::sessions::messages).post(routes::sessions::send_message).layer(DefaultBodyLimit::max(1024 * 1024)))
        .route("/v1/sessions/:session_id/messages/stream", get(routes::stream::sse_stream))
        .route("/v1/sessions/:session_id/abort", post(routes::sessions::abort))
        .route_layer(middleware::from_fn_with_state(
            state.auth_state.clone(),
            auth_middleware,
        ));

    // Main router
    Router::new()
        .route("/pair", post(routes::pair::pair))
        .route("/v1/health", get(routes::health::health_check))
        .merge(protected)
        .with_state(state)
}

// Test modules
mod auth_tests;
mod sync_tests;
mod session_tests;
mod health_tests;
mod error_tests;
pub mod param_compat_tests;
