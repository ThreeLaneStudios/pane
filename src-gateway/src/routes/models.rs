use crate::auth::TenantContext;
use crate::error::GatewayError;
use axum::{extract::{Query, State}, Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Model information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Vec<String>>,
}

/// Models response
#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub models: Vec<ModelInfo>,
    pub source: String,
    pub refreshed_at: String,
}

/// OC models.list response
#[derive(Debug, Deserialize)]
struct OcModelsResponse {
    models: Vec<OcModel>,
}

#[derive(Debug, Deserialize)]
struct OcModel {
    id: String,
    name: String,
    provider: String,
    #[serde(rename = "contextWindow")]
    context_window: Option<u64>,
    reasoning: Option<bool>,
    input: Option<Vec<String>>,
}

/// Query parameters
#[derive(Debug, Deserialize)]
pub struct ModelsQuery {
    #[serde(default)]
    refresh: u8,
}

/// Models cache
pub struct ModelsCache {
    models: Arc<RwLock<Option<Vec<ModelInfo>>>>,
    last_refresh: Arc<RwLock<Option<DateTime<Utc>>>>,
}

impl ModelsCache {
    pub fn new() -> Self {
        Self {
            models: Arc::new(RwLock::new(None)),
            last_refresh: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn is_stale(&self) -> bool {
        let last_refresh = self.last_refresh.read().await;
        match *last_refresh {
            None => true,
            Some(timestamp) => {
                let age = Utc::now() - timestamp;
                age.num_seconds() > 30
            }
        }
    }

    pub async fn get_cached(&self) -> Option<(Vec<ModelInfo>, DateTime<Utc>)> {
        let models = self.models.read().await;
        let last_refresh = self.last_refresh.read().await;

        match (&*models, &*last_refresh) {
            (Some(m), Some(t)) => Some((m.clone(), *t)),
            _ => None,
        }
    }

    pub async fn update(&self, models: Vec<ModelInfo>) {
        let now = Utc::now();
        *self.models.write().await = Some(models);
        *self.last_refresh.write().await = Some(now);
    }
}

/// GET /v1/models handler
pub async fn get_models(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(_tenant): Extension<TenantContext>,
    Query(query): Query<ModelsQuery>,
) -> Result<Json<ModelsResponse>, GatewayError> {
    eprintln!("[MODELS] Models endpoint called (refresh={})", query.refresh);

    let force_refresh = query.refresh == 1;

    // Check cache
    if !force_refresh {
        if let Some((cached_models, refreshed_at)) = state.models_cache.get_cached().await {
            if !state.models_cache.is_stale().await {
                eprintln!("[MODELS] Returning {} cached models", cached_models.len());
                return Ok(Json(ModelsResponse {
                    models: cached_models,
                    source: "openclaw".to_string(),
                    refreshed_at: refreshed_at.to_rfc3339(),
                }));
            }
        }
    }

    // Fetch from OC
    eprintln!("[MODELS] Fetching models from OC");
    let result = state.oc_client.call("models.list", serde_json::json!({})).await?;

    // Parse response
    let oc_response: OcModelsResponse = serde_json::from_value(result)
        .map_err(|e| GatewayError::OcClient(format!("Failed to parse models.list response: {}", e)))?;

    let models: Vec<ModelInfo> = oc_response
        .models
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            name: m.name,
            provider: m.provider,
            context_window: m.context_window,
            reasoning: m.reasoning,
            input: m.input,
        })
        .collect();

    let now = Utc::now();
    state.models_cache.update(models.clone()).await;

    eprintln!("[MODELS] Cached {} models", models.len());

    Ok(Json(ModelsResponse {
        models,
        source: "openclaw".to_string(),
        refreshed_at: now.to_rfc3339(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_models_cache_creation() {
        let cache = ModelsCache::new();
        assert!(cache.is_stale().await);
    }

    #[tokio::test]
    async fn test_models_cache_stale() {
        let cache = ModelsCache::new();
        
        // Empty cache is stale
        assert!(cache.is_stale().await);
        
        // Fresh cache is not stale
        cache.update(vec![]).await;
        assert!(!cache.is_stale().await);
    }

    #[tokio::test]
    async fn test_models_cache_get_empty() {
        let cache = ModelsCache::new();
        assert!(cache.get_cached().await.is_none());
    }

    #[tokio::test]
    async fn test_models_cache_update_and_get() {
        let cache = ModelsCache::new();
        
        let models = vec![ModelInfo {
            id: "test-model".to_string(),
            name: "Test Model".to_string(),
            provider: "test".to_string(),
            context_window: None,
            reasoning: None,
            input: None,
        }];
        
        cache.update(models.clone()).await;
        
        let cached = cache.get_cached().await;
        assert!(cached.is_some());
        
        let (cached_models, _) = cached.unwrap();
        assert_eq!(cached_models.len(), 1);
        assert_eq!(cached_models[0].id, "test-model");
    }
}
