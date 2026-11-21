use crate::error::GatewayError;
use crate::oc_client::OcClient;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Agent information
#[derive(Debug, Clone, Serialize)]
pub struct AgentInfo {
    pub oc_agent_id: String,
    pub name: String,
    pub model: Option<String>,
    pub role: Option<String>,
    /// Workspace path from OC's agents.list response (ADR-0040)
    pub workspace: Option<String>,
    /// OC's per-agent `isDefault` flag (ADR-0067 D5, spec `sync.md` §22C.6.2 tier 1).
    /// Deserialized from `agents.list` and forwarded on `/v1/sync/initial` so tier 1 of
    /// the three-step default-resolution order can fire. On OC 2026.8.2 every agent
    /// reports `false` (§22C.10 capture) — wiring is correct-but-dormant today.
    pub is_default: bool,
    pub registered_at: DateTime<Utc>,
}

/// Agent registry response
#[derive(Debug, Serialize)]
pub struct AgentsResponse {
    pub agents: Vec<AgentInfo>,
}

/// OC agents.list response
#[derive(Debug, Deserialize)]
struct OcAgentListResponse {
    agents: Vec<OcAgent>,
}

#[derive(Debug, Deserialize)]
struct OcAgent {
    id: String,
    name: Option<String>,
    #[serde(default)]
    model: Option<serde_json::Value>,
    #[serde(default)]
    role: Option<String>,
    /// Workspace path from OC (ADR-0040 §22C.6.4)
    #[serde(default)]
    workspace: Option<String>,
    /// Per-agent default flag (ADR-0067 D5). Not deserialized prior to this fix —
    /// `#[serde(default)]` keeps deserialization tolerant of OC versions that omit it.
    #[serde(default, rename = "isDefault")]
    is_default: bool,
}

/// Agent registry cache
pub struct AgentRegistry {
    /// Cache: (tenant_id, oc_agent_id) -> AgentInfo
    cache: Arc<RwLock<HashMap<(String, String), AgentInfo>>>,
    oc_client: Arc<OcClient>,
}

impl AgentRegistry {
    pub fn new(oc_client: Arc<OcClient>) -> Self {
        eprintln!("[AGENT_REGISTRY] Creating agent registry");
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            oc_client,
        }
    }

    /// Get agents for a tenant, refreshing cache if needed
    pub async fn get_agents(&self, tenant_id: &str, force_refresh: bool) -> Result<Vec<AgentInfo>, GatewayError> {
        eprintln!("[AGENT_REGISTRY] Getting agents for tenant: {}", tenant_id);

        // Check cache first
        if !force_refresh {
            let cache = self.cache.read().await;
            let cached_agents: Vec<AgentInfo> = cache
                .iter()
                .filter(|((tid, _), _)| tid == tenant_id)
                .map(|(_, info)| info.clone())
                .collect();

            if !cached_agents.is_empty() {
                eprintln!("[AGENT_REGISTRY] Returning {} cached agents", cached_agents.len());
                return Ok(cached_agents);
            }
        }

        // Fetch from OC
        eprintln!("[AGENT_REGISTRY] Fetching agents from OC");
        let result = self.oc_client.call("agents.list", serde_json::json!({})).await?;

        // Parse response
        let oc_response: OcAgentListResponse = serde_json::from_value(result)
            .map_err(|e| GatewayError::OcClient(format!("Failed to parse agents.list response: {}", e)))?;

        let now = Utc::now();
        let mut agents = Vec::new();
        let mut cache = self.cache.write().await;

        for oc_agent in oc_response.agents {
            // Extract model string from Value (if it's an object with "primary" field)
            let model_str = oc_agent.model.as_ref().and_then(|v| {
                if let Some(obj) = v.as_object() {
                    obj.get("primary").and_then(|p| p.as_str()).map(|s| s.to_string())
                } else {
                    v.as_str().map(|s| s.to_string())
                }
            });

            let agent_info = AgentInfo {
                oc_agent_id: oc_agent.id.clone(),
                name: oc_agent.name.clone().unwrap_or_else(|| oc_agent.id.clone()),
                model: model_str,
                role: oc_agent.role,
                // ADR-0040: store workspace from agents.list; filter empty strings
                workspace: oc_agent.workspace.filter(|w| !w.trim().is_empty()),
                // ADR-0067 D5: forward OC's per-agent isDefault flag (tier 1 input).
                is_default: oc_agent.is_default,
                registered_at: now,
            };

            // Update cache
            cache.insert(
                (tenant_id.to_string(), oc_agent.id),
                agent_info.clone(),
            );

            agents.push(agent_info);
        }

        eprintln!("[AGENT_REGISTRY] Cached {} agents for tenant: {}", agents.len(), tenant_id);
        Ok(agents)
    }

    /// Refresh cache for a tenant
    pub async fn refresh(&self, tenant_id: &str) -> Result<Vec<AgentInfo>, GatewayError> {
        self.get_agents(tenant_id, true).await
    }

    /// Pre-populate the agent cache for a tenant with a known set of agents.
    ///
    /// This is a test-seam method used by integration tests to inject agent data without
    /// requiring a live OC WebSocket connection. Gated behind `cfg(test)` or the
    /// `test-utils` feature — not a production API. Production builds cannot reach this
    /// cache-poisoning seam.
    ///
    /// Additional test surface cited in PR #1323 description: this method is the only
    /// new public API introduced by the fix.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn seed_agents(&self, tenant_id: &str, agents: Vec<AgentInfo>) {
        let mut cache = self.cache.write().await;
        for agent in agents {
            cache.insert((tenant_id.to_string(), agent.oc_agent_id.clone()), agent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_registry_creation() {
        let oc_client = Arc::new(OcClient::new(
            "ws://localhost:8080".to_string(),
            "test_token".to_string(),
        ).expect("Failed to create OC client"));
        
        let _registry = AgentRegistry::new(oc_client);
        // Just verify it creates successfully
    }

    #[tokio::test]
    async fn test_empty_cache_triggers_fetch() {
        let oc_client = Arc::new(OcClient::new(
            "ws://localhost:8080".to_string(),
            "test_token".to_string(),
        ).expect("Failed to create OC client"));
        
        let registry = AgentRegistry::new(oc_client);
        
        // This will fail because there's no actual OC server,
        // but it verifies the code path that attempts to fetch
        let result = registry.get_agents("default", false).await;
        assert!(result.is_err());
    }

    /// Fixture source of truth: `docs/spec/sync.md` §22C.10 "ADR-0023 capture —
    /// `agents.list`, `isDefault` field (2026-09-21)". OC v2026.8.2, live gateway probe +
    /// `openclaw agents list`, captured 2026-09-21. Versioning: unstable; re-verify on OC
    /// version bumps (ADR-0023 discipline — do not hand-edit this fixture from a guess;
    /// re-capture against a live gateway and update the spec citation above first).
    ///
    /// This replaces a prior fixture (stale as of ADR-0067, four OC versions out of date)
    /// that asserted a top-level `"defaultId": "main"` field OC does not send, and that
    /// was missing the fifth agent (`support`). Per the §22C.10 capture: five agents, each
    /// carrying a per-agent `isDefault` boolean (all five report `false` on the live
    /// gateway; `OcAgent` does not deserialize `isDefault` today — it is not consumed for
    /// any gateway decision — so it is present in the fixture JSON for capture fidelity
    /// but simply ignored by `serde`, same as `identity`, `mainKey`, and `scope`). No
    /// top-level `defaultId` is present in the real response; this fixture must not
    /// assert one. See ADR-0067 D5 / `docs/spec/sync.md` §22C.6.2 "Default agent
    /// resolution" for how default-agent status is actually resolved (not from this RPC
    /// shape at all).
    #[test]
    fn test_oc_agent_deserialization() {
        let oc_response_json = serde_json::json!({
            "mainKey": "main",
            "scope": "per-sender",
            "agents": [
                {
                    "id": "main",
                    "name": null,
                    "isDefault": false,
                    "identity": {"name": "Brit", "emoji": "🧠"},
                    "workspace": "/path/to/workspace",
                    "model": {"primary": "anthropic/claude-sonnet-4-5", "thinking": "low"}
                },
                {
                    "id": "researcher",
                    "name": "Reed",
                    "isDefault": false,
                    "identity": {"name": "Reed", "emoji": "🔍"},
                    "workspace": "/path/to/workspace-researcher",
                    "model": {"primary": "anthropic/claude-sonnet-4-5", "thinking": "low"}
                },
                {
                    "id": "coder",
                    "name": "Stia",
                    "isDefault": false,
                    "identity": {"name": "Stia", "emoji": "💻"},
                    "workspace": "/path/to/workspace-coder",
                    "model": {"primary": "anthropic/claude-sonnet-4-5", "thinking": "low"}
                },
                {
                    "id": "designer",
                    "name": "Lane",
                    "isDefault": false,
                    "identity": {"name": "Lane", "emoji": "🎨"},
                    "workspace": "/path/to/workspace-designer",
                    "model": {"primary": "anthropic/claude-sonnet-4-5", "thinking": "low"}
                },
                {
                    "id": "support",
                    "name": null,
                    "isDefault": false,
                    "identity": {"name": "Support", "emoji": "🛟"},
                    "workspace": "/path/to/workspace-support",
                    "model": {"primary": "anthropic/claude-sonnet-4-5", "thinking": "low"}
                }
            ]
        });

        // Deserialize into OcAgentListResponse
        let oc_response: OcAgentListResponse = serde_json::from_value(oc_response_json)
            .expect("Failed to deserialize OC agents.list response");

        assert_eq!(oc_response.agents.len(), 5, "Should have 5 agents (§22C.10 capture)");

        // Test agent 1: main with null name
        let main_agent = &oc_response.agents[0];
        assert_eq!(main_agent.id, "main");
        assert_eq!(main_agent.name, None, "main agent should have null name");
        assert!(main_agent.model.is_some(), "main agent should have model object");

        // Test agent 2: researcher with non-null name
        let researcher_agent = &oc_response.agents[1];
        assert_eq!(researcher_agent.id, "researcher");
        assert_eq!(researcher_agent.name, Some("Reed".to_string()));

        // Test agent 3: coder
        let coder_agent = &oc_response.agents[2];
        assert_eq!(coder_agent.id, "coder");
        assert_eq!(coder_agent.name, Some("Stia".to_string()));

        // Test agent 4: designer
        let designer_agent = &oc_response.agents[3];
        assert_eq!(designer_agent.id, "designer");
        assert_eq!(designer_agent.name, Some("Lane".to_string()));

        // Test agent 5: support (the previously-missing fifth agent, §22C.10 capture)
        let support_agent = &oc_response.agents[4];
        assert_eq!(support_agent.id, "support");
        assert_eq!(support_agent.name, None, "support agent should have null name");
        assert!(support_agent.model.is_some(), "support agent should have model object");
    }

    /// §22C.10 capture: the live gateway's `agents.list` response has no top-level
    /// `defaultId` field at all. `OcAgentListResponse` must keep deserializing
    /// successfully from a response shaped exactly like the real one — i.e. it must
    /// NOT require a `defaultId` key. This is the direct regression test for the stale
    /// fixture ADR-0067 replaced: the old fixture's presence of `defaultId` masked the
    /// fact that no code ever depended on it, and that a `defaultId`-less response
    /// (the real shape) works identically.
    #[test]
    fn test_oc_agent_list_response_deserializes_without_top_level_default_id() {
        let oc_response_json = serde_json::json!({
            "mainKey": "main",
            "scope": "per-sender",
            "agents": [
                {"id": "main", "name": null, "isDefault": false, "model": {"primary": "anthropic/claude-sonnet-4-5"}}
            ]
            // No "defaultId" key anywhere — matches the real OC 2026.8.2 shape.
        });

        let oc_response: OcAgentListResponse = serde_json::from_value(oc_response_json)
            .expect("OcAgentListResponse must deserialize a response with no top-level defaultId");

        assert_eq!(oc_response.agents.len(), 1);
        assert_eq!(oc_response.agents[0].id, "main");
    }

    #[test]
    fn test_agent_info_mapping() {
        // Fixture: Real OC agents.list response
        let oc_response_json = serde_json::json!({
            "agents": [
                {
                    "id": "main",
                    "name": null,
                    "model": {"primary": "anthropic/claude-sonnet-4-5"},
                    "identity": {},
                    "workspace": "/path"
                },
                {
                    "id": "researcher",
                    "name": "Reed",
                    "model": {"primary": "openai/gpt-4"},
                    "identity": {},
                    "workspace": "/path"
                }
            ]
        });

        let oc_response: OcAgentListResponse = serde_json::from_value(oc_response_json)
            .expect("Failed to deserialize");

        let now = Utc::now();

        // Map to AgentInfo (simulating get_agents logic)
        let mut agents = Vec::new();
        for oc_agent in oc_response.agents {
            // Extract model string from Value
            let model_str = oc_agent.model.as_ref().and_then(|v| {
                if let Some(obj) = v.as_object() {
                    obj.get("primary").and_then(|p| p.as_str()).map(|s| s.to_string())
                } else {
                    v.as_str().map(|s| s.to_string())
                }
            });

            let agent_info = AgentInfo {
                oc_agent_id: oc_agent.id.clone(),
                name: oc_agent.name.clone().unwrap_or_else(|| oc_agent.id.clone()),
                model: model_str,
                role: oc_agent.role,
                workspace: oc_agent.workspace.filter(|w| !w.trim().is_empty()),
                is_default: oc_agent.is_default,
                registered_at: now,
            };

            agents.push(agent_info);
        }

        // Verify main agent: null name falls back to id
        assert_eq!(agents[0].oc_agent_id, "main");
        assert_eq!(agents[0].name, "main", "null name should fall back to id");
        assert_eq!(agents[0].model, Some("anthropic/claude-sonnet-4-5".to_string()));
        assert_eq!(agents[0].workspace, Some("/path".to_string()));

        // Verify researcher agent: name preserved
        assert_eq!(agents[1].oc_agent_id, "researcher");
        assert_eq!(agents[1].name, "Reed");
        assert_eq!(agents[1].model, Some("openai/gpt-4".to_string()));
        assert_eq!(agents[1].workspace, Some("/path".to_string()));
    }

    #[test]
    fn test_workspace_field_populated_from_oc_response() {
        // ADR-0040: workspace field must be captured from agents.list response
        let oc_response_json = serde_json::json!({
            "agents": [
                {
                    "id": "main",
                    "name": "Brit",
                    "workspace": "/Users/owner/.openclaw/workspace",
                    "model": {"primary": "anthropic/claude-sonnet-4-5"}
                },
                {
                    "id": "coder",
                    "name": "Stia",
                    "workspace": "/Users/owner/.openclaw/workspace-coder",
                    "model": {"primary": "anthropic/claude-sonnet-4-5"}
                }
            ]
        });

        let oc_response: OcAgentListResponse = serde_json::from_value(oc_response_json)
            .expect("Failed to deserialize");

        let now = Utc::now();
        let mut agents = Vec::new();
        for oc_agent in oc_response.agents {
            let model_str = oc_agent.model.as_ref().and_then(|v| {
                if let Some(obj) = v.as_object() {
                    obj.get("primary").and_then(|p| p.as_str()).map(|s| s.to_string())
                } else {
                    v.as_str().map(|s| s.to_string())
                }
            });
            agents.push(AgentInfo {
                oc_agent_id: oc_agent.id.clone(),
                name: oc_agent.name.clone().unwrap_or_else(|| oc_agent.id.clone()),
                model: model_str,
                role: oc_agent.role,
                workspace: oc_agent.workspace.filter(|w| !w.trim().is_empty()),
                is_default: oc_agent.is_default,
                registered_at: now,
            });
        }

        assert_eq!(agents[0].workspace, Some("/Users/owner/.openclaw/workspace".to_string()));
        assert_eq!(agents[1].workspace, Some("/Users/owner/.openclaw/workspace-coder".to_string()));
    }

    #[test]
    fn test_empty_workspace_filtered_to_none() {
        // ADR-0040 Decision 4: empty workspace must be treated same as missing
        let oc_response_json = serde_json::json!({
            "agents": [
                {"id": "main", "name": "Brit", "workspace": "", "model": null},
                {"id": "coder", "name": "Stia", "workspace": "   ", "model": null}
            ]
        });

        let oc_response: OcAgentListResponse = serde_json::from_value(oc_response_json)
            .expect("Failed to deserialize");

        let now = Utc::now();
        for oc_agent in oc_response.agents {
            let info = AgentInfo {
                oc_agent_id: oc_agent.id.clone(),
                name: oc_agent.name.clone().unwrap_or_else(|| oc_agent.id.clone()),
                model: None,
                role: None,
                workspace: oc_agent.workspace.filter(|w| !w.trim().is_empty()),
                is_default: oc_agent.is_default,
                registered_at: now,
            };
            assert_eq!(info.workspace, None, "empty/whitespace workspace must be None for agent {}", oc_agent.id);
        }
    }

    #[test]
    fn test_missing_workspace_field_is_none() {
        // ADR-0040: agents.list response without workspace field (older OC version)
        let oc_response_json = serde_json::json!({
            "agents": [
                {"id": "main", "name": "Brit", "model": {"primary": "anthropic/claude-sonnet-4-5"}}
            ]
        });

        let oc_response: OcAgentListResponse = serde_json::from_value(oc_response_json)
            .expect("Failed to deserialize");

        let now = Utc::now();
        let oc_agent = &oc_response.agents[0];
        let info = AgentInfo {
            oc_agent_id: oc_agent.id.clone(),
            name: oc_agent.name.clone().unwrap_or_else(|| oc_agent.id.clone()),
            model: None,
            role: None,
            workspace: oc_agent.workspace.clone().filter(|w| !w.trim().is_empty()),
            is_default: oc_agent.is_default,
            registered_at: now,
        };
        assert_eq!(info.workspace, None, "missing workspace field must be None");

        assert_eq!(info.is_default, false, "isDefault absent from payload must default to false");
    }

    /// ADR-0067 D5: `OcAgent`/`AgentInfo` must actually deserialize+forward `isDefault`.
    /// Prior to this fix, the field was present in fixture JSON but silently ignored by
    /// serde (§22C.10 capture comment on `test_oc_agent_deserialization` above) — this
    /// test is the direct regression guard for that gap, and asserts both the `true` and
    /// `false` cases map through `AgentInfo.is_default` correctly.
    #[test]
    fn test_is_default_deserializes_and_forwards_to_agent_info() {
        let oc_response_json = serde_json::json!({
            "agents": [
                {"id": "main", "name": "Brit", "isDefault": true, "model": {"primary": "anthropic/claude-sonnet-4-5"}},
                {"id": "coder", "name": "Stia", "isDefault": false, "model": {"primary": "anthropic/claude-sonnet-4-5"}}
            ]
        });

        let oc_response: OcAgentListResponse = serde_json::from_value(oc_response_json)
            .expect("Failed to deserialize");

        assert_eq!(oc_response.agents[0].is_default, true, "main agent's isDefault:true must deserialize to true");
        assert_eq!(oc_response.agents[1].is_default, false, "coder agent's isDefault:false must deserialize to false");

        let now = Utc::now();
        let mapped: Vec<AgentInfo> = oc_response
            .agents
            .into_iter()
            .map(|oc_agent| AgentInfo {
                oc_agent_id: oc_agent.id.clone(),
                name: oc_agent.name.clone().unwrap_or_else(|| oc_agent.id.clone()),
                model: None,
                role: None,
                workspace: None,
                is_default: oc_agent.is_default,
                registered_at: now,
            })
            .collect();

        assert_eq!(mapped[0].is_default, true, "AgentInfo must forward is_default: true");
        assert_eq!(mapped[1].is_default, false, "AgentInfo must forward is_default: false");
    }

    /// ADR-0067 D5 / §22C.10: OC 2026.8.2's live shape — every agent reports `isDefault:
    /// false`. Tier 1 of default resolution is therefore correct-but-dormant today; this
    /// test documents that this specific, real-world payload deserializes cleanly and
    /// every mapped agent is `is_default == false`, matching the live gateway capture.
    #[test]
    fn test_is_default_all_false_matches_live_gateway_capture() {
        let oc_response_json = serde_json::json!({
            "agents": [
                {"id": "main", "name": null, "isDefault": false},
                {"id": "researcher", "name": "Reed", "isDefault": false},
                {"id": "coder", "name": "Stia", "isDefault": false},
                {"id": "designer", "name": "Lane", "isDefault": false},
                {"id": "support", "name": null, "isDefault": false}
            ]
        });

        let oc_response: OcAgentListResponse = serde_json::from_value(oc_response_json)
            .expect("Failed to deserialize");

        assert_eq!(oc_response.agents.len(), 5);
        assert!(
            oc_response.agents.iter().all(|a| !a.is_default),
            "every agent must report is_default == false, matching the live OC 2026.8.2 capture"
        );
    }
}
