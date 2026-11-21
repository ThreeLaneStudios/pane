/// Context Budget Manager — §27 Context Budget Manager Spec
/// 
/// Budget-aware context assembly with priority tiers.
/// Ensures token budgets are enforced before sending to AI providers.

use crate::ai::provider::ChatMessage;
use std::sync::Mutex;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

// ==================== Backend enum (§27.9, Q3) ====================
// Lightweight routing discriminant. Does NOT import AiProvider (which carries
// api_key/token fields). The single allowed reference is from_ai_provider's
// argument type, written as a fully-qualified path.

/// Routing discriminant for get_context_window. Mirrors AiProvider variants but
/// carries only what the resolver needs (no secrets).
/// Provider name strings match ModelInfo.provider from fetch_*_models:
///   Anthropic → "anthropic", OpenRouter → "openrouter",
///   Ollama → "ollama", OpenClaw → "openclaw", Unknown → "unknown".
#[derive(Debug, Clone)]
pub enum Backend {
    Anthropic,
    OpenRouter,
    /// Carries the Ollama endpoint URL for the lazy /api/show lookup.
    Ollama { endpoint: String },
    OpenClaw,
    Unknown,
}

impl Backend {
    /// Convert from AiProvider without importing it at module level.
    /// This is the ONLY place in context_budget.rs that references AiProvider.
    pub fn from_ai_provider(p: &crate::ai::provider::AiProvider) -> Self {
        match p {
            crate::ai::provider::AiProvider::Anthropic { .. } => Backend::Anthropic,
            crate::ai::provider::AiProvider::OpenRouter { .. } => Backend::OpenRouter,
            crate::ai::provider::AiProvider::Ollama { endpoint } => Backend::Ollama { endpoint: endpoint.clone() },
            crate::ai::provider::AiProvider::OpenClaw { .. } => Backend::OpenClaw,
            // ADR-0065 D1: Venice uses the OpenRouter-shaped backend for context-budget purposes.
            crate::ai::provider::AiProvider::Venice { .. } => Backend::OpenRouter,
            crate::ai::provider::AiProvider::None => Backend::Unknown,
        }
    }

    /// Stable provider name string matching ModelInfo.provider convention.
    pub fn provider_name(&self) -> &'static str {
        match self {
            Backend::Anthropic => "anthropic",
            Backend::OpenRouter => "openrouter",
            Backend::Ollama { .. } => "ollama",
            Backend::OpenClaw => "openclaw",
            Backend::Unknown => "unknown",
        }
    }
}

/// State tracking for cache prefix and message windowing (§27.5)
struct WindowState {
    last_prefix_hash: u64,
    last_included_count: usize,
    last_total_count: usize,  // Total messages in DB last time
}

/// Injectable budget state container — replaces global statics
/// Managed by Tauri, tests create their own instances
///
/// `window` (Reed's audit, 2026-10-30, PR #3122 FAIL #2; Matt's ruling:
/// session-keyed window) — keyed by session id, one `WindowState` per
/// session, instead of a single app-wide slot shared by every session and
/// job dispatch. See `apply_budget`'s own doc comment for the mechanism
/// and the defect it closes.
pub struct BudgetState {
    window: Mutex<HashMap<String, WindowState>>,
    component_hashes: Mutex<Option<ComponentHashes>>,
    latest_stats: Mutex<Option<BudgetResult>>,
}

impl BudgetState {
    pub fn new() -> Self {
        Self {
            window: Mutex::new(HashMap::new()),
            component_hashes: Mutex::new(None),
            latest_stats: Mutex::new(None),
        }
    }
}

/// Compute hash of the cache prefix (tiers 1-5: system + tools + spatial + summary)
fn compute_prefix_hash(
    system_prompt: &str,
    tools_json: &str,
    spatial_context: &str,
    compaction_summary: Option<&str>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    system_prompt.hash(&mut hasher);
    tools_json.hash(&mut hasher);
    spatial_context.hash(&mut hasher);
    if let Some(summary) = compaction_summary {
        summary.hash(&mut hasher);
    }
    hasher.finish()
}

/// State to track individual component hashes for change detection
struct ComponentHashes {
    system: u64,
    tools: u64,
    spatial: u64,
    summary: u64,
}

/// Detect why the cache prefix changed by comparing individual components
fn detect_prefix_change_reason(
    state: &BudgetState,
    system_prompt: &str,
    tools_json: &str,
    spatial_context: &str,
    compaction_summary: Option<&str>,
) -> String {
    let mut hasher = DefaultHasher::new();
    system_prompt.hash(&mut hasher);
    let system_hash = hasher.finish();
    
    let mut hasher = DefaultHasher::new();
    tools_json.hash(&mut hasher);
    let tools_hash = hasher.finish();
    
    let mut hasher = DefaultHasher::new();
    spatial_context.hash(&mut hasher);
    let spatial_hash = hasher.finish();
    
    let mut hasher = DefaultHasher::new();
    if let Some(summary) = compaction_summary {
        summary.hash(&mut hasher);
    }
    let summary_hash = hasher.finish();
    
    let mut prev_hashes = state.component_hashes.lock().unwrap_or_else(|e| e.into_inner());
    
    let reason = if let Some(ref prev) = *prev_hashes {
        // Determine which component changed
        if summary_hash != prev.summary {
            "compaction"
        } else if spatial_hash != prev.spatial {
            "navigation"
        } else if system_hash != prev.system {
            "memory_update"
        } else if tools_hash != prev.tools {
            "system_change"
        } else {
            // All components match - prefix is stable
            "stable"
        }
    } else {
        "initial"
    };
    
    // Store current hashes for next comparison
    *prev_hashes = Some(ComponentHashes {
        system: system_hash,
        tools: tools_hash,
        spatial: spatial_hash,
        summary: summary_hash,
    });
    
    reason.to_string()
}

/// Token estimation ratio per provider
#[derive(Debug, Clone, Copy)]
pub struct TokenEstimationRatio {
    pub chars_per_token: f32,
}

impl TokenEstimationRatio {
    /// Get the appropriate ratio for a given model name
    pub fn for_model(model: &str) -> Self {
        let model_lower = model.to_lowercase();
        
        // Claude models: 3.5 chars/token
        if model_lower.contains("claude") || model_lower.contains("opus") || 
           model_lower.contains("sonnet") || model_lower.contains("haiku") {
            return Self { chars_per_token: 3.5 };
        }
        
        // GPT models: 4.0 chars/token
        if model_lower.contains("gpt") || model_lower.contains("openai") {
            return Self { chars_per_token: 4.0 };
        }
        
        // OpenClaw: use conservative estimate (delegates to unknown model)
        if model_lower.contains(crate::ai::provider::PROVIDER_OPENCLAW) {
            return Self { chars_per_token: 3.5 };
        }
        
        // Ollama/unknown: conservative 3.5
        Self { chars_per_token: 3.5 }
    }
    
    /// Estimate tokens from character count
    pub fn estimate_tokens(&self, text: &str) -> usize {
        let chars = text.chars().count() as f32;
        (chars / self.chars_per_token).ceil() as usize
    }
}

/// Static seed table — private helper extracted from the old get_context_window.
/// Returns Some(tokens) on a known substring match, None if unknown.
/// Values are intentionally preserved as-is from the pre-refactor table.
fn seed_lookup(model_id: &str) -> Option<u64> {
    let m = model_id.to_lowercase();
    // OpenClaw gateway models
    if m.contains("openclaw:main") { return Some(200_000); }
    // Claude models: 200K
    if m.contains("opus") || m.contains("claude-opus") { return Some(200_000); }
    if m.contains("sonnet") || m.contains("claude-sonnet") { return Some(200_000); }
    if m.contains("haiku") { return Some(200_000); }
    // GPT models
    if m.contains("gpt-4o") || m.contains("gpt-4") { return Some(128_000); }
    None
}

/// Single authoritative context-window resolver. G5 anchor — do NOT inline.
///
/// Precedence chain (§27.9, ADR-0043 Decision #1):
///   1. OpenClaw wire value (non-overridable — relayed by Pane Gateway)
///   2. User override (context_window_override setting, clamped + floored)
///   3. Provider API value (OpenRouter wire, Ollama /api/show, OpenClaw wire)
///   4. Static seed table (logged as estimated)
///   5. 128 k floor
///
/// `context_length` and `override_tokens` are pre-resolved by the caller;
/// this function is pure precedence logic + Ollama HTTP (branch #3).
pub async fn get_context_window(
    backend: &Backend,
    model_id: &str,
    context_length: Option<u64>,    // from ModelRegistry lookup at callsite
    override_tokens: Option<u64>,   // from settings read at callsite
) -> u64 {
    // --- precedence #1: OpenClaw wire value (non-overridable) ---
    if matches!(backend, Backend::OpenClaw) {
        if let Some(oc_val) = context_length {
            return oc_val;
        }
    }

    // --- precedence #3: provider API value (computed before applying override so we
    //     can clamp the override to api_max when both are known) ---
    let api_max: Option<u64> = match backend {
        Backend::OpenClaw => context_length,   // already returned above if Some; None falls through
        Backend::OpenRouter => context_length,
        Backend::Ollama { endpoint } => {
            crate::ai::connections::get_ollama_context_window(endpoint, model_id).await
        }
        Backend::Anthropic | Backend::Unknown => None,
    };

    // --- precedence #2: user override (clamp to api_max; floor at 50_000) ---
    if let Some(raw_override) = override_tokens {
        let floored = raw_override.max(50_000);
        let (clamped, note) = if let Some(max) = api_max {
            if floored > max {
                (max, format!("clamped to api_max={}", max))
            } else {
                (floored, "applied".to_string())
            }
        } else {
            if raw_override < 50_000 {
                (floored, format!("floored from {} to 50_000", raw_override))
            } else {
                (floored, "applied".to_string())
            }
        };
        log::warn!(
            "[CONTEXT-WINDOW] override {}: {} (raw={}) for {}/{}",
            note, clamped, raw_override, backend.provider_name(), model_id
        );
        eprintln!("[CONTEXT-BUDGET] override_applied raw={} clamped={} note={} model={}",
            raw_override, clamped, note, model_id);
        return clamped;
    }

    // --- precedence #3 hit (no override) ---
    if let Some(api_val) = api_max {
        return api_val;
    }

    // --- Anthropic-specific precedence-3 miss log (Q2) ---
    if matches!(backend, Backend::Anthropic) {
        // Announce the miss before falling to seed so logs are ordered correctly.
        // The seed value (if any) is logged immediately after.
        if let Some(seed) = seed_lookup(model_id) {
            log::warn!(
                "[CONTEXT-WINDOW] anthropic precedence-3 miss (no API source), seed {} for {}",
                seed, model_id
            );
            log::warn!(
                "[CONTEXT-WINDOW] estimated seed value {} for {}/{} — no live API source",
                seed, backend.provider_name(), model_id
            );
            return seed;
        }
    }

    // --- precedence #4: static seed table (all other backends) ---
    if let Some(seed) = seed_lookup(model_id) {
        log::warn!(
            "[CONTEXT-WINDOW] estimated seed value {} for {}/{} — no live API source",
            seed, backend.provider_name(), model_id
        );
        return seed;
    }

    // --- precedence #5: 128k floor ---
    log::warn!(
        "[CONTEXT-WINDOW] no source found for {}/{}, using 128k floor",
        backend.provider_name(), model_id
    );
    128_000
}

/// Lightweight synchronous context-window estimate for call sites that cannot use async
/// (e.g. the compaction budget estimation in commands.rs — full G5 refactor is #1405-G).
/// Uses the same seed table + 128k floor as the async resolver's precedence #4/#5 path.
/// Does NOT consult the ModelRegistry or live provider APIs.
// Retained for synchronous legacy call sites.
#[allow(dead_code)]
pub fn context_window_estimate(model_id: &str) -> usize {
    seed_lookup(model_id).unwrap_or(128_000) as usize
}

/// Budget breakdown for a single tier
#[derive(Debug, Clone, serde::Serialize)]
pub struct TierBudget {
    pub tier: u8,
    pub name: String,
    pub tokens: usize,
}

/// Wraps the resolver output with an observability flag classifying the value's source.
/// Returned by `resolve_context_window_with_source`; only the `get_context_budget_stats`
/// call site uses this — `apply_budget` / `maybe_compact_thread` continue using `u64`.
pub struct ResolvedContextWindow {
    /// The resolved context window in tokens (identical to what `get_context_window` returns).
    pub value: u64,
    /// `true` when the value came from the static seed table (precedence 4) or the 128k
    /// floor (precedence 5); `false` when from OpenClaw wire (1), user override (2), or
    /// provider API (3). Used by the frontend to render the "(estimated)" badge.
    pub is_estimated: bool,
}

/// Resolver wrapper that returns both the context window value and whether it is estimated.
///
/// Wraps `get_context_window` (unchanged) so existing call sites (`apply_budget`,
/// `maybe_compact_thread`) require zero churn. Estimated = precedence 4 (seed) or 5 (128k
/// floor); real = precedence 1 (OC wire), 2 (user override), or 3 (provider API).
///
/// Ollama (Backend::Ollama) is conservatively classified as NOT estimated because
/// `/api/show` is reliable when reachable; a silent HTTP failure falls to seed/floor
/// but cannot be distinguished here without a second HTTP call. This is an acceptable
/// approximation — the displayed value is always correct; only the badge may be absent.
pub async fn resolve_context_window_with_source(
    backend: &Backend,
    model_id: &str,
    context_length: Option<u64>,
    override_tokens: Option<u64>,
) -> ResolvedContextWindow {
    let value = get_context_window(backend, model_id, context_length, override_tokens).await;

    let is_estimated = match backend {
        // Precedence 1: OpenClaw with a real wire value — not estimated.
        Backend::OpenClaw if context_length.is_some() => false,
        // Precedence 2: user override applied — not estimated.
        _ if override_tokens.is_some() => false,
        // Precedence 3: OpenRouter with a registry-sourced context_length — not estimated.
        Backend::OpenRouter if context_length.is_some() => false,
        // Precedence 3: Ollama — /api/show provides a real value when reachable;
        // conservatively treat as not estimated (see doc comment above).
        Backend::Ollama { .. } => false,
        // All remaining cases fall to seed table (precedence 4) or 128k floor (5) → estimated.
        _ => true,
    };

    ResolvedContextWindow { value, is_estimated }
}

/// Update the `is_estimated` flag on the latest stored budget result.
///
/// Called by the `get_context_budget_stats` call site in `commands.rs` immediately after
/// `apply_budget` stores its result, so the Health dashboard reflects the resolver's
/// source classification without changing `apply_budget`'s signature.
pub fn update_budget_is_estimated(state: &BudgetState, is_estimated: bool) {
    let mut latest_stats = state.latest_stats.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref mut stats) = *latest_stats {
        stats.is_estimated = is_estimated;
    }
}

/// Complete budget calculation result
#[derive(Debug, Clone, serde::Serialize)]
pub struct BudgetResult {
    pub model: String,
    pub context_window: usize,
    pub response_reserve: usize,
    pub available_budget: usize,
    pub tiers: Vec<TierBudget>,
    pub total_used: usize,
    pub messages_included: usize,
    pub messages_total: usize,
    pub images_included: usize,
    pub images_replaced: usize,
    pub cache_prefix_tokens: usize,
    pub cache_prefix_stable: bool,
    pub cache_prefix_change_reason: String,
    /// Precise tokens_demanded — display/observability surface (Health dashboard, future).
    /// NOT consumed by the compaction trigger (which uses a coarse pre-budget estimate
    /// in maybe_compact_thread). Crossing these two consumers re-introduces the
    /// timing bug Reed §2.5 caught in the D scope brief.
    pub tokens_demanded: usize,
    /// `true` when the context window value came from the static seed table (precedence 4)
    /// or the 128k floor (precedence 5). Set post-apply by `update_budget_is_estimated`
    /// at the `commands.rs` call site. Defaults to `false` (safe for test callers that
    /// do not invoke `resolve_context_window_with_source`).
    pub is_estimated: bool,
}

impl BudgetResult {
    /// Format budget log lines per §27.8.1
    pub fn log(&self) {
        eprintln!(
            "[CONTEXT-BUDGET] model={} window={} available={}",
            self.model, self.context_window, self.available_budget
        );
        
        // Build tier summary line
        let tier_summary = self.tiers.iter()
            .map(|t| format!("T{}-{}: {}", t.tier, t.name, t.tokens))
            .collect::<Vec<_>>()
            .join(" | ");
        
        eprintln!("[CONTEXT-BUDGET] {}", tier_summary);
        
        eprintln!(
            "[CONTEXT-BUDGET] T6-messages: {}/{} included ({} tokens) | T7-images: {} included, {} replaced",
            self.messages_included,
            self.messages_total,
            self.tiers.iter().find(|t| t.tier == 6).map(|t| t.tokens).unwrap_or(0),
            self.images_included,
            self.images_replaced
        );
        
        let usage_pct = (self.total_used as f32 / self.available_budget as f32 * 100.0).round();
        
        // §27.5: Cache prefix stability logging
        let cache_status = if self.cache_prefix_stable {
            format!("cache_prefix: {} tokens (stable — identical to previous request)", self.cache_prefix_tokens)
        } else if self.cache_prefix_change_reason == "initial" {
            format!("cache_prefix: {} tokens (initial request)", self.cache_prefix_tokens)
        } else {
            format!("cache_prefix: {} tokens (changed — {})", self.cache_prefix_tokens, self.cache_prefix_change_reason)
        };
        
        eprintln!(
            "[CONTEXT-BUDGET] total: {}/{} ({:.1}%) | {}",
            self.total_used, self.available_budget, usage_pct, cache_status
        );
    }
}

/// Estimate image token cost using tile-based calculation
/// Per §27.2.2: Images are resized to 1568×1568, divided into 768×768 tiles
// Retained as a deterministic image-budget test seam.
#[allow(dead_code)]
pub fn estimate_image_tokens(width: u32, height: u32) -> usize {
    // Resize to fit within 1568×1568 maintaining aspect ratio
    let max_dim = 1568.0;
    let scale = if width as f32 > max_dim || height as f32 > max_dim {
        max_dim / (width.max(height) as f32)
    } else {
        1.0
    };
    
    let scaled_width = (width as f32 * scale) as u32;
    let scaled_height = (height as f32 * scale) as u32;
    
    // Divide into 768×768 tiles
    let tile_size = 768;
    let tiles_x = (scaled_width + tile_size - 1) / tile_size;
    let tiles_y = (scaled_height + tile_size - 1) / tile_size;
    let total_tiles = tiles_x * tiles_y;
    
    // Each tile ~800 tokens + 100 base
    (total_tiles * 800 + 100) as usize
}

/// Conservative estimate for images without dimensions
pub fn estimate_image_tokens_conservative() -> usize {
    // Typical screenshot ~1200×800 = ~1,700 tokens
    1700
}

/// Apply budget trimming to messages
/// Returns (trimmed_messages, budget_result)
///
/// `context_window` is pre-resolved by the caller via `get_context_window(...).await`
/// (see #1402-C). `apply_budget` remains sync; async lives only in the resolver.
///
/// `session_id` (Reed's audit, 2026-10-30, PR #3122 FAIL #2; Matt's ruling:
/// session-keyed window) — `BudgetState.window`'s §27.5.2 append-only
/// cache-continuity state was a single app-wide slot, keyed only on the
/// `{system_prompt, tools_json, spatial_context, compaction_summary}`
/// prefix hash, with no session identifier anywhere in it. Job dispatch
/// uses an invariant, unparameterized system prompt for every job — so a
/// second job's call saw the SAME "stable" prefix hash as the first job's
/// unrelated prior call, took the append-only branch
/// (`target_message_count = previous_included_count + new_message_count`,
/// arithmetic inherited from a different job's bookkeeping), and got
/// handed every one of its own messages with zero re-check against its
/// own real token budget — Reed's probe measured a 476% overflow. Every
/// caller now passes the session id whose conversation the message array
/// actually represents, and `window` is keyed by it: the prefix-hash
/// comparison that decides "stable vs. changed" happens PER SESSION ENTRY,
/// not against one shared slot, so two sessions (or two job dispatches on
/// different sessions) with byte-identical prefixes each get their own
/// independent windowing continuity instead of inheriting each other's.
pub fn apply_budget(
    state: &BudgetState,
    session_id: &str,
    model: &str,
    context_window: u64,
    system_prompt: &str,
    tools_json: &str,
    spatial_context: &str,
    compaction_summary: Option<&str>,
    messages: Vec<ChatMessage>,
    image_retention_messages: usize,
) -> (Vec<ChatMessage>, BudgetResult) {
    let ratio = TokenEstimationRatio::for_model(model);
    let context_window = context_window as usize;
    
    // Calculate budget per §27.2.1
    let response_reserve = std::cmp::min(4096, (context_window as f32 * 0.05) as usize);
    let available_budget = context_window - response_reserve;
    
    let mut remaining = available_budget;
    let mut tiers = Vec::new();
    
    // T1: System files (system_prompt includes SOUL, IDENTITY, USER, TOOLS, AGENTS)
    let t1_tokens = ratio.estimate_tokens(system_prompt);
    tiers.push(TierBudget {
        tier: 1,
        name: "system".to_string(),
        tokens: t1_tokens,
    });
    remaining = remaining.saturating_sub(t1_tokens);
    
    // T2: Memory (included in system_prompt, counted separately for logging)
    // For now, estimate ~6,000 tokens (will be refined when we parse system_prompt structure)
    // This is already counted in T1, so we don't subtract from remaining
    let t2_tokens = 6_000; // Estimate, included in system_prompt
    tiers.push(TierBudget {
        tier: 2,
        name: "memory".to_string(),
        tokens: t2_tokens,
    });
    
    // T3: Tools
    let t3_tokens = ratio.estimate_tokens(tools_json);
    tiers.push(TierBudget {
        tier: 3,
        name: "tools".to_string(),
        tokens: t3_tokens,
    });
    remaining = remaining.saturating_sub(t3_tokens);
    
    // T4: Compaction summary
    let t4_tokens = if let Some(summary) = compaction_summary {
        ratio.estimate_tokens(summary)
    } else {
        0
    };
    if t4_tokens > 0 {
        tiers.push(TierBudget {
            tier: 4,
            name: "summary".to_string(),
            tokens: t4_tokens,
        });
        remaining = remaining.saturating_sub(t4_tokens);
    }
    
    // T5: Spatial context
    let t5_tokens = ratio.estimate_tokens(spatial_context);
    tiers.push(TierBudget {
        tier: 5,
        name: "spatial".to_string(),
        tokens: t5_tokens,
    });
    remaining = remaining.saturating_sub(t5_tokens);
    
    // === §27.5: Cache-Aware Message Windowing ===
    // Compute prefix hash and check if it changed
    let current_prefix_hash = compute_prefix_hash(
        system_prompt,
        tools_json,
        spatial_context,
        compaction_summary,
    );
    
    // Detect reason for any prefix change (updates component hashes internally)
    let change_reason = detect_prefix_change_reason(
        state,
        system_prompt,
        tools_json,
        spatial_context,
        compaction_summary,
    );
    
    // Session-keyed (Reed's audit, PR #3122 FAIL #2; Matt's ruling): the
    // prefix-hash "stable vs. changed" comparison reads/writes THIS
    // session's own map entry only — never another session's (or another
    // job dispatch's) leftover windowing state.
    let mut window_map = state.window.lock().unwrap_or_else(|e| e.into_inner());
    let (prefix_changed, previous_included_count, previous_total_count) = if let Some(session_window) = window_map.get(session_id) {
        let changed = session_window.last_prefix_hash != current_prefix_hash;
        (changed, session_window.last_included_count, session_window.last_total_count)
    } else {
        // First request ever for this session
        (true, 0, 0)
    };
    
    // T6: Chat messages — append-only windowing (§27.5.2)
    // Minimum guarantee: last 6 exchanges (12 messages)
    let min_messages = std::cmp::min(12, messages.len());
    let mut messages_to_include = Vec::new();
    let mut t6_tokens = 0;
    let mut images_included = 0;
    let mut images_replaced = 0;
    
    // Determine how many messages to include
    let target_message_count = if prefix_changed || previous_included_count == 0 {
        // Prefix changed (compaction/navigation) or first request: use newest-first scan
        // Scan from newest to oldest to find how many fit
        let mut count = 0;
        let mut budget_remaining = remaining;
        
        for (idx, msg) in messages.iter().rev().enumerate() {
            let msg_tokens = ratio.estimate_tokens(&msg.content);
            let image_tokens = if let Some(ref att_ids) = msg.attachment_ids {
                let attachment_count = att_ids.as_str().split(',').filter(|s: &&str| !s.trim().is_empty()).count();
                if idx < image_retention_messages {
                    attachment_count * estimate_image_tokens_conservative()
                } else {
                    0
                }
            } else {
                0
            };
            
            let total_msg_tokens = msg_tokens + image_tokens;
            
            if idx < min_messages || total_msg_tokens <= budget_remaining {
                count += 1;
                budget_remaining = budget_remaining.saturating_sub(total_msg_tokens);
            } else {
                break;
            }
        }
        
        count
    } else {
        // Prefix stable: append-only windowing (§27.5.2)
        // Maintain the same starting point (same number of messages were excluded)
        // but include any new messages that were added
        let new_message_count = messages.len().saturating_sub(previous_total_count);
        previous_included_count + new_message_count
    };
    
    // Now collect the messages (newest N messages)
    let messages_to_skip = messages.len().saturating_sub(target_message_count);
    
    for (idx, msg) in messages.iter().skip(messages_to_skip).enumerate() {
        let msg_tokens = ratio.estimate_tokens(&msg.content);
        
        // Calculate position from end for image retention
        let position_from_end = target_message_count - idx - 1;
        let image_tokens = if let Some(ref att_ids) = msg.attachment_ids {
            let attachment_count = att_ids.as_str().split(',').filter(|s: &&str| !s.trim().is_empty()).count();
            if position_from_end < image_retention_messages {
                images_included += attachment_count;
                attachment_count * estimate_image_tokens_conservative()
            } else {
                images_replaced += attachment_count;
                0
            }
        } else {
            0
        };
        
        let total_msg_tokens = msg_tokens + image_tokens;
        messages_to_include.push(msg.clone());
        t6_tokens += total_msg_tokens;
    }
    
    // Update THIS session's window state for its next request — never
    // overwrites another session's entry in the map.
    window_map.insert(session_id.to_string(), WindowState {
        last_prefix_hash: current_prefix_hash,
        last_included_count: messages_to_include.len(),
        last_total_count: messages.len(),
    });
    
    tiers.push(TierBudget {
        tier: 6,
        name: "messages".to_string(),
        tokens: t6_tokens,
    });
    
    // Calculate total used and cache prefix
    // Cache prefix = T1 + T2 + T3 + T4 (everything before messages)
    let cache_prefix_tokens = tiers.iter()
        .filter(|t| t.tier <= 4)
        .map(|t| t.tokens)
        .sum();
    
    let total_used = tiers.iter().map(|t| t.tokens).sum();
    
    // Compute tokens_demanded: fixed tiers + all message tokens (included + excluded).
    // T2 is intentionally excluded from the fixed sum — it is already counted inside T1.
    // Excluded messages were not tokenized during the scan (Path A breaks early; Path B
    // does no per-message scan), so we do one additional O(char count) pass here.
    //
    // Precise tokens_demanded — display/observability surface (Health dashboard, future).
    // NOT consumed by the compaction trigger (which uses a coarse pre-budget estimate
    // in maybe_compact_thread). Crossing these two consumers re-introduces the
    // timing bug Reed §2.5 caught in the D scope brief.
    let fixed_tokens = t1_tokens + t3_tokens + t4_tokens + t5_tokens;
    let excluded_tokens: usize = messages.iter()
        .take(messages_to_skip)
        .map(|msg| ratio.estimate_tokens(&msg.content))
        .sum();
    let tokens_demanded = fixed_tokens + t6_tokens + excluded_tokens;
    
    let budget_result = BudgetResult {
        model: model.to_string(),
        context_window,
        response_reserve,
        available_budget,
        tiers,
        total_used,
        messages_included: messages_to_include.len(),
        messages_total: messages.len(),
        images_included,
        images_replaced,
        cache_prefix_tokens,
        cache_prefix_stable: change_reason == "stable",
        cache_prefix_change_reason: change_reason,
        tokens_demanded,
        is_estimated: false, // Updated post-apply by update_budget_is_estimated at the commands.rs call site.
    };
    
    // Store in state for Health dashboard
    {
        let mut latest_stats = state.latest_stats.lock().unwrap_or_else(|e| e.into_inner());
        *latest_stats = Some(budget_result.clone());
    }
    
    (messages_to_include, budget_result)
}

/// Retrieve the latest budget stats for the Health dashboard
/// Returns None if no budget calculation has been performed yet
pub fn get_latest_budget_stats(state: &BudgetState) -> Option<BudgetResult> {
    let stats = state.latest_stats.lock().unwrap_or_else(|e| e.into_inner());
    stats.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_budget_fills_in_priority_order() {
        // Tiers 1-5 always included before Tier 6
        let ratio = TokenEstimationRatio::for_model("claude-opus-4");
        let system = "System files content here".repeat(100); // ~2600 chars
        let tools = "Tool definitions here".repeat(50); // ~1050 chars
        let spatial = "Spatial context".repeat(10); // ~150 chars
        
        let t1 = ratio.estimate_tokens(&system);
        let t3 = ratio.estimate_tokens(&tools);
        let t5 = ratio.estimate_tokens(&spatial);
        
        // All should be non-zero
        assert!(t1 > 0);
        assert!(t3 > 0);
        assert!(t5 > 0);
        
        // T1 should be larger than T3 and T5
        assert!(t1 > t3);
        assert!(t1 > t5);
    }
    
    #[test]
    fn test_budget_trims_messages_when_full() {
        // Create messages that will definitely exceed budget
        let messages: Vec<ChatMessage> = (0..100)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}: ", i) + &"x".repeat(50000), // ~50K chars each = ~14K tokens
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            })
            .collect();
        
        // Very large system to eat up budget
        let large_system = "x".repeat(400000); // ~114K tokens, leaving only ~10K for messages
        
        let state = BudgetState::new();
        let (trimmed, budget) = apply_budget(
            &state,
            "test-session",
            "gpt-4", // 128K window
            128_000,
            &large_system,
            "{}",
            "",
            None,
            messages.clone(),
            10,
        );
        
        // With ~10K tokens available and each message ~14K tokens, we can't fit many
        // But minimum guarantee is 12 messages, so we'll get at least those
        assert!(trimmed.len() < messages.len(), 
            "Expected trimming but got {}/{} messages included", 
            trimmed.len(), messages.len());
        assert!(budget.messages_included < budget.messages_total);
        
        // Should include newest messages
        assert_eq!(trimmed.last().unwrap().content, messages.last().unwrap().content);
        
        // Should respect minimum guarantee (at least 12 messages)
        assert!(trimmed.len() >= 12, "Should guarantee minimum 12 messages");
    }
    
    #[test]
    fn test_budget_minimum_exchanges_guaranteed() {
        // Last 6 exchanges always present even if over budget
        let messages: Vec<ChatMessage> = (0..12)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: "A".repeat(100000), // Huge messages
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            })
            .collect();
        
        let state = BudgetState::new();
        let (trimmed, _budget) = apply_budget(
            &state,
            "test-session",
            "test-model",
            128_000,
            "System",
            "{}",
            "",
            None,
            messages.clone(),
            10,
        );
        
        // Should include all 12 messages (minimum guarantee)
        assert_eq!(trimmed.len(), 12);
    }
    
    #[test]
    fn test_budget_provider_aware_estimation() {
        let claude_ratio = TokenEstimationRatio::for_model("claude-sonnet-4");
        let gpt_ratio = TokenEstimationRatio::for_model("gpt-4o");
        
        // Claude uses 3.5 chars/token
        assert_eq!(claude_ratio.chars_per_token, 3.5);
        
        // GPT uses 4.0 chars/token
        assert_eq!(gpt_ratio.chars_per_token, 4.0);
        
        let text = "a".repeat(1000); // 1000 chars
        
        let claude_tokens = claude_ratio.estimate_tokens(&text);
        let gpt_tokens = gpt_ratio.estimate_tokens(&text);
        
        // Claude should estimate more tokens for same text
        assert!(claude_tokens > gpt_tokens);
        assert_eq!(claude_tokens, 286); // 1000 / 3.5 = 285.7 → 286
        assert_eq!(gpt_tokens, 250); // 1000 / 4.0 = 250
    }
    
    #[test]
    fn test_budget_unknown_model_uses_conservative() {
        let ratio = TokenEstimationRatio::for_model("some-unknown-model-xyz");
        
        // Should use conservative 3.5
        assert_eq!(ratio.chars_per_token, 3.5);
    }
    
    #[test]
    fn test_budget_logging_output() {
        let messages: Vec<ChatMessage> = (0..10)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}", i),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            })
            .collect();
        
        let state = BudgetState::new();
        let (_trimmed, budget) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System prompt",
            "{}",
            "Spatial",
            Some("Compaction summary"),
            messages,
            10,
        );
        
        // Log should contain all required fields
        assert!(!budget.model.is_empty());
        assert!(budget.context_window > 0);
        assert!(budget.available_budget > 0);
        assert!(!budget.tiers.is_empty());
        assert!(budget.total_used > 0);
        
        // Actually call log to ensure no panics
        budget.log();
    }
    
    #[test]
    fn test_image_token_calculation() {
        // Typical screenshot 1200×800
        // X: ceil(1200/768) = 2, Y: ceil(800/768) = 2 → 4 tiles
        // 4 * 800 + 100 = 3300 tokens
        let tokens = estimate_image_tokens(1200, 800);
        assert_eq!(tokens, 3300);
        
        // Small image that fits in one tile
        let small_tokens = estimate_image_tokens(600, 400);
        assert_eq!(small_tokens, 900); // 1 tile * 800 + 100
        
        // Large image that needs resizing
        let large_tokens = estimate_image_tokens(3000, 2000);
        
        // Should be more tiles than small image
        assert!(large_tokens > small_tokens);
        
        // Conservative estimate
        let conservative = estimate_image_tokens_conservative();
        assert_eq!(conservative, 1700);
    }
    
    // ===========================================================
    // get_context_window resolver tests (§27.9, #1402-C)
    // ===========================================================

    /// Test 1: Precedence-1 wins — OpenClaw with context_length beats everything.
    #[tokio::test]
    async fn test_resolver_p1_openclaw_wins() {
        let result = get_context_window(
            &Backend::OpenClaw,
            "anthropic/claude-sonnet-4-5",
            Some(300_000),
            Some(150_000), // override present but must be ignored
        ).await;
        assert_eq!(result, 300_000, "OpenClaw wire value must override everything");
    }

    /// Test 2: Override beats precedence-3, no clamp needed.
    #[tokio::test]
    async fn test_resolver_override_beats_p3_no_clamp() {
        let result = get_context_window(
            &Backend::OpenRouter,
            "some-model",
            Some(200_000),  // api_max
            Some(150_000),  // override < api_max → no clamp
        ).await;
        assert_eq!(result, 150_000);
    }

    /// Test 3: Override clamped to api_max.
    #[tokio::test]
    async fn test_resolver_override_clamped_to_api_max() {
        let result = get_context_window(
            &Backend::OpenRouter,
            "some-model",
            Some(100_000),  // api_max
            Some(200_000),  // override > api_max → clamped to 100_000
        ).await;
        assert_eq!(result, 100_000);
    }

    /// Test 4: Override floored to 50k when below minimum.
    #[tokio::test]
    async fn test_resolver_override_floored_to_50k() {
        let result = get_context_window(
            &Backend::OpenRouter,
            "some-model",
            None,           // no api_max
            Some(10_000),   // below floor → 50_000
        ).await;
        assert_eq!(result, 50_000);
    }

    /// Test 5: Precedence-3 OpenRouter — context_length returned directly.
    #[tokio::test]
    async fn test_resolver_p3_openrouter() {
        let result = get_context_window(
            &Backend::OpenRouter,
            "some-model",
            Some(128_000),
            None,
        ).await;
        assert_eq!(result, 128_000);
    }

    /// Test 6: Precedence-3 Ollama via prepopulated cache (no HTTP).
    #[tokio::test]
    async fn test_resolver_p3_ollama_cache() {
        // Prepopulate the Ollama context-window cache directly
        let endpoint = "http://127.0.0.1:11434";
        let model_id = "llama3:8b";
        let cache_key = format!("{}:{}", endpoint, model_id);
        {
            let mut cache = crate::ai::connections::get_ollama_ctx_cache().lock().unwrap();
            cache.insert(cache_key, 131_072);
        }

        let result = get_context_window(
            &Backend::Ollama { endpoint: endpoint.to_string() },
            model_id,
            None, // context_length from registry is None for Ollama
            None,
        ).await;
        assert_eq!(result, 131_072, "Should return cached Ollama context window without HTTP");
    }

    /// Test 7: Seed fallback for Anthropic (precedence-4).
    #[tokio::test]
    async fn test_resolver_seed_fallback_anthropic() {
        let result = get_context_window(
            &Backend::Anthropic,
            "claude-opus-4",
            None,
            None,
        ).await;
        assert_eq!(result, 200_000, "claude-opus-4 must match seed table entry");
    }

    /// Test 8: 128k floor for unknown model.
    #[tokio::test]
    async fn test_resolver_128k_floor() {
        let result = get_context_window(
            &Backend::Anthropic,
            "totally-unknown-llm",
            None,
            None,
        ).await;
        assert_eq!(result, 128_000);
    }

    /// Test 9: Precedence-1 ignores override for OpenClaw.
    #[tokio::test]
    async fn test_resolver_p1_ignores_override() {
        let result = get_context_window(
            &Backend::OpenClaw,
            "anthropic/claude-sonnet-4-5",
            Some(200_000),
            Some(50_000), // override must be ignored because OpenClaw is #1
        ).await;
        assert_eq!(result, 200_000);
    }

    // ===========================================================
    // resolve_context_window_with_source tests (#1404-E)
    // ===========================================================

    /// E-Test-1: Seed-table path (precedence 4) is marked is_estimated=true.
    /// Anthropic backend with no context_length and no override falls to seed lookup;
    /// for a known Anthropic model the seed returns 200_000 (estimated).
    #[tokio::test]
    async fn test_resolve_context_window_with_source_marks_seed_estimated() {
        let resolved = resolve_context_window_with_source(
            &Backend::Anthropic,
            "claude-opus-4",
            None,  // no registry / API source
            None,  // no user override
        ).await;
        assert_eq!(resolved.value, 200_000, "seed value for claude-opus-4 must be 200k");
        assert!(
            resolved.is_estimated,
            "Anthropic model with no API source must be marked estimated (seed/floor path)"
        );
    }

    /// E-Test-2: User override (precedence 2) is marked is_estimated=false.
    /// When a user override is present, the resolver returns that value (floored/clamped);
    /// it is a real user-supplied value, not a guess.
    #[tokio::test]
    async fn test_resolve_context_window_with_source_marks_override_real() {
        let resolved = resolve_context_window_with_source(
            &Backend::Anthropic,
            "claude-opus-4",
            None,            // no API source
            Some(100_000),  // user override
        ).await;
        assert_eq!(resolved.value, 100_000, "override value must pass through unmodified");
        assert!(
            !resolved.is_estimated,
            "User override must NOT be marked estimated — it is an explicit user setting"
        );
    }

    // ===========================================================
    // ModelRegistry tests (§27.9, Q1 key-shape correctness)
    // ===========================================================

    /// Test 10 (CRITICAL): Registry key is (provider, model_id), not model_id alone.
    /// Two providers may serve the same model id with different context windows.
    #[test]
    fn test_registry_key_collision() {
        use crate::ai::provider::ModelRegistry;
        let registry = ModelRegistry::new();

        registry.insert("openclaw", "anthropic/claude-sonnet-4-5", Some(300_000));
        registry.insert("openrouter", "anthropic/claude-sonnet-4-5", Some(200_000));

        let oc = registry.lookup("openclaw", "anthropic/claude-sonnet-4-5");
        let or_val = registry.lookup("openrouter", "anthropic/claude-sonnet-4-5");

        assert_eq!(oc, Some(Some(300_000)), "openclaw entry must return 300_000");
        assert_eq!(or_val, Some(Some(200_000)), "openrouter entry must return 200_000");
        assert_ne!(oc, or_val, "Two providers must have independent registry entries");
    }

    /// Test 11: Cold registry lookup returns None.
    #[test]
    fn test_registry_cold_lookup() {
        use crate::ai::provider::ModelRegistry;
        let registry = ModelRegistry::new();

        assert!(registry.is_empty(), "New registry must be empty");
        let result = registry.lookup("anthropic", "claude-opus-4");
        assert_eq!(result, None, "Cold registry lookup must return None");
    }

    /// Test 12: Registry returns Some(None) when key matched but value is None.
    /// This covers the Some(None) branch in commands.rs lookup — distinct from
    /// an absent key (None) and a known value (Some(Some(v))).
    #[test]
    fn registry_lookup_returns_some_none_when_value_is_none() {
        use crate::ai::provider::ModelRegistry;
        let registry = ModelRegistry::new();
        registry.insert("anthropic", "claude-opus-4", None);

        // Key matches, value is None
        let result = registry.lookup("anthropic", "claude-opus-4");
        assert_eq!(result, Some(None), "registry should return Some(None) when key matched but value is None");

        // Key doesn't match
        let miss = registry.lookup("anthropic", "claude-sonnet-4");
        assert_eq!(miss, None, "registry should return None for absent key");
    }

    // Legacy seed-table test (tests seed_lookup directly via the async resolver on Anthropic backend)
    #[tokio::test]
    async fn test_seed_table_values() {
        // Claude models via Anthropic backend (no API source → seed)
        assert_eq!(get_context_window(&Backend::Anthropic, "claude-opus-4", None, None).await, 200_000);
        assert_eq!(get_context_window(&Backend::Anthropic, "anthropic/claude-sonnet-4-5", None, None).await, 200_000);
        assert_eq!(get_context_window(&Backend::Anthropic, "claude-haiku", None, None).await, 200_000);
        // GPT via Unknown backend
        assert_eq!(get_context_window(&Backend::Unknown, "gpt-4o", None, None).await, 128_000);
        assert_eq!(get_context_window(&Backend::Unknown, "openai/gpt-4", None, None).await, 128_000);
        // Unknown fallback
        assert_eq!(get_context_window(&Backend::Unknown, "totally-unknown-model", None, None).await, 128_000);
    }
    
    // === §27.5 Cache-Aware Message Windowing Tests ===
    
    #[test]
    fn test_cache_prefix_stable_between_requests() {
        let state = BudgetState::new();
        
        let messages: Vec<ChatMessage> = (0..20)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}", i),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            })
            .collect();
        
        // First request
        let (_trimmed1, budget1) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System prompt",
            "{}",
            "Spatial context",
            Some("Summary"),
            messages.clone(),
            10,
        );
        
        // Second request with same context
        let (_trimmed2, budget2) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System prompt",
            "{}",
            "Spatial context",
            Some("Summary"),
            messages.clone(),
            10,
        );
        
        // Cache prefix should be stable
        assert!(budget2.cache_prefix_stable, "Cache prefix should be stable between identical requests");
        assert_eq!(budget2.cache_prefix_change_reason, "stable");
        assert_eq!(budget1.cache_prefix_tokens, budget2.cache_prefix_tokens);
    }
    
    #[test]
    fn test_cache_prefix_changes_on_navigation() {
        let state = BudgetState::new();
        
        let messages: Vec<ChatMessage> = (0..20)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}", i),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            })
            .collect();
        
        // First request
        let (_trimmed1, _budget1) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System prompt",
            "{}",
            "Spatial context A",
            Some("Summary"),
            messages.clone(),
            10,
        );
        
        // Second request with different spatial context
        let (_trimmed2, budget2) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System prompt",
            "{}",
            "Spatial context B",  // Changed
            Some("Summary"),
            messages.clone(),
            10,
        );
        
        // Cache prefix should have changed
        assert!(!budget2.cache_prefix_stable, "Cache prefix should change when spatial context changes");
        assert_eq!(budget2.cache_prefix_change_reason, "navigation");
    }
    
    #[test]
    fn test_cache_prefix_changes_on_compaction() {
        let state = BudgetState::new();
        
        let messages: Vec<ChatMessage> = (0..20)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}", i),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            })
            .collect();
        
        // First request
        let (_trimmed1, _budget1) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System prompt",
            "{}",
            "Spatial context",
            Some("Summary A"),
            messages.clone(),
            10,
        );
        
        // Second request with different summary (compaction happened)
        let (_trimmed2, budget2) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System prompt",
            "{}",
            "Spatial context",
            Some("Summary B"),  // Changed - indicates compaction
            messages.clone(),
            10,
        );
        
        // Cache prefix should have changed due to compaction
        assert!(!budget2.cache_prefix_stable, "Cache prefix should change when summary changes");
        assert_eq!(budget2.cache_prefix_change_reason, "compaction");
    }
    
    #[test]
    fn test_messages_append_only() {
        let state = BudgetState::new();
        
        // Start with 15 messages (all fit)
        let mut messages: Vec<ChatMessage> = (0..15)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}", i),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            })
            .collect();
        
        // First request - all messages included
        let (trimmed1, _budget1) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System prompt",
            "{}",
            "Spatial",
            None,
            messages.clone(),
            10,
        );
        
        assert_eq!(trimmed1.len(), 15, "All 15 messages should be included initially");
        
        // Add 2 new messages
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: "Message 15".to_string(),
            tool_calls: None,
            tool_call_id: None,
            content_blocks: None,
            attachment_ids: None,
                id: None,
        });
        messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: "Message 16".to_string(),
            tool_calls: None,
            tool_call_id: None,
            content_blocks: None,
            attachment_ids: None,
                id: None,
        });
        
        // Second request with same context - should be append-only
        let (trimmed2, budget2) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System prompt",
            "{}",
            "Spatial",
            None,
            messages.clone(),
            10,
        );
        
        // Should include all previous messages + new ones (17 total)
        assert_eq!(trimmed2.len(), 17, "Should include all 15 previous + 2 new = 17 messages");
        assert!(budget2.cache_prefix_stable, "Cache prefix should be stable");
        
        // Verify first message is the same (window didn't shift)
        assert_eq!(trimmed2.first().unwrap().content, "Message 0");
        assert_eq!(trimmed2.last().unwrap().content, "Message 16");
    }
    
    #[test]
    fn test_windowing_drops_oldest_not_middle() {
        let state = BudgetState::new();
        
        // Create many large messages that won't all fit
        let messages: Vec<ChatMessage> = (0..150)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}: ", i) + &"x".repeat(5000), // Each message ~5000 chars = ~1400 tokens
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            })
            .collect();
        
        // Large system to force message trimming
        // 150 messages × 1400 tokens = 210K tokens, but context window is only 200K
        let large_system = "x".repeat(400000); // ~114K tokens, leaves ~76K for messages
        
        let (trimmed, budget) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4", // 200K window, ~190K available
            200_000,
            &large_system,
            "{}",
            "",
            None,
            messages.clone(),
            10,
        );
        
        // Should have excluded some messages (76K tokens / 1400 per message = ~54 messages fit)
        assert!(
            trimmed.len() < messages.len(),
            "Should exclude some messages when budget is tight. Included {}/{}, budget: {}/{}",
            trimmed.len(), messages.len(), budget.total_used, budget.available_budget
        );
        
        // Should include the NEWEST messages (drops oldest first)
        assert_eq!(
            trimmed.last().unwrap().content,
            messages.last().unwrap().content,
            "Should include newest message"
        );
        
        // The excluded messages should be from the START (oldest)
        let first_included_idx = messages.len() - trimmed.len();
        assert_eq!(
            trimmed.first().unwrap().content,
            messages[first_included_idx].content,
            "Should start from a stable cutoff point, not skip middle messages"
        );
        
        // Verify no middle messages were skipped (all included messages are consecutive)
        for (i, msg) in trimmed.iter().enumerate() {
            let expected_idx = first_included_idx + i;
            assert_eq!(
                msg.content,
                messages[expected_idx].content,
                "Messages should be consecutive (no gaps in the middle)"
            );
        }
    }
    
    // === P1 Tests from §341 Test Audit ===
    
    #[test]
    fn test_budget_system_prompt_exceeds_window() {
        let state = BudgetState::new();
        // P1: Budget exhaustion cascade — system prompt alone exceeds context window
        // Scenario: 200K system prompt in a 128K window (GPT-4)
        // Expected: System prompt gets gracefully capped, messages get minimum 6-exchange guarantee
        
        let huge_system = "x".repeat(600_000); // ~171K tokens for GPT-4 (4.0 chars/token)
        let messages: Vec<ChatMessage> = (0..12)
            .map(|i| ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}", i),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            })
            .collect();
        
        let (trimmed, budget) = apply_budget(
            &state,
            "test-session",
            "gpt-4", // 128K window, ~122K available
            128_000,
            &huge_system,
            "{}",
            "",
            None,
            messages.clone(),
            10,
        );
        
        // Should include minimum 12 messages despite oversized system
        assert_eq!(
            trimmed.len(), 12,
            "Minimum guarantee must survive system overflow"
        );
        
        // Total used will exceed budget due to system overflow, but shouldn't panic
        // We allow graceful degradation where minimum messages are kept
        assert!(
            budget.total_used > 0,
            "Budget should calculate total usage even with overflow"
        );
        
        eprintln!(
            "[TEST] System overflow: window={}, available={}, total_used={}, messages={}/{}",
            budget.context_window, budget.available_budget, budget.total_used,
            budget.messages_included, budget.messages_total
        );
    }
    
    #[test]
    fn test_budget_single_message_exceeds_budget() {
        let state = BudgetState::new();
        // P1: Single message exceeding budget
        // Scenario: One user message is larger than entire remaining budget
        // Expected: Message should be included (minimum guarantee) even if it causes overflow
        
        let normal_system = "System prompt";
        let huge_message = ChatMessage {
            role: "user".to_string(),
            content: "x".repeat(600_000), // ~171K tokens for GPT-4
            tool_calls: None,
            tool_call_id: None,
            content_blocks: None,
            attachment_ids: None,
            id: None,
        };
        
        // Add a few more messages to verify minimum guarantee
        let mut messages = vec![huge_message];
        for i in 1..6 {
            messages.push(ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Small message {}", i),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            });
        }
        
        let (trimmed, budget) = apply_budget(
            &state,
            "test-session",
            "gpt-4", // 128K window
            128_000,
            normal_system,
            "{}",
            "",
            None,
            messages.clone(),
            10,
        );
        
        // Should include at least the huge message (minimum guarantee protects it)
        assert!(
            trimmed.len() >= 1,
            "Should include at least the oversized message"
        );
        
        // First message should be the huge one
        assert_eq!(trimmed[0].content.len(), 600_000, "Huge message should be included");
        
        // Budget will show overflow, but system handles it gracefully
        eprintln!(
            "[TEST] Single message overflow: available={}, message_tokens~={}, total_used={}",
            budget.available_budget,
            budget.tiers.iter().find(|t| t.tier == 6).map(|t| t.tokens).unwrap_or(0),
            budget.total_used
        );
    }
    
    #[test]
    fn test_image_age_out_boundary_conditions() {
        let state = BudgetState::new();
        // P1: Image age-out at exactly 10th message boundary
        // Scenario: 15 total messages, image at index 4 (11th from end)
        // Calculation: distance = 15 - (4 + 1) = 10
        // Expected: 10 <= 10 = true → image should be KEPT (within retention)
        
        let mut messages: Vec<ChatMessage> = Vec::new();
        
        // First 5 messages without images
        for i in 0..5 {
            messages.push(ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}", i),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            });
        }
        
        // Message at index 5 has an image (will be 10th from end in 15-message set)
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: "Message with image at boundary".to_string(),
            tool_calls: None,
            tool_call_id: None,
            content_blocks: None,
            attachment_ids: Some("img-boundary".to_string()),
            id: None,
        });
        
        // 9 more messages (total = 15)
        for i in 6..15 {
            messages.push(ChatMessage {
                role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
                content: format!("Message {}", i),
                tool_calls: None,
                tool_call_id: None,
                content_blocks: None,
                attachment_ids: None,
                id: None,
            });
        }
        
        let (trimmed, budget) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System",
            "{}",
            "",
            None,
            messages.clone(),
            10, // 10-message retention window
        );
        
        // All 15 messages should fit easily
        assert_eq!(trimmed.len(), 15, "All messages should be included");
        
        // Image at index 5 is 10 messages from the end → EXACTLY at boundary
        // Distance = 15 - (5 + 1) = 9 messages from end (0-indexed position)
        // So actually, the image at index 5 is the 10th message from the end (0-based indexing)
        // In the trimmed output: index 5, position from end = 15 - 5 - 1 = 9
        // Wait, let me recalculate: if we have 15 messages (indices 0-14), and image is at index 5
        // Position from end = 15 - 5 - 1 = 9 (i.e., there are 9 messages after it)
        // So it's within the 10-message retention window
        
        // Should have 1 image included (at boundary, should be KEPT)
        assert_eq!(
            budget.images_included, 1,
            "Image at 10th message boundary should be KEPT (within retention)"
        );
        assert_eq!(
            budget.images_replaced, 0,
            "No images should be replaced"
        );
        
        // Now test the opposite: image at index 4 (11th from end) should be REPLACED
        let mut messages2 = messages.clone();
        messages2[4].attachment_ids = Some("img-outside".to_string());
        messages2[5].attachment_ids = None; // Remove the boundary image
        
        let (_trimmed2, budget2) = apply_budget(
            &state,
            "test-session",
            "claude-opus-4",
            200_000,
            "System",
            "{}",
            "",
            None,
            messages2,
            10,
        );
        
        // Image at index 4 is 11 messages from end → should be REPLACED
        assert_eq!(
            budget2.images_replaced, 1,
            "Image at 11th message (outside 10-message window) should be REPLACED"
        );
        assert_eq!(
            budget2.images_included, 0,
            "No images should be included"
        );
    }

    // === tokens_demanded tests (#1403-D) ===

    fn make_messages(n: usize, chars_each: usize) -> Vec<ChatMessage> {
        (0..n).map(|i| ChatMessage {
            role: if i % 2 == 0 { "user".to_string() } else { "assistant".to_string() },
            content: "x".repeat(chars_each),
            tool_calls: None,
            tool_call_id: None,
            content_blocks: None,
            attachment_ids: None,
            id: None,
        }).collect()
    }

    #[test]
    fn test_tokens_demanded_under_fit() {
        // All messages fit → excluded_tokens == 0 → tokens_demanded == T1+T3+T4+T5+T6.
        // Note: T2 (memory, hardcoded ~6,000) is counted inside T1 per spec; it is
        // included in total_used (all tiers summed) but EXCLUDED from tokens_demanded
        // to avoid double-counting. So in the under-fit case:
        //   tokens_demanded = total_used - t2_tokens (6_000 hardcoded)
        let state = BudgetState::new();
        let messages = make_messages(5, 200); // 5 tiny messages, easily fit
        let (trimmed, budget) = apply_budget(
            &state,
            "test-session",
            "gpt-4",
            128_000,
            "System",
            "{}",
            "",
            None,
            messages.clone(),
            10,
        );
        // All 5 should be included
        assert_eq!(trimmed.len(), 5, "All messages should be included");
        // T2 (6_000 hardcoded) is in total_used but not in tokens_demanded
        let t2_estimate = 6_000usize;
        assert_eq!(
            budget.tokens_demanded + t2_estimate,
            budget.total_used,
            "Under-fit: tokens_demanded must equal total_used minus T2 estimate ({}) when no messages excluded",
            t2_estimate
        );
        assert!(budget.tokens_demanded > 0, "tokens_demanded must be non-zero");
    }

    #[test]
    fn test_tokens_demanded_over_fit() {
        // Messages exceed window → some excluded → tokens_demanded > total_used
        let state = BudgetState::new();
        // 100 huge messages to force exclusion
        let messages = make_messages(100, 50_000); // ~14K tokens each
        let large_system = "x".repeat(400_000); // ~114K tokens
        let (trimmed, budget) = apply_budget(
            &state,
            "test-session",
            "gpt-4",
            128_000,
            &large_system,
            "{}",
            "",
            None,
            messages.clone(),
            10,
        );
        // Some messages should be excluded
        assert!(
            trimmed.len() < messages.len(),
            "Expected exclusion but all {} messages included", trimmed.len()
        );
        // tokens_demanded must include excluded messages' tokens
        assert!(
            budget.tokens_demanded > budget.total_used,
            "Over-fit: tokens_demanded ({}) must exceed total_used ({}) because excluded messages are counted",
            budget.tokens_demanded, budget.total_used
        );
    }

    #[test]
    fn test_tokens_demanded_includes_all_five_tiers() {
        // Verify that tokens_demanded covers T1 + T3 + T4 + T5 (not just messages)
        let state = BudgetState::new();
        let messages = make_messages(3, 100);
        let system = "S".repeat(1000); // ~250 tokens
        let tools = "T".repeat(500);   // ~125 tokens
        let spatial = "P".repeat(300); // ~75 tokens
        let summary = "U".repeat(400); // ~100 tokens
        let (_, budget) = apply_budget(
            &state,
            "test-session",
            "gpt-4",
            128_000,
            &system,
            &tools,
            &spatial,
            Some(&summary),
            messages,
            10,
        );
        // tokens_demanded must be strictly greater than just T6
        let t6_only = budget.tiers.iter().find(|t| t.tier == 6).map(|t| t.tokens).unwrap_or(0);
        assert!(
            budget.tokens_demanded > t6_only,
            "tokens_demanded ({}) must include fixed tiers beyond T6 ({})",
            budget.tokens_demanded, t6_only
        );
        // tokens_demanded = total_used - t2_tokens (T2 excluded to avoid double-count with T1)
        // In all-fit case: tokens_demanded + 6_000 (T2 estimate) == total_used
        let t2_estimate = 6_000usize;
        assert!(
            budget.tokens_demanded + t2_estimate >= budget.total_used.saturating_sub(1),
            "tokens_demanded ({}) + T2 ({}) must approximately equal total_used ({})",
            budget.tokens_demanded, t2_estimate, budget.total_used
        );
    }

    #[test]
    fn test_tokens_demanded_stable_prefix_path() {
        // Path B (stable prefix): excluded messages still counted via take(messages_to_skip)
        let state = BudgetState::new();
        let system = "System".to_string();
        let messages_first = make_messages(5, 200);

        // First call (prefix change / new) — Path A
        let (_, budget1) = apply_budget(
            &state, "test-session", "gpt-4", 128_000, &system, "{}", "", None,
            messages_first.clone(), 10,
        );

        // Second call with same prefix (stable) + two new messages — Path B
        let mut messages_second = messages_first.clone();
        messages_second.push(ChatMessage {
            role: "user".to_string(),
            content: "x".repeat(200),
            tool_calls: None, tool_call_id: None, content_blocks: None,
            attachment_ids: None, id: None,
        });
        messages_second.push(ChatMessage {
            role: "assistant".to_string(),
            content: "x".repeat(200),
            tool_calls: None, tool_call_id: None, content_blocks: None,
            attachment_ids: None, id: None,
        });
        let (_, budget2) = apply_budget(
            &state, "test-session", "gpt-4", 128_000, &system, "{}", "", None,
            messages_second.clone(), 10,
        );

        // Both calls: tokens_demanded must be non-zero and >= total_used
        assert!(budget1.tokens_demanded > 0);
        assert!(budget2.tokens_demanded > 0);
        assert!(budget2.tokens_demanded >= budget1.tokens_demanded,
            "More messages → more tokens demanded: {} < {}",
            budget2.tokens_demanded, budget1.tokens_demanded);
    }

    /// Reed's audit, 2026-10-30, PR #3122 FAIL #2 — reproduced as a named
    /// test (Matt's ruling: session-keyed window). Pre-fix, `BudgetState.
    /// window` was one app-wide slot keyed only on the `{system_prompt,
    /// tools_json, spatial_context, compaction_summary}` prefix hash — no
    /// session id anywhere in it. Job dispatch's own system prompt is
    /// invariant (`default_system_prompt()`), so a SECOND job dispatched
    /// on a DIFFERENT session, with that identical prefix, inherited the
    /// FIRST job's append-only bookkeeping
    /// (`target_message_count = previous_included_count +
    /// new_message_count`) — a branch that does ZERO budget-fitting
    /// arithmetic, by design (it exists to avoid re-scanning a session's
    /// own history on every request within THAT session). Reed's real
    /// probe measured a 476% overflow this exact way (job A: 3 tiny
    /// messages; job B: 40 huge messages, same prefix, all 42 messages
    /// handed out with zero re-check against job B's own real budget).
    #[test]
    fn test_session_keyed_window_second_job_dispatch_different_session_gets_fresh_window_reed_476_percent_probe() {
        let state = BudgetState::new();
        let system = "the same invariant job dispatch system prompt".to_string();

        // Job A, session "job-a": 3 tiny messages, all fit easily — this
        // is the call whose bookkeeping a shared, unkeyed window would
        // leak into job B below.
        let job_a_messages = make_messages(3, 50);
        let (_trimmed_a, budget_a) = apply_budget(
            &state, "job-a", "test-model", 128_000, &system, "{}", "", None,
            job_a_messages.clone(), 10,
        );
        assert_eq!(budget_a.messages_included, 3, "sanity: job A's 3 tiny messages all fit");

        // Job B, session "job-b" — a DIFFERENT session, the SAME
        // invariant prefix, 40 messages at ~6,000 tokens each (sized so a
        // real budget-fitting scan includes roughly half of them — large
        // enough that "all 40 inherited" is unambiguously distinguishable
        // from "trimmed to what actually fits," without landing in the
        // separate min-12-guaranteed-overflow edge case `test_budget_
        // system_prompt_exceeds_window` already covers).
        let job_b_messages = make_messages(40, 21_000);
        let (trimmed_b, budget_b) = apply_budget(
            &state, "job-b", "test-model", 128_000, &system, "{}", "", None,
            job_b_messages.clone(), 10,
        );

        // The fix: job B is a session the window map has never seen — it
        // must get a FRESH budget-fitting scan (Path A), not job A's
        // append-only arithmetic. The broken (unkeyed) path would include
        // all 40 messages (3 + (40 - 3) = 40) regardless of budget — the
        // exact 476%-style overflow Reed measured; a real scan trims to
        // roughly 20 (12 minimum-guaranteed + ~8 more that fit the
        // remaining budget).
        assert!(
            trimmed_b.len() < 30,
            "job B must be trimmed against its OWN real budget, not inherit job A's append-only bookkeeping — got {}/{} included (close to 40 means the overflow reproduced)",
            trimmed_b.len(), job_b_messages.len()
        );
        assert!(
            budget_b.total_used <= budget_b.context_window,
            "job B's budgeted total ({}) must not overflow its own context window ({}) — overflow here means session state leaked from job A",
            budget_b.total_used, budget_b.context_window
        );

        // Job A's own window entry must be untouched by job B's call —
        // proves the map holds two independent entries, not one shared
        // slot job B's call overwrote.
        let (_trimmed_a2, budget_a2) = apply_budget(
            &state, "job-a", "test-model", 128_000, &system, "{}", "", None,
            job_a_messages.clone(), 10,
        );
        assert_eq!(
            budget_a2.messages_included, 3,
            "job A's own window must still show its own 3 messages, unaffected by job B's call in between"
        );
    }
}
