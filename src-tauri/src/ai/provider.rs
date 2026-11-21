use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// Provider identifiers retained for legacy database/string compatibility.
#[allow(dead_code)]
pub const PROVIDER_ANTHROPIC: &str = "anthropic";
#[allow(dead_code)]
pub const PROVIDER_OPENROUTER: &str = "openrouter";
#[allow(dead_code)]
pub const PROVIDER_OLLAMA: &str = "ollama";
pub const PROVIDER_OPENCLAW: &str = "openclaw";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "system", "user", "assistant", "tool"
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    /// Multimodal content blocks (text + image_url). When present, used instead of content string.
    pub content_blocks: Option<serde_json::Value>,
    /// Comma-separated attachment IDs from chat_messages_local
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_ids: Option<String>,
    /// Message ID from chat_messages_local (used for compaction filtering)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub text: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// AI Provider enum (used instead of trait objects to avoid State + async issues)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AiProvider {
    Anthropic { api_key: String },
    OpenRouter { api_key: String },
    Ollama { endpoint: String },
    OpenClaw {
        base_url: String,
        token: String,
        /// ADR-0067 D2/D5: `ai_agents.runtime_ref` for the agent this provider instance
        /// was resolved for. Carried on the enum (mirrors how `Venice` already carries
        /// `include_system_prompt`) so every OC-origin dispatch path — streaming AND
        /// non-streaming, `call_llm` included — has the wire selector available without
        /// threading a brand-new parameter through every intermediate function
        /// signature. **Never** `ai_agents.id` (Pane's local UUID) — see
        /// `openrouter::oc_wire_model()`'s doc comment for the full D2 leak-guard
        /// rationale. `#[serde(default)]` for forward/backward compatibility.
        #[serde(default)]
        runtime_ref: Option<String>,
    },
    /// ADR-0065 D1: Venice is its own variant — never a URL-override of OpenRouter.
    /// `include_system_prompt` mirrors `ai_settings.venice_system_prompt_enabled` (D3,
    /// default off), resolved at dispatch-config time in
    /// `build_provider_for_runtime_type_from_snapshot`.
    Venice { api_key: String, include_system_prompt: bool },
    None,
}

/// Model information returned by fetch_available_models
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub context_length: Option<u64>,
    /// Parameters supported by this model (from OpenRouter /api/v1/models `supported_parameters`).
    /// None = not provided by this provider's model-list endpoint.
    #[serde(default)]
    pub supported_parameters: Option<Vec<String>>,
    /// ADR-0065 D5: Venice privacy/tier label, relayed verbatim from Venice's
    /// `/models` response `data[i].model_spec.privacy` field. `None` for every
    /// other provider. The picker renders this as a small trailing tag; when
    /// absent or empty, no tag renders — Pane never substitutes "Unknown".
    #[serde(default)]
    pub tier_label: Option<String>,
    /// #2988 PR2: Venice TEE attestation capability, relayed from
    /// `data[i].model_spec.capabilities.supportsTeeAttestation`. `None` for
    /// every other provider, and `None` for Venice whenever Venice's response
    /// omits `capabilities` entirely (non-text model types) or omits this one
    /// flag within it. **`None` renders identically to `false` in the picker —
    /// absent must never be read as "supported."** Mirrors `tier_label`'s own
    /// absent-never-substitutes rule above; see `SearchableModelSelect.svelte`
    /// for the render site.
    #[serde(default)]
    pub supports_tee_attestation: Option<bool>,
    /// #2988 PR2: Venice E2EE capability, relayed from
    /// `data[i].model_spec.capabilities.supportsE2EE`. Same absent-means-not-
    /// supported rule as `supports_tee_attestation` above — `None` never
    /// renders as "supported." **Pane implements zero of Venice's E2EE
    /// handshake** (no `X-Venice-TEE-*` headers anywhere in this codebase), so
    /// even when this is `Some(true)`, the picker must render "E2EE not
    /// supported in Pane," never a "Private"/E2EE-implying badge — see
    /// `SearchableModelSelect.svelte`.
    #[serde(default)]
    pub supports_e2ee: Option<bool>,
    /// #2988 PR2: Venice function-calling (tool use) capability, relayed from
    /// `data[i].model_spec.capabilities.supportsFunctionCalling`. Same
    /// absent-means-not-supported rule. Matt's live test (2026-10-24,
    /// `e2ee-glm-5-3-p`) confirmed tools work natively on at least one E2EE
    /// model via a plain request; Venice's docs say function-calling support
    /// "depends on the model." **Not rendered as a picker tag in this PR** —
    /// carried through the struct for a future per-model tool-eligibility
    /// filter, per Matt's #2988 ruling.
    #[serde(default)]
    pub supports_function_calling: Option<bool>,
}

/// Inner data for ModelRegistry — holds both context windows and supported_parameters.
/// Keyed by "{provider}:{model_id}" throughout.
#[derive(Default)]
pub struct ModelRegistryData {
    pub context_lengths: HashMap<String, Option<u64>>,
    /// Maps model key → list of supported parameters (e.g. ["temperature", "max_tokens", ...]).
    /// Only populated for OpenRouter models (sourced from /api/v1/models `supported_parameters`).
    /// Absent = model not fetched yet (cold). Empty vec = model fetched but no params listed.
    pub supported_parameters: HashMap<String, Vec<String>>,
}

/// Per-session registry of model context windows and supported parameters, keyed by
/// "{provider}:{model_id}".
///
/// Provider string conventions (matching ModelInfo.provider from fetch_*_models):
///   "anthropic"  — set by fetch_anthropic_models
///   "openrouter" — set by fetch_openrouter_models
///   "ollama"     — set by fetch_ollama_models
///   "openclaw"   — set by fetch_openclaw_gateway_models
///
/// Keyed by (provider, model_id) — NOT model_id alone — because the same
/// model id may appear under both "openclaw" and "openrouter" with different
/// context windows (e.g. one Anthropic Claude variant served via two providers).
///
/// Managed via app.manage(); populated inside fetch_available_models on each
/// model-list load. Cold lookup (empty registry) falls through to the seed table
/// in get_context_window with a distinct log marker (#1402-C).
pub struct ModelRegistry(pub Arc<Mutex<ModelRegistryData>>);

impl ModelRegistry {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(ModelRegistryData::default())))
    }

    /// Insert or overwrite a model's context window in the registry.
    /// For backward compatibility — does not touch supported_parameters.
    #[allow(dead_code)] // Legacy registry insertion API.
    pub fn insert(&self, provider: &str, model_id: &str, context_length: Option<u64>) {
        if let Ok(mut data) = self.0.lock() {
            let key = format!("{}:{}", provider, model_id);
            data.context_lengths.insert(key, context_length);
        }
    }

    /// Insert context window AND supported_parameters for a model.
    /// Used by OpenRouter model-list fetch where `supported_parameters` is available.
    pub fn insert_with_capabilities(
        &self,
        provider: &str,
        model_id: &str,
        context_length: Option<u64>,
        supported_parameters: Option<Vec<String>>,
    ) {
        if let Ok(mut data) = self.0.lock() {
            let key = format!("{}:{}", provider, model_id);
            data.context_lengths.insert(key.clone(), context_length);
            if let Some(params) = supported_parameters {
                data.supported_parameters.insert(key, params);
            }
        }
    }

    /// Look up a model's context window.
    /// Returns `Some(context_length)` if the key exists (value may be None if provider
    /// did not supply a context window), or `None` if the key is absent.
    pub fn lookup(&self, provider: &str, model_id: &str) -> Option<Option<u64>> {
        let data = self.0.lock().ok()?;
        let key = format!("{}:{}", provider, model_id);
        data.context_lengths.get(&key).copied()
    }

    /// Returns whether the model supports the `temperature` parameter, based on
    /// live OpenRouter `supported_parameters` metadata.
    ///
    /// - `Some(true)`  — confirmed supported ("temperature" ∈ supported_parameters)
    /// - `Some(false)` — confirmed unsupported (model fetched, "temperature" absent)
    /// - `None`        — model not in registry (cold / never fetched) → callers MUST fail-safe (omit)
    ///
    /// Spec rule (#1846): Pane sends temperature only where (1) load-bearing AND
    /// (2) this returns `Some(true)`. On cold registry, omit.
    pub fn supports_temperature(&self, provider: &str, model_id: &str) -> Option<bool> {
        let data = self.0.lock().ok()?;
        let key = format!("{}:{}", provider, model_id);
        // If supported_parameters is absent, the model hasn't been fetched → None (cold).
        let params = data.supported_parameters.get(&key)?;
        Some(params.iter().any(|p| p == "temperature"))
    }

    /// Returns true if the registry has no entries (cold / never populated).
    pub fn is_empty(&self) -> bool {
        self.0
            .lock()
            .map(|d| d.context_lengths.is_empty())
            .unwrap_or(true)
    }
}

// ---------------------------------------------------------------------------
// #3097: LLM-call primitives absorbed from the former `ai/commands.rs`.
// `get_internal_provider`/`call_llm`/`call_llm_from_config` are called from
// three of the other four split modules (chat, scheduler, tools) and from
// nowhere else, so they live here as a shared hub rather than in any one
// spoke. `normalize_api_endpoint` moved here too, for the same reason: it
// was textually adjacent to chat's retry/classify cluster in the original
// file, but its real callers (below, plus `ai/connections.rs`) are this
// file's own `get_internal_provider` and `ai/runtime_resolution.rs`'s own
// separate copy -- nothing in `ai/chat.rs` calls it. Leaving it in `chat.rs`
// and importing it from here would have re-created the exact
// chat<->provider cycle this split exists to avoid (chat already imports
// `super::provider::*`); provider.rs is a hub other modules call into, so
// the function belongs here, not at a spoke.
// ---------------------------------------------------------------------------

/// Normalize API endpoint URL to ensure /v1 suffix for OpenAI-compatible providers.
/// Applied at storage time so all consumers can use {base_url}/chat/completions directly.
pub(crate) fn normalize_api_endpoint(url: &str) -> String {
    let url = url.trim_end_matches('/');
    if url.ends_with("/v1") || url.contains("/v1/") {
        url.to_string()
    } else {
        format!("{}/v1", url)
    }
}
use super::anthropic::{AnthropicProvider, AnthropicResponse};
use super::openrouter::{OpenRouterProvider, OpenRouterResponse, RequestOrigin};
use crate::database::DbState;
use std::time::Duration;
use tokio::time::timeout;


// ==================== §29 Dual-Provider Routing ====================

/// Resolve provider config for internal AI calls (classifier, rerank, extraction).
/// Falls back to primary provider if internal_provider not set (Tier 1/2 compat).
///
/// Resolution chain:
/// 1. internal_provider + internal_api_key from ai_settings (Tier 3 users with OR key)
/// 2. Primary provider if it's direct (Tier 2 users)
/// 3. None → smart routing and rerank DISABLED (Tier 3 without OR key)
///
/// Constraint: Internal provider must be direct. If primary is gateway, step 2 is skipped.
pub(crate) async fn get_internal_provider(db: &DbState) -> Result<AiProvider, String> {
    eprintln!("[INTERNAL-PROVIDER] Resolving internal provider...");
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Step 1: Try internal_provider first
    if let Some(provider_name) = crate::database::get_ai_setting(&conn, "internal_provider") {
        eprintln!(
            "[INTERNAL-PROVIDER] Using internal provider: {}",
            provider_name
        );

        // Ollama doesn't need an API key
        if provider_name == "ollama" {
            let endpoint = crate::database::get_ai_setting(&conn, "internal_endpoint")
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());
            return Ok(AiProvider::Ollama { endpoint });
        }

        // Venice is keychain-only (D4) and reads from its own dedicated slot —
        // NOT the shared OpenRouter/Anthropic `read_internal_key` slot. Handle it
        // before the shared-key branch so it never touches that path.
        // #2795 R1: `include_system_prompt: false` unconditionally on the internal
        // path — extraction/classification/rerank are background machinery, not
        // agent conversation, so Venice's system-prompt toggle (D3, chat-scoped)
        // does not apply here. See smart-routing.md §27.6.2.
        if provider_name == "venice" {
            if let Some(api_key) = crate::ai::credential_migration::read_venice_key(&conn) {
                return Ok(AiProvider::Venice {
                    api_key,
                    include_system_prompt: false,
                });
            } else {
                return Err(
                    "Internal provider venice requires an API key".to_string(),
                );
            }
        }

        // OpenRouter and Anthropic: C50 (#3171 PR 1) — read the key from
        // Provider Connections' own per-provider keychain slot
        // (`read_provider_key`), NOT the retired `ai_internal_key` shared slot
        // (`read_internal_key`). `ai_internal_key` was a single opaque keychain
        // account shared across both providers — switch `internal_provider`
        // from openrouter to anthropic and back, and whatever was typed got
        // overwritten in the SAME slot. That's the exact `api_key`-conflation
        // bug this module's Decision 1 split already fixed for the primary
        // provider, left unfixed for this one feature until now.
        // `credential_migration::run_feature_key_consolidation` backfills any
        // pre-existing `ai_internal_key` value into the matching connection
        // once at launch, then clears it for good — see that function's doc
        // comment. No empty-key fallback: a missing connection key errors here,
        // before any network call — silent degradation to an empty key would
        // produce a 401 indistinguishable from a provider outage.
        if provider_name == "openrouter" || provider_name == "anthropic" {
            if let Some(api_key) = crate::ai::credential_migration::read_provider_key(&conn, &provider_name) {
                let provider = match provider_name.as_str() {
                    "openrouter" => AiProvider::OpenRouter { api_key },
                    "anthropic" => AiProvider::Anthropic { api_key },
                    _ => unreachable!("provider_name already matched to openrouter/anthropic above"),
                };
                return Ok(provider);
            } else {
                return Err(format!(
                    "Internal provider {} is not connected — add it in Settings → AI → Provider Connections",
                    provider_name
                ));
            }
        }

        return Err(format!("Unsupported internal provider: {}", provider_name));
    }

    // Step 2: Fallback to primary provider (backward compat for Tier 1/2)
    // #2586 C2 fix: was `crate::database::get_ai_setting(&conn, "provider")
    // .ok_or("No AI provider configured")?` — a raw read of the retired
    // `ai_settings.provider` scalar (deleted for real by #2564/B1, ADR-0063 D8).
    // This was one of B1's own deferred fast-follow items (#4) — left this Step-2
    // fallback permanently `Err` for every install that never explicitly configured
    // Smart Features, since `ai_settings.provider` has been NULL on every install
    // since B1 landed. Re-pointed to the same `derive_active_provider_label` swap
    // used by `list_available_models` (item #12). (#2617: the `fetch_primary_provider_models`
    // sibling reference here was removed — that function and its caller no longer exist.)
    let primary_provider = crate::ai::runtime_resolution::derive_active_provider_label(&conn);
    if primary_provider == "none" {
        return Err("No AI provider configured".to_string());
    }

    // Primary provider can be used for internal calls (including gateway)
    eprintln!(
        "[INTERNAL-PROVIDER] Using primary provider (fallback): {}",
        primary_provider
    );
    let provider = match primary_provider.as_str() {
        "openrouter" => {
            let primary_key = crate::ai::credential_migration::read_provider_key(&conn, "openrouter")
                .ok_or("No API key configured")?;
            AiProvider::OpenRouter {
                api_key: primary_key,
            }
        }
        "anthropic" => {
            let primary_key = crate::ai::credential_migration::read_provider_key(&conn, "anthropic")
                .ok_or("No API key configured")?;
            AiProvider::Anthropic {
                api_key: primary_key,
            }
        }
        "openclaw" => {
            let base_url = normalize_api_endpoint(
                &crate::database::get_ai_setting(&conn, "endpoint")
                    .unwrap_or_else(|| "http://127.0.0.1:18789".to_string()),
            );
            let token = crate::ai::credential_migration::read_gateway_token(&conn)
                .ok_or("No gateway token configured")?;
            // #29 / this fn's own comment: internal-provider path bypasses the
            // gateway entirely (binary classifier, reranker) — never OC-origin
            // dispatch, so no wire selector applies here.
            AiProvider::OpenClaw { base_url, token, runtime_ref: None }
        }
        "ollama" => {
            // B3 (#2565) Req 2: internal-provider fallback also needs the explicit
            // ollama_endpoint signal — consistent with the primary dispatch path in
            // runtime_resolution.rs. No silent localhost default for an unconfigured
            // ollama connection.
            let endpoint = crate::database::get_ai_setting(&conn, "ollama_endpoint")
                .ok_or("No Ollama connection configured")?;
            AiProvider::Ollama { endpoint }
        }
        "venice" => {
            // #2795 R2: Venice-primary with internal_provider unset must resolve,
            // not Err — this broken state is what motivated #2795. Keychain-only
            // key (D4), same as Step 1's venice arm above.
            // #2795 R1: include_system_prompt: false unconditionally — this is
            // still the internal/background call path, not agent conversation.
            let primary_key = crate::ai::credential_migration::read_venice_key(&conn)
                .ok_or("No Venice API key configured")?;
            AiProvider::Venice {
                api_key: primary_key,
                include_system_prompt: false,
            }
        }
        _ => {
            return Err(format!(
                "Unsupported provider for internal calls: {}",
                primary_provider
            ))
        }
    };

    Ok(provider)
}


// ==================== End §29 Dual-Provider Routing ====================

// ==================== Unified Internal LLM Call Function (§4.5) ====================

/// Unified non-streaming LLM call for all internal AI tasks
/// Returns the text response from the LLM
/// Timeout is configurable per call (default 30s)
pub(crate) async fn call_llm(
    provider: &AiProvider,
    model: &str,
    messages: &[ChatMessage],
    max_tokens: u32,
    temperature: f64,
    timeout_secs: u64,
    db: Option<&crate::database::DbState>,
    model_registry: Option<Arc<Mutex<ModelRegistryData>>>,
) -> Result<String, String> {
    // Bug 1c fix (#2777 venice live-test scoping): `migrate_v83` (migrations.rs)
    // seeds the literal placeholder string "openclaw-pending" into `ai_agents.model`
    // for any openclaw row lacking a real model yet. The ONLY place that previously
    // rewrote this sentinel to the real "openclaw" model id before hitting the wire
    // was `stream_openrouter_inner` (openrouter.rs) — the non-streaming path
    // (`chat_with_tools_inner`, openrouter.rs) had no equivalent rewrite, so the
    // consolidator, memory extraction, synthesis, and the classifier (all non-streaming
    // callers of `call_llm`) could leak the raw sentinel to the gateway verbatim.
    //
    // Per Matt's ruling: resolve the sentinel upstream, ONCE, here — not as a second
    // copy of the same one-line rewrite duplicated into `chat_with_tools_inner`. Two
    // independent copies of this fix staying in sync is exactly how the gap opened in
    // the first place. This covers both the streaming and non-streaming paths from a
    // single site, since both ultimately dispatch through `call_llm`'s `OpenClaw` arm.
    let resolved_model = if matches!(provider, AiProvider::OpenClaw { .. })
        && (model == "openclaw-pending" || model.starts_with("openclaw-pending"))
    {
        "openclaw".to_string()
    } else {
        model.to_string()
    };

    let request = ChatRequest {
        messages: messages.to_vec(),
        model: resolved_model,
        max_tokens,
        temperature,
    };

    let future = async {
        match provider {
            AiProvider::Anthropic { api_key } => {
                let provider_impl = AnthropicProvider::new(api_key.clone());
                let response: AnthropicResponse =
                    provider_impl.chat_with_tools(request, None, db).await?;
                Ok(response.response.text)
            }
            AiProvider::OpenRouter { api_key } => {
                let base = OpenRouterProvider::new(api_key.clone());
                let provider_impl = match model_registry.clone() {
                    Some(reg) => base.with_registry(reg),
                    None => base,
                };
                let response: OpenRouterResponse =
                    provider_impl.chat_with_tools(request, None, db).await?;
                Ok(response.response.text)
            }
            AiProvider::OpenClaw { base_url, token, runtime_ref } => {
                // ADR-0067 D2/D5: stamp the agent-scoped wire selector — never the
                // agent's local id, which never reaches this enum in the first place.
                let provider_impl = OpenRouterProvider::with_base_url(
                    token.clone(),
                    base_url.clone(),
                    RequestOrigin::OpenClaw,
                )
                .with_oc_agent_ref(runtime_ref.clone());
                let response: OpenRouterResponse =
                    provider_impl.chat_with_tools(request, None, db).await?;
                Ok(response.response.text)
            }
            AiProvider::Ollama { endpoint } => {
                let provider_impl = super::ollama::OllamaProvider::new(Some(endpoint.clone()));
                let response = provider_impl.chat(request).await?;
                Ok(response.text)
            }
            AiProvider::Venice { api_key, include_system_prompt } => {
                // ADR-0065 D2: Venice uses the OpenAI-shaped client with VENICE_API_BASE,
                // RequestOrigin::Venice, and include_venice_system_prompt injection.
                let provider_impl = super::openrouter::OpenRouterProvider::with_base_url(
                    api_key.clone(),
                    super::openrouter::VENICE_API_BASE.to_string(),
                    super::openrouter::RequestOrigin::Venice,
                )
                .with_venice_system_prompt(*include_system_prompt);
                let response: super::openrouter::OpenRouterResponse =
                    provider_impl.chat_with_tools(request, None, db).await?;
                Ok(response.response.text)
            }
            AiProvider::None => Err("AI provider not configured".to_string()),
        }
    };

    timeout(Duration::from_secs(timeout_secs), future)
        .await
        .map_err(|_| format!("LLM call timeout after {}s", timeout_secs))?
}


/// String-based wrapper for callers without AiProvider instance (e.g., background workers)
/// Converts provider config strings into AiProvider enum and calls unified function
pub(crate) async fn call_llm_from_config(
    provider_name: &str,
    api_key: &str,
    model: &str,
    endpoint: Option<&str>,
    messages: &[ChatMessage],
    max_tokens: u32,
    temperature: f64,
    timeout_secs: u64,
    db: Option<&crate::database::DbState>,
    model_registry: Option<Arc<Mutex<ModelRegistryData>>>,
    // ADR-0067 D2/D5: `ai_agents.runtime_ref` for the agent this call is scoped to.
    // Only read by the "openclaw" arm below. **Never** pass the agent's local id here
    // — see `openrouter::oc_wire_model()`'s doc comment for the D2 leak-guard rationale.
    // Callers that are provider-agnostic / not OC-specific (or that have no agent
    // context at all) pass `None`.
    oc_runtime_ref: Option<&str>,
) -> Result<String, String> {
    let provider = match provider_name {
        "anthropic" => AiProvider::Anthropic {
            api_key: api_key.to_string(),
        },
        "openrouter" => AiProvider::OpenRouter {
            api_key: api_key.to_string(),
        },
        "openclaw" => AiProvider::OpenClaw {
            base_url: normalize_api_endpoint(endpoint.unwrap_or("http://127.0.0.1:18789")),
            token: api_key.to_string(),
            runtime_ref: oc_runtime_ref.map(|r| r.to_string()),
        },
        "ollama" => AiProvider::Ollama {
            endpoint: endpoint.unwrap_or("http://127.0.0.1:11434").to_string(),
        },
        "venice" => AiProvider::Venice {
            api_key: api_key.to_string(),
            // ADR-0065 D2: background synthesis/consolidation context — no user-facing
            // Venice system prompt needed.
            include_system_prompt: false,
        },
        _ => return Err(format!("Unsupported provider: {}", provider_name)),
    };

    call_llm(
        &provider,
        model,
        messages,
        max_tokens,
        temperature,
        timeout_secs,
        db,
        model_registry,
    )
    .await
}

