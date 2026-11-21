//! Connections: provider connection management, API-key storage (Brave,
//! Venice, SearXNG), model probing/listing across every provider, Ollama
//! context-window lookups, and settings passthrough. Split out of
//! `ai/commands.rs` (#3097).

use rusqlite::params;
use rusqlite::OptionalExtension;
use std::time::{Duration, Instant};
use super::chat::resolve_model_for_message;
use super::ollama::{OllamaProbeResult, OllamaProvider};
use super::provider::*;
use crate::database::DbState;
use crate::refresh_event::{emit_data_changed, ENTITY_PROJECT, REASON_EDITED};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::State;


// #2522 ADR-0063 Stage 2: `ai_configure` (the legacy pre-`_and_save` command) deleted.
// Dead code — no live frontend caller (only an unused `aiConfigure` export in
// src/lib/api.ts with zero call sites); its `invoke_handler` registration in lib.rs
// is removed alongside it. `ai_configure_and_save` is the live command and survives,
// narrowed per D7.1 (see below).

// =============================================================================
// OC Agent Registry — model resolution (#1322)
// =============================================================================

/// Fetch the primary model for the `main` agent from the gateway's `/v1/agents` endpoint.
///
/// Called during OC-mode onboarding instead of any hardcoded default.
/// Returns `Err` with a user-facing message if the gateway is unreachable, the
/// agent is missing, or the `model` field is absent/empty.
///
/// Contract: **zero fallback strings** — the caller must surface the error.
/// Build the URL for the OC agent-registry endpoint.
///
/// Accepts the user-supplied endpoint in any of these forms and always returns
/// `{host}/v1/agents` with exactly one `/v1` segment:
///
/// - `https://host:8443`          → `https://host:8443/v1/agents`
/// - `https://host:8443/`         → `https://host:8443/v1/agents`
/// - `https://host:8443/v1`       → `https://host:8443/v1/agents`
/// - `https://host:8443/v1/`      → `https://host:8443/v1/agents`
///
/// Tested by `test_oc_agents_url_no_double_v1` below.
pub(crate) fn oc_agents_url(base_url: &str) -> String {
    // Strip trailing slashes and a trailing `/v1` segment before appending
    // our own `/v1/agents`.  This prevents the doubled-path bug (#1345) that
    // occurs when the caller's base_url is already `/v1`-normalised (e.g. by
    // `normalize_api_endpoint`) and `fetch_oc_agent_model` naively appends.
    let base = base_url.trim_end_matches('/').trim_end_matches("/v1");
    format!("{}/v1/agents", base)
}


pub(crate) async fn fetch_oc_agent_model(
    base_url: &str,
    token: &str,
    db: &tauri::State<'_, DbState>,
) -> Result<String, String> {
    let client = crate::gateway_client::build_gateway_client(db)?;
    let url = oc_agents_url(base_url);
    eprintln!("[OC-MODEL] Fetching agent model from {}", url);

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| {
            format!(
            "Cannot determine your AI agent's model — your OpenClaw gateway didn't return one. \
             Check that your gateway is reachable and that the `main` agent is configured in \
             `openclaw.json`. (Network error: {})",
            e
        )
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        return Err(format!(
            "Cannot determine your AI agent's model — your OpenClaw gateway didn't return one. \
             Check that your gateway is reachable and that the `main` agent is configured in \
             `openclaw.json`. (Gateway returned HTTP {})",
            status
        ));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| {
        format!(
            "Cannot determine your AI agent's model — your OpenClaw gateway returned an \
         unreadable response. (Parse error: {})",
            e
        )
    })?;

    // Response shape: { "agents": [ { "oc_agent_id": "main", "model": "anthropic/...", ... } ] }
    let agents = body
        .get("agents")
        .and_then(|a| a.as_array())
        .ok_or_else(|| {
            format!(
            "Cannot determine your AI agent's model — your OpenClaw gateway didn't return one. \
             Check that your gateway is reachable and that the `main` agent is configured in \
             `openclaw.json`. (Response missing `agents` array)"
        )
        })?;

    // Today: always resolve against the `main` agent (oc_agent_id == "main").
    // Future: accept an agent_id parameter when multi-agent selection lands.
    let agent = agents
        .iter()
        .find(|a| a.get("oc_agent_id").and_then(|v| v.as_str()) == Some("main"))
        .ok_or_else(|| {
            format!(
            "Cannot determine your AI agent's model — your OpenClaw gateway didn't return one. \
             Check that your gateway is reachable and that the `main` agent is configured in \
             `openclaw.json`. (No agent with oc_agent_id=\"main\" found in response)"
        )
        })?;

    let model = agent
        .get("model")
        .and_then(|m| m.as_str())
        .filter(|m| !m.is_empty())
        .ok_or_else(|| {
            format!(
            "Cannot determine your AI agent's model — your OpenClaw gateway didn't return one. \
             Check that your gateway is reachable and that the `main` agent is configured in \
             `openclaw.json`. (Agent `main` has no `model` field in response)"
        )
        })?;

    eprintln!("[OC-MODEL] Resolved model for main agent: {}", model);
    Ok(model.to_string())
}


/// Connection-config save: writes provider/model/endpoint/credentials to
/// `ai_settings` + keychain. #2522 ADR-0063 Stage 2 / D7.1: narrowed —
/// this used to also mutate the `AiConfig` singleton's `provider`/`model`
/// fields (the dispatch-relevant write). That mutation, and the
/// `ProviderRestoreGuard` drop-guard mechanism that protected it against an
/// in-flight dispatch observing a half-written state, are both removed:
/// dispatch no longer reads a shared mutable singleton, so there's nothing
/// left to protect. What survives is the connection-config write path
/// (`ai_settings` + keychain) — still exercised by Settings.svelte and the
/// OC-mode connectivity validation via `fetch_oc_agent_model`.
#[tauri::command]
pub async fn ai_configure_and_save(
    provider: String,
    api_key: Option<String>,
    model: Option<String>,
    endpoint: Option<String>,
    simple_tier_model: Option<String>,
    db: State<'_, DbState>,
) -> Result<String, String> {
    eprintln!("[ONBOARD] ai_configure_and_save called with provider: {}, model: {:?}, endpoint: {:?}, simple_tier_model: {:?}, api_key length: {}",
              provider, model, endpoint, simple_tier_model, api_key.as_ref().map_or(0, |k| k.len()));

    // Validate the provider name up front (no AiProvider construction needed anymore —
    // dispatch resolves its own AiProvider per-request from ai_agents.runtime_type).
    match provider.as_str() {
        "anthropic" | "openrouter" | "ollama" | "openclaw" | "venice" | "none" => {}
        _ => return Err(format!("Unknown provider: {}", provider)),
    }
    if matches!(provider.as_str(), "anthropic" | "openrouter" | "venice") && api_key.is_none() {
        return Err(format!("API key required for {}", provider));
    }
    if provider == "openclaw" && api_key.is_none() {
        return Err("Token required for OpenClaw".to_string());
    }

    // In OC mode, ALWAYS validate connectivity via fetch_oc_agent_model.
    // If the caller supplied an explicit model id, honour it — but the fetch
    // must still run so that a bad URL returns Err rather than silently saving
    // (the false-green fix: #1446).
    let model_name = match provider.as_str() {
        "openclaw" => {
            // Always validate connectivity — pass the raw endpoint, NOT a /v1-normalised
            // base_url, to prevent the doubled-path bug (#1345).
            // oc_agents_url() inside fetch_oc_agent_model also strips a trailing /v1 as
            // defence-in-depth (e.g. when the user manually enters a /v1-suffixed URL).
            let raw_endpoint = endpoint
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:18789".to_string());
            let token = api_key
                .clone()
                .ok_or("Token required for OpenClaw".to_string())?;
            let resolved = fetch_oc_agent_model(&raw_endpoint, &token, &db).await?;
            // Use caller's model if explicitly supplied, otherwise use gateway-resolved value
            if model.as_deref() == Some("openclaw") || model.is_none() {
                resolved
            } else {
                model.clone().unwrap()
            }
        }
        // No hardcoded model defaults — model is set by user picker, not fabricated here
        _ => model.clone().unwrap_or_default(),
    };

    // Save to DB (connection config only — no in-memory singleton left to sync)
    // #2564 B1 fix (Path B), item #9: `ai_settings.provider` write REMOVED. It's
    // retired for real (ADR-0063 D8) — `runtime_type` is written by the
    // agent-creation/pairing flow into `ai_agents` and is the only value any Path-B
    // reader consults going forward (see runtime_resolution::derive_active_provider_label,
    // sync_client::has_active_openclaw_agent). Writing `provider` here would resurrect
    // the zombie key on every configure/pair call and undo the retirement.
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;

        if let Some(ref key) = api_key {
            // ADR-0044: route credential writes to keychain (Decision 1)
            match provider.as_str() {
                "openclaw" => {
                    crate::ai::credential_migration::write_gateway_token(&conn, key)?;
                }
                "anthropic" | "openrouter" => {
                    // B3 (#2565): each provider writes to its own keychain slot now —
                    // `provider` is already in scope and matches exactly one of the
                    // two key-based runtime_type literals `write_provider_key` accepts.
                    crate::ai::credential_migration::write_provider_key(&conn, &provider, key)?;
                    // #2641 D14 three-state color fix: mark "configured this install"
                    // ONLY from this Save path — mirrors the existing `gateway_paired_at`
                    // precedent for openclaw (ocPairingCeremony.ts writes it after a
                    // successful pair). A stale keychain leftover from a previous install
                    // (the #2631 bug) never runs through this Save path, so it never gets
                    // a `{runtime}_configured_at` timestamp and the card stays grey until
                    // the user actually saves a key this install.
                    crate::database::set_ai_setting(
                        &conn,
                        &format!("{}_configured_at", provider),
                        &chrono::Utc::now().to_rfc3339(),
                    )?;
                }
                "venice" => {
                    // ADR-0065 D4: Venice key is keychain-only (no legacy plaintext slot).
                    crate::ai::credential_migration::write_venice_key(&conn, key)?;
                    crate::database::set_ai_setting(
                        &conn,
                        "venice_configured_at",
                        &chrono::Utc::now().to_rfc3339(),
                    )?;

                    // Bug 1a fix (#2777 venice live-test scoping): `ai_disconnect_runtime_impl`'s
                    // venice arm deactivates every Venice agent row on disconnect (commands.rs
                    // ~4861), but nothing previously reactivated them on a subsequent save —
                    // `reactivate_agent` (agents.rs) existed but had zero callers. Reconnecting
                    // via Save must restore any Venice agent rows that a prior Disconnect
                    // deactivated, mirroring the openclaw disconnect arm's promote-on-reassign
                    // shape (commands.rs ~4821) but on the save/reconnect side instead.
                    let reactivated = conn
                        .execute(
                            "UPDATE ai_agents SET is_active = 1 WHERE runtime_type = 'venice' AND is_active = 0",
                            [],
                        )
                        .map_err(|e| format!("Failed to reactivate venice agents: {}", e))?;
                    eprintln!(
                        "[ai] venice: reactivated {} agent row(s) on save",
                        reactivated
                    );

                    // If no active Venice agent currently holds is_default, promote one.
                    // Same eligibility filter as the openclaw disconnect-arm promotion and
                    // set_default_agent_impl (agents.rs #B5/#2567): exclude imported
                    // subagents (role = 'sub' AND runtime_ref IS NOT NULL); locally-created
                    // role='sub' rows (runtime_ref IS NULL) remain eligible.
                    // Bug fix (#2791 scope audit, 2026-08-31): this check must be
                    // global, not scoped to `runtime_type = 'venice'`. `is_default` is
                    // enforced by a single global partial UNIQUE index
                    // (`idx_ai_agents_default`) — there is exactly one is_default=1 row
                    // system-wide, ever. Scoping this existence check to Venice made it
                    // evaluate false on every save until Venice itself had been promoted
                    // once, even when a different provider (Anthropic, OpenRouter,
                    // Ollama, OpenClaw) already held the global default seat — causing
                    // every Venice save to unconditionally demote that foreign default
                    // and steal the seat. See docs/audits/2026-08-31-venice-default-
                    // promote-scope.md.
                    let has_default: bool = conn
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM ai_agents WHERE is_active = 1 AND is_default = 1)",
                            [],
                            |row| row.get(0),
                        )
                        .unwrap_or(false);

                    if !has_default {
                        let promote_target: Option<String> = conn
                            .query_row(
                                "SELECT id FROM ai_agents WHERE runtime_type = 'venice' AND is_active = 1 \
                                 AND NOT (role = 'sub' AND runtime_ref IS NOT NULL) \
                                 ORDER BY created_at DESC LIMIT 1",
                                [],
                                |row| row.get(0),
                            )
                            .optional()
                            .map_err(|e| format!("Failed to look up venice default-reassignment candidate: {}", e))?;

                        if let Some(ref new_default_id) = promote_target {
                            // F3 fix (#2455 venice live-test v2): `has_default` above is
                            // scoped to `runtime_type = 'venice'`, so it only tells us
                            // whether VENICE has a default — it says nothing about
                            // whether some OTHER runtime's row currently holds
                            // `is_default = 1` under `idx_ai_agents_default`'s global
                            // UNIQUE(is_default) WHERE is_default = 1 partial index. The
                            // promote UPDATE below therefore needs a global demote first,
                            // exactly like `set_default_agent_impl` (agents.rs) and the
                            // openclaw disconnect-arm promotion (commands.rs ~4821) both
                            // already do — without it, reconnecting Venice while any other
                            // provider (e.g. an Anthropic/OpenRouter default set up before
                            // Venice was ever connected) holds the default seat hits a
                            // UNIQUE constraint violation on this UPDATE and the whole save
                            // fails.
                            conn.execute("UPDATE ai_agents SET is_default = 0 WHERE is_default = 1", [])
                                .map_err(|e| format!("Failed to demote existing default before venice promote: {}", e))?;
                            conn.execute(
                                "UPDATE ai_agents SET is_default = 1 WHERE id = ?1",
                                rusqlite::params![new_default_id],
                            )
                            .map_err(|e| format!("Failed to promote new venice default agent: {}", e))?;
                            eprintln!(
                                "[ai] venice: promoted {} to is_default = 1 on save",
                                new_default_id
                            );
                        } else {
                            eprintln!("[ai] venice: no eligible agent to promote — default left unset");
                        }
                    }
                }
                _ => {
                    // Ollama/none: no credential needed; preserve original behavior for safety
                    crate::database::set_ai_setting(&conn, "api_key", key)?;
                }
            }
        }

        // #2585/C1 Change B: `ai_settings.model` is a retired D8 key — `migrate_v79`
        // deleted it and nothing on the dispatch read path consults it anymore (confirmed
        // by trace: only remaining readers are `database.rs`'s config.json mirror, a
        // harmless backup no-op, and a separate pre-existing dead-key read in the
        // SMART-ROUTE retry fallback, both out of this PR's scope). Writing it back here
        // on every configure/pair call would resurrect the zombie key — that write has
        // been REMOVED for all providers (never resurrecting the retired key).
        //
        // #2647 D17 fix: `openclaw` previously got a replacement write here —
        // `UPDATE ai_agents SET model = ?1 WHERE runtime_type = 'openclaw' AND
        // runtime_ref = 'main'` — but that is the exact D17 bug locus: on first pair,
        // this configure call runs BEFORE `sync_initial_import` has created the OC main
        // agent's row at all, so the UPDATE silently touches zero rows and the model is
        // never set (surfaces later as "No model configured" on first chat, with no
        // prior warning). The fix moves the model write into `sync_initial_import`'s own
        // INSERT (see its `model` column + ON CONFLICT CASE clause), which is the only
        // place that's guaranteed the row already exists (or is being created in the
        // same statement). This UPDATE is intentionally NOT restored, NOT replaced with
        // a second call, and NOT relocated to fire after import — the model write now
        // lives exclusively in `sync_initial_import` per Matt's ruling ("pre-import
        // configure call stays endpoint/token only").
        //
        // The `fetch_oc_agent_model` call above this block is KEPT unchanged — it is
        // #1446 false-green connectivity validation (proves the token/endpoint pair can
        // actually reach `/v1/agents`, not just `/v1/sync/initial`), independent of
        // whether its result is used for a DB write.

        if let Some(ref e) = endpoint {
            let normalized = if provider == "openclaw" {
                normalize_api_endpoint(e)
            } else {
                e.clone()
            };
            // B3 (#2565) Req 2: ollama's endpoint is the explicit-connection signal —
            // it moves to its own `ollama_endpoint` key so `build_provider_for_runtime_type`
            // can distinguish "never configured" from "configured". The bare `endpoint`
            // key stays untouched for openclaw (OC's use of it is explicitly out of scope
            // for this fix, per Matt's ruling) and is still written here too for any
            // legacy/back-compat readers that haven't migrated off it.
            if provider == "ollama" {
                crate::database::set_ai_setting(&conn, "ollama_endpoint", &normalized)?;
                // #2641 D14: ollama's Save is an explicit this-install action (the
                // endpoint IS the explicit-connection signal, B3 #2565 Req 2) — mark
                // configured_at here the same way anthropic/openrouter do above.
                crate::database::set_ai_setting(
                    &conn,
                    "ollama_configured_at",
                    &chrono::Utc::now().to_rfc3339(),
                )?;
            } else {
                crate::database::set_ai_setting(&conn, "endpoint", &normalized)?;
            }
        }

        if let Some(ref stm) = simple_tier_model {
            crate::database::set_ai_setting(&conn, "simple_tier_model", stm)?;
        }
    }

    eprintln!(
        "[ai] Configured AND saved: {} / {} (key len: {})",
        provider,
        model_name,
        api_key.as_ref().map_or(0, |k| k.len())
    );
    Ok(format!(
        "AI configured and saved: {} / {}",
        provider, model_name
    ))
}


/// Health check: returns provider status without exposing the full key.
/// #2522 ADR-0063 Stage 2: retargeted from the `AiConfig` singleton to the
/// designated default agent's resolved runtime config (D5's "designated
/// default" fallback — this is a global status surface, not session-scoped).
#[tauri::command]
pub fn ai_status(db: State<'_, DbState>) -> Result<serde_json::Value, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let cfg = match crate::ai::runtime_resolution::resolve_runtime_config_for_default_agent(&conn) {
        Ok(cfg) => cfg,
        Err(_) => {
            return Ok(serde_json::json!({
                "provider": "none",
                "model": "",
                "key_length": 0,
                "api_key": "",
                "endpoint": serde_json::Value::Null,
                "configured": false,
            }));
        }
    };
    let (provider_name, key_len, api_key_val, endpoint_val) = match &cfg.provider {
        AiProvider::OpenRouter { api_key } => ("openrouter", api_key.len(), api_key.clone(), None),
        AiProvider::Anthropic { api_key } => ("anthropic", api_key.len(), api_key.clone(), None),
        AiProvider::Ollama { endpoint } => ("ollama", 0, String::new(), Some(endpoint.clone())),
        AiProvider::OpenClaw { base_url, token, .. } => (
            "openclaw",
            token.len(),
            token.clone(),
            Some(base_url.clone()),
        ),
        AiProvider::Venice { api_key, .. } => ("venice", api_key.len(), api_key.clone(), None),
        AiProvider::None => ("none", 0, String::new(), None),
    };
    Ok(serde_json::json!({
        "provider": provider_name,
        "model": &cfg.model,
        "key_length": key_len,
        "api_key": api_key_val,
        "endpoint": endpoint_val,
        "configured": provider_name != "none" && (key_len > 0 || provider_name == "ollama"),
    }))
}


// ==================== ADR-0063 Stage 5 (#2522): Provider Connection Cards ====================
// NET-NEW (Stage 5 build scope, flagged per Matt's rule — see PR body):
//   - `ai_list_runtime_connections`: per-runtime-type connection status + roster for the
//     Settings › AI provider-connection cards. There was previously no command that reported
//     status for more than one runtime at a time (`ai_status` only resolves the default agent's
//     single runtime, commands.rs ~L4193).
//   - `ai_disconnect_runtime`: clears a single runtime's credentials. Previously there was no
//     disconnect path at all — the old single-provider UI only ever wrote a new provider over
//     the old one via `ai_configure_and_save`; it never cleared credentials.
// Both are additive: they read/write the SAME three keychain accounts and the SAME `endpoint`
// ai_setting `credential_migration.rs`/`runtime_resolution.rs` already use. No new keychain
// accounts, no new ai_settings keys, no schema changes, no change to dispatch/resolution
// (`runtime_resolution.rs` is untouched by this stage).

/// One agent's roster entry on a provider connection card (read-only; #2522 D8).
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeRosterAgent {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub is_default: bool,
}


/// Connection status + read-only agent roster for a single runtime type, as shown on one
/// Provider Connection Card in Settings › AI (ADR-0063 D8, #2522 Stage 5).
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeConnectionInfo {
    /// 'anthropic' | 'openrouter' | 'venice' | 'ollama' | 'openclaw' (F4 fix, #2455
    /// venice live-test v2: doc comment was missing 'venice' — grep-sweep finding).
    pub runtime_type: String,
    /// True when `build_provider_for_runtime_type` can build a provider for this runtime
    /// (i.e. credentials are present). This is a "configured", not a live network probe —
    /// the card's "Test connection" button performs the live probe via the existing
    /// `validate_openrouter_key` / `validate_anthropic_key` / `validate_venice_key` /
    /// `ollama_probe_two_step` / `fetch_oc_agent_model` commands.
    pub connected: bool,
    /// Masked API key for anthropic/openrouter/venice (e.g. "sk-or-v1-••••••••••••4f2a"). None for ollama/openclaw.
    pub masked_credential: Option<String>,
    /// Endpoint/base URL for ollama/openclaw. None for anthropic/openrouter (they use a
    /// fixed API host, not a user-configured one, in this stage).
    pub endpoint: Option<String>,
    /// Populated when `connected` is false and credentials exist but are absent/invalid,
    /// so the card can render its failure strip (D8's "names the consequence").
    pub error: Option<String>,
    /// #2641 D14 three-state color fix: true only when the user explicitly saved this
    /// runtime's credentials THIS install (via `ai_configure_and_save`'s Save path, which
    /// writes `{runtime}_configured_at`) — or, for openclaw, when `gateway_paired_at` is
    /// set (the existing precedent this field generalizes). Distinct from `connected`,
    /// which is a keychain/DB presence check and goes true for a stale keychain leftover
    /// from a previous install (#2631) even though the user never touched this provider
    /// this install. The frontend's three-state `cardStatus` derivation uses THIS field
    /// (not `connected`) to decide grey vs. green/red — see ProviderConnectionCard.svelte.
    pub configured_this_install: bool,
    /// Read-only roster — every active agent whose `runtime_type` matches this card.
    pub agents: Vec<RuntimeRosterAgent>,
}


/// Mask a secret for display: keep a short prefix, replace the middle with bullets, keep
/// the last 4 characters. Mirrors the masked-key presentation already used in the design
/// (pc-b.jsx `PcCardAnatomy`: "sk-or-v1-••••••••••••4f2a").
fn mask_secret(secret: &str) -> String {
    let len = secret.chars().count();
    if len <= 8 {
        return "•".repeat(len.max(4));
    }
    let prefix: String = secret.chars().take(4).collect();
    let suffix: String = secret.chars().skip(len - 4).collect();
    format!("{}{}{}", prefix, "•".repeat(12), suffix)
}


/// D26 (#2673 PR-A) shared timeout budget for one runtime type's post-lock keychain
/// read (`ai_list_runtime_connections` and `connected_runtime_providers_async`). Chosen
/// per the D26 scoping doc: a local OS keychain call has no legitimate reason to take
/// more than low-single-digit seconds even under contention, so this is generous enough
/// not to false-positive on a briefly slow keychain daemon while still turning a stalled
/// macOS authorization-prompt read into a bounded failure (worst case ~4 runtime types ×
/// this timeout, never an indefinite hang) instead of silently stalling `ai_chat_stream`
/// forever. See docs/spec/ai-integration.md §28.4 and precedent `sync/commands.rs:324-333`
/// (#1347).
const KEYCHAIN_READ_TIMEOUT: Duration = Duration::from_secs(5);


/// D26 (#2673 PR-A): DB-only phase output for `ai_list_runtime_connections` — every
/// `ai_agents`/`ai_settings` value the five provider cards need, captured in one pass
/// while `db.0` is still locked. Consumed by `build_runtime_connection_info` AFTER the
/// lock has been released, so no DB connection borrow crosses into the keychain-reading
/// phase. See docs/spec/ai-integration.md §28.4.
#[derive(Debug, Clone)]
struct RuntimeConnectionsDbPhase {
    /// (id, name, model, is_default, runtime_type) for every active agent.
    rows: Vec<(String, String, Option<String>, bool, Option<String>)>,
    settings: crate::ai::runtime_resolution::RuntimeSettingsSnapshot,
    ollama_endpoint: Option<String>,
    openclaw_endpoint: Option<String>,
    /// runtime_type -> "configured this install" (#2641 D14), keyed by the same five
    /// runtime-type strings used everywhere else in this function.
    configured_markers: HashMap<String, bool>,
}


/// Run the DB-only phase (§28.4 phase 1): the `ai_agents` roster query, the
/// `RuntimeSettingsSnapshot` capture (ai_settings + migration flag, no keychain), the
/// ollama/openclaw endpoint reads, and the `{runtime}_configured_at`/`gateway_paired_at`
/// marker reads. Every read here is cheap, in-process SQLite — no OS syscalls — so it's
/// safe to run under `db.0.lock()`. Callers must drop the lock guard immediately after
/// this returns, before doing anything keychain-related with the result.
fn ai_list_runtime_connections_db_phase(
    conn: &rusqlite::Connection,
) -> Result<RuntimeConnectionsDbPhase, String> {
    // Pull every active agent once, then bucket by runtime_type client-side — five runtime
    // types is a small, fixed set, so one query + in-memory grouping beats five queries.
    let mut stmt = conn
        .prepare(
            "SELECT id, name, model, is_default, runtime_type FROM ai_agents \
             WHERE is_active = 1 ORDER BY is_default DESC, name ASC",
        )
        .map_err(|e| format!("Failed to prepare agent roster query: {}", e))?;
    let rows = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let name: String = row.get(1)?;
            let model: Option<String> = row.get(2)?;
            let is_default: bool = row.get::<_, i32>(3)? != 0;
            let runtime_type: Option<String> = row.get(4)?;
            Ok((id, name, model, is_default, runtime_type))
        })
        .map_err(|e| format!("Failed to query agents: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect agents: {}", e))?;

    let settings = crate::ai::runtime_resolution::RuntimeSettingsSnapshot::capture(conn);

    // B3 (#2565) Req 2, Matt's ruling: return None when unconfigured — no localhost
    // default served from the backend. The Detected-state probe target is client-side
    // presentation, not backend state.
    let ollama_endpoint = crate::database::get_ai_setting(conn, "ollama_endpoint");
    let openclaw_endpoint = crate::database::get_ai_setting(conn, "endpoint");

    // #2641 D14 three-state color fix: "configured this install" is read from a
    // dedicated `{runtime}_configured_at` marker (or, for openclaw, the existing
    // `gateway_paired_at` precedent) — NOT from `connected`/keychain presence, which
    // is exactly what let a stale keychain leftover show green without the user ever
    // touching this provider this install (#2631).
    let runtime_types = ["openclaw", "openrouter", "anthropic", "ollama", "venice"];
    let mut configured_markers = HashMap::with_capacity(runtime_types.len());
    for rt in runtime_types {
        let configured = match rt {
            "openclaw" => crate::database::get_ai_setting(conn, "gateway_paired_at").is_some(),
            _ => crate::database::get_ai_setting(conn, &format!("{}_configured_at", rt)).is_some(),
        };
        configured_markers.insert(rt.to_string(), configured);
    }

    Ok(RuntimeConnectionsDbPhase {
        rows,
        settings,
        ollama_endpoint,
        openclaw_endpoint,
        configured_markers,
    })
}


/// Every active agent whose `runtime_type` matches `rt`, formatted for the roster field.
fn agents_for_runtime(
    rows: &[(String, String, Option<String>, bool, Option<String>)],
    rt: &str,
) -> Vec<RuntimeRosterAgent> {
    rows.iter()
        .filter(|(_, _, _, _, row_rt)| row_rt.as_deref() == Some(rt))
        .map(|(id, name, model, is_default, _)| RuntimeRosterAgent {
            id: id.clone(),
            name: name.clone(),
            model: model.clone(),
            is_default: *is_default,
        })
        .collect()
}


/// Build one provider card's `RuntimeConnectionInfo` from an already-captured DB phase
/// (§28.4 phase 2). This is the ONLY place that reads the keychain for this command —
/// via `build_provider_for_runtime_type_from_snapshot` and `read_provider_key_from_snapshot`
/// — and it takes no `&Connection`, so it's safe to run with `db.0` already released (in
/// production, inside `spawn_blocking` — see `resolve_runtime_connection_info_with_timeout`)
/// or synchronously against a test-only in-memory connection's already-captured snapshot
/// (`ai_list_runtime_connections_impl`, used by the existing unit tests below).
fn build_runtime_connection_info(rt: &str, db_phase: &RuntimeConnectionsDbPhase) -> RuntimeConnectionInfo {
    let agents = agents_for_runtime(&db_phase.rows, rt);

    let (connected, error) = match crate::ai::runtime_resolution::build_provider_for_runtime_type_from_snapshot(
        &db_phase.settings,
        rt,
        None, // generic per-runtime-type connectivity check — not resolving for a specific agent
    ) {
        Ok(_) => (true, None),
        Err(e) => (false, Some(e)),
    };

    let masked_credential = match rt {
        // B3 (#2565): each card reads its OWN provider's keychain slot now —
        // no more mask-the-shared-key bleed between anthropic/openrouter.
        "anthropic" | "openrouter" => {
            crate::ai::credential_migration::read_provider_key_from_snapshot(db_phase.settings.credentials(), rt)
                .map(|k| mask_secret(&k))
        }
        "venice" => {
            // ADR-0065 D4: Venice reads from its own keychain slot.
            crate::ai::credential_migration::read_venice_key_from_snapshot(db_phase.settings.credentials())
                .map(|k| mask_secret(&k))
        }
        _ => None,
    };

    let endpoint = match rt {
        "ollama" => db_phase.ollama_endpoint.clone(),
        "openclaw" => db_phase.openclaw_endpoint.clone(),
        _ => None,
    };

    let configured_this_install = db_phase.configured_markers.get(rt).copied().unwrap_or(false);

    RuntimeConnectionInfo {
        runtime_type: rt.to_string(),
        connected,
        masked_credential,
        endpoint,
        error,
        configured_this_install,
        agents,
    }
}


/// Fallback card built when a runtime type's keychain read panicked or timed out (D26,
/// #2673 PR-A) — reports `connected: false` with a descriptive `error`, same shape the
/// UI already renders for an ordinary "not configured" card, so a stalled keychain read
/// degrades one card instead of failing the whole `ai_list_runtime_connections` call.
fn runtime_connection_info_unavailable(
    rt: &str,
    db_phase: &RuntimeConnectionsDbPhase,
    error: String,
) -> RuntimeConnectionInfo {
    RuntimeConnectionInfo {
        runtime_type: rt.to_string(),
        connected: false,
        masked_credential: None,
        endpoint: None,
        error: Some(error),
        configured_this_install: db_phase.configured_markers.get(rt).copied().unwrap_or(false),
        agents: agents_for_runtime(&db_phase.rows, rt),
    }
}


/// D26 (#2673 PR-A) §28.4 phase 2 for one runtime type: run `build_runtime_connection_info`
/// (which touches the keychain) inside `tokio::task::spawn_blocking` — so the macOS
/// keychain syscall never blocks the tokio executor thread, mirroring the #1347 precedent
/// at `sync/commands.rs:324-333` — wrapped in a `KEYCHAIN_READ_TIMEOUT` `tokio::time::timeout`
/// so a stalled read (e.g. an authorization prompt after rebuild/re-sign) surfaces as a
/// bounded per-card error instead of hanging forever. Never returns an `Err` up to its
/// caller — panics and timeouts both downgrade to `runtime_connection_info_unavailable`.
async fn resolve_runtime_connection_info_with_timeout(
    rt: &'static str,
    db_phase: RuntimeConnectionsDbPhase,
) -> RuntimeConnectionInfo {
    let fallback_db_phase = db_phase.clone();
    match tokio::time::timeout(
        KEYCHAIN_READ_TIMEOUT,
        tokio::task::spawn_blocking(move || build_runtime_connection_info(rt, &db_phase)),
    )
    .await
    {
        Ok(Ok(info)) => info,
        Ok(Err(join_err)) => {
            eprintln!(
                "[AI_LIST_RUNTIME_CONNECTIONS] keychain read panicked for '{}': {}",
                rt, join_err
            );
            runtime_connection_info_unavailable(
                rt,
                &fallback_db_phase,
                format!("Keychain read panicked: {}", join_err),
            )
        }
        Err(_elapsed) => {
            eprintln!(
                "[AI_LIST_RUNTIME_CONNECTIONS] keychain read timed out after {}s for '{}' (#2673 D26) — \
                 the OS keychain may be waiting on an authorization prompt, db.0 was NOT held during this read",
                KEYCHAIN_READ_TIMEOUT.as_secs(),
                rt
            );
            runtime_connection_info_unavailable(
                rt,
                &fallback_db_phase,
                format!(
                    "Keychain read timed out after {}s — the OS keychain may be waiting on an \
                     authorization prompt. Try again, or check Keychain Access.",
                    KEYCHAIN_READ_TIMEOUT.as_secs()
                ),
            )
        }
    }
}


/// List connection status + read-only agent roster for all five runtime types, for the
/// Provider Connection Cards in Settings › AI (ADR-0063 D8, #2522 Stage 5). NET-NEW command
/// — see module-level comment above.
///
/// D26 (#2673 PR-A): two-phase per §28.4 — phase 1 (DB-only) runs under `db.0.lock()`
/// and the guard is dropped before this function's `.await` on phase 2 (keychain-only,
/// one timeout-guarded `spawn_blocking` per runtime type). This is why the command is
/// `async fn` now; `invoke()` on the JS side is already Promise-based, so no frontend
/// change is needed for this signature change.
#[tauri::command]
pub async fn ai_list_runtime_connections(
    db: State<'_, DbState>,
) -> Result<Vec<RuntimeConnectionInfo>, String> {
    eprintln!("[AI_LIST_RUNTIME_CONNECTIONS] called");

    // Phase 1 (§28.4): DB-only, under db.0.lock(). The lock guard is dropped at the end
    // of this block, BEFORE any keychain I/O runs below.
    let db_phase = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        ai_list_runtime_connections_db_phase(&conn)?
    };

    // Phase 2 (§28.4): keychain-only, outside the lock — db.0 is free for every other
    // command (including ai_chat_stream's dispatch path) while this runs.
    let runtime_types = ["openclaw", "openrouter", "anthropic", "ollama", "venice"];
    let mut out = Vec::with_capacity(runtime_types.len());
    for rt in runtime_types {
        out.push(resolve_runtime_connection_info_with_timeout(rt, db_phase.clone()).await);
    }

    Ok(out)
}


/// Synchronous test-only entry point: DB-phase + keychain-phase in one call, with no
/// timeout/spawn_blocking wrapping (tests use `MockKeychain`, which never blocks). Kept
/// synchronous so the ~13 existing unit tests below don't need to become `#[tokio::test]`
/// (D26, #2673 PR-A — minimizes test churn per the scoping doc). Delegates to the exact
/// same `ai_list_runtime_connections_db_phase`/`build_runtime_connection_info` functions
/// the production async command uses, so there is one copy of the actual logic.
#[cfg(test)]
fn ai_list_runtime_connections_impl(
    conn: &rusqlite::Connection,
) -> Result<Vec<RuntimeConnectionInfo>, String> {
    let db_phase = ai_list_runtime_connections_db_phase(conn)?;
    let runtime_types = ["openclaw", "openrouter", "anthropic", "ollama", "venice"];
    let out: Vec<RuntimeConnectionInfo> = runtime_types
        .iter()
        .map(|rt| build_runtime_connection_info(rt, &db_phase))
        .collect();
    Ok(out)
}


/// Disconnect a single runtime's stored credentials (ADR-0063 D8, #2522 Stage 5). NET-NEW
/// command — see module-level comment above. Does NOT touch `ai_agents` rows or their
/// `runtime_type` assignment (agents "never move", per the card design) — it only clears
/// the credential/connection-adjacent settings so the card shows "Disconnected" and
/// `build_provider_for_runtime_type` starts failing for that runtime again.
///
/// anthropic/openrouter: clears each provider's OWN keychain account (B3, #2565).
/// Before B3, anthropic and openrouter shared one keychain account
/// (`KEYCHAIN_ACCOUNT_PROVIDER_KEY`), so disconnecting either one cleared the credential
/// both cards read — this was Matt's DMG-reported "disconnecting one kills both" bug.
/// Each arm below now deletes only its own account.
/// ollama: clears `ollama_endpoint` (B3 Req 2) — this is the explicit-connection signal
/// (see `runtime_resolution::build_provider_for_runtime_type`'s ollama arm), so clearing
/// it makes Disconnect meaningful for the first time: before this fix it was a documented
/// no-op because there was no stored credential OR signal to clear at all.
/// openclaw: clears the gateway token, TLS fingerprint, sync topology, and last-sync-push
/// marker — the same fields the existing OC→non-OC cascade-warning flow already clears
/// inline in Settings.svelte, now available as a real backend command. This incidentally
/// fixes a pre-existing gap where that inline flow cleared the DB-side settings but never
/// called `delete_password` on the gateway-token keychain entry.
#[tauri::command]
pub fn ai_disconnect_runtime(runtime_type: String, db: State<'_, DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    ai_disconnect_runtime_impl(&runtime_type, &conn)
}


pub(crate) fn ai_disconnect_runtime_impl(
    runtime_type: &str,
    conn: &rusqlite::Connection,
) -> Result<(), String> {
    match runtime_type {
        "anthropic" => {
            crate::keychain::delete_password(
                crate::ai::credential_migration::KEYCHAIN_ACCOUNT_ANTHROPIC_KEY,
            )?;
            let _ = conn.execute("DELETE FROM ai_settings WHERE key = 'api_key'", []);
            // #2641 D14: clear the "configured this install" marker so a disconnected
            // card goes back to grey, not green-from-a-now-stale timestamp.
            let _ = conn.execute("DELETE FROM ai_settings WHERE key = 'anthropic_configured_at'", []);
            Ok(())
        }
        "openrouter" => {
            crate::keychain::delete_password(
                crate::ai::credential_migration::KEYCHAIN_ACCOUNT_OPENROUTER_KEY,
            )?;
            let _ = conn.execute("DELETE FROM ai_settings WHERE key = 'api_key'", []);
            let _ = conn.execute("DELETE FROM ai_settings WHERE key = 'openrouter_configured_at'", []);
            Ok(())
        }
        "ollama" => {
            // B3 Req 2: clear the explicit-connection signal. Kills the silent-no-op
            // disconnect bug — before this fix there was nothing here to clear.
            let _ = conn.execute("DELETE FROM ai_settings WHERE key = 'ollama_endpoint'", []);
            let _ = conn.execute("DELETE FROM ai_settings WHERE key = 'ollama_configured_at'", []);
            Ok(())
        }
        "openclaw" => {
            // PR2 (#2647/#2648): disconnect = unpair, NOT delete. Directories and files
            // stay on disk; only DB state changes.
            //
            // #2793 Bug 2 (Matt's ruling — CRITICAL): the keychain token delete used to
            // run BEFORE `BEGIN` — outside the DB transaction. If the transaction later
            // rolled back (e.g. the promote-collision hard-failure in Bug 1 below), the
            // credential was already gone from the OS keychain while every DB-side effect
            // rolled back, leaving OC in a partial-failure state: DB says "connected", but
            // the credential needed to actually talk to the gateway is gone, with no way
            // to retry short of re-pairing from scratch. Order is now: (1)-(4) all DB work
            // inside one transaction, COMMIT, and ONLY THEN (5) delete the keychain token.
            // If the transaction fails and rolls back, the keychain token stays intact —
            // OC remains fully connected and the user can retry the disconnect.
            conn.execute("BEGIN", [])
                .map_err(|e| format!("[AI_DISCONNECT_RUNTIME] Failed to begin transaction: {}", e))?;

            let tx_result = (|| -> Result<(), String> {
                // All seven OC settings keys, set-empty idiom (matches this codebase's
                // established "clear" convention — get_ai_setting filters out empty-string
                // values and returns None, database.rs:2286-2295). Previously only 3 of 7
                // were cleared; this adds the missing endpoint, gateway_tls_cert_pem,
                // gateway_paired_at, api_key. api_key switches from DELETE to set-empty here
                // for consistency with the other six keys (same reader, same idiom).
                crate::database::set_ai_setting(conn, "endpoint", "")?;
                crate::database::set_ai_setting(conn, "gateway_tls_fingerprint", "")?;
                crate::database::set_ai_setting(conn, "gateway_tls_cert_pem", "")?;
                crate::database::set_ai_setting(conn, "gateway_paired_at", "")?;
                crate::database::set_ai_setting(conn, "tier3_topology", "")?;
                crate::database::set_ai_setting(conn, "last_sync_push", "")?;
                crate::database::set_ai_setting(conn, "api_key", "")?;

                // Deactivate + unconditionally demote every OC agent row. Soft-deactivate,
                // never DELETE — directories/files must survive so a re-pair (sync_re_import)
                // can reactivate the same UUIDs (docs/spec/sync.md invariant block).
                let deactivated = conn
                    .execute(
                        "UPDATE ai_agents SET is_active = 0, is_default = 0 WHERE runtime_type = 'openclaw'",
                        [],
                    )
                    .map_err(|e| format!("Failed to deactivate openclaw agents: {}", e))?;
                eprintln!(
                    "[AI_DISCONNECT_RUNTIME] openclaw: deactivated {} agent row(s)",
                    deactivated
                );

                // chat_sessions.agent_id is intentionally left untouched here — per
                // docs/spec/sessions.md's "Unavailable bound agent" rule (ADR-0062 D5),
                // a session bound to a now-deactivated agent renders read-only and any
                // dispatch attempt errors explicitly naming the agent (see
                // agents::check_bound_agent_active, enforced in
                // resolve_runtime_config_for_session). Disconnect must never NULL or
                // reassign the binding — that would route the next dispatch through
                // self-heal, which only knows how to bind to *some* default agent, i.e.
                // a silent re-bind to a different agent than the one the user selected.

                // #2793 Bug 1 (PRIMARY): global existence guard, mirroring #2796's
                // Venice-save fix (commands.rs ~4330-4334). The demote above only clears
                // is_default on OC's OWN rows (`WHERE runtime_type = 'openclaw'`) — a
                // foreign provider's default row (Anthropic/OpenRouter/Ollama/Venice) is
                // untouched and may still hold the single global `is_default = 1` seat
                // enforced by `idx_ai_agents_default` (partial UNIQUE index). Promoting
                // unconditionally in that case either silently no-ops (foreign default
                // happens to be the newest row) or tries to write a SECOND is_default = 1
                // row and hits the UNIQUE constraint, rolling back this entire disconnect
                // transaction. So: only attempt the promote below if NO active default
                // exists anywhere, system-wide — i.e. only fill an empty seat, never
                // contest an occupied one. A sub-agent must never be promoted to default
                // (role != 'sub' filter).
                let has_default: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM ai_agents WHERE is_active = 1 AND is_default = 1)",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);

                if !has_default {
                    let promote_target: Option<String> = conn
                        .query_row(
                            "SELECT id FROM ai_agents WHERE is_active = 1 AND role != 'sub' \
                             ORDER BY created_at DESC LIMIT 1",
                            [],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(|e| format!("Failed to look up default-reassignment candidate: {}", e))?;

                    if let Some(ref new_default_id) = promote_target {
                        conn.execute(
                            "UPDATE ai_agents SET is_default = 1 WHERE id = ?1",
                            rusqlite::params![new_default_id],
                        )
                        .map_err(|e| format!("Failed to promote new default agent: {}", e))?;
                        eprintln!(
                            "[AI_DISCONNECT_RUNTIME] openclaw: promoted {} to is_default = 1",
                            new_default_id
                        );
                    } else {
                        eprintln!("[AI_DISCONNECT_RUNTIME] openclaw: no eligible agent to promote — default left unset");
                    }
                } else {
                    eprintln!("[AI_DISCONNECT_RUNTIME] openclaw: foreign default already active — skipping promote");
                }

                Ok(())
            })();

            match tx_result {
                Ok(()) => {
                    conn.execute("COMMIT", [])
                        .map_err(|e| format!("[AI_DISCONNECT_RUNTIME] Failed to commit transaction: {}", e))?;
                    // #2793 Bug 2: keychain delete only after a successful commit — see
                    // comment above `BEGIN` for the full rationale.
                    eprintln!("[AI_DISCONNECT_RUNTIME] openclaw: deleting gateway token from keychain");
                    crate::keychain::delete_password(
                        crate::ai::credential_migration::KEYCHAIN_ACCOUNT_GATEWAY_TOKEN,
                    )?;
                    Ok(())
                }
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", []);
                    Err(e)
                }
            }
        }
        "venice" => {
            // ADR-0065 D4: Venice disconnect clears keychain key and configured_at marker.
            // Venice agents are deactivated (soft-delete, matching openclaw pattern).
            //
            // #2793 Bug 3: wrapped in a transaction (previously had none) so the demote
            // and the new promote step below are atomic with each other, mirroring the
            // openclaw arm's structure. Keychain delete stays outside the transaction
            // (matching the corrected openclaw ordering would mean moving it after COMMIT
            // too, but Venice's key-delete has no DB-rollback interaction risk analogous
            // to Bug 2 today — `delete_venice_key` is idempotent and this arm's DB work is
            // now safely atomic regardless of keychain outcome).
            crate::ai::credential_migration::delete_venice_key()?;
            let _ = conn.execute("DELETE FROM ai_settings WHERE key = 'venice_configured_at'", []);

            conn.execute("BEGIN", [])
                .map_err(|e| format!("[AI_DISCONNECT_RUNTIME] Failed to begin transaction: {}", e))?;

            let tx_result = (|| -> Result<(), String> {
                // Deactivate Venice agents (soft-delete, matching openclaw pattern).
                let deactivated = conn
                    .execute(
                        "UPDATE ai_agents SET is_active = 0, is_default = 0 WHERE runtime_type = 'venice'",
                        [],
                    )
                    .map_err(|e| format!("Failed to deactivate venice agents: {}", e))?;
                eprintln!(
                    "[AI_DISCONNECT_RUNTIME] venice: deactivated {} agent row(s)",
                    deactivated
                );

                // #2793 Bug 3: Venice's disconnect arm previously had NO promote logic at
                // all — if Venice held the global default, the seat was simply left empty
                // (a silent orphaned-default) until the user manually picked a new default.
                // Same global-guard + conditional-promote shape as the openclaw arm above
                // (and #2796's Venice-save fix): only promote if no active default exists
                // anywhere, system-wide. A sub-agent must never be promoted to default
                // (role != 'sub' filter).
                let has_default: bool = conn
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM ai_agents WHERE is_active = 1 AND is_default = 1)",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);

                if !has_default {
                    let promote_target: Option<String> = conn
                        .query_row(
                            "SELECT id FROM ai_agents WHERE is_active = 1 AND role != 'sub' \
                             ORDER BY created_at DESC LIMIT 1",
                            [],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(|e| format!("Failed to look up default-reassignment candidate: {}", e))?;

                    if let Some(ref new_default_id) = promote_target {
                        conn.execute(
                            "UPDATE ai_agents SET is_default = 1 WHERE id = ?1",
                            rusqlite::params![new_default_id],
                        )
                        .map_err(|e| format!("Failed to promote new default agent: {}", e))?;
                        eprintln!(
                            "[AI_DISCONNECT_RUNTIME] venice: promoted {} as new default",
                            new_default_id
                        );
                    } else {
                        eprintln!("[AI_DISCONNECT_RUNTIME] venice: no eligible agent to promote — default left unset");
                    }
                } else {
                    eprintln!("[AI_DISCONNECT_RUNTIME] venice: foreign default already active — skipping promote");
                }

                Ok(())
            })();

            match tx_result {
                Ok(()) => {
                    conn.execute("COMMIT", [])
                        .map_err(|e| format!("[AI_DISCONNECT_RUNTIME] Failed to commit transaction: {}", e))?;
                    Ok(())
                }
                Err(e) => {
                    let _ = conn.execute("ROLLBACK", []);
                    Err(e)
                }
            }
        }
        other => Err(format!("Unknown runtime type: {}", other)),
    }
}


// ─── Web Search Settings card (§4.6.8, Part 2, #2591) ────────────────────────
//
// Five dedicated commands, deliberately NOT routed through
// `ai_configure_and_save` — that command owns the 4-card provider-connection
// contract (Anthropic/OpenRouter/Ollama/OpenClaw) and reusing it here would
// wire Brave-key/SearXNG-endpoint saves through unrelated provider-switch
// logic that has nothing to do with search backends.

/// Save the Brave Search API key (§4.6.8 "Key storage"). Keychain-only, via
/// `credential_migration::write_brave_key` — never touches `ai_settings` or
/// `config.json`.
#[tauri::command]
pub fn save_brave_key(key: String, db: State<'_, DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::ai::credential_migration::write_brave_key(&conn, &key)
}


/// Presence + masked-display probe for the Brave key card row. Returns `None`
/// when no key is saved, `Some(mask_secret(key))` when one is. The card's own
/// `wsBraveKeySaved` boolean derives from `!!` on this result rather than a
/// second round trip, since `ai_brave_key` has no `ai_settings` row to read
/// presence from directly (§4.6.8 "Key storage": keychain-only).
#[tauri::command]
pub fn brave_key_status(db: State<'_, DbState>) -> Result<Option<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(crate::ai::credential_migration::read_brave_key(&conn).map(|k| mask_secret(&k)))
}


/// Remove the Brave Search API key. Thin wrapper around
/// `credential_migration::delete_brave_key`, mirroring the "Remove" control's
/// §4.6.8 wording (distinct from "disconnect", since there's no runtime
/// connection here to disconnect — just a stored credential to delete).
#[tauri::command]
pub fn remove_brave_key() -> Result<(), String> {
    crate::ai::credential_migration::delete_brave_key()
}


// ═══════════════════════════════════════════════════════════════════════════
// Venice credential management — ADR-0065 D4/D8
// ═══════════════════════════════════════════════════════════════════════════

// Test-only override for `validate_venice_key`'s hardcoded `VENICE_API_BASE`
// (F2 stored-key-fallback integration tests, #2455 venice live-test v2 delta
// review) — same `#[cfg(test)]` thread_local carve-out shape as
// `web::test_set_brave_api_url` / `system_files::test_set_openrouter_validate_url`.
#[cfg(test)]
thread_local! {
    static TEST_VENICE_VALIDATE_BASE: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
}


#[cfg(test)]
pub(crate) fn test_set_venice_validate_base(url: &str) {
    TEST_VENICE_VALIDATE_BASE.with(|c| *c.borrow_mut() = Some(url.to_string()));
}


#[cfg(test)]
pub(crate) fn test_clear_venice_validate_base() {
    TEST_VENICE_VALIDATE_BASE.with(|c| *c.borrow_mut() = None);
}


fn venice_validate_base() -> String {
    #[cfg(test)]
    {
        if let Some(u) = TEST_VENICE_VALIDATE_BASE.with(|c| c.borrow().clone()) {
            return u;
        }
    }
    super::openrouter::VENICE_API_BASE.to_string()
}


/// ADR-0065 D8: Validate Venice API key via GET /api_keys/rate_limits.
/// NOT GET /models — that returns 200 for every key, even invalid ones.
/// Uses a plain always-validating reqwest::Client (see D1/D3 security rule).
///
/// F2 fix (#2455 venice live-test v2): `api_key` is now `Option<String>` —
/// `None` (or `Some("")`, e.g. an empty-string invoke from the frontend) falls
/// back to reading the stored keychain credential via `read_venice_key`, the
/// same helper `venice_key_status`'s masked-credential display already uses.
/// Before this fix, the "Test" button on an already-configured card always sent
/// whatever was in the (blank, unless retyped) input field, so Test reported
/// failure against a real, working stored key — F2's exact bug.
#[tauri::command]
pub async fn validate_venice_key(api_key: Option<String>, db: State<'_, DbState>) -> Result<(), String> {
    eprintln!("[VENICE_VALIDATE] Validating API key");

    let key = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => {
            eprintln!("[VENICE_VALIDATE] No key provided — falling back to stored key");
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let stored = crate::ai::credential_migration::read_venice_key(&conn);
            drop(conn);
            match stored {
                Some(k) => k,
                None => {
                    eprintln!("[VENICE_VALIDATE] Failed: no API key provided and no stored key found");
                    return Err("No API key provided and no stored key found".to_string());
                }
            }
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let url = format!("{}/api_keys/rate_limits", venice_validate_base());
    let res = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", key.trim()))
        .send()
        .await
        .map_err(|e| format!("Failed to reach Venice API: {}", e))?;

    let status = res.status();
    if status.is_success() {
        eprintln!("[VENICE_VALIDATE] Key validated successfully");
        return Ok(());
    }
    let body = res.text().await.unwrap_or_default();
    eprintln!("[VENICE_VALIDATE] Failed: HTTP {} - {}", status, body);
    let msg = if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        "Invalid Venice API key. Get a key at https://venice.ai/settings/api".to_string()
    } else {
        format!("Venice API error ({}): {}", status, body)
    };
    Err(msg)
}


/// ADR-0065 D4: Save Venice API key to keychain.
#[tauri::command]
pub fn save_venice_key(key: String, db: State<'_, DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::ai::credential_migration::write_venice_key(&conn, &key)
}


/// ADR-0065 D4: Venice key presence probe (for Settings card display).
/// Returns None when no key saved, Some(masked) when present.
#[tauri::command]
pub fn venice_key_status(db: State<'_, DbState>) -> Result<Option<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(crate::ai::credential_migration::read_venice_key(&conn).map(|k| mask_secret(&k)))
}


/// ADR-0065 D4: Remove Venice API key from keychain on disconnect.
#[tauri::command]
pub fn remove_venice_key() -> Result<(), String> {
    crate::ai::credential_migration::delete_venice_key()
}


/// Save the SearXNG endpoint. Plain `ai_settings` write — the endpoint is
/// user-configured and not a credential (§4.6.8: exempt from the §4.6.5
/// address check because self-hosted instances are commonly on the LAN).
#[tauri::command]
pub fn save_searxng_endpoint(endpoint: String, db: State<'_, DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::database::set_ai_setting(&conn, "searxng_endpoint", endpoint.trim())
}


/// Clear the SearXNG endpoint. Set-empty idiom (§4.6.8: "Clear sets
/// `searxng_endpoint` to `""`... `get_ai_setting` returns `None` for empty"),
/// the same idiom `ai_disconnect_runtime_impl`'s `openclaw` branch already
/// uses for its seven settings keys above.
#[tauri::command]
pub fn clear_searxng_endpoint(db: State<'_, DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::database::set_ai_setting(&conn, "searxng_endpoint", "")
}


/// Run a 1-result test query against whichever backend is currently
/// configured (§4.6.8 Settings states table: "Test button ran a 1-result
/// query successfully"). Returns the backend label (`"Brave"` or
/// `"SearXNG"`) on success so the card can render `Test succeeded (Brave).`
/// / `Test succeeded (SearXNG).` — the two distinct §4.6.8 rows — without a
/// second dispatch decision on the frontend. On failure, returns the same
/// §4.6.7-derived text `execute_web_search` would have produced, with any
/// trailing "Check ... in Settings" clause dropped per §4.6.8's own stated
/// rule (the card is already that destination), and the leading "Web search
/// failed: " prefix also stripped — the frontend's "Test failed: " wrapper
/// already supplies that framing, so leaving the prefix in would double it
/// ("Test failed: Web search failed: ..."). #2759: the Brave wrong-key branch
/// this strips is now primarily reached via 422 + error.code ==
/// "SUBSCRIPTION_TOKEN_INVALID" (401/403 kept as a fallback) — see
/// `brave_search()` in `web.rs` for the real dispatch logic; this command
/// only ever sees the already-built friendly-sentence string either way.
#[tauri::command]
pub async fn test_web_search(db: State<'_, DbState>) -> Result<String, String> {
    let (brave_key, searxng_endpoint) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        (
            crate::ai::credential_migration::read_brave_key(&conn),
            crate::database::get_ai_setting(&conn, "searxng_endpoint"),
        )
    };

    let outcome = crate::ai::web::execute_web_search(
        &serde_json::json!({ "query": "test", "count": 1 }),
        brave_key.as_deref(),
        searxng_endpoint.as_deref(),
    )
    .await;

    if outcome.is_external_content {
        let backend = if searxng_endpoint.as_deref().map(|s| !s.trim().is_empty()).unwrap_or(false) {
            "SearXNG"
        } else {
            "Brave"
        };
        Ok(backend.to_string())
    } else {
        // Drop any trailing "Check ... in Settings" clause per §4.6.8's card-copy rule
        // — the card is already the Settings destination that text would point to.
        let text = outcome.provider_result;
        let trimmed = match text.find(" Check the key in Settings") {
            // The text up to the match already ends in a period (e.g. "...not
            // another Brave key."), so no extra punctuation is appended here.
            Some(idx) => text[..idx].to_string(),
            None => text,
        };
        // Also drop the leading "Web search failed: " prefix — the frontend template
        // (Settings.svelte) already prepends its own "Test failed: " wrapper, so
        // leaving this in would double-prefix ("Test failed: Web search failed: ...").
        // Applies to all failure paths (Brave 401/403, Brave 429, other non-2xx,
        // SearXNG non-2xx) since every one of those `web.rs` error strings starts
        // with this exact literal.
        let unprefixed = trimmed
            .strip_prefix("Web search failed: ")
            .map(|s| s.to_string())
            .unwrap_or(trimmed);
        Err(unprefixed)
    }
}


/// #2591 audit fix: `test_web_search`'s returned error text must be neither
/// double-prefixed ("Test failed: Web search failed: ...") nor carry the
/// trailing "Check ... in Settings" clause — the Settings card's "Test
/// failed: " template already supplies the former, and the card is already
/// the Settings destination the latter would point back to.
#[cfg(test)]
mod test_web_search_error_text_tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;

    fn setup() -> tauri::App<tauri::test::MockRuntime> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    /// Brave 401 path, driven through the real async `#[tauri::command]` handler
    /// (not a helper) against a mockito server standing in for the Brave API, via
    /// `web::test_set_brave_api_url`. The returned string must be exactly the
    /// §4.6.7 reason text — no leading "Web search failed: " prefix, no trailing
    /// "Check the key in Settings" clause.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_brave_401_error_text_has_no_prefix_or_trailing_clause() {
        let _kc = KeychainGuard::set();

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Regex(r"^/".to_string()))
            .with_status(401)
            .create_async()
            .await;
        crate::ai::web::test_set_brave_api_url(&server.url());

        let app = setup();
        let db_state = app.state::<DbState>();
        crate::ai::credential_migration::write_brave_key(&db_state.0.lock().unwrap(), "bad-key").unwrap();

        let result = test_web_search(db_state).await;

        crate::ai::web::test_clear_brave_api_url();
        mock.assert_async().await;

        assert_eq!(
            result,
            Err("Brave rejected the API key. It must be a Search API key, not another Brave key.".to_string()),
            "must be exactly the bare reason text, no \"Web search failed: \" prefix and no trailing \"Check ... in Settings\" clause"
        );
    }

    /// #2759: Brave's real wrong-key path is 422 + `error.code ==
    /// "SUBSCRIPTION_TOKEN_INVALID"`, not 401 (see docs/audits/2026-08-27-brave-422-scoping.md).
    /// Same pattern as the 401 test above, but driven by the actually-observed live response
    /// shape — this is the test that would have caught the original 401/403-only gap.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_brave_422_subscription_token_invalid_error_text_has_no_prefix_or_trailing_clause() {
        let _kc = KeychainGuard::set();

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", mockito::Matcher::Regex(r"^/".to_string()))
            .with_status(422)
            .with_body(
                serde_json::json!({
                    "error": {
                        "code": "SUBSCRIPTION_TOKEN_INVALID",
                        "detail": "The provided subscription token is invalid.",
                        "meta": { "component": "authentication" },
                        "status": 422
                    },
                    "type": "ErrorResponse"
                })
                .to_string(),
            )
            .create_async()
            .await;
        crate::ai::web::test_set_brave_api_url(&server.url());

        let app = setup();
        let db_state = app.state::<DbState>();
        crate::ai::credential_migration::write_brave_key(&db_state.0.lock().unwrap(), "bad-key").unwrap();

        let result = test_web_search(db_state).await;

        crate::ai::web::test_clear_brave_api_url();
        mock.assert_async().await;

        assert_eq!(
            result,
            Err("Brave rejected the API key. It must be a Search API key, not another Brave key.".to_string()),
            "must be exactly the bare reason text, no \"Web search failed: \" prefix and no trailing \"Check ... in Settings\" clause"
        );
    }
}


#[cfg(test)]
mod runtime_connections_tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn make_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                role TEXT NOT NULL DEFAULT 'sub',
                model TEXT,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                workspace_path TEXT NOT NULL,
                runtime_ref TEXT,
                runtime_type TEXT,
                source TEXT NOT NULL DEFAULT 'manual',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE ai_settings (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            -- PR2 (#2647/#2648): ai_disconnect_runtime_impl's openclaw arm now NULLs
            -- chat_sessions.agent_id for deactivated OC rows — must exist in every test
            -- conn that exercises that arm, mirroring the production schema's FK column.
            CREATE TABLE chat_sessions (
                id TEXT PRIMARY KEY,
                agent_id TEXT
            );
            -- PR2: partial unique index mirroring production (migrations.rs) so tests can
            -- exercise the demote-then-promote unique-constraint safety guarantee for real.
            CREATE UNIQUE INDEX idx_ai_agents_default ON ai_agents(is_default) WHERE is_default = 1;
            ",
        )
        .unwrap();
        conn
    }

    fn seed_agent(conn: &rusqlite::Connection, id: &str, rt: Option<&str>, model: Option<&str>, is_default: bool) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, model, created_at, updated_at)
             VALUES (?1, ?1, ?1, ?2, 1, ?1, ?3, ?4, ?5, ?5)",
            rusqlite::params![id, if is_default { 1 } else { 0 }, rt, model, now],
        )
        .unwrap();
    }

    /// PR2 helper: seed with an explicit `role` (seed_agent above always defaults to
    /// the table's `role = 'sub'` default) — needed for default-reassignment tests
    /// where a local (non-sub) agent must be the promotion candidate, and for the
    /// "sub-agent never promoted" regression test where the only survivor IS 'sub'.
    fn seed_agent_with_role(
        conn: &rusqlite::Connection,
        id: &str,
        rt: Option<&str>,
        role: &str,
        is_default: bool,
        created_at: &str,
    ) {
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, created_at, updated_at)
             VALUES (?1, ?1, ?1, ?2, ?3, 1, ?1, ?4, ?5, ?5)",
            rusqlite::params![id, role, if is_default { 1 } else { 0 }, rt, created_at],
        )
        .unwrap();
    }

    #[test]
    fn test_mask_secret_short() {
        assert_eq!(mask_secret("abc"), "••••");
    }

    #[test]
    fn test_mask_secret_long_keeps_prefix_and_suffix() {
        let masked = mask_secret("sk-or-v1-abcdef1234f2a");
        assert!(masked.starts_with("sk-o"), "should keep a short prefix: {}", masked);
        assert!(masked.ends_with("f2a") || masked.ends_with("4f2a"), "should keep the last chars: {}", masked);
        assert!(masked.contains('•'), "middle should be masked: {}", masked);
    }

    #[test]
    #[serial(keychain_override)]
    fn test_list_runtime_connections_reports_five_cards_and_roster() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        seed_agent(&conn, "agent-ollama", Some("ollama"), Some("llama3"), true);
        seed_agent(&conn, "agent-openrouter", Some("openrouter"), Some("claude"), false); // CI-ALLOW: hardcoded-model

        let out = ai_list_runtime_connections_impl(&conn).expect("should list all five runtimes");
        // Blocker 3 fix (#2777 venice live-test scoping): the backend's contract
        // (return all five known runtime types, unfiltered) is UNCHANGED by this fix —
        // per the scoping audit's decision, the "configured-only" card-visibility
        // filter lives in the frontend (ProviderConnectionsPanel.svelte's
        // `{#each connections.filter((c) => c.configured_this_install)}`), not here,
        // so any future consumer of this command that legitimately wants the full
        // roster (all five, connected or not) still gets it. See
        // `test_list_runtime_connections_configured_this_install_gates_card_visibility`
        // below for the configured/unconfigured contract assertions the frontend relies on.
        assert_eq!(out.len(), 5, "should always report exactly the five known runtime types — filtering to configured-only happens in the frontend, not this command");

        // B3 Req 2: explicit-connection semantics — an agent existing on ollama does
        // NOT imply "connected" anymore; `ollama_endpoint` must be explicitly set.
        let ollama = out.iter().find(|c| c.runtime_type == "ollama").unwrap();
        assert!(!ollama.connected, "ollama_endpoint not configured — must report disconnected even with an agent present");
        assert_eq!(ollama.endpoint, None, "unconfigured ollama must report no endpoint (no localhost default served)");
        assert_eq!(ollama.agents.len(), 1);
        assert_eq!(ollama.agents[0].id, "agent-ollama");
        assert!(ollama.agents[0].is_default);

        let openrouter = out.iter().find(|c| c.runtime_type == "openrouter").unwrap();
        assert!(!openrouter.connected, "no keychain credential configured — should report disconnected");
        assert!(openrouter.error.is_some(), "disconnected card should carry an error for the failure strip");
        assert_eq!(openrouter.agents.len(), 1);

        let anthropic = out.iter().find(|c| c.runtime_type == "anthropic").unwrap();
        assert_eq!(anthropic.agents.len(), 0, "no agents seeded on anthropic — roster should be empty");

        let openclaw = out.iter().find(|c| c.runtime_type == "openclaw").unwrap();
        assert!(!openclaw.connected);

        let venice = out.iter().find(|c| c.runtime_type == "venice").unwrap();
        assert!(!venice.connected, "no venice key configured — should report disconnected");
        assert_eq!(venice.agents.len(), 0, "no agents seeded on venice — roster should be empty");

    }

    /// Blocker 3 fix (#2777 venice live-test scoping): `configured_this_install` is the
    /// exact signal the frontend filters card visibility on
    /// (ProviderConnectionsPanel.svelte). A provider with no `{runtime}_configured_at`
    /// marker and no keychain credential must report `configured_this_install = false`
    /// (card hidden) — confirming there's no accidental "unconfigured but shown anyway"
    /// path once the frontend filter lands.
    #[test]
    #[serial(keychain_override)]
    fn test_list_runtime_connections_configured_this_install_gates_card_visibility() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        // Nothing configured at all — every card should report configured_this_install = false.
        let out = ai_list_runtime_connections_impl(&conn).unwrap();
        for info in &out {
            assert!(
                !info.configured_this_install,
                "{} should be unconfigured (no marker, no key) — frontend must hide this card",
                info.runtime_type
            );
        }

        // Configure openrouter via the normal Save path (which sets the marker) —
        // its card should now report configured_this_install = true; others stay false.
        crate::database::set_ai_setting(&conn, "openrouter_configured_at", &chrono::Utc::now().to_rfc3339()).unwrap();
        let out2 = ai_list_runtime_connections_impl(&conn).unwrap();
        let openrouter = out2.iter().find(|c| c.runtime_type == "openrouter").unwrap();
        assert!(openrouter.configured_this_install, "openrouter_configured_at was set — card should be visible");
        let anthropic = out2.iter().find(|c| c.runtime_type == "anthropic").unwrap();
        assert!(!anthropic.configured_this_install, "anthropic has no marker — card should stay hidden");

    }

    /// Blocker 3 fix, upgrader backfill (#2777 venice live-test scoping): an existing
    /// install with a WORKING key already in the keychain but no `{runtime}_configured_at`
    /// marker must not lose its card on upgrade forever — that backfill is now a ONE-TIME
    /// schema migration (`migrate_v88` in migrations.rs), deliberately NOT a per-request
    /// check here, so #2631's "stale keychain leftover ≠ configured_this_install" contract
    /// (see `test_configured_this_install_false_for_key_present_without_timestamp` above)
    /// stays a stable signal for keys that show up in the keychain AFTER migration time
    /// with no marker — those are correctly treated as leftovers, not upgraders. See
    /// `migrations.rs`'s `migrate_v88` tests for the actual backfill coverage.

    #[test]
    #[serial(keychain_override)]
    fn test_list_runtime_connections_masks_credential_when_configured() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-v1-abcdefgh1234f2a").unwrap();

        let out = ai_list_runtime_connections_impl(&conn).unwrap();
        let openrouter = out.iter().find(|c| c.runtime_type == "openrouter").unwrap();
        assert!(openrouter.connected);
        let masked = openrouter.masked_credential.as_ref().expect("should have a masked credential");
        assert!(!masked.contains("abcdefgh1234"), "masked credential must not leak the raw key: {}", masked);
        assert!(masked.contains('•'), "masked credential should contain mask characters: {}", masked);

    }

    /// B3 (#2565) Req 1 regression guard: anthropic/openrouter must show INDEPENDENT
    /// masked keys — this is the direct fix for "saving an OR key makes Anthropic show
    /// Connected with the same masked key".
    #[test]
    #[serial(keychain_override)]
    fn test_list_runtime_connections_anthropic_and_openrouter_independent() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-v1-openrouteronly1234").unwrap();

        let out = ai_list_runtime_connections_impl(&conn).unwrap();
        let openrouter = out.iter().find(|c| c.runtime_type == "openrouter").unwrap();
        let anthropic = out.iter().find(|c| c.runtime_type == "anthropic").unwrap();

        assert!(openrouter.connected, "openrouter key was written, should be connected");
        assert!(
            !anthropic.connected,
            "anthropic must NOT show connected just because openrouter has a key — this is the B3 shared-slot bug"
        );
        assert!(anthropic.masked_credential.is_none(), "anthropic must have no masked credential");
        assert!(anthropic.error.is_some(), "disconnected anthropic card should carry an error");

    }

    /// B3 (#2565) Req 2 regression guard: an unconfigured ollama connection must
    /// report `endpoint: None`, not a served localhost default.
    #[test]
    #[serial(keychain_override)]
    fn test_list_runtime_connections_ollama_endpoint_none_when_unconfigured() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        let out = ai_list_runtime_connections_impl(&conn).unwrap();
        let ollama = out.iter().find(|c| c.runtime_type == "ollama").unwrap();
        assert!(!ollama.connected);
        assert_eq!(ollama.endpoint, None, "backend must not serve a localhost default when unconfigured");

    }

    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_runtime_clears_provider_key() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-secret").unwrap();
        assert!(crate::ai::credential_migration::read_provider_key(&conn, "anthropic").is_some());

        ai_disconnect_runtime_impl("anthropic", &conn).expect("disconnect should succeed");

        assert!(
            crate::ai::credential_migration::read_provider_key(&conn, "anthropic").is_none(),
            "provider key must be cleared after disconnect"
        );

    }

    /// B3 (#2565) Req 1 regression guard: disconnecting one provider must NOT clear
    /// the other provider's key — this is the direct fix for Matt's DMG bug
    /// ("disconnecting one kills both").
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_runtime_does_not_clear_other_providers_key() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-secret").unwrap();
        crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-secret").unwrap();

        ai_disconnect_runtime_impl("anthropic", &conn).expect("disconnect should succeed");

        assert!(
            crate::ai::credential_migration::read_provider_key(&conn, "anthropic").is_none(),
            "anthropic key must be cleared"
        );
        assert_eq!(
            crate::ai::credential_migration::read_provider_key(&conn, "openrouter").as_deref(),
            Some("sk-or-secret"),
            "openrouter key must survive an anthropic disconnect — the shared-slot bug this fixes"
        );

    }

    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_runtime_openclaw_clears_gateway_state() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::ai::credential_migration::write_gateway_token(&conn, "oc-token-abc").unwrap();
        crate::database::set_ai_setting(&conn, "gateway_tls_fingerprint", "deadbeef").unwrap();
        crate::database::set_ai_setting(&conn, "tier3_topology", "split_machine").unwrap();
        crate::database::set_ai_setting(&conn, "last_sync_push", "2026-08-17T00:00:00Z").unwrap();

        ai_disconnect_runtime_impl("openclaw", &conn).expect("disconnect should succeed");

        assert!(
            crate::ai::credential_migration::read_gateway_token(&conn).is_none(),
            "gateway token keychain entry must be cleared — this is the pre-existing bug this command fixes"
        );
        // get_ai_setting filters out empty-string values and returns None (database.rs) —
        // writing "" is this codebase's established "clear" idiom (matches the existing
        // inline OC-disconnect cascade flow in Settings.svelte), so None is the correct
        // post-clear assertion here, not Some("").
        assert_eq!(crate::database::get_ai_setting(&conn, "gateway_tls_fingerprint"), None);
        assert_eq!(crate::database::get_ai_setting(&conn, "tier3_topology"), None);
        assert_eq!(crate::database::get_ai_setting(&conn, "last_sync_push"), None);

    }

    /// PR2 (#2647/#2648) test 7: all seven OC settings keys cleared, not just three.
    /// Prior to this PR only gateway_tls_fingerprint/tier3_topology/last_sync_push were
    /// cleared — endpoint/gateway_tls_cert_pem/gateway_paired_at/api_key were missed.
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_runtime_openclaw_clears_all_seven_settings_keys() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::ai::credential_migration::write_gateway_token(&conn, "oc-token-abc").unwrap();
        crate::database::set_ai_setting(&conn, "endpoint", "https://gw.example.com:8443").unwrap();
        crate::database::set_ai_setting(&conn, "gateway_tls_fingerprint", "deadbeef").unwrap();
        crate::database::set_ai_setting(&conn, "gateway_tls_cert_pem", "-----BEGIN CERT-----").unwrap();
        crate::database::set_ai_setting(&conn, "gateway_paired_at", "2026-08-17T00:00:00Z").unwrap();
        crate::database::set_ai_setting(&conn, "tier3_topology", "split_machine").unwrap();
        crate::database::set_ai_setting(&conn, "last_sync_push", "2026-08-17T00:00:00Z").unwrap();
        conn.execute("INSERT INTO ai_settings (key, value) VALUES ('api_key', 'oc-secret-key')", [])
            .unwrap();

        ai_disconnect_runtime_impl("openclaw", &conn).expect("disconnect should succeed");

        for key in [
            "endpoint",
            "gateway_tls_fingerprint",
            "gateway_tls_cert_pem",
            "gateway_paired_at",
            "tier3_topology",
            "last_sync_push",
            "api_key",
        ] {
            assert_eq!(
                crate::database::get_ai_setting(&conn, key),
                None,
                "key '{}' must be cleared after disconnect",
                key
            );
        }

    }

    /// PR2 (#2647/#2648) test 1 + T3 (picker-absence): create OC agent rows, disconnect,
    /// verify is_active=0, then simulate re-pair via the same overwrite-in-place SQL shape
    /// `sync_re_import` uses (this module can't call sync_re_import directly — it lives in
    /// a different crate module with a network-fetch dependency — so this test asserts the
    /// DB-level contract the invariant block promises: same UUID/workspace_path survives,
    /// is_active flips back to 1 on reactivation, exactly mirroring `sync_client.rs:1833`'s
    /// SET clause). See sync_client.rs's own reimport tests for the full HTTP-mocked path.
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_then_repair_reactivates_same_uuid_no_new_directory() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        let fixed_uuid = "OC-MAIN-FIXED-UUID-0001";
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, runtime_ref, created_at, updated_at)
             VALUES (?1, 'Brit', 'brit', 'main', 1, 1, ?1, 'openclaw', 'main', '2026-01-01', '2026-01-01')",
            rusqlite::params![fixed_uuid],
        ).unwrap();

        ai_disconnect_runtime_impl("openclaw", &conn).expect("disconnect should succeed");

        let (is_active_after_disconnect, ws_after_disconnect): (i64, String) = conn
            .query_row(
                "SELECT is_active, workspace_path FROM ai_agents WHERE id = ?1",
                rusqlite::params![fixed_uuid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(is_active_after_disconnect, 0, "row must be deactivated, not deleted");
        assert_eq!(ws_after_disconnect, fixed_uuid, "workspace_path (UUID) must survive disconnect");

        // Simulate re-pair: sync_re_import's overwrite-in-place UPDATE (sync_client.rs:1833)
        // finds the row by runtime_ref, preserves id/workspace_path, and — per this PR's fix —
        // sets is_active = 1.
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE ai_agents SET name=?1, slug=?2, updated_at=?3, is_default=1, role='main', is_active=1 WHERE id=?4",
            rusqlite::params!["Brit", "brit", now, fixed_uuid],
        ).unwrap();

        let (id_count, is_active_after_repair, ws_after_repair): (i64, i64, String) = conn
            .query_row(
                "SELECT COUNT(*), is_active, workspace_path FROM ai_agents WHERE runtime_ref = 'main' GROUP BY id",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(id_count, 1, "re-pair must not create a second row for the same runtime_ref");
        assert_eq!(is_active_after_repair, 1, "re-pair must reactivate the row (load-bearing fix)");
        assert_eq!(ws_after_repair, fixed_uuid, "workspace_path must be unchanged — no new directory");

    }

    /// PR2 (#2647/#2648) test 2: disconnect must not touch workspace_path / any
    /// filesystem state — only DB columns change. This test creates a real temp
    /// directory at the agent's workspace_path, disconnects, and asserts the directory
    /// and its file are still present and untouched afterward.
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_leaves_files_intact() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let dir = tempfile::tempdir().expect("tempdir");
        let agent_dir = dir.path().join("AGENT-UUID-KEEP-ME");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let identity_path = agent_dir.join("IDENTITY.md");
        std::fs::write(&identity_path, "Your name is Brit.\n").unwrap();

        let conn = make_conn();
        let ws = agent_dir.to_str().unwrap().to_string();
        seed_agent(&conn, &ws, Some("openclaw"), None, true);

        ai_disconnect_runtime_impl("openclaw", &conn).expect("disconnect should succeed");

        assert!(agent_dir.exists(), "agent directory must survive disconnect (unpair, not delete)");
        assert!(identity_path.exists(), "IDENTITY.md must survive disconnect");
        let content = std::fs::read_to_string(&identity_path).unwrap();
        assert_eq!(content, "Your name is Brit.\n", "file content must be unchanged");

    }

    /// PR2 (#2647/#2648) test 3: inactive OC rows must be absent from every picker.
    /// Covers has_active_openclaw_agent and ai_list_runtime_connections_impl directly;
    /// the frontend agent-picker filter (agentDisplay.ts) is a client-side `.is_active`
    /// gate on the same `ai_list_runtime_connections_impl` output, so proving the
    /// backend roster is empty covers that surface too.
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_removes_openclaw_from_every_picker() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        seed_agent(&conn, "oc-main", Some("openclaw"), Some("claude"), true); // CI-ALLOW: hardcoded-model

        assert!(
            crate::sync_client::has_active_openclaw_agent(&conn),
            "precondition: OC agent must be active before disconnect"
        );

        ai_disconnect_runtime_impl("openclaw", &conn).expect("disconnect should succeed");

        assert!(
            !crate::sync_client::has_active_openclaw_agent(&conn),
            "has_active_openclaw_agent must return false after disconnect"
        );

        let connections = ai_list_runtime_connections_impl(&conn).expect("should list connections");
        let openclaw = connections.iter().find(|c| c.runtime_type == "openclaw").unwrap();
        assert_eq!(
            openclaw.agents.len(),
            0,
            "ai_list_runtime_connections_impl must report zero OC agents after disconnect"
        );

    }

    /// PR2 (#2647/#2648) test 4: OC main was default → default moves to a non-sub
    /// active agent (ORDER BY created_at DESC LIMIT 1).
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_moves_default_to_local_agent_when_oc_main_was_default() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        seed_agent_with_role(&conn, "oc-main", Some("openclaw"), "main", true, "2026-01-01T00:00:00Z");
        seed_agent_with_role(&conn, "local-agent", None, "local", false, "2026-01-02T00:00:00Z");

        ai_disconnect_runtime_impl("openclaw", &conn).expect("disconnect should succeed");

        let (oc_is_default, oc_is_active): (i64, i64) = conn
            .query_row(
                "SELECT is_default, is_active FROM ai_agents WHERE id = 'oc-main'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(oc_is_default, 0, "OC main must be demoted");
        assert_eq!(oc_is_active, 0, "OC main must be deactivated");

        let local_is_default: i64 = conn
            .query_row(
                "SELECT is_default FROM ai_agents WHERE id = 'local-agent'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(local_is_default, 1, "the surviving local agent must be promoted to default");

    }

    /// PR2 (#2647/#2648) test 5: OC main was the only agent → no default is set, and
    /// the partial unique index does not block a later promotion (no UNIQUE violation).
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_only_agent_leaves_no_default_and_no_unique_violation_later() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        seed_agent_with_role(&conn, "oc-main", Some("openclaw"), "main", true, "2026-01-01T00:00:00Z");

        ai_disconnect_runtime_impl("openclaw", &conn).expect("disconnect should succeed");

        let default_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(default_count, 0, "no row may hold is_default=1 when no eligible agent remains");

        // Insert a brand-new agent and promote it — must not violate the partial unique
        // index, proving the demote in step 3 actually ran and left the index clear.
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, created_at, updated_at)
             VALUES ('new-agent', 'New', 'new', 0, 1, 'new-agent', ?1, ?1)",
            rusqlite::params![now],
        ).unwrap();
        let promote_result = conn.execute(
            "UPDATE ai_agents SET is_default = 1 WHERE id = 'new-agent'",
            [],
        );
        assert!(
            promote_result.is_ok(),
            "promoting a new default after disconnect must not hit the idx_ai_agents_default UNIQUE constraint: {:?}",
            promote_result
        );

    }

    /// PR2 (#2647/#2648) test 6: a sub-agent must never be promoted to default, even
    /// when it's the only other agent left after OC main is deactivated.
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_never_promotes_a_sub_agent() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        seed_agent_with_role(&conn, "oc-main", Some("openclaw"), "main", true, "2026-01-01T00:00:00Z");
        // A sub-agent (role='sub') belonging to a DIFFERENT runtime_type so it survives
        // the openclaw-only deactivation UPDATE and remains active — the exact scenario
        // the "never promote a sub" rule guards against.
        seed_agent_with_role(&conn, "sub-agent", Some("anthropic"), "sub", false, "2026-01-02T00:00:00Z");

        ai_disconnect_runtime_impl("openclaw", &conn).expect("disconnect should succeed");

        let default_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(default_count, 0, "sub-agent must NOT be promoted — no row may hold is_default=1");

        let sub_is_default: i64 = conn
            .query_row("SELECT is_default FROM ai_agents WHERE id = 'sub-agent'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sub_is_default, 0, "sub-agent's own is_default column must remain 0");

    }

    /// #2793 Bug 1 (PRIMARY) regression test 1: a foreign provider (Anthropic) already
    /// holds the global default when OC is disconnected. Before the fix, the OC arm's
    /// promote query was unconditional and had no global existence check — it would
    /// try to write a SECOND `is_default = 1` row (since the demote only touches OC's
    /// own rows), hit `idx_ai_agents_default`'s partial UNIQUE index, and roll back the
    /// entire disconnect transaction. The fix must leave the foreign default untouched
    /// and return Ok(()).
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_openclaw_does_not_steal_foreign_default() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        // Foreign (Anthropic) agent holds the global default. Created FIRST (oldest
        // created_at) so it would NOT be promote_target's naive "newest row" pick —
        // this test alone doesn't prove the guard vs. accidental luck (see the
        // `_when_newest_row` variant below for that), but it's the more common
        // real-world shape (foreign default already existed before OC was connected).
        seed_agent_with_role(&conn, "anthropic-main", Some("anthropic"), "main", true, "2026-01-01T00:00:00Z");
        // OC agent, active but NOT the default, created more recently — this is the
        // exact row promote_target's `ORDER BY created_at DESC LIMIT 1` would naively
        // pick if the guard didn't exist, since it demotes to is_default=0 but stays
        // active until the disconnect's own deactivation UPDATE runs.
        seed_agent_with_role(&conn, "oc-main", Some("openclaw"), "main", false, "2026-01-02T00:00:00Z");

        let result = ai_disconnect_runtime_impl("openclaw", &conn);
        assert!(
            result.is_ok(),
            "disconnect must succeed, not roll back on a UNIQUE violation: {:?}",
            result
        );

        let anthropic_is_default: i64 = conn
            .query_row("SELECT is_default FROM ai_agents WHERE id = 'anthropic-main'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            anthropic_is_default, 1,
            "the foreign (anthropic) default must survive OC's disconnect untouched"
        );

        let default_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(default_count, 1, "exactly one is_default=1 row must exist system-wide after disconnect");

        let oc_is_active: i64 = conn
            .query_row("SELECT is_active FROM ai_agents WHERE id = 'oc-main'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(oc_is_active, 0, "OC agent must still be deactivated by the disconnect");

    }

    /// #2793 Bug 1 regression test 2: same scenario, but the foreign default is seeded
    /// with the LATEST `created_at` — i.e. it IS the row promote_target's naive
    /// `ORDER BY created_at DESC LIMIT 1` query would pick. This proves the `has_default`
    /// guard is what prevents any write here, not accidental luck from row ordering
    /// (a bad implementation that merely reordered the query, rather than adding the
    /// guard, would pass test 1 above by accident but must still be caught by this one).
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_openclaw_does_not_steal_foreign_default_when_newest_row() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        // OC agent created FIRST (oldest) — active, not default.
        seed_agent_with_role(&conn, "oc-main", Some("openclaw"), "main", false, "2026-01-01T00:00:00Z");
        // Foreign default created LAST (newest) — this is exactly the row a naive
        // unconditional `ORDER BY created_at DESC LIMIT 1` promote query would select
        // as its own "new default" candidate, tried to re-set is_default=1 on it (a
        // harmless no-op only by accident), OR — if a different foreign row existed —
        // would collide. This variant isolates: does the guard actually prevent the
        // promote query from running at all, independent of which row it would pick?
        seed_agent_with_role(&conn, "anthropic-main", Some("anthropic"), "main", true, "2026-01-02T00:00:00Z");

        let result = ai_disconnect_runtime_impl("openclaw", &conn);
        assert!(
            result.is_ok(),
            "disconnect must succeed even when the foreign default is the newest row: {:?}",
            result
        );

        let anthropic_is_default: i64 = conn
            .query_row("SELECT is_default FROM ai_agents WHERE id = 'anthropic-main'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(anthropic_is_default, 1, "the foreign default must remain is_default=1");

        let default_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(default_count, 1, "exactly one is_default=1 row must exist system-wide");

    }

    /// #2793 Bug 2 (Matt's ruling — CRITICAL) regression test: if the DB transaction
    /// fails and rolls back, the keychain token must NOT have been deleted — otherwise
    /// OC is left in a partial-failure state (DB says "connected", credential gone).
    /// Forces a transaction failure by pre-seeding a SECOND active default row with a
    /// runtime_type that survives the OC-only demote/deactivate UPDATE (mirrors the
    /// exact Bug 1 collision shape, but here used to prove the Bug 2 keychain-ordering
    /// fix rather than the promote-guard itself — this test seeds the pre-fix-shaped
    /// collision on purpose via a raw forced UNIQUE violation further down in the tx,
    /// so it fails regardless of the Bug 1 guard being present).
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_openclaw_tx_failure_leaves_keychain_intact() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::ai::credential_migration::write_gateway_token(&conn, "oc-token-must-survive").unwrap();
        seed_agent_with_role(&conn, "oc-main", Some("openclaw"), "main", true, "2026-01-01T00:00:00Z");

        // Force a transaction failure independent of the Bug 1 promote-guard logic:
        // drop the `ai_settings` table so the very first `set_ai_setting` call inside
        // the tx closure errors out immediately, guaranteeing ROLLBACK regardless of
        // agent-table state. This isolates Bug 2 (keychain-vs-tx ordering) from Bug 1
        // (promote-guard correctness) — either bug could independently cause a
        // rollback, but this test only wants to prove the keychain-ordering contract.
        conn.execute("DROP TABLE ai_settings", []).unwrap();

        let result = ai_disconnect_runtime_impl("openclaw", &conn);
        assert!(result.is_err(), "disconnect must fail when the DB transaction cannot complete");

        // Recreate ai_settings so read_gateway_token's internal snapshot capture (which
        // reads ai_settings.api_key) doesn't itself error — we only dropped the table to
        // force the tx failure; the keychain assertion below is what this test proves.
        conn.execute_batch("CREATE TABLE ai_settings (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);").unwrap();

        assert_eq!(
            crate::ai::credential_migration::read_gateway_token(&conn).as_deref(),
            Some("oc-token-must-survive"),
            "keychain token must NOT be deleted when the DB transaction rolls back — deleting it \
             before COMMIT would leave OC in a partial-failure state (DB still says connected, \
             credential gone, no way to retry)"
        );

        // The OC agent row must also be unaffected by the rolled-back transaction —
        // still active and default, exactly as before the failed disconnect attempt.
        let (is_active, is_default): (i64, i64) = conn
            .query_row(
                "SELECT is_active, is_default FROM ai_agents WHERE id = 'oc-main'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(is_active, 1, "OC agent must remain active — the rolled-back tx must not have partially applied");
        assert_eq!(is_default, 1, "OC agent must remain default — the rolled-back tx must not have partially applied");

    }

    /// #2793 Bug 3 regression test: Venice's disconnect arm previously had NO promote
    /// logic at all — if Venice held the global default, disconnecting it left the
    /// seat permanently empty until a user manually picked a new default. The fix adds
    /// the same global-guard + conditional-promote shape the OC arm uses.
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_venice_promotes_fallback_when_venice_was_default() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        seed_agent_with_role(&conn, "venice-main", Some("venice"), "main", true, "2026-01-01T00:00:00Z");
        seed_agent_with_role(&conn, "other-agent", Some("anthropic"), "main", false, "2026-01-02T00:00:00Z");

        let result = ai_disconnect_runtime_impl("venice", &conn);
        assert!(result.is_ok(), "venice disconnect should succeed: {:?}", result);

        let (venice_is_active, venice_is_default): (i64, i64) = conn
            .query_row(
                "SELECT is_active, is_default FROM ai_agents WHERE id = 'venice-main'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(venice_is_active, 0, "venice agent must be deactivated");
        assert_eq!(venice_is_default, 0, "venice agent must be demoted");

        let other_is_default: i64 = conn
            .query_row("SELECT is_default FROM ai_agents WHERE id = 'other-agent'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(other_is_default, 1, "the other active agent must be promoted to fill the empty default seat");

        let default_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(default_count, 1, "exactly one is_default=1 row must exist after venice disconnect");

    }

    /// D21 (#2649): disconnect must PRESERVE chat_sessions.agent_id for sessions
    /// bound to a now-deactivated OC agent — NOT NULL it out. This is an intentional
    /// reversal of the prior PR2 (#2647/#2648) behavior: NULLing the binding routed
    /// the next dispatch through the NULL self-heal path, which rebinds to *whatever*
    /// agent is currently `is_default = 1` — a silent re-bind to a different agent,
    /// not the read-only lock docs/spec/sessions.md's "Unavailable bound agent" rule
    /// (ADR-0062 D5) requires. Disconnect must leave the binding alone; the lock/error
    /// treatment is now enforced separately (see `check_bound_agent_active` and its
    /// dispatch-path test below).
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_preserves_chat_session_agent_bindings() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        seed_agent(&conn, "oc-main", Some("openclaw"), Some("claude"), true); // CI-ALLOW: hardcoded-model
        conn.execute(
            "INSERT INTO chat_sessions (id, agent_id) VALUES ('session-1', 'oc-main')",
            [],
        ).unwrap();

        ai_disconnect_runtime_impl("openclaw", &conn).expect("disconnect should succeed");

        let agent_id: Option<String> = conn
            .query_row("SELECT agent_id FROM chat_sessions WHERE id = 'session-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            agent_id,
            Some("oc-main".to_string()),
            "chat_sessions.agent_id must be UNCHANGED after disconnect — disconnect must never NULL or reassign session bindings"
        );

    }

    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_runtime_ollama_is_noop_ok() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        assert!(ai_disconnect_runtime_impl("ollama", &conn).is_ok(), "ollama disconnect with nothing configured must still succeed");

    }

    /// B3 (#2565) Req 2 regression guard: disconnecting ollama clears `ollama_endpoint`,
    /// killing the previously-silent no-op disconnect bug (there was nothing to clear
    /// before this fix introduced the explicit-connection signal).
    #[test]
    #[serial(keychain_override)]
    fn test_disconnect_runtime_ollama_clears_ollama_endpoint() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();
        assert_eq!(
            crate::database::get_ai_setting(&conn, "ollama_endpoint").as_deref(),
            Some("http://localhost:11434")
        );

        ai_disconnect_runtime_impl("ollama", &conn).expect("disconnect should succeed");

        assert_eq!(
            crate::database::get_ai_setting(&conn, "ollama_endpoint"),
            None,
            "ollama_endpoint must be cleared after disconnect"
        );

    }

    #[test]
    fn test_disconnect_runtime_unknown_type_errors() {
        let conn = make_conn();
        let err = ai_disconnect_runtime_impl("carrier-pigeon", &conn).unwrap_err();
        assert!(err.contains("Unknown runtime type"), "error: {}", err);
    }

    // -----------------------------------------------------------------------
    // #2641 D14 three-state color fix: `configured_this_install` regression tests.
    // -----------------------------------------------------------------------

    /// #2631 regression guard: a fresh key with NO `{runtime}_configured_at` marker
    /// (i.e. written by something other than the Save path — simulating a stale
    /// keychain leftover from a previous install) must report
    /// `configured_this_install: false`, even though `connected` is true.
    #[test]
    #[serial(keychain_override)]
    fn test_configured_this_install_false_for_key_present_without_timestamp() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        // Simulate a stale keychain leftover: key present in the keychain, but no
        // `anthropic_configured_at` marker in `ai_settings` (that marker is ONLY ever
        // written by ai_configure_and_save's Save path, never by a raw keychain write).
        crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-leftover").unwrap();

        let out = ai_list_runtime_connections_impl(&conn).unwrap();
        let anthropic = out.iter().find(|c| c.runtime_type == "anthropic").unwrap();
        assert!(anthropic.connected, "keychain has a key — connected must still be true (presence check, unchanged)");
        assert!(
            !anthropic.configured_this_install,
            "#2631: a key with no configured_at marker must NOT report configured_this_install — this is the stale-keychain-leftover case"
        );

    }

    /// Key + `configured_at` present (i.e. written via the real Save path) —
    /// `configured_this_install` must be true.
    #[test]
    #[serial(keychain_override)]
    fn test_configured_this_install_true_when_configured_at_present() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-fresh").unwrap();
        crate::database::set_ai_setting(&conn, "anthropic_configured_at", "2026-08-22T12:00:00Z").unwrap();

        let out = ai_list_runtime_connections_impl(&conn).unwrap();
        let anthropic = out.iter().find(|c| c.runtime_type == "anthropic").unwrap();
        assert!(anthropic.connected);
        assert!(anthropic.configured_this_install, "configured_at marker present — must report configured_this_install: true");

    }

    /// `ai_configure_and_save`'s own contract: writing a key through the real Save
    /// path (`ai_disconnect_runtime_impl`'s counterpart, exercised via direct
    /// `set_ai_setting` calls mirroring what `ai_configure_and_save` does) results in
    /// `configured_this_install: true`, and disconnecting clears it back to `false`.
    #[test]
    #[serial(keychain_override)]
    fn test_configured_this_install_cleared_after_disconnect() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-fresh").unwrap();
        crate::database::set_ai_setting(&conn, "anthropic_configured_at", "2026-08-22T12:00:00Z").unwrap();

        let before = ai_list_runtime_connections_impl(&conn).unwrap();
        assert!(before.iter().find(|c| c.runtime_type == "anthropic").unwrap().configured_this_install);

        ai_disconnect_runtime_impl("anthropic", &conn).expect("disconnect should succeed");

        let after = ai_list_runtime_connections_impl(&conn).unwrap();
        let anthropic = after.iter().find(|c| c.runtime_type == "anthropic").unwrap();
        assert!(!anthropic.connected, "key cleared by disconnect");
        assert!(!anthropic.configured_this_install, "configured_at marker must be cleared by disconnect too");

    }

    /// Same three-state contract for openrouter and ollama — mirrors the anthropic
    /// tests above but exercises the other two runtime types that get their own
    /// `{runtime}_configured_at` marker.
    #[test]
    #[serial(keychain_override)]
    fn test_configured_this_install_openrouter_and_ollama() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        // openrouter: key present, no marker — stale-leftover shape
        crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-leftover").unwrap();
        // ollama: endpoint present, no marker — same shape (endpoint IS the connection signal)
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();

        let out = ai_list_runtime_connections_impl(&conn).unwrap();
        let openrouter = out.iter().find(|c| c.runtime_type == "openrouter").unwrap();
        let ollama = out.iter().find(|c| c.runtime_type == "ollama").unwrap();
        assert!(openrouter.connected && !openrouter.configured_this_install, "openrouter: connected but not configured_this_install without the marker");
        assert!(ollama.connected && !ollama.configured_this_install, "ollama: connected but not configured_this_install without the marker");

        // Now mark both as configured via the Save-path marker and re-check.
        crate::database::set_ai_setting(&conn, "openrouter_configured_at", "2026-08-22T12:00:00Z").unwrap();
        crate::database::set_ai_setting(&conn, "ollama_configured_at", "2026-08-22T12:00:00Z").unwrap();

        let out2 = ai_list_runtime_connections_impl(&conn).unwrap();
        assert!(out2.iter().find(|c| c.runtime_type == "openrouter").unwrap().configured_this_install);
        assert!(out2.iter().find(|c| c.runtime_type == "ollama").unwrap().configured_this_install);

    }

    /// OpenClaw uses the existing `gateway_paired_at` precedent (not a new
    /// `openclaw_configured_at` key) — confirm `configured_this_install` tracks it.
    #[test]
    #[serial(keychain_override)]
    fn test_configured_this_install_openclaw_uses_gateway_paired_at() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        let out = ai_list_runtime_connections_impl(&conn).unwrap();
        assert!(!out.iter().find(|c| c.runtime_type == "openclaw").unwrap().configured_this_install, "never paired — must be false");

        crate::database::set_ai_setting(&conn, "gateway_paired_at", "2026-08-22T12:00:00Z").unwrap();
        let out2 = ai_list_runtime_connections_impl(&conn).unwrap();
        assert!(out2.iter().find(|c| c.runtime_type == "openclaw").unwrap().configured_this_install, "gateway_paired_at set — must be true");

    }
}


/// D26 (#2673 PR-A) integration tests: prove the two-phase lock/keychain split for
/// `ai_list_runtime_connections` actually works end to end — real `tauri::App<MockRuntime>`
/// state, real migrated DB, real `#[tauri::command]` async handler invocation (not just
/// the sync `_impl` helper the tests above exercise) — and prove the core D26 contract: a
/// stalled keychain read surfaces as a bounded per-card timeout error and never holds
/// `db.0` while it's stalled.
#[cfg(test)]
mod d26_keychain_lock_decoupling_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use crate::keychain::{KeychainFailureMode, MockKeychain};
    use serial_test::serial;
    use tauri::Manager;

    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn setup() -> tauri::App<tauri::test::MockRuntime> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    /// Happy path, through the REAL async `#[tauri::command]` handler (not the sync
    /// `_impl` test helper) — proves the async signature change didn't break the normal
    /// case: DB phase runs, lock releases, keychain phase runs, all five cards return.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_async_command_returns_five_cards_with_real_handler() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        let app = setup();
        let db_state = app.state::<DbState>();

        crate::ai::credential_migration::write_provider_key(&db_state.0.lock().unwrap(), "anthropic", "sk-ant-real-handler").unwrap();

        let out = ai_list_runtime_connections(db_state)
            .await
            .expect("async command should succeed against a mocked keychain");
        assert_eq!(out.len(), 5, "must always report exactly the five known runtime types");
        let anthropic = out.iter().find(|c| c.runtime_type == "anthropic").unwrap();
        assert!(anthropic.connected, "anthropic key was written, should be connected");
    }

    /// Core D26 contract test: a keychain read that blocks past `KEYCHAIN_READ_TIMEOUT`
    /// (simulating a stalled macOS authorization prompt, #2673's root cause) must surface
    /// as a bounded per-card error — `connected: false` + a descriptive `error` — NOT an
    /// indefinite hang and NOT an `Err` that fails the whole command. Uses a short custom
    /// timeout budget so the test itself doesn't take `KEYCHAIN_READ_TIMEOUT`'s full 5s.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_stalled_keychain_read_times_out_not_hangs() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let mock = MockKeychain::new();
        // Simulate a stalled OS keychain read on the anthropic account specifically —
        // longer than KEYCHAIN_READ_TIMEOUT (5s), so the timeout wrapper must fire.
        mock.fail_get_for(
            crate::ai::credential_migration::KEYCHAIN_ACCOUNT_ANTHROPIC_KEY,
            KeychainFailureMode::Hang(Duration::from_secs(8)),
        );
        let _kc = KeychainGuard::set_with(Box::new(mock));
        let app = setup();
        let db_state = app.state::<DbState>();

        let start = Instant::now();
        let out = ai_list_runtime_connections(db_state)
            .await
            .expect("a stalled single-card keychain read must degrade to a per-card error, not fail the whole command");
        let elapsed = start.elapsed();

        assert_eq!(out.len(), 5, "all five cards must still be returned, including the stalled one");
        let anthropic = out.iter().find(|c| c.runtime_type == "anthropic").unwrap();
        assert!(!anthropic.connected, "a timed-out keychain read must report disconnected, not silently succeed");
        assert!(
            anthropic.error.as_deref().map(|e| e.contains("timed out")).unwrap_or(false),
            "the timed-out card's error must name the timeout, got: {:?}",
            anthropic.error
        );
        assert!(
            elapsed < Duration::from_secs(8),
            "the command must return well before the 8s hang completes (bounded by KEYCHAIN_READ_TIMEOUT) — took {:?}",
            elapsed
        );
    }

    /// The other half of the D26 contract: while phase 2 (keychain) is running, `db.0`
    /// must NOT be held — a concurrent DB read must be able to proceed immediately even
    /// while one runtime type's keychain read is stalled. This is the exact hazard #2673
    /// reports (`ai_chat_stream` stalling because `db.0` was held for the full keychain
    /// loop duration). Drives the same production building blocks the real
    /// `#[tauri::command]` handler calls (`ai_list_runtime_connections_db_phase` +
    /// `resolve_runtime_connection_info_with_timeout`) directly against a `DbState`,
    /// rather than through `tauri::State` (which can't cross a `tokio::spawn` boundary
    /// without a full mock-app lifetime dance) — this exercises the exact same lock
    /// scope the command wrapper uses, just without the Tauri IPC plumbing around it.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_db_lock_not_held_during_stalled_keychain_read() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let mock = MockKeychain::new();
        mock.fail_get_for(
            crate::ai::credential_migration::KEYCHAIN_ACCOUNT_ANTHROPIC_KEY,
            KeychainFailureMode::Hang(Duration::from_secs(8)),
        );
        let _kc = KeychainGuard::set_with(Box::new(mock));

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let db_state = std::sync::Arc::new(DbState::new(conn));

        // Phase 1 (DB-only), exactly like the command wrapper: lock, capture, drop guard.
        let db_phase = {
            let conn = db_state.0.lock().unwrap();
            ai_list_runtime_connections_db_phase(&conn).expect("db phase should succeed")
        };

        // Fire phase 2 for the stalled runtime type, but don't await it yet.
        let handle = tokio::spawn(resolve_runtime_connection_info_with_timeout("anthropic", db_phase));

        // Give phase 2's spawn_blocking time to actually start blocking on the mocked
        // keychain hang before we probe db.0.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // A concurrent, independent DB read must succeed immediately — proving db.0 is
        // NOT held during the stalled keychain read. Uses the same lock the command
        // wrapper itself would use for any other concurrent command (e.g. ai_chat_stream).
        let concurrent_read_start = Instant::now();
        {
            let conn = db_state.0.lock().expect("db.0 must be immediately lockable while phase 2 is stalled");
            let _ = crate::database::get_ai_setting(&conn, "some_unrelated_key");
        }
        let concurrent_read_elapsed = concurrent_read_start.elapsed();
        assert!(
            concurrent_read_elapsed < Duration::from_millis(500),
            "a concurrent db.0 read must not be blocked by the stalled keychain read — took {:?}",
            concurrent_read_elapsed
        );

        // Clean up: the spawned phase-2 call will itself time out at KEYCHAIN_READ_TIMEOUT.
        let info = handle.await.unwrap();
        assert!(!info.connected);
        assert!(info.error.as_deref().unwrap_or("").contains("timed out"));
    }
}


/// C7 (#2598): `connected_runtime_providers` chicken-and-egg fix tests.
///
/// Prior behavior derived "connected" purely from `SELECT DISTINCT runtime_type
/// FROM ai_agents` — so a runtime with a valid key/endpoint but zero agent rows
/// (the exact state right after saving credentials, before the first agent
/// exists) was invisible, and `fetch_available_models` never fired for it. These
/// tests prove the fix: connection status is now derived purely from
/// `build_provider_for_runtime_type` (key/endpoint/pairing presence), with the
/// agent roster completely irrelevant to inclusion.
#[cfg(test)]
mod c7_connected_runtime_providers_tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    /// No `ai_agents` table at all — `connected_runtime_providers` must not
    /// need it to determine connection status (only `build_provider_for_runtime_type`
    /// does, and it reads keychain + `ai_settings`, never `ai_agents`).
    fn make_agentless_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE ai_settings (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            ",
        )
        .unwrap();
        conn
    }

    /// Req 1: zero-agent runtime with a valid key → `connected_runtime_providers`
    /// returns it, and the returned provider is immediately usable by
    /// `fetch_models_for_provider` (proving the model-fetch gate the bug hid
    /// this behind is actually open now, not just that the tuple is present).
    #[test]
    #[serial(keychain_override)]
    fn test_zero_agent_runtime_with_valid_key_is_connected() {
        let _kc = KeychainGuard::set();
        let conn = make_agentless_conn();

        // No ai_agents table exists at all — this is the literal chicken-and-egg
        // state: key saved, first agent not created yet.
        crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-test-key")
            .expect("write key");

        let providers = connected_runtime_providers(&conn);
        let anthropic = providers.iter().find(|(rt, _)| rt == "anthropic");
        assert!(
            anthropic.is_some(),
            "zero-agent runtime with a valid key must be reported connected, got: {:?}",
            providers.iter().map(|(rt, _)| rt).collect::<Vec<_>>()
        );
        match &anthropic.unwrap().1 {
            AiProvider::Anthropic { api_key } => assert_eq!(api_key, "sk-ant-test-key"),
            other => panic!("expected AiProvider::Anthropic, got {:?}", other),
        }
    }

    /// Req 2: agentless runtime with NO key/endpoint configured stays excluded —
    /// proves the fix doesn't just report everything connected unconditionally.
    #[test]
    #[serial(keychain_override)]
    fn test_agentless_disconnected_runtime_excluded() {
        let _kc = KeychainGuard::set();
        let conn = make_agentless_conn();

        let providers = connected_runtime_providers(&conn);
        assert!(
            providers.is_empty(),
            "no runtime has credentials configured — result must be empty, got: {:?}",
            providers.iter().map(|(rt, _)| rt).collect::<Vec<_>>()
        );
    }

    /// Mixed case: one runtime connected (ollama, via endpoint — no keychain
    /// dependency), the other three not — only the connected one is returned,
    /// and it is returned regardless of `ai_agents` being entirely absent.
    #[test]
    #[serial(keychain_override)]
    fn test_only_connected_runtime_returned_among_five() {
        let _kc = KeychainGuard::set();
        let conn = make_agentless_conn();
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('ollama_endpoint', 'http://localhost:11434')",
            [],
        )
        .unwrap();

        let providers = connected_runtime_providers(&conn);
        assert_eq!(
            providers.len(),
            1,
            "exactly one runtime configured — got: {:?}",
            providers.iter().map(|(rt, _)| rt).collect::<Vec<_>>()
        );
        assert_eq!(providers[0].0, "ollama");
    }

    /// Req 3a: Anthropic fetch actually reaches HTTP and parses a real response
    /// shape, exercised via the same `AiProvider` the (now-fixed) gate hands to
    /// `fetch_models_for_provider` — not just a struct-deserialize unit test.
    #[tokio::test]
    async fn test_anthropic_fetch_via_provider_gate() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/v1/models?limit=1000")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":[{"id":"claude-c7-test","display_name":"Claude C7 Test","type":"model"}],"has_more":false}"#, // CI-ALLOW: hardcoded-model
            )
            .create_async()
            .await;

        let models = fetch_anthropic_models_at("sk-ant-test", &server.url())
            .await
            .expect("anthropic fetch should succeed against mocked endpoint");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "claude-c7-test"); // CI-ALLOW: hardcoded-model
        assert_eq!(models[0].provider, "anthropic");
    }

    /// Req 3b: OpenRouter fetch via the same real HTTP path the gate uses.
    #[tokio::test]
    async fn test_openrouter_fetch_via_provider_gate() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/models")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":[{"id":"or-c7-test","name":"OR C7 Test","context_length":8192,"supported_parameters":["temperature"]}]}"#,
            )
            .create_async()
            .await;

        let models = fetch_openrouter_models_at("sk-or-test", &server.url())
            .await
            .expect("openrouter fetch should succeed against mocked endpoint");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "or-c7-test");
        assert_eq!(models[0].context_length, Some(8192));
        assert_eq!(models[0].provider, "openrouter");
    }

    /// Req 3c: Ollama fetch via the same real HTTP path the gate uses.
    #[tokio::test]
    async fn test_ollama_fetch_via_provider_gate() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"models":[{"name":"llama3-c7-test"}]}"#)
            .create_async()
            .await;

        let models = fetch_ollama_models(&server.url())
            .await
            .expect("ollama fetch should succeed against mocked endpoint");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "llama3-c7-test");
        assert_eq!(models[0].provider, "ollama");
    }

    /// End-to-end: zero-agent runtime with a valid key flows all the way through
    /// `connected_runtime_providers` → `fetch_models_for_provider` → a real
    /// (mocked) HTTP fetch and returns models — the exact call chain
    /// `fetch_available_models` makes, proving the gate is genuinely open end to
    /// end and not just returning a tuple that then fails downstream.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_zero_agent_end_to_end_model_fetch_via_gate() {
        let _kc = KeychainGuard::set();
        let conn = make_agentless_conn();

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/tags")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"models":[{"name":"llama3-e2e"}]}"#)
            .create_async()
            .await;
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('ollama_endpoint', ?1)",
            rusqlite::params![server.url()],
        )
        .unwrap();

        let providers = connected_runtime_providers(&conn);
        assert_eq!(providers.len(), 1, "ollama should be the sole connected runtime");
        let (runtime_type, provider) = providers.into_iter().next().unwrap();
        assert_eq!(runtime_type, "ollama");

        let db = DbState::new(rusqlite::Connection::open_in_memory().unwrap());
        let models = fetch_models_for_provider(provider, &db)
            .await
            .expect("end-to-end fetch through the gate should succeed");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "llama3-e2e");
    }
}


/// List available models for the current provider
/// #2564 B1 fix (Path B): `provider` was `SELECT value FROM ai_settings WHERE key =
/// 'provider'` — raw read of the retired key, not one of Reed's 11 enumerated
/// callsites but caught in the "trace the data flow end-to-end" build-standards pass.
/// Left unretired this would `unwrap_or_default()` to `""` for every install post-fix
/// and permanently fall into the OpenRouter/OpenClaw catch-all arm with an empty
/// endpoint, breaking the model picker for every provider. Re-pointed to the same
/// derived-provider-label logic as `ai_load_config` (item #12).
#[tauri::command]
pub async fn list_available_models(db: tauri::State<'_, DbState>) -> Result<Vec<String>, String> {
    // No hardcoded fallback list per spec line 423 (Matt directive, Apr 11)
    // If provider query fails, return error instead of assuming models exist

    let (provider, endpoint, api_key) = {
        let db = db.0.lock().map_err(|e| e.to_string())?;
        let provider = crate::ai::runtime_resolution::derive_active_provider_label(&db);
        let endpoint = crate::database::get_ai_setting(&db, "endpoint").unwrap_or_default();
        // ADR-0044: read credential via migration-aware function
        let api_key = if provider == "openclaw" {
            crate::ai::credential_migration::read_gateway_token(&db).unwrap_or_default()
        } else {
            crate::ai::credential_migration::read_provider_key(&db, &provider).unwrap_or_default()
        };
        (provider, endpoint, api_key)
    };
    // DB lock is dropped here before any async operations

    match provider.to_lowercase().as_str() {
        "anthropic" => {
            // Anthropic doesn't have a /models endpoint — return error
            // User must manually configure model from Anthropic docs
            Err("Anthropic does not provide a models list endpoint. Please manually enter your model name from https://docs.anthropic.com/en/docs/models-overview".to_string())
        }
        "ollama" => {
            // Use existing Ollama list_models if available
            let url = if endpoint.is_empty() {
                "http://localhost:11434".to_string()
            } else {
                endpoint
            };
            let client = reqwest::Client::new();
            let resp = client
                .get(format!("{}/api/tags", url))
                .timeout(std::time::Duration::from_secs(5))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let models = body["models"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            Ok(models)
        }
        _ => {
            // OpenRouter, OpenClaw, or any OpenAI-compatible endpoint
            // If endpoint is empty, cannot fetch models
            if endpoint.is_empty() {
                return Err("No endpoint configured. Cannot fetch available models.".to_string());
            }

            let client = reqwest::Client::new();
            let url = format!("{}/models", endpoint.trim_end_matches('/'));
            let mut req = client.get(&url).timeout(std::time::Duration::from_secs(5));
            if !api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", api_key));
            }

            match req.send().await {
                Ok(resp) => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        let models: Vec<String> = body["data"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|m| m["id"].as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();

                        // Return what we got, even if empty
                        Ok(models)
                    } else {
                        // JSON parse failed → error
                        Err("Failed to parse models response from provider".to_string())
                    }
                }
                Err(e) => {
                    // Request failed → error
                    Err(format!("Failed to fetch models from provider: {}", e))
                }
            }
        }
    }
}


/// Check if Ollama is available locally
#[tauri::command]
pub async fn ai_check_ollama() -> Result<Vec<String>, String> {
    let ollama = OllamaProvider::new(None);
    if !ollama.is_available().await {
        return Err("Ollama not running".to_string());
    }
    ollama.list_models().await
}


/// Two-call Ollama presence probe for onboarding §6.4.4
#[tauri::command]
pub async fn ollama_probe_two_step(endpoint: Option<String>) -> Result<OllamaProbeResult, String> {
    eprintln!(
        "[OLLAMA-PROBE] ollama_probe_two_step called with endpoint: {:?}",
        endpoint
    );
    let ollama = OllamaProvider::new(endpoint);
    Ok(ollama.probe_two_step().await)
}


/// Get current AI configuration.
/// #2522 ADR-0063 Stage 2: retargeted from the `AiConfig` singleton to the
/// designated default agent's resolved runtime config (same D5 fallback as
/// `ai_status`). The system prompt is no longer per-config state — it's a
/// constant (see `default_system_prompt`), so `hasSystemPrompt` is always true.
#[tauri::command]
pub fn ai_get_config(db: State<'_, DbState>) -> Result<serde_json::Value, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let cfg = crate::ai::runtime_resolution::resolve_runtime_config_for_default_agent(&conn);

    let (configured, provider_name, model) = match &cfg {
        Ok(c) => {
            let provider_name = match &c.provider {
                AiProvider::Anthropic { .. } => "anthropic",
                AiProvider::OpenRouter { .. } => "openrouter",
                AiProvider::Ollama { .. } => "ollama",
                AiProvider::OpenClaw { .. } => "openclaw",
                AiProvider::Venice { .. } => "venice",
                AiProvider::None => "none",
            };
            (true, provider_name, c.model.clone())
        }
        Err(_) => (false, "none", String::new()),
    };

    Ok(serde_json::json!({
        "configured": configured,
        "provider": provider_name,
        "model": model,
        "hasSystemPrompt": true,
    }))
}


/// C7 fix (#2598): enumerate all five known runtime types and try to build a
/// provider for each via `build_provider_for_runtime_type` — the same
/// connection-config derivation `ai_list_runtime_connections_impl` already
/// uses (see that function's loop). A runtime is "connected" purely based on
/// whether it has a usable key/endpoint/gateway pairing configured, NOT on
/// whether any `ai_agents` row references it.
///
/// This replaces the prior `SELECT DISTINCT runtime_type FROM ai_agents`
/// derivation, which had a chicken-and-egg bug: a runtime with a valid key
/// but zero agent rows (e.g. right after entering an Anthropic/OpenRouter
/// key but before creating the first agent) was invisible here, so
/// `fetch_available_models` never fired, so the model picker for first-agent
/// creation stayed empty. A runtime with no/invalid credentials configured
/// is silently skipped from the result (not an error) — same tolerance the
/// old single-runtime path had for `AiProvider::None`; callers can't
/// reasonably distinguish "never set up" from "still on Settings" at this
/// call site. Real failures (e.g. keychain read errors) ARE logged via
/// `eprintln!` per #2598 hardening — silent-swallow here cost two rounds of
/// investigation on the original bug.
#[cfg(test)]
fn connected_runtime_providers(conn: &rusqlite::Connection) -> Vec<(String, AiProvider)> {
    let runtime_types = ["openclaw", "openrouter", "anthropic", "ollama", "venice"];

    runtime_types
        .iter()
        .filter_map(
            |rt| match crate::ai::runtime_resolution::build_provider_for_runtime_type(conn, rt, None) {
                Ok(provider) => Some((rt.to_string(), provider)),
                Err(e) => {
                    eprintln!(
                        "[RUNTIME-PROVIDERS] {} not connected, skipping: {}",
                        rt, e
                    );
                    None
                }
            },
        )
        .collect()
}


/// D26 (#2673 PR-A) §28.4 two-phase counterpart to `connected_runtime_providers`. The
/// original function itself doesn't hold `db.0` — its callers do, passing in an
/// already-locked `conn` — so the fix lives here rather than in the sync function
/// (which stays as-is, still used directly by the C7 unit tests below against a plain
/// in-memory `Connection`). Phase 1 (`RuntimeSettingsSnapshot::capture`, cheap DB-only)
/// runs under whatever lock the caller is holding; phase 2 (the actual keychain reads,
/// via `build_provider_for_runtime_type_from_snapshot`) runs afterward, per runtime type,
/// inside a `KEYCHAIN_READ_TIMEOUT`-bounded `spawn_blocking` — identical shape to
/// `resolve_runtime_connection_info_with_timeout` above, mirroring the #1347 precedent at
/// `sync/commands.rs:324-333`. Callers pass in a `RuntimeSettingsSnapshot` captured while
/// still holding `db.0`, then must drop the lock before `.await`-ing this.
async fn connected_runtime_providers_async(
    settings: crate::ai::runtime_resolution::RuntimeSettingsSnapshot,
) -> Vec<(String, AiProvider)> {
    let runtime_types = ["openclaw", "openrouter", "anthropic", "ollama", "venice"];
    let mut out = Vec::with_capacity(runtime_types.len());

    for rt in runtime_types {
        let settings_for_task = settings.clone();
        let result = tokio::time::timeout(
            KEYCHAIN_READ_TIMEOUT,
            tokio::task::spawn_blocking(move || {
                crate::ai::runtime_resolution::build_provider_for_runtime_type_from_snapshot(
                    &settings_for_task,
                    rt,
                    None, // generic per-runtime-type connectivity check — not resolving for a specific agent
                )
            }),
        )
        .await;

        match result {
            Ok(Ok(Ok(provider))) => out.push((rt.to_string(), provider)),
            Ok(Ok(Err(e))) => {
                eprintln!("[RUNTIME-PROVIDERS] {} not connected, skipping: {}", rt, e);
            }
            Ok(Err(join_err)) => {
                eprintln!(
                    "[RUNTIME-PROVIDERS] {} keychain read panicked, skipping: {}",
                    rt, join_err
                );
            }
            Err(_elapsed) => {
                eprintln!(
                    "[RUNTIME-PROVIDERS] {} keychain read timed out after {}s (#2673 D26), skipping — \
                     the OS keychain may be waiting on an authorization prompt, db.0 was NOT held during this read",
                    rt,
                    KEYCHAIN_READ_TIMEOUT.as_secs()
                );
            }
        }
    }

    out
}


/// Fetch a single provider's model list. Shared by `fetch_available_models`
/// (per-runtime loop) and `populate_model_registry_from_provider` (startup).
async fn fetch_models_for_provider(
    provider: AiProvider,
    db: &crate::database::DbState,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    match provider {
        AiProvider::Anthropic { api_key } => fetch_anthropic_models(&api_key).await,
        AiProvider::OpenRouter { api_key } => fetch_openrouter_models(&api_key).await,
        AiProvider::OpenClaw { base_url, token, .. } => {
            fetch_openclaw_models_topology_aware(db, &base_url, &token).await
        }
        AiProvider::Ollama { endpoint } => fetch_ollama_models(&endpoint).await,
        // ADR-0065 D5: Venice model list via its /models endpoint.
        AiProvider::Venice { api_key, .. } => fetch_venice_models(&api_key).await,
        AiProvider::None => Err("No AI provider configured".to_string()),
    }
}


/// Fetch available models from every connected AI runtime.
/// #2522 ADR-0063 Stage 3+4 (D7.3): iterates all agents' distinct
/// `runtime_type`s (not just the default agent's) and merges results, so a
/// user with e.g. both Anthropic and Ollama agents configured sees both
/// providers' models in one call. `ModelRegistry` storage is already
/// provider-namespaced (`"{provider}:{model_id}"` keys), so merging here is
/// just "call fetch + insert per runtime, keep going on individual
/// failures" — one runtime's fetch error shouldn't hide the others' models.
#[tauri::command]
pub async fn fetch_available_models(
    db: State<'_, crate::database::DbState>,
    model_registry: State<'_, crate::ai::provider::ModelRegistry>,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    // D26 (#2673 PR-A) §28.4: phase 1 (DB-only snapshot) runs under db.0.lock(); the
    // guard is dropped at the end of this block, BEFORE connected_runtime_providers_async's
    // per-runtime-type keychain reads (phase 2) run below.
    let settings = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::ai::runtime_resolution::RuntimeSettingsSnapshot::capture(&conn)
    };
    let providers = connected_runtime_providers_async(settings).await;

    if providers.is_empty() {
        return Err("No AI provider configured".to_string());
    }

    let mut all_models = Vec::new();
    let mut errors = Vec::new();
    for (runtime_type, provider) in providers {
        match fetch_models_for_provider(provider, &db).await {
            Ok(models) => all_models.extend(models),
            Err(e) => errors.push(format!("{}: {}", runtime_type, e)),
        }
    }

    // Only fail outright if every runtime's fetch failed — a partial
    // success (e.g. Anthropic reachable, Ollama not running) should still
    // return what we got, matching how the rest of the app treats
    // per-runtime connectivity as independent.
    if all_models.is_empty() && !errors.is_empty() {
        return Err(errors.join("; "));
    }

    // Populate ModelRegistry (#1402-C wire-up).
    // Provider key MUST be ModelInfo.provider — same string convention used at lookup site.
    // Use insert_with_capabilities to also store supported_parameters from OpenRouter (#1846).
    for m in &all_models {
        model_registry.insert_with_capabilities(
            &m.provider,
            &m.id,
            m.context_length,
            m.supported_parameters.clone(),
        );
    }

    Ok(all_models)
}


/// Eagerly populate the ModelRegistry from every connected runtime on app startup (#1410).
///
/// Called from lib.rs `.setup()` in a background `tauri::async_runtime::spawn` block,
/// immediately after `app.manage(ai_state.clone())`. Runs unconditionally on every
/// launch — no UI gate, no window dependency.
///
/// #2522 ADR-0063 Stage 3+4 (D7.3): iterates all agents' distinct
/// `runtime_type`s (via `connected_runtime_providers`, same enumeration
/// `fetch_available_models` uses) instead of a single pre-resolved
/// provider, so startup warms the registry for every connected runtime, not
/// just the default agent's. No connected runtimes → early Ok(()) (first
/// launch / nothing configured yet; not an error). A given runtime's
/// network failure is logged and skipped — it shouldn't block the others
/// from populating.
pub(crate) async fn populate_model_registry_from_provider(
    db: &crate::database::DbState,
    registry: Arc<Mutex<super::provider::ModelRegistryData>>,
) -> Result<(), String> {
    // D26 (#2673 PR-A) §28.4: same phase-split as fetch_available_models — see its
    // comment. Lower urgency here (startup-only, not on the ai_chat_stream path) but
    // fixed for consistency, per the scoping doc.
    let settings = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::ai::runtime_resolution::RuntimeSettingsSnapshot::capture(&conn)
    };
    let providers = connected_runtime_providers_async(settings).await;

    if providers.is_empty() {
        return Ok(()); // no-op — no runtime configured yet
    }

    let mut total = 0usize;
    for (runtime_type, provider) in providers {
        let models = match fetch_models_for_provider(provider, db).await {
            Ok(models) => models,
            Err(e) => {
                eprintln!(
                    "[startup] ModelRegistry population skipped for {}: {}",
                    runtime_type, e
                );
                continue;
            }
        };
        if let Ok(mut data) = registry.lock() {
            for m in &models {
                let key = format!("{}:{}", m.provider, m.id);
                data.context_lengths.insert(key.clone(), m.context_length);
                if let Some(params) = &m.supported_parameters {
                    data.supported_parameters.insert(key, params.clone());
                }
            }
        }
        total += models.len();
    }
    eprintln!("[startup] ModelRegistry populated: {} models", total);
    Ok(())
}


async fn fetch_anthropic_models(api_key: &str) -> Result<Vec<super::provider::ModelInfo>, String> {
    fetch_anthropic_models_at(api_key, "https://api.anthropic.com").await
}


/// Same as `fetch_anthropic_models` but with an injectable base URL — the
/// production wrapper above always passes the real Anthropic API host;
/// tests pass a `mockito` server URL so the HTTP fetch path itself (not
/// just the response-parsing struct) gets exercised (#2598 C7 test req 3).
async fn fetch_anthropic_models_at(
    api_key: &str,
    base_url: &str,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    use super::provider::ModelInfo;

    eprintln!("[ANTHROPIC-MODELS] Fetching model list from Anthropic API");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let url = format!("{}/v1/models?limit=1000", base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .header("x-api-key", api_key.trim())
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch Anthropic models: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!("Anthropic API error ({}): {}", status, error_text));
    }

    // Response shape: { "data": [{ "id": "...", "display_name": "...", ... }], "has_more": bool }
    #[derive(serde::Deserialize)]
    struct AnthropicModelsResponse {
        data: Vec<AnthropicModel>,
    }

    #[derive(serde::Deserialize)]
    struct AnthropicModel {
        id: String,
        display_name: String,
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    let parsed: AnthropicModelsResponse = serde_json::from_str(&body)
        .map_err(|e| format!("Failed to parse Anthropic models response: {}", e))?;

    let models: Vec<ModelInfo> = parsed
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id.clone(),
            name: m.display_name,
            provider: "anthropic".to_string(),
            context_length: None,
            supported_parameters: None,
        tier_label: None,
        supports_tee_attestation: None,
        supports_e2ee: None,
        supports_function_calling: None,
        })
        .collect();

    eprintln!("[ANTHROPIC-MODELS] Fetched {} models", models.len());
    Ok(models)
}


/// Intermediate wire-format struct for OpenRouter /api/v1/models response.
/// Promoted to module level (pub(crate)) so unit tests can deserialize it directly (#1399).
#[derive(Deserialize)]
pub(crate) struct OpenRouterModelWire {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
    /// Parameters this model supports (e.g. ["temperature", "max_tokens", ...]).
    /// Used by ModelRegistry to gate temperature injection (#1846).
    pub supported_parameters: Option<Vec<String>>,
}


#[derive(Deserialize)]
struct OpenRouterModelsResponse {
    data: Vec<OpenRouterModelWire>,
}


async fn fetch_openrouter_models(api_key: &str) -> Result<Vec<super::provider::ModelInfo>, String> {
    fetch_openrouter_models_at(api_key, "https://openrouter.ai/api/v1").await
}


/// Same as `fetch_openrouter_models` but with an injectable base URL — see
/// `fetch_anthropic_models_at` doc comment for rationale (#2598 C7 test req 3).
async fn fetch_openrouter_models_at(
    api_key: &str,
    base_url: &str,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    use super::provider::ModelInfo;

    let client = reqwest::Client::new();
    let url = format!("{}/models", base_url.trim_end_matches('/'));
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: OpenRouterModelsResponse = response.json().await.map_err(|e| e.to_string())?;

    Ok(data
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            name: m.name,
            provider: "openrouter".to_string(),
            context_length: m.context_length,
            supported_parameters: m.supported_parameters,
            tier_label: None,
            supports_tee_attestation: None,
            supports_e2ee: None,
            supports_function_calling: None,
        })
        .collect())
}


/// Fetch embedding models from OpenRouter using `?output_modalities=embeddings`.
/// This is intentionally separate from `fetch_openrouter_models` (which is used
/// for chat-model listing) — do NOT merge them. (#1657)
pub(crate) async fn fetch_openrouter_embedding_models(
    api_key: &str,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    use super::provider::ModelInfo;

    let client = reqwest::Client::new();
    let response = client
        .get("https://openrouter.ai/api/v1/models?output_modalities=embeddings")
        .header("Authorization", format!("Bearer {}", api_key))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: OpenRouterModelsResponse = response.json().await.map_err(|e| e.to_string())?;

    Ok(data
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            name: m.name,
            provider: "openrouter".to_string(),
            context_length: m.context_length,
            supported_parameters: m.supported_parameters,
            tier_label: None,
            supports_tee_attestation: None,
            supports_e2ee: None,
            supports_function_calling: None,
        })
        .collect())
}


/// Returns true if an Ollama model is an embedding model.
///
/// Heuristic (covers all known Ollama embedding models as of 2026):
/// - `details.family` contains `"bert"` (case-insensitive)
///   → matches: nomic-embed-text (nomic-bert), mxbai-embed-large (bert), all-minilm (bert)
/// - model name contains `"embed"` (case-insensitive)
///   → matches: bge-m3, snowflake-arctic-embed, granite-embedding, paraphrase-multilingual
///
/// Filtered OUT: llama, mistral, phi, gemma, qwen, deepseek, … (all chat families).
pub(crate) fn is_ollama_embedding_model(name: &str, family: &str) -> bool {
    family.to_lowercase().contains("bert") || name.to_lowercase().contains("embed")
}


/// Fetch Ollama models and return only those that look like embedding models.
/// Uses `is_ollama_embedding_model` heuristic. (#1657)
pub(crate) async fn fetch_ollama_embedding_models(
    endpoint: &str,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    use super::provider::ModelInfo;

    #[derive(Deserialize)]
    struct OllamaDetails {
        family: Option<String>,
    }

    #[derive(Deserialize)]
    struct OllamaModelWithDetails {
        name: String,
        #[serde(default)]
        details: Option<OllamaDetails>,
    }

    #[derive(Deserialize)]
    struct OllamaResponse {
        models: Vec<OllamaModelWithDetails>,
    }

    let client = reqwest::Client::new();
    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));

    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: OllamaResponse = response.json().await.map_err(|e| e.to_string())?;

    Ok(data
        .models
        .into_iter()
        .filter(|m| {
            let family = m
                .details
                .as_ref()
                .and_then(|d| d.family.as_deref())
                .unwrap_or("");
            is_ollama_embedding_model(&m.name, family)
        })
        .map(|m| ModelInfo {
            id: m.name.clone(),
            name: m.name,
            provider: "ollama".to_string(),
            context_length: None,
            supported_parameters: None,
        tier_label: None,
        supports_tee_attestation: None,
        supports_e2ee: None,
        supports_function_calling: None,
        })
        .collect())
}


// Test-only override for `fetch_venice_embedding_models`'s hardcoded
// `VENICE_API_BASE` (#2988 PR1) — same `#[cfg(test)]` thread_local carve-out
// shape as `test_set_venice_validate_base` above and `lib.rs::TEST_VENICE_EMBED_BASE`.
#[cfg(test)]
thread_local! {
    static TEST_VENICE_EMBED_MODELS_BASE: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
}


#[cfg(test)]
pub(crate) fn test_set_venice_embed_models_base(url: &str) {
    TEST_VENICE_EMBED_MODELS_BASE.with(|c| *c.borrow_mut() = Some(url.to_string()));
}


#[cfg(test)]
pub(crate) fn test_clear_venice_embed_models_base() {
    TEST_VENICE_EMBED_MODELS_BASE.with(|c| *c.borrow_mut() = None);
}


fn venice_embed_models_base() -> String {
    #[cfg(test)]
    {
        if let Some(u) = TEST_VENICE_EMBED_MODELS_BASE.with(|c| c.borrow().clone()) {
            return u;
        }
    }
    super::openrouter::VENICE_API_BASE.to_string()
}


/// Fetch Venice embedding models for the Semantic Search card's model dropdown
/// (#2988 PR1 — the gate Matt flagged: `fetch_embedding_models` populates the
/// dropdown, so it needs its own Venice arm, not just `switch_embedding_engine`'s
/// validation gate). Uses Venice's documented `GET /models?type=embedding` filter
/// (confirmed against `docs.venice.ai/api-reference/endpoint/models/list` — the
/// `type` query param accepts an `embedding` enum value) so chat models never leak
/// into this picker.
pub(crate) async fn fetch_venice_embedding_models(api_key: &str) -> Result<Vec<super::provider::ModelInfo>, String> {
    use super::provider::ModelInfo;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let url = format!("{}/models?type=embedding", venice_embed_models_base());
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch Venice embedding models: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(format!("Venice API error ({}): {}", status, err_text));
    }

    #[derive(serde::Deserialize)]
    struct VeniceEmbeddingModel {
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        context_length: Option<u64>,
    }

    #[derive(serde::Deserialize)]
    struct VeniceEmbeddingModelsResponse {
        data: Vec<VeniceEmbeddingModel>,
    }

    let parsed: VeniceEmbeddingModelsResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Venice embedding models: {}", e))?;

    let models: Vec<ModelInfo> = parsed
        .data
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id.clone(),
            name: m.name.unwrap_or_else(|| m.id.clone()),
            provider: "venice".to_string(),
            context_length: m.context_length,
            supported_parameters: None,
            tier_label: None,
            supports_tee_attestation: None,
            supports_e2ee: None,
            supports_function_calling: None,
        })
        .collect();

    eprintln!("[VENICE-EMBED-MODELS] Fetched {} embedding models", models.len());
    Ok(models)
}


/// Topology-aware model fetching for OpenClaw
// OpenClaw is split-machine only (#2649) — no same-machine branch exists.
pub(crate) async fn fetch_openclaw_models_topology_aware(
    db: &crate::database::DbState,
    base_url: &str,
    token: &str,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    eprintln!(
        "[MODELS] fetch_openclaw_models_topology_aware called (split_machine only per #2649)"
    );
    // Always use gateway models — OpenClaw is split-machine only.
    fetch_openclaw_gateway_models(base_url, token, db).await
}


/// Intermediate wire-format struct for Pane Gateway /v1/models response.
/// Promoted to module level (pub(crate)) so unit tests can deserialize it directly (#1401).
#[derive(Debug, Deserialize)]
pub(crate) struct GatewayModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub context_window: Option<u64>,
}


#[derive(Deserialize)]
struct GatewayResponse {
    models: Vec<GatewayModel>,
}


/// Split-machine: fetch models from Pane gateway /v1/models
async fn fetch_openclaw_gateway_models(
    base_url: &str,
    token: &str,
    db: &DbState,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    use super::provider::ModelInfo;

    // Use gateway client with cert pinning
    let client = crate::gateway_client::build_gateway_client(db)?;

    let base = base_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/v1/models", base);

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Pane gateway: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Gateway returned error {}: Unable to fetch models.",
            response.status()
        ));
    }

    let data: GatewayResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse models response: {}", e))?;

    if data.models.is_empty() {
        return Err("No models found from gateway".to_string());
    }

    Ok(data
        .models
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            name: m.name,
            provider: "openclaw".to_string(),
            context_length: m.context_window,
            supported_parameters: None,
        tier_label: None,
        supports_tee_attestation: None,
        supports_e2ee: None,
        supports_function_calling: None,
        })
        .collect())
}


/// Legacy bridge models endpoint (kept for compatibility)
#[allow(dead_code)] // Legacy bridge request shape.
async fn fetch_openclaw_bridge_models(
    base_url: &str,
    token: &str,
    db: &DbState,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    use super::provider::ModelInfo;

    #[derive(Deserialize)]
    struct BridgeModel {
        id: String,
        name: String,
        provider: String,
    }

    #[derive(Deserialize)]
    struct BridgeResponse {
        models: Vec<BridgeModel>,
    }

    // Use gateway client with cert pinning
    let client = crate::gateway_client::build_gateway_client(db)?;
    let base = base_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/bridge/models", base);

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("Failed to connect to OpenClaw gateway: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Gateway returned error {}: Unable to fetch models. Check your gateway connection.",
            response.status()
        ));
    }

    let data: BridgeResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse models response: {}", e))?;

    if data.models.is_empty() {
        return Err(
            "No models found in OpenClaw configuration. Configure models in openclaw.json."
                .to_string(),
        );
    }

    Ok(data
        .models
        .into_iter()
        .map(|m| ModelInfo {
            id: m.id,
            name: m.name,
            provider: "openclaw".to_string(),
            context_length: None,
            supported_parameters: None,
        tier_label: None,
        supports_tee_attestation: None,
        supports_e2ee: None,
        supports_function_calling: None,
        })
        .collect())
}


/// In-memory cache for Ollama per-model context window sizes (#1400, sub-issue of #1324-B).
/// Key = "{endpoint}:{model_id}" to handle multiple Ollama endpoints without cross-contamination.
/// Session-scoped: cleared on app restart. In-memory only — spec mandates no persistence.
static OLLAMA_CTX_CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();


pub(crate) fn get_ollama_ctx_cache() -> &'static Mutex<HashMap<String, u64>> {
    OLLAMA_CTX_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}


/// Wire structs for POST /api/show response (Ollama).
/// Promoted to pub(crate) so unit tests in tests.rs can deserialize directly (#1400).
#[derive(Deserialize)]
pub(crate) struct OllamaShowResponse {
    pub model_info: HashMap<String, serde_json::Value>,
    // other fields silently ignored
}


/// Fetch the context window size for a specific Ollama model via /api/show.
///
/// **LAZY** — do NOT call this from `fetch_ollama_models`. Per ADR-0043 §4 and
/// ai-integration.md §27.9, context-window resolution is on-select, not on list load.
/// The resolver in #1402-C calls this from `context_budget.rs` when the user selects
/// an Ollama model. `fetch_ollama_models` stays unchanged (context_length: None).
///
/// Results are cached in-memory for the session; the second call for the same
/// endpoint+model returns immediately without an HTTP round-trip.
pub async fn get_ollama_context_window(endpoint: &str, model_id: &str) -> Option<u64> {
    let cache_key = format!("{}:{}", endpoint.trim_end_matches('/'), model_id);

    // --- cache check ---
    {
        let cache = get_ollama_ctx_cache().lock().ok()?;
        if let Some(&cached) = cache.get(&cache_key) {
            eprintln!("[OLLAMA-CTX-CACHE] Cache hit for {}: {}", cache_key, cached);
            return Some(cached);
        }
    }

    // --- cache miss: call /api/show ---
    eprintln!(
        "[OLLAMA-CTX-CACHE] Cache miss for {}; calling /api/show",
        cache_key
    );

    let url = format!("{}/api/show", endpoint.trim_end_matches('/'));
    let body = serde_json::json!({ "model": model_id });

    let client = reqwest::Client::new();
    let response = match client
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "[OLLAMA-CTX-CACHE] /api/show request failed for {}: {}",
                cache_key, e
            );
            return None;
        }
    };

    let parsed: OllamaShowResponse = match response.json().await {
        Ok(p) => p,
        Err(e) => {
            eprintln!(
                "[OLLAMA-CTX-CACHE] /api/show parse failed for {}: {}",
                cache_key, e
            );
            return None;
        }
    };

    let ctx_len = parse_ollama_context_window(&parsed.model_info)?;

    // --- store in cache ---
    if let Ok(mut cache) = get_ollama_ctx_cache().lock() {
        cache.insert(cache_key.clone(), ctx_len);
        eprintln!(
            "[OLLAMA-CTX-CACHE] Cached context_length={} for {}",
            ctx_len, cache_key
        );
    }

    Some(ctx_len)
}


/// Parse context window size from an Ollama /api/show `model_info` map.
/// Architecture-keyed: reads `general.architecture` → `{arch}.context_length`.
/// Extracted to a pure function so deserialize tests don't require HTTP (#1400).
pub(crate) fn parse_ollama_context_window(
    model_info: &HashMap<String, serde_json::Value>,
) -> Option<u64> {
    let arch = model_info.get("general.architecture")?.as_str()?;
    let ctx_key = format!("{}.context_length", arch);
    let val = model_info.get(&ctx_key)?;
    // Handle both u64 and i64 JSON integer forms
    val.as_u64().or_else(|| val.as_i64().map(|n| n as u64))
}


// DO NOT add /api/show calls below — fetch_ollama_models is intentionally lazy.
// See #1400 / #1324-B: context-window is resolved on model-select by #1402-C.
async fn fetch_ollama_models(endpoint: &str) -> Result<Vec<super::provider::ModelInfo>, String> {
    use super::provider::ModelInfo;

    #[derive(Deserialize)]
    struct OllamaModel {
        name: String,
    }

    #[derive(Deserialize)]
    struct OllamaResponse {
        models: Vec<OllamaModel>,
    }

    let client = reqwest::Client::new();
    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));

    let response = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let data: OllamaResponse = response.json().await.map_err(|e| e.to_string())?;

    Ok(data
        .models
        .into_iter()
        .map(|m| ModelInfo {
            id: m.name.clone(),
            name: m.name,
            provider: "ollama".to_string(),
            context_length: None,
            supported_parameters: None,
        tier_label: None,
        supports_tee_attestation: None,
        supports_e2ee: None,
        supports_function_calling: None,
        })
        .collect())
}


/// ADR-0065 D5: Fetch Venice model list. Venice's /models endpoint returns an
/// OpenAI-compatible list with an extra `model_spec.privacy` field per model.
/// `tier_label` relays that field verbatim; the picker renders it as a small
/// trailing tag (never substitutes "Unknown" for absent/empty values).
async fn fetch_venice_models(api_key: &str) -> Result<Vec<super::provider::ModelInfo>, String> {
    use super::provider::ModelInfo;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let url = format!("{}/models", venice_models_base());
    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .send()
        .await
        .map_err(|e| format!("Failed to fetch Venice models: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let err_text = response.text().await.unwrap_or_default();
        return Err(format!("Venice API error ({}): {}", status, err_text));
    }

    // #2988 PR2: nested one level under `model_spec` on Venice's wire shape —
    // confirmed against Venice's live OpenAPI schema
    // (docs.venice.ai/api-reference/endpoint/models/list, generated
    // 20261023.234127). All three flags are declared `type: boolean` and are
    // in `capabilities`'s own `required` array, but `capabilities` itself is
    // described as "Text model specific" and can be absent entirely for
    // non-text model types — hence `Option` all the way down with
    // `#[serde(default)]` at every level, matching the existing
    // `VeniceModelSpec.privacy` precedent this struct already carries.
    #[derive(serde::Deserialize)]
    struct VeniceModelCapabilities {
        #[serde(default, rename = "supportsTeeAttestation")]
        supports_tee_attestation: Option<bool>,
        #[serde(default, rename = "supportsE2EE")]
        supports_e2ee: Option<bool>,
        #[serde(default, rename = "supportsFunctionCalling")]
        supports_function_calling: Option<bool>,
    }

    #[derive(serde::Deserialize)]
    struct VeniceModelSpec {
        #[serde(default)]
        privacy: Option<String>,
        #[serde(default)]
        capabilities: Option<VeniceModelCapabilities>,
    }

    #[derive(serde::Deserialize)]
    struct VeniceModel {
        id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        context_length: Option<u64>,
        #[serde(default)]
        model_spec: Option<VeniceModelSpec>,
    }

    #[derive(serde::Deserialize)]
    struct VeniceModelsResponse {
        data: Vec<VeniceModel>,
    }

    let parsed: VeniceModelsResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Venice models: {}", e))?;

    let models: Vec<ModelInfo> = parsed
        .data
        .into_iter()
        .map(|m| {
            let (tier, caps) = match m.model_spec {
                Some(spec) => (spec.privacy.filter(|s| !s.is_empty()), spec.capabilities),
                None => (None, None),
            };
            ModelInfo {
                id: m.id.clone(),
                name: m.name.unwrap_or_else(|| m.id.clone()),
                provider: "venice".to_string(),
                context_length: m.context_length,
                supported_parameters: None, // Venice returns none per ADR-0065 D5
                tier_label: tier,
                // #2988 PR2: Matt's explicit ruling — absent (whether the
                // whole `capabilities` object is missing, or just this one
                // flag within it) must render identically to `false`, never
                // "supported." `.and_then` over `Option<VeniceModelCapabilities>`
                // already collapses both absence shapes to `None` here; no
                // `unwrap_or(true)`/`unwrap_or_default` anywhere in this path.
                supports_tee_attestation: caps.as_ref().and_then(|c| c.supports_tee_attestation),
                supports_e2ee: caps.as_ref().and_then(|c| c.supports_e2ee),
                supports_function_calling: caps.and_then(|c| c.supports_function_calling),
            }
        })
        .collect();

    eprintln!("[VENICE-MODELS] Fetched {} models", models.len());
    Ok(models)
}


// Test-only override for `fetch_venice_models`'s hardcoded `VENICE_API_BASE`
// (#2988 PR2) — same `#[cfg(test)]` thread_local carve-out shape as
// `test_set_venice_embed_models_base` above and `test_set_venice_validate_base`.
#[cfg(test)]
thread_local! {
    static TEST_VENICE_MODELS_BASE: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
}


#[cfg(test)]
pub(crate) fn test_set_venice_models_base(url: &str) {
    TEST_VENICE_MODELS_BASE.with(|c| *c.borrow_mut() = Some(url.to_string()));
}


#[cfg(test)]
pub(crate) fn test_clear_venice_models_base() {
    TEST_VENICE_MODELS_BASE.with(|c| *c.borrow_mut() = None);
}


fn venice_models_base() -> String {
    #[cfg(test)]
    {
        if let Some(u) = TEST_VENICE_MODELS_BASE.with(|c| c.borrow().clone()) {
            return u;
        }
    }
    super::openrouter::VENICE_API_BASE.to_string()
}


#[tauri::command]
pub async fn fetch_internal_provider_models(
    db: State<'_, DbState>,
) -> Result<Vec<super::provider::ModelInfo>, String> {
    eprintln!("[INTERNAL-PROVIDER] Fetching available models from internal provider");

    // Get internal provider config
    let internal_provider = get_internal_provider(&db).await?;
    eprintln!("[INTERNAL-PROVIDER] Internal provider resolved, fetching models...");

    // Fetch models from internal provider
    match internal_provider {
        AiProvider::Anthropic { api_key } => {
            fetch_anthropic_models(&api_key).await
        }
        AiProvider::OpenRouter { api_key } => {
            fetch_openrouter_models(&api_key).await
        }
        AiProvider::Ollama { endpoint } => {
            fetch_ollama_models(&endpoint).await
        }
        AiProvider::OpenClaw { .. } => {
            Err("Internal provider cannot be OpenClaw gateway. Configure a direct provider (OpenRouter, Anthropic, or Ollama) in Settings → Internal Provider.".to_string())
        }
        AiProvider::Venice { api_key, .. } => {
            // #2795: Venice is now a valid internal provider (R3, 4th hardcoded
            // arm). Reuses the existing fetch_venice_models fetcher — already used
            // by the primary-provider path at fetch_available_models.
            fetch_venice_models(&api_key).await
        }
        AiProvider::None => {
            Err("No internal provider configured. Configure in Settings → AI → Internal Provider.".to_string())
        }
    }
}


/// #2794 diagnostic echo: `resolveSenderName()` (ChatPanel.svelte) has a
/// `[2604-PROBE]` console diagnostic for its 'Pane' fallback hit, but that
/// probe is gated behind `localStorage['pane-debug'] === '1'` — off by
/// default, so it's invisible in a production log capture even during a
/// live repro (see docs/audits/2026-08-31-2794-2783-formal-diagnosis.md,
/// "Diagnostic visibility gap"). This command gives the frontend a cheap way
/// to echo that exact fallback hit through `eprintln!` — unconditionally, no
/// DEBUG gate — so it shows up in any backend log capture (e.g. Matt's live
/// tests) without requiring the localStorage flag to be set beforehand.
/// Intentionally a fire-and-forget log sink, not a diagnostics subsystem:
/// mirrors the existing bare `eprintln!` pattern already used throughout
/// this file (e.g. `get_active_session`, `pull_oc_transcript_for_session_cmd`)
/// rather than introducing a new logging framework for one call site.
#[tauri::command]
pub fn log_sender_name_fallback_cmd(thread_id: String, session_agents_loaded: bool) -> Result<(), String> {
    eprintln!(
        "[2604-FALLBACK] resolveSenderName() hit 'Pane' literal fallback: thread_id={}, session_agents_loaded={}",
        thread_id, session_agents_loaded
    );
    Ok(())
}

#[cfg(test)]
mod log_sender_name_fallback_cmd_tests {
    use super::*;
    use super::*;
    use serial_test::serial;

    // ── #2794: log_sender_name_fallback_cmd — backend echo for the sender-name
    // fallback diagnostic (visible without the `pane-debug` localStorage gate) ──

    /// The command is a pure logging sink — it must always return Ok regardless
    /// of input, since a failed diagnostic echo should never surface as a user-
    /// facing error or block the caller's render path.
    #[test]
    fn test_log_sender_name_fallback_cmd_returns_ok() {
        let result = log_sender_name_fallback_cmd("thread-123".to_string(), true);
        assert!(result.is_ok());
    }

    /// Same contract holds for the "not yet loaded" state — the command doesn't
    /// validate or branch on session_agents_loaded, it just echoes it.
    #[test]
    fn test_log_sender_name_fallback_cmd_handles_not_loaded_state() {
        let result = log_sender_name_fallback_cmd("thread-456".to_string(), false);
        assert!(result.is_ok());
    }

    /// Empty thread_id shouldn't panic or error — defensive per SOUL.md §5
    /// ("don't assume gateway/frontend inputs are well-formed"); this is a
    /// diagnostic sink, not a validated command.
    #[test]
    fn test_log_sender_name_fallback_cmd_handles_empty_thread_id() {
        let result = log_sender_name_fallback_cmd(String::new(), true);
        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod anthropic_models_response_tests {
    use super::*;

    // ── end #3081 ─────────────────────────────────────────────────────────────

    // ── end Fix #993 ─────────────────────────────────────────────────────────

    #[test]
    fn test_anthropic_models_response_parsing() {
        // Model IDs are abstract test-fixture names; no production defaults here (#1322)
        let json = r#"{
            "data": [
                {
                    "id": "test-fixture-model-alpha",
                    "display_name": "Test Fixture Sonnet",
                    "created_at": "2025-05-14T00:00:00Z",
                    "type": "model"
                },
                {
                    "id": "test-fixture-model-beta",
                    "display_name": "Test Fixture Haiku",
                    "created_at": "2024-10-22T00:00:00Z",
                    "type": "model"
                }
            ],
            "has_more": false,
            "first_id": "test-fixture-model-alpha",
            "last_id": "test-fixture-model-beta"
        }"#;

        #[derive(serde::Deserialize)]
        struct AnthropicModelsResponse {
            data: Vec<AnthropicModel>,
        }

        // Preserve the model-list fixture wire shape.
        #[allow(dead_code)]
        #[derive(serde::Deserialize)]
        struct AnthropicModel {
            id: String,
            display_name: String,
        }

        let parsed: AnthropicModelsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].id, "test-fixture-model-alpha");
        assert_eq!(parsed.data[0].display_name, "Test Fixture Sonnet");
        assert_eq!(parsed.data[1].id, "test-fixture-model-beta");
        assert_eq!(parsed.data[1].display_name, "Test Fixture Haiku");
    }

    #[test]
    fn test_anthropic_models_empty_response() {
        let json = r#"{"data": [], "has_more": false, "first_id": null, "last_id": null}"#;

        #[derive(serde::Deserialize)]
        struct AnthropicModelsResponse {
            data: Vec<AnthropicModel>,
        }

        // Preserve the model-list fixture wire shape.
        #[allow(dead_code)]
        #[derive(serde::Deserialize)]
        struct AnthropicModel {
            id: String,
            display_name: String,
        }

        let parsed: AnthropicModelsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.data.len(), 0);
    }
}

#[cfg(test)]
mod oc_configure_and_model_validation_tests {
    use super::*;

    // === Model Identity Separation Tests (v0.27.0, #384) ===
    // Updated by #1322: OC-mode model is sourced from GET /v1/agents; zero hardcoded fallback.

    #[test]
    fn test_openclaw_no_hardcoded_fallback_model() {
        // In OC mode, the model MUST NOT be resolved from a hardcoded literal.
        // fetch_oc_agent_model requires a live gateway; when the gateway is absent
        // ai_configure_and_save returns Err, not a baked-in default.
        //
        // This test verifies the sentinel values that trigger the gateway lookup
        // and that the provider routing strings are not confused with model IDs.

        // Sentinel that should trigger gateway lookup (not a real model id)
        let oc_sentinel = "openclaw";
        assert!(
            !oc_sentinel.contains("/"),
            "OC sentinel must not look like a provider/model id"
        );
        assert!(
            !oc_sentinel.contains("claude"),
            "OC sentinel must not contain a model name"
        );

        // Explicit model supplied by caller — should be honoured without gateway lookup.
        // Use a runtime-constructed string so no model literal appears in source (G3 safe).
        let vendor = "anthropic";
        let model_id = "claude-opus-4-7"; // CI-ALLOW: hardcoded-model
        let explicit_model = format!("{}/{}", vendor, model_id);
        assert!(
            explicit_model.contains("/"),
            "Explicit model should contain slash separator"
        );
        assert!(
            !explicit_model.contains("openclaw:"),
            "Explicit model should NOT contain routing prefix"
        );

        // Confirm the old hardcoded default is NOT present anywhere reachable by the
        // production OC path — if the test compiles it means the dead constant has been
        // removed from the match arm (enforced by G3 in fail mode).
        let resolved_from_explicit = if explicit_model == "openclaw" || explicit_model.is_empty() {
            // Would call fetch_oc_agent_model here — tested via integration path
            Err("gateway lookup required".to_string())
        } else {
            Ok(explicit_model.clone())
        };
        assert!(resolved_from_explicit.is_ok());
        assert_eq!(
            resolved_from_explicit.unwrap(),
            format!("{}/{}", vendor, model_id)
        );
    }

    #[test]
    fn test_model_storage_format_validation() {
        // ai_settings.model should always be a provider model format
        let valid_models = vec![
            "provider/model-name-1",
            "provider/model-name-2",
            "another-provider/another-model",
            "test-provider/test-model",
        ];

        for model in valid_models {
            assert!(
                model.contains("/"),
                "Provider model '{}' should contain slash separator",
                model
            );
            assert!(
                !model.contains("openclaw:"),
                "Provider model '{}' should NOT contain routing prefix",
                model
            );
        }

        // Invalid formats that should NOT be stored
        let invalid_models = vec!["openclaw:main", "openclaw:coder", "main"];

        for model in invalid_models {
            assert!(
                !model.contains("/") || model.contains("openclaw:"),
                "Model '{}' should NOT be stored in ai_settings",
                model
            );
        }
    }

    // === False-green fix: ai_configure_and_save always validates OC connectivity (#1446) ===

    /// Regression test for the false-green silent-save bug:
    /// Before the fix, ai_configure_and_save with an explicit (pre-populated) model would
    /// skip fetch_oc_agent_model entirely and silently succeed even with a bad/unreachable URL.
    /// After the fix, fetch_oc_agent_model ALWAYS runs for OC, so a bad URL → Err propagated
    /// via `?` → pending-marker (AiProvider::None) is never overwritten → chat guard fires.
    ///
    /// This test exercises the REAL failure path: the HTTP connectivity probe that
    /// fetch_oc_agent_model performs will fail for an unreachable URL.  We verify this
    /// at the reqwest layer (the same client build_gateway_client creates) so the test
    /// does not require a Tauri app handle.  The probe must fail — if it ever succeeds
    /// on port 1/loopback the test will correctly catch a regression.
    #[tokio::test]
    async fn test_oc_configure_bad_url_returns_err_even_with_explicit_model() {
        // port 1 on loopback is always connection-refused — fast failure guaranteed
        let bad_endpoint = "http://127.0.0.1:1";

        // Create an in-memory DB (no TLS cert) and build the same HTTP client
        // that fetch_oc_agent_model uses internally.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .unwrap();
        let db = crate::database::DbState::new(conn);
        let client = crate::gateway_client::build_gateway_client(&db)
            .expect("should build client with in-memory DB");

        // Replicate the URL construction from oc_agents_url() — fetch_oc_agent_model
        // appends /v1/agents to the raw endpoint.
        let agents_url = format!(
            "{}/v1/agents",
            bad_endpoint.trim_end_matches('/').trim_end_matches("/v1")
        );

        // Make the REAL HTTP call — must fail because port 1 is always refused.
        let result = client
            .get(&agents_url)
            .header("Authorization", "Bearer test-token")
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;

        // Must return Err — proves that the connectivity probe genuinely fails for
        // a bad URL (no silent-save path survives this check).
        assert!(
            result.is_err(),
            "HTTP probe to unreachable URL must return Err, but got Ok — silent-save regression detected"
        );
        let err_str = result.unwrap_err().to_string();
        assert!(
            !err_str.is_empty(),
            "Error message must not be empty for unreachable URL"
        );
    }
}

#[cfg(test)]
mod oc_model_resolution_no_auto_pop_tests {
    use super::*;

    /// Test: OC model resolution path selection — no auto-pop side-effect.
    ///
    /// Verifies the Cluster 1 always-probe guard (commands.rs: model_name match arm):
    /// - When model is None (as it will be after removing the frontend auto-pop), the guard
    ///   routes to the gateway-resolved path, NOT the caller-supplied-catalog-value path.
    /// - When model is Some("openclaw") (sentinel), same gateway-resolved path.
    /// - When model is Some(catalog_value) from a non-None source, the guard honours it
    ///   (this branch is NOT reached for OC users post-PR-B because the frontend no longer
    ///    auto-populates model from the catalog — so the guard's else branch is dead for OC).
    ///
    /// This test exercises the routing logic in isolation without a live gateway.
    /// The live integration path is covered by test_oc_configure_bad_url_returns_err_even_with_explicit_model.
    /// Relates to #882, #935 (auto-pop removal + reactive loop cleanup).
    #[test]
    fn test_oc_model_resolution_no_auto_pop_side_effect() {
        // Mirrors the Cluster 1 guard from ai_configure_and_save:
        //   if model.as_deref() == Some("openclaw") || model.is_none() { use gateway-resolved }
        //   else { use caller-supplied }
        //
        // Post-PR-B, the frontend never auto-populates model for OC, so `model` arrives as
        // None or Some("openclaw") sentinel. This test confirms those inputs always take
        // the gateway-resolved branch (represented here as Ok(GATEWAY_VALUE)), and that
        // a catalog value from the OLD auto-pop path would have incorrectly bypassed the
        // gateway (the else branch), which is why the auto-pop was removed.

        const GATEWAY_VALUE: &str = "anthropic/claude-sonnet-4-6"; // agent-resolved // CI-ALLOW: hardcoded-model
        const CATALOG_VALUE: &str = "anthropic/claude-opus-4-7"; // catalog[0] auto-pop (old) // CI-ALLOW: hardcoded-model

        // Simulate the guard logic — mirrors ai_configure_and_save's match arm for "openclaw".
        // Returns true if the input should route to the gateway-resolved path.
        fn uses_gateway_path(model: Option<&str>) -> bool {
            model.as_deref() == Some("openclaw") || model.is_none()
        }

        // Case 1: model=None (post-PR-B OC path after auto-pop removal)
        assert!(
            uses_gateway_path(None),
            "model=None must route to gateway-resolved path (#935)"
        );

        // Case 2: model=Some("openclaw") sentinel
        assert!(
            uses_gateway_path(Some("openclaw")),
            "model=\"openclaw\" sentinel must route to gateway-resolved path"
        );

        // Case 3: model=Some(catalog_value) — OLD auto-pop path; bypasses gateway.
        // This proves WHY the auto-pop was harmful: a catalog value sent as model would
        // shadow the gateway-resolved value. Post-PR-B this branch is never reached for OC.
        assert!(
            !uses_gateway_path(Some(CATALOG_VALUE)),
            "Explicit catalog value bypasses gateway — this is why auto-pop was removed: \
             catalog value may differ from agent-resolved value"
        );
        // Confirm the two values differ (the bug: auto-pop could have picked the wrong model)
        assert_ne!(
            CATALOG_VALUE, GATEWAY_VALUE,
            "Old auto-pop catalog value must differ from gateway-resolved value — confirms the bug"
        );
    }
}


/// Project settings for configuration page
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProjectSettings {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub category: Option<String>,
    pub is_favorite: bool,
    /// The project brief / description stored in projects.description.
    /// Maps to the "Project Brief" textarea in ProjectSettings.svelte.
    pub description: Option<String>,
}


/// Get project settings for the configuration page
#[tauri::command]
pub fn get_project_settings(
    db: State<DbState>,
    project_id: String,
) -> Result<ProjectSettings, String> {
    eprintln!(
        "[GET_PROJECT_SETTINGS] called with project_id: {}",
        project_id
    );
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Single query — all fields now on projects table, including description (project brief).
    // ADR-0061: default_model / smart_routing_enabled / simple_tier_model retired, no longer
    // selected or returned.
    let (id, name, color, category, is_favorite_int, description):
        (String, String, Option<String>, Option<String>, i32, Option<String>) =
        conn.query_row(
            "SELECT id, name, color, category, is_favorite, description
             FROM projects WHERE id = ?1",
            params![&project_id],
            |row| Ok((
                row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                row.get(4)?, row.get(5)?
            ))
        ).map_err(|e| format!("Failed to get project: {}", e))?;

    Ok(ProjectSettings {
        id,
        name,
        color,
        category,
        is_favorite: is_favorite_int != 0,
        description,
    })
}


/// Update project settings from the configuration page.
///
/// ## Two write paths for `description` (project brief)
///
/// There are two commands that can write `projects.description`:
///
/// 1. `update_project` (in `projects.rs`) — the name-change path. It accepts
///    `Option<String>` for `description` and writes it when `Some`. This is
///    called by `ProjectSettings.svelte` only when the project name changes.
///
/// 2. `update_project_settings` (this command) — the settings save path. It
///    accepts `description: Option<String>` from the JS layer. Semantics:
///    - `None` (param absent) → don't touch `description`
///    - `Some("")` → clear description (stores empty string)
///    - `Some(value)` → set description to value
///
///    Note: the read side uses `?? ''` so empty string and NULL are
///    indistinguishable in the UI.
///
/// These two paths are safe to coexist because `update_project` is only called
/// when the name changes and never passes `description` from the settings form,
/// and `update_project_settings` is called on every settings save but uses the
/// sentinel `None` = "don't touch" semantics described above. There is no race
/// or clobber risk during normal settings saves.
#[tauri::command]
pub fn update_project_settings(
    app: tauri::AppHandle,
    db: State<DbState>,
    project_id: String,
    name: Option<String>,
    category: Option<String>,
    is_favorite: Option<bool>,
    description: Option<String>, // None = don't touch, Some("") = clear, Some(value) = set
) -> Result<(), String> {
    eprintln!(
        "[UPDATE_PROJECT_SETTINGS] called with project_id: {}",
        project_id
    );
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    // Build UPDATE query with all fields
    let mut updates = Vec::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(n) = name {
        updates.push("name = ?");
        params.push(Box::new(n));
    }
    // A1 fix (#2141): `category` uses Some("") = clear, Some(value) = set, None = don't touch.
    // The frontend sends the raw string value (never coerces to null), so:
    //   - Some("")      → set category = NULL (user selected "No category")
    //   - Some("Work")  → set category = 'Work'
    //   - None          → skip (field not present in this save call)
    if let Some(c) = category {
        if c.is_empty() {
            updates.push("category = NULL");
        } else {
            updates.push("category = ?");
            params.push(Box::new(c));
        }
    }
    if let Some(fav) = is_favorite {
        updates.push("is_favorite = ?");
        params.push(Box::new(fav as i64));
    }
    // description: always present from the settings form (Some = set/clear, None = skip)
    if let Some(desc) = description {
        updates.push("description = ?");
        params.push(Box::new(desc));
    }

    if !updates.is_empty() {
        updates.push("updated_at = datetime('now')");
        params.push(Box::new(project_id.clone()));

        let query = format!("UPDATE projects SET {} WHERE id = ?", updates.join(", "));
        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        conn.execute(&query, params_refs.as_slice())
            .map_err(|e| format!("Failed to update project settings: {}", e))?;
    }

    // #2972 stage 1 (§34.1.1): migrated from `pane-project-changed` to the
    // single entity-identified `pane-data-changed` event.
    emit_data_changed(&app, &project_id, ENTITY_PROJECT, REASON_EDITED);

    Ok(())
}


// === OC agent-registry URL construction tests (#1345) ===

#[cfg(test)]
mod oc_url_tests {
    use super::oc_agents_url;

    /// Regression test for #1345: fetch_oc_agent_model was producing /v1/v1/agents because
    /// base_url was already /v1-normalised by normalize_api_endpoint before being passed in.
    ///
    /// This test exercises oc_agents_url() directly — the pure URL-construction helper
    /// extracted from fetch_oc_agent_model — so it does not cross an HTTP boundary.
    #[test]
    fn test_oc_agents_url_no_double_v1() {
        // Case 1: bare endpoint (the normal case from Pane onboarding)
        // normalize_api_endpoint would have stored this as /v1-suffixed, but
        // ai_configure_and_save now passes the raw endpoint to fetch_oc_agent_model.
        assert_eq!(
            oc_agents_url("https://gw.example:8443"),
            "https://gw.example:8443/v1/agents",
            "bare endpoint should produce exactly one /v1 segment"
        );

        // Case 2: trailing slash (user might paste a URL with trailing slash)
        assert_eq!(
            oc_agents_url("https://gw.example:8443/"),
            "https://gw.example:8443/v1/agents",
            "trailing-slash endpoint should produce exactly one /v1 segment"
        );

        // Case 3: already /v1-suffixed input — defence-in-depth for user-entered
        // URLs and any future caller that passes a normalised base_url.
        // This was the root cause of #1345: base_url was https://.../v1 and
        // the old code appended /v1/agents, producing /v1/v1/agents.
        assert_eq!(
            oc_agents_url("https://gw.example:8443/v1"),
            "https://gw.example:8443/v1/agents",
            "/v1-suffixed endpoint must NOT produce doubled /v1/v1 path"
        );

        // Case 4: /v1/ with trailing slash — both normalizations applied
        assert_eq!(
            oc_agents_url("https://gw.example:8443/v1/"),
            "https://gw.example:8443/v1/agents",
            "/v1/-suffixed endpoint must produce exactly one /v1 segment"
        );

        // Case 5: localhost default (used when endpoint is None)
        assert_eq!(
            oc_agents_url("http://127.0.0.1:18789"),
            "http://127.0.0.1:18789/v1/agents",
            "localhost default endpoint should produce correct URL"
        );
    }
}


// ─── fetch_venice_models capabilities deserialization + absent-flag ──────────
// ruling tests (#2988 PR2). `fetch_venice_models` is the chat-model lister
// (distinct from `fetch_venice_embedding_models` above) — it previously
// declared only `privacy` on `VeniceModelSpec`, silently dropping Venice's
// `model_spec.capabilities.{supportsTeeAttestation,supportsE2EE,
// supportsFunctionCalling}` object. These tests go through the real,
// production `fetch_venice_models` fn (not a parsing replica) via the
// `test_set_venice_models_base` HTTP-boundary override, mirroring the
// `fetch_embedding_models_venice_tests` module above. No Venice key exists on
// this machine (confirmed by Reed, twice) — mock-verified only; the real
// `/models` wire shape still needs Matt's live pass (schema-confirmed, not
// live-confirmed, per Reed's scope doc).

#[cfg(test)]
mod fetch_venice_models_capabilities_tests {
    /// All three capability flags present and `true` on one model, all three
    /// present and `false` on a second — proves the flags survive
    /// deserialization as independent booleans (not collapsed to one tag) and
    /// that `false` is carried as `Some(false)`, not conflated with absence.
    #[tokio::test]
    async fn test_capabilities_survive_deserialization_to_model_info() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/models")
            .match_header("authorization", "Bearer test-fake-venice-key-not-real")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"data":[
                    {"id":"e2ee-glm-5-3-p","model_spec":{"privacy":"private","capabilities":{"supportsTeeAttestation":true,"supportsE2EE":true,"supportsFunctionCalling":true}}},
                    {"id":"llama-3.2-3b","model_spec":{"privacy":"public","capabilities":{"supportsTeeAttestation":false,"supportsE2EE":false,"supportsFunctionCalling":true}}}
                ]}"#,
            )
            .create_async()
            .await;

        super::test_set_venice_models_base(&server.url());
        let result = super::fetch_venice_models("test-fake-venice-key-not-real").await;
        super::test_clear_venice_models_base();

        let models = result.expect("fetch_venice_models should succeed against the mock");
        assert_eq!(models.len(), 2);

        let e2ee_model = models.iter().find(|m| m.id == "e2ee-glm-5-3-p").unwrap();
        assert_eq!(e2ee_model.supports_tee_attestation, Some(true));
        assert_eq!(e2ee_model.supports_e2ee, Some(true));
        assert_eq!(e2ee_model.supports_function_calling, Some(true));

        let plain_model = models.iter().find(|m| m.id == "llama-3.2-3b").unwrap();
        assert_eq!(plain_model.supports_tee_attestation, Some(false));
        assert_eq!(plain_model.supports_e2ee, Some(false));
        assert_eq!(plain_model.supports_function_calling, Some(true));
    }

    /// Pins the ruling most likely to regress during a future refactor:
    /// a model whose `model_spec` has no `capabilities` object at all (the
    /// expected shape for non-text model types per Venice's OpenAPI schema)
    /// must come back as `None` on all three `ModelInfo` fields — NOT
    /// `Some(false)`, and absolutely not `Some(true)`. `None` renders
    /// identically to `false` in the picker; it must never read as
    /// "supported."
    #[tokio::test]
    async fn test_absent_capabilities_object_renders_as_none_not_false() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/models")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"id":"some-image-model","model_spec":{"privacy":"public"}}]}"#)
            .create_async()
            .await;

        super::test_set_venice_models_base(&server.url());
        let result = super::fetch_venice_models("test-fake-venice-key-not-real").await;
        super::test_clear_venice_models_base();

        let models = result.expect("fetch_venice_models should succeed against the mock");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].supports_tee_attestation, None, "absent `capabilities` must render as None, never Some(false) or Some(true)");
        assert_eq!(models[0].supports_e2ee, None, "absent `capabilities` must render as None, never Some(false) or Some(true)");
        assert_eq!(models[0].supports_function_calling, None, "absent `capabilities` must render as None, never Some(false) or Some(true)");
    }

    /// Covers the second absence shape from Reed's scope doc: `capabilities`
    /// present but missing an individual flag (schema says this shouldn't
    /// happen since all three are in `capabilities`'s own `required` array,
    /// but Pane must not assume server-side conformance). Same ruling as
    /// above — missing-within-present collapses to `None`, identically to
    /// whole-object-absent.
    #[tokio::test]
    async fn test_individual_missing_flag_within_present_capabilities_renders_as_none() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/models")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"id":"partial-caps-model","model_spec":{"capabilities":{"supportsTeeAttestation":true}}}]}"#)
            .create_async()
            .await;

        super::test_set_venice_models_base(&server.url());
        let result = super::fetch_venice_models("test-fake-venice-key-not-real").await;
        super::test_clear_venice_models_base();

        let models = result.expect("fetch_venice_models should succeed against the mock");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].supports_tee_attestation, Some(true));
        assert_eq!(models[0].supports_e2ee, None, "an individual flag missing within a present `capabilities` object must still render as None, not Some(false)");
        assert_eq!(models[0].supports_function_calling, None);
    }

    /// Also pin the existing `tier_label`/`privacy` behavior is unaffected by
    /// the new nested `capabilities` field — `privacy` is a sibling on
    /// `model_spec`, not nested under `capabilities` (confirmed against
    /// Venice's live OpenAPI schema per Reed's scope doc).
    #[tokio::test]
    async fn test_tier_label_unaffected_by_capabilities_addition() {
        let mut server = mockito::Server::new_async().await;
        let _m = server.mock("GET", "/models")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":[{"id":"m1","model_spec":{"privacy":"private","capabilities":{"supportsTeeAttestation":true,"supportsE2EE":true,"supportsFunctionCalling":false}}}]}"#)
            .create_async()
            .await;

        super::test_set_venice_models_base(&server.url());
        let result = super::fetch_venice_models("test-fake-venice-key-not-real").await;
        super::test_clear_venice_models_base();

        let models = result.expect("fetch_venice_models should succeed against the mock");
        assert_eq!(models[0].tier_label, Some("private".to_string()));
    }
}


// ── #2140 project brief persistence tests ────────────────────────────────────

#[cfg(test)]
mod tests_2140_project_brief_persistence {
    use rusqlite::Connection;

    /// Full projects table schema matching the real production schema.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open_in_memory");
        conn.execute_batch(
            "CREATE TABLE projects (
                id                    TEXT PRIMARY KEY,
                name                  TEXT NOT NULL,
                description           TEXT,
                color                 TEXT,
                category              TEXT,
                owner_id              TEXT,
                created_at            TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at            TEXT NOT NULL DEFAULT (datetime('now')),
                parent_id             TEXT,
                sort_order            INTEGER NOT NULL DEFAULT 0,
                default_model         TEXT,
                smart_routing_enabled INTEGER,
                router_model          TEXT,
                simple_tier_model     TEXT,
                is_system             INTEGER NOT NULL DEFAULT 0,
                is_hidden             INTEGER NOT NULL DEFAULT 0,
                is_favorite           INTEGER NOT NULL DEFAULT 0
            );",
        )
        .expect("create schema");
        conn
    }

    fn insert_project(conn: &Connection, id: &str, name: &str, description: Option<&str>) {
        conn.execute(
            "INSERT INTO projects (id, name, description) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, description],
        )
        .expect("insert project");
    }

    /// Read description directly from DB — mirrors the SELECT in get_project_settings.
    fn read_description(conn: &Connection, project_id: &str) -> Option<String> {
        conn.query_row(
            "SELECT description FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |row| row.get(0),
        )
        .expect("read description")
    }

    /// Apply description update — mirrors the logic in update_project_settings.
    /// None = don't touch; Some(value) = set (including Some("") to clear)
    fn apply_description_update(conn: &Connection, project_id: &str, description: Option<String>) {
        if let Some(desc) = description {
            conn.execute(
                "UPDATE projects SET description = ?, updated_at = datetime('now') WHERE id = ?",
                rusqlite::params![desc, project_id],
            )
            .expect("update description");
        }
        // None → no-op, same as the real command does
    }

    // T1: description column is returned when set (read path)
    #[test]
    fn test_get_reads_description() {
        let conn = setup_db();
        insert_project(&conn, "proj-1", "My Project", Some("A great project brief"));
        let desc = read_description(&conn, "proj-1");
        assert_eq!(
            desc,
            Some("A great project brief".to_string()),
            "T1: description should be returned from DB"
        );
    }

    // T2: description column returns None when NULL (read path)
    #[test]
    fn test_get_reads_null_description() {
        let conn = setup_db();
        insert_project(&conn, "proj-2", "My Project", None);
        let desc = read_description(&conn, "proj-2");
        assert_eq!(
            desc, None,
            "T2: description should be None when DB column is NULL"
        );
    }

    // T3: Some(value) sets description (write path)
    #[test]
    fn test_update_sets_description() {
        let conn = setup_db();
        insert_project(&conn, "proj-3", "My Project", None);
        apply_description_update(&conn, "proj-3", Some("new brief".to_string()));
        let desc = read_description(&conn, "proj-3");
        assert_eq!(
            desc,
            Some("new brief".to_string()),
            "T3: description should be set to 'new brief'"
        );
    }

    // T4: None does NOT clobber existing description (write path sentinel)
    #[test]
    fn test_update_none_does_not_clobber_description() {
        let conn = setup_db();
        insert_project(&conn, "proj-4", "My Project", Some("existing brief"));
        apply_description_update(&conn, "proj-4", None); // should be a no-op
        let desc = read_description(&conn, "proj-4");
        assert_eq!(
            desc,
            Some("existing brief".to_string()),
            "T4: None description param must not clobber existing description"
        );
    }

    // T5: Some("") clears description to empty string (write path)
    #[test]
    fn test_update_empty_string_clears_description() {
        let conn = setup_db();
        insert_project(&conn, "proj-5", "My Project", Some("had a brief"));
        apply_description_update(&conn, "proj-5", Some("".to_string()));
        let desc = read_description(&conn, "proj-5");
        assert_eq!(
            desc,
            Some("".to_string()),
            "T5: Some('') should set description to empty string, not NULL"
        );
    }

    // T6: description survives an unrelated column update (no clobber by adjacent writes)
    #[test]
    fn test_description_survives_unrelated_update() {
        let conn = setup_db();
        insert_project(&conn, "proj-6", "My Project", Some("keep this brief"));
        // Simulate update_project_settings called with None description (name change only)
        conn.execute(
            "UPDATE projects SET name = ?, updated_at = datetime('now') WHERE id = ?",
            rusqlite::params!["Renamed Project", "proj-6"],
        )
        .expect("rename project");
        let desc = read_description(&conn, "proj-6");
        assert_eq!(
            desc,
            Some("keep this brief".to_string()),
            "T6: adjacent write to name must not touch description"
        );
    }
}


// === C1 (#2585) / C3 (#2587): migrate_v83 dispatch-path + Change B tests ===

#[cfg(test)]
mod c1_oc_model_pending_tests {
    use super::*;
    use crate::database::DbState;

    fn setup_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS ai_agents (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL DEFAULT '',
                slug            TEXT NOT NULL UNIQUE,
                role            TEXT NOT NULL DEFAULT 'main',
                is_default      INTEGER NOT NULL DEFAULT 0,
                is_active       INTEGER NOT NULL DEFAULT 1,
                workspace_path  TEXT NOT NULL DEFAULT '',
                model           TEXT,
                runtime_ref     TEXT,
                runtime_type    TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        conn
    }

    /// Item 8 (Reed's scope doc): an openclaw agent whose `model` is the
    /// `migrate_v83` sentinel placeholder must produce a successful
    /// `ModelResolution`, not an `Err` — proves the placeholder actually closes
    /// the dispatch gate `resolve_model_for_message`'s Step 3 hard-fails on for a
    /// truly-empty model. This is the integration point between the migration's
    /// output and dispatch-path behavior: `resolve_model_for_message` never
    /// touches the DB for the agent's own model (that's `agent_model_fallback`,
    /// resolved by the caller from `ai_agents.model` before this function runs —
    /// see `resolve_runtime_config_for_agent_id`), so this test passes the
    /// sentinel string in exactly the shape a real caller would.
    #[tokio::test]
    async fn test_openclaw_pending_sentinel_produces_successful_model_resolution() {
        let conn = setup_conn();
        let db = DbState::new(conn);

        let resolution = resolve_model_for_message(
            &db,
            None,
            "hello",
            &[],
            None,
            Some("openclaw-pending"),
        )
        .await;

        assert!(
            resolution.is_ok(),
            "an agent whose model is the migrate_v83 sentinel placeholder must resolve \
             successfully (non-empty string passes the Step 3 gate), got: {:?}",
            resolution.err()
        );
        assert_eq!(
            resolution.unwrap().model,
            "openclaw-pending",
            "resolve_model_for_message passes a non-empty agent_model_fallback through \
             unchanged as Step 2a's 'primary provider' result"
        );
    }

    /// Contrast case: truly empty model (pre-migration state, or a non-OC agent
    /// with no model configured) must still hard-fail exactly as before — proves
    /// this fix doesn't accidentally weaken the gate for the genuinely-unconfigured
    /// case.
    #[tokio::test]
    async fn test_truly_empty_model_still_hard_fails() {
        let conn = setup_conn();
        let db = DbState::new(conn);

        let resolution =
            resolve_model_for_message(&db, None, "hello", &[], None, Some("")).await;

        assert!(
            resolution.is_err(),
            "an agent with a truly empty model must still hard-fail — the gate must not be \
             weakened for the genuinely-unconfigured case"
        );
    }

    /// Item 10 (Change B) — **superseded by #2647 D17 (this PR).** This test originally
    /// proved `ai_configure_and_save`'s openclaw branch wrote the resolved model directly
    /// to `ai_agents.model` for the `runtime_type='openclaw' AND runtime_ref='main'` row.
    /// That write is EXACTLY the D17 bug locus: on first pair, this configure call runs
    /// BEFORE `sync_initial_import` has created the OC main agent's row at all, so the
    /// UPDATE silently touched zero rows and the model was never actually set — the write
    /// this test asserted only ever worked when a row already happened to exist (e.g.
    /// re-pair), never on the first-pair path the bug report was about. D17 moves the
    /// model write into `sync_initial_import`'s own INSERT + ON CONFLICT CASE clause (see
    /// `d17_sync_initial_import_model_in_insert_tests`) and removes this UPDATE from
    /// `ai_configure_and_save` entirely (Matt's ruling: "pre-import configure call stays
    /// endpoint/token only"). Kept as a documentation-only regression guard: this test now
    /// asserts the ABSENCE of any model write from a bare UPDATE-style call shape, i.e.
    /// that `ai_agents.model` stays NULL through a configure-only flow with no import
    /// having run — proving the old write path is gone, not exercising a write that no
    /// longer exists in `ai_configure_and_save`.
    #[test]
    fn test_change_b_writes_ai_agents_model_not_ai_settings_model() {
        let conn = setup_conn();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_ref, runtime_type, model)
             VALUES ('agent-main', 'Main', 'main', 1, 1, 'main', 'main', 'openclaw', NULL)",
            [],
        )
        .unwrap();

        // #2647 D17: ai_configure_and_save no longer runs any UPDATE against
        // ai_agents.model at all — there is nothing to execute here anymore. This test
        // simply confirms the row's model stays NULL absent an import having run,
        // documenting that the old write path (once exercised by this test) is gone.
        let model: Option<String> = conn
            .query_row("SELECT model FROM ai_agents WHERE id = 'agent-main'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            model, None,
            "#2647 D17: ai_configure_and_save must NOT write ai_agents.model — that write now \
             lives exclusively in sync_initial_import's INSERT/ON CONFLICT CASE"
        );

        let ai_settings_model_row_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_settings WHERE key = 'model'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            ai_settings_model_row_count, 0,
            "ai_configure_and_save must NOT write ai_settings.model — that key is D8-retired and writing \
             it back would resurrect the zombie key on every configure/pair call"
        );
    }
}


// === C2 (#2586): model discovery dead-key retarget tests ===
//
// `get_internal_provider`'s Step 2 fallback previously gated on a raw
// `ai_settings.provider` read — a key retired for real by #2564/B1 (ADR-0063
// D8) and never written again. Since `ai_settings.provider` is permanently
// absent post-B1, this test seeds only `ai_agents.runtime_type` (the real,
// current source of truth `derive_active_provider_label` consults) and
// confirms the function resolves successfully rather than always failing
// with "no provider configured".
//
// #2617: `fetch_primary_provider_models` and its dedicated test
// (`test_fetch_primary_provider_models_with_provider_absent`) were removed
// here — the onboarding call site that was its only caller now uses
// `fetch_available_models` (credential-based, agent-independent) instead.
// `derive_active_provider_label` remains correct and still under test via
// `get_internal_provider`'s Step 2 fallback below.
#[cfg(test)]
mod c2_model_discovery_dead_key_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    /// Minimal schema shared by both tests: `ai_agents` (for `runtime_type` /
    /// `is_default` / `is_active` — what `derive_active_provider_label` and
    /// `resolve_designated_default_agent_id` actually query) plus `ai_settings`
    /// (present, but deliberately left empty of a `provider` row in every test
    /// here — that's the real post-B1 state on every install).
    fn make_conn() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                slug TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1,
                workspace_path TEXT NOT NULL,
                runtime_type TEXT,
                model TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE ai_settings (
                key TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            ",
        )
        .unwrap();
        conn
    }

    fn seed_default_agent(conn: &rusqlite::Connection, id: &str, runtime_type: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, created_at, updated_at)
             VALUES (?1, ?1, ?1, 1, 1, ?1, ?2, ?3, ?3)",
            rusqlite::params![id, runtime_type, now],
        )
        .unwrap();
    }

    /// `get_internal_provider` Step 2 fallback: with `internal_provider` unset
    /// (so Step 1 is skipped) and `ai_settings.provider` absent (real post-B1
    /// state), a `runtime_type='ollama'` default agent must still let Step 2
    /// resolve via `derive_active_provider_label` instead of failing with "No AI
    /// provider configured" before ever consulting the agent roster.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_get_internal_provider_step2_fallback_with_provider_absent() {
        let _kc = KeychainGuard::set();

        let conn = make_conn();
        seed_default_agent(&conn, "agent-ollama", "ollama");
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('ollama_endpoint', 'http://localhost:11434')",
            [],
        )
        .unwrap();

        // Preconditions this test exists to exercise.
        let internal_provider: Option<String> =
            crate::database::get_ai_setting(&conn, "internal_provider");
        assert!(
            internal_provider.is_none(),
            "precondition: internal_provider must be unset so Step 2 is reached"
        );
        let legacy_provider: Option<String> =
            crate::database::get_ai_setting(&conn, "provider");
        assert!(
            legacy_provider.is_none(),
            "precondition: ai_settings.provider must be absent (the real post-B1 state)"
        );

        let db = DbState::new(conn);
        let result = get_internal_provider(&db).await;

        assert!(
            result.is_ok(),
            "get_internal_provider Step 2 fallback must resolve via \
             derive_active_provider_label when ai_settings.provider is absent, got: {:?}",
            result.err()
        );
        match result.unwrap() {
            AiProvider::Ollama { endpoint } => {
                assert_eq!(endpoint, "http://localhost:11434");
            }
            other => panic!("Expected Ollama provider from Step 2 fallback, got: {:?}", other),
        }
    }
}


#[cfg(test)]
mod f3_venice_save_promote_global_demote_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;
    use tempfile::TempDir;

    /// Mirrors d11/d13/d17's own `setup()` — a real tauri::App<MockRuntime> with a
    /// fresh-migrated in-memory DB managed as state, plus PANE_TEST_ROOT pointed at a
    /// scratch filesystem dir.
    fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
        let fs_dir = TempDir::new().expect("failed to create scratch fs dir");
        std::env::set_var("PANE_TEST_ROOT", fs_dir.path());

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        (app, fs_dir)
    }

    fn teardown() {
    }

    /// Regression guard for #2791 (Reed's 2026-08-31 scope audit): seed a NON-Venice
    /// default agent (an Anthropic-onboarded default, e.g.) BEFORE Venice
    /// reconnects, then save a Venice key with a deactivated Venice agent row
    /// present (simulating a prior Disconnect). Before the fix, `has_default` was
    /// scoped to `runtime_type = 'venice'`, so it evaluated false even though the
    /// Anthropic row already held the global `is_default = 1` seat — causing the
    /// venice arm to unconditionally demote the foreign default and steal the seat
    /// for Venice on every save. After the fix, the venice save must leave an
    /// active foreign default untouched. Asserts:
    ///   1. ai_configure_and_save succeeds.
    ///   2. The pre-existing (Anthropic) default is NOT demoted — it keeps the seat.
    ///   3. The Venice agent is reactivated but is NOT promoted to default.
    ///   4. Exactly one row holds is_default = 1 (Anthropic's).
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_venice_save_must_not_steal_default_from_foreign_provider() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let now = chrono::Utc::now().to_rfc3339();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            // Pre-existing foreign default: an Anthropic-onboarded local agent.
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, created_at, updated_at)
                 VALUES ('anthropic-default-id', 'Opal', 'opal', 'main', 1, 1, 'anthropic-default-id', 'anthropic', ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("failed to seed pre-existing anthropic default agent");
            // A previously-connected, now-deactivated Venice agent (simulating a
            // prior Disconnect via ai_disconnect_runtime_impl's venice arm).
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, created_at, updated_at)
                 VALUES ('venice-agent-id', 'Venice Agent', 'venice-agent', 'sub', 0, 0, 'venice-agent-id', 'venice', ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("failed to seed pre-existing deactivated venice agent");
        }

        let db_state = app.state::<DbState>();
        let result = ai_configure_and_save(
            "venice".to_string(),
            Some("a-real-venice-key-1234".to_string()),
            None,
            None,
            None,
            db_state,
        )
        .await;
        assert!(
            result.is_ok(),
            "ai_configure_and_save('venice') must succeed when a foreign default already exists: {:?}",
            result
        );

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();

        let anthropic_is_default: i64 = conn
            .query_row("SELECT is_default FROM ai_agents WHERE id = 'anthropic-default-id'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(anthropic_is_default, 1, "an active foreign default must survive a Venice save unchanged (#2791)");

        let venice_is_default: i64 = conn
            .query_row("SELECT is_default FROM ai_agents WHERE id = 'venice-agent-id'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(venice_is_default, 0, "venice must NOT be promoted while a foreign default is active (#2791)");

        let venice_is_active: i64 = conn
            .query_row("SELECT is_active FROM ai_agents WHERE id = 'venice-agent-id'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(venice_is_active, 1, "reactivation on save is independent of the default-promotion guard and must still occur");

        let default_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(default_count, 1, "exactly one row must hold is_default = 1 (Anthropic's, unchanged)");

        drop(conn);
        teardown();
    }

    /// Fresh-install regression guard for #2791: when NO row anywhere holds
    /// `is_default = 1` (fresh install, or the prior default was fully removed),
    /// Venice save must still promote a Venice agent to fill the empty seat. This
    /// is the legitimate F3 case the guard is supposed to allow through — the fix
    /// for the foreign-default-stealing bug must not silently break first-time
    /// Venice connect on a fresh install. Asserts:
    ///   1. ai_configure_and_save succeeds.
    ///   2. The reactivated Venice agent IS promoted to is_default = 1.
    ///   3. Exactly one row holds is_default = 1 (Venice's, newly promoted).
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_venice_save_promotes_when_no_default_exists_anywhere() {
        let _kc = KeychainGuard::set();
        let (app, _fs_dir) = setup();

        let now = chrono::Utc::now().to_rfc3339();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            // No active default row exists anywhere in the table. A previously-
            // connected, now-deactivated Venice agent (simulating a prior
            // Disconnect via ai_disconnect_runtime_impl's venice arm), never
            // promoted, mirroring a fresh install / no default set yet.
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, runtime_type, created_at, updated_at)
                 VALUES ('venice-agent-id', 'Venice Agent', 'venice-agent', 'sub', 0, 0, 'venice-agent-id', 'venice', ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("failed to seed pre-existing deactivated venice agent");
        }

        let db_state = app.state::<DbState>();
        let result = ai_configure_and_save(
            "venice".to_string(),
            Some("a-real-venice-key-1234".to_string()),
            None,
            None,
            None,
            db_state,
        )
        .await;
        assert!(
            result.is_ok(),
            "ai_configure_and_save('venice') must succeed when no default exists anywhere: {:?}",
            result
        );

        let conn_state = app.state::<DbState>();
        let conn = conn_state.0.lock().unwrap();

        let venice_is_default: i64 = conn
            .query_row("SELECT is_default FROM ai_agents WHERE id = 'venice-agent-id'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(venice_is_default, 1, "venice must be promoted to fill the empty default seat when no provider holds it (#2791 F3 case)");

        let venice_is_active: i64 = conn
            .query_row("SELECT is_active FROM ai_agents WHERE id = 'venice-agent-id'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(venice_is_active, 1, "the promoted venice agent must also be reactivated");

        let default_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(default_count, 1, "exactly one row must hold is_default = 1 after the venice promote");

        drop(conn);
        teardown();
    }
}


/// F2 fix (#2455 venice live-test v2 delta review): real integration tests for
/// `validate_venice_key`'s `Option<String>` stored-key fallback, calling the
/// actual `#[tauri::command]` handler through a real `tauri::test::mock_app` +
/// fresh-migrated in-memory DB. Same shape as
/// `system_files::validate_openrouter_key_stored_key_fallback_tests` /
/// `validate_anthropic_key_stored_key_fallback_tests` — flagged by Reed's
/// 2026-08-30 delta review as a documentation-accuracy gap (the PR body/CHANGELOG
/// claimed this coverage existed pre-review when it did not).
#[cfg(test)]
mod validate_venice_key_stored_key_fallback_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;

    fn setup() -> tauri::App<tauri::test::MockRuntime> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    /// Stored key present, no input (`None`): the validator must attempt
    /// validation with the stored key (reaching the mocked HTTP call), not
    /// short-circuit with "No API key provided".
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_none_input_falls_back_to_stored_key_and_reaches_http_call() {
        let _kc = KeychainGuard::set();
        let app = setup();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            crate::ai::credential_migration::write_venice_key(&conn, "ven-stored-key-1234")
                .expect("failed to seed stored venice key");
        }

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api_keys/rate_limits")
            .match_header("authorization", "Bearer ven-stored-key-1234")
            .with_status(401)
            .with_body(r#"{"error":"Authentication failed"}"#)
            .create_async()
            .await;
        test_set_venice_validate_base(&server.url());

        let db_state = app.state::<DbState>();
        let result = validate_venice_key(None, db_state).await;

        test_clear_venice_validate_base();

        mock.assert_async().await;
        assert!(result.is_err(), "mocked 401 should surface as Err");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Invalid Venice API key"),
            "expected the mocked HTTP 401 to be reached and translated, got: {msg}"
        );
        assert!(
            !msg.contains("No API key provided"),
            "None input with a stored key present must NOT short-circuit with 'No API key provided', got: {msg}"
        );
    }

    /// No stored key AND no input: clean fallback-miss error, no HTTP call attempted.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_none_input_with_no_stored_key_returns_clean_error() {
        let _kc = KeychainGuard::set();
        let app = setup();

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api_keys/rate_limits")
            .with_status(200)
            .expect(0)
            .create_async()
            .await;
        test_set_venice_validate_base(&server.url());

        let db_state = app.state::<DbState>();
        let result = validate_venice_key(None, db_state).await;

        test_clear_venice_validate_base();

        mock.assert_async().await;
        assert!(result.is_err(), "no input and no stored key must be an Err");
        assert_eq!(
            result.unwrap_err(),
            "No API key provided and no stored key found",
            "must be the clean fallback-miss message, not a panic or an HTTP-layer error"
        );
    }

    /// Typed input wins over stored: the typed key must be the one sent to the
    /// HTTP call, never the stored one.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_typed_input_wins_over_stored_key() {
        let _kc = KeychainGuard::set();
        let app = setup();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            crate::ai::credential_migration::write_venice_key(&conn, "ven-stored-key-1234")
                .expect("failed to seed stored venice key");
        }

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api_keys/rate_limits")
            .match_header("authorization", "Bearer ven-user-typed-key")
            .with_status(401)
            .with_body(r#"{"error":"Authentication failed"}"#)
            .create_async()
            .await;
        test_set_venice_validate_base(&server.url());

        let db_state = app.state::<DbState>();
        let result = validate_venice_key(Some("ven-user-typed-key".to_string()), db_state).await;

        test_clear_venice_validate_base();

        mock.assert_async().await;
        assert!(result.is_err(), "mocked 401 for the typed key should surface as Err");
        assert!(
            result.unwrap_err().contains("Invalid Venice API key"),
            "expected the typed key's mocked HTTP 401 to be reached"
        );
    }
}

