use crate::auth::{PairRequest, PairResponse};
use crate::error::GatewayError;
use crate::server::SharedState;
use axum::{extract::State, Json};
use std::sync::Arc;

/// POST /pair handler
pub async fn pair(
    State(state): State<Arc<SharedState>>,
    Json(request): Json<PairRequest>,
) -> Result<Json<PairResponse>, GatewayError> {
    // #3106 M2: never log the submitted code value.
    eprintln!("[PAIR] Pair endpoint called");

    // Exchange code for token
    let token_info = state.auth_state.exchange_code_for_token(&request.code).await?;

    Ok(Json(PairResponse {
        token: token_info.token,
        expires_at: token_info.expires_at.to_rfc3339(),
        tenant_id: token_info.tenant_id,
        gateway_version: env!("CARGO_PKG_VERSION").to_string(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentRegistry;
    use crate::auth::AuthState;
    use crate::oc_client::OcClient;
    use crate::routes::models::ModelsCache;
    use crate::server::SharedState;
    use time::OffsetDateTime;

    #[tokio::test]
    async fn test_pair_with_valid_code() {
        let tokens_path = std::env::temp_dir().join("test-tokens-pair-valid.json");
        let auth_state = Arc::new(AuthState::new(tokens_path));
        let code = auth_state.generate_pairing_code().await;
        
        let oc_client = Arc::new(OcClient::new(
            "ws://localhost:8080".to_string(),
            "test_token".to_string(),
        ).expect("Failed to create OC client"));
        let agent_registry = Arc::new(AgentRegistry::new(oc_client.clone()));
        
        let state = Arc::new(SharedState {
            start_time: OffsetDateTime::now_utc(),
            tls_fingerprint: "test".to_string(),
            tls_cert_pem: "test_cert_pem".to_string(),
            auth_state,
            oc_client,
            agent_registry,
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

        let request = PairRequest { code };
        let result = pair(State(state), Json(request)).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.tenant_id, "default");
        assert_eq!(response.gateway_version, env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn test_pair_with_invalid_code() {
        let tokens_path = std::env::temp_dir().join("test-tokens-pair-invalid.json");
        let auth_state = Arc::new(AuthState::new(tokens_path));
        let oc_client = Arc::new(OcClient::new(
            "ws://localhost:8080".to_string(),
            "test_token".to_string(),
        ).expect("Failed to create OC client"));
        let agent_registry = Arc::new(AgentRegistry::new(oc_client.clone()));
        
        let state = Arc::new(SharedState {
            start_time: OffsetDateTime::now_utc(),
            tls_fingerprint: "test".to_string(),
            tls_cert_pem: "test_cert_pem".to_string(),
            auth_state,
            oc_client,
            agent_registry,
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
        
        let request = PairRequest { code: "999999".to_string() };
        let result = pair(State(state), Json(request)).await;

        assert!(result.is_err());
    }
}
