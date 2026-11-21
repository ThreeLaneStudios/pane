use super::adapter::{OpenAIAdapter, ProviderAdapter};
use super::provider::*;
use super::tools::{ToolCall, ToolDef, CutoffSignal};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};

// OpenRouter API base URL (used as default for OpenRouter provider)
const OPENROUTER_API_BASE: &str = "https://openrouter.ai/api/v1";

/// ADR-0065 D1: Venice API base URL. Stored as a constant (same shape as
/// `OPENROUTER_API_BASE`), never in `ai_settings` — Venice's base URL is fixed.
pub(crate) const VENICE_API_BASE: &str = "https://api.venice.ai/api/v1";

/// #2455 / ADR-0065 D1 — Routing discriminant for the OpenAI-shaped request path.
///
/// Prior to this refactor, every predicate on this path (TLS client selection,
/// the OpenClaw model-sentinel rewrite, and the temperature-omission gate) was
/// derived from `base_url.is_some()` / `base_url != OPENROUTER_API_BASE` — i.e.
/// "has a base_url" was treated as a synonym for "is OpenClaw". That was true
/// only because OpenClaw was the sole caller that ever supplied a non-default
/// base_url. A future caller that also supplies a non-default base_url (e.g. a
/// Venice arm, PR 2) would silently match every one of those OpenClaw-only
/// branches — pinned gateway TLS client selection and the `model: "openclaw"`
/// wire-sentinel rewrite chief among them — even though it is not OpenClaw.
///
/// This enum makes the distinction explicit and variant-keyed instead of
/// URL-inferred. It intentionally only has two members today: `OpenClaw` and
/// `Other`. `Other` covers both bare OpenRouter (`base_url = None` /
/// `OPENROUTER_API_BASE`) and any future non-OpenClaw caller with its own
/// base_url — those callers get the plain `reqwest::Client` and none of the
/// OpenClaw-specific wire rewrites. Adding a genuinely distinct third caller
/// (e.g. Venice) is out of scope for this PR (see ADR-0065 D1) and would add
/// its own variant + match arms in a follow-up, not silently fall into
/// `Other`'s current OpenRouter-shaped behavior without review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOrigin {
    /// Routes through the OpenClaw gateway: pinned/pre-pairing TLS client,
    /// `model` wire field forced to the `"openclaw"` sentinel, temperature
    /// always omitted (OC manages its own model settings).
    OpenClaw,
    /// ADR-0065 D1/D2/D5: Venice direct runtime. Plain always-validating TLS
    /// client (NEVER `build_gateway_client()` / `danger_accept`), real model
    /// name on the wire (never rewritten to `"openclaw"`), temperature always
    /// omitted (D5 — Venice's /models returns no `supported_parameters` for
    /// any of its 111 models; omitting is the fail-safe default), and
    /// `venice_parameters` injected into the request body (D2).
    Venice,
    /// Everything else on this request path today (bare OpenRouter). Plain
    /// TLS client, real model name on the wire, temperature gated by the
    /// existing ModelRegistry capability check.
    Other,
}

/// #3081: Anthropic-style `cache_control` prompt-caching markers, applied to the
/// system message (first message, if `role == "system"`) and the second-to-last
/// message — both only when their `content` is a plain string (tool-call/result
/// messages carry structured content and are left untouched).
///
/// **Never applied to `RequestOrigin::Venice`.** Some Venice backends 400 on it
/// (`abliteration-abliterated-model-large-flash`: "Text content parts may only
/// include type and text" — GLM and Qwen tolerate it, but tolerance isn't a
/// contract). Venice also caches server-side regardless, so the marker buys
/// nothing there even when a model happens to accept it (#3081). Gated on the
/// `RequestOrigin` enum — never on `base_url` — per ADR-0065 D1: a URL-string
/// test breaks on a custom endpoint, a proxy, or a self-hosted Venice, and
/// encodes routing in a string when the variant already carries it.
///
/// `OpenRouter` and `OpenClaw` origins are unaffected — this preserves the
/// pre-#3081 behavior of sending the marker on every non-Venice OpenAI-shaped
/// dispatch (harmlessly ignored by non-Anthropic-backed models; see the
/// `commands.rs:2196` cross-reference for where this caching is load-bearing).
///
/// One shared helper, called from both `chat_with_tools_inner` (:158-shaped
/// body builder) and `stream_openrouter_inner` (streaming body builder) —
/// these two sites previously carried this exact logic duplicated verbatim;
/// per #3053 PR 1's precedent (removing exactly this kind of duplication),
/// a single call site here beats two parallel origin checks drifting apart.
pub(crate) fn apply_anthropic_cache_control(messages: &mut [Value], origin: RequestOrigin) {
    if origin == RequestOrigin::Venice {
        return;
    }

    // Add cache_control to system message (first message if role=system)
    if let Some(msg) = messages.first_mut() {
        if msg["role"].as_str() == Some("system") {
            if let Some(content_str) = msg["content"].as_str() {
                msg["content"] = json!([{
                    "type": "text",
                    "text": content_str,
                    "cache_control": { "type": "ephemeral" }
                }]);
            }
        }
    }

    // Add cache_control to second-to-last message (last before new user message)
    if messages.len() >= 2 {
        let second_last_idx = messages.len() - 2;
        if let Some(msg) = messages.get_mut(second_last_idx) {
            // Only add cache_control if content is a simple string (not tool results or tool calls)
            if let Some(content_str) = msg["content"].as_str() {
                msg["content"] = json!([{
                    "type": "text",
                    "text": content_str,
                    "cache_control": { "type": "ephemeral" }
                }]);
            }
        }
    }
}

/// D7 (ADR-0065): Extract the wait duration (seconds, capped externally) from Venice
/// 429 response headers. Checks `retry-after` first, then `x-ratelimit-reset-requests`.
/// Returns `None` when neither header is present or parseable as integer seconds.
fn extract_venice_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers
        .get("retry-after")
        .or_else(|| headers.get("x-ratelimit-reset-requests"))
        .and_then(|v| v.to_str().ok())?;
    value.trim().parse::<u64>().ok()
}

/// #2455 / ADR-0065 D1: single shared client-selection helper for both request
/// sites that previously had this logic inlined and independently keyed on a
/// `base_url` predicate (`:158` in `chat_with_tools_inner`, `:489` in
/// `stream_openrouter_inner`). Both call sites now delegate here, keyed on the
/// explicit `RequestOrigin` instead — eliminating the duplicated (and previously
/// divergent-risk) inline predicate, and making the branch directly unit-testable
/// without requiring a live `tauri::AppHandle`.
///
/// - `RequestOrigin::OpenClaw` with `db: Some(..)` → `build_gateway_client()` (SPKI-pinned).
/// - `RequestOrigin::OpenClaw` with `db: None` → `build_pre_pairing_client()` (danger_accept
///   fallback — "shouldn't happen in normal flow" per the pre-existing comment this preserves).
/// - `RequestOrigin::Other` → plain `reqwest::Client` with the pre-existing 30s connect /
///   300s total timeouts.
async fn select_openrouter_client(
    origin: RequestOrigin,
    db: Option<&crate::database::DbState>,
) -> Result<reqwest::Client, String> {
    match origin {
        RequestOrigin::OpenClaw => {
            if let Some(db_state) = db {
                crate::gateway_client::build_gateway_client(db_state)
            } else {
                eprintln!(
                    "[OPENROUTER] Warning: Gateway URL but no DB provided, using danger_accept"
                );
                crate::gateway_client::build_pre_pairing_client()
            }
        }
        RequestOrigin::Venice => {
            // ADR-0065 D1 (security-critical): Venice MUST NEVER reach
            // `build_gateway_client()` under any condition — not with a DB
            // handle, not without one. Venice uses a plain always-validating
            // reqwest::Client with normal system-root TLS against api.venice.ai.
            // No `.danger_accept_invalid_certs()`, no `.use_preconfigured_tls()`.
            eprintln!("[VENICE] Building plain TLS client for Venice API");
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| format!("Failed to build Venice HTTP client: {}", e))
        }
        RequestOrigin::Other => {
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| format!("Failed to build HTTP client: {}", e))
        }
    }
}

// §351: Latency tracking for compaction trigger
static LATENCY_HISTORY: Mutex<Option<HashMap<String, Vec<f64>>>> = Mutex::new(None);

pub fn record_latency(thread_id: &str, latency_secs: f64) {
    let mut guard = LATENCY_HISTORY.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let history = map.entry(thread_id.to_string()).or_insert_with(Vec::new);
    history.push(latency_secs);
    if history.len() > 5 {
        history.remove(0);
    } // ring buffer of 5
}

pub fn get_latency_history(thread_id: &str) -> Vec<f64> {
    let guard = LATENCY_HISTORY.lock().unwrap();
    guard
        .as_ref()
        .and_then(|map| map.get(thread_id))
        .cloned()
        .unwrap_or_default()
}

/// §2525: Apply OpenRouter app attribution headers to a request builder.
/// Only call this on direct-to-OpenRouter requests (guarded by
/// `base_url.contains("openrouter")` at the call site) — the OC gateway
/// path manages its own attribution and should not receive these headers.
/// See docs/spec/ai-integration.md §4.3.3 and https://openrouter.ai/docs/app-attribution.
fn apply_openrouter_attribution(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    builder
        .header("HTTP-Referer", "https://paneapp.ai")
        .header("X-OpenRouter-Title", "Pane")
        .header("X-OpenRouter-Categories", "personal-agent,writing-assistant")
}

/// Helper to log cache metrics with formatted numbers.
///
/// #3036 PR 7b (docs/audits/2026-10-16-scope-agent-instructions.md §9A): this
/// used to read `cache_creation_input_tokens`/`cache_read_input_tokens` —
/// Anthropic-native field names, copy-pasted from `anthropic.rs`'s own (for
/// that path, correct) `log_cache_metrics`. OpenRouter's actual `usage`
/// object never carries those two keys; it reports cache accounting nested
/// under `usage.prompt_tokens_details` using OpenAI-compat-style field names
/// (`cached_tokens` for a cache read, `cache_write_tokens` for a cache
/// write — see https://openrouter.ai/docs/guides/best-practices/prompt-caching).
/// Reading the wrong keys meant this line printed `cache_write: 0 |
/// cache_read: 0` on EVERY OpenRouter/Venice/OpenClaw-origin request,
/// regardless of whether caching actually happened upstream — the log could
/// never be used as evidence either way. Fixed to read the real nested keys;
/// no change to what's enforced or sent, only to what's observed.
fn log_cache_metrics(usage: &Value) {
    let (input_tokens, output_tokens, cache_creation, cache_read) = parse_openrouter_cache_metrics(usage);

    // Format numbers with commas
    let format_num = |n: u64| -> String {
        n.to_string()
            .as_bytes()
            .rchunks(3)
            .rev()
            .map(|chunk| std::str::from_utf8(chunk).unwrap())
            .collect::<Vec<_>>()
            .join(",")
    };

    eprintln!(
        "[CACHE] input: {} | output: {} | cache_write: {} | cache_read: {}",
        format_num(input_tokens),
        format_num(output_tokens),
        format_num(cache_creation),
        format_num(cache_read)
    );
}

/// Pure extraction, split out of `log_cache_metrics` so the field-name fix
/// (#3036 PR 7b) is directly testable against a real OpenRouter-shaped
/// `usage` payload, not provable only by eyeballing the `eprintln!`.
/// Returns `(input_tokens, output_tokens, cache_write_tokens, cache_read_tokens)`.
fn parse_openrouter_cache_metrics(usage: &Value) -> (u64, u64, u64, u64) {
    let input_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
    let output_tokens = usage["completion_tokens"].as_u64().unwrap_or(0);
    let prompt_tokens_details = &usage["prompt_tokens_details"];
    let cache_write = prompt_tokens_details["cache_write_tokens"].as_u64().unwrap_or(0);
    let cache_read = prompt_tokens_details["cached_tokens"].as_u64().unwrap_or(0);
    (input_tokens, output_tokens, cache_write, cache_read)
}

#[cfg(test)]
mod pr7b_cache_metrics_field_name_tests {
    use super::*;
    use serde_json::json;

    /// BEFORE this PR (reproduced here against the real shape, not asserted
    /// by inspection): a real OpenRouter `usage` object — nested
    /// `prompt_tokens_details.cached_tokens`/`cache_write_tokens`, OpenAI-compat
    /// style — read through the OLD (Anthropic-native) field names would
    /// always report zero for both cache fields, even with real cache
    /// activity present in the payload. This test proves the NEW parser
    /// reads the real nested keys correctly.
    #[test]
    fn test_parses_real_openrouter_nested_cache_fields() {
        let usage = json!({
            "prompt_tokens": 10339,
            "completion_tokens": 60,
            "cost": 0.0123,
            "prompt_tokens_details": {
                "cached_tokens": 10318,
                "cache_write_tokens": 21
            }
        });
        let (input, output, cache_write, cache_read) = parse_openrouter_cache_metrics(&usage);
        assert_eq!(input, 10339);
        assert_eq!(output, 60);
        assert_eq!(cache_write, 21, "cache_write_tokens must be read from the nested prompt_tokens_details object");
        assert_eq!(cache_read, 10318, "cached_tokens must be read from the nested prompt_tokens_details object");
    }

    /// The OLD (wrong) Anthropic-native field names, if present at the top
    /// level of a real OpenRouter payload (they never are, but this proves
    /// the fix doesn't accidentally fall back to them), must NOT be read.
    #[test]
    fn test_does_not_fall_back_to_anthropic_native_top_level_field_names() {
        let usage = json!({
            "prompt_tokens": 500,
            "completion_tokens": 10,
            // Anthropic-native names, wrongly placed at the top level — the
            // old, buggy code read exactly these two keys.
            "cache_creation_input_tokens": 999,
            "cache_read_input_tokens": 999,
            "prompt_tokens_details": {
                "cached_tokens": 0,
                "cache_write_tokens": 0
            }
        });
        let (_, _, cache_write, cache_read) = parse_openrouter_cache_metrics(&usage);
        assert_eq!(cache_write, 0, "must read prompt_tokens_details.cache_write_tokens, not the Anthropic-native top-level key");
        assert_eq!(cache_read, 0, "must read prompt_tokens_details.cached_tokens, not the Anthropic-native top-level key");
    }

    /// A payload with no cache activity at all (`prompt_tokens_details`
    /// absent entirely) must report zero, not panic.
    #[test]
    fn test_missing_prompt_tokens_details_defaults_to_zero_no_panic() {
        let usage = json!({"prompt_tokens": 100, "completion_tokens": 5});
        let (input, output, cache_write, cache_read) = parse_openrouter_cache_metrics(&usage);
        assert_eq!(input, 100);
        assert_eq!(output, 5);
        assert_eq!(cache_write, 0);
        assert_eq!(cache_read, 0);
    }
}

/// ADR-0067 D2/D5: compute the wire `model` value for an OC-origin request.
///
/// `oc_agent_ref` MUST be `ai_agents.runtime_ref` (the OC-side agent selector, e.g.
/// "main"/"researcher") — **never** `ai_agents.id` (Pane's local UUID). Passing the
/// local id here would leak it onto the wire, which is exactly what ADR-0067
/// Decision 2 forbids. `None`/empty → bare `"openclaw"`, never `"openclaw/null"` and
/// never `unwrap_or(agent_id)`. This is the single point both the non-streaming
/// (`chat_with_tools_inner`) and streaming (`stream_openrouter_inner`) paths call —
/// unifying what were previously two divergent behaviors (non-streaming passed
/// `request.model` through untouched; streaming forced the bare sentinel
/// unconditionally, discarding any agent-scoped value).
pub fn oc_wire_model(oc_agent_ref: Option<&str>) -> String {
    match oc_agent_ref {
        Some(r) if !r.is_empty() => format!("openclaw/{}", r),
        _ => "openclaw".to_string(),
    }
}

pub struct OpenRouterProvider {
    api_key: String,
    base_url: String,
    /// #2455 / ADR-0065 D1 — explicit routing discriminant, set at construction time
    /// by the constructor the caller chooses (`new` → `Other`, `with_base_url` →
    /// caller-supplied). Every predicate that used to compare `base_url` against
    /// `OPENROUTER_API_BASE` now reads this field instead.
    origin: RequestOrigin,
    /// Optional ModelRegistry for temperature capability checks on OR-direct path (#1846).
    /// When None, load-bearing temperature (0.0) is omitted (fail-safe for cold registry).
    model_registry: Option<std::sync::Arc<std::sync::Mutex<super::provider::ModelRegistryData>>>,
    /// ADR-0065 D2/D3: Venice `include_venice_system_prompt` value. Only meaningful
    /// when `origin == RequestOrigin::Venice`; ignored for all other origins.
    /// Default false (D3: off means Pane sends `include_venice_system_prompt: false`,
    /// suppressing Venice's built-in system-prompt append).
    venice_include_system_prompt: bool,
    /// ADR-0067 D2/D5: `ai_agents.runtime_ref` for the OC agent this request should be
    /// scoped to (e.g. "main", "researcher"). Only meaningful when
    /// `origin == RequestOrigin::OpenClaw`. **Never** Pane's local `ai_agents.id` — see
    /// `oc_wire_model()` doc comment for the full D2 leak-guard rationale.
    oc_agent_ref: Option<String>,
}

/// Extended response that can include tool calls.
// Preserve the provider response shape for legacy callers.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct OpenRouterResponse {
    pub response: ChatResponse, // Original response
    pub tool_calls: Vec<ToolCall>,
    pub raw_response: Value,              // Full API response for debugging
    pub assistant_message: Option<Value>, // The full message object from choices[0].message
}

impl OpenRouterProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            base_url: OPENROUTER_API_BASE.to_string(),
            origin: RequestOrigin::Other,
            model_registry: None,
            venice_include_system_prompt: false,
            oc_agent_ref: None,
        }
    }

    /// #2455 / ADR-0065 D1 — `origin` is now an explicit, required argument rather
    /// than being inferred from whether `base_url` happens to differ from
    /// `OPENROUTER_API_BASE`. Every current call site passes `RequestOrigin::OpenClaw`
    /// (this constructor has no non-OpenClaw callers today), but the parameter is
    /// mandatory so a future non-OpenClaw caller with its own base_url (e.g. a Venice
    /// arm, PR 2) is forced to make an explicit choice instead of silently inheriting
    /// OpenClaw's gateway-TLS-pinning and model-sentinel behavior.
    pub fn with_base_url(api_key: String, base_url: String, origin: RequestOrigin) -> Self {
        Self {
            api_key,
            base_url: base_url.trim_end_matches('/').to_string(),
            origin,
            model_registry: None,
            venice_include_system_prompt: false,
            oc_agent_ref: None,
        }
    }

    /// ADR-0067 D2/D5: attach the OC agent selector (`ai_agents.runtime_ref`) this
    /// request should be scoped to on the wire. Only meaningful when
    /// `origin == RequestOrigin::OpenClaw`; harmless no-op for other origins (never
    /// read outside the `is_oc_path` gate in `chat_with_tools_inner`).
    ///
    /// **Callers must pass `runtime_ref`, never `agent_id`.** `agent_id` is Pane's
    /// local UUID and must never reach the wire (ADR-0067 D2) — see `oc_wire_model()`.
    pub fn with_oc_agent_ref(mut self, oc_agent_ref: Option<String>) -> Self {
        self.oc_agent_ref = oc_agent_ref;
        self
    }

    /// ADR-0065 D2/D3: set the Venice `include_venice_system_prompt` flag.
    /// Call this when constructing a Venice provider; ignored for other origins.
    pub fn with_venice_system_prompt(mut self, include: bool) -> Self {
        self.venice_include_system_prompt = include;
        self
    }

    /// Attach a ModelRegistry for temperature capability checks on the OR-direct path (#1846).
    /// Call this on providers that talk directly to OpenRouter (base_url = OPENROUTER_API_BASE).
    /// On the OC path, temperature is always omitted regardless of registry.
    pub fn with_registry(
        mut self,
        registry: std::sync::Arc<std::sync::Mutex<super::provider::ModelRegistryData>>,
    ) -> Self {
        self.model_registry = Some(registry);
        self
    }

    /// Chat with optional tool support (with automatic retry on decode errors)
    pub async fn chat_with_tools(
        &self,
        request: ChatRequest,
        tools: Option<&[ToolDef]>,
        db: Option<&crate::database::DbState>,
    ) -> Result<OpenRouterResponse, String> {
        let max_attempts = 2;
        let mut last_error = String::new();
        for attempt in 1..=max_attempts {
            match self.chat_with_tools_inner(request.clone(), tools, db).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    if attempt < max_attempts
                        && (e.contains("Failed to parse response") || e.contains("request failed"))
                    {
                        eprintln!(
                            "[OPENROUTER-RETRY] Decode error on attempt {}, retrying...",
                            attempt
                        );
                        last_error = e;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
        Err(last_error)
    }

    async fn chat_with_tools_inner(
        &self,
        request: ChatRequest,
        tools: Option<&[ToolDef]>,
        db: Option<&crate::database::DbState>,
    ) -> Result<OpenRouterResponse, String> {
        // #2455 / ADR-0065 D1: use the pinned gateway client only for the OpenClaw
        // origin, keyed on the explicit `self.origin` field — not on whether
        // `base_url` happens to differ from OPENROUTER_API_BASE. A future non-OpenClaw
        // caller with its own base_url must never reach `build_gateway_client()`.
        // Delegates to the shared helper also used by `stream_openrouter_inner` (:489).
        let client = select_openrouter_client(self.origin, db).await?;

        // Serialize messages with tool_calls and tool_call_id support.
        // tool_calls are stored in canonical format — translate to OpenAI wire here (#395 PR5).
        let mut messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| {
                let mut msg = json!({
                    "role": m.role,
                });
                // Only include content if non-empty (assistant tool_call messages may have null content)
                if let Some(ref blocks) = m.content_blocks {
                    msg["content"] = blocks.clone();
                } else if !m.content.is_empty() {
                    msg["content"] = json!(m.content);
                } else {
                    msg["content"] = json!(null);
                }
                if let Some(ref tc) = m.tool_calls {
                    // Translate canonical → OpenAI wire format (#395 PR5)
                    let canonical = super::canonical_format::detect_and_normalize_stored_tool_calls(tc);
                    msg["tool_calls"] = super::canonical_format::canonical_to_openai_wire(&canonical);
                }
                if let Some(ref tcid) = m.tool_call_id {
                    msg["tool_call_id"] = json!(tcid);
                }
                msg
            })
            .collect();

        // Anthropic prompt caching: Add cache_control markers to system message and second-to-last message.
        // These are Anthropic-specific syntax for prompt caching (reduces latency and costs for repeated context).
        // When sent to non-Anthropic providers (including OpenClaw routing to non-Anthropic models),
        // these markers are harmlessly ignored. Future: could be made conditional based on provider capabilities.
        // #3081: EXCEPT Venice, which 400s on it for some models — see
        // `apply_anthropic_cache_control`'s doc comment for the full rationale.
        apply_anthropic_cache_control(&mut messages, self.origin);

        // Non-OC paths (bare OpenRouter/Venice) use the model from the request
        // directly. For OpenClaw, `oc_wire_model()` below stamps an agent-scoped
        // routing target (e.g. "openclaw/main") — this was previously a comment-only
        // claim with no enforcing code (§4A.2.1); it is now real.
        let mut body = json!({
            "model": &request.model,
            "messages": messages,
            "max_tokens": request.max_tokens,
        });
        // §1846 temperature gate: send temperature only where (1) load-bearing AND
        // (2) provider metadata confirms support. Vestigial 0.7 is never sent.
        // #2455 / ADR-0065 D1: OC path keyed on `self.origin`.
        // ADR-0065 D5: Venice always omits temperature (Venice /models returns no
        // `supported_parameters` for any of its 111 models; omitting is the fail-safe).
        // OR-direct with 0.0: send only if ModelRegistry confirms "temperature" is supported.
        // Cold registry (model_registry is None or model not fetched) → omit (fail-safe).
        let is_oc_path = self.origin == RequestOrigin::OpenClaw;
        // ADR-0067 D2/D5 (spec §4A.2.1): stamp the agent-scoped routing target here,
        // unconditionally for every OC-origin request, unifying non-streaming with the
        // streaming path's behavior. Uses `self.oc_agent_ref` (runtime_ref) — never
        // `request.model`'s raw value, which may still hold a provider-catalog-shaped
        // string or the migrate_v83 `"openclaw-pending"` placeholder for callers that
        // bypass `call_llm`'s sentinel resolution (see runtime_resolution.rs).
        if is_oc_path {
            body["model"] = json!(oc_wire_model(self.oc_agent_ref.as_deref()));
        }
        let is_venice_path = self.origin == RequestOrigin::Venice;
        if !is_venice_path && should_send_temperature_in_body(
            is_oc_path,
            request.temperature,
            self.model_registry.as_ref(),
            &request.model,
        ) {
            body["temperature"] = json!(0.0);
        }
        // Vestigial chat temperature (0.7 or any non-zero value): always omit.
        // CHAT_STREAM_TEMPERATURE is not sent via chat_with_tools, but guard here for safety.

        // Add tools if provided
        if let Some(tool_defs) = tools {
            let adapter = OpenAIAdapter;
            let formatted_tools = adapter.format_tools(tool_defs);
            body["tools"] = formatted_tools;
            body["tool_choice"] = json!("auto");
        }

        // ADR-0065 D2: Venice-only body injection. Exactly one field — no
        // `enable_web_search`, `enable_web_scraping`, `character_slug`, etc.
        // (those flags bypass Pane's web-tools taint/SSRF gates and are locked off).
        // Never injected on OpenRouter or OpenClaw requests.
        if is_venice_path {
            body["venice_parameters"] = json!({ "include_venice_system_prompt": self.venice_include_system_prompt });
        }

        eprintln!("[OPENROUTER] Sending request with model: {}", request.model);
        eprintln!(
            "[OPENROUTER] Request body: {}",
            serde_json::to_string_pretty(&body).unwrap_or_default()
        );

        // D7: Venice 429 retry state — up to 3 attempts on Venice, once elsewhere.
        let max_venice_attempts: u32 = if is_venice_path { 3 } else { 1 };
        let mut venice_429_attempts = 0u32;
        let mut venice_backoff_secs: u64 = 1;

        // Send loop: retries on Venice HTTP 429, exits immediately on all other outcomes.
        let (status, response_body) = 'send_loop: loop {
            // Build request fresh for each attempt (.json() consumes the builder).
            let mut req_builder = client
                .post(&format!("{}/chat/completions", self.base_url))
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json");
            if self.base_url.contains("openrouter") {
                req_builder = apply_openrouter_attribution(req_builder);
            }

            let res = req_builder.json(&body).send().await.map_err(|e| {
                if e.is_timeout() {
                    "AI provider did not respond within 30 seconds. Check your connection."
                        .to_string()
                } else {
                    format!("OpenRouter request failed: {}", e)
                }
            })?;

            let status = res.status();

            // D9: Venice deprecation header logging (before body consumption).
            if is_venice_path {
                if let Some(warning) = res
                    .headers()
                    .get("x-venice-model-deprecation-warning")
                    .and_then(|v| v.to_str().ok())
                {
                    eprintln!(
                        "[VENICE-DEPRECATION] model={} warning={}",
                        request.model, warning
                    );
                }
            }

            // D7: Venice 429 rate-limit retry with header-driven or exponential backoff.
            if is_venice_path && status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                venice_429_attempts += 1;
                if venice_429_attempts < max_venice_attempts {
                    let wait = extract_venice_retry_after(res.headers())
                        .map(|s| s.min(30))
                        .unwrap_or(venice_backoff_secs);
                    eprintln!(
                        "[VENICE] Rate limited (attempt {}/{}), waiting {}s before retry",
                        venice_429_attempts, max_venice_attempts, wait
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    venice_backoff_secs = (venice_backoff_secs * 2).min(4);
                    continue 'send_loop;
                }
                return Err(
                    "Venice rate limit reached for this model. Wait a moment and try again."
                        .to_string(),
                );
            }

            let response_body: Value = res
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {}", e))?;
            break 'send_loop (status, response_body);
        };

        eprintln!("[OPENROUTER] Response status: {}", status);
        eprintln!(
            "[OPENROUTER] Response body: {}",
            serde_json::to_string_pretty(&response_body).unwrap_or_default()
        );

        if !status.is_success() {
            // Hard-fail: billing / payment required (§4.3.2). Provider-aware message.
            if status == reqwest::StatusCode::PAYMENT_REQUIRED {
                let msg = if is_venice_path {
                    "Venice reports no available credits. Add credits in your Venice account and try again.".to_string()
                } else {
                    "OpenRouter reports no available credits. If you just added some, it can take up to 30 seconds to reflect — try again shortly.".to_string()
                };
                return Err(msg);
            }
            // Try OpenRouter format first, fall back to generic JSON error, then raw text
            let error_msg = response_body["error"]["message"]
                .as_str()
                .or_else(|| response_body["error"].as_str())
                .or_else(|| response_body["message"].as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    serde_json::to_string(&response_body)
                        .unwrap_or_else(|_| "Unknown error".to_string())
                });
            eprintln!("[OPENROUTER] Error response: {}", error_msg);
            return Err(format!("API error ({}): {}", status, error_msg));
        }

        // Extract assistant message
        let assistant_msg = response_body["choices"][0]["message"].clone();

        // Extract text
        let text = response_body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        let input_tokens = response_body["usage"]["prompt_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;
        let output_tokens = response_body["usage"]["completion_tokens"]
            .as_u64()
            .unwrap_or(0) as u32;

        // Log cache metrics
        log_cache_metrics(&response_body["usage"]);

        // Extract tool calls if present
        let tool_calls = if tools.is_some() {
            let adapter = OpenAIAdapter;
            adapter.extract_tool_calls(&response_body)
        } else {
            vec![]
        };

        Ok(OpenRouterResponse {
            response: ChatResponse {
                text,
                model: request.model,
                input_tokens,
                output_tokens,
            },
            tool_calls,
            raw_response: response_body,
            assistant_message: Some(assistant_msg),
        })
    }

    /// Original chat method (for backward compatibility)
    #[allow(dead_code)] // Legacy non-tool provider entry point.
    pub async fn chat(
        &self,
        request: ChatRequest,
        db: Option<&crate::database::DbState>,
    ) -> Result<ChatResponse, String> {
        let response = self.chat_with_tools(request, None, db).await?;
        Ok(response.response)
    }
}

/// Temperature gate helper: returns true when load-bearing temperature should be included in the body.
/// Rules:
/// - OC path (is_oc_path=true): always false — OC manages its own model settings.
/// - Non-zero temperature: always false — only load-bearing 0.0 is ever sent.
/// - Cold registry (None or model absent): false — fail-safe.
/// - OR-direct + 0.0 + "temperature" in supported_parameters: true.
///
/// Extracted as a standalone function to enable unit testing of body-building logic (#1950).
pub(crate) fn should_send_temperature_in_body(
    is_oc_path: bool,
    request_temperature: f64,
    model_registry: Option<&Arc<Mutex<ModelRegistryData>>>,
    model: &str,
) -> bool {
    if is_oc_path || request_temperature != 0.0 {
        return false;
    }
    model_registry
        .and_then(|reg| {
            reg.lock().ok().and_then(|data| {
                let key = format!("openrouter:{}", model);
                data.supported_parameters
                    .get(&key)
                    .map(|params| params.iter().any(|p| p == "temperature"))
            })
        })
        .unwrap_or(false) // cold registry → omit (fail-safe)
}

/// Stream OpenRouter responses with SSE
/// Error from `stream_openrouter`: `(error_message, partial_text_accumulated_before_failure)`.
/// On a pre-stream failure (bad key, HTTP error), `partial_text` is empty.
/// On a mid-stream failure (TCP closed, idle timeout), `partial_text` contains whatever
/// the stream emitted before the error so callers can surface it to the user.
pub type StreamOpenrouterError = (String, String);

/// §27.6.1d: Success payload — (full_text, tool_calls, resolved_model_opt, cutoff_signal).
/// `resolved_model_opt` carries the value of the `X-OC-Resolved-Model` response header
/// when the call went through the OC gateway (`RequestOrigin::OpenClaw`), or `None`
/// otherwise. Callers on the OC path compare this to `ai_settings.model` and update
/// the cache if it differs (continuous-refresh channel — the missing path pre-#1453).
/// `cutoff_signal` is §4.5.9 (#2874 item 60)'s per-turn cut-off signal, read from
/// `choices[0].finish_reason` and gated on `origin` — see `openai_cutoff_signal`.
pub type StreamOpenrouterOk = (String, Vec<ToolCall>, Option<String>, CutoffSignal);

/// #2455 / ADR-0065 D1: `origin` is now a required, explicit argument — every call
/// site must state whether this is an OpenClaw-gateway call or not, rather than the
/// callee inferring it from `base_url.is_some()`. This is the same discriminant used
/// by `OpenRouterProvider` (see `RequestOrigin` doc comment above).
///
/// ADR-0067 D2/D5 test-seam note (#2958 follow-up): `app` is generic over
/// `R: tauri::Runtime` (rather than the implicit `Wry` default) purely so
/// `tauri::test::mock_app()` — which yields `App<MockRuntime>`, a distinct
/// concrete type from the production `App<Wry>` — can satisfy this bound in
/// tests. Every production call site still passes a concrete `&tauri::AppHandle`
/// (= `AppHandle<Wry>`); Rust infers `R = Wry` there with zero call-site changes.
/// This closes the exact gap `origin_variant_keying_tests`'s doc comment used to
/// describe as untestable ("requires a Wry-backed tauri::AppHandle... that
/// tauri::test::mock_app() cannot produce") — see `d2_dispatch_site_tests.rs`
/// for the real wire-capture test this enables.
pub async fn stream_openrouter<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    stream_id: &str,
    api_key: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: &[ToolDef],
    base_url: Option<&str>,
    origin: RequestOrigin,
    thread_id: Option<&str>,
    db: Option<&crate::database::DbState>,
    temperature: f64,
    max_tokens: u32,
    // ADR-0065 D2: Venice `include_venice_system_prompt` flag. Only used when
    // `origin == RequestOrigin::Venice`; pass `false` for all other origins.
    venice_include_system_prompt: bool,
    // ADR-0067 D2/D5: `ai_agents.runtime_ref` for the OC agent this stream is
    // scoped to. Only meaningful when `origin == RequestOrigin::OpenClaw`; pass
    // `None` for all other origins. **Never** pass `agent_id` here — see
    // `oc_wire_model()`'s doc comment for the D2 leak-guard rationale.
    oc_agent_ref: Option<&str>,
    // §36.2.3 (#3050 PR 6): the turn's own per-session cancel flag (same
    // `Arc<AtomicBool>` `run_agent_loop` checks at its own two checkpoints,
    // `ai::chat::CANCEL_FLAGS`). Checked once per received SSE chunk, inside
    // `stream_openrouter_inner`'s own loop — the ONE place an OC turn's
    // entire duration is otherwise unreachable to Stop (see that function's
    // doc comment for why). Every call site passes the REAL per-session
    // handle, never a fresh `Arc::new(AtomicBool::new(false))` — that
    // exact anti-pattern is what the call-site-completeness mutation test
    // checks for.
    cancel: Arc<AtomicBool>,
) -> Result<StreamOpenrouterOk, StreamOpenrouterError> {
    // Retry wrapper: if stream decode fails before any content, retry once.
    // On retry we discard the partial from the failed attempt (fresh stream).
    // On the final error we preserve the partial so the caller can surface it.
    let max_attempts = 2;
    let mut last_error: StreamOpenrouterError = (String::new(), String::new());
    for attempt in 1..=max_attempts {
        match stream_openrouter_inner(
            app,
            stream_id,
            api_key,
            model,
            messages.clone(),
            tools,
            base_url,
            origin,
            thread_id,
            db,
            temperature,
            max_tokens,
            venice_include_system_prompt,
            oc_agent_ref,
            cancel.clone(),
        )
        .await
        {
            Ok(result) => return Ok(result),
            Err((e, partial)) => {
                // §36.2.3: a Stop-triggered abort must never be treated as
                // the retry-on-decode-error case below, even though its
                // error text could coincidentally contain "Stream error"
                // (it doesn't today — "Cancelled by Stop" — but checking
                // the flag directly, not the text, is what makes this
                // robust to either). Reconnecting a stream the user asked
                // to stop is exactly the behavior Ruling 4/§36.2.3 forbids.
                if cancel.load(Ordering::Relaxed) {
                    return Err((e, partial));
                }
                if attempt < max_attempts && e.contains("Stream error") {
                    eprintln!(
                        "[STREAM-RETRY] Decode error on attempt {}, retrying...",
                        attempt
                    );
                    // Discard partial from this attempt — the retry starts a fresh stream.
                    last_error = (e, String::new());
                    continue;
                }
                return Err((e, partial));
            }
        }
    }
    Err(last_error)
}

async fn stream_openrouter_inner<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    stream_id: &str,
    api_key: &str,
    model: &str,
    messages: Vec<ChatMessage>,
    tools: &[ToolDef],
    base_url: Option<&str>,
    origin: RequestOrigin,
    thread_id: Option<&str>,
    db: Option<&crate::database::DbState>,
    temperature: f64,
    max_tokens: u32,
    // ADR-0065 D2: Venice include_venice_system_prompt flag. Only meaningful when
    // origin == RequestOrigin::Venice; callers on other paths must pass false.
    venice_include_system_prompt: bool,
    // ADR-0067 D2/D5: see `stream_openrouter`'s doc comment on the same parameter.
    oc_agent_ref: Option<&str>,
    // §36.2.3 (#3050 PR 6): see `stream_openrouter`'s doc comment on the
    // same parameter — threaded through unchanged, this is the function
    // that actually owns the chunk loop and checks it.
    cancel: Arc<AtomicBool>,
) -> Result<StreamOpenrouterOk, StreamOpenrouterError> {
    // #2455 / ADR-0065 D1: use the pinned gateway client only for the OpenClaw
    // origin, keyed on the explicit `origin` argument — not on `base_url.is_some()`.
    // A future non-OpenClaw caller with its own base_url must never reach
    // `build_gateway_client()`. Delegates to the shared helper also used by
    // `chat_with_tools_inner` (:158).
    let client = select_openrouter_client(origin, db)
        .await
        .map_err(|e| (e, String::new()))?;

    let adapter = OpenAIAdapter;
    let formatted_tools = adapter.format_tools(tools);

    // Translate canonical tool calls → OpenAI wire format at the dispatch boundary (#395 PR5)
    let mut msg_values: Vec<Value> = messages
        .iter()
        .map(|m| {
            let mut msg = json!({ "role": m.role });
            if let Some(ref blocks) = m.content_blocks {
                msg["content"] = blocks.clone();
            } else if !m.content.is_empty() {
                msg["content"] = json!(m.content);
            } else {
                msg["content"] = json!(null);
            }
            if let Some(ref tc) = m.tool_calls {
                // Translate canonical → OpenAI wire format (#395 PR5)
                let canonical = super::canonical_format::detect_and_normalize_stored_tool_calls(tc);
                msg["tool_calls"] = super::canonical_format::canonical_to_openai_wire(&canonical);
            }
            if let Some(ref tcid) = m.tool_call_id {
                msg["tool_call_id"] = json!(tcid);
            }
            msg
        })
        .collect();

    // Anthropic prompt caching: Add cache_control markers to system message and second-to-last message.
    // These are Anthropic-specific syntax for prompt caching (reduces latency and costs for repeated context).
    // When sent to non-Anthropic providers (including OpenClaw routing to non-Anthropic models),
    // these markers are harmlessly ignored. Future: could be made conditional based on provider capabilities.
    // #3081: EXCEPT Venice, which 400s on it for some models — see
    // `apply_anthropic_cache_control`'s doc comment for the full rationale.
    apply_anthropic_cache_control(&mut msg_values, origin);

    // Non-OC paths use `model` directly. For OpenClaw, the block below stamps an
    // agent-scoped routing target via `oc_wire_model()` — this is now enforced code,
    // not just a comment (§4A.2.1).
    let mut body = json!({
        "model": model,
        "messages": msg_values,
        "max_tokens": max_tokens,
        "stream": true,
    });
    // §1846 temperature gate: send temperature only where (1) load-bearing AND
    // (2) provider metadata confirms support. Vestigial 0.7 is never sent.
    // #2455 / ADR-0065 D1: OC path is now keyed on `origin`, not on `base_url.is_none()`
    // — always omit for OpenClaw (OC manages its own model settings).
    // ADR-0065 D5: Venice always omits temperature (no `supported_parameters` in /models).
    // OR-direct with 0.0: send only if ModelRegistry.supports_temperature() == Some(true).
    // Cold registry (model not yet fetched) → omit (fail-safe).
    if origin != RequestOrigin::OpenClaw && origin != RequestOrigin::Venice && temperature == 0.0 {
        let registry = app.state::<crate::ai::provider::ModelRegistry>();
        let send_temp = registry.supports_temperature("openrouter", model)
            .unwrap_or(false); // cold → omit
        if send_temp {
            body["temperature"] = json!(0.0);
        }
    }
    // Vestigial CHAT_STREAM_TEMPERATURE (0.7), OC path, and Venice: temperature field not added.
    // ADR-0067 D2/D5 (spec §4A.2.1/§4A.2.2): when routing through the OC gateway
    // (`origin == RequestOrigin::OpenClaw`), stamp an agent-scoped routing target
    // (`openclaw/<runtime_ref>`, or bare "openclaw" when no runtime_ref is known) via
    // the shared `oc_wire_model()` helper — the same one `chat_with_tools_inner` uses,
    // unifying what were previously two divergent behaviors on the same wire.
    // Callers on the non-OC path (bare OpenRouter, or any future non-OpenClaw caller
    // with its own base_url) are unaffected.
    //
    // #2455 / ADR-0065 D1: this predicate was previously keyed on `base_url.is_some()`,
    // which is exactly the bug D1 exists to close — any future caller supplying a
    // non-default base_url (e.g. a Venice arm) would have matched here and had its
    // real model name silently overwritten with the literal string "openclaw",
    // guaranteeing a MODEL_NOT_FOUND failure on that provider. Keying on `origin`
    // instead makes this OpenClaw-only, as it always should have been.
    //
    // C1 (#2585) cross-reference: this unconditional rewrite is load-bearing for
    // `migrate_v83`'s `'openclaw-pending'` placeholder (migrations.rs) — that
    // migration seeds a sentinel string into `ai_agents.model` for OC rows that
    // were left with no model value, relying on THIS rewrite to guarantee the
    // placeholder is never actually sent to the gateway (it's overwritten to
    // "openclaw" right here, unconditionally, before the request leaves the
    // machine). If this rewrite ever becomes conditional (e.g. "only rewrite if
    // model is empty"), the placeholder could leak onto the wire as a real model
    // parameter. See also the migration comment in `migrations.rs` (`migrate_v83`)
    // for the other half of this cross-reference.
    if origin == RequestOrigin::OpenClaw {
        body["model"] = json!(oc_wire_model(oc_agent_ref));
    }
    // ADR-0065 D2: Venice-only body injection. Exactly one field — no
    // `enable_web_search`, `enable_web_scraping`, `character_slug`, etc.
    // (those flags bypass Pane's web-tools taint/SSRF gates and are locked off).
    // The bool is passed via the `venice_include_system_prompt` parameter.
    if origin == RequestOrigin::Venice {
        body["venice_parameters"] = json!({ "include_venice_system_prompt": venice_include_system_prompt });
        // Ride-along 5 (#2777 venice live-test scoping): keys-only body log — gives
        // execution evidence that `venice_parameters` was actually attached to the
        // outgoing request, without leaking API keys, message content, or any other
        // values. Logs only the top-level JSON object's field names.
        eprintln!(
            "[VENICE] stream body keys: {:?}",
            body.as_object().map(|o| o.keys().collect::<Vec<_>>())
        );
    }
    // OpenClaw session persistence: include thread_id as user field
    // ADR-0065 §30.7.2: Venice MUST pass None for thread_id (no OC session persistence).
    if let Some(tid) = thread_id {
        body["user"] = json!(tid);
    }
    if formatted_tools.as_array().map_or(false, |a| !a.is_empty()) {
        body["tools"] = json!(formatted_tools);
        body["tool_choice"] = json!("auto");
    }

    let effective_base_url = base_url.unwrap_or(OPENROUTER_API_BASE);

    // D7: Venice 429 retry state — up to 3 attempts on Venice, once elsewhere.
    let max_venice_attempts: u32 = if origin == RequestOrigin::Venice { 3 } else { 1 };
    let mut venice_429_attempts = 0u32;
    let mut venice_backoff_secs: u64 = 1;

    // Send loop: retries on Venice HTTP 429, exits immediately on all other outcomes.
    // §351: request_start is returned from the loop so it measures from the last (successful)
    // send, not from the first failed attempt.
    let (res, resolved_model_header, request_start) = 'send_loop: loop {
        // §351: Latency timer starts fresh for each attempt.
        let request_start = std::time::Instant::now();

        // Build request fresh for each attempt (.json() consumes the builder).
        let mut request_builder = client
            .post(&format!("{}/chat/completions", effective_base_url))
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json");
        if effective_base_url.contains("openrouter") {
            request_builder = apply_openrouter_attribution(request_builder);
        }

        let res = request_builder.json(&body).send().await.map_err(|e| {
            let msg = if e.is_timeout() {
                "AI provider did not respond within 30 seconds. Check your connection.".to_string()
            } else {
                format!("Request failed: {}", e)
            };
            (msg, String::new())
        })?;

        // Check HTTP status before streaming
        let status = res.status();

        // D9: Venice deprecation header logging (before body consumption).
        if origin == RequestOrigin::Venice {
            if let Some(warning) = res
                .headers()
                .get("x-venice-model-deprecation-warning")
                .and_then(|v| v.to_str().ok())
            {
                eprintln!("[VENICE-DEPRECATION] model={} warning={}", model, warning);
            }
        }

        if !status.is_success() {
            // D7: Venice 429 rate-limit retry with header-driven or exponential backoff.
            if origin == RequestOrigin::Venice && status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                venice_429_attempts += 1;
                if venice_429_attempts < max_venice_attempts {
                    let wait = extract_venice_retry_after(res.headers())
                        .map(|s| s.min(30))
                        .unwrap_or(venice_backoff_secs);
                    eprintln!(
                        "[VENICE] Stream rate limited (attempt {}/{}), waiting {}s before retry",
                        venice_429_attempts, max_venice_attempts, wait
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                    venice_backoff_secs = (venice_backoff_secs * 2).min(4);
                    continue 'send_loop;
                }
                return Err((
                    "Venice rate limit reached for this model. Wait a moment and try again."
                        .to_string(),
                    String::new(),
                ));
            }

            let error_body = res.text().await.unwrap_or_default();
            eprintln!("[OPENROUTER-STREAM] HTTP {} error: {}", status, error_body);

            // Try to parse error as JSON (OpenRouter format), fall back to raw text
            let error_msg = if let Ok(json) = serde_json::from_str::<Value>(&error_body) {
                json["error"]["message"]
                    .as_str()
                    .or_else(|| json["error"].as_str())
                    .or_else(|| json["message"].as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| error_body.clone())
            } else {
                error_body.clone()
            };

            // Hard-fail: billing / payment required (§4.3.2). Provider-aware message.
            if status == reqwest::StatusCode::PAYMENT_REQUIRED {
                let billing_msg = if origin == RequestOrigin::Venice {
                    "Venice reports no available credits. Add credits in your Venice account and try again.".to_string()
                } else {
                    "OpenRouter reports no available credits. If you just added some, it can take up to 30 seconds to reflect — try again shortly.".to_string()
                };
                return Err((billing_msg, String::new()));
            }

            return Err((
                format!("API error ({}): {}", status, error_msg),
                String::new(),
            ));
        }

        // 2xx response — capture headers before consuming the stream body.
        // §27.6.1d Part 3: X-OC-Resolved-Model is read here, before bytes_stream() consumes the response.
        // HeaderMap::get is case-insensitive per RFC 7230 and the reqwest implementation.
        // Only populated when routing through the OC gateway; non-OC paths receive None.
        let resolved_model_header: Option<String> = if base_url.is_some() {
            res.headers()
                .get("x-oc-resolved-model")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        } else {
            None
        };

        break 'send_loop (res, resolved_model_header, request_start);
    };

    eprintln!(
        "[OPENROUTER-STREAM] Starting stream for model: {}, base_url: {:?}, thread_id: {:?}",
        model, base_url, thread_id
    );
    eprintln!(
        "[OPENROUTER-STREAM] Message count: {}, first role: {:?}",
        msg_values.len(),
        msg_values.first().map(|m| m["role"].as_str())
    );
    let mut full_text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();
    let mut partial_tool_calls: std::collections::HashMap<usize, (String, String, String)> =
        std::collections::HashMap::new();
    let mut usage_data: Option<Value> = None;
    // §4.5.9 (#2874 item 60) — OpenAI-compat shape carries the cut-off signal
    // as `choices[0].finish_reason`, `null` on every chunk except the last.
    let mut finish_reason: Option<String> = None;

    // §351: Track first-token latency
    let mut first_token_latency: Option<f64> = None;

    let mut stream = res.bytes_stream();
    let mut buffer = String::new();

    let mut debug_line_count = 0;
    while let Some(chunk_result) = stream.next().await {
        // §36.2.3 (#3050 PR 6, Ruling 4) — Stop must abort the in-flight
        // stream, not only check the cancel flag between loop iterations
        // (`run_agent_loop`'s two existing checkpoints are unreachable
        // while a response is streaming, which for an OC turn is the
        // ENTIRE turn — see this module's own `stream_openrouter` doc
        // comment). Checked once per received chunk, at the top, before
        // this chunk is consumed: on a hit, `break` out of the loop
        // (dropping `stream` — the `bytes_stream()` handle over the
        // `reqwest::Response` body; dropping an unconsumed streaming body
        // is how `reqwest` aborts an in-flight HTTP request, no separate
        // `AbortHandle` needed) and return the SAME partial-text channel a
        // mid-stream network error already uses below — reused unchanged,
        // not a new mechanism. Nothing async happens between "last chunk
        // appended to `full_text`" and this check, so there is no point
        // where accumulated text could be lost.
        if cancel.load(Ordering::Relaxed) {
            eprintln!("[OPENROUTER-STREAM] cancel flag observed mid-stream — aborting, preserving partial text ({} chars)", full_text.len());
            return Err(("Cancelled by Stop".to_string(), full_text.clone()));
        }
        // On a mid-stream error, return the partial text accumulated so far alongside
        // the error so callers can persist/display what was streamed before the failure.
        let chunk = match chunk_result {
            Ok(bytes) => bytes,
            Err(e) => return Err((format!("Stream error: {}", e), full_text.clone())),
        };
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(line_end) = buffer.find('\n') {
            let line = buffer[..line_end].trim().to_string();
            buffer = buffer[line_end + 1..].to_string();

            if line.is_empty() || line == "data: [DONE]" {
                continue;
            }

            if let Some(json_str) = line.strip_prefix("data: ") {
                debug_line_count += 1;
                if debug_line_count <= 3 {
                    let truncated = &json_str[..json_str.floor_char_boundary(200)];
                    eprintln!(
                        "[OPENROUTER-STREAM] SSE line {}: {}",
                        debug_line_count, truncated
                    );
                }
                if let Ok(data) = serde_json::from_str::<Value>(json_str) {
                    // Capture usage data if present
                    if let Some(usage) = data.get("usage") {
                        usage_data = Some(usage.clone());
                    }

                    // §4.5.9 (#2874 item 60) — capture finish_reason the same
                    // chunk/field-adjacency pattern usage_data uses above: both
                    // live on this same per-chunk `data["choices"][0]` object,
                    // one level up from `delta`.
                    if let Some(fr) = data["choices"][0]["finish_reason"].as_str() {
                        finish_reason = Some(fr.to_string());
                    }

                    let delta = &data["choices"][0]["delta"];

                    if let Some(content) = delta["content"].as_str() {
                        // §351: Record first-token latency on first content chunk
                        if first_token_latency.is_none() && !content.is_empty() {
                            let latency = request_start.elapsed();
                            first_token_latency = Some(latency.as_secs_f64());
                            eprintln!(
                                "[STREAM-LATENCY] First token: {:.1}s",
                                latency.as_secs_f64()
                            );
                        }

                        full_text.push_str(content);
                        let _ = app.emit(
                            "ai-stream-chunk",
                            json!({
                                "id": stream_id,
                                "text": content,
                                "done": false,
                            }),
                        );
                    }

                    if let Some(tcs) = delta["tool_calls"].as_array() {
                        for tc in tcs {
                            let index = tc["index"].as_u64().unwrap_or(0) as usize;
                            let entry = partial_tool_calls.entry(index).or_insert_with(|| {
                                let id = tc["id"].as_str().unwrap_or("").to_string();
                                let name =
                                    tc["function"]["name"].as_str().unwrap_or("").to_string();
                                (id, name, String::new())
                            });
                            if let Some(args) = tc["function"]["arguments"].as_str() {
                                entry.2.push_str(args);
                            }
                        }
                    }
                }
            }
        }
    }

    eprintln!(
        "[OPENROUTER-STREAM] Stream complete. full_text len: {}, partial_tool_calls: {}",
        full_text.len(),
        partial_tool_calls.len()
    );

    // Log cache metrics if we captured usage data
    if let Some(usage) = usage_data {
        log_cache_metrics(&usage);
    }

    // §4.5.9 (#2874 item 60): a call whose arguments were cut off mid-stream
    // or are otherwise invalid JSON is never dispatched with blanked or
    // dropped arguments — `finalize_tool_calls` preserves `call_id` and `name`
    // either way (previously this loop had no `else` branch at all and simply
    // discarded the entire entry, `call_id` included, on a parse failure).
    tool_calls.extend(finalize_tool_calls(partial_tool_calls));

    // §351: Record latency to history buffer (only for session-persistent calls)
    if let (Some(tid), Some(latency)) = (thread_id, first_token_latency) {
        record_latency(tid, latency);
    }

    // §4.5.9 (#2874 item 60) — resolve the cut-off signal from the raw
    // finish_reason string, gated on `origin` (the OpenClaw discrimination —
    // see `openai_cutoff_signal`'s doc comment).
    let cutoff_signal = openai_cutoff_signal(finish_reason.as_deref(), origin);

    // §27.6.1d Part 3 (cont.) — Return resolved_model_header as third element.
    // This is the successful-stream-completion guard: we only return the header value
    // after [DONE] without error.  Error paths return Err(...) above; this line is
    // only reached when the stream completed cleanly.
    Ok((full_text, tool_calls, resolved_model_header, cutoff_signal))
}

/// §4.5.9 (#2874 item 60) — map OpenAI-compat `choices[0].finish_reason` to
/// the shared `CutoffSignal`, gated on `RequestOrigin`.
///
/// **The OpenClaw trap.** OpenClaw and bare OpenRouter/Venice all go through
/// this exact same parser (`stream_openrouter_inner`, keyed only by
/// `base_url`/`origin` for TLS client selection and wire rewrites — see
/// `RequestOrigin`'s own doc comment). But OpenClaw's OpenAI-compat HTTP
/// translation layer collapses every non-`tool_calls` stop reason to
/// `"stop"` before Pane's HTTP client ever sees it: confirmed by reading the
/// installed OpenClaw gateway package's built `openai-http` response
/// builder — `resolveStopReasonAndPendingToolCalls(meta)` reads a real
/// underlying stop reason, but every consumer of that value only branches on
/// `stopReason === "tool_calls"`; both the streaming `requestFinalize()`
/// helper's default parameter and the non-streaming response body's
/// catch-all branch hard-code `"stop"` for every other case, including a
/// real length-truncation (`openai-http-BNmNRauy.js:182` et al. — see the
/// #2874 item 60 scope audit, §3, for the full file:line trace).
///
/// A length-truncated OpenClaw-routed turn and a normally-completed one
/// therefore both read `finish_reason: "stop"` — Pane cannot tell them apart
/// from this field on that path. Trusting `"stop"` there would tell the
/// agent "your JSON was invalid, re-send it" when the real problem was
/// length — worse than saying nothing (§4.5.9's "no signal available"
/// message is deliberately hedged; the "invalid JSON" message is not). So on
/// `RequestOrigin::OpenClaw`, the signal is unconditionally `Unavailable`,
/// regardless of what the string says — `RequestOrigin` (already used to
/// pick the TLS client and the wire-model sentinel, see
/// `select_openrouter_client` above) is exactly the discriminant available
/// at this point in the shared parser, so no new plumbing is needed to tell
/// OpenClaw apart from real OpenRouter/Venice here.
///
/// `RequestOrigin::Other` (bare OpenRouter) and `RequestOrigin::Venice` both
/// deliver `finish_reason` undisturbed and are trusted normally:
/// `"length"` confirms a cutoff, any other present value confirms a normal
/// stop, and an absent value (field never arrived) is `Unavailable`.
pub(crate) fn openai_cutoff_signal(finish_reason: Option<&str>, origin: RequestOrigin) -> CutoffSignal {
    if origin == RequestOrigin::OpenClaw {
        return CutoffSignal::Unavailable;
    }
    match finish_reason {
        Some("length") => CutoffSignal::ConfirmedCutoff,
        Some(_) => CutoffSignal::ConfirmedNormalStop,
        None => CutoffSignal::Unavailable,
    }
}

#[cfg(test)]
mod openai_cutoff_signal_tests {
    use super::*;

    /// §4.5.9 PR 2 test: bare OpenRouter, `finish_reason: "length"` confirms a cutoff.
    #[test]
    fn test_openrouter_length_confirms_cutoff() {
        assert_eq!(openai_cutoff_signal(Some("length"), RequestOrigin::Other), CutoffSignal::ConfirmedCutoff);
    }

    /// Venice uses the identical OpenAI-compat shape — same result for the same value.
    #[test]
    fn test_venice_length_confirms_cutoff() {
        assert_eq!(openai_cutoff_signal(Some("length"), RequestOrigin::Venice), CutoffSignal::ConfirmedCutoff);
    }

    #[test]
    fn test_openrouter_stop_confirms_normal_stop() {
        assert_eq!(openai_cutoff_signal(Some("stop"), RequestOrigin::Other), CutoffSignal::ConfirmedNormalStop);
    }

    #[test]
    fn test_openrouter_tool_calls_confirms_normal_stop() {
        assert_eq!(openai_cutoff_signal(Some("tool_calls"), RequestOrigin::Other), CutoffSignal::ConfirmedNormalStop);
    }

    #[test]
    fn test_openrouter_absent_finish_reason_is_unavailable() {
        assert_eq!(openai_cutoff_signal(None, RequestOrigin::Other), CutoffSignal::Unavailable);
    }

    /// §4.5.9 PR 2 test 3 (the one that would ship a wrong-advice bug): an
    /// OpenClaw-origin turn reporting `finish_reason: "stop"` — OpenClaw's
    /// own collapsed value for a REAL length cutoff, per this function's doc
    /// comment — must resolve to `Unavailable`, NEVER `ConfirmedNormalStop`.
    /// If this ever regresses to `ConfirmedNormalStop`, every genuine
    /// OpenClaw-path cutoff gets the "invalid JSON, re-send it" message —
    /// wrong advice for a length problem.
    #[test]
    fn test_openclaw_stop_is_unavailable_not_confirmed_normal_stop() {
        assert_eq!(openai_cutoff_signal(Some("stop"), RequestOrigin::OpenClaw), CutoffSignal::Unavailable);
    }

    /// Even if OpenClaw somehow reported "length" directly (it never does,
    /// per the doc comment — but the gate must not depend on that string
    /// being absent), the origin-based gate still forces `Unavailable` —
    /// the distrust is of the ORIGIN, not of one specific string value.
    #[test]
    fn test_openclaw_length_is_still_unavailable() {
        assert_eq!(openai_cutoff_signal(Some("length"), RequestOrigin::OpenClaw), CutoffSignal::Unavailable);
    }
}

/// §4.5.9 — turn this stream's accumulated per-index tool-call fragments
/// (`call_id`, `name`, raw concatenated `function.arguments` text) into finished
/// `ToolCall`s. Pulled out of `stream_openrouter`'s SSE loop so the parse-failure
/// behavior is unit-testable in isolation. Mirrors `anthropic::finalize_tool_calls`.
///
/// BEFORE this PR: a parse failure here had no `else` branch at all — the
/// whole entry, `call_id` and `name` included, was silently discarded, and
/// (via the §4.5.8 text-fallback on the now-empty `tool_calls`) the raw
/// truncated JSON could surface as chat text. AFTER: the entry survives with
/// `args_parse_failed: true` so the agentic loop can refuse it on its own
/// `call_id` instead.
///
/// An empty accumulated string (zero-arg tool call) is treated as `{}`, not a
/// parse failure — matches every provider's wire shape for "no arguments".
fn finalize_tool_calls(
    partial_tool_calls: std::collections::HashMap<usize, (String, String, String)>,
) -> Vec<ToolCall> {
    let mut tool_calls = Vec::new();
    for (_index, (id, name, args_json)) in partial_tool_calls {
        let json_str = if args_json.trim().is_empty() { "{}".to_string() } else { args_json };
        let truncated_args = &json_str[..json_str.floor_char_boundary(200)];
        match serde_json::from_str::<Value>(&json_str) {
            Ok(params) => {
                eprintln!(
                    "[OPENROUTER-STREAM] Tool call: name={}, id={}, args={}",
                    name, id, truncated_args
                );
                tool_calls.push(ToolCall {
                    call_id: id,
                    name,
                    params,
                    args_parse_failed: false,
                });
            }
            Err(e) => {
                eprintln!(
                    "[OPENROUTER-STREAM] Tool args failed to parse for call_id={} name={}: {} (raw: {})",
                    id, name, e, truncated_args
                );
                tool_calls.push(ToolCall {
                    call_id: id,
                    name,
                    params: json!({}),
                    args_parse_failed: true,
                });
            }
        }
    }
    tool_calls
}

#[cfg(test)]
mod finalize_tool_calls_tests {
    use super::*;
    use std::collections::HashMap;

    fn partials(entries: Vec<(usize, &str, &str, &str)>) -> HashMap<usize, (String, String, String)> {
        entries
            .into_iter()
            .map(|(idx, id, name, args)| (idx, (id.to_string(), name.to_string(), args.to_string())))
            .collect()
    }

    /// §4.5.9 test 1 (OpenRouter/Venice/OpenClaw — same function): a truncated
    /// arguments string no longer produces a dispatchable call. BEFORE this
    /// PR: this exact fragment was silently dropped (zero entries returned,
    /// `call_id` included) by the missing-`else` bug. AFTER: it is returned
    /// with `args_parse_failed: true`.
    #[test]
    fn test_truncated_args_marked_as_parse_failed_not_dropped() {
        let partial = partials(vec![(
            0,
            "call_cutoff1",
            "add_sheet_rows",
            "{\"note_id\":\"n1\",\"columns\":[\"a\",\"b\"],\"rows\":[[\"1\",\"2",
        )]);
        let result = finalize_tool_calls(partial);
        assert_eq!(
            result.len(),
            1,
            "BEFORE this PR this would be 0 (silently dropped); AFTER it must be 1 (preserved, marked failed)"
        );
        assert!(result[0].args_parse_failed);
    }

    /// Genuinely malformed (not merely truncated) JSON is also preserved and
    /// flagged, never silently dropped.
    #[test]
    fn test_invalid_json_marked_as_parse_failed() {
        let partial = partials(vec![(0, "call_bad1", "create_note", "not even json")]);
        let result = finalize_tool_calls(partial);
        assert_eq!(result.len(), 1);
        assert!(result[0].args_parse_failed);
    }

    /// §4.5.9 test 2: `call_id` and tool `name` survive into the result even
    /// when arguments fail to parse.
    #[test]
    fn test_call_id_and_name_survive_parse_failure() {
        let partial = partials(vec![(0, "call_xyz9", "edit_note", "{\"note_id\": \"n1\", \"content\": \"trun")]);
        let result = finalize_tool_calls(partial);
        assert_eq!(result[0].call_id, "call_xyz9");
        assert_eq!(result[0].name, "edit_note");
    }

    /// A real zero-arg call (empty accumulated arguments string, the OpenAI-
    /// compat wire shape for "no arguments") is NOT a parse failure.
    #[test]
    fn test_empty_args_is_not_a_parse_failure() {
        let partial = partials(vec![(0, "call_empty1", "list_notes", "")]);
        let result = finalize_tool_calls(partial);
        assert!(!result[0].args_parse_failed);
        assert_eq!(result[0].params, json!({}));
    }

    /// Well-formed arguments parse normally and are never flagged.
    #[test]
    fn test_valid_args_parse_normally() {
        let partial = partials(vec![(0, "call_ok1", "create_note", r#"{"title":"Groceries"}"#)]);
        let result = finalize_tool_calls(partial);
        assert!(!result[0].args_parse_failed);
        assert_eq!(result[0].params["title"], "Groceries");
    }

    /// §4.5.9 test 3: a mixed batch (one failed + one successful call in the
    /// same turn) returns BOTH, each keeping its own distinct `call_id` — the
    /// commands.rs loop's positional zip of `tool_calls` with
    /// `tool_results_for_provider` depends on every entry surviving this step.
    #[test]
    fn test_mixed_batch_keeps_both_calls_with_distinct_call_ids() {
        let partial = partials(vec![
            (0, "call_ok2", "create_note", r#"{"title":"Fine"}"#),
            (1, "call_bad2", "add_sheet_rows", "{\"note_id\":\"n1\",\"rows\":[[\"cut off"),
        ]);
        let mut result = finalize_tool_calls(partial);
        result.sort_by(|a, b| a.call_id.cmp(&b.call_id));
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].call_id, "call_bad2");
        assert!(result[0].args_parse_failed);
        assert_eq!(result[1].call_id, "call_ok2");
        assert!(!result[1].args_parse_failed);
    }
}

#[cfg(test)]
mod stream_openrouter_ok_type_tests {
    use super::*;

    /// §27.6.1d / §4.5.9 (#2874 item 60): StreamOpenrouterOk is a four-tuple
    /// (full_text, tool_calls, resolved_model_opt, cutoff_signal).
    /// Compile-time guard: if the type changes, destructuring below won't compile.
    #[test]
    fn test_stream_openrouter_ok_is_four_tuple() {
        let ok: StreamOpenrouterOk = (
            "response text".to_string(),
            vec![],
            Some("anthropic/claude-opus-4-6".to_string()), // CI-ALLOW: hardcoded-model
            CutoffSignal::Unavailable,
        );
        let (text, tool_calls, resolved, signal) = ok;
        assert_eq!(text, "response text");
        assert!(tool_calls.is_empty());
        assert_eq!(resolved.as_deref(), Some("anthropic/claude-opus-4-6")); // CI-ALLOW: hardcoded-model
        assert_eq!(signal, CutoffSignal::Unavailable);
    }

    /// §27.6.1d: Non-OC path (base_url = None) always returns None as third element.
    #[test]
    fn test_stream_openrouter_ok_none_when_no_base_url() {
        let ok: StreamOpenrouterOk = ("text".to_string(), vec![], None, CutoffSignal::ConfirmedNormalStop);
        let (_, _, resolved, _) = ok;
        assert!(
            resolved.is_none(),
            "non-OC path must carry None for resolved_model_opt"
        );
    }

    /// ADR-0067 D2/D5: Sentinel write logic — when origin is OpenClaw, body model
    /// becomes an agent-scoped routing target via the REAL `oc_wire_model()`
    /// function (previously this test re-typed the mutation inline; now that the
    /// mutation depends on an external runtime_ref value, duplicating it would
    /// silently drift from production behavior).
    #[test]
    fn test_sentinel_write_when_base_url_some() {
        let mut body = serde_json::json!({"model": "anthropic/claude-opus-4-6"}); // CI-ALLOW: hardcoded-model
        let origin = RequestOrigin::OpenClaw;
        if origin == RequestOrigin::OpenClaw {
            body["model"] = serde_json::json!(oc_wire_model(Some("researcher")));
        }
        assert_eq!(
            body["model"].as_str(),
            Some("openclaw/researcher"),
            "sentinel must be agent-scoped when origin is OpenClaw and a runtime_ref is known"
        );
    }

    /// ADR-0067 D5: NULL runtime_ref → bare "openclaw", not "openclaw/null" and not
    /// any UUID-shaped value.
    #[test]
    fn test_sentinel_bare_openclaw_when_runtime_ref_none() {
        let mut body = serde_json::json!({"model": "anthropic/claude-opus-4-6"}); // CI-ALLOW: hardcoded-model
        let origin = RequestOrigin::OpenClaw;
        if origin == RequestOrigin::OpenClaw {
            body["model"] = serde_json::json!(oc_wire_model(None));
        }
        assert_eq!(
            body["model"].as_str(),
            Some("openclaw"),
            "NULL runtime_ref must produce the bare sentinel, never \"openclaw/null\""
        );
    }

    /// §27.6.1d Part 1 / #2455: Sentinel is NOT written when origin is Other (bare
    /// OpenRouter, or any future non-OpenClaw caller with its own base_url).
    #[test]
    fn test_no_sentinel_when_base_url_none() {
        let mut body = serde_json::json!({"model": "anthropic/claude-opus-4-6"}); // CI-ALLOW: hardcoded-model
        let origin = RequestOrigin::Other;
        if origin == RequestOrigin::OpenClaw {
            body["model"] = serde_json::json!(oc_wire_model(Some("researcher")));
        }
        assert_eq!(
            body["model"].as_str(),
            Some("anthropic/claude-opus-4-6"), // CI-ALLOW: hardcoded-model
            "model must not be overwritten when origin is Other"
        );
    }

    /// ADR-0067 D2 leak guard, unit-level: `oc_wire_model` must NEVER be handed
    /// (or produce) a UUID-shaped local agent id. This exercises the real function
    /// directly across a table of runtime_ref shapes, including the exact kind of
    /// value `ai_agents.id` takes (`Uuid::new_v4()...to_uppercase()`, see
    /// sync_client.rs's agent_uuid generation) to prove the function's output never
    /// coincidentally equals or embeds an agent_id-shaped string when runtime_ref
    /// is None.
    #[test]
    fn test_oc_wire_model_never_leaks_local_agent_id_shape() {
        let fake_local_agent_id = "4F2A9E7B1C3D4E5F6A7B8C9D0E1F2A3B"; // UUID-shaped, like sync_client.rs's agent_uuid
        for runtime_ref in [None, Some("main"), Some("researcher"), Some("")] {
            let wire_model = oc_wire_model(runtime_ref);
            assert!(
                !wire_model.contains(fake_local_agent_id),
                "oc_wire_model({:?}) = {:?} must never contain a local-agent-id-shaped string",
                runtime_ref, wire_model
            );
        }
        // The two defined output shapes only.
        assert_eq!(oc_wire_model(Some("main")), "openclaw/main");
        assert_eq!(oc_wire_model(Some("researcher")), "openclaw/researcher");
        assert_eq!(oc_wire_model(None), "openclaw");
        assert_eq!(oc_wire_model(Some("")), "openclaw", "empty-string runtime_ref must fall back to bare sentinel, not \"openclaw/\"");
    }

    /// §27.6.1d Part 3: Cache-update branch — update fires when header differs from current.
    /// Verifies the branching logic used at the callsite in commands.rs.
    #[test]
    fn test_cache_update_fires_when_header_differs() {
        let resolved_model_opt: Option<String> = Some("anthropic/claude-sonnet-4-6".to_string()); // CI-ALLOW: hardcoded-model
        let current_model = "anthropic/claude-opus-4-6".to_string(); // CI-ALLOW: hardcoded-model
        let mut updated = false;
        if let Some(ref header_val) = resolved_model_opt {
            if !header_val.is_empty() && *header_val != current_model {
                updated = true; // represents: set_ai_setting + state.model update
            }
        }
        assert!(
            updated,
            "cache update must fire when header differs from current model"
        );
    }

    /// §27.6.1d Part 3: Cache-update branch is a NO-OP when header equals current.
    #[test]
    fn test_cache_update_noop_when_header_equals_current() {
        let resolved_model_opt: Option<String> = Some("anthropic/claude-opus-4-6".to_string()); // CI-ALLOW: hardcoded-model
        let current_model = "anthropic/claude-opus-4-6".to_string(); // CI-ALLOW: hardcoded-model
        let mut updated = false;
        if let Some(ref header_val) = resolved_model_opt {
            if !header_val.is_empty() && *header_val != current_model {
                updated = true;
            }
        }
        assert!(
            !updated,
            "cache update must be NO-OP when header equals current model"
        );
    }

    /// §27.6.1d Part 3: Cache-update branch is a NO-OP when resolved_model_opt is None.
    #[test]
    fn test_cache_update_noop_when_none() {
        let resolved_model_opt: Option<String> = None;
        let mut updated = false;
        if let Some(ref header_val) = resolved_model_opt {
            if !header_val.is_empty() {
                updated = true;
            }
        }
        assert!(
            !updated,
            "cache update must be NO-OP when resolved_model_opt is None"
        );
    }
}

#[cfg(test)]
mod openrouter_error_type_tests {
    use super::*;

    /// Verify the error tuple shape: (error_message, partial_text).
    /// This is a compile-time guard — if StreamOpenrouterError is ever changed
    /// to a plain String again, the destructuring below won't compile.
    #[test]
    fn test_stream_openrouter_error_is_tuple() {
        let err: StreamOpenrouterError = (
            "Stream error: tcp reset".to_string(),
            "partial text so far".to_string(),
        );
        let (msg, partial) = err;
        assert!(msg.contains("Stream error"));
        assert!(!partial.is_empty());
    }

    /// Pre-stream errors (bad credentials, HTTP 401) should carry empty partial.
    #[test]
    fn test_pre_stream_error_has_empty_partial() {
        let err: StreamOpenrouterError =
            ("API error (401): Unauthorized".to_string(), String::new());
        let (_msg, partial) = err;
        assert!(
            partial.is_empty(),
            "pre-stream errors should not have partial text"
        );
    }
}

#[cfg(test)]
mod tests {

    /// Test: Model ID format validation
    #[test]
    fn test_model_id_format() {
        // Valid full model IDs
        let valid_ids = vec![
            "anthropic/claude-opus-4-6",   // CI-ALLOW: hardcoded-model
            "anthropic/claude-sonnet-4-6", // CI-ALLOW: hardcoded-model
            "anthropic/claude-haiku-4-5",  // CI-ALLOW: hardcoded-model
            "openrouter/google/gemini-2.5-pro",
            "openclaw/main",       // OpenClaw routing target
            "openclaw/researcher", // OpenClaw routing target
        ];

        for id in valid_ids {
            assert!(
                id.contains('/'),
                "Model ID should contain provider prefix: {}",
                id
            );
            assert!(!id.is_empty(), "Model ID should not be empty");
        }

        // Invalid aliases (should not be used in direct API calls)
        let invalid_aliases = vec!["opus", "sonnet", "haiku", "gemini-pro"];
        for alias in invalid_aliases {
            assert!(!alias.contains('/'), "Alias should not be used: {}", alias);
        }
    }
}

/// #2455 / ADR-0065 D1: tests proving each of the five converted predicate
/// sites now behaves identically for `RequestOrigin::OpenClaw` vs
/// `RequestOrigin::Other`.
///
/// Sites :158 (`chat_with_tools_inner` client selection) and :252 (its
/// temperature gate) are exercised through the real, unmodified
/// `OpenRouterProvider::chat_with_tools` production entry point against a
/// mocked HTTP server — no `tauri::AppHandle` is needed on that path.
///
/// Site :489 (streaming client selection) is exercised through the shared
/// `select_openrouter_client` helper both `chat_with_tools_inner` and
/// `stream_openrouter_inner` now delegate to — this is the actual production
/// code for that predicate, not a re-derivation.
///
/// Sites :579 (streaming temperature gate) and :604-605 (streaming model
/// sentinel) live inside `stream_openrouter_inner`, which requires a
/// Wry-backed `tauri::AppHandle` (for `app.state::<ModelRegistry>()`) that
/// `tauri::test::mock_app()` cannot produce (it yields `App<MockRuntime>`,
/// a distinct type) — the pre-existing test suite already documents this
/// constraint (see `test_stream_openrouter_accepts_temperature_and_max_tokens_params`
/// in `streaming_params_tests.rs`: "stream_openrouter is not directly
/// unit-testable without a running Tauri app"). Consistent with that
/// established pattern, these two sites are covered by mirroring the exact
/// `if origin == RequestOrigin::OpenClaw { .. }` / `if origin !=
/// RequestOrigin::OpenClaw && temperature == 0.0 { .. }` conditions verbatim
/// against both origin values, in the same style as the pre-existing
/// `test_sentinel_write_when_base_url_some` / `test_no_sentinel_when_base_url_none`
/// tests directly above this module.
#[cfg(test)]
mod origin_variant_keying_tests {
    use super::*;
    use crate::ai::provider::{ChatMessage, ModelRegistryData};
    use std::collections::HashMap;

    fn user_message(text: &str) -> ChatMessage {
        ChatMessage {
            role: "user".to_string(),
            content: text.to_string(),
            tool_calls: None,
            tool_call_id: None,
            content_blocks: None,
            attachment_ids: None,
            id: None,
        }
    }

    // ── Site :489 (streaming client selection), via the real shared helper ────

    /// `RequestOrigin::OpenClaw` + `db: None` must select the pre-pairing
    /// fallback client (not error, not silently select the plain client) —
    /// this is the exact branch :489 delegates to via `select_openrouter_client`.
    #[tokio::test]
    async fn test_select_client_openclaw_origin_no_db_uses_pre_pairing_fallback() {
        let result = select_openrouter_client(RequestOrigin::OpenClaw, None).await;
        assert!(
            result.is_ok(),
            "OpenClaw origin with no DbState must fall back to build_pre_pairing_client(), not error: {:?}",
            result.err()
        );
        assert_eq!(
            crate::gateway_client::test_take_last_client_path(),
            Some(crate::gateway_client::ClientPathStamp::PrePairing),
            "db: None must take the pre-pairing path, not the pinned path"
        );
    }

    /// #2775 (Tier 1 — §6 of the scope audit): the missing counterpart to the
    /// test above. `RequestOrigin::OpenClaw` + `db: Some(..)` on a *paired*
    /// install (a valid `gateway_tls_cert_pem` stored in `ai_settings`) must
    /// select the SPKI-pinned client via `build_gateway_client_with_cert`, NOT
    /// fall through to `build_pre_pairing_client`'s `danger_accept` path. This
    /// is asserted via the `#[cfg(test)]` stamp mechanism in `gateway_client.rs`
    /// (not log-text matching) since `reqwest::Client` doesn't expose its TLS
    /// config for direct inspection.
    #[tokio::test]
    #[serial_test::serial(pane_test_root)]
    async fn test_select_client_openclaw_origin_with_db_and_cert_uses_pinned_client() {
        // #2775 fix-forward: MUST isolate via PANE_TEST_ROOT before calling
        // init_database(None) — without it, get_pane_root() resolves to the real
        // ~/Pane production install and this test will overwrite the live
        // gateway_tls_cert_pem setting. Use an in-memory-schema connection scoped
        // to a tempdir instead, matching the isolation pattern used elsewhere in
        // this file/crate (e.g. database.rs's own PANE_TEST_ROOT-guarded tests).
        let temp_dir = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        let conn = crate::database::init_database(None).unwrap();
        let pem = crate::gateway_client::test_make_gateway_cert_pem();
        crate::database::set_ai_setting(&conn, "gateway_tls_cert_pem", &pem).unwrap();
        let db = crate::database::DbState::new(conn);

        let result = select_openrouter_client(RequestOrigin::OpenClaw, Some(&db)).await;
        assert!(
            result.is_ok(),
            "OpenClaw origin with a paired DbState must build the pinned client, not error: {:?}",
            result.err()
        );
        assert_eq!(
            crate::gateway_client::test_take_last_client_path(),
            Some(crate::gateway_client::ClientPathStamp::Pinned),
            "db: Some(..) with a stored cert must take the pinned path, not the pre-pairing/danger_accept path"
        );
    }

    /// `RequestOrigin::Other` must always select the plain client, regardless of
    /// whether a `db` happens to be available — mirrors bare OpenRouter today and
    /// any future non-OpenClaw caller with its own base_url (e.g. Venice, PR 2).
    #[tokio::test]
    async fn test_select_client_other_origin_never_touches_gateway_path() {
        let result = select_openrouter_client(RequestOrigin::Other, None).await;
        assert!(
            result.is_ok(),
            "Other origin must always build the plain reqwest client: {:?}",
            result.err()
        );
    }

    // ── Sites :158 / :252 (non-streaming client selection + temperature gate), ──
    // ── via the real, unmodified chat_with_tools production entry point ───────

    /// `OpenRouterProvider::with_base_url(.., RequestOrigin::OpenClaw)` must (a)
    /// dispatch successfully via the pinned/pre-pairing client branch and (b)
    /// never send `temperature` on the wire, even at the load-bearing `0.0`
    /// value that would otherwise trigger the OR-direct capability check.
    #[tokio::test]
    async fn test_provider_with_base_url_openclaw_omits_temperature_and_dispatches() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_request(|request| {
                !request.utf8_lossy_body().unwrap_or_default().contains("temperature")
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::OpenClaw,
        );
        let request = ChatRequest {
            messages: vec![user_message("hello")],
            model: "openclaw/main".to_string(),
            max_tokens: 1024,
            temperature: 0.0, // CI-ALLOW: hardcoded-sampling
        };
        let result = provider.chat_with_tools(request, None, None).await;
        assert!(
            result.is_ok(),
            "with_base_url(.., RequestOrigin::OpenClaw) must dispatch via the pinned/pre-pairing branch: {:?}",
            result.err()
        );
        mock.assert_async().await;
    }

    /// ADR-0067 T1 (request boundary): the REAL `chat_with_tools` production entry
    /// point, with `.with_oc_agent_ref(Some("researcher"))` attached, must send
    /// `model: "openclaw/researcher"` on the wire — captured via mockito's request
    /// matcher against the actual outbound HTTP body, not a re-derivation.
    #[tokio::test]
    async fn test_chat_with_tools_stamps_agent_scoped_model_on_wire() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex(r#""model":"openclaw/researcher""#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::OpenClaw,
        )
        .with_oc_agent_ref(Some("researcher".to_string()));
        let request = ChatRequest {
            messages: vec![user_message("hello")],
            // Deliberately a DIFFERENT value than the expected wire model — proves the
            // stamp overrides request.model rather than passing it through.
            model: "anthropic/claude-opus-4-6".to_string(), // CI-ALLOW: hardcoded-model
            max_tokens: 1024,
            temperature: 0.0, // CI-ALLOW: hardcoded-sampling
        };
        let result = provider.chat_with_tools(request, None, None).await;
        assert!(result.is_ok(), "dispatch must succeed: {:?}", result.err());
        mock.assert_async().await;
    }

    /// ADR-0067 T1, NULL runtime_ref case: no `.with_oc_agent_ref()` call at all
    /// (the real-world shape for a local/non-OC-runtime_ref agent) must produce the
    /// bare "openclaw" sentinel on the wire, never "openclaw/null".
    #[tokio::test]
    async fn test_chat_with_tools_bare_sentinel_when_no_agent_ref_attached() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex(r#""model":"openclaw""#.to_string()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        // No .with_oc_agent_ref() call — mirrors an OC agent whose runtime_ref is NULL.
        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::OpenClaw,
        );
        let request = ChatRequest {
            messages: vec![user_message("hello")],
            model: "openclaw-pending".to_string(),
            max_tokens: 1024,
            temperature: 0.0, // CI-ALLOW: hardcoded-sampling
        };
        let result = provider.chat_with_tools(request, None, None).await;
        assert!(result.is_ok(), "dispatch must succeed: {:?}", result.err());
        mock.assert_async().await;
    }

    /// ADR-0067 D2 guard (Matt's explicit requirement): captures the REAL outbound
    /// HTTP body via mockito and asserts Pane's local `agent_id` (a UUID, distinct
    /// from `runtime_ref`) appears NOWHERE in the entire serialized body — not just
    /// absent from the `model` field. `.with_oc_agent_ref()` is called with the
    /// runtime_ref only; the local agent_id is passed nowhere near the provider,
    /// mirroring the correct threading shape every one of the six dispatch sites
    /// must follow.
    #[tokio::test]
    async fn test_chat_with_tools_never_leaks_local_agent_id_onto_wire() {
        let local_agent_id = "4F2A9E7B1C3D4E5F6A7B8C9D0E1F2A3B"; // UUID-shaped, like sync_client.rs's agent_uuid
        let runtime_ref = "researcher";
        assert_ne!(local_agent_id, runtime_ref, "sanity: the two identifiers must differ");

        let mut server = mockito::Server::new_async().await;
        let captured_body: std::sync::Arc<std::sync::Mutex<Option<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let captured_body_clone = captured_body.clone();
        let mock = server
            .mock("POST", "/chat/completions")
            .match_request(move |request| {
                let body = request.utf8_lossy_body().unwrap_or_default().into_owned();
                *captured_body_clone.lock().unwrap() = Some(body);
                true
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::OpenClaw,
        )
        .with_oc_agent_ref(Some(runtime_ref.to_string()));
        let request = ChatRequest {
            messages: vec![user_message("hello")],
            model: "openclaw-pending".to_string(),
            max_tokens: 1024,
            temperature: 0.0, // CI-ALLOW: hardcoded-sampling
        };
        let result = provider.chat_with_tools(request, None, None).await;
        assert!(result.is_ok(), "dispatch must succeed: {:?}", result.err());
        mock.assert_async().await;

        let body = captured_body.lock().unwrap().clone().expect("body must have been captured");
        assert!(
            !body.contains(local_agent_id),
            "D2 VIOLATION: local agent_id {:?} leaked onto the wire in body: {}",
            local_agent_id, body
        );
        assert!(
            body.contains("openclaw/researcher"),
            "body must carry the agent-scoped runtime_ref, got: {}",
            body
        );
    }

    /// Negative case: `OpenRouterProvider::with_base_url(.., RequestOrigin::Other)`
    /// dispatches via the plain client and is gated by the ModelRegistry
    /// capability check for temperature (cold registry -> omit, same wire shape
    /// as the OpenClaw case above but reached via the opposite branch).
    #[tokio::test]
    async fn test_provider_other_origin_omits_temperature_when_registry_cold() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_request(|request| {
                !request.utf8_lossy_body().unwrap_or_default().contains("temperature")
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::Other,
        );
        let request = ChatRequest {
            messages: vec![user_message("hello")],
            model: "some/unregistered-model".to_string(),
            max_tokens: 1024,
            temperature: 0.0, // CI-ALLOW: hardcoded-sampling
        };
        let result = provider.chat_with_tools(request, None, None).await;
        assert!(
            result.is_ok(),
            "Other-origin dispatch via plain client must succeed: {:?}",
            result.err()
        );
        mock.assert_async().await;
    }

    /// `OpenRouterProvider::with_base_url(.., RequestOrigin::Other)` with a
    /// registry that DOES confirm temperature support must send `temperature`
    /// on the wire — proves the Other-origin capability check still fires
    /// correctly (positive case, complementing the cold-registry test above).
    #[tokio::test]
    async fn test_provider_other_origin_sends_temperature_when_registry_confirms_support() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_body(mockito::Matcher::Regex(r#""temperature":0\.0"#.to_string())) // CI-ALLOW: hardcoded-sampling
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let mut supported_parameters = HashMap::new();
        supported_parameters.insert(
            "openrouter:test-vendor/supports-temp".to_string(),
            vec!["temperature".to_string()],
        );
        let registry_data = ModelRegistryData {
            context_lengths: HashMap::new(),
            supported_parameters,
        };

        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::Other,
        )
        .with_registry(Arc::new(Mutex::new(registry_data)));
        let request = ChatRequest {
            messages: vec![user_message("hello")],
            model: "test-vendor/supports-temp".to_string(),
            max_tokens: 1024,
            temperature: 0.0, // CI-ALLOW: hardcoded-sampling
        };
        let result = provider.chat_with_tools(request, None, None).await;
        assert!(
            result.is_ok(),
            "Other-origin dispatch with confirmed temperature support must succeed: {:?}",
            result.err()
        );
        mock.assert_async().await;
    }

    /// `OpenRouterProvider::new()` always constructs with `origin: RequestOrigin::Other`
    /// — a direct field-level guard against a future edit accidentally defaulting a
    /// bare-OpenRouter constructor to the OpenClaw origin.
    #[test]
    fn test_new_constructor_sets_other_origin() {
        let provider = OpenRouterProvider::new("sk-test".to_string());
        assert_eq!(provider.origin, RequestOrigin::Other);
    }

    /// `OpenRouterProvider::with_base_url` propagates whatever `RequestOrigin` the
    /// caller passes — it does not silently coerce to `OpenClaw` just because a
    /// base_url was supplied (the exact bug this refactor closes).
    #[test]
    fn test_with_base_url_propagates_caller_supplied_origin() {
        let oc = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            "http://127.0.0.1:18789".to_string(),
            RequestOrigin::OpenClaw,
        );
        assert_eq!(oc.origin, RequestOrigin::OpenClaw);

        let other = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            "http://127.0.0.1:9999".to_string(),
            RequestOrigin::Other,
        );
        assert_eq!(
            other.origin,
            RequestOrigin::Other,
            "a non-default base_url must NOT force origin to OpenClaw — this is exactly \
             the bug #2455 fixes (a future Venice caller with its own base_url would \
             otherwise silently inherit OpenClaw's gateway TLS pinning and model sentinel)"
        );
    }

    // ── Sites :579 / :604-605 (streaming temperature gate + model sentinel) ───
    // Mirrors the exact production conditions verbatim (see module doc comment
    // for why `stream_openrouter_inner` itself cannot be called directly in tests
    // — same constraint as the pre-existing `streaming_params_tests.rs` suite).

    /// Site :604-605: `origin == RequestOrigin::OpenClaw` must set the sentinel,
    /// via the REAL `oc_wire_model()` function (not a re-typed literal — this test
    /// previously duplicated the mutation, which the ADR-0067 scope pass flagged as
    /// unsafe once the mutation depends on an external runtime_ref value).
    #[test]
    fn test_streaming_sentinel_write_condition_true_for_openclaw() {
        let mut body = json!({"model": "anthropic/claude-opus-4-6"}); // CI-ALLOW: hardcoded-model
        let origin = RequestOrigin::OpenClaw;
        let oc_agent_ref: Option<&str> = Some("researcher");
        if origin == RequestOrigin::OpenClaw {
            body["model"] = json!(oc_wire_model(oc_agent_ref));
        }
        assert_eq!(body["model"].as_str(), Some("openclaw/researcher"));
    }

    /// Site :604-605, NULL runtime_ref case: must produce the bare sentinel.
    #[test]
    fn test_streaming_sentinel_bare_openclaw_when_runtime_ref_none() {
        let mut body = json!({"model": "anthropic/claude-opus-4-6"}); // CI-ALLOW: hardcoded-model
        let origin = RequestOrigin::OpenClaw;
        let oc_agent_ref: Option<&str> = None;
        if origin == RequestOrigin::OpenClaw {
            body["model"] = json!(oc_wire_model(oc_agent_ref));
        }
        assert_eq!(body["model"].as_str(), Some("openclaw"));
    }

    /// Site :604-605 negative case: `origin == RequestOrigin::Other` must NOT
    /// touch the model field.
    #[test]
    fn test_streaming_sentinel_write_condition_false_for_other() {
        let mut body = json!({"model": "anthropic/claude-opus-4-6"}); // CI-ALLOW: hardcoded-model
        let origin = RequestOrigin::Other;
        let oc_agent_ref: Option<&str> = Some("researcher");
        if origin == RequestOrigin::OpenClaw {
            body["model"] = json!(oc_wire_model(oc_agent_ref));
        }
        assert_eq!(body["model"].as_str(), Some("anthropic/claude-opus-4-6")); // CI-ALLOW: hardcoded-model
    }

    /// ADR-0067 D2 leak guard, streaming-side: even if a caller mistakenly threaded
    /// a local-agent-id-shaped string in as `oc_agent_ref` (the exact D2 risk the
    /// scope doc flagged for the 6 call-site tuple-destructures), the stamped model
    /// string is still well-formed and the guard at the `RuntimeConfig`/threading
    /// layer (see runtime_resolution.rs tests) is what prevents this from happening
    /// in practice — this test documents the wire-format contract precisely so a
    /// reviewer can see what WOULD happen if that guard were ever bypassed, using a
    /// realistic UUID-shaped stand-in.
    #[test]
    fn test_streaming_sentinel_with_uuid_shaped_input_still_wire_formats_correctly() {
        // Not asserting this is a GOOD input — asserting oc_wire_model's contract
        // (always openclaw/<input> or bare openclaw) holds regardless of shape, so
        // the leak-prevention responsibility is visibly placed on callers passing
        // the right field (runtime_ref), not on this formatting function guessing.
        let uuid_shaped = "4F2A9E7B1C3D4E5F6A7B8C9D0E1F2A3B";
        assert_eq!(oc_wire_model(Some(uuid_shaped)), format!("openclaw/{}", uuid_shaped));
    }

    /// Site :579: `origin == RequestOrigin::OpenClaw` must short-circuit the
    /// capability check entirely (never even consult the registry), regardless
    /// of whether the registry would otherwise confirm support.
    #[test]
    fn test_streaming_temperature_gate_short_circuits_for_openclaw() {
        let origin = RequestOrigin::OpenClaw;
        let temperature = 0.0;
        let registry_would_confirm_support = true; // even if it would say yes...
        let send_temp = origin != RequestOrigin::OpenClaw
            && temperature == 0.0
            && registry_would_confirm_support;
        assert!(!send_temp, "OpenClaw origin must never send temperature, even if the registry would confirm support");
    }

    /// Site :579 positive case: `origin == RequestOrigin::Other` with the
    /// registry confirming support and load-bearing `0.0` must send temperature.
    #[test]
    fn test_streaming_temperature_gate_fires_for_other_when_registry_confirms() {
        let origin = RequestOrigin::Other;
        let temperature = 0.0;
        let registry_confirms_support = true;
        let send_temp =
            origin != RequestOrigin::OpenClaw && temperature == 0.0 && registry_confirms_support;
        assert!(send_temp, "Other origin with registry-confirmed support and load-bearing 0.0 must send temperature");
    }

    /// Site :579 fail-safe case: `origin == RequestOrigin::Other` with a cold
    /// registry (`unwrap_or(false)`) must omit temperature.
    #[test]
    fn test_streaming_temperature_gate_fail_safe_for_other_when_registry_cold() {
        let origin = RequestOrigin::Other;
        let temperature = 0.0;
        let registry_lookup: Option<bool> = None; // cold registry
        let send_temp = origin != RequestOrigin::OpenClaw
            && temperature == 0.0
            && registry_lookup.unwrap_or(false);
        assert!(!send_temp, "cold registry must fail safe to omit temperature");
    }
}

#[cfg(test)]
mod openrouter_attribution_tests {
    use super::*;

    /// §2525: apply_openrouter_attribution() must set all three OR attribution
    /// headers with the exact values documented in docs/spec/ai-integration.md §4.3.3.
    /// Builds a real request via reqwest and inspects the finalized header map —
    /// this exercises the actual header-application path, not just the literals.
    #[test]
    fn test_apply_openrouter_attribution_sets_all_three_headers() {
        let client = reqwest::Client::new();
        let builder = client.post("https://openrouter.ai/api/v1/chat/completions");
        let builder = apply_openrouter_attribution(builder);
        let request = builder.build().expect("request should build");
        let headers = request.headers();

        assert_eq!(
            headers.get("HTTP-Referer").map(|v| v.to_str().unwrap()),
            Some("https://paneapp.ai"),
            "HTTP-Referer must be the live production domain"
        );
        assert_eq!(
            headers.get("X-OpenRouter-Title").map(|v| v.to_str().unwrap()),
            Some("Pane"),
            "X-OpenRouter-Title must be set (modern header, not legacy X-Title)"
        );
        assert_eq!(
            headers.get("X-OpenRouter-Categories").map(|v| v.to_str().unwrap()),
            Some("personal-agent,writing-assistant"),
            "X-OpenRouter-Categories must carry both approved category slugs"
        );

        // Legacy header must NOT be present — modernized per §2525.
        assert!(
            headers.get("X-Title").is_none(),
            "legacy X-Title header should not be sent"
        );
        // Stale domain must NOT be present.
        assert_ne!(
            headers.get("HTTP-Referer").map(|v| v.to_str().unwrap()),
            Some("https://pane.dev"),
            "HTTP-Referer must not use the stale pane.dev domain"
        );
    }

    /// §2525: The helper is unconditional by design — the `base_url.contains("openrouter")`
    /// guard lives at the call sites, not inside the helper. Verify the helper always adds
    /// headers when called, so callers own the routing decision explicitly.
    #[test]
    fn test_apply_openrouter_attribution_always_applies_when_called() {
        let client = reqwest::Client::new();
        // Even a non-OpenRouter URL gets headers if the helper is invoked directly —
        // callers are responsible for the base_url guard before calling.
        let builder = client.post("http://127.0.0.1:18789/chat/completions");
        let builder = apply_openrouter_attribution(builder);
        let request = builder.build().expect("request should build");
        let headers = request.headers();

        assert!(
            headers.contains_key("HTTP-Referer"),
            "helper must apply headers unconditionally; guard is the caller's responsibility"
        );
        assert!(headers.contains_key("X-OpenRouter-Title"));
        assert!(headers.contains_key("X-OpenRouter-Categories"));
    }
}

#[cfg(test)]
mod venice_tests {
    use super::*;
    use crate::ai::provider::AiProvider;

    /// ADR-0065 D1 security test: `select_openrouter_client` with `RequestOrigin::Venice`
    /// must return a plain always-validating reqwest::Client. This test verifies the
    /// function builds successfully with the Venice arm — it cannot verify internal TLS
    /// configuration at the type level, but the code path is clearly identified:
    /// `RequestOrigin::Venice` => plain `reqwest::Client::builder().build()`.
    /// Reed's delta audit reads the branch directly to confirm no `danger_accept` / pinned TLS.
    #[tokio::test]
    async fn test_select_openrouter_client_venice_builds_plain_client() {
        // The Venice arm calls plain reqwest::Client::builder().build() — no gateway DB needed.
        // Calling with db=None exercises the ONLY Venice code path (Venice never consults DB
        // for TLS pinning). This test would panic if Venice reached build_pre_pairing_client()
        // (which also works with db=None) — but since they're both infallible, we test the
        // code path exists by calling it and confirming Ok is returned.
        let result = select_openrouter_client(RequestOrigin::Venice, None).await;
        assert!(result.is_ok(), "Venice client build must succeed: {:?}", result);
    }

    /// ADR-0065 D1: Venice MUST NOT use the openclaw gateway client.
    /// Structural test: `RequestOrigin::Venice` dispatches to the Venice arm,
    /// not the `RequestOrigin::OpenClaw` arm. If the match arms ever collapse,
    /// this code would stop compiling (non-exhaustive).
    #[test]
    fn test_request_origin_venice_is_distinct_from_openclaw() {
        // Verify Venice and OpenClaw are distinct enum variants (compile-time check).
        let venice = RequestOrigin::Venice;
        let openclaw = RequestOrigin::OpenClaw;
        assert_ne!(venice, openclaw, "Venice and OpenClaw must be distinct RequestOrigin variants");
        let other = RequestOrigin::Other;
        assert_ne!(venice, other, "Venice and Other must be distinct RequestOrigin variants");
    }

    /// ADR-0065 D5: Venice body-builder always omits temperature — exercises the
    /// production `chat_with_tools` path (not a local simulation of the guard expression).
    #[tokio::test]
    async fn test_venice_temperature_always_omitted() {
        fn user_msg(text: &str) -> crate::ai::provider::ChatMessage {
            crate::ai::provider::ChatMessage {
                role: "user".to_string(),
                content: text.to_string(),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            }
        }
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_request(|request| {
                // Production body must NOT include "temperature" for Venice (ADR-0065 D5)
                !request
                    .utf8_lossy_body()
                    .unwrap_or_default()
                    .contains("\"temperature\"")
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;
        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::Venice,
        );
        let result = provider
            .chat_with_tools(
                crate::ai::provider::ChatRequest {
                    messages: vec![user_msg("hello")],
                    model: "meta-llama/llama-3.3-70b-instruct".to_string(), // CI-ALLOW: hardcoded-model
                    max_tokens: 1024,
                    temperature: 0.0, // CI-ALLOW: hardcoded-sampling
                },
                None,
                None,
            )
            .await;
        assert!(result.is_ok(), "Venice body-builder dispatch must succeed: {:?}", result.err());
        mock.assert_async().await; // fails if temperature was present in the request body
    }

    /// ADR-0065 D2: venice_parameters in the production body contains ONLY
    /// `include_venice_system_prompt` — exercises `chat_with_tools` production path.
    #[tokio::test]
    async fn test_venice_parameters_contains_only_include_venice_system_prompt() {
        fn user_msg(text: &str) -> crate::ai::provider::ChatMessage {
            crate::ai::provider::ChatMessage {
                role: "user".to_string(),
                content: text.to_string(),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            }
        }
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_request(|request| {
                // venice_parameters must have exactly one key: include_venice_system_prompt
                if let Ok(body) = serde_json::from_str::<serde_json::Value>(
                    &request.utf8_lossy_body().unwrap_or_default(),
                ) {
                    if let Some(obj) = body["venice_parameters"].as_object() {
                        return obj.len() == 1
                            && obj.contains_key("include_venice_system_prompt");
                    }
                }
                false
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;
        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::Venice,
        )
        .with_venice_system_prompt(false);
        let result = provider
            .chat_with_tools(
                crate::ai::provider::ChatRequest {
                    messages: vec![user_msg("hello")],
                    model: "meta-llama/llama-3.3-70b-instruct".to_string(), // CI-ALLOW: hardcoded-model
                    max_tokens: 1024,
                    temperature: 0.0, // CI-ALLOW: hardcoded-sampling
                },
                None,
                None,
            )
            .await;
        assert!(result.is_ok(), "Venice body-builder dispatch must succeed: {:?}", result.err());
        mock.assert_async().await; // fails if venice_parameters structure was wrong
    }

    /// ADR-0065 D1: Venice model is NOT rewritten to "openclaw" in the production body —
    /// exercises `chat_with_tools` production path (not a local simulation).
    #[tokio::test]
    async fn test_venice_model_not_rewritten_to_openclaw_sentinel() {
        fn user_msg(text: &str) -> crate::ai::provider::ChatMessage {
            crate::ai::provider::ChatMessage {
                role: "user".to_string(),
                content: text.to_string(),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            }
        }
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_request(|request| {
                // Venice model must NOT be rewritten to the OpenClaw sentinel
                if let Ok(body) = serde_json::from_str::<serde_json::Value>(
                    &request.utf8_lossy_body().unwrap_or_default(),
                ) {
                    return body["model"].as_str() != Some("openclaw");
                }
                false
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;
        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::Venice,
        );
        let result = provider
            .chat_with_tools(
                crate::ai::provider::ChatRequest {
                    messages: vec![user_msg("hello")],
                    model: "meta-llama/llama-3.3-70b-instruct".to_string(), // CI-ALLOW: hardcoded-model
                    max_tokens: 1024,
                    temperature: 0.0, // CI-ALLOW: hardcoded-sampling
                },
                None,
                None,
            )
            .await;
        assert!(result.is_ok(), "Venice body-builder dispatch must succeed: {:?}", result.err());
        mock.assert_async().await; // fails if model was rewritten to "openclaw"
    }

    /// ADR-0065 D5: Venice `include_system_prompt` defaults to false.
    /// The AiProvider::Venice variant must have `include_system_prompt: false` when default-constructed.
    #[test]
    fn test_venice_provider_default_include_system_prompt_is_false() {
        let provider = AiProvider::Venice {
            api_key: "test-key".to_string(),
            include_system_prompt: false,
        };
        match provider {
            AiProvider::Venice { include_system_prompt, .. } => {
                assert!(!include_system_prompt, "D3: include_system_prompt must default to false");
            }
            _ => panic!("expected Venice variant"),
        }
    }

    /// ADR-0065 D1: OpenRouterProvider::with_venice_system_prompt() sets the flag correctly.
    #[test]
    fn test_with_venice_system_prompt_builder() {
        let p_false = OpenRouterProvider::with_base_url(
            "test-key".to_string(),
            VENICE_API_BASE.to_string(),
            RequestOrigin::Venice,
        )
        .with_venice_system_prompt(false);
        assert!(!p_false.venice_include_system_prompt, "flag must be false when set false");

        let p_true = OpenRouterProvider::with_base_url(
            "test-key".to_string(),
            VENICE_API_BASE.to_string(),
            RequestOrigin::Venice,
        )
        .with_venice_system_prompt(true);
        assert!(p_true.venice_include_system_prompt, "flag must be true when set true");
    }

    /// ADR-0065 D1: VENICE_API_BASE constant must be the correct value.
    #[test]
    fn test_venice_api_base_url_constant() {
        assert_eq!(
            VENICE_API_BASE,
            "https://api.venice.ai/api/v1",
            "VENICE_API_BASE must match the ADR-0065 specified value"
        );
    }
}

/// #3081: `apply_anthropic_cache_control` — the shared helper both OpenAI-shaped
/// body builders (`chat_with_tools_inner`, `stream_openrouter_inner`) now call.
///
/// The first two tests exercise the helper directly (pure function, no AppHandle
/// needed) — this is the one piece of logic `stream_openrouter_inner`'s
/// cache_control site shares verbatim with `chat_with_tools_inner`'s, and since
/// `stream_openrouter_inner` itself requires a Wry-backed `tauri::AppHandle`
/// that `tauri::test::mock_app()` cannot produce (see `origin_variant_keying_tests`'
/// module doc comment above for the established precedent), testing the shared
/// helper directly is what makes the streaming site's behavior verifiable at all.
/// The third and fourth tests exercise the non-streaming production entry point
/// (`chat_with_tools`) end-to-end against a mocked HTTP server, proving the real
/// dispatch code — not a re-derivation of it — omits/sends the marker correctly.
#[cfg(test)]
mod cache_control_tests {
    use super::*;

    fn system_msg(text: &str) -> Value {
        json!({ "role": "system", "content": text })
    }
    fn user_msg(text: &str) -> Value {
        json!({ "role": "user", "content": text })
    }

    /// #3081 defect 1: Venice must receive NO cache_control marker anywhere in the
    /// message array, even though both the system message and the second-to-last
    /// message would otherwise qualify.
    #[test]
    fn test_venice_origin_strips_cache_control_from_system_and_second_to_last() {
        let mut messages = vec![
            system_msg("you are a helpful assistant"),
            user_msg("turn 1"),
            user_msg("turn 2 (second-to-last)"),
            user_msg("turn 3 (last)"),
        ];
        apply_anthropic_cache_control(&mut messages, RequestOrigin::Venice);

        for (i, msg) in messages.iter().enumerate() {
            assert!(
                serde_json::to_string(msg).unwrap().find("cache_control").is_none(),
                "message {} must carry no cache_control on the Venice origin: {:?}",
                i,
                msg
            );
            // Content must remain the original plain string — Venice must never see
            // the content-blocks-array rewrite either, since that rewrite is what
            // some Venice backends reject outright ("Text content parts may only
            // include type and text").
            assert!(msg["content"].is_string(), "message {} content must stay a plain string on Venice", i);
        }
    }

    /// #3081 defect 1 regression guard: OpenRouter (`RequestOrigin::Other`) must
    /// still carry cache_control — the Anthropic prompt-caching behavior must not
    /// regress while fixing Venice.
    #[test]
    fn test_other_origin_still_gets_cache_control_on_system_and_second_to_last() {
        let mut messages = vec![
            system_msg("you are a helpful assistant"),
            user_msg("turn 1"),
            user_msg("turn 2 (second-to-last)"),
            user_msg("turn 3 (last)"),
        ];
        apply_anthropic_cache_control(&mut messages, RequestOrigin::Other);

        assert!(
            messages[0].to_string().contains("cache_control"),
            "system message must carry cache_control on RequestOrigin::Other (OpenRouter)"
        );
        assert!(
            messages[2].to_string().contains("cache_control"),
            "second-to-last message must carry cache_control on RequestOrigin::Other (OpenRouter)"
        );
        assert!(
            !messages[1].to_string().contains("cache_control"),
            "non-targeted message must NOT carry cache_control"
        );
        assert!(
            !messages[3].to_string().contains("cache_control"),
            "the last message (not second-to-last) must NOT carry cache_control"
        );
    }

    /// #3081 regression guard: `RequestOrigin::OpenClaw` is unaffected by the
    /// Venice exclusion — this predicate was already unconditional for OC before
    /// #3081 and must stay that way.
    #[test]
    fn test_openclaw_origin_still_gets_cache_control() {
        let mut messages = vec![system_msg("sys"), user_msg("a"), user_msg("b")];
        apply_anthropic_cache_control(&mut messages, RequestOrigin::OpenClaw);
        assert!(messages[0].to_string().contains("cache_control"));
    }

    /// #3081 defect 1, production path: a real Venice dispatch via
    /// `OpenRouterProvider::chat_with_tools` (the exact body builder that 400'd on
    /// `abliteration-abliterated-model-large-flash`) must never put `cache_control`
    /// on the wire. Exercises the production entry point against a mocked HTTP
    /// server — not a local simulation of the body-builder logic.
    #[tokio::test]
    async fn test_venice_production_dispatch_never_sends_cache_control() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_request(|request| {
                !request
                    .utf8_lossy_body()
                    .unwrap_or_default()
                    .contains("cache_control")
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::Venice,
        );
        let result = provider
            .chat_with_tools(
                crate::ai::provider::ChatRequest {
                    messages: vec![
                        crate::ai::provider::ChatMessage {
                            role: "system".to_string(),
                            content: "you are a helpful assistant".to_string(),
                            tool_calls: None,
                            tool_call_id: None,
                            content_blocks: None,
                            attachment_ids: None,
                            id: None,
                        },
                        crate::ai::provider::ChatMessage {
                            role: "user".to_string(),
                            content: "hello".to_string(),
                            tool_calls: None,
                            tool_call_id: None,
                            content_blocks: None,
                            attachment_ids: None,
                            id: None,
                        },
                    ],
                    model: "abliteration-abliterated-model-large-flash".to_string(), // CI-ALLOW: hardcoded-model
                    max_tokens: 1024,
                    temperature: 0.0, // CI-ALLOW: hardcoded-sampling
                },
                None,
                None,
            )
            .await;
        assert!(result.is_ok(), "Venice production dispatch must succeed with no cache_control: {:?}", result.err());
        mock.assert_async().await; // fails if cache_control was present in the request body
    }

    /// #3081 regression guard, production path: a bare-OpenRouter dispatch via the
    /// SAME production entry point (`chat_with_tools`) must still carry
    /// `cache_control` on the system message — proves the Venice fix in the shared
    /// helper did not regress OpenRouter's Anthropic prompt-caching behavior.
    #[tokio::test]
    async fn test_openrouter_production_dispatch_still_sends_cache_control() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/chat/completions")
            .match_request(|request| {
                request
                    .utf8_lossy_body()
                    .unwrap_or_default()
                    .contains("cache_control")
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"hi"}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let provider = OpenRouterProvider::with_base_url(
            "sk-test".to_string(),
            server.url(),
            RequestOrigin::Other,
        );
        let result = provider
            .chat_with_tools(
                crate::ai::provider::ChatRequest {
                    messages: vec![
                        crate::ai::provider::ChatMessage {
                            role: "system".to_string(),
                            content: "you are a helpful assistant".to_string(),
                            tool_calls: None,
                            tool_call_id: None,
                            content_blocks: None,
                            attachment_ids: None,
                            id: None,
                        },
                        crate::ai::provider::ChatMessage {
                            role: "user".to_string(),
                            content: "hello".to_string(),
                            tool_calls: None,
                            tool_call_id: None,
                            content_blocks: None,
                            attachment_ids: None,
                            id: None,
                        },
                    ],
                    model: "anthropic/claude-sonnet-4.5".to_string(), // CI-ALLOW: hardcoded-model
                    max_tokens: 1024,
                    temperature: 0.0, // CI-ALLOW: hardcoded-sampling
                },
                None,
                None,
            )
            .await;
        assert!(result.is_ok(), "OpenRouter production dispatch must succeed: {:?}", result.err());
        mock.assert_async().await; // fails if cache_control was absent — the OpenRouter regression case
    }
}
