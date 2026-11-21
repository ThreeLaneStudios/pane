//! ADR-0063 Stage 2 — per-dispatch runtime resolution (#2522).
//!
//! Replaces the `AiConfig`/`AiStateWrapper` singleton as the source of
//! provider/model for AI dispatch. The chain this module implements:
//!
//!   session_id → chat_sessions.agent_id → ai_agents.runtime_type/model
//!     → AiProvider (built from global connection config: ai_settings +
//!       keychain, same credential storage `lib.rs`'s old `.setup()` block
//!       used — per-runtime credential storage is a Stage 5 concern, not
//!       yet built)
//!
//! Every dispatch-path and background-command call site that used to read
//! `ai_state.0.lock()` now calls one of the three entry points below:
//!   - `resolve_runtime_config_for_session` — the normal chat-dispatch path,
//!     keyed on the caller's session id (self-heals a NULL `agent_id`
//!     binding via `agents::resolve_and_selfheal_session_agent`).
//!   - `resolve_runtime_config_for_default_agent` — sessionless background
//!     commands (memory save/synthesis/consolidation, consolidation thread)
//!     that have no session to resolve from; uses the designated default
//!     agent per D5 ("session agent where derivable, designated default
//!     otherwise").
//!   - `resolve_runtime_config_for_agent_id` — shared plumbing both of the
//!     above call once they have a concrete agent id; also usable directly
//!     by any future caller that already knows which agent it wants.
//!
//! All three return `RuntimeConfig`, a small per-dispatch value (not a
//! singleton, not cached, not shared across requests) — it is resolved
//! fresh from the DB on every call, so there is no "stale state" class of
//! bug the old wrapper was exposed to.

use crate::ai::provider::AiProvider;
use rusqlite::{Connection, OptionalExtension};

/// The resolved runtime + model for a single dispatch. Not cached, not a
/// singleton — built fresh from the DB (and keychain) on every call.
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Pane's LOCAL `ai_agents.id` (UUID). Load-bearing for stream-done event
    /// attribution and the [DISPATCH] log — never a wire value (ADR-0067 D2).
    pub agent_id: String,
    /// `ai_agents.runtime_ref` — the OC-side agent selector (e.g. "main",
    /// "researcher") used to build the `openclaw/<runtime_ref>` wire model string.
    /// `None` for local/non-OC agents and legacy OC rows with no runtime_ref yet.
    /// ADR-0067 D2: this field, never `agent_id`, is what may reach the wire.
    pub runtime_ref: Option<String>,
    pub runtime_type: String,
    pub provider: AiProvider,
    pub model: String,
}

/// Normalize an OpenAI-compatible-style API endpoint to end in `/v1`.
/// Mirrors `commands.rs::normalize_api_endpoint` (kept private there —
/// duplicated here rather than widening that function's visibility, since
/// the two call sites have no other reason to share a module boundary).
fn normalize_api_endpoint(url: &str) -> String {
    let url = url.trim_end_matches('/');
    if url.ends_with("/v1") || url.contains("/v1/") {
        url.to_string()
    } else {
        format!("{}/v1", url)
    }
}

/// Build an `AiProvider` for a given `runtime_type`, using the existing
/// global connection config (ai_settings `endpoint`, keychain credentials).
/// Stage 2 scope note: per-runtime credential storage (one keychain entry
/// per connected runtime, rather than one global slot) is Stage 5 build
/// scope — this reuses the same global `ai_settings`/keychain reads
/// `lib.rs`'s retired `.setup()` block used to perform once at startup.
pub(crate) fn build_provider_for_runtime_type(
    conn: &Connection,
    runtime_type: &str,
    // ADR-0067 D2/D5: `ai_agents.runtime_ref` for the specific agent this provider is
    // being built for, if known. `None` for generic per-runtime-type connectivity
    // checks (Settings runtime cards) that aren't resolving for a specific agent.
    runtime_ref: Option<&str>,
) -> Result<AiProvider, String> {
    let snapshot = RuntimeSettingsSnapshot::capture(conn);
    build_provider_for_runtime_type_from_snapshot(&snapshot, runtime_type, runtime_ref)
}

/// D26 (#2673 PR-A): the `ai_settings`/migration-flag inputs
/// `build_provider_for_runtime_type` needs, captured once while `db.0` is still
/// locked so the actual keychain reads (`read_provider_key_from_snapshot` /
/// `read_gateway_token_from_snapshot`) can run afterward with no live
/// `&Connection` in scope. See docs/spec/ai-integration.md §28.4.
#[derive(Debug, Clone)]
pub struct RuntimeSettingsSnapshot {
    credentials: crate::ai::credential_migration::CredentialDbSnapshot,
    ollama_endpoint: Option<String>,
    openclaw_endpoint: Option<String>,
    venice_system_prompt_enabled: bool,
}

impl RuntimeSettingsSnapshot {
    /// Capture every DB-derived input `build_provider_for_runtime_type` needs,
    /// across all four runtime types, in one pass. Call this while `db.0` is
    /// still locked; the returned snapshot has no connection borrow and can
    /// safely outlive the lock.
    pub fn capture(conn: &Connection) -> Self {
        Self {
            credentials: crate::ai::credential_migration::CredentialDbSnapshot::capture(conn),
            ollama_endpoint: crate::database::get_ai_setting(conn, "ollama_endpoint"),
            openclaw_endpoint: crate::database::get_ai_setting(conn, "endpoint"),
            venice_system_prompt_enabled: crate::database::get_ai_setting(conn, "venice_system_prompt_enabled")
                .map(|v| v == "1")
                .unwrap_or(false),
        }
    }

    /// Access the inner `CredentialDbSnapshot` — callers that need a raw
    /// `read_provider_key_from_snapshot`/`read_gateway_token_from_snapshot` call (e.g.
    /// for the masked-credential display, which is orthogonal to whether a full
    /// provider builds successfully) use this instead of re-capturing.
    pub fn credentials(&self) -> &crate::ai::credential_migration::CredentialDbSnapshot {
        &self.credentials
    }
}

/// Keychain-only counterpart to `build_provider_for_runtime_type` (D26, #2673
/// PR-A): takes a pre-captured `RuntimeSettingsSnapshot` instead of a live
/// `&Connection`, so it can run after `db.0` has been released (e.g. inside
/// `tokio::task::spawn_blocking` + `tokio::time::timeout`, mirroring the #1347
/// precedent at `sync/commands.rs:324-333`). Identical logic/error messages to
/// `build_provider_for_runtime_type` — see its doc comment for the full
/// per-runtime-type contract.
pub(crate) fn build_provider_for_runtime_type_from_snapshot(
    snapshot: &RuntimeSettingsSnapshot,
    runtime_type: &str,
    // ADR-0067 D2/D5: see `build_provider_for_runtime_type`'s doc comment on the
    // same parameter. Only read by the "openclaw" arm below.
    runtime_ref: Option<&str>,
) -> Result<AiProvider, String> {
    match runtime_type {
        "anthropic" => {
            let api_key = crate::ai::credential_migration::read_provider_key_from_snapshot(
                &snapshot.credentials,
                "anthropic",
            )
            .ok_or("No Anthropic API key configured. Set it up in Settings → AI.")?;
            Ok(AiProvider::Anthropic { api_key })
        }
        "openrouter" => {
            let api_key = crate::ai::credential_migration::read_provider_key_from_snapshot(
                &snapshot.credentials,
                "openrouter",
            )
            .ok_or("No OpenRouter API key configured. Set it up in Settings → AI.")?;
            Ok(AiProvider::OpenRouter { api_key })
        }
        "ollama" => {
            // B3 (#2565): explicit-connection semantics — Ollama's "Connected" state
            // now requires the user to have gone through the Save flow at least once
            // (which writes `ollama_endpoint`), not merely a live reachability probe.
            // See ai_configure_and_save / ai_disconnect_runtime_impl (commands.rs) for
            // the write/clear side of this signal, and migrate_v82 (migrations.rs) for
            // the D8 key-split + grandfather backfill for pre-existing ollama agents.
            match snapshot.ollama_endpoint.clone() {
                Some(endpoint) => Ok(AiProvider::Ollama { endpoint }),
                None => Err("No Ollama connection configured".to_string()),
            }
        }
        "openclaw" => {
            let token = crate::ai::credential_migration::read_gateway_token_from_snapshot(
                &snapshot.credentials,
            )
            .ok_or("No OpenClaw gateway token configured. Pair with your gateway in Settings → AI.")?;
            // #2564 B1 fix / #2568 closure: was `.unwrap_or_else(|| "http://127.0.0.1:18789")`.
            // v79's D8 dissolution (ADR-0063) deleted `ai_settings.endpoint` along with
            // `provider` with no per-agent home for it — any install that migrated through
            // v79 without the (separate) v80 backup-recovery running is left with a truly
            // orphaned openclaw agent: paired, but with no known gateway URL. Silently
            // falling back to localhost made the OC card show a false "Connected" and sent
            // the consolidator/sync client silently at 127.0.0.1 instead of the real
            // gateway. Missing endpoint is now a hard error, exactly like a missing token
            // above — one change closes both failure modes (#2568).
            let raw_endpoint = snapshot.openclaw_endpoint.clone().ok_or(
                "No gateway endpoint configured. Re-pair required in Settings → AI.",
            )?;
            Ok(AiProvider::OpenClaw {
                base_url: normalize_api_endpoint(&raw_endpoint),
                token,
                // ADR-0067 D2/D5: carry the wire selector on the provider itself —
                // never the agent's local id.
                runtime_ref: runtime_ref.map(|r| r.to_string()),
            })
        }
        "venice" => {
            // ADR-0065 D4: Venice key is keychain-only (account `ai_venice_key`).
            // `read_venice_key_from_snapshot` returns None on keychain miss — no
            // plaintext fallback, matching read_brave_key's posture.
            let api_key = crate::ai::credential_migration::read_venice_key_from_snapshot(
                &snapshot.credentials,
            )
            .ok_or("No Venice API key configured. Set it up in Settings → AI.")?;
            Ok(AiProvider::Venice {
                api_key,
                // ADR-0065 D3: `include_system_prompt` defaults to false (off).
                include_system_prompt: snapshot.venice_system_prompt_enabled,
            })
        }
        other => Err(format!(
            "Agent has an unrecognized runtime_type '{}'. This is a data integrity issue — \
             contact support or reconfigure the agent.",
            other
        )),
    }
}

/// Resolve the full runtime config for a specific agent id.
///
/// Shared plumbing for both session-based and default-agent resolution
/// below. Returns a clear onboarding-style error (not a panic, not a silent
/// `AiProvider::None`) when:
///   - the agent id doesn't exist,
///   - the agent's `runtime_type` is NULL (D4.3 legacy-unpinned row that
///     hasn't been through the one-time runtime picker yet — Stage 7 build
///     scope; Stage 2 surfaces this as an explicit error rather than
///     guessing a runtime),
///   - the resolved runtime's credentials aren't configured.
pub fn resolve_runtime_config_for_agent_id(
    conn: &Connection,
    agent_id: &str,
) -> Result<RuntimeConfig, String> {
    let row: Option<(Option<String>, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT runtime_type, model, runtime_ref FROM ai_agents WHERE id = ?1",
            rusqlite::params![agent_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("Failed to look up agent {}: {}", agent_id, e))?;

    let Some((runtime_type_opt, model_opt, runtime_ref)) = row else {
        return Err(format!("Agent {} not found.", agent_id));
    };

    let runtime_type = runtime_type_opt.ok_or_else(|| {
        format!(
            "Agent {} has no runtime configured yet. This agent predates per-agent runtime \
             assignment — pick a runtime for it in Settings → AI to continue.",
            agent_id
        )
    })?;

    let model = model_opt.unwrap_or_default();
    let provider = build_provider_for_runtime_type(conn, &runtime_type, runtime_ref.as_deref())?;

    Ok(RuntimeConfig {
        agent_id: agent_id.to_string(),
        runtime_ref,
        runtime_type,
        provider,
        model,
    })
}

/// Resolve the runtime config for a chat session: `session_id` →
/// `chat_sessions.agent_id` (self-healing a NULL binding to the designated
/// default per ADR-0062 D3.4) → `ai_agents.runtime_type`/`model`.
///
/// This is the dispatch-hot-path entry point — `ai_chat_stream` calls this
/// instead of locking the old `AiStateWrapper` singleton.
///
/// Guard order matters: `check_bound_agent_active` runs first, against the
/// session's *existing* (possibly non-NULL) `agent_id`, before self-heal gets
/// a chance to run. This is what makes an `is_active = 0` binding refuse to
/// dispatch (explicit error naming the agent) rather than either self-healing
/// (which only triggers on NULL, so it wouldn't fire here anyway) or silently
/// resolving through the stale, deactivated agent. A genuinely NULL `agent_id`
/// (legacy migration backfill) is a no-op for the guard and falls through to
/// self-heal exactly as before.
pub fn resolve_runtime_config_for_session(
    conn: &Connection,
    session_id: &str,
) -> Result<RuntimeConfig, String> {
    crate::agents::check_bound_agent_active(conn, session_id)?;

    let agent_id = crate::agents::resolve_and_selfheal_session_agent(conn, session_id)?
        .ok_or_else(|| {
            "No agent is set up yet — finish onboarding or create an agent in Settings → AI \
             before starting a chat."
                .to_string()
        })?;

    resolve_runtime_config_for_agent_id(conn, &agent_id)
}

/// Resolve the runtime config for sessionless background work (memory save,
/// memory synthesis/consolidation, the consolidation thread). Per D5 ("session
/// agent where derivable, designated default otherwise"), these callers have
/// no session to derive from, so they always use the designated default agent.
pub fn resolve_runtime_config_for_default_agent(conn: &Connection) -> Result<RuntimeConfig, String> {
    let agent_id = crate::agents::resolve_designated_default_agent_id(conn)?
        .ok_or_else(|| {
            "No default agent is configured yet — finish onboarding or create an agent in \
             Settings → AI."
                .to_string()
        })?;

    resolve_runtime_config_for_agent_id(conn, &agent_id)
}

/// #2564 B1 fix (Path B), item #12 — the load-bearing re-point of the whole Path B
/// plan. Derives a single "active provider" label for legacy frontend surfaces that
/// still expect a scalar `provider` string (`ai_load_config` and everything that
/// transitively consumes it: ChatPanel, Settings, SessionDropdownHeader,
/// ProjectSettings, SessionsDrawer). `ai_settings.provider` is retired (ADR-0063 D8)
/// — this replaces it with a derived value:
///   1. If any active `ai_agents` row has `runtime_type = 'openclaw'`, report
///      `"openclaw"` (OC's presence takes priority — matches the pre-dissolution
///      convention where pairing an OC gateway made it "the" provider).
///   2. Otherwise, report the designated default agent's `runtime_type`, if one
///      exists and is non-NULL.
///   3. Otherwise (`no agents, or default agent is legacy-unpinned`), report
///      `"none"` — matches the old `unwrap_or("none")` fallback shape so downstream
///      "no provider configured" UI checks keep working unmodified.
pub fn derive_active_provider_label(conn: &Connection) -> String {
    if crate::sync_client::has_active_openclaw_agent(conn) {
        return "openclaw".to_string();
    }

    let default_agent_id = match crate::agents::resolve_designated_default_agent_id(conn) {
        Ok(id) => id,
        Err(e) => {
            eprintln!(
                "[RUNTIME-RESOLUTION] derive_active_provider_label: failed to resolve default agent: {}",
                e
            );
            return "none".to_string();
        }
    };

    let Some(agent_id) = default_agent_id else {
        return "none".to_string();
    };

    let runtime_type: Option<String> = conn
        .query_row(
            "SELECT runtime_type FROM ai_agents WHERE id = ?1",
            rusqlite::params![agent_id],
            |row| row.get(0),
        )
        .optional()
        .unwrap_or(None)
        .flatten();

    runtime_type.unwrap_or_else(|| "none".to_string())
}

/// Write the resolved model back to the answering agent's `ai_agents.model`
/// column, write-on-change only (ADR-0063 D9). Replaces the old
/// `set_ai_setting(&conn, "model", ...)` + in-memory `AiStateWrapper` mutation
/// — the write target is now the specific agent row, not a global key, and
/// there is no in-memory singleton left to sync.
pub fn write_back_agent_model_if_changed(
    conn: &Connection,
    agent_id: &str,
    resolved_model: &str,
) -> Result<bool, String> {
    if resolved_model.is_empty() {
        return Ok(false);
    }

    let current_model: Option<String> = conn
        .query_row(
            "SELECT model FROM ai_agents WHERE id = ?1",
            rusqlite::params![agent_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read current agent model: {}", e))?
        .flatten();

    if current_model.as_deref() == Some(resolved_model) {
        return Ok(false);
    }

    conn.execute(
        "UPDATE ai_agents SET model = ?1, updated_at = ?2 WHERE id = ?3",
        rusqlite::params![resolved_model, chrono::Utc::now().to_rfc3339(), agent_id],
    )
    .map_err(|e| format!("Failed to persist agent model update: {}", e))?;

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn make_test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                slug            TEXT NOT NULL UNIQUE,
                role            TEXT NOT NULL DEFAULT 'sub',
                description     TEXT,
                model           TEXT,
                is_default      INTEGER NOT NULL DEFAULT 0,
                is_active       INTEGER NOT NULL DEFAULT 1,
                workspace_path  TEXT NOT NULL,
                soul_hash       TEXT,
                memory_hash     TEXT,
                config_json     TEXT NOT NULL DEFAULT '{}',
                owner_id        TEXT,
                runtime_ref     TEXT,
                runtime_type    TEXT,
                source          TEXT NOT NULL DEFAULT 'manual',
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL
            );
            CREATE TABLE chat_sessions (
                id          TEXT PRIMARY KEY,
                project_id  TEXT,
                agent_id    TEXT REFERENCES ai_agents(id),
                is_active   INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            ",
        )
        .expect("setup schema");
        conn
    }

    fn seed_agent(
        conn: &Connection,
        id: &str,
        runtime_type: Option<&str>,
        model: Option<&str>,
        is_default: bool,
    ) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, config_json, source, runtime_type, model, created_at, updated_at)
             VALUES (?1, ?1, ?1, ?2, 1, ?1, '{}', 'manual', ?3, ?4, ?5, ?5)",
            params![id, if is_default { 1 } else { 0 }, runtime_type, model, now],
        )
        .expect("seed agent");
    }

    fn seed_session(conn: &Connection, id: &str, agent_id: Option<&str>) {
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, agent_id, is_active) VALUES (?1, 'workspace', ?2, 1)",
            params![id, agent_id],
        )
        .expect("seed session");
    }

    /// Like `seed_agent`, but also sets `runtime_ref` — needed by the
    /// ADR-0067 RuntimeConfig.runtime_ref tests below. Kept separate from
    /// `seed_agent` rather than widening its signature, since the overwhelming
    /// majority of existing tests deliberately don't care about runtime_ref
    /// (it should default to SQL NULL for them, exercising the None path).
    fn seed_agent_with_runtime_ref(
        conn: &Connection,
        id: &str,
        runtime_type: Option<&str>,
        model: Option<&str>,
        is_default: bool,
        runtime_ref: Option<&str>,
    ) {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, config_json, source, runtime_type, model, runtime_ref, created_at, updated_at)
             VALUES (?1, ?1, ?1, ?2, 1, ?1, '{}', 'manual', ?3, ?4, ?5, ?6, ?6)",
            params![id, if is_default { 1 } else { 0 }, runtime_type, model, runtime_ref, now],
        )
        .expect("seed agent with runtime_ref");
    }

    #[test]
    fn test_resolve_by_agent_id_happy_path_openrouter() {
        let conn = make_test_conn();
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('provider_api_key_plaintext_test_only', 'sk-test')",
            [],
        )
        .ok();
        // Use the keychain-migration-aware read: fall back path is tested via
        // the credential_migration module's own tests. Here we only need to
        // confirm the resolver surfaces a clear error when no key exists.
        seed_agent(&conn, "agent-1", Some("openrouter"), Some("anthropic/claude-3"), true); // CI-ALLOW: hardcoded-model

        let err = resolve_runtime_config_for_agent_id(&conn, "agent-1")
            .expect_err("no keychain credential configured in this in-memory test env");
        assert!(
            err.contains("OpenRouter API key"),
            "error should name the missing credential: {}",
            err
        );
    }

    #[test]
    fn test_resolve_by_agent_id_missing_agent() {
        let conn = make_test_conn();
        let err = resolve_runtime_config_for_agent_id(&conn, "does-not-exist")
            .expect_err("missing agent should error");
        assert!(err.contains("not found"), "error should say not found: {}", err);
    }

    #[test]
    fn test_resolve_by_agent_id_null_runtime_type() {
        let conn = make_test_conn();
        seed_agent(&conn, "legacy-agent", None, Some("some-model"), true);
        let err = resolve_runtime_config_for_agent_id(&conn, "legacy-agent")
            .expect_err("NULL runtime_type should error, not guess");
        assert!(
            err.contains("no runtime configured"),
            "error should explain the legacy-unpinned state: {}",
            err
        );
    }

    #[test]
    fn test_resolve_by_agent_id_unknown_runtime_type() {
        let conn = make_test_conn();
        seed_agent(&conn, "weird-agent", Some("carrier-pigeon"), Some("m"), true);
        let err = resolve_runtime_config_for_agent_id(&conn, "weird-agent")
            .expect_err("unrecognized runtime_type should error");
        assert!(err.contains("unrecognized runtime_type"), "error: {}", err);
    }

    #[test]
    fn test_resolve_for_session_no_agent_configured() {
        let conn = make_test_conn();
        seed_session(&conn, "session-1", None);
        let err = resolve_runtime_config_for_session(&conn, "session-1")
            .expect_err("empty ai_agents table should produce a clear onboarding error");
        assert!(
            err.contains("No agent is set up"),
            "error should be onboarding-shaped: {}",
            err
        );
    }

    #[test]
    fn test_resolve_for_session_self_heals_and_resolves() {
        let conn = make_test_conn();
        seed_agent(&conn, "default-agent", Some("ollama"), Some("llama3"), true);
        seed_session(&conn, "session-1", None);
        // B3 (#2565) Req 2: ollama now requires an explicit ollama_endpoint write —
        // no more "needs no credentials" free pass.
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();

        let cfg = resolve_runtime_config_for_session(&conn, "session-1")
            .expect("ollama_endpoint is configured, should resolve");
        assert_eq!(cfg.agent_id, "default-agent");
        assert_eq!(cfg.runtime_type, "ollama");
        assert_eq!(cfg.model, "llama3");
        assert!(matches!(cfg.provider, AiProvider::Ollama { .. }));
        assert_eq!(cfg.runtime_ref, None, "seed_agent never sets runtime_ref; must resolve to None, not panic");

        // Confirm the self-heal actually wrote the binding.
        let bound: Option<String> = conn
            .query_row(
                "SELECT agent_id FROM chat_sessions WHERE id = 'session-1'",
                [],
                |row| row.get(0),
            )
            .expect("query should succeed");
        assert_eq!(bound.as_deref(), Some("default-agent"));
    }

    #[test]
    fn test_resolve_for_session_uses_bound_agent_not_default() {
        let conn = make_test_conn();
        seed_agent(&conn, "default-agent", Some("ollama"), Some("llama3"), true);
        seed_agent(&conn, "other-agent", Some("ollama"), Some("mistral"), false);
        seed_session(&conn, "session-1", Some("other-agent"));
        // B3 (#2565) Req 2: ollama now requires an explicit ollama_endpoint write.
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();

        let cfg = resolve_runtime_config_for_session(&conn, "session-1")
            .expect("should resolve via the session's own binding");
        assert_eq!(
            cfg.agent_id, "other-agent",
            "must use the session's bound agent, not the designated default"
        );
        assert_eq!(cfg.model, "mistral");
    }

    /// D21 (#2649): a session bound to a deactivated agent (`is_active = 0`) must
    /// refuse to dispatch with an explicit lock error naming the agent — never a
    /// generic credential error, and never a silent self-heal/rebind to a
    /// different (e.g. the designated default) agent. Per docs/spec/sessions.md's
    /// "Unavailable bound agent" rule (ADR-0062 D5).
    #[test]
    fn test_resolve_for_session_errors_when_bound_agent_is_inactive() {
        let conn = make_test_conn();
        seed_agent(&conn, "default-agent", Some("ollama"), Some("llama3"), true);
        seed_agent(&conn, "locked-agent", Some("openclaw"), Some("claude"), false); // CI-ALLOW: hardcoded-model
        seed_session(&conn, "session-1", Some("locked-agent"));
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();

        // Deactivate the bound agent, e.g. via a disconnect (D21: disconnect no
        // longer NULLs the binding, so the session stays pointed at this row).
        conn.execute(
            "UPDATE ai_agents SET is_active = 0 WHERE id = 'locked-agent'",
            [],
        )
        .expect("deactivate locked-agent");

        let err = resolve_runtime_config_for_session(&conn, "session-1")
            .expect_err("dispatch against a session bound to an inactive agent must error");
        assert!(
            err.contains("locked-agent") && err.contains("inactive"),
            "error must explicitly name the bound agent and state it's inactive, got: {}",
            err
        );

        // The binding itself must be untouched — no self-heal, no rebind.
        let bound: Option<String> = conn
            .query_row(
                "SELECT agent_id FROM chat_sessions WHERE id = 'session-1'",
                [],
                |row| row.get(0),
            )
            .expect("query should succeed");
        assert_eq!(
            bound.as_deref(),
            Some("locked-agent"),
            "agent_id must remain pointed at the deactivated agent — no silent rebind to the default"
        );
    }

    /// D21 (#2649): the NULL self-heal path (for legacy/migration-backfill rows)
    /// must keep working unchanged — the new is_active guard only fires on a
    /// non-NULL agent_id pointing at an inactive row, never on NULL.
    #[test]
    fn test_resolve_for_session_null_agent_id_still_self_heals_despite_guard() {
        let conn = make_test_conn();
        seed_agent(&conn, "default-agent", Some("ollama"), Some("llama3"), true);
        seed_session(&conn, "session-1", None);
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();

        let cfg = resolve_runtime_config_for_session(&conn, "session-1")
            .expect("NULL agent_id must still self-heal to the designated default, guard must not block it");
        assert_eq!(cfg.agent_id, "default-agent");
    }

    #[test]
    fn test_resolve_for_default_agent_no_agents() {
        let conn = make_test_conn();
        let err = resolve_runtime_config_for_default_agent(&conn)
            .expect_err("empty ai_agents table should error clearly");
        assert!(err.contains("No default agent"), "error: {}", err);
    }

    #[test]
    fn test_resolve_for_default_agent_happy_path() {
        let conn = make_test_conn();
        seed_agent(&conn, "default-agent", Some("ollama"), Some("llama3"), true);
        // B3 (#2565) Req 2: ollama now requires an explicit ollama_endpoint write.
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();
        let cfg = resolve_runtime_config_for_default_agent(&conn)
            .expect("should resolve the designated default");
        assert_eq!(cfg.agent_id, "default-agent");
    }

    /// ADR-0067 Part 1: `RuntimeConfig.runtime_ref` must be populated from
    /// `ai_agents.runtime_ref` — the field the wire-stamp fix (Part 2) depends on
    /// entirely. Prior to this fix, the resolver's SQL didn't select the column at
    /// all, so this field could never carry a real value regardless of DB state.
    #[test]
    fn test_resolve_by_agent_id_returns_runtime_ref_when_present() {
        let conn = make_test_conn();
        seed_agent_with_runtime_ref(
            &conn,
            "oc-agent",
            Some("openclaw"),
            Some("openclaw"),
            true,
            Some("researcher"),
        );
        crate::ai::credential_migration::write_gateway_token(&conn, "tok").ok();
        crate::database::set_ai_setting(&conn, "endpoint", "http://127.0.0.1:18789").ok();

        let cfg = resolve_runtime_config_for_agent_id(&conn, "oc-agent")
            .expect("openclaw agent with token+endpoint should resolve");
        assert_eq!(
            cfg.runtime_ref,
            Some("researcher".to_string()),
            "runtime_ref must be selected and returned, not silently dropped"
        );
        assert_eq!(cfg.agent_id, "oc-agent", "agent_id (local UUID/id) must remain separate from runtime_ref");
    }

    /// The NULL case — local/non-OC agents and legacy OC rows never get a
    /// runtime_ref (agents.rs, confirmed in the app-half scope doc §1.3). Must
    /// resolve to `None`, never panic, never coerce to an empty string or the
    /// agent's local id.
    #[test]
    fn test_resolve_by_agent_id_runtime_ref_is_none_when_null() {
        let conn = make_test_conn();
        seed_agent(&conn, "local-agent", Some("ollama"), Some("llama3"), true);
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();

        let cfg = resolve_runtime_config_for_agent_id(&conn, "local-agent")
            .expect("should resolve");
        assert_eq!(cfg.runtime_ref, None, "NULL runtime_ref column must resolve to Option::None");
        assert_ne!(
            cfg.runtime_ref.as_deref(),
            Some(cfg.agent_id.as_str()),
            "runtime_ref must never be silently backfilled with agent_id (ADR-0067 D2 leak guard)"
        );
    }

    /// `resolve_runtime_config_for_default_agent` delegates to
    /// `resolve_runtime_config_for_agent_id` — confirms runtime_ref survives that
    /// delegation for the default-agent resolution path specifically, since that's
    /// the path 6 of Part 2's dispatch sites actually call.
    #[test]
    fn test_resolve_for_default_agent_returns_runtime_ref() {
        let conn = make_test_conn();
        seed_agent_with_runtime_ref(
            &conn,
            "oc-default",
            Some("openclaw"),
            Some("openclaw"),
            true,
            Some("main"),
        );
        crate::ai::credential_migration::write_gateway_token(&conn, "tok").ok();
        crate::database::set_ai_setting(&conn, "endpoint", "http://127.0.0.1:18789").ok();

        let cfg = resolve_runtime_config_for_default_agent(&conn)
            .expect("should resolve the designated default");
        assert_eq!(cfg.runtime_ref, Some("main".to_string()));
    }

    #[test]
    fn test_write_back_agent_model_if_changed_writes_on_diff() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("old-model"), true);

        let changed = write_back_agent_model_if_changed(&conn, "agent-1", "new-model")
            .expect("write should succeed");
        assert!(changed, "should report a change occurred");

        let stored: String = conn
            .query_row("SELECT model FROM ai_agents WHERE id = 'agent-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, "new-model");
    }

    #[test]
    fn test_write_back_agent_model_if_changed_noop_when_same() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("same-model"), true);

        let changed = write_back_agent_model_if_changed(&conn, "agent-1", "same-model")
            .expect("write should succeed");
        assert!(!changed, "should report no change when model is identical");
    }

    /// Item 9 (Reed's scope doc, #2585/C1): explicit regression test using the
    /// exact real-world transition `migrate_v83`'s placeholder relies on —
    /// `'openclaw-pending'` as the OLD value, a real resolved model as the NEW
    /// value. D9 write-back is value-agnostic (it only compares strings), so this
    /// should already pass unmodified against the generic diff test above — this
    /// test exists to pin that specific transition down by name, since it's the
    /// one this migration's entire safety argument depends on: the placeholder
    /// must always get corrected to a real model on the agent's first successful
    /// dispatch.
    #[test]
    fn test_write_back_agent_model_if_changed_corrects_openclaw_pending_sentinel() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("openclaw-pending"), true);

        let changed = write_back_agent_model_if_changed(&conn, "agent-1", "anthropic/claude-sonnet-4-6") // CI-ALLOW: hardcoded-model
            .expect("write should succeed");
        assert!(
            changed,
            "D9 must report a change when correcting the migrate_v83 sentinel to a real model"
        );

        let stored: String = conn
            .query_row("SELECT model FROM ai_agents WHERE id = 'agent-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            stored, "anthropic/claude-sonnet-4-6", // CI-ALLOW: hardcoded-model
            "the 'openclaw-pending' placeholder must be fully replaced by the real resolved \
             model on first successful dispatch — this is the exact self-heal migrate_v83's \
             migration comment promises"
        );
        assert_ne!(
            stored, "openclaw-pending",
            "the sentinel must never survive a successful write-back"
        );
    }

    /// ADR-0063 Stage 3+4 (#2522) — dual-provider concurrency harness (backend half).
    ///
    /// This is new test infrastructure: prior to Stage 3, the suite never asserted
    /// that two agents on *different* `runtime_type`s resolve independently with no
    /// shared mutable state. Post-Stage 2, `resolve_runtime_config_for_session` reads
    /// fresh from the DB on every call (no `AiConfig` singleton left to share) — this
    /// test proves that property holds when two different sessions, bound to two
    /// different agents on two different runtimes, resolve back-to-back (and
    /// interleaved) without either resolution observing or clobbering the other's
    /// result. It is the backend counterpart to the frontend dual-stream harness in
    /// `streamManager.test.ts`.
    #[test]
    fn test_two_concurrent_runtimes_resolve_independently_no_shared_state() {
        let conn = make_test_conn();
        // Two agents, two distinct runtime_types, two distinct models — deliberately
        // chosen so any cross-contamination (agent-a's config leaking into agent-b's
        // resolution, or vice versa) is immediately visible in the asserted fields.
        seed_agent(&conn, "agent-a", Some("ollama"), Some("llama3"), true);
        seed_agent(&conn, "agent-b", Some("anthropic"), Some("claude-3-opus"), false); // CI-ALLOW: hardcoded-model
        // B3 (#2565) Req 2: ollama now requires an explicit ollama_endpoint write.
        crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434").unwrap();

        seed_session(&conn, "session-a", Some("agent-a"));
        seed_session(&conn, "session-b", Some("agent-b"));

        // Resolve session-a first, session-b second — sequential baseline.
        let cfg_a = resolve_runtime_config_for_session(&conn, "session-a")
            .expect("agent-a (ollama) has its ollama_endpoint configured, should resolve");
        assert_eq!(cfg_a.agent_id, "agent-a");
        assert_eq!(cfg_a.runtime_type, "ollama");
        assert_eq!(cfg_a.model, "llama3");
        assert!(matches!(cfg_a.provider, AiProvider::Ollama { .. }));

        // session-b resolves independently — anthropic requires a credential, which
        // this in-memory test env doesn't have, so we expect a clear credential error
        // (not a silent fallback to agent-a's already-resolved ollama config, which is
        // exactly the class of bug a shared-state singleton would produce: if the two
        // resolutions shared any mutable slot, this call could incorrectly return
        // Ollama instead of erroring on the missing Anthropic key).
        let err_b = resolve_runtime_config_for_session(&conn, "session-b")
            .expect_err("agent-b (anthropic) has no keychain credential in this test env");
        assert!(
            err_b.contains("Anthropic API key"),
            "session-b's error must name its OWN runtime's missing credential, not agent-a's: {}",
            err_b
        );

        // Re-resolve session-a AFTER session-b's attempt — proves session-b's failed
        // resolution (and its distinct runtime_type/agent_id) left no residue that
        // would alter session-a's independently-resolved result on a second call.
        let cfg_a_again = resolve_runtime_config_for_session(&conn, "session-a")
            .expect("agent-a should resolve identically on a second, later call");
        assert_eq!(cfg_a_again.agent_id, cfg_a.agent_id);
        assert_eq!(cfg_a_again.runtime_type, cfg_a.runtime_type);
        assert_eq!(cfg_a_again.model, cfg_a.model);

        // Interleaved order (b, a, b, a) — asserts resolution is stateless with
        // respect to call order, which is the property concurrent tokio::spawn
        // dispatch actually depends on (two spawns interleave in ways sequential
        // test code can only approximate, but call-order independence is the
        // necessary condition for that to be safe).
        for _ in 0..2 {
            let a = resolve_runtime_config_for_agent_id(&conn, "agent-a")
                .expect("agent-a resolves regardless of interleaving");
            assert_eq!(a.runtime_type, "ollama");
            let b_err = resolve_runtime_config_for_agent_id(&conn, "agent-b")
                .expect_err("agent-b still errors on its own missing credential");
            assert!(b_err.contains("Anthropic API key"));
        }
    }

    #[test]
    fn test_write_back_agent_model_if_changed_ignores_empty() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-1", Some("openclaw"), Some("old-model"), true);

        let changed = write_back_agent_model_if_changed(&conn, "agent-1", "")
            .expect("write should succeed");
        assert!(!changed, "empty resolved model must never overwrite an existing value");

        let stored: String = conn
            .query_row("SELECT model FROM ai_agents WHERE id = 'agent-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stored, "old-model", "model must be unchanged");
    }

    // ── #2564 B1 fix / #2568 closure ──────────────────────────────────────────────
    // build_provider_for_runtime_type's "openclaw" arm no longer falls back to
    // http://127.0.0.1:18789 when `endpoint` is absent — missing endpoint is now a
    // hard Err, same shape as a missing token. These use the real keychain seam
    // (MockKeychain) via #[serial] since KEYCHAIN_OVERRIDE is process-global state.

    mod openclaw_endpoint_hardening_tests {
        use super::*;
        use crate::keychain::test_support::KeychainGuard;
        use serial_test::serial;

        fn with_mock_keychain<F: FnOnce()>(f: F) {
            let _kc = KeychainGuard::set();
            f();
        }

        /// #2568 B4 closure test: this is the evidence the linked issue closes on.
        /// A default agent with `runtime_type = 'openclaw'`, a valid gateway token, but
        /// NO `endpoint` (the exact v79-orphaned, not-yet-v80-recovered state) must
        /// resolve to `Err`, never silently to `Ok` with a localhost `base_url`.
        #[test]
        #[serial(keychain_override)]
        fn test_resolve_for_default_agent_errors_not_localhost_when_endpoint_missing() {
            with_mock_keychain(|| {
                let conn = make_test_conn();
                seed_agent(&conn, "oc-agent", Some("openclaw"), Some("claude-sonnet"), true); // CI-ALLOW: hardcoded-model
                crate::ai::credential_migration::write_gateway_token(&conn, "oc-token-abc").unwrap();
                // endpoint deliberately never set — simulates the orphaned post-v79,
                // pre-v80-recovery state.

                let result = resolve_runtime_config_for_default_agent(&conn);

                match result {
                    Ok(cfg) => panic!(
                        "must not silently fall back to localhost — got Ok with base_url {:?}",
                        match cfg.provider {
                            AiProvider::OpenClaw { base_url, .. } => base_url,
                            _ => "<non-openclaw provider, unexpected>".to_string(),
                        }
                    ),
                    Err(e) => {
                        assert!(
                            e.contains("gateway endpoint") || e.contains("Re-pair"),
                            "error should name the missing endpoint and point at re-pairing: {}",
                            e
                        );
                        assert!(
                            !e.contains("127.0.0.1") && !e.contains("18789"),
                            "error text must not contain the old localhost fallback: {}",
                            e
                        );
                    }
                }
            });
        }

        /// Confirms the hardening doesn't break the happy path: once endpoint IS
        /// restored (e.g. by migrate_v80), resolution succeeds and carries the real URL.
        #[test]
        #[serial(keychain_override)]
        fn test_resolve_for_default_agent_succeeds_when_endpoint_present() {
            with_mock_keychain(|| {
                let conn = make_test_conn();
                seed_agent(&conn, "oc-agent", Some("openclaw"), Some("claude-sonnet"), true); // CI-ALLOW: hardcoded-model
                crate::ai::credential_migration::write_gateway_token(&conn, "oc-token-abc").unwrap();
                crate::database::set_ai_setting(&conn, "endpoint", "http://100.64.1.2:18789").unwrap();

                let cfg = resolve_runtime_config_for_default_agent(&conn)
                    .expect("endpoint present + valid token → must resolve");
                match cfg.provider {
                    AiProvider::OpenClaw { base_url, .. } => {
                        assert!(base_url.contains("100.64.1.2"), "base_url: {}", base_url);
                    }
                    other => panic!("expected OpenClaw provider, got {:?}", other),
                }
            });
        }

        /// Missing token still errors first (unchanged pre-existing behavior) — the
        /// hardening only changes the endpoint arm, doesn't relax the token check.
        #[test]
        #[serial(keychain_override)]
        fn test_resolve_for_default_agent_errors_on_missing_token_even_with_endpoint() {
            with_mock_keychain(|| {
                let conn = make_test_conn();
                seed_agent(&conn, "oc-agent", Some("openclaw"), Some("claude-sonnet"), true); // CI-ALLOW: hardcoded-model
                crate::database::set_ai_setting(&conn, "endpoint", "http://100.64.1.2:18789").unwrap();
                // token deliberately never written

                let err = resolve_runtime_config_for_default_agent(&conn)
                    .expect_err("missing token must still error");
                assert!(err.contains("gateway token"), "error: {}", err);
            });
        }
    }

    // ── B3 (#2565) Req 2: Ollama explicit-connection semantics ──────────────────
    // build_provider_for_runtime_type's "ollama" arm no longer synthesizes a
    // provider from a bare reachability probe — it requires `ollama_endpoint` to
    // have been written by the Save flow (or migrate_v82's grandfather backfill).
    mod ollama_explicit_connection_tests {
        use super::*;

        #[test]
        fn test_build_provider_ollama_err_when_unconfigured() {
            let conn = make_test_conn();
            // ai_settings deliberately has no `ollama_endpoint` row.

            let err = build_provider_for_runtime_type(&conn, "ollama", None)
                .expect_err("unconfigured ollama must error, not silently default to localhost");
            assert_eq!(
                err, "No Ollama connection configured",
                "error text is the explicit-connection signal callers/UI match on"
            );
        }

        #[test]
        fn test_build_provider_ollama_ok_when_configured() {
            let conn = make_test_conn();
            crate::database::set_ai_setting(&conn, "ollama_endpoint", "http://localhost:11434")
                .unwrap();

            let provider = build_provider_for_runtime_type(&conn, "ollama", None)
                .expect("configured ollama_endpoint must resolve");
            match provider {
                AiProvider::Ollama { endpoint } => {
                    assert_eq!(endpoint, "http://localhost:11434");
                }
                other => panic!("expected Ollama provider, got {:?}", other),
            }
        }

        /// A *reachable* Ollama with no `ollama_endpoint` write must still error — the
        /// explicit-connection signal is the DB row, never a live probe. This module's
        /// `make_test_conn` has no network access at all, so "reachable but
        /// unconfigured" is exercised by proxy: absence of the row is the only
        /// condition under test, matching Req 2's actual dispatch-time check.
        #[test]
        fn test_build_provider_ollama_err_even_with_active_agent_but_no_endpoint() {
            let conn = make_test_conn();
            seed_agent(&conn, "agent-ollama", Some("ollama"), Some("llama3"), true);

            let err = build_provider_for_runtime_type(&conn, "ollama", None)
                .expect_err("an active ollama agent existing does not imply connected — only ollama_endpoint does");
            assert_eq!(err, "No Ollama connection configured");
        }
    }
}
