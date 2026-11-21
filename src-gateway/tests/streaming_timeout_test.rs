/// Fix #1358 — Streaming idle timeout tests
///
/// This file contains three tests:
///
/// 1. `test_streaming_idle_timeout_config` — config loads the env var correctly. REAL.
///
/// 2. `test_idle_timeout_triggers_via_real_chat_handler` — sends a streaming request
///    through the production `chat_completions` Axum handler backed by a mock upstream
///    that stalls after one chunk. Verifies the gateway drops the connection within the
///    configured idle window. REVERTING the per-chunk `TokioStreamExt::timeout` wrapper
///    in `chat.rs` to `.timeout(Duration::from_secs(120))` breaks this test because the
///    old total timeout wouldn't fire in time.
///
/// 3. `test_idle_timeout_does_not_trigger_on_slow_flowing_stream` — same real-handler
///    path, but the mock upstream sends 4 chunks spaced 200 ms apart with a 2 s idle
///    threshold. Verifies a slow-but-flowing stream is NOT dropped prematurely.
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    http::StatusCode,
    response::Response,
    routing::post,
    Router,
};
use bytes::Bytes;
use futures_util::StreamExt as FuturesStreamExt;
use pane_gateway::{
    agents::AgentRegistry,
    auth::{auth_middleware, AuthState},
    config::Config,
    idempotency::IdempotencyStore,
    metrics::Metrics,
    oc_client::OcClient,
    param_compat::CompatTable,
    ring_buffer::RingBuffer,
    routes,
    server::SharedState,
    sync_queue::SyncQueue,
};
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

// ── Config tests ─────────────────────────────────────────────────────────────

/// Asserts that:
/// 1. The default is 60s (NOT the old hard-coded 120s total timeout).
/// 2. The value is overridden by GATEWAY_STREAMING_IDLE_TIMEOUT_SECS.
///
/// Both sub-assertions run in a single test function so they execute
/// sequentially and cannot race against each other when `--test-threads>1`.
#[test]
fn test_streaming_idle_timeout_config() {
    let key = "GATEWAY_STREAMING_IDLE_TIMEOUT_SECS";

    // 1. Default when env var is absent.
    std::env::remove_var(key);
    let config = Config::load().expect("Config::load failed");
    assert_eq!(
        config.streaming_idle_timeout_secs, 60,
        "Default streaming idle timeout must be 60s, NOT the old hard-coded 120s total timeout"
    );

    // 2. Override via env var.
    std::env::set_var(key, "120");
    let config = Config::load().expect("Config::load failed");
    assert_eq!(
        config.streaming_idle_timeout_secs, 120,
        "streaming_idle_timeout_secs must reflect GATEWAY_STREAMING_IDLE_TIMEOUT_SECS=120"
    );

    // Cleanup so other tests see a clean environment.
    std::env::remove_var(key);
}

// ── Integration test helpers ──────────────────────────────────────────────────

/// Build a gateway test router wired to a mock HTTP upstream.
///
/// The returned tuple is `(router, base_url, auth_token, client)`.
/// `streaming_idle_timeout_secs` is set by the caller so individual tests
/// can pick a value that keeps them fast.
async fn build_stream_test_gateway(
    mock_upstream_base_url: String,
    streaming_idle_timeout_secs: u64,
) -> (String, String, reqwest::Client) {
    let tokens_path = std::env::temp_dir().join(format!(
        "test-tokens-stream-{}.json",
        uuid::Uuid::new_v4()
    ));
    let auth_state = Arc::new(AuthState::new(tokens_path));
    let pairing_code = auth_state.generate_pairing_code().await;
    let token_info = auth_state
        .exchange_code_for_token(&pairing_code)
        .await
        .unwrap();
    let auth_token = token_info.token;

    // OcClient requires a WS URL but the chat handler uses oc_http_base_url for streaming.
    // Use a dummy port that won't be contacted during these tests.
    let oc_client = Arc::new(
        OcClient::new("ws://127.0.0.1:1".to_string(), "test-token".to_string()).unwrap(),
    );

    let shared_state = Arc::new(SharedState {
        start_time: OffsetDateTime::now_utc(),
        tls_fingerprint: "test-fp".to_string(),
        tls_cert_pem: "test-cert".to_string(),
        tls_hostname: None,
        auth_state: auth_state.clone(),
        oc_client: oc_client.clone(),
        agent_registry: Arc::new(AgentRegistry::new(oc_client.clone())),
        models_cache: Arc::new(routes::models::ModelsCache::new()),
        sync_queue: Arc::new(SyncQueue::new()),
        ring_buffer: Arc::new(RingBuffer::new()),
        idempotency_store: IdempotencyStore::new(),
        metrics: Arc::new(Metrics::new()),
        session_mappings: Arc::new(RwLock::new(HashMap::new())),
        oc_http_base_url: mock_upstream_base_url,
        oc_bearer_token: "test-token".to_string(),
        param_compat_table: Arc::new(CompatTable::default()),
        streaming_idle_timeout_secs,
    });

    let protected = Router::new()
        .route(
            "/v1/chat/completions",
            post(routes::chat::chat_completions)
                .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            shared_state.auth_state.clone(),
            auth_middleware,
        ));

    let app = Router::new()
        .merge(protected)
        .with_state(shared_state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    // No total reqwest timeout on the client — we want to observe the
    // gateway's own idle-timeout mechanism, not the test client timing out.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    (base_url, auth_token, client)
}

/// Start a mock HTTP server at a random port that returns one SSE chunk
/// immediately and then stalls indefinitely (simulating a backend that stops
/// sending mid-stream).
///
/// Returns the `http://host:port` base URL of the mock.
async fn start_stalling_sse_upstream() -> String {
    async fn stalling_handler() -> Response<Body> {
        let first_chunk = Bytes::from(
            "data: {\"id\":\"1\",\"object\":\"chat.completion.chunk\",\
             \"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        );

        // One immediate chunk, then stall for 60 s (much longer than any test timeout).
        let stream = futures_util::stream::iter([Ok::<Bytes, std::io::Error>(first_chunk)])
            .chain(futures_util::stream::once(async {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n"))
            }));

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(Body::from_stream(stream))
            .unwrap()
    }

    let app = Router::new().route("/v1/chat/completions", post(stalling_handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(30)).await;
    format!("http://{}", addr)
}

/// Start a mock HTTP server that returns 4 SSE chunks spaced `chunk_delay_ms`
/// apart, then closes normally.
async fn start_slow_flowing_sse_upstream(chunk_delay_ms: u64, chunk_count: usize) -> String {
    let chunk_delay_ms = chunk_delay_ms;

    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || async move {
            let chunks: Vec<Result<Bytes, std::io::Error>> = (0..chunk_count)
                .map(|i| {
                    Ok(Bytes::from(format!(
                        "data: {{\"id\":\"{}\",\"object\":\"chat.completion.chunk\",\
                         \"choices\":[{{\"delta\":{{\"content\":\"chunk{}\"}}}}]}}\n\n",
                        i, i
                    )))
                })
                .collect();

            // Interleave sleep futures between chunks.
            let stream = futures_util::stream::unfold(
                (chunks.into_iter(), chunk_delay_ms),
                |(mut it, delay_ms)| async move {
                    let chunk = it.next()?;
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    Some((chunk, (it, delay_ms)))
                },
            )
            .chain(futures_util::stream::once(async {
                Ok::<Bytes, std::io::Error>(Bytes::from("data: [DONE]\n\n"))
            }));

            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(Body::from_stream(stream))
                .unwrap()
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(30)).await;
    format!("http://{}", addr)
}

// ── Stream idle-timeout behaviour tests (REAL — exercise routes/chat.rs) ─────

/// A streaming request through the real `chat_completions` handler, backed by
/// a mock upstream that stalls after one chunk, MUST terminate within the
/// configured idle window.
///
/// **Regression guard:** reverting `chat.rs` to use `.timeout(Duration::from_secs(120))`
/// (total wall-clock timeout) instead of the per-chunk `TokioStreamExt::timeout` wrapper
/// would cause this test to hang past the 5-second outer `tokio::time::timeout` and fail.
/// The hollow predecessor tests did NOT catch this revert; this integration test does.
#[tokio::test]
async fn test_idle_timeout_triggers_via_real_chat_handler() {
    let upstream_url = start_stalling_sse_upstream().await;
    // 1-second idle timeout so the test completes quickly.
    let (gateway_url, auth_token, client) =
        build_stream_test_gateway(upstream_url, 1).await;

    let start = std::time::Instant::now();

    let response = client
        .post(format!("{}/v1/chat/completions", gateway_url))
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&serde_json::json!({
            "model": "openclaw",
            "messages": [{"role": "user", "content": "ping"}],
            "stream": true
        }))
        .send()
        .await
        .expect("gateway should accept the request");

    assert_eq!(response.status(), StatusCode::OK, "gateway should return 200 for a streaming request");

    // Consume the stream. After the first SSE chunk the upstream stalls, so the
    // gateway's per-chunk idle timeout (1 s) should fire and close the connection.
    let mut byte_stream = response.bytes_stream();

    // First chunk must arrive (mock sends it immediately).
    let first = byte_stream.next().await;
    assert!(
        first.is_some() && first.unwrap().is_ok(),
        "first SSE chunk should arrive before the idle timeout"
    );

    // Now wait: the stream should terminate due to the 1-second idle timeout.
    // We allow 5 seconds total (1 s timeout + 4 s slack for CI overhead).
    let termination = tokio::time::timeout(Duration::from_secs(5), byte_stream.next()).await;

    let elapsed = start.elapsed();
    eprintln!(
        "[TEST] Stream terminated after {:.2}s (idle_timeout=1s)",
        elapsed.as_secs_f64()
    );

    // The stream must have terminated — either the gateway closed it (None) or
    // sent an error byte (Err). Both satisfy the assertion. What matters is that
    // we didn't time out waiting (which would mean the gateway is NOT applying
    // the idle timeout — i.e. the hollow-test scenario).
    assert!(
        termination.is_ok(),
        "stream should have terminated within 5s via the idle timeout, but it didn't — \
         this means chat.rs is NOT applying the per-chunk TokioStreamExt::timeout wrapper"
    );

    // Sanity: it should terminate well before 60s (the default / old value).
    assert!(
        elapsed < Duration::from_secs(10),
        "stream took {:.2}s — expected termination within ~2s (1s idle + overhead)",
        elapsed.as_secs_f64()
    );
}

/// A slow-but-flowing stream through the real `chat_completions` handler MUST
/// NOT be dropped prematurely when each chunk arrives within the idle window.
///
/// Setup: 4 chunks × 200 ms apart, idle threshold = 2 s.
/// Each chunk resets the idle timer so all 4 chunks should arrive successfully.
#[tokio::test]
async fn test_idle_timeout_does_not_trigger_on_slow_flowing_stream() {
    // Chunks every 200 ms; idle threshold 2 s — well within the window.
    let upstream_url = start_slow_flowing_sse_upstream(200, 4).await;
    let (gateway_url, auth_token, client) =
        build_stream_test_gateway(upstream_url, 2).await;

    let response = client
        .post(format!("{}/v1/chat/completions", gateway_url))
        .header("Authorization", format!("Bearer {}", auth_token))
        .json(&serde_json::json!({
            "model": "openclaw",
            "messages": [{"role": "user", "content": "stream test"}],
            "stream": true
        }))
        .send()
        .await
        .expect("gateway should accept the request");

    assert_eq!(response.status(), StatusCode::OK);

    // Collect all bytes from the stream. The mock sends 4 chunks + DONE so
    // the stream will close naturally; all bytes should arrive without error.
    // Wrap in a generous timeout in case something goes wrong in CI.
    let all_bytes_result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut all_bytes = Vec::new();
        let mut byte_stream = response.bytes_stream();
        while let Some(chunk) = byte_stream.next().await {
            match chunk {
                Ok(b) => all_bytes.extend_from_slice(&b),
                Err(e) => return Err(format!("stream error: {}", e)),
            }
        }
        Ok(all_bytes)
    })
    .await
    .expect("slow-flowing stream should complete within 15s");

    let all_bytes = all_bytes_result
        .expect("slow-flowing stream should complete without error");

    let body_str = String::from_utf8_lossy(&all_bytes);
    eprintln!("[TEST] Received {} bytes from slow-flowing stream", all_bytes.len());

    // All 4 chunk payloads must be present.
    for i in 0..4 {
        assert!(
            body_str.contains(&format!("chunk{}", i)),
            "chunk {} should be present in the proxied stream body, got: {}",
            i,
            body_str
        );
    }

    assert!(
        body_str.contains("[DONE]"),
        "DONE marker should be present — stream should have completed normally"
    );
}
