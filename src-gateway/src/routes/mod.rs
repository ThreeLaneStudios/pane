pub mod chat;
pub mod health;
pub mod models;
pub mod pair;
pub mod sessions;
pub mod stream;
pub mod sync;

use crate::agents::AgentsResponse;
use crate::auth::TenantContext;
use crate::error::GatewayError;
use axum::{extract::State, Extension, Json};
use axum::response::IntoResponse;
use axum::http::header;
use std::sync::Arc;

/// GET /v1/agents handler
pub async fn get_agents(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
) -> Result<Json<AgentsResponse>, GatewayError> {
    tracing::info!(tenant_id = %tenant.tenant_id, "Agents endpoint called");
    
    let agents = state.agent_registry.get_agents(&tenant.tenant_id, false).await?;
    
    Ok(Json(AgentsResponse { agents }))
}

/// GET /v1/metrics handler (Prometheus format)
pub async fn metrics_endpoint(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
) -> impl IntoResponse {
    tracing::info!(tenant_id = %tenant.tenant_id, "Metrics endpoint called");
    let prometheus_text = state.metrics.export_prometheus();
    ([(header::CONTENT_TYPE, "text/plain; version=0.0.4")], prometheus_text)
}
