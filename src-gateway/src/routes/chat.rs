use crate::auth::TenantContext;
use crate::error::GatewayError;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::{
    body::Body,
    extract::State,
    response::{IntoResponse, Response},
    Extension, Json,
};
use futures_util::StreamExt;
use serde_json::Value;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::Instant;
use tokio_stream::wrappers::ReceiverStream;

/// §22C.4.1.1 — Opaque error type for the correlated-stream channel.
type BoxedError = Box<dyn std::error::Error + Send + Sync>;
type ChunkResult = Result<bytes::Bytes, BoxedError>;

/// §22C.4.1.1 — Decide whether to extend the HTTP SSE idle deadline based on WS activity.
///
/// Returns `true`  → WS channel was active within the window; extend the deadline.
/// Returns `false` → both HTTP SSE **and** WS have been silent; fire the timeout.
///
/// `last_ws_activity_ms`: value from `OcClient::last_ws_activity_ms()` (0 = no message ever).
/// `now_ms`:             caller-provided unix timestamp (ms) — extracted for unit testability.
/// `idle_timeout_ms`:   deadline window in ms.
pub(crate) fn should_extend_deadline(last_ws_activity_ms: u64, now_ms: u64, idle_timeout_ms: u64) -> bool {
    let ws_age_ms = if last_ws_activity_ms == 0 {
        u64::MAX // no WS message ever received — treat as infinitely stale
    } else {
        now_ms.saturating_sub(last_ws_activity_ms)
    };
    ws_age_ms < idle_timeout_ms
}

/// §22C.4.1.1 — WS-correlated SSE stream forwarding task.
///
/// Forwards chunks from `raw_stream` to `tx`.  When the per-chunk deadline fires:
/// - If `ws_activity_fn()` + `now_ms_fn()` show the WS was active within the window,
///   extend the deadline and wait for the next chunk.
/// - If both channels have been silent, emit an error to `tx` and terminate.
///
/// Extracted as a standalone `async fn` so tests can drive it without a live OC connection.
async fn forward_with_ws_correlated_deadline<S, WsFn, NowFn>(
    mut raw_stream: S,
    idle_timeout: Duration,
    ws_activity_fn: WsFn,
    now_ms_fn: NowFn,
    tx: tokio::sync::mpsc::Sender<ChunkResult>,
    idle_timeout_secs: u64,
) where
    S: futures_util::Stream<Item = ChunkResult> + Unpin + Send,
    WsFn: Fn() -> u64 + Send,
    NowFn: Fn() -> u64 + Send,
{
    let idle_timeout_ms = idle_timeout_secs.saturating_mul(1000);
    let mut deadline = Instant::now() + idle_timeout;

    loop {
        tokio::select! {
            biased; // prioritise incoming chunks over the deadline branch

            chunk = raw_stream.next() => {
                match chunk {
                    Some(Ok(bytes)) => {
                        // Chunk received — reset deadline and forward.
                        deadline = Instant::now() + idle_timeout;
                        if tx.send(Ok(bytes)).await.is_err() {
                            // Receiver dropped (Pane disconnected); stop silently.
                            break;
                        }
                    }
                    Some(Err(e)) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                    None => break, // stream ended normally
                }
            }

            _ = tokio::time::sleep_until(deadline) => {
                // HTTP SSE has been silent — check the WS channel.
                let now_ms = now_ms_fn();
                let last_ws_ms = ws_activity_fn();

                if should_extend_deadline(last_ws_ms, now_ms, idle_timeout_ms) {
                    // WS active — OC is executing a tool.  Extend the deadline.
                    eprintln!(
                        "[CHAT] HTTP SSE idle {}s but WS active within window \
                         — extending deadline (s22C.4.1.1)",
                        idle_timeout_secs
                    );
                    deadline = Instant::now() + idle_timeout;
                    // loop continues: back to select!
                } else {
                    // Both channels silent — genuine dead or stuck connection.
                    eprintln!(
                        "[CHAT] Stream idle timeout: HTTP SSE silent {}s, WS also silent \
                         — dropping connection (s22C.4.1.1)",
                        idle_timeout_secs
                    );
                    let _ = tx.send(Err(Box::new(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "Stream idle timeout: no chunk received for {}s (s22C.4.1.1)",
                            idle_timeout_secs
                        ),
                    )))).await;
                    break;
                }
            }
        }
    }
}

/// ADR-0067 D3 / §4A.2.2: extract the agent id from an already agent-scoped `model`
/// selector — `openclaw/<agentId>`, `openclaw:<agentId>`, or `agent:<agentId>` — if the
/// incoming value has one. Returns `None` for a bare `"openclaw"`, a catalog model id
/// (e.g. `"claude-opus-4-6"`), an absent `model`, or a degenerate empty-id selector
/// (`"openclaw/"`) — all of which fall back to gateway-side default resolution instead.
pub(crate) fn extract_agent_selector(model: &str) -> Option<String> {
    for prefix in ["openclaw/", "openclaw:", "agent:"] {
        if let Some(rest) = model.strip_prefix(prefix) {
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

/// ADR-0067 D3 / §4A.2.2: true when `model` already names a specific OC agent and must be
/// forwarded to OC unchanged rather than rewritten to the bare `"openclaw"` sentinel.
///
/// A bare `"openclaw"` (no separator) is NOT agent-scoped — it is OC's own default-agent
/// sentinel, and it — along with an absent or catalog model id — MUST still be rewritten to
/// stay bit-for-bit identical to the pre-ADR-0067 behavior the currently-installed app
/// depends on (the gateway-ships-first backward-compatibility requirement, ADR-0067 D6).
pub(crate) fn is_agent_scoped_model(model: &str) -> bool {
    extract_agent_selector(model).is_some()
}

/// POST /v1/chat/completions - Proxy to OC's chat endpoint
pub async fn chat_completions(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Json(body): Json<Value>,
) -> Result<Response, GatewayError> {
    eprintln!(
        "[CHAT] chat_completions called for tenant: {}, stream: {}",
        tenant.tenant_id,
        body.get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    );

    // Detect streaming mode
    let is_streaming = body
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // §4A.2.2 PARAM-COMPAT: strip deprecated params BEFORE the §22C/ADR-0067 model rewrite.
    //
    // ARCHITECTURAL NOTE: the routing step below may overwrite body["model"] with "openclaw"
    // (§22C / ADR-0067 D3), which means we could lose the real downstream model name. We
    // must resolve and strip FIRST.
    //
    // IMPORTANT — always resolve the actual downstream *model* from agent_registry, never
    // from Pane's body["model"] catalog-id string. Pane's string may be stale (e.g. Pane
    // cache says "claude-opus-4-6" while OC is actually running "claude-opus-4-7").
    // Consulting the strip table with the wrong key lets deprecated params slip through →
    // Anthropic 422 (#1323).
    //
    // ADR-0067 D3 exception: when body["model"] is agent-scoped (openclaw/<id>,
    // openclaw:<id>, agent:<id>) the <id> IS a trustworthy agent SELECTOR — it names which
    // agent's compat rules apply. This is agent selection, not model-string matching; the
    // raw model string still never drives the strip-table lookup key directly (that always
    // stays `agent.model`, read from the registry below).
    //
    // Resolution:
    //   1. An explicit openclaw/<agentId> (or openclaw:/agent:) selector wins — resolve
    //      against THAT agent, never a hardcoded "main" (§0b's #1323-class regression from
    //      the other direction: stripping a non-main agent's request against main's model).
    //   2. Otherwise fall back to the default-resolution order (sync.md §22C.6.2): prefer
    //      the "main" agent, else the first available agent. (Steps 1/2 of that order — OC's
    //      isDefault, the user's Settings choice — need data this layer doesn't carry on
    //      AgentInfo; this is effectively step 3 of that order.)
    //   3. agent_registry.get_agents() is in-memory cache after first call — sub-ms on hot path.
    //   4. On registry error → log explicitly, skip strip (never fail the request).
    let mut body = body;

    // Pane's model field — a debug hint, AND (per ADR-0067 D3) the source of the agent
    // selector when the incoming value is already agent-scoped. Never used to drive the
    // strip-table lookup by literal string match either way.
    let pane_model_hint = body
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    eprintln!(
        "[PARAM_COMPAT] Resolving real downstream model via agent_registry for tenant {} \
         (Pane body[\"model\"] hint: {})",
        tenant.tenant_id,
        pane_model_hint.as_deref().unwrap_or("(missing)")
    );

    // ADR-0067 D3: extract the agent selector, if the incoming model already names one.
    let selected_agent_id: Option<String> = pane_model_hint
        .as_deref()
        .and_then(extract_agent_selector);

    let resolved_model: Option<String> = match state
        .agent_registry
        .get_agents(&tenant.tenant_id, false)
        .await
    {
        Ok(agents) => {
            // §4A.2.2 "Model Resolution for Stripping" / ADR-0067 D3.
            let agent = match selected_agent_id.as_deref() {
                Some(id) => agents.iter().find(|a| a.oc_agent_id == id),
                None => agents
                    .iter()
                    .find(|a| a.oc_agent_id == "main")
                    .or_else(|| agents.first()),
            };
            let model = agent.and_then(|a| a.model.clone());
            match (selected_agent_id.as_deref(), model.as_deref()) {
                (Some(id), Some(m)) => eprintln!(
                    "[PARAM_COMPAT] Resolved model from agent_registry (selected agent={}): {}",
                    id, m
                ),
                (None, Some(m)) => eprintln!(
                    "[PARAM_COMPAT] Resolved model from agent_registry (default agent={}): {}",
                    agent.map(|a| a.oc_agent_id.as_str()).unwrap_or("?"),
                    m
                ),
                (Some(id), None) => eprintln!(
                    "[PARAM_COMPAT] agent_registry: selected agent \"{}\" not found or has no \
                     configured model; skipping param strip",
                    id
                ),
                (None, None) => eprintln!(
                    "[PARAM_COMPAT] agent_registry: default agent has no configured model; \
                     skipping param strip"
                ),
            }
            model
        }
        Err(e) => {
            eprintln!(
                "[PARAM_COMPAT] Warning: agent_registry lookup failed ({}); skipping param strip",
                e
            );
            None
        }
    };

    if let Some(ref model) = resolved_model {
        let stripped = state.param_compat_table.strip_params(model, &mut body);
        for (param, reason) in &stripped {
            eprintln!(
                "[PARAM_COMPAT] Stripped {} for model {}: {}",
                param, model, reason
            );
        }
        if !stripped.is_empty() {
            eprintln!(
                "[PARAM_COMPAT] {} param(s) stripped for model {}",
                stripped.len(),
                model
            );
        }
    }

    // §22C / ADR-0067 D3: pass through an already agent-scoped model unchanged; rewrite
    // only a bare, absent, or catalog model id to "openclaw" for OC's default-agent
    // routing. OC resolves bare "openclaw" to the default agent (whatever model is
    // configured in openclaw.json); "openclaw/<agentId>" (or "openclaw:"/"agent:") routes
    // to that specific agent instead, and is forwarded exactly as received.
    //
    // Backward compatibility (ADR-0067 D6, gateway-ships-first): the currently-installed
    // Pane app still sends a bare "openclaw", a stale catalog model id (e.g.
    // "claude-opus-4-6"), or no model field at all — those MUST land on the exact same
    // rewrite-to-"openclaw" path they do today. This gateway change alone does not fix
    // #2900 for that app; it only stops clobbering a newer app's explicit agent selector.
    // See docs/spec/ai-integration.md §4A.2.2 "Agent-Scoped Routing Pass-Through" and
    // ADR-0067 D3/D6.
    let incoming_model_is_agent_scoped = pane_model_hint
        .as_deref()
        .map(is_agent_scoped_model)
        .unwrap_or(false);

    if !incoming_model_is_agent_scoped {
        body["model"] = serde_json::json!("openclaw");
    }

    // Build upstream URL
    let upstream_url = format!("{}/v1/chat/completions", state.oc_http_base_url);

    eprintln!("[CHAT] Proxying to OC: {}", upstream_url);

    // Build reqwest client.
    //
    // For non-streaming requests we keep the 60s total timeout — those are short-lived.
    //
    // For streaming requests we do NOT set a total wall-clock timeout on the reqwest
    // client; a 120s total timeout fires while a long Opus response is still flowing
    // (see #1358 post-mortem, gateway hop-2 timeout).  Instead we apply a per-chunk
    // idle timeout after we receive the response: if no byte arrives for
    // `state.streaming_idle_timeout_secs` seconds we drop the connection.
    // This means a 5-minute response that trickles chunks every 2 seconds succeeds,
    // while a genuinely stalled connection (nothing for 60+ s) is dropped.
    let client = if is_streaming {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            // No total timeout — per-chunk idle timeout is applied below on the stream.
            .build()
            .map_err(|e| {
                GatewayError::OcClient(format!("Failed to build streaming HTTP client: {}", e))
            })?
    } else {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|e| GatewayError::OcClient(format!("Failed to build HTTP client: {}", e)))?
    };

    // Forward request to OC with bearer token
    let response = client
        .post(&upstream_url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", state.oc_bearer_token))
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            eprintln!("[CHAT] Failed to send request to OC: {}", e);
            GatewayError::OcClient(format!("Failed to proxy chat request: {}", e))
        })?;

    let status = response.status();
    eprintln!("[CHAT] OC response status: {}", status);

    // Handle error responses
    if !status.is_success() {
        let error_body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());

        eprintln!("[CHAT] OC returned error: {}", error_body);

        return Ok((
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            error_body,
        )
            .into_response());
    }

    // §22C.6.7: Capture X-OC-Resolved-Model header value — sourced from `resolved_model` which was
    // already computed above by agent_registry.get_agents() for PARAM_COMPAT (zero extra lookups).
    // Present on 2xx responses (streaming and non-streaming) when model resolved.
    // Absent when resolved_model is None (registry error, agent has no model) or on non-2xx.
    let x_oc_resolved_model_value: Option<HeaderValue> = resolved_model
        .as_deref()
        .and_then(|m| HeaderValue::from_str(m).ok());

    // Proxy response based on streaming mode
    if is_streaming {
        // Streaming: forward SSE byte stream with per-chunk idle timeout.
        //
        // We wrap each call to `next()` with `tokio::time::timeout`.  As long as
        // chunks keep arriving within `idle_timeout_secs` the stream lives.  If the
        // upstream goes completely silent for that window we inject an error chunk
        // which terminates the Body — axum closes the client connection and Pane
        // surface the decode error through the normal error path.
        let idle_timeout_secs = state.streaming_idle_timeout_secs;
        eprintln!(
            "[CHAT] Streaming response from OC (idle_timeout={}s)",
            idle_timeout_secs
        );

        let raw_stream = response.bytes_stream();
        let idle_timeout = Duration::from_secs(idle_timeout_secs);

        // §22C.4.1.1 — WS-correlated idle deadline.
        //
        // The old TokioStreamExt::timeout fired on any HTTP SSE silence, including
        // the normal silence during OC tool execution.  The new design:
        //   - deadline fires after `idle_timeout` of HTTP SSE silence
        //   - check OC WS channel via oc_client.last_ws_activity_ms()
        //   - if WS had traffic within the window → OC is alive; extend deadline
        //   - if both channels silent → genuinely dead; inject error and terminate
        // The 30s WS ping cycle (§22C.4.2) ensures OC liveness is visible even
        // during long tool runs that emit no HTTP SSE bytes.
        let (tx, rx) = tokio::sync::mpsc::channel::<ChunkResult>(32);
        let oc_client = state.oc_client.clone();
        // Map reqwest::Error to BoxedError before passing into the generic task.
        let boxed_stream = raw_stream.map(|r| r.map_err(|e| Box::new(e) as BoxedError));
        tokio::spawn(forward_with_ws_correlated_deadline(
            boxed_stream,
            idle_timeout,
            move || oc_client.last_ws_activity_ms(),
            || {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64
            },
            tx,
            idle_timeout_secs,
        ));

        let body = Body::from_stream(ReceiverStream::new(rx));

        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        headers.insert(
            axum::http::header::CACHE_CONTROL,
            HeaderValue::from_static("no-cache"),
        );
        headers.insert(
            axum::http::header::CONNECTION,
            HeaderValue::from_static("keep-alive"),
        );
        // §22C.6.7: Emit resolved model header on streaming 2xx response.
        if let Some(ref val) = x_oc_resolved_model_value {
            headers.insert(
                axum::http::header::HeaderName::from_static("x-oc-resolved-model"),
                val.clone(),
            );
        }

        Ok((StatusCode::OK, headers, body).into_response())
    } else {
        // Non-streaming: forward JSON response
        let response_body = response.text().await.map_err(|e| {
            eprintln!("[CHAT] Failed to read OC response: {}", e);
            GatewayError::OcClient(format!("Failed to read response: {}", e))
        })?;

        eprintln!("[CHAT] Non-streaming response proxied successfully");

        // §22C.6.7: Non-streaming 2xx — build HeaderMap to include X-OC-Resolved-Model when resolved.
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        if let Some(ref val) = x_oc_resolved_model_value {
            headers.insert(
                axum::http::header::HeaderName::from_static("x-oc-resolved-model"),
                val.clone(),
            );
        }

        Ok((StatusCode::OK, headers, response_body).into_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_derivation() {
        // ws:// -> http://
        let ws_url = "ws://127.0.0.1:18789";
        let http_url = ws_url.replace("ws://", "http://");
        assert_eq!(http_url, "http://127.0.0.1:18789");

        // wss:// -> https://
        let wss_url = "wss://example.com:18789";
        let https_url = wss_url.replace("wss://", "https://");
        assert_eq!(https_url, "https://example.com:18789");
    }

    #[test]
    fn test_streaming_detection() {
        // Test with stream: true
        let json_streaming: Value = serde_json::json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [],
            "stream": true
        });
        assert_eq!(
            json_streaming.get("stream").and_then(|v| v.as_bool()),
            Some(true)
        );

        // Test with stream: false
        let json_non_streaming: Value = serde_json::json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": [],
            "stream": false
        });
        assert_eq!(
            json_non_streaming.get("stream").and_then(|v| v.as_bool()),
            Some(false)
        );

        // Test without stream field (defaults to false)
        let json_no_stream: Value = serde_json::json!({
            "model": "claude-3-5-sonnet-20241022",
            "messages": []
        });
        assert_eq!(
            json_no_stream
                .get("stream")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            false
        );
    }

    /// §22C.6.7: HeaderValue construction from valid model ID string.
    #[test]
    fn test_x_oc_resolved_model_header_value_from_str() {
        let model = "anthropic/claude-opus-4-6"; // CI-ALLOW: hardcoded-model
        let val = HeaderValue::from_str(model);
        assert!(
            val.is_ok(),
            "Valid model string should produce valid HeaderValue"
        );
        assert_eq!(val.unwrap().to_str().unwrap(), model);
    }

    /// §22C.6.7: Header name "x-oc-resolved-model" is valid.
    /// from_static panics on invalid names; passing the test proves the name is valid.
    #[test]
    fn test_x_oc_resolved_model_header_name_valid() {
        let _name = axum::http::header::HeaderName::from_static("x-oc-resolved-model");
    }

    /// §22C.6.7: No HeaderValue produced when resolved_model is None.
    #[test]
    fn test_x_oc_resolved_model_absent_on_none() {
        let resolved_model: Option<String> = None;
        let header_val = resolved_model
            .as_deref()
            .and_then(|m| HeaderValue::from_str(m).ok());
        assert!(header_val.is_none());
    }

    /// §22C.6.7: HeaderValue produced and has correct value when resolved_model is Some.
    #[test]
    fn test_x_oc_resolved_model_present_on_some() {
        let resolved_model: Option<String> = Some("anthropic/claude-opus-4-6".to_string()); // CI-ALLOW: hardcoded-model
        let header_val = resolved_model
            .as_deref()
            .and_then(|m| HeaderValue::from_str(m).ok());
        assert!(header_val.is_some());
        assert_eq!(
            header_val.unwrap().to_str().unwrap(),
            "anthropic/claude-opus-4-6"
        ); // CI-ALLOW: hardcoded-model
    }

    #[test]
    fn test_upstream_url_construction() {
        let base_url = "http://127.0.0.1:18789";
        let upstream_url = format!("{}/v1/chat/completions", base_url);
        assert_eq!(upstream_url, "http://127.0.0.1:18789/v1/chat/completions");
    }

    /// §22C.6.7 integration test: `chat_completions` handler emits `X-OC-Resolved-Model`
    /// on a 2xx non-streaming response with the value sourced from the agent_registry.
    ///
    /// Pattern: mock OC backend (axum oneshot) + seeded AgentRegistry (test-seam) +
    /// tower `oneshot` — no real OC WebSocket required.
    /// This closes the gap that the four unit tests above don't cover: it verifies the
    /// handler actually wires `resolved_model` → response header end-to-end.
    #[tokio::test]
    async fn test_chat_completions_emits_x_oc_resolved_model_integration() {
        use crate::agents::AgentInfo;
        use crate::auth::TenantContext;
        use crate::server::SharedState;
        use axum::{
            body::Body,
            http::{Request, StatusCode},
            routing::post,
            Router,
        };
        use std::sync::Arc;
        use tower::ServiceExt;

        // 1. Spawn a minimal mock OC HTTP backend.
        //    Returns a valid non-streaming chat completion JSON so the handler succeeds.
        let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_addr = mock_listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mock_app = Router::new().route(
                "/v1/chat/completions",
                post(|| async {
                    axum::http::Response::builder()
                        .status(200)
                        .header("content-type", "application/json")
                        .body(Body::from(
                            r#"{"id":"test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":1,"total_tokens":6}}"#,
                        ))
                        .unwrap()
                }),
            );
            axum::serve(mock_listener, mock_app).await.unwrap();
        });

        // 2. Build SharedState: oc_http_base_url → mock backend, agent_registry pre-seeded.
        let oc_client = Arc::new(
            crate::oc_client::OcClient::new(
                "ws://127.0.0.1:1".to_string(), // won't be called — registry is pre-seeded
                "dummy".to_string(),
            )
            .unwrap(),
        );
        let agent_registry = Arc::new(crate::agents::AgentRegistry::new(oc_client.clone()));
        agent_registry
            .seed_agents(
                "test-tenant",
                vec![AgentInfo {
                    oc_agent_id: "main".to_string(),
                    name: "main".to_string(),
                    model: Some("anthropic/claude-opus-4-6".to_string()), // CI-ALLOW: hardcoded-model
                    role: None,
                    workspace: None,
                    is_default: false,
                    registered_at: chrono::Utc::now(),
                }],
            )
            .await;

        let state = Arc::new(SharedState {
            start_time: time::OffsetDateTime::now_utc(),
            tls_fingerprint: "test-fp".to_string(),
            tls_cert_pem: "test-cert".to_string(),
            auth_state: Arc::new(crate::auth::AuthState::new(
                "/tmp/oc_test_tokens_chat.json".into(),
            )),
            oc_client,
            agent_registry,
            models_cache: Arc::new(crate::routes::models::ModelsCache::new()),
            sync_queue: Arc::new(crate::sync_queue::SyncQueue::new()),
            ring_buffer: Arc::new(crate::ring_buffer::RingBuffer::new()),
            idempotency_store: crate::idempotency::IdempotencyStore::new(),
            metrics: Arc::new(crate::metrics::Metrics::new()),
            session_mappings: Arc::new(tokio::sync::RwLock::new(Default::default())),
            oc_http_base_url: format!("http://{}", mock_addr),
            oc_bearer_token: "test-bearer-token".to_string(),
            tls_hostname: None,
            param_compat_table: Arc::new(crate::param_compat::CompatTable::default()),
            streaming_idle_timeout_secs: 60,
        });

        // 3. Build router with handler + state + tenant extension.
        let tenant = TenantContext {
            tenant_id: "test-tenant".to_string(),
        };
        let app = Router::new()
            .route("/v1/chat/completions", post(super::chat_completions))
            .with_state(state)
            .layer(axum::middleware::from_fn(
                move |mut req: Request<Body>, next: axum::middleware::Next| {
                    let tenant = tenant.clone();
                    async move {
                        req.extensions_mut().insert(tenant);
                        next.run(req).await
                    }
                },
            ));

        // 4. Send non-streaming request.
        let req_body = serde_json::json!({
            "model": "anthropic/claude-opus-4-6", // CI-ALLOW: hardcoded-model
            "messages": [{"role": "user", "content": "hello"}],
            "stream": false
        });
        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&req_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();

        // 5. Assert the header is present with the value from the agent_registry.
        assert_eq!(response.status(), StatusCode::OK);
        let header_val = response.headers().get("x-oc-resolved-model").expect(
            "x-oc-resolved-model header must be present on 2xx response when agent has a model",
        );
        assert_eq!(
            header_val.to_str().unwrap(),
            "anthropic/claude-opus-4-6", // CI-ALLOW: hardcoded-model
            "header value must match the registry-resolved model"
        );
    }

    // ────────────────────────────────────────────────────────────────────────────
    // §22C.4.1.1 — WS-correlated deadline: unit + integration tests
    // ────────────────────────────────────────────────────────────────────────────

    // ——— Pure unit tests: should_extend_deadline ———

    /// WS was active 1 ms ago — inside the window → extend.
    #[test]
    fn test_should_extend_deadline_ws_recently_active() {
        let now_ms: u64 = 1_700_000_000_000;
        let last_ws = now_ms - 1; // 1 ms ago
        assert!(
            should_extend_deadline(last_ws, now_ms, 60_000),
            "WS active 1ms ago should extend the deadline"
        );
    }

    /// WS was active exactly at the boundary (idle_timeout_ms ago) — outside → fire.
    #[test]
    fn test_should_extend_deadline_ws_at_boundary() {
        let now_ms: u64 = 1_700_000_000_000;
        let last_ws = now_ms - 60_000; // exactly at boundary (age == timeout)
        assert!(
            !should_extend_deadline(last_ws, now_ms, 60_000),
            "WS age equal to timeout should NOT extend (age is not strictly < timeout)"
        );
    }

    /// WS has never received a message (last_ws == 0) → fire timeout.
    #[test]
    fn test_should_extend_deadline_ws_never_active() {
        let now_ms: u64 = 1_700_000_000_000;
        assert!(
            !should_extend_deadline(0, now_ms, 60_000),
            "WS never active (0) should NOT extend the deadline"
        );
    }

    /// WS was active more than idle_timeout_ms ago → fire timeout.
    #[test]
    fn test_should_extend_deadline_ws_stale() {
        let now_ms: u64 = 1_700_000_000_000;
        let last_ws = now_ms - 120_000; // 2 minutes ago
        assert!(
            !should_extend_deadline(last_ws, now_ms, 60_000),
            "WS silent for 2x timeout should NOT extend the deadline"
        );
    }

    // ——— Integration tests: forward_with_ws_correlated_deadline ———

    /// Branch 1: chunks are forwarded correctly during normal streaming.
    ///
    /// Verifies that all chunks from the underlying stream arrive at the receiver
    /// in order, without any errors, when chunks arrive well within the deadline.
    #[tokio::test]
    async fn test_correlated_stream_forwards_chunks_normally() {
        use tokio_stream::wrappers::ReceiverStream;

        let chunks: Vec<ChunkResult> = vec![
            Ok(bytes::Bytes::from_static(b"data: chunk1\n\n")),
            Ok(bytes::Bytes::from_static(b"data: chunk2\n\n")),
            Ok(bytes::Bytes::from_static(b"data: [DONE]\n\n")),
        ];
        let raw = futures_util::stream::iter(chunks);

        let (tx, rx) = tokio::sync::mpsc::channel::<ChunkResult>(16);
        // 60s timeout — deadline will never fire during this test.
        tokio::spawn(forward_with_ws_correlated_deadline(
            raw,
            Duration::from_secs(60),
            || 0,           // WS activity not relevant for this branch
            || 1_700_000_000_000u64,
            tx,
            60,
        ));

        let results: Vec<ChunkResult> = ReceiverStream::new(rx).collect().await;

        assert_eq!(results.len(), 3, "all 3 chunks must arrive");
        assert_eq!(results[0].as_ref().unwrap(), &bytes::Bytes::from_static(b"data: chunk1\n\n"));
        assert_eq!(results[1].as_ref().unwrap(), &bytes::Bytes::from_static(b"data: chunk2\n\n"));
        assert_eq!(results[2].as_ref().unwrap(), &bytes::Bytes::from_static(b"data: [DONE]\n\n"));
    }

    /// Branch 2: deadline extends when WS shows activity within the idle window.
    ///
    /// Proves the WS-correlation fix: an HTTP-SSE-silent stream that has WS activity
    /// within `idle_timeout` gets an extended deadline rather than an error.
    /// After the extension the stream delivers its final chunk normally.
    #[tokio::test]
    async fn test_correlated_stream_extends_deadline_on_ws_activity() {
        use tokio_stream::wrappers::ReceiverStream;

        // idle_timeout_secs drives idle_timeout_ms = idle_timeout_secs * 1000.
        // We use a 1-second logical window so 1ms WS age < 1000ms → extend.
        // The actual deadline fires after 50ms wall time so the test is fast.
        let idle_timeout_secs: u64 = 1; // 1-second window → idle_timeout_ms = 1000
        let fixed_now_ms: u64 = 1_700_000_100_000; // arbitrary fixed wall time
        // WS was active 1ms ago → age 1ms < 1000ms → should_extend_deadline == true.
        let ws_last_ms: u64 = fixed_now_ms - 1;

        // Controllable stream: delivers its only chunk AFTER the first deadline fires.
        let (stream_tx, stream_rx) = tokio::sync::mpsc::channel::<ChunkResult>(4);
        let stream = ReceiverStream::new(stream_rx);

        let (tx, rx) = tokio::sync::mpsc::channel::<ChunkResult>(16);
        let task_handle = tokio::spawn(forward_with_ws_correlated_deadline(
            stream,
            Duration::from_millis(50), // deadline fires after 50ms wall time
            move || ws_last_ms,
            move || fixed_now_ms,
            tx,
            idle_timeout_secs,
        ));

        // Wait for the deadline to fire (at least 50ms with some slack).
        tokio::time::sleep(Duration::from_millis(80)).await;

        // By now the deadline has fired.  WS was active 1ms ago (relative to
        // fixed_now_ms) so the loop should have extended the deadline instead of
        // injecting an error.  Now deliver the final chunk and close the stream.
        stream_tx
            .send(Ok(bytes::Bytes::from_static(b"data: [DONE]\n\n")))
            .await
            .expect("task must still be alive after deadline extension");
        drop(stream_tx); // EOF signals task to exit

        task_handle.await.expect("task should not panic");

        let results: Vec<ChunkResult> = ReceiverStream::new(rx).collect().await;

        // Must have exactly 1 chunk with no error — the deadline extended, not fired.
        assert_eq!(results.len(), 1, "expected the final chunk; no error should be injected");
        assert!(
            results[0].is_ok(),
            "chunk after deadline extension must be Ok, not an error"
        );
        assert_eq!(
            results[0].as_ref().unwrap(),
            &bytes::Bytes::from_static(b"data: [DONE]\n\n")
        );
    }

    /// Branch 3: timeout fires on genuine dual-channel silence.
    ///
    /// Proves the dead-connection guarantee: when BOTH HTTP SSE and WS have been
    /// silent for `idle_timeout`, an error is injected and the stream terminates.
    #[tokio::test]
    async fn test_correlated_stream_fires_timeout_on_dual_silence() {
        use tokio_stream::wrappers::ReceiverStream;

        // Empty channel stream (no chunks ever arrive) — simulates dead connection.
        let (_stream_tx, stream_rx) = tokio::sync::mpsc::channel::<ChunkResult>(4);
        let stream = ReceiverStream::new(stream_rx);

        let (tx, rx) = tokio::sync::mpsc::channel::<ChunkResult>(16);
        let task_handle = tokio::spawn(forward_with_ws_correlated_deadline(
            stream,
            Duration::from_millis(50), // fast deadline for test speed
            || 0,                       // WS never active — both channels silent
            || 1_700_000_000_000u64,    // fixed now
            tx,
            0,
        ));

        task_handle.await.expect("task should not panic");

        let results: Vec<ChunkResult> = ReceiverStream::new(rx).collect().await;

        // Must have exactly 1 item and it must be an error.
        assert_eq!(
            results.len(),
            1,
            "exactly one error item must be emitted on dual-channel silence"
        );
        assert!(
            results[0].is_err(),
            "the emitted item must be an error (timeout sentinel)"
        );
        let err_msg = results[0].as_ref().unwrap_err().to_string();
        assert!(
            err_msg.contains("idle timeout") || err_msg.contains("TimedOut") || err_msg.contains("22C"),
            "error message should reference the timeout cause; got: {}",
            err_msg
        );
    }

    // ────────────────────────────────────────────────────────────────────────────
    // ADR-0067 D3/D6 — agent-scoped model selector: unit tests for the two pure helpers
    // ────────────────────────────────────────────────────────────────────────────

    #[test]
    fn test_extract_agent_selector_openclaw_slash() {
        assert_eq!(
            extract_agent_selector("openclaw/researcher"),
            Some("researcher".to_string())
        );
    }

    #[test]
    fn test_extract_agent_selector_openclaw_colon() {
        assert_eq!(
            extract_agent_selector("openclaw:researcher"),
            Some("researcher".to_string())
        );
    }

    #[test]
    fn test_extract_agent_selector_agent_colon() {
        assert_eq!(
            extract_agent_selector("agent:researcher"),
            Some("researcher".to_string())
        );
    }

    #[test]
    fn test_extract_agent_selector_bare_openclaw_is_none() {
        assert_eq!(extract_agent_selector("openclaw"), None);
    }

    #[test]
    fn test_extract_agent_selector_catalog_model_is_none() {
        assert_eq!(extract_agent_selector("claude-opus-4-6"), None); // CI-ALLOW: hardcoded-model
    }

    #[test]
    fn test_extract_agent_selector_empty_agent_id_is_none() {
        // A degenerate "openclaw/" (nothing after the separator) is not a real selector.
        assert_eq!(extract_agent_selector("openclaw/"), None);
        assert_eq!(extract_agent_selector("openclaw:"), None);
        assert_eq!(extract_agent_selector("agent:"), None);
    }

    #[test]
    fn test_is_agent_scoped_model_true_cases() {
        assert!(is_agent_scoped_model("openclaw/main"));
        assert!(is_agent_scoped_model("openclaw:researcher"));
        assert!(is_agent_scoped_model("agent:coder"));
    }

    #[test]
    fn test_is_agent_scoped_model_false_for_bare_openclaw() {
        // The non-regression contract (ADR-0067 D6): a bare "openclaw" must NOT be treated
        // as agent-scoped, or the currently-installed app's requests would stop being
        // rewritten and reach OC as a non-routing string.
        assert!(!is_agent_scoped_model("openclaw"));
    }

    #[test]
    fn test_is_agent_scoped_model_false_for_catalog_id() {
        assert!(!is_agent_scoped_model("claude-opus-4-6")); // CI-ALLOW: hardcoded-model
    }

    #[test]
    fn test_is_agent_scoped_model_false_for_empty_string() {
        assert!(!is_agent_scoped_model(""));
    }

    // ────────────────────────────────────────────────────────────────────────────
    // ADR-0067 — chat_completions integration tests: outbound body + resolved model
    // ────────────────────────────────────────────────────────────────────────────

    /// Shared test harness for the ADR-0067 integration tests below. Spins up a mock OC
    /// HTTP backend that captures the JSON body it actually received — so tests assert on
    /// exactly what `chat_completions` forwarded, not just on the handler's response — and
    /// always answers with a valid non-streaming chat completion. Returns the built router
    /// plus the shared capture slot.
    async fn build_test_app_with_capture(
        label: &str,
        agents: Vec<crate::agents::AgentInfo>,
    ) -> (axum::Router, Arc<std::sync::Mutex<Option<Value>>>) {
        use crate::auth::TenantContext;
        use crate::server::SharedState;
        use axum::{body::Body, http::Request, routing::post, Router};

        let captured: Arc<std::sync::Mutex<Option<Value>>> = Arc::new(std::sync::Mutex::new(None));
        let captured_for_mock = captured.clone();

        let mock_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_addr = mock_listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mock_app = Router::new().route(
                "/v1/chat/completions",
                post(move |Json(body): Json<Value>| {
                    let captured = captured_for_mock.clone();
                    async move {
                        *captured.lock().unwrap() = Some(body);
                        axum::http::Response::builder()
                            .status(200)
                            .header("content-type", "application/json")
                            .body(Body::from(
                                r#"{"id":"test","object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":1,"total_tokens":6}}"#,
                            ))
                            .unwrap()
                    }
                }),
            );
            axum::serve(mock_listener, mock_app).await.unwrap();
        });

        let oc_client = Arc::new(
            crate::oc_client::OcClient::new("ws://127.0.0.1:1".to_string(), "dummy".to_string())
                .unwrap(),
        );
        let agent_registry = Arc::new(crate::agents::AgentRegistry::new(oc_client.clone()));
        agent_registry.seed_agents("test-tenant", agents).await;

        let state = Arc::new(SharedState {
            start_time: time::OffsetDateTime::now_utc(),
            tls_fingerprint: "test-fp".to_string(),
            tls_cert_pem: "test-cert".to_string(),
            auth_state: Arc::new(crate::auth::AuthState::new(
                format!(
                    "/tmp/oc_test_tokens_chat_adr0067_{}_{}.json",
                    std::process::id(),
                    label
                )
                .into(),
            )),
            oc_client,
            agent_registry,
            models_cache: Arc::new(crate::routes::models::ModelsCache::new()),
            sync_queue: Arc::new(crate::sync_queue::SyncQueue::new()),
            ring_buffer: Arc::new(crate::ring_buffer::RingBuffer::new()),
            idempotency_store: crate::idempotency::IdempotencyStore::new(),
            metrics: Arc::new(crate::metrics::Metrics::new()),
            session_mappings: Arc::new(tokio::sync::RwLock::new(Default::default())),
            oc_http_base_url: format!("http://{}", mock_addr),
            oc_bearer_token: "test-bearer-token".to_string(),
            tls_hostname: None,
            param_compat_table: Arc::new(crate::param_compat::CompatTable::default()),
            streaming_idle_timeout_secs: 60,
        });

        let tenant = TenantContext {
            tenant_id: "test-tenant".to_string(),
        };
        let app = Router::new()
            .route("/v1/chat/completions", post(super::chat_completions))
            .with_state(state)
            .layer(axum::middleware::from_fn(
                move |mut req: Request<Body>, next: axum::middleware::Next| {
                    let tenant = tenant.clone();
                    async move {
                        req.extensions_mut().insert(tenant);
                        next.run(req).await
                    }
                },
            ));

        (app, captured)
    }

    fn test_agent(oc_agent_id: &str, model: &str) -> crate::agents::AgentInfo {
        crate::agents::AgentInfo {
            oc_agent_id: oc_agent_id.to_string(),
            name: oc_agent_id.to_string(),
            model: Some(model.to_string()),
            role: None,
            workspace: None,
            is_default: false,
            registered_at: chrono::Utc::now(),
        }
    }

    async fn send_chat_request(app: axum::Router, req_body: Value) {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&req_body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::OK,
            "mock OC backend always returns 200; a non-200 means the request never reached it"
        );
    }

    /// **THE NON-REGRESSION GATE (ADR-0067 D6 / scope doc §5.1).** Reed's part-1 audit
    /// flagged this test as vacuous: it used to send bare `model: "openclaw"` and assert
    /// the wire body is `"openclaw"` — input equal to expected output, so it passed
    /// identically whether `chat_completions`'s stamping logic ran or was deleted
    /// outright.
    ///
    /// Audited fact (tag `v1.0.55`, commit `3130a321`, `src-tauri/src/ai/openrouter.rs`):
    /// for `RequestOrigin::OpenClaw` the installed app unconditionally overwrites
    /// `body["model"] = "openclaw"` on both the streaming and non-streaming paths
    /// immediately before the request leaves the machine (see the load-bearing comment
    /// there cross-referencing `migrate_v83`'s `"openclaw-pending"` placeholder) — so in
    /// practice 1.0.55 *never* puts a raw catalog id on the wire for an OpenClaw request;
    /// bare `"openclaw"` is the only string it actually sends. That client-side rewrite is
    /// exactly the kind of redundant belt-and-suspenders behavior that made the old
    /// version of this test unable to distinguish "gateway stamps correctly" from "gateway
    /// stamping is gone but the client papered over it".
    ///
    /// This test now sends the catalog model id that lives in `ai_agents.model` for the OC
    /// agent — the value `request.model` holds internally immediately before that
    /// client-side override strips it (the same `"anthropic/claude-sonnet-4-5"` shape used
    /// throughout this fixture set and in `context_budget.rs`'s registry) — bypassing the
    /// client-side stamp entirely and asserting the GATEWAY independently rewrites it to
    /// `"openclaw"`. Input now differs from the asserted output, so this fails if
    /// `chat_completions`'s own stamping logic regresses or is deleted, which is what "THE
    /// NON-REGRESSION GATE" is supposed to catch. (Deliberately a *current*, non-stale
    /// catalog id — the stale-id case is already covered, and already discriminates, by
    /// `test_non_regression_stale_catalog_model_still_stamped_openclaw` below.)
    #[tokio::test]
    async fn test_non_regression_bare_openclaw_still_stamped_openclaw() {
        let (app, captured) = build_test_app_with_capture(
            "bare",
            vec![test_agent("main", "anthropic/claude-sonnet-4-5")],
        )
        .await;

        send_chat_request(
            app,
            serde_json::json!({
                "model": "anthropic/claude-sonnet-4-5", // CI-ALLOW: hardcoded-model
                "messages": [{"role": "user", "content": "hi"}],
                "stream": false
            }),
        )
        .await;

        let body = captured
            .lock()
            .unwrap()
            .clone()
            .expect("mock OC must have received a request");
        assert_eq!(
            body["model"], "openclaw",
            "a catalog model id (the agent's real downstream model, not the bare \"openclaw\" \
             sentinel) must still be stamped \"openclaw\" by the gateway's OWN rewrite logic — \
             this fails if that logic regresses, unlike the vacuous bare-in/bare-out shape this \
             test used before (Reed's part-1 audit)"
        );
    }

    /// **THE NON-REGRESSION GATE, absent-model variant.** No `model` field at all must
    /// still land on the same "stamp openclaw" path as today.
    #[tokio::test]
    async fn test_non_regression_absent_model_still_stamped_openclaw() {
        let (app, captured) = build_test_app_with_capture(
            "absent",
            vec![test_agent("main", "anthropic/claude-sonnet-4-5")],
        )
        .await;

        send_chat_request(
            app,
            serde_json::json!({
                "messages": [{"role": "user", "content": "hi"}],
                "stream": false
            }),
        )
        .await;

        let body = captured
            .lock()
            .unwrap()
            .clone()
            .expect("mock OC must have received a request");
        assert_eq!(
            body["model"], "openclaw",
            "absent model must be stamped \"openclaw\", same as today"
        );
    }

    /// **THE NON-REGRESSION GATE, stale-catalog-id variant.** A stale Pane catalog id
    /// (e.g. an old app's cached "claude-opus-4-6") must still be rewritten to
    /// "openclaw" — never forwarded as-is and never treated as agent-scoped.
    #[tokio::test]
    async fn test_non_regression_stale_catalog_model_still_stamped_openclaw() {
        let (app, captured) = build_test_app_with_capture(
            "catalog",
            vec![test_agent("main", "anthropic/claude-sonnet-4-5")],
        )
        .await;

        send_chat_request(
            app,
            serde_json::json!({
                "model": "claude-opus-4-6", // CI-ALLOW: hardcoded-model
                "messages": [{"role": "user", "content": "hi"}],
                "stream": false
            }),
        )
        .await;

        let body = captured
            .lock()
            .unwrap()
            .clone()
            .expect("mock OC must have received a request");
        assert_eq!(
            body["model"], "openclaw",
            "a stale catalog model id must be rewritten to \"openclaw\", not forwarded"
        );
    }

    /// **Pass-through (ADR-0067 D3).** An `openclaw/<agentId>` selector is forwarded to
    /// OC unchanged — the gateway does not clobber a newer app's explicit agent choice.
    #[tokio::test]
    async fn test_agent_scoped_openclaw_slash_model_passes_through_unchanged() {
        let (app, captured) = build_test_app_with_capture(
            "slash",
            vec![
                test_agent("main", "anthropic/claude-sonnet-4-5"),
                test_agent("researcher", "anthropic/claude-sonnet-4-5"),
            ],
        )
        .await;

        send_chat_request(
            app,
            serde_json::json!({
                "model": "openclaw/researcher",
                "messages": [{"role": "user", "content": "hi"}],
                "stream": false
            }),
        )
        .await;

        let body = captured
            .lock()
            .unwrap()
            .clone()
            .expect("mock OC must have received a request");
        assert_eq!(
            body["model"], "openclaw/researcher",
            "an agent-scoped model must pass through to OC byte-for-byte unchanged"
        );
    }

    /// **Pass-through, alternate selector prefixes.** `openclaw:<agentId>` and
    /// `agent:<agentId>` are both documented OC selector forms (ADR-0067 D1/D3) and must
    /// also pass through unchanged, not just the `openclaw/` form.
    #[tokio::test]
    async fn test_agent_scoped_alternate_prefixes_pass_through_unchanged() {
        for model in ["openclaw:researcher", "agent:researcher"] {
            let (app, captured) = build_test_app_with_capture(
                &format!("alt-{}", model.replace(':', "-")),
                vec![
                    test_agent("main", "anthropic/claude-sonnet-4-5"),
                    test_agent("researcher", "anthropic/claude-sonnet-4-5"),
                ],
            )
            .await;

            send_chat_request(
                app,
                serde_json::json!({
                    "model": model,
                    "messages": [{"role": "user", "content": "hi"}],
                    "stream": false
                }),
            )
            .await;

            let body = captured
                .lock()
                .unwrap()
                .clone()
                .expect("mock OC must have received a request");
            assert_eq!(
                body["model"], model,
                "selector form \"{}\" must pass through unchanged",
                model
            );
        }
    }

    /// **Param-strip resolves against the selected agent, not "main" (ADR-0067 D3; the
    /// #1323-class regression from the other direction).** Seed two agents with
    /// DIFFERENT models; send a request scoped to the non-main agent; assert the
    /// `X-OC-Resolved-Model` response header (sourced from the exact same
    /// `resolved_model` the strip table consults) reflects the SELECTED agent's model,
    /// not main's.
    #[tokio::test]
    async fn test_param_compat_resolves_against_selected_agent_not_main() {
        let (app, _captured) = build_test_app_with_capture(
            "selected-agent",
            vec![
                test_agent("main", "anthropic/claude-opus-4-6"), // CI-ALLOW: hardcoded-model
                test_agent("researcher", "anthropic/claude-sonnet-4-5"),
            ],
        )
        .await;

        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "model": "openclaw/researcher",
                    "messages": [{"role": "user", "content": "hi"}],
                    "stream": false
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let resolved = response
            .headers()
            .get("x-oc-resolved-model")
            .expect("x-oc-resolved-model header must be present")
            .to_str()
            .unwrap()
            .to_string();

        assert_eq!(
            resolved, "anthropic/claude-sonnet-4-5",
            "must resolve against the SELECTED agent (researcher), not main"
        );
        assert_ne!(
            resolved,
            "anthropic/claude-opus-4-6", // CI-ALLOW: hardcoded-model
            "must NOT resolve against main's model when a different agent is selected"
        );
    }

    /// **Default-resolution fallback is unchanged when no selector is present** (non-
    /// regression companion to the test above). No `openclaw/<agentId>` selector in the
    /// request → strip-table resolution still prefers "main", exactly as before
    /// ADR-0067.
    #[tokio::test]
    async fn test_param_compat_default_resolution_still_prefers_main_without_selector() {
        let (app, _captured) = build_test_app_with_capture(
            "default-main",
            vec![
                test_agent("researcher", "anthropic/claude-sonnet-4-5"),
                test_agent("main", "anthropic/claude-opus-4-6"), // CI-ALLOW: hardcoded-model
            ],
        )
        .await;

        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::to_string(&serde_json::json!({
                    "model": "openclaw",
                    "messages": [{"role": "user", "content": "hi"}],
                    "stream": false
                }))
                .unwrap(),
            ))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);

        let resolved = response
            .headers()
            .get("x-oc-resolved-model")
            .expect("x-oc-resolved-model header must be present")
            .to_str()
            .unwrap()
            .to_string();

        assert_eq!(
            resolved,
            "anthropic/claude-opus-4-6", // CI-ALLOW: hardcoded-model
            "with no selector present, resolution must still prefer the \"main\" agent"
        );
    }
}
