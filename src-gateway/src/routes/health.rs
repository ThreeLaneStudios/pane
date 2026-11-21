use crate::auth::TenantContext;
use axum::{extract::State, http::HeaderMap, Extension, Json};
use serde::Serialize;
use std::sync::Arc;
use time::OffsetDateTime;

/// Full health check response (authenticated)
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub uptime: u64,
    /// SHA-256 of the SPKI DER of the TLS certificate (HPKP-style pin, RFC 7469 §2.4), hex-encoded.
    /// Used by the Pane app's PinnedCertVerifier; also displayed to the user during pairing.
    pub tls_fingerprint: String,
    pub tls_cert_pem: String,
    pub oc_connection_status: String,
    pub gateway_version: String,
    pub gateway_hostname: Option<String>,
}

/// Partial health check response (unauthenticated)
#[derive(Debug, Serialize)]
pub struct PartialHealthResponse {
    pub status: String,
    pub uptime: u64,
    /// SHA-256 of the SPKI DER of the TLS certificate (HPKP-style pin, RFC 7469 §2.4), hex-encoded.
    /// Used by the Pane app's PinnedCertVerifier; also displayed to the user during pairing.
    pub tls_fingerprint: String,
    pub tls_cert_pem: String,
    pub gateway_hostname: Option<String>,
}

/// GET /v1/health handler (partial response if no auth, full if authenticated)
pub async fn health_check(
    State(state): State<Arc<crate::server::SharedState>>,
    headers: HeaderMap,
    tenant: Option<Extension<TenantContext>>,
) -> Json<serde_json::Value> {
    let now = OffsetDateTime::now_utc();
    let uptime = (now - state.start_time).whole_seconds() as u64;

    // Check for optional bearer token in headers (when not going through auth middleware)
    let is_authenticated = if tenant.is_some() {
        true
    } else {
        // Try to manually verify the token
        if let Some(auth_header) = headers.get("authorization") {
            if let Ok(auth_str) = auth_header.to_str() {
                if auth_str.starts_with("Bearer ") {
                    let token = &auth_str[7..];
                    state.auth_state.verify_token(token).await.is_ok()
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        }
    };

    eprintln!("[HEALTH] Health check requested (authenticated: {})", is_authenticated);

    if is_authenticated {
        // Full response for authenticated requests
        let oc_state = state.oc_client.get_state().await;
        
        Json(serde_json::json!(HealthResponse {
            status: "healthy".to_string(),
            uptime,
            tls_fingerprint: state.tls_fingerprint.clone(),
            tls_cert_pem: state.tls_cert_pem.clone(),
            oc_connection_status: oc_state.as_str().to_string(),
            gateway_version: env!("CARGO_PKG_VERSION").to_string(),
            gateway_hostname: state.tls_hostname.clone(),
        }))
    } else {
        // Partial response for unauthenticated requests
        Json(serde_json::json!(PartialHealthResponse {
            status: "healthy".to_string(),
            uptime,
            tls_fingerprint: state.tls_fingerprint.clone(),
            tls_cert_pem: state.tls_cert_pem.clone(),
            gateway_hostname: state.tls_hostname.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentRegistry;
    use crate::auth::AuthState;
    use crate::oc_client::OcClient;
    use crate::routes::models::ModelsCache;
    use crate::server::SharedState;
    use axum::extract::State;

    #[tokio::test]
    async fn test_health_check_unauthenticated() {
        let oc_client = Arc::new(OcClient::new(
            "ws://localhost:8080".to_string(),
            "test_token".to_string(),
        ).expect("Failed to create OC client"));
        let agent_registry = Arc::new(AgentRegistry::new(oc_client.clone()));
        let tokens_path = std::env::temp_dir().join("test-tokens-health-unauth.json");
        let auth_state = Arc::new(AuthState::new(tokens_path));
        
        let state = Arc::new(SharedState {
            start_time: OffsetDateTime::now_utc(),
            tls_fingerprint: "test_fingerprint".to_string(),
            tls_cert_pem: "test_cert_pem".to_string(),
            oc_client,
            agent_registry,
            auth_state,
            models_cache: Arc::new(ModelsCache::new()),
            sync_queue: Arc::new(crate::sync_queue::SyncQueue::new()),
            ring_buffer: Arc::new(crate::ring_buffer::RingBuffer::new()),
            idempotency_store: crate::idempotency::IdempotencyStore::new(),
            metrics: Arc::new(crate::metrics::Metrics::new()),
            session_mappings: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            oc_http_base_url: "http://localhost:8080".to_string(),
            oc_bearer_token: "test_token".to_string(),
            tls_hostname: None,
            param_compat_table: Arc::new(crate::param_compat::CompatTable::default()),
            streaming_idle_timeout_secs: 60,
        });

        let headers = HeaderMap::new();
        let response = health_check(State(state), headers, None).await;
        let value = response.0;
        
        assert_eq!(value["status"], "healthy");
        assert!(value.get("uptime").is_some());
        // Should have fingerprint for pairing flow
        assert_eq!(value["tls_fingerprint"], "test_fingerprint");
        // Should have cert PEM for client-side pinning
        assert_eq!(value["tls_cert_pem"], "test_cert_pem");
        // Should not have authenticated-only fields
        assert!(value.get("oc_connection_status").is_none());
    }

    #[tokio::test]
    async fn test_health_check_authenticated() {
        let oc_client = Arc::new(OcClient::new(
            "ws://localhost:8080".to_string(),
            "test_token".to_string(),
        ).expect("Failed to create OC client"));
        let agent_registry = Arc::new(AgentRegistry::new(oc_client.clone()));
        let tokens_path = std::env::temp_dir().join("test-tokens-health-auth.json");
        let auth_state = Arc::new(AuthState::new(tokens_path));
        
        let state = Arc::new(SharedState {
            start_time: OffsetDateTime::now_utc(),
            tls_fingerprint: "test_fingerprint".to_string(),
            tls_cert_pem: "test_cert_pem".to_string(),
            oc_client,
            agent_registry,
            auth_state,
            models_cache: Arc::new(ModelsCache::new()),
            sync_queue: Arc::new(crate::sync_queue::SyncQueue::new()),
            ring_buffer: Arc::new(crate::ring_buffer::RingBuffer::new()),
            idempotency_store: crate::idempotency::IdempotencyStore::new(),
            metrics: Arc::new(crate::metrics::Metrics::new()),
            session_mappings: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            oc_http_base_url: "http://localhost:8080".to_string(),
            oc_bearer_token: "test_token".to_string(),
            tls_hostname: None,
            param_compat_table: Arc::new(crate::param_compat::CompatTable::default()),
            streaming_idle_timeout_secs: 60,
        });

        let tenant = Extension(TenantContext {
            tenant_id: "default".to_string(),
        });

        let headers = HeaderMap::new();
        let response = health_check(State(state), headers, Some(tenant)).await;
        let value = response.0;
        
        assert_eq!(value["status"], "healthy");
        assert!(value.get("uptime").is_some());
        assert_eq!(value["tls_fingerprint"], "test_fingerprint");
        assert_eq!(value["tls_cert_pem"], "test_cert_pem");
        assert_eq!(value["oc_connection_status"], "disconnected");
        assert_eq!(value["gateway_version"], env!("CARGO_PKG_VERSION"));
    }
}
