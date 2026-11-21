use crate::agents::AgentRegistry;
use crate::allowlist::Allowlist;
use crate::auth::{auth_middleware, AuthState};
use crate::error::GatewayError;
use crate::idempotency::IdempotencyStore;
use crate::metrics::Metrics;
use crate::oc_client::OcClient;
use crate::param_compat::CompatTable;
use crate::ring_buffer::RingBuffer;
use crate::routes;
use crate::sync_queue::SyncQueue;
use axum::{
    extract::DefaultBodyLimit,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use serde::Serialize;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::ServiceExt;
use tower_http::trace::TraceLayer;

/// Shared application state
#[derive(Clone)]
pub struct SharedState {
    pub start_time: OffsetDateTime,
    pub tls_fingerprint: String,
    pub tls_cert_pem: String,
    pub auth_state: Arc<AuthState>,
    pub oc_client: Arc<OcClient>,
    pub agent_registry: Arc<AgentRegistry>,
    pub models_cache: Arc<routes::models::ModelsCache>,
    pub sync_queue: Arc<SyncQueue>,
    pub ring_buffer: Arc<RingBuffer>,
    pub idempotency_store: Arc<IdempotencyStore>,
    pub metrics: Arc<Metrics>,
    /// Map gateway session IDs to OC session IDs
    pub session_mappings: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    /// HTTP base URL for OC (derived from ws:// URL)
    pub oc_http_base_url: String,
    /// OC bearer token for upstream requests
    pub oc_bearer_token: String,
    /// Tailscale hostname if using a Tailscale cert, None otherwise
    pub tls_hostname: Option<String>,
    /// Parameter compatibility table — strips deprecated params per downstream model (§4A.2.2)
    pub param_compat_table: Arc<CompatTable>,
    /// Idle timeout for streaming: drop connection if no chunk arrives for this many seconds.
    /// Set from GATEWAY_STREAMING_IDLE_TIMEOUT_SECS via Config::load() (default: 60).
    pub streaming_idle_timeout_secs: u64,
}



#[derive(Debug, Serialize)]
struct NotFoundResponse {
    error: String,
    message: String,
}

/// Start the HTTP/2 gateway server
pub async fn start_server(
    port: u16,
    tls_config: Arc<rustls::ServerConfig>,
    tls_fingerprint: String,
    tls_cert_pem: String,
    tls_hostname: Option<String>,
    auth_state: Arc<AuthState>,
    oc_client: Arc<OcClient>,
    agent_registry: Arc<AgentRegistry>,
    metrics: Arc<Metrics>,
    oc_http_base_url: String,
    oc_bearer_token: String,
    data_dir: PathBuf,
    param_compat_table: Arc<CompatTable>,
    streaming_idle_timeout_secs: u64,
    allowlist: Arc<Allowlist>,
) -> Result<(), GatewayError> {
    eprintln!("[SERVER] start_server called on port {}", port);
    tracing::info!(port = port, "Starting gateway server");

    // Spawn Unix socket IPC listener
    {
        let socket_path = data_dir.join("pane-gateway.sock");
        let auth_state_ipc = auth_state.clone();

        // Ensure data_dir exists
        std::fs::create_dir_all(&data_dir)
            .map_err(GatewayError::Io)?;

        // Stale socket cleanup
        if socket_path.exists() {
            use tokio::net::UnixStream;
            match UnixStream::connect(&socket_path).await {
                Ok(_) => {
                    return Err(GatewayError::Config(format!(
                        "Error: Gateway already running at {}",
                        socket_path.display()
                    )));
                }
                Err(_) => {
                    eprintln!("[IPC] Removing stale socket at {}", socket_path.display());
                    std::fs::remove_file(&socket_path).map_err(GatewayError::Io)?;
                }
            }
        }

        spawn_ipc_listener(socket_path, auth_state_ipc);
    }

    // Create shared state
    let shared_state = Arc::new(SharedState {
        start_time: OffsetDateTime::now_utc(),
        tls_fingerprint,
        tls_cert_pem,
        tls_hostname,
        auth_state: auth_state.clone(),
        oc_client,
        agent_registry,
        models_cache: Arc::new(routes::models::ModelsCache::new()),
        sync_queue: Arc::new(SyncQueue::new()),
        ring_buffer: Arc::new(RingBuffer::new()),
        idempotency_store: IdempotencyStore::new(),
        metrics,
        streaming_idle_timeout_secs,
        session_mappings: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        oc_http_base_url,
        oc_bearer_token,
        param_compat_table,
    });

    // Protected routes (require auth)
    let protected = Router::new()
        .route("/v1/agents", get(routes::get_agents))
        .route("/v1/models", get(routes::models::get_models))
        .route("/v1/metrics", get(routes::metrics_endpoint))
        // Chat completions proxy (with payload limit)
        .route("/v1/chat/completions", post(routes::chat::chat_completions).layer(DefaultBodyLimit::max(8 * 1024 * 1024)))
        // Sync endpoints (with payload limits)
        .route("/v1/sync/push", post(routes::sync::push).layer(DefaultBodyLimit::max(10 * 1024 * 1024)))
        .route("/v1/sync/pull", get(routes::sync::pull))
        .route("/v1/sync/pull/confirm", delete(routes::sync::confirm))
        .route("/v1/sync/initial", get(routes::sync::initial))
        // Session endpoints (with payload limits on messages)
        .route("/v1/sessions", post(routes::sessions::create).get(routes::sessions::list))
        .route("/v1/sessions/:session_id", delete(routes::sessions::delete_session))
        .route("/v1/sessions/:session_id/messages", get(routes::sessions::messages).post(routes::sessions::send_message).layer(DefaultBodyLimit::max(1024 * 1024)))
        .route("/v1/sessions/:session_id/messages/stream", get(routes::stream::sse_stream))
        .route("/v1/sessions/:session_id/abort", post(routes::sessions::abort))
        .route_layer(middleware::from_fn_with_state(
            shared_state.auth_state.clone(),
            auth_middleware,
        ));

    // Build main router with public routes + protected routes
    let app = Router::new()
        .route("/pair", post(routes::pair::pair))
        .route("/v1/health", get(routes::health::health_check))
        .merge(protected)
        .fallback(not_found_handler)
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(crate::middleware::request_id_middleware))
        .with_state(shared_state);

    // Bind to address
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr)
        .await
        .map_err(GatewayError::Io)?;

    eprintln!("[SERVER] Listening on https://{}", addr);
    eprintln!("[SERVER] HTTP/2 enabled with TLS");

    let tls_acceptor = TlsAcceptor::from(tls_config);

    // Accept connections
    loop {
        let (stream, remote_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                eprintln!("[SERVER] Failed to accept connection: {}", e);
                continue;
            }
        };

        // #3106 H1: the bind above stays 0.0.0.0 — SplitMachine, the only live
        // topology, requires the gateway reachable from a different machine.
        // The allowlist is the control: reject here, before TLS, any remote
        // address outside the configured ranges. The reject log carries the
        // remote address and nothing else — no request body, no header, no code.
        if !allowlist.is_allowed(remote_addr.ip()) {
            eprintln!("[SERVER] Rejected connection from {} (not in allowlist)", remote_addr);
            continue;
        }

        eprintln!("[SERVER] New connection from {}", remote_addr);

        let tls_acceptor = tls_acceptor.clone();
        let app = app.clone();

        tokio::spawn(async move {
            match tls_acceptor.accept(stream).await {
                Ok(tls_stream) => {
                    eprintln!("[SERVER] TLS handshake successful with {}", remote_addr);
                    
                    // Serve the connection
                    if let Err(e) = hyper_util::server::conn::auto::Builder::new(
                        hyper_util::rt::TokioExecutor::new(),
                    )
                    .http2()
                    .initial_stream_window_size(65536)
                    .initial_connection_window_size(1048576)
                    .keep_alive_interval(std::time::Duration::from_secs(60))
                    .keep_alive_timeout(std::time::Duration::from_secs(20))
                    .serve_connection(
                        hyper_util::rt::TokioIo::new(tls_stream),
                        hyper::service::service_fn(move |req| {
                            let app = app.clone();
                            async move {
                                Ok::<_, std::convert::Infallible>(app.oneshot(req).await.unwrap())
                            }
                        }),
                    )
                    .await
                    {
                        eprintln!("[SERVER] Error serving connection from {}: {}", remote_addr, e);
                    }
                }
                Err(e) => {
                    eprintln!("[SERVER] TLS handshake failed with {}: {}", remote_addr, e);
                }
            }
        });
    }
}

/// Spawn the Unix socket IPC listener thread.
/// Called by `start_server()` and by tests.
pub(crate) fn spawn_ipc_listener(socket_path: PathBuf, auth_state: Arc<AuthState>) {
    let tokio_handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener as StdUnixListener;

        let std_listener = match StdUnixListener::bind(&socket_path) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[IPC] Failed to bind std listener: {}", e);
                return;
            }
        };
        // Set socket permissions to 0600
        let perms = std::fs::Permissions::from_mode(0o600);
        if let Err(e) = std::fs::set_permissions(&socket_path, perms) {
            eprintln!("[IPC] Failed to set socket permissions: {}", e);
        }
        eprintln!("[IPC] Listener thread running at {}", socket_path.display());
        for stream in std_listener.incoming() {
            match stream {
                Ok(mut conn) => {
                    eprintln!("[IPC] New connection received");
                    let mut buf = String::new();
                    if let Err(e) = conn.read_to_string(&mut buf) {
                        eprintln!("[IPC] Failed to read request: {}", e);
                        continue;
                    }
                    eprintln!("[IPC] Request: {}", buf.trim());
                    let parsed: serde_json::Value = match serde_json::from_str(&buf) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!("[IPC] Invalid JSON: {}", e);
                            continue;
                        }
                    };
                    if parsed["command"].as_str() == Some("generate_pairing_code") {
                        let code = tokio_handle.block_on(auth_state.generate_pairing_code());
                        // #3106 M2: never log the code value itself.
                        eprintln!("[IPC] Generated pairing code");
                        let response = format!("{{\"code\":\"{}\"}}", code);
                        if let Err(e) = conn.write_all(response.as_bytes()) {
                            eprintln!("[IPC] Failed to write response: {}", e);
                        }
                    } else {
                        eprintln!("[IPC] Unknown command: {}", buf.trim());
                    }
                }
                Err(e) => {
                    eprintln!("[IPC] Accept error: {}", e);
                }
            }
        }
    });
}

/// 404 handler for unknown routes
async fn not_found_handler() -> impl IntoResponse {
    eprintln!("[SERVER] 404 - Unknown endpoint requested");
    
    let response = NotFoundResponse {
        error: "not_found".to_string(),
        message: "Unknown endpoint".to_string(),
    };

    (StatusCode::NOT_FOUND, Json(response))
}

/// Build rustls ServerConfig from certificate and key
pub fn build_tls_config(cert_pem: Vec<u8>, key_pem: Vec<u8>) -> Result<Arc<rustls::ServerConfig>, GatewayError> {
    eprintln!("[SERVER] Building TLS configuration...");
    
    use rustls_pemfile::{certs, private_key};
    use std::io::Cursor;

    // Parse certificate
    let mut cert_reader = Cursor::new(cert_pem);
    let certs: Vec<_> = certs(&mut cert_reader)
        .collect::<Result<_, _>>()
        .map_err(|e| GatewayError::Tls(format!("Failed to parse certificate: {}", e)))?;

    if certs.is_empty() {
        return Err(GatewayError::Tls("No certificates found in PEM".to_string()));
    }

    // Parse private key
    let mut key_reader = Cursor::new(key_pem);
    let key = private_key(&mut key_reader)
        .map_err(|e| GatewayError::Tls(format!("Failed to parse private key: {}", e)))?
        .ok_or_else(|| GatewayError::Tls("No private key found in PEM".to_string()))?;

    // Build TLS config
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| GatewayError::Tls(format!("Failed to build TLS config: {}", e)))?;

    eprintln!("[SERVER] TLS configuration built successfully");

    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::AuthState;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream as StdUnixStream;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn test_build_tls_config_requires_valid_cert() {
        // Invalid PEM should fail
        let result = build_tls_config(b"invalid".to_vec(), b"invalid".to_vec());
        assert!(result.is_err());
    }

    /// Helper: spawn an IPC listener in a temp dir using the production function, return socket_path
    fn spawn_ipc_listener_for_test(dir: &PathBuf, auth_state: Arc<AuthState>) -> PathBuf {
        let socket_path = dir.join("pane-gateway.sock");
        super::spawn_ipc_listener(socket_path.clone(), auth_state);
        socket_path
    }

    /// Helper: do a synchronous IPC request and return the code
    fn ipc_get_code(socket_path: &PathBuf) -> String {
        let mut conn = StdUnixStream::connect(socket_path).unwrap();
        conn.write_all(br#"{"command":"generate_pairing_code"}"#).unwrap();
        conn.shutdown(std::net::Shutdown::Write).unwrap();
        let mut response = String::new();
        conn.read_to_string(&mut response).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
        parsed["code"].as_str().unwrap().to_string()
    }

    /// 1. Socket round-trip: connect, send request, get 8-digit code (#3106 H1)
    #[tokio::test]
    async fn test_ipc_socket_round_trip() {
        let dir = tempdir().unwrap();
        let auth_state = Arc::new(AuthState::new(dir.path().join("tokens.json")));
        let socket_path = spawn_ipc_listener_for_test(&dir.path().to_path_buf(), auth_state);

        // Give listener a moment to be ready
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let code = tokio::task::spawn_blocking(move || ipc_get_code(&socket_path))
            .await
            .unwrap();

        // #3106 H1 part 3: widened from 6 to 8 digits.
        assert_eq!(code.len(), 8, "Code should be 8 digits");
        assert!(code.chars().all(|c| c.is_ascii_digit()), "Code should be numeric");
    }

    /// 2. Gateway not running: connecting to nonexistent socket should fail
    #[tokio::test]
    async fn test_ipc_gateway_not_running() {
        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("pane-gateway.sock");
        let result = StdUnixStream::connect(&socket_path);
        assert!(result.is_err(), "Should fail when gateway not running");
    }

    /// 3. Stale socket cleanup: dead file replaced successfully
    #[test]
    fn test_ipc_stale_socket_cleanup() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener as StdUnixListener;

        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("pane-gateway.sock");

        // Create a dead (non-socket) file at the socket path
        std::fs::write(&socket_path, b"stale").unwrap();
        assert!(socket_path.exists());

        // Connecting to a non-socket file should fail
        let connect_result = StdUnixStream::connect(&socket_path);
        assert!(connect_result.is_err(), "Stale file should not be connectable");

        // Remove stale and rebind
        std::fs::remove_file(&socket_path).unwrap();
        let listener = StdUnixListener::bind(&socket_path).unwrap();
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&socket_path, perms).unwrap();

        // Verify it's now connectable
        let connect_result = StdUnixStream::connect(&socket_path);
        assert!(connect_result.is_ok(), "Fresh socket should be connectable");
        drop(listener);
    }

    /// 4. Live gateway detection: connect to live socket succeeds
    #[test]
    fn test_ipc_live_gateway_detection() {
        use std::os::unix::net::UnixListener as StdUnixListener;

        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("pane-gateway.sock");

        // Bind a real listener (simulates running gateway)
        let _listener = StdUnixListener::bind(&socket_path).unwrap();

        // Stale-socket-check logic: if connect succeeds → already running
        let connect_result = StdUnixStream::connect(&socket_path);
        assert!(
            connect_result.is_ok(),
            "Should be able to connect to live gateway socket"
        );
        // In production: connect OK → return GatewayError::Config("already running")
    }

    /// 5. Code accepted by exchange_code_for_token after IPC generation
    #[tokio::test]
    async fn test_ipc_code_accepted_by_exchange() {
        let dir = tempdir().unwrap();
        let auth_state = Arc::new(AuthState::new(dir.path().join("tokens.json")));
        let socket_path = spawn_ipc_listener_for_test(&dir.path().to_path_buf(), auth_state.clone());

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let code = tokio::task::spawn_blocking(move || ipc_get_code(&socket_path))
            .await
            .unwrap();

        // Exchange code for token
        let result = auth_state.exchange_code_for_token(&code).await;
        assert!(result.is_ok(), "Code from IPC should be exchangeable for token");
    }

    /// 6. Two successive requests: second overwrites first (single-slot semantics)
    #[tokio::test]
    async fn test_ipc_two_successive_requests() {
        let dir = tempdir().unwrap();
        let auth_state = Arc::new(AuthState::new(dir.path().join("tokens.json")));
        let socket_path = spawn_ipc_listener_for_test(&dir.path().to_path_buf(), auth_state);

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let sp1 = socket_path.clone();
        let code_a = tokio::task::spawn_blocking(move || ipc_get_code(&sp1)).await.unwrap();
        let sp2 = socket_path.clone();
        let code_b = tokio::task::spawn_blocking(move || ipc_get_code(&sp2)).await.unwrap();

        // #3106 H1 part 3: widened from 6 to 8 digits.
        assert_eq!(code_a.len(), 8);
        assert_eq!(code_b.len(), 8);
        assert!(code_a.chars().all(|c| c.is_ascii_digit()));
        assert!(code_b.chars().all(|c| c.is_ascii_digit()));
        // The important thing: both requests succeeded (single-slot overwrite per §22C.5)
    }
}
