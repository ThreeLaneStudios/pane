use crate::auth::TenantContext;
use crate::error::GatewayError;
use axum::{
    extract::{Path, Query, State},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

/// Maximum message content size (1 MiB = 1,048,576 bytes)
pub const SESSION_MESSAGE_MAX_BYTES: usize = 1 * 1024 * 1024;

/// POST /v1/sessions request
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub oc_agent_id: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub context_hint: Option<String>,
}

/// POST /v1/sessions response
#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub oc_session_id: String,
}

/// GET /v1/sessions response
#[derive(Debug, Serialize)]
pub struct ListSessionsResponse {
    pub sessions: Vec<SessionInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub title: String,
    pub oc_agent_id: String,
    pub created_at: DateTime<Utc>,
    pub last_message_at: Option<DateTime<Utc>>,
}



/// GET /v1/sessions/:session_id/messages query params
#[derive(Debug, Deserialize)]
pub struct MessagesQuery {
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    50
}

const MAX_LIMIT: usize = 200;

/// GET /v1/sessions/:session_id/messages response
#[derive(Debug, Serialize)]
pub struct MessagesResponse {
    pub messages: Vec<serde_json::Value>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
}

/// DELETE /v1/sessions/:session_id response
#[derive(Debug, Serialize)]
pub struct DeleteSessionResponse {
    pub deleted_at: DateTime<Utc>,
}

/// POST /v1/sessions/:session_id/messages request
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default = "generate_idempotency_key")]
    pub idempotency_key: String,
}

fn generate_idempotency_key() -> String {
    Uuid::new_v4().to_string()
}

fn default_role() -> String {
    "user".to_string()
}

/// POST /v1/sessions/:session_id/messages response
#[derive(Debug, Serialize, Clone)]
pub struct SendMessageResponse {
    pub message_id: String,
    pub accepted_at: DateTime<Utc>,
    pub oc_message_id: String,
}

/// POST /v1/sessions/:session_id/abort response
#[derive(Debug, Serialize)]
pub struct AbortResponse {
    pub aborted_at: DateTime<Utc>,
}

/// OC's documented `sessions.create` request schema (docs/spec/sync.md §22C.6.5.2,
/// captured from OC v2026.4.11). OC's own validator rejects any property outside
/// this set with `INVALID_REQUEST: unexpected property '<name>'` — this list is
/// the single source of truth for what the gateway is allowed to send. Keeping it
/// here (rather than hand-pinning the JSON shape in a test) means a future field
/// invented in `build_oc_create_params` fails `test_build_oc_create_params_only_emits_schema_fields`
/// below without anyone having to update this list's consumer first.
const OC_SESSIONS_CREATE_SCHEMA_FIELDS: &[&str] =
    &["key", "agentId", "label", "model", "parentSessionKey", "task", "message"];

/// Build the OC `sessions.create` RPC params from Pane's gateway request.
///
/// Pane's `CreateSessionRequest` has two fields (`title`, `context_hint`) that
/// are NOT part of OC's schema at all (see `OC_SESSIONS_CREATE_SCHEMA_FIELDS`).
/// `title` maps to OC's `label`; `context_hint` has no OC equivalent and is
/// dropped — OC has no slot for it (docs/spec/sync.md §22C.6.5.2's "Gateway →
/// Pane mapping" never lists one). Extracted as a pure function so the exact
/// wire shape is unit-testable without the WS/mock-server layer.
fn build_oc_create_params(request: &CreateSessionRequest) -> serde_json::Value {
    let mut params = serde_json::json!({ "agentId": request.oc_agent_id });
    if let Some(title) = &request.title {
        // Send "label" only when present — OC's schema marks it optional, and
        // explicitly sending `"label": null` for an omitted title could itself
        // trip a stricter validator depending on how OC treats null vs. absent.
        params["label"] = serde_json::json!(title);
    }
    // Defensive runtime check against regressions: if a future edit adds a
    // field here that isn't in OC's documented schema, fail loudly in debug
    // builds (and the unit test below catches it in CI either way) rather
    // than silently shipping another `unexpected property` 400.
    debug_assert!(
        params
            .as_object()
            .map(|obj| obj
                .keys()
                .all(|k| OC_SESSIONS_CREATE_SCHEMA_FIELDS.contains(&k.as_str())))
            .unwrap_or(false),
        "build_oc_create_params emitted a field outside OC's documented sessions.create schema"
    );
    params
}

/// POST /v1/sessions - Create a new session
pub async fn create(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, GatewayError> {
    eprintln!(
        "[SESSIONS] Create session: tenant={}, agent={}",
        tenant.tenant_id, request.oc_agent_id
    );

    // Call sessions.create over OC WebSocket. OC expects agentId (not
    // oc_agent_id) and has no context_hint field at all — see
    // build_oc_create_params for the full mapping rationale.
    let params = build_oc_create_params(&request);

    let result = state.oc_client.call("sessions.create", params).await?;

    // Parse response.
    // OC's session KEY is the stable identifier — every OC handler (sessions.send,
    // chat.history, ...) resolves sessions via loadSessionEntry(sessionKey), and
    // sessionId is a field ON the resolved entry, not an entry point itself. OC can
    // mint a NEW sessionId under the SAME key (e.g. an out-of-turn send that finds
    // no live entry for the key), so caching sessionId here would go stale. Prefer
    // "key" so session_mappings stores the value that survives rotation; fall back
    // to "sessionId" only if OC didn't return a key at all (unexpected shape).
    let oc_session_key = result
        .get("key")
        .or_else(|| result.get("sessionId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| GatewayError::OcClient("Missing key/sessionId in response".to_string()))?
        .to_string();

    let session_id = Uuid::new_v4().to_string();

    eprintln!(
        "[SESSIONS] Session created: id={}, oc_session_key={}",
        session_id, oc_session_key
    );

    // Store gateway session_id → OC session KEY mapping (not sessionId — see above).
    state.session_mappings.write().await.insert(session_id.clone(), oc_session_key.clone());

    Ok(Json(CreateSessionResponse {
        session_id,
        created_at: Utc::now(),
        oc_session_id: oc_session_key,
    }))
}

/// GET /v1/sessions - List sessions
pub async fn list(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
) -> Result<Json<ListSessionsResponse>, GatewayError> {
    eprintln!("[SESSIONS] List sessions: tenant={}", tenant.tenant_id);

    // Call sessions.list over OC WebSocket
    let result = state
        .oc_client
        .call("sessions.list", serde_json::json!({}))
        .await?;

    // Parse response
    let sessions_array = result
        .get("sessions")
        .and_then(|v| v.as_array())
        .ok_or_else(|| GatewayError::OcClient("Missing sessions in response".to_string()))?;

    // OC returns camelCase fields; manually map to gateway's snake_case SessionInfo
    let sessions: Vec<SessionInfo> = sessions_array
        .iter()
        .filter_map(|s| {
            let session_id = s.get("sessionId")?.as_str()?.to_string();
            let title = s.get("displayName").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let oc_agent_id = s.get("agentId")?.as_str()?.to_string();
            let created_at = s.get("createdAt")?.as_str()?;
            let created_at = DateTime::parse_from_rfc3339(created_at).ok()?.with_timezone(&Utc);
            let last_message_at = s.get("updatedAt")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            
            Some(SessionInfo {
                session_id,
                title,
                oc_agent_id,
                created_at,
                last_message_at,
            })
        })
        .collect();

    eprintln!("[SESSIONS] Returning {} sessions", sessions.len());

    Ok(Json(ListSessionsResponse { sessions }))
}



/// GET /v1/sessions/:session_id/messages - Paginated message history
pub async fn messages(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(session_id): Path<String>,
    Query(query): Query<MessagesQuery>,
) -> Result<Json<MessagesResponse>, GatewayError> {
    eprintln!(
        "[SESSIONS] Get messages: tenant={}, session={}, limit={}",
        tenant.tenant_id, session_id, query.limit
    );

    // Enforce max limit
    let limit = query.limit.min(MAX_LIMIT);

    // Look up OC session key (session_mappings now stores the stable key, not a
    // sessionId that can rotate — see create()'s comment).
    let oc_session_key = state.session_mappings.read().await
        .get(&session_id).cloned()
        .unwrap_or_else(|| session_id.clone());

    // Call chat.history over OC WebSocket.
    // OC's ChatHistoryParamsSchema requires "sessionKey" (not "key") and declares
    // additionalProperties: false with no "before"/"since" fields at all — sending
    // either (even as JSON null, which is what serde_json::json! produces for
    // Option::None) causes OC to reject the call, which is the 502 this fixes.
    // OC has no native since-by-message-id cursor (see sync.md:1599), so for this
    // slice we always request the most recent `limit` messages and let the caller
    // dedup client-side; before/since are intentionally dropped from the outgoing
    // OC call rather than translated.
    let params = serde_json::json!({
        "sessionKey": oc_session_key,
        "limit": limit,
    });

    let result = state.oc_client.call("chat.history", params).await?;

    // Parse response
    let messages_value = result
        .get("messages")
        .ok_or_else(|| GatewayError::OcClient("Missing messages in response".to_string()))?
        .clone();

    let messages: Vec<serde_json::Value> = serde_json::from_value(messages_value)
        .map_err(|e| GatewayError::OcClient(format!("Failed to parse messages: {}", e)))?;

    // OC returns camelCase pagination fields; gateway adapts to snake_case
    let has_more = result
        .get("hasMore")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let next_cursor = result
        .get("nextCursor")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    eprintln!("[SESSIONS] Returning {} messages", messages.len());

    Ok(Json(MessagesResponse {
        messages,
        has_more,
        next_cursor,
    }))
}

/// DELETE /v1/sessions/:session_id - Delete session
pub async fn delete_session(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(session_id): Path<String>,
) -> Result<Json<DeleteSessionResponse>, GatewayError> {
    eprintln!(
        "[SESSIONS] Delete session: tenant={}, session={}",
        tenant.tenant_id, session_id
    );

    // Look up OC ID
    let oc_id = state.session_mappings.read().await
        .get(&session_id).cloned()
        .unwrap_or_else(|| session_id.clone());

    // Call sessions.delete over OC WebSocket
    // OC expects key (not session_id)
    let params = serde_json::json!({
        "key": oc_id,
    });

    state.oc_client.call("sessions.delete", params).await?;

    // Remove mapping after successful deletion
    state.session_mappings.write().await.remove(&session_id);

    eprintln!("[SESSIONS] Session deleted: {}", session_id);

    Ok(Json(DeleteSessionResponse {
        deleted_at: Utc::now(),
    }))
}

/// POST /v1/sessions/:session_id/messages - Send message (with idempotency)
pub async fn send_message(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(session_id): Path<String>,
    Json(request): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, GatewayError> {
    eprintln!(
        "[SESSIONS] Send message: tenant={}, session={}, key={}",
        tenant.tenant_id, session_id, request.idempotency_key
    );

    // Validate message content size
    if request.content.len() > SESSION_MESSAGE_MAX_BYTES {
        eprintln!(
            "[SESSIONS] Message content too large: {} bytes (max {})",
            request.content.len(),
            SESSION_MESSAGE_MAX_BYTES
        );
        return Err(GatewayError::PayloadTooLarge { max_bytes: SESSION_MESSAGE_MAX_BYTES });
    }

    let message_id = Uuid::new_v4().to_string();

    // Check idempotency - this returns existing record if key was already used
    if let Some(existing) = state
        .idempotency_store
        .check_or_insert(
            &tenant.tenant_id,
            &session_id,
            &request.idempotency_key,
            message_id.clone(),
            String::new(), // Will be filled in after OC call
        )
        .await
    {
        eprintln!(
            "[SESSIONS] Returning existing response for idempotency key: {}",
            request.idempotency_key
        );
        return Ok(Json(SendMessageResponse {
            message_id: existing.message_id,
            accepted_at: existing.accepted_at,
            oc_message_id: existing.oc_message_id,
        }));
    }

    // Look up OC ID
    let oc_id = state.session_mappings.read().await
        .get(&session_id).cloned()
        .unwrap_or_else(|| session_id.clone());

    // Call sessions.send over OC WebSocket
    // OC expects key (not session_id), message (not content), and idempotencyKey (camelCase)
    let params = serde_json::json!({
        "key": oc_id,
        "message": request.content,
        "idempotencyKey": request.idempotency_key,
    });

    let result = match state.oc_client.call("sessions.send", params).await {
        Ok(r) => r,
        Err(e) => {
            // Remove poisoned placeholder so retries get a clean slate
            state.idempotency_store.remove(
                &tenant.tenant_id,
                &session_id,
                &request.idempotency_key,
            ).await;
            return Err(e);
        }
    };

    // OC returns messageSeq (number), convert to string for oc_message_id
    let oc_message_id = result
        .get("messageSeq")
        .and_then(|v| {
            if let Some(num) = v.as_u64() {
                Some(num.to_string())
            } else if let Some(s) = v.as_str() {
                Some(s.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let accepted_at = Utc::now();

    // Update idempotency record with real OC message ID
    state
        .idempotency_store
        .update(
            &tenant.tenant_id,
            &session_id,
            &request.idempotency_key,
            crate::idempotency::IdempotencyRecord {
                message_id: message_id.clone(),
                oc_message_id: oc_message_id.clone(),
                accepted_at,
                created_at: accepted_at,
            },
        )
        .await;

    eprintln!(
        "[SESSIONS] Message sent: id={}, oc_message_id={}",
        message_id, oc_message_id
    );

    Ok(Json(SendMessageResponse {
        message_id,
        accepted_at,
        oc_message_id,
    }))
}

/// POST /v1/sessions/:session_id/abort - Abort generation
pub async fn abort(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Path(session_id): Path<String>,
) -> Result<Json<AbortResponse>, GatewayError> {
    eprintln!(
        "[SESSIONS] Abort session: tenant={}, session={}",
        tenant.tenant_id, session_id
    );

    // Look up OC ID
    let oc_id = state.session_mappings.read().await
        .get(&session_id).cloned()
        .unwrap_or_else(|| session_id.clone());

    // Call sessions.abort over OC WebSocket
    // OC expects key (not session_id)
    let params = serde_json::json!({
        "key": oc_id,
    });

    state.oc_client.call("sessions.abort", params).await?;

    eprintln!("[SESSIONS] Session aborted: {}", session_id);

    Ok(Json(AbortResponse {
        aborted_at: Utc::now(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 50);
    }

    #[test]
    fn test_max_limit_enforcement() {
        let query = MessagesQuery {
            before: None,
            since: None,
            limit: 1000,
        };

        let limit = query.limit.min(MAX_LIMIT);
        assert_eq!(limit, MAX_LIMIT);
    }

    #[test]
    fn test_default_role() {
        assert_eq!(default_role(), "user");
    }

    #[tokio::test]
    async fn test_session_mapping_store_and_retrieve() {
        let mappings = tokio::sync::RwLock::new(std::collections::HashMap::new());
        mappings.write().await.insert("gw-123".to_string(), "oc-456".to_string());
        let oc_id = mappings.read().await.get("gw-123").cloned();
        assert_eq!(oc_id, Some("oc-456".to_string()));
    }

    #[tokio::test]
    async fn test_session_mapping_cleanup_on_delete() {
        let mappings = tokio::sync::RwLock::new(std::collections::HashMap::new());
        mappings.write().await.insert("gw-123".to_string(), "oc-456".to_string());
        mappings.write().await.remove("gw-123");
        assert_eq!(mappings.read().await.get("gw-123"), None);
    }

    #[tokio::test]
    async fn test_session_mapping_fallback() {
        let mappings = tokio::sync::RwLock::new(std::collections::HashMap::<String, String>::new());
        let session_id = "unknown-id".to_string();
        let oc_id = mappings.read().await.get(&session_id).cloned().unwrap_or_else(|| session_id.clone());
        assert_eq!(oc_id, "unknown-id");
    }

    #[tokio::test]
    async fn test_create_populates_mapping_for_send() {
        // Simulate what create() does
        let mappings = tokio::sync::RwLock::new(std::collections::HashMap::new());
        
        let gateway_id = "gw-abc".to_string();
        let oc_session_id = "oc-xyz".to_string();
        
        // This is what create() SHOULD do (and now does):
        mappings.write().await.insert(gateway_id.clone(), oc_session_id.clone());
        
        // This is what send_message() does — look up the OC ID:
        let looked_up = mappings.read().await
            .get(&gateway_id).cloned()
            .unwrap_or_else(|| gateway_id.clone());
        
        // Assert: send_message gets the OC ID, not the gateway ID
        assert_eq!(looked_up, "oc-xyz");
        assert_ne!(looked_up, "gw-abc");
    }

    /// Matt's requirement: catch ANY future invented field, not just today's
    /// known-bad three (`title`, `context_hint`). This asserts every key the
    /// gateway actually emits is a member of OC's documented schema allowlist
    /// (`OC_SESSIONS_CREATE_SCHEMA_FIELDS`, sourced from docs/spec/sync.md
    /// §22C.6.5.2) — it does not hand-pin the exact JSON shape. If a future
    /// change adds e.g. `params["priority"] = ...` to `build_oc_create_params`,
    /// this test reddens on `priority` without needing to know that name in
    /// advance, exactly as OC's own validator would reject it live.
    /// Mutation: add `params["context_hint"] = serde_json::json!(request.context_hint);`
    /// back into `build_oc_create_params` → reddens: "gateway sent field
    /// 'context_hint' which is not in OC's documented sessions.create schema".
    #[test]
    fn test_build_oc_create_params_only_emits_schema_fields() {
        let request = CreateSessionRequest {
            oc_agent_id: "main".to_string(),
            title: Some("Alarm".to_string()),
            context_hint: Some(
                "Pane scheduled job 74fba7b7-781c-4e47-9512-75d32903c46d".to_string(),
            ),
        };
        let params = build_oc_create_params(&request);
        let obj = params
            .as_object()
            .expect("build_oc_create_params must return a JSON object");
        for key in obj.keys() {
            assert!(
                OC_SESSIONS_CREATE_SCHEMA_FIELDS.contains(&key.as_str()),
                "gateway sent field '{}' which is not in OC's documented sessions.create \
                 schema (sync.md §22C.6.5.2) — OC's real validator would reject this with \
                 INVALID_REQUEST: unexpected property '{}'",
                key,
                key
            );
        }
    }

    /// Named-mismatch regression: with a title present, the params must be
    /// exactly `{"agentId": ..., "label": ...}` — no `context_hint`, no `title`
    /// key (OC has no slot for either name).
    /// Mutation: revert to the original `{"agentId", "title", "context_hint"}`
    /// shape → reddens (extra/wrong keys present, assert_eq on key set fails).
    #[test]
    fn test_build_oc_create_params_maps_title_to_label_drops_context_hint() {
        let request = CreateSessionRequest {
            oc_agent_id: "main".to_string(),
            title: Some("Alarm".to_string()),
            context_hint: Some(
                "Pane scheduled job 74fba7b7-781c-4e47-9512-75d32903c46d".to_string(),
            ),
        };
        let params = build_oc_create_params(&request);
        assert_eq!(
            params,
            serde_json::json!({ "agentId": "main", "label": "Alarm" })
        );
        assert!(params.get("context_hint").is_none());
        assert!(params.get("title").is_none());
    }

    /// With no title, `label` must be absent entirely, not present-as-null —
    /// OC's schema marks `label` optional, and sending an explicit null could
    /// trip a stricter validator depending on how OC treats null vs. absent.
    /// Mutation: build params unconditionally via
    /// `serde_json::json!({"agentId": ..., "label": request.title})` (no `if let`)
    /// → reddens (label key present with a null value instead of absent).
    #[test]
    fn test_build_oc_create_params_omits_label_when_title_absent() {
        let request = CreateSessionRequest {
            oc_agent_id: "main".to_string(),
            title: None,
            context_hint: None,
        };
        let params = build_oc_create_params(&request);
        assert_eq!(params, serde_json::json!({ "agentId": "main" }));
        assert!(params.get("label").is_none());
    }

    #[test]
    fn test_create_prefers_key_over_session_id() {
        // Mock OC response with both key and sessionId present. Per OC's model,
        // "key" is the stable identifier (sessionId can rotate under the same key
        // on out-of-turn sends), so create() must store "key", not "sessionId".
        let oc_response = serde_json::json!({
            "ok": true,
            "sessionId": "oc-abc-123",
            "key": "agent:main:openai-user:pane-abc-123",
            "entry": {},
            "runStarted": false
        });

        let oc_session_key = oc_response
            .get("key")
            .or_else(|| oc_response.get("sessionId"))
            .and_then(|v| v.as_str())
            .unwrap();

        assert_eq!(oc_session_key, "agent:main:openai-user:pane-abc-123");
    }

    #[test]
    fn test_create_falls_back_to_session_id_when_key_missing() {
        // Mock OC response with only sessionId (no key) — unexpected shape, but
        // create() should still not crash; fall back rather than error.
        let oc_response = serde_json::json!({
            "ok": true,
            "sessionId": "oc-fallback-456",
            "entry": {},
            "runStarted": false
        });

        let oc_session_key = oc_response
            .get("key")
            .or_else(|| oc_response.get("sessionId"))
            .and_then(|v| v.as_str())
            .unwrap();

        assert_eq!(oc_session_key, "oc-fallback-456");
    }

    #[test]
    fn test_send_reads_message_seq_as_oc_message_id() {
        // Mock OC response with messageSeq (number)
        let oc_response = serde_json::json!({
            "ok": true,
            "messageSeq": 5
        });

        // Extract messageSeq and convert to string
        let oc_message_id = oc_response
            .get("messageSeq")
            .and_then(|v| {
                if let Some(num) = v.as_u64() {
                    Some(num.to_string())
                } else if let Some(s) = v.as_str() {
                    Some(s.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        assert_eq!(oc_message_id, "5");
    }

    #[test]
    fn test_send_handles_missing_message_seq() {
        // Mock OC response with missing messageSeq
        let oc_response = serde_json::json!({
            "ok": true
        });

        // Extract messageSeq with UUID fallback
        let oc_message_id = oc_response
            .get("messageSeq")
            .and_then(|v| {
                if let Some(num) = v.as_u64() {
                    Some(num.to_string())
                } else if let Some(s) = v.as_str() {
                    Some(s.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "fallback-uuid".to_string());

        assert_eq!(oc_message_id, "fallback-uuid");
    }

    #[test]
    fn test_list_remaps_camel_case_to_session_info() {
        use chrono::DateTime;

        // Mock OC response with camelCase sessions
        let oc_response = serde_json::json!({
            "sessions": [
                {
                    "sessionId": "abc-123",
                    "displayName": "My Session",
                    "agentId": "main",
                    "createdAt": "2026-05-02T12:00:00Z",
                    "updatedAt": "2026-05-02T13:00:00Z"
                },
                {
                    "sessionId": "def-456",
                    "displayName": null,
                    "agentId": "coder",
                    "createdAt": "2026-05-02T14:00:00Z",
                    "updatedAt": null
                }
            ]
        });

        // Map camelCase to SessionInfo (snake_case)
        let sessions_array = oc_response.get("sessions").unwrap().as_array().unwrap();
        let sessions: Vec<SessionInfo> = sessions_array
            .iter()
            .filter_map(|s| {
                let session_id = s.get("sessionId")?.as_str()?.to_string();
                let title = s.get("displayName").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let oc_agent_id = s.get("agentId")?.as_str()?.to_string();
                let created_at = s.get("createdAt")?.as_str()?;
                let created_at = DateTime::parse_from_rfc3339(created_at).ok()?.with_timezone(&Utc);
                let last_message_at = s.get("updatedAt")
                    .and_then(|v| v.as_str())
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc));
                
                Some(SessionInfo {
                    session_id,
                    title,
                    oc_agent_id,
                    created_at,
                    last_message_at,
                })
            })
            .collect();

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].session_id, "abc-123");
        assert_eq!(sessions[0].title, "My Session");
        assert_eq!(sessions[0].oc_agent_id, "main");
        assert!(sessions[0].last_message_at.is_some());

        assert_eq!(sessions[1].session_id, "def-456");
        assert_eq!(sessions[1].title, ""); // null displayName → empty string
        assert_eq!(sessions[1].oc_agent_id, "coder");
        assert!(sessions[1].last_message_at.is_none());
    }

    #[test]
    fn test_delete_sends_key_param() {
        let oc_id = "oc-session-123";
        
        // Construct params as gateway does
        let params = serde_json::json!({
            "key": oc_id,
        });

        // Verify key field exists (not session_id)
        assert!(params.get("key").is_some());
        assert_eq!(params.get("key").unwrap().as_str().unwrap(), "oc-session-123");
        assert!(params.get("session_id").is_none());
    }

    #[test]
    fn test_abort_sends_key_param() {
        let oc_id = "oc-session-456";
        
        // Construct params as gateway does
        let params = serde_json::json!({
            "key": oc_id,
        });

        // Verify key field exists (not session_id)
        assert!(params.get("key").is_some());
        assert_eq!(params.get("key").unwrap().as_str().unwrap(), "oc-session-456");
        assert!(params.get("session_id").is_none());
    }

    #[test]
    fn test_messages_remaps_pagination_casing() {
        // Mock OC response with camelCase pagination fields
        let oc_response = serde_json::json!({
            "messages": [
                {"id": "msg1", "content": "Hello"},
                {"id": "msg2", "content": "World"}
            ],
            "hasMore": true,
            "nextCursor": "cursor-abc-123"
        });

        // Extract pagination fields as gateway does (OC camelCase → gateway snake_case)
        let has_more = oc_response
            .get("hasMore")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let next_cursor = oc_response
            .get("nextCursor")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        assert_eq!(has_more, true);
        assert_eq!(next_cursor, Some("cursor-abc-123".to_string()));

        // Verify snake_case fields don't exist in OC response
        assert!(oc_response.get("has_more").is_none());
        assert!(oc_response.get("next_cursor").is_none());
    }

    #[test]
    fn test_messages_sends_session_key_param_not_key() {
        // Regression test for the gateway->OC chat.history 502 bug.
        // OC's ChatHistoryParamsSchema requires "sessionKey" (NonEmptyString) and
        // declares additionalProperties: false with no "before"/"since" fields at
        // all, so the params object built by the messages handler must use
        // "sessionKey" (not "key") and must NOT include "before"/"since" keys —
        // not even as JSON null.
        let oc_session_key = "agent:main:openai-user:pane-e9716b41-f159-4823-9238-f21596adaf29";
        let limit: usize = 50;

        // Construct params exactly as the messages() handler does.
        let params = serde_json::json!({
            "sessionKey": oc_session_key,
            "limit": limit,
        });

        // Correct field present with correct value.
        assert_eq!(
            params.get("sessionKey").and_then(|v| v.as_str()),
            Some(oc_session_key)
        );
        assert_eq!(params.get("limit").and_then(|v| v.as_u64()), Some(50));

        // Old broken field name must not be present.
        assert!(params.get("key").is_none(), "params must not use the old 'key' field name");

        // before/since must be entirely absent — not even present as JSON null —
        // since OC's schema has additionalProperties: false and no before/since
        // fields defined at all.
        assert!(params.get("before").is_none(), "before must be omitted, not sent as null");
        assert!(params.get("since").is_none(), "since must be omitted, not sent as null");

        // Sanity: the object has exactly the two expected keys.
        let obj = params.as_object().unwrap();
        assert_eq!(obj.len(), 2, "params should contain only sessionKey and limit");
    }

    #[test]
    fn test_messages_uses_stored_key_from_session_mappings() {
        // session_mappings now stores the OC session KEY (set by create(), which
        // prefers "key" over "sessionId" — see test_create_prefers_key_over_session_id).
        // The messages() handler must read that stored value verbatim as sessionKey,
        // not re-derive or mutate it.
        let mappings = std::collections::HashMap::from([(
            "gw-session-1".to_string(),
            "agent:main:openai-user:pane-gw-session-1".to_string(),
        )]);

        let session_id = "gw-session-1".to_string();
        let oc_session_key = mappings.get(&session_id).cloned().unwrap_or_else(|| session_id.clone());

        let params = serde_json::json!({
            "sessionKey": oc_session_key,
            "limit": 50usize,
        });

        assert_eq!(
            params.get("sessionKey").and_then(|v| v.as_str()),
            Some("agent:main:openai-user:pane-gw-session-1")
        );
    }
}
