use crate::error::GatewayError;
use crate::device_identity::DeviceIdentity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{timeout, Duration};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};
use tokio_tungstenite::tungstenite::Message;
use futures_util::{SinkExt, StreamExt};

/// Lowest gateway-protocol version this client speaks. Inherited from the v3
/// device-auth signing scheme that OC server still accepts (see protocol.md
/// "Device Auth").
pub(crate) const OC_PROTOCOL_MIN: u32 = 3;

/// Upper bound of gateway-protocol versions this client advertises support
/// for in `connect`. The value is intentionally broad: OC's protocol.md §644
/// promises that the server "rejects ranges that do not include its current
/// protocol," which means a wide max is the documented forward-compatible
/// pattern. This trusts OC's additive-versioning promise — a TRULY wire-
/// breaking v5+ (different frame schemas, not just additive features) would
/// require a real protocol-bump PR with code changes, not just a number bump
/// here. 99 is human-readable; the actual upper bound is u32::MAX.
pub(crate) const OC_PROTOCOL_MAX: u32 = 99;

/// Connection state
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    Connected,
    Connecting,
    Disconnected,
    AuthFailed,
}

impl ConnectionState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Connected => "connected",
            Self::Connecting => "connecting",
            Self::Disconnected => "disconnected",
            Self::AuthFailed => "auth_failed",
        }
    }
}

/// OC request frame
#[derive(Debug, Serialize, Deserialize)]
struct OcRequest {
    #[serde(rename = "type")]
    frame_type: String,  // "req"
    id: String,
    method: String,
    params: Value,
}

/// OC response frame
#[derive(Debug, Serialize, Deserialize)]
struct OcResponse {
    #[serde(rename = "type")]
    frame_type: String,  // "res"
    id: String,
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

/// OC hello-ok response
#[derive(Debug, Serialize, Deserialize)]
struct OcHelloOk {
    #[serde(rename = "type")]
    frame_type: String,  // "hello-ok"
    protocol: u32,
    server: Value,
    #[serde(default)]
    features: Value,
    #[serde(default)]
    snapshot: Value,
    #[serde(default)]
    policy: Value,
}

/// OC event frame
#[derive(Debug, Serialize, Deserialize)]
struct OcEvent {
    #[serde(rename = "type")]
    frame_type: String,  // "event"
    event: String,
    payload: Value,
}

/// Whitelisted methods
const ALLOWED_METHODS: &[&str] = &[
    "models.list",
    "agents.list",
    "sessions.create",
    "sessions.list",
    "sessions.get",
    "sessions.delete",
    "sessions.send",
    "sessions.messages.subscribe",
    "sessions.abort",
    "chat.history",
];

/// OC WebSocket client
pub struct OcClient {
    url: String,
    bearer_token: String,
    device_identity: Arc<DeviceIdentity>,
    state: Arc<RwLock<ConnectionState>>,
    request_id: Arc<AtomicU64>,
    pending_requests: Arc<RwLock<HashMap<String, tokio::sync::oneshot::Sender<Result<Value, GatewayError>>>>>,
    /// Separate channel for hello-ok response during initial handshake
    hello_ok_tx: Arc<RwLock<Option<tokio::sync::oneshot::Sender<Result<(), GatewayError>>>>>,
    /// Connection ID from hello-ok
    conn_id: Arc<RwLock<Option<String>>>,
    write_tx: Arc<RwLock<Option<mpsc::UnboundedSender<String>>>>,
    /// Active subscriptions: method -> Vec<sender>
    /// Multiple clients can subscribe to the same method
    subscriptions: Arc<RwLock<HashMap<String, Vec<mpsc::UnboundedSender<Value>>>>>,
    // Previously mpsc::Sender (bounded 100); changed to UnboundedSender to prevent
    // the WS read loop from stalling on a slow consumer (#1998).
    /// Whether we've completed the connect challenge auth handshake
    authenticated: Arc<RwLock<bool>>,
    /// Unix timestamp (ms) of the last received WS message (Text, Ping, or Pong).
    /// Shared with the chat proxy so it can extend the HTTP SSE idle deadline when
    /// the WS channel is active during OC tool execution (§22C.4.1.1).
    /// 0 = no message received yet.
    last_ws_activity_ms: Arc<AtomicU64>,
}

impl OcClient {
    pub fn new(url: String, bearer_token: String) -> Result<Self, GatewayError> {
        eprintln!("[OC_CLIENT] Creating OC client for URL: {}", url);
        
        // Load or generate device identity
        let device_identity = DeviceIdentity::load_or_create()?;
        eprintln!("[OC_CLIENT] Device identity loaded: {}", device_identity.device_id);
        
        Ok(Self {
            url,
            bearer_token,
            device_identity: Arc::new(device_identity),
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            request_id: Arc::new(AtomicU64::new(1)),
            pending_requests: Arc::new(RwLock::new(HashMap::new())),
            write_tx: Arc::new(RwLock::new(None)),
            subscriptions: Arc::new(RwLock::new(HashMap::new())),
            authenticated: Arc::new(RwLock::new(false)),
            hello_ok_tx: Arc::new(RwLock::new(None)),
            conn_id: Arc::new(RwLock::new(None)),
            last_ws_activity_ms: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Get current connection state
    pub async fn get_state(&self) -> ConnectionState {
        self.state.read().await.clone()
    }

    /// Unix timestamp (ms) of the last received WS message (Text, Ping, or Pong).
    /// Returns 0 if no message has been received yet.
    /// Used by the chat proxy (§22C.4.1.1) to extend the HTTP SSE idle deadline
    /// when the WS channel is active during OC tool execution.
    pub fn last_ws_activity_ms(&self) -> u64 {
        self.last_ws_activity_ms.load(Ordering::Relaxed)
    }

    /// Call a JSON-RPC method over WebSocket
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, GatewayError> {
        eprintln!("[OC_CLIENT] RPC call: method={}", method);

        // Enforce method whitelist
        if !ALLOWED_METHODS.contains(&method) {
            eprintln!("[OC_CLIENT] Method not whitelisted: {}", method);
            return Err(GatewayError::OcClient(format!("Method not allowed: {}", method)));
        }

        // Generate request ID (string format for OC)
        let id = self.request_id.fetch_add(1, Ordering::SeqCst).to_string();

        let request = OcRequest {
            frame_type: "req".to_string(),
            id: id.clone(),
            method: method.to_string(),
            params,
        };

        // Create response channel
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending_requests.write().await.insert(id.clone(), tx);

        // Connect if needed and send request
        self.ensure_connected_and_send(request).await?;

        // Wait for response with timeout
        match timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                eprintln!("[OC_CLIENT] Response channel closed");
                Err(GatewayError::OcConnectionLost)
            }
            Err(_) => {
                eprintln!("[OC_CLIENT] Request timeout");
                self.pending_requests.write().await.remove(&id);
                Err(GatewayError::OcTimeout)
            }
        }
    }

    /// Subscribe to events - full implementation
    /// Sends a subscribe request and returns a channel for receiving notifications
    pub async fn subscribe(&self, method: &str, params: Value) -> Result<mpsc::UnboundedReceiver<Value>, GatewayError> {
        eprintln!("[OC_CLIENT] Subscribe: method={}", method);

        // Enforce method whitelist
        if !ALLOWED_METHODS.contains(&method) {
            return Err(GatewayError::OcClient(format!("Method not allowed: {}", method)));
        }

        // Create channel for this subscription
        // Unbounded: the WS read loop must never block on a slow SSE consumer.
        // A slow consumer accumulates ~3 MB at peak for a 30-second stall; safe on
        // all target hardware. Dead consumers return Err immediately, no accumulation.
        // See: docs/audits/2026-07-15-issue1998-unbounded-confirm.md
        let (tx, rx) = mpsc::unbounded_channel();

        // Add to subscriptions map
        {
            let mut subs = self.subscriptions.write().await;
            subs.entry(method.to_string())
                .or_insert_with(Vec::new)
                .push(tx);
        }

        // Send subscribe request over WebSocket
        let request_id = self.request_id.fetch_add(1, Ordering::SeqCst).to_string();
        let request = OcRequest {
            frame_type: "req".to_string(),
            id: request_id,
            method: method.to_string(),
            params,
        };

        self.ensure_connected_and_send(request).await?;

        eprintln!("[OC_CLIENT] Subscription active: method={}", method);
        Ok(rx)
    }

    async fn ensure_connected_and_send(&self, request: OcRequest) -> Result<(), GatewayError> {
        let current_state = self.get_state().await;
        
        if current_state != ConnectionState::Connected {
            eprintln!("[OC_CLIENT] Not connected, attempting to connect...");
            self.connect().await?;
        }

        // Wait for authentication (unless this IS the connect request)
        if request.method != "connect" {
            let mut retries = 0;
            while !*self.authenticated.read().await {
                if retries > 50 {  // 5 seconds max wait
                    return Err(GatewayError::OcClient("Authentication timeout".to_string()));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
                retries += 1;
            }
        }

        // Send request
        let message = serde_json::to_string(&request)
            .map_err(|e| GatewayError::OcClient(format!("Failed to serialize request: {}", e)))?;
        
        // #3218 item 10: the `connect` request embeds the OC bearer token and
        // device signature in full (see the write-task log site below) —
        // never log the raw message, only the RPC method name.
        eprintln!("[OC_CLIENT] Sending request: {}", request.method);
        
        // Send through channel to write task
        let write_tx = self.write_tx.read().await;
        if let Some(tx) = write_tx.as_ref() {
            tx.send(message)
                .map_err(|e| GatewayError::OcClient(format!("Failed to send to write channel: {}", e)))?;
        } else {
            return Err(GatewayError::OcClient("Write channel not initialized".to_string()));
        }
        
        Ok(())
    }

    async fn connect(&self) -> Result<(), GatewayError> {
        eprintln!("[OC_CLIENT] Connecting to {}", self.url);
        
        *self.state.write().await = ConnectionState::Connecting;
        *self.authenticated.write().await = false;
        
        // Set up hello-ok receiver
        let (hello_tx, hello_rx) = tokio::sync::oneshot::channel();
        *self.hello_ok_tx.write().await = Some(hello_tx);

        // Parse URL and build request with auth header
        let url = self.url.parse::<http::Uri>()
            .map_err(|e| GatewayError::OcClient(format!("Invalid URL: {}", e)))?;
        
        let authority = url.authority()
            .ok_or_else(|| GatewayError::OcClient("URL missing authority".to_string()))?;
        
        let host = authority.host();
        
        let request = http::Request::builder()
            .uri(&self.url)
            .header("Host", host)
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
            .header("Authorization", format!("Bearer {}", self.bearer_token))
            .body(())
            .map_err(|e| GatewayError::OcClient(format!("Failed to build request: {}", e)))?;

        // Attempt connection
        match connect_async(request).await {
            Ok((ws_stream, _)) => {
                eprintln!("[OC_CLIENT] WebSocket connected successfully");
                *self.state.write().await = ConnectionState::Connected;
                
                // Spawn connection handler
                self.spawn_connection_handler(ws_stream).await;
                
                // Wait for hello-ok with timeout
                eprintln!("[OC_CLIENT] Waiting for hello-ok...");
                match timeout(Duration::from_secs(10), hello_rx).await {
                    Ok(Ok(Ok(()))) => {
                        eprintln!("[OC_CLIENT] Connection fully authenticated");
                        Ok(())
                    }
                    Ok(Ok(Err(e))) => {
                        eprintln!("[OC_CLIENT] Authentication failed: {:?}", e);
                        *self.state.write().await = ConnectionState::AuthFailed;
                        Err(e)
                    }
                    Ok(Err(_)) => {
                        eprintln!("[OC_CLIENT] Hello-ok channel closed");
                        *self.state.write().await = ConnectionState::AuthFailed;
                        Err(GatewayError::OcClient("Authentication channel closed".to_string()))
                    }
                    Err(_) => {
                        eprintln!("[OC_CLIENT] Hello-ok timeout");
                        *self.state.write().await = ConnectionState::AuthFailed;
                        // §22C.5.4 / ADR-0055: hello-ok timeout gets its own variant so Pane
                        // can surface a token-rotation recovery message. This is the ONLY
                        // arm that changes — the 5 s auth-retry-loop OcClient("Authentication
                        // timeout") at ensure_connected_and_send remains untouched.
                        Err(GatewayError::OcHandshakeTimeout)
                    }
                }
            }
            Err(e) => {
                eprintln!("[OC_CLIENT] Connection failed: {}", e);
                *self.state.write().await = ConnectionState::Disconnected;
                Err(GatewayError::OcConnectionLost)
            }
        }
    }

    async fn spawn_connection_handler(&self, ws_stream: WebSocketStream<MaybeTlsStream<TcpStream>>) {
        let state = self.state.clone();
        let pending_requests = self.pending_requests.clone();
        let subscriptions = self.subscriptions.clone();
        let authenticated = self.authenticated.clone();
        let bearer_token = self.bearer_token.clone();
        let device_identity = self.device_identity.clone();
        let request_id_counter = self.request_id.clone();
        let hello_ok_tx = self.hello_ok_tx.clone();
        let conn_id = self.conn_id.clone();
        let last_ws_activity_ms = self.last_ws_activity_ms.clone();
        
        // Reset authentication state
        *authenticated.write().await = false;
        
        // Create write channel
        let (write_tx, mut write_rx) = mpsc::unbounded_channel::<String>();
        *self.write_tx.write().await = Some(write_tx.clone());
        
        tokio::spawn(async move {
            let (mut write, mut read) = ws_stream.split();
            
            let state_for_write = state.clone();
            
            // Write task - handles outgoing messages and heartbeats
            let write_task = tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(30));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                
                loop {
                    tokio::select! {
                        // Handle outgoing messages
                        Some(message) = write_rx.recv() => {
                            // #3106 M2: the `connect` request embeds the OC
                            // bearer token and device signature in full — never
                            // log the raw message. Log that a write happened.
                            eprintln!("[OC_CLIENT] Write task sending message");
                            if let Err(e) = write.send(Message::Text(message)).await {
                                eprintln!("[OC_CLIENT] Write failed: {}", e);
                                *state_for_write.write().await = ConnectionState::Disconnected;
                                break;
                            }
                        }
                        
                        // Periodic heartbeat
                        _ = interval.tick() => {
                            let current_state = state_for_write.read().await.clone();
                            if current_state != ConnectionState::Connected {
                                break;
                            }
                            
                            eprintln!("[OC_CLIENT] Sending ping");
                            if let Err(e) = write.send(Message::Ping(vec![])).await {
                                eprintln!("[OC_CLIENT] Ping failed: {}", e);
                                *state_for_write.write().await = ConnectionState::Disconnected;
                                break;
                            }
                        }
                    }
                }
                
                eprintln!("[OC_CLIENT] Write task exiting");
            });
            
            // Helper: record current unix-ms into last_ws_activity_ms.
            let record_ws_activity = |counter: &Arc<AtomicU64>| {
                let ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                counter.store(ms, Ordering::Relaxed);
            };

            // Read messages
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        // §22C.4.1.1: record WS activity so the chat proxy can extend its
                        // HTTP SSE idle deadline while OC is executing a tool.
                        record_ws_activity(&last_ws_activity_ms);
                        eprintln!("[OC_CLIENT] Received: {}", text);
                        
                        // Parse message type
                        if let Ok(value) = serde_json::from_str::<Value>(&text) {
                            let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
                            
                            match msg_type {
                                "event" => {
                                    // Handle event frames
                                    if let Ok(event) = serde_json::from_str::<OcEvent>(&text) {
                                        if event.event == "connect.challenge" {
                                            eprintln!("[OC_CLIENT] Received connect.challenge");
                                            
                                            // Extract nonce
                                            let nonce = event.payload.get("nonce")
                                                .and_then(|n| n.as_str())
                                                .unwrap_or("");
                                            
                                            // #3106 M2: the nonce is a one-time
                                            // authentication value fed into the
                                            // device-identity signature below —
                                            // never log it in full.
                                            eprintln!("[OC_CLIENT] Challenge nonce received");
                                            
                                            // Sign the v3 payload with device identity
                                            let scopes = vec![
                                                "operator.admin",
                                                "operator.read",
                                                "operator.write",
                                                "operator.approvals",
                                                "operator.pairing",
                                                "operator.talk.secrets",
                                            ];
                                            
                                            let device_auth = match device_identity.sign_v3_payload(nonce, &bearer_token, &scopes) {
                                                Ok(auth) => auth,
                                                Err(e) => {
                                                    eprintln!("[OC_CLIENT] Failed to sign device auth: {:?}", e);
                                                    if let Some(tx) = hello_ok_tx.write().await.take() {
                                                        let _ = tx.send(Err(e));
                                                    }
                                                    continue;
                                                }
                                            };
                                            
                                            eprintln!("[OC_CLIENT] Device identity signed, device_id: {}", device_auth.device_id);
                                            
                                            // Build connect request with device identity
                                            let id = request_id_counter.fetch_add(1, Ordering::SeqCst).to_string();
                                            let connect_params = serde_json::json!({
                                                "minProtocol": OC_PROTOCOL_MIN,
                                                "maxProtocol": OC_PROTOCOL_MAX,
                                                "client": {
                                                    "id": "gateway-client",
                                                    "version": "0.1.0", // protocol version, intentionally distinct from gateway binary version — do not wire to CARGO_PKG_VERSION
                                                    "platform": "macos",
                                                    "mode": "backend"
                                                },
                                                "auth": {
                                                    "token": bearer_token
                                                },
                                                "device": {
                                                    "id": device_auth.device_id,
                                                    "publicKey": device_auth.public_key,
                                                    "signature": device_auth.signature,
                                                    "signedAt": device_auth.signed_at,
                                                    "nonce": device_auth.nonce
                                                },
                                                "scopes": scopes,
                                                "caps": [],
                                                "role": "operator"
                                            });
                                            
                                            let connect_request = OcRequest {
                                                frame_type: "req".to_string(),
                                                id: id.clone(),
                                                method: "connect".to_string(),
                                                params: connect_params,
                                            };
                                            
                                            // Send connect request
                                            let connect_msg = serde_json::to_string(&connect_request)
                                                .expect("Failed to serialize connect request");
                                            
                                            eprintln!("[OC_CLIENT] Sending connect request with device identity");
                                            if let Err(e) = write_tx.send(connect_msg) {
                                                eprintln!("[OC_CLIENT] Failed to send connect request: {}", e);
                                                if let Some(tx) = hello_ok_tx.write().await.take() {
                                                    let _ = tx.send(Err(GatewayError::OcClient(format!("Send failed: {}", e))));
                                                }
                                            }
                                        } else {
                                            // Other events - broadcast to subscriptions
                                            eprintln!("[OC_CLIENT] Received event: {}", event.event);
                                            let subs = subscriptions.read().await;
                                            for (_method, senders) in subs.iter() {
                                                for sender in senders {
                                                    // Sync send — no .await; unbounded channel never blocks.
                                                    let _ = sender.send(value.clone());
                                                }
                                            }
                                        }
                                    }
                                }
                                "hello-ok" => {
                                    // Handle hello-ok response
                                    if let Ok(hello) = serde_json::from_str::<OcHelloOk>(&text) {
                                        eprintln!("[OC_CLIENT] Received hello-ok, protocol: {}", hello.protocol);
                                        
                                        // Extract connection ID
                                        if let Some(conn_id_str) = hello.server.get("connId").and_then(|v| v.as_str()) {
                                            eprintln!("[OC_CLIENT] Connection ID: {}", conn_id_str);
                                            *conn_id.write().await = Some(conn_id_str.to_string());
                                        }
                                        
                                        // Mark as authenticated
                                        eprintln!("[OC_CLIENT] Authentication successful");
                                        *authenticated.write().await = true;
                                        
                                        // Signal hello-ok received
                                        if let Some(tx) = hello_ok_tx.write().await.take() {
                                            let _ = tx.send(Ok(()));
                                        }
                                    }
                                }
                                "res" => {
                                    // Handle response frames
                                    if let Ok(response) = serde_json::from_str::<OcResponse>(&text) {
                                        // Check if this is a hello-ok response (payload.type == "hello-ok")
                                        let is_hello_ok = response.payload.as_ref()
                                            .and_then(|p| p.get("type"))
                                            .and_then(|t| t.as_str())
                                            .map(|t| t == "hello-ok")
                                            .unwrap_or(false);
                                        
                                        if is_hello_ok {
                                            // Handle hello-ok response
                                            if let Some(payload) = &response.payload {
                                                if let Some(protocol) = payload.get("protocol").and_then(|p| p.as_u64()) {
                                                    eprintln!("[OC_CLIENT] Received hello-ok, protocol: {}", protocol);
                                                }
                                                
                                                // Extract connection ID
                                                if let Some(server) = payload.get("server") {
                                                    if let Some(conn_id_str) = server.get("connId").and_then(|v| v.as_str()) {
                                                        eprintln!("[OC_CLIENT] Connection ID: {}", conn_id_str);
                                                        *conn_id.write().await = Some(conn_id_str.to_string());
                                                    }
                                                }
                                                
                                                // Mark as authenticated
                                                eprintln!("[OC_CLIENT] Authentication successful");
                                                *authenticated.write().await = true;
                                                
                                                // Signal hello-ok received
                                                if let Some(tx) = hello_ok_tx.write().await.take() {
                                                    let _ = tx.send(Ok(()));
                                                }
                                            }
                                        }
                                        
                                        // Send response to pending request
                                        let mut pending = pending_requests.write().await;
                                        if let Some(tx) = pending.remove(&response.id) {
                                            if response.ok {
                                                let payload = response.payload.unwrap_or(Value::Null);
                                                let _ = tx.send(Ok(payload));
                                            } else if let Some(error) = response.error {
                                                eprintln!("[OC_CLIENT] RPC error: {} - {}", error.code, error.message);
                                                // #3050 fix PR, Bug 1(b) secondary/gateway commit:
                                                // preserve OC's own `error.code` (e.g.
                                                // "INVALID_REQUEST") instead of discarding it —
                                                // `GatewayError::OcClient(String)` only ever kept
                                                // `error.message`, which is why a clean OC rejection
                                                // ("session not found") surfaced as a generic 502
                                                // instead of an honest 4xx (see error.rs's
                                                // `OcRpcError` IntoResponse arm for the mapping).
                                                let _ = tx.send(Err(GatewayError::OcRpcError {
                                                    code: error.code,
                                                    message: error.message,
                                                }));
                                            } else {
                                                let _ = tx.send(Err(GatewayError::OcClient("Response not ok".to_string())));
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    eprintln!("[OC_CLIENT] Unknown message type: {}", msg_type);
                                }
                            }
                        }
                    }
                    Ok(Message::Pong(_)) => {
                        // §22C.4.1.1: pong is a WS-channel liveness signal.
                        record_ws_activity(&last_ws_activity_ms);
                        eprintln!("[OC_CLIENT] Received pong");
                    }
                    Ok(Message::Close(_)) => {
                        eprintln!("[OC_CLIENT] Connection closed by server");
                        *state.write().await = ConnectionState::Disconnected;
                        
                        // Fail all pending requests with connection lost error
                        let mut pending = pending_requests.write().await;
                        for (_, tx) in pending.drain() {
                            let _ = tx.send(Err(GatewayError::OcConnectionLost));
                        }
                        break;
                    }
                    Err(e) => {
                        eprintln!("[OC_CLIENT] WebSocket error: {}", e);
                        *state.write().await = ConnectionState::Disconnected;
                        
                        // Fail all pending requests with connection lost error
                        let mut pending = pending_requests.write().await;
                        for (_, tx) in pending.drain() {
                            let _ = tx.send(Err(GatewayError::OcConnectionLost));
                        }
                        break;
                    }
                    Ok(Message::Ping(_)) => {
                        // §22C.4.1.1: Ping from server is a WS-channel liveness signal.
                        // tungstenite auto-replies with Pong; we just record activity.
                        record_ws_activity(&last_ws_activity_ms);
                        eprintln!("[OC_CLIENT] Received ping from server");
                    }
                    _ => {}
                }
            }
            
            eprintln!("[OC_CLIENT] Connection handler exiting");
            write_task.abort();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_client_creation() {
        let client = OcClient::new(
            "ws://localhost:8080".to_string(),
            "test_token".to_string(),
        ).expect("Failed to create client");
        
        let state = client.get_state().await;
        assert_eq!(state, ConnectionState::Disconnected);
    }

    #[tokio::test]
    async fn test_method_whitelist_allowed() {
        let client = OcClient::new(
            "ws://localhost:8080".to_string(),
            "test_token".to_string(),
        ).expect("Failed to create client");
        
        // This will fail to connect, but should pass whitelist check
        let result = client.call("models.list", Value::Null).await;
        // Error will be connection-related, not whitelist
        if let Err(e) = result {
            assert!(!e.to_string().contains("not allowed"));
        }
    }

    #[tokio::test]
    async fn test_method_whitelist_blocked() {
        let client = OcClient::new(
            "ws://localhost:8080".to_string(),
            "test_token".to_string(),
        ).expect("Failed to create client");
        
        let result = client.call("forbidden.method", Value::Null).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not allowed"));
    }

    #[test]
    fn test_connection_state_string() {
        assert_eq!(ConnectionState::Connected.as_str(), "connected");
        assert_eq!(ConnectionState::Connecting.as_str(), "connecting");
        assert_eq!(ConnectionState::Disconnected.as_str(), "disconnected");
        assert_eq!(ConnectionState::AuthFailed.as_str(), "auth_failed");
    }

    /// Integration test: verifies full round-trip with mock WebSocket server
    #[tokio::test]
    async fn test_round_trip_communication() {
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
        
        // Spawn mock WebSocket server
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let port = addr.port();
        
        eprintln!("[TEST] Mock server listening on port {}", port);
        
        // Server task
        tokio::spawn(async move {
            if let Ok((stream, _)) = listener.accept().await {
                eprintln!("[TEST] Server accepted connection");
                
                // Accept WebSocket with custom callback to validate auth header
                let callback = |req: &Request, response: Response| {
                    eprintln!("[TEST] Server validating request headers");
                    
                    // Check for Authorization header
                    if let Some(auth_header) = req.headers().get("Authorization") {
                        eprintln!("[TEST] Auth header: {:?}", auth_header);
                    }
                    
                    Ok(response)
                };
                
                if let Ok(ws_stream) = tokio_tungstenite::accept_hdr_async(stream, callback).await {
                    eprintln!("[TEST] Server upgraded to WebSocket");
                    let (mut write, mut read) = ws_stream.split();
                    
                    // Send connect.challenge event immediately
                    let challenge_event = serde_json::json!({
                        "type": "event",
                        "event": "connect.challenge",
                        "payload": {
                            "nonce": "test-nonce-12345"
                        }
                    });
                    eprintln!("[TEST] Server sending connect.challenge");
                    let _ = write.send(Message::Text(serde_json::to_string(&challenge_event).unwrap())).await;
                    
                    while let Some(Ok(msg)) = read.next().await {
                        if let Message::Text(text) = msg {
                            eprintln!("[TEST] Server received: {}", text);
                            
                            // Parse incoming request and send mock response
                            if let Ok(request) = serde_json::from_str::<OcRequest>(&text) {
                                if request.method == "connect" {
                                    // Send hello-ok for connect request
                                    let hello_ok = OcResponse {
                                        frame_type: "res".to_string(),
                                        id: request.id,
                                        ok: true,
                                        payload: Some(serde_json::json!({
                                            "type": "hello-ok",
                                            "protocol": OC_PROTOCOL_MIN, // test mock: OC server selected the lowest protocol the client supports
                                            "server": {
                                                "connId": "test-conn-123",
                                                "version": "1.0.0"
                                            },
                                            "features": {},
                                            "snapshot": {},
                                            "policy": {}
                                        })),
                                        error: None,
                                    };
                                    eprintln!("[TEST] Server sending hello-ok");
                                    let _ = write.send(Message::Text(serde_json::to_string(&hello_ok).unwrap())).await;
                                } else {
                                    // Regular RPC response
                                    let response = OcResponse {
                                        frame_type: "res".to_string(),
                                        id: request.id,
                                        ok: true,
                                        payload: Some(serde_json::json!({
                                            "models": [
                                                {"id": "test-model", "name": "Test Model"}
                                            ]
                                        })),
                                        error: None,
                                    };
                                    
                                    let response_text = serde_json::to_string(&response).unwrap();
                                    eprintln!("[TEST] Server sending: {}", response_text);
                                    let _ = write.send(Message::Text(response_text)).await;
                                }
                            }
                        } else if let Message::Ping(_) = msg {
                            eprintln!("[TEST] Server received ping, sending pong");
                            let _ = write.send(Message::Pong(vec![])).await;
                        }
                    }
                }
            }
        });
        
        // Give server time to start
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Create client and call method
        let client = OcClient::new(
            format!("ws://127.0.0.1:{}", port),
            "test_token".to_string(),
        ).expect("Failed to create client");
        
        eprintln!("[TEST] Client calling models.list");
        let result = client.call("models.list", Value::Null).await;
        
        eprintln!("[TEST] Result: {:?}", result);
        assert!(result.is_ok(), "Expected successful round-trip, got: {:?}", result);
        
        let data = result.unwrap();
        assert!(data.get("models").is_some());
        assert!(data["models"].as_array().unwrap().len() > 0);
    }
}
