// Pane v0.17.9 — Agent Management (§24)
// First-class agent entities, separate from projects

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use crate::database::{get_pane_root, DbState};
use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};
use tauri::State;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub role: String,
    pub description: Option<String>,
    pub model: Option<String>,
    pub is_default: bool,
    pub is_active: bool,
    pub workspace_path: String,
    pub soul_hash: Option<String>,
    pub memory_hash: Option<String>,
    pub config_json: String,
    pub owner_id: Option<String>,
    pub runtime_ref: Option<String>,  // §10.1 ADR-0059 D1 / ADR-0063 D1: the agent's identity within its runtime's own registry
    pub runtime_type: Option<String>, // ADR-0063 D1: 'openclaw' | 'openrouter' | 'anthropic' | 'ollama' | 'venice'; NULL only as migration legacy (D4.3)
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateLocalAgentRequest {
    pub name: String,
    pub model: Option<String>,
    pub description: Option<String>,
    /// ADR-0063 D1 / #2522 Stage 6a: the runtime a brand-new local agent is created under.
    /// Callers (frontend) resolve this at the call site — the picker is 6b's scope; until
    /// then `AgentCreateOverlay` fills it via `resolve_runtime_type_for_new_local_agent`
    /// (or its JS equivalent) as a temporary seam. Must be non-empty and one of the four
    /// known runtime types; `create_local_agent_impl` validates and rejects anything else.
    pub runtime_type: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateAgentRequest {
    pub name: String,
    pub role: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    pub is_default: Option<bool>,
    pub workspace_path: Option<String>,
    pub source: Option<String>,
    /// #2629 (D7 fix): the runtime a brand-new agent row should be created under,
    /// supplied explicitly by a caller that already knows the answer (e.g. Onboarding's
    /// `connectProvider` — the provider the user just chose and validated a key for).
    /// When `Some(valid_value)`, `upsert_agent`'s INSERT branch uses it directly and
    /// SKIPS `resolve_runtime_type_for_new_local_agent` entirely — the explicit answer
    /// always wins over ambient keychain inference, even if both anthropic and
    /// openrouter keys happen to be present (see `resolve_runtime_type_for_upsert`
    /// below and its tests). When `None`, behavior is UNCHANGED from before this
    /// field existed: falls back to `resolve_runtime_type_for_new_local_agent`'s
    /// keychain-presence inference. Every existing caller that doesn't yet pass this
    /// field keeps its current (inferred) behavior — see PR #2629 description for the
    /// full caller audit. Valid values: "openclaw" | "openrouter" | "anthropic" | "ollama" | "venice"
    /// (same set `build_provider_for_runtime_type` accepts); anything else is rejected.
    /// Ignored entirely on the UPDATE branch — runtime_type remains immutable after
    /// creation (ADR-0063 D1), same as today.
    pub runtime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAgentRequest {
    pub agent_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub model: Option<String>,
    pub is_active: Option<bool>,
    /// ADR-0063 D4.3 / #2522 Stage 7c: the ONE legal way to write a non-NULL
    /// `runtime_type` onto a pre-existing migration-legacy row (see `Agent.runtime_type`
    /// doc comment). Every other path treats `runtime_type` as immutable-after-creation
    /// (see the comment on the `UPDATE ai_agents` branch below) — this field only ever
    /// succeeds when the row's current `runtime_type` is NULL; it is rejected once a
    /// value is already set. Frontend: `LegacyProviderPrompt`'s one-time "Set provider"
    /// action (never surfaced anywhere else — no settings control re-opens this).
    pub runtime_type: Option<String>,
}

/// Generate slug from name
fn generate_slug(name: &str) -> String {
    name.to_lowercase()
        .replace(" ", "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect::<String>()
}

/// Compute SHA-256 hash of content
fn compute_hash(content: &str) -> String {
    use sha2::{Sha256, Digest};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Get all agents
#[tauri::command]
pub fn get_agents(db: State<DbState>) -> Result<Vec<Agent>, String> {
    let conn = db.0.lock().unwrap();
    
    let mut stmt = conn.prepare(
        "SELECT id, name, slug, role, description, model, is_default, is_active, 
                workspace_path, soul_hash, memory_hash, config_json, owner_id, runtime_ref, source, 
                created_at, updated_at, runtime_type
         FROM ai_agents
         ORDER BY is_default DESC, name ASC"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    let agents = stmt.query_map([], |row| {
        Ok(Agent {
            id: row.get(0)?,
            name: row.get(1)?,
            slug: row.get(2)?,
            role: row.get(3)?,
            description: row.get(4)?,
            model: row.get(5)?,
            is_default: row.get::<_, i32>(6)? != 0,
            is_active: row.get::<_, i32>(7)? != 0,
            workspace_path: row.get(8)?,
            soul_hash: row.get(9)?,
            memory_hash: row.get(10)?,
            config_json: row.get(11)?,
            owner_id: row.get(12)?,
            runtime_ref: row.get(13)?,
            source: row.get(14)?,
            created_at: row.get(15)?,
            updated_at: row.get(16)?,
            runtime_type: row.get(17)?,
        })
    }).map_err(|e| format!("Failed to query agents: {}", e))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("Failed to collect agents: {}", e))?;
    
    Ok(agents)
}

/// Get single agent by ID
#[tauri::command]
pub fn get_agent(agent_id: String, db: State<DbState>) -> Result<Agent, String> {
    let conn = db.0.lock().unwrap();
    
    conn.query_row(
        "SELECT id, name, slug, role, description, model, is_default, is_active,
                workspace_path, soul_hash, memory_hash, config_json, owner_id, runtime_ref, source,
                created_at, updated_at, runtime_type
         FROM ai_agents
         WHERE id = ?1",
        params![agent_id],
        |row| {
            Ok(Agent {
                id: row.get(0)?,
                name: row.get(1)?,
                slug: row.get(2)?,
                role: row.get(3)?,
                description: row.get(4)?,
                model: row.get(5)?,
                is_default: row.get::<_, i32>(6)? != 0,
                is_active: row.get::<_, i32>(7)? != 0,
                workspace_path: row.get(8)?,
                soul_hash: row.get(9)?,
                memory_hash: row.get(10)?,
                config_json: row.get(11)?,
                owner_id: row.get(12)?,
                runtime_ref: row.get(13)?,
                source: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
                runtime_type: row.get(17)?,
            })
        },
    ).map_err(|e| format!("Failed to get agent: {}", e))
}

/// Get default agent
#[tauri::command]
pub fn get_main_agent(db: State<DbState>) -> Result<Option<Agent>, String> {
    let conn = db.0.lock().unwrap();
    
    conn.query_row(
        "SELECT id, name, slug, role, description, model, is_default, is_active,
                workspace_path, soul_hash, memory_hash, config_json, owner_id, runtime_ref, source,
                created_at, updated_at, runtime_type
         FROM ai_agents
         WHERE is_default = 1 AND is_active = 1",
        [],
        |row| {
            Ok(Agent {
                id: row.get(0)?,
                name: row.get(1)?,
                slug: row.get(2)?,
                role: row.get(3)?,
                description: row.get(4)?,
                model: row.get(5)?,
                is_default: row.get::<_, i32>(6)? != 0,
                is_active: row.get::<_, i32>(7)? != 0,
                workspace_path: row.get(8)?,
                soul_hash: row.get(9)?,
                memory_hash: row.get(10)?,
                config_json: row.get(11)?,
                owner_id: row.get(12)?,
                runtime_ref: row.get(13)?,
                source: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
                runtime_type: row.get(17)?,
            })
        },
    ).optional()
    .map_err(|e| format!("Failed to get main agent: {}", e))
}

/// ADR-0062 D3/D7 / ADR-0063 D3/D5: resolve "the designated default" agent —
/// the agent every sessionless or auto-created surface binds to. Fallback
/// chain (same order as `migrate_v78`'s backfill, and the same shape most
/// call sites in this codebase used to duplicate ad hoc as an inline
/// `is_default=1 AND is_active=1` query):
///   1. The active default agent (`is_default = 1 AND is_active = 1`).
///   2. Else the most-recently-created active agent (`is_active = 1 ORDER BY
///      created_at DESC`).
///   3. Else `None` — callers decide how to handle "no agent exists at all"
///      (a legitimately empty-agents-table state, e.g. very first launch
///      before onboarding creates the default agent).
///
/// This is the single shared resolver ADR-0062's build-scope audit calls for
/// ("no such shared resolver currently exists ... suggesting the build should
/// introduce one `resolve_designated_default_agent_id` helper rather than a
/// 4th copy-paste") — every INSERT site and ambient-resolution (D7) site that
/// needs "the default agent" should call this, not re-derive the query inline.
pub fn resolve_designated_default_agent_id(conn: &Connection) -> Result<Option<String>, String> {
    let main: Option<String> = conn
        .query_row(
            "SELECT id FROM ai_agents WHERE is_default = 1 AND is_active = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to query designated default agent: {}", e))?;

    if main.is_some() {
        return Ok(main);
    }

    conn
        .query_row(
            "SELECT id FROM ai_agents WHERE is_active = 1 ORDER BY created_at DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to query most-recently-created active agent: {}", e))
}

/// ADR-0062 D3.4: the self-heal contract. "Any reader encountering NULL
/// post-migration resolves the designated default, writes it back, and logs
/// distinctively" — and per the ADR's Consequences section, "the self-heal write
/// ... is the single sanctioned post-INSERT write path and must not grow." This
/// function is that one sanctioned write path; every read site that could
/// encounter a session with a NULL `agent_id` (post-migration legacy rows, or
/// rows where `migrate_v78`'s backfill found no agent at all) must funnel
/// through here rather than writing `chat_sessions.agent_id` directly.
///
/// Returns:
///   - `Ok(Some(agent_id))` if the session is already bound, or was just healed.
///   - `Ok(None)` if the session exists but has no agent to bind to yet (empty
///     `ai_agents` table — caller should treat this the same as "unbound").
///   - `Err` if the session itself does not exist, or on a DB error.
///
/// The write uses `WHERE id = ?1 AND agent_id IS NULL` as the one-shot guard:
/// a second call against an already-healed row finds `agent_id` non-NULL and
/// takes the read-only "already bound" branch below, never re-entering the
/// write path (idempotent, race-tolerant under this app's single-connection
/// `Mutex<Connection>` model).
pub fn resolve_and_selfheal_session_agent(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<String>, String> {
    // rusqlite's `.optional()` on a query mapped to `Option<String>` produces
    // `Option<Option<String>>` (outer: row-found-or-not, inner: NULL-or-not on
    // the nullable column itself) — `.ok().flatten()` collapses both "no row"
    // and "row with NULL column" into a single `None`, which is exactly what we
    // want here since we distinguish "session missing" via a separate check below.
    let session_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM chat_sessions WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to check session existence: {}", e))?;

    if !session_exists {
        return Err(format!("Session {} does not exist", session_id));
    }

    let current: Option<String> = conn
        .query_row(
            "SELECT agent_id FROM chat_sessions WHERE id = ?1",
            params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read session agent_id: {}", e))?
        .flatten();

    if let Some(agent_id) = current {
        return Ok(Some(agent_id));
    }

    // NULL — resolve the designated default and self-heal.
    let default_agent_id = resolve_designated_default_agent_id(conn)?;

    let Some(ref default_id) = default_agent_id else {
        // No agent exists anywhere yet (e.g. pre-onboarding). Nothing to heal with.
        return Ok(None);
    };

    let rows_updated = conn
        .execute(
            "UPDATE chat_sessions SET agent_id = ?1 WHERE id = ?2 AND agent_id IS NULL",
            params![default_id, session_id],
        )
        .map_err(|e| format!("Failed to self-heal session agent_id: {}", e))?;

    if rows_updated > 0 {
        eprintln!(
            "[BINDING SELF-HEAL] session {} had NULL agent_id — healed to designated default {}",
            session_id, default_id
        );
    }

    Ok(Some(default_id.clone()))
}

/// Dispatch-path guard: refuses to proceed when a session's *existing,
/// non-NULL* `agent_id` binding points at an agent that has been deactivated
/// (`is_active = 0`) — e.g. by disconnecting its runtime. Per
/// `docs/spec/sessions.md` ("Unavailable bound agent", ADR-0062 D5), a session
/// bound to an inactive agent must render read-only, and any dispatch attempt
/// must error explicitly naming the bound agent — never silently re-bind or
/// self-heal through a different agent.
///
/// Must run BEFORE `resolve_and_selfheal_session_agent`: self-heal only fires
/// on a NULL `agent_id` (legacy migration backfill) and is not a substitute
/// for this check — a non-NULL `agent_id` pointing at an inactive row sails
/// straight through self-heal's `Ok(Some(agent_id))` early-return (above) and
/// would otherwise only fail later, if at all, on a generic missing-credential
/// error that never names the agent.
///
/// No-ops (`Ok(())`) when:
///   - `agent_id` is NULL (nothing bound yet — self-heal's job, not this guard's),
///   - the bound agent row no longer exists (dangling reference — out of scope
///     for this guard; downstream resolution surfaces its own "not found"
///     error), or
///   - the bound agent exists and is active.
///
/// Errors when the bound agent exists and `is_active = 0`.
pub fn check_bound_agent_active(
    conn: &Connection,
    session_id: &str,
) -> Result<(), String> {
    let current: Option<String> = conn
        .query_row(
            "SELECT agent_id FROM chat_sessions WHERE id = ?1",
            params![session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read session agent_id: {}", e))?
        .flatten();

    let Some(agent_id) = current else {
        return Ok(());
    };

    let row: Option<(String, bool)> = conn
        .query_row(
            "SELECT name, is_active FROM ai_agents WHERE id = ?1",
            params![agent_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("Failed to look up bound agent {}: {}", agent_id, e))?;

    let Some((agent_name, is_active)) = row else {
        return Ok(());
    };

    if !is_active {
        return Err(format!(
            "Session is bound to agent '{}' which is currently inactive. Start a new session or reconnect the agent's provider.",
            agent_name
        ));
    }

    Ok(())
}

/// ADR-0063 D1: every creation path must write a non-NULL `runtime_type`
/// (NULL is legal ONLY as `migrate_v79`'s D4.3 migration legacy on
/// pre-existing rows). `upsert_agent` and `create_local_agent_impl` create
/// *local* agents only (neither ever sets `runtime_ref` — that column stays
/// NULL for both, exactly as before this rename); OC-imported agents are
/// created exclusively by the sync-import INSERT in `ai/commands.rs`, which
/// sets `runtime_type = 'openclaw'` directly and does not call this helper.
///
/// Stage 1 stopgap (documented, not a Stage 2+ dispatch change): a real
/// per-creation runtime picker is D6/Stage-2+ build scope ("the flow picks a
/// runtime… among connected runtimes"). Until that lands, this picks a
/// concrete runtime_type for a brand-new local row using currently-configured
/// credentials/agents as the signal, per this section's build standards
/// ("trace the data flow end-to-end" — the old `ai_settings.provider` read
/// this used to mirror is retired for real, ADR-0063 D8, #2564 B1 fix Path B;
/// this is a genuine logic change, not a mechanical swap — see B1 fix-scope
/// doc item #2).
///
/// B3 (#2565): `anthropic` and `openrouter` now have INDEPENDENT keychain slots
/// (`KEYCHAIN_ACCOUNT_ANTHROPIC_KEY` / `KEYCHAIN_ACCOUNT_OPENROUTER_KEY` — see
/// `credential_migration.rs`), so the old ambiguous-shared-key disambiguation
/// heuristic this function used to need is gone — each provider's key presence
/// is checked directly, with no need to consult `ai_agents` rows to guess which
/// provider a shared key "belongs to" (that state is no longer representable).
/// Resolution order:
///   1. Anthropic key present, OpenRouter key absent — pick `'anthropic'`.
///   2. Both keys present (or only OpenRouter present) — default to
///      `'openrouter'` (matches `AgentCreateOverlay.svelte`'s own default; this
///      tie-break is kept for the genuinely-both-configured case, per Matt's
///      ruling in the B3 scope doc).
///   3. No provider keys, but an active `ai_agents` row exists with
///      `runtime_type = 'ollama'` (ollama needs no credential to "be active",
///      only a reachable endpoint) — pick `'ollama'`.
///   4. Neither — fall back to `'openrouter'` (unchanged from the pre-fix
///      default; matches the frontend picker's own default so this stopgap
///      never surprises the picker it's standing in for).
/// A new row is never allowed to land NULL (unlike the migration's legacy
/// backfill, which may leave NULL for pre-existing rows).
pub fn resolve_runtime_type_for_new_local_agent(conn: &Connection) -> String {
    let has_anthropic_key =
        crate::ai::credential_migration::read_provider_key(conn, "anthropic").is_some();
    let has_openrouter_key =
        crate::ai::credential_migration::read_provider_key(conn, "openrouter").is_some();

    if has_anthropic_key && !has_openrouter_key {
        return "anthropic".to_string();
    }
    if has_anthropic_key || has_openrouter_key {
        // Both present, or only openrouter present — default to openrouter,
        // matching AgentCreateOverlay.svelte's own picker default.
        return "openrouter".to_string();
    }

    let has_active_ollama = conn
        .query_row(
            "SELECT 1 FROM ai_agents WHERE runtime_type = 'ollama' AND is_active = 1 LIMIT 1",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if has_active_ollama {
        return "ollama".to_string();
    }

    "openrouter".to_string()
}

/// #2629 (D7 fix): resolve the runtime_type to use for a brand-new `upsert_agent`
/// INSERT. If the caller supplied an explicit, valid `runtime_type` (e.g. Onboarding's
/// `connectProvider`), it wins outright — `resolve_runtime_type_for_new_local_agent`'s
/// ambient-keychain inference is skipped entirely. This is the fix for the reported bug:
/// a user who explicitly chose Anthropic must get `runtime_type: anthropic`, never
/// `openrouter`, regardless of what other provider keys happen to be sitting in the
/// keychain (leftover from a prior install, D8, or otherwise).
///
/// An explicit-but-invalid value is rejected with an error rather than silently falling
/// back to inference — never trust a network-origin string straight into an INSERT
/// (same posture as `create_local_agent_impl`'s validation of `CreateLocalAgentRequest`).
/// `None` preserves the exact pre-#2629 behavior for every caller that doesn't yet know
/// its runtime.
fn resolve_runtime_type_for_upsert(
    conn: &Connection,
    explicit_runtime_type: &Option<String>,
) -> Result<String, String> {
    match explicit_runtime_type {
        Some(value) => {
            if !matches!(value.as_str(), "openclaw" | "openrouter" | "anthropic" | "ollama" | "venice") {
                return Err(format!("Invalid runtime_type: '{}'", value));
            }
            eprintln!("[UPSERT_AGENT] Using caller-supplied runtime_type: {}", value);
            Ok(value.clone())
        }
        None => Ok(resolve_runtime_type_for_new_local_agent(conn)),
    }
}

/// Create or update agent (upsert)
#[tauri::command]
pub fn upsert_agent(request: CreateAgentRequest, db: State<DbState>) -> Result<Agent, String> {
    // #2649 (D19 closure, PR 3C) mechanical guard: OC agent rows come ONLY from
    // `sync_initial_import` / `sync_re_import` (both INSERT `ai_agents` directly with
    // `runtime_ref` set — see `sync_client.rs`). `upsert_agent` never sets `runtime_ref`,
    // so any caller reaching this function with `runtime_type: 'openclaw'` would mint
    // the exact `runtime_ref=NULL` phantom row that caused D19. This check runs before
    // any DB write (and before any directory/IDENTITY.md creation below) — rejection
    // must happen before any side effect, not just before the INSERT.
    if request.runtime_type.as_deref() == Some("openclaw") {
        return Err(
            "runtime_type 'openclaw' is not allowed via upsert_agent — OC agents are created \
             exclusively by sync_initial_import/sync_re_import"
                .to_string(),
        );
    }

    let slug = generate_slug(&request.name);
    let workspace_path = request.workspace_path.unwrap_or_else(|| {
        // Generate UUID if not provided - workspace_path should NEVER use display name
        uuid::Uuid::new_v4().simple().to_string().to_uppercase()
    });
    PaneFs::validate_name(&workspace_path)
        .map_err(|e| format!("Invalid agent workspace path: {}", e))?;
    let is_uuid_format = workspace_path.len() == 32
        && workspace_path.chars().all(|c| c.is_ascii_hexdigit());
    if !is_uuid_format {
        return Err(format!(
            "workspace_path '{}' is not a valid UUID format — display names and slugs must not be used as folder paths (§24.2)",
            workspace_path
        ));
    }
    // #2567 B6: role='local' default — this function never sets runtime_ref, so any
    // row it creates with no explicit role is definitionally a locally-created agent,
    // never a subagent. No live caller currently omits `role` (Onboarding.svelte
    // always passes role: 'main' explicitly), but the default must match
    // create_local_agent_impl's birth label for the same structural reason.
    let role = request.role.unwrap_or_else(|| "local".to_string());
    let is_default = request.is_default.unwrap_or(false);
    let source = request.source.unwrap_or_else(|| "import".to_string());
    
    // Get .pane/system directory
    let pane_fs = PaneFs::for_runtime()
        .map_err(|e| format!("Filesystem policy error: {}", e))?;
    let agent_relative = PaneFs::relative_from_names([workspace_path.as_str()])
        .map_err(|e| format!("Invalid agent directory path: {}", e))?;
    
    // Create directory structure
    pane_fs.create_dir_all(PaneRoot::System, &agent_relative)
        .map_err(|e| format!("Failed to create agent directory: {}", e))?;
    
    // Create logs/ subdirectory for default agent only
    if is_default {
        let logs_relative = PaneFs::relative_from_names([workspace_path.as_str(), "logs"])
            .map_err(|e| format!("Invalid logs directory path: {}", e))?;
        pane_fs.create_dir_all(PaneRoot::System, &logs_relative)
            .map_err(|e| format!("Failed to create logs directory: {}", e))?;
    }
    
    // ADR-0039 Decision 7: Write IDENTITY.md BEFORE any DB insert/update.
    // If this write fails, abort — do not create an agent record without an IDENTITY.md.
    let identity_relative = PaneFs::relative_from_names([workspace_path.as_str(), "IDENTITY.md"])
        .map_err(|e| format!("Invalid IDENTITY.md path: {}", e))?;
    let identity_path = pane_fs.authorize_write(PaneRoot::System, &identity_relative)
        .map_err(|e| format!("Unsafe IDENTITY.md destination: {}", e))?;
    if !identity_path.as_path().exists() {
        eprintln!("[AGENT] Writing IDENTITY.md for agent: {}", request.name);
        let identity_content = format!("# {}\n\nYour name is {}.\n", request.name, request.name);
        crate::file_ops::atomic_write(
            identity_path.as_path(),
            &identity_content,
            None,
            crate::file_ops::WriteOrigin::Local,
        ).map_err(|e| format!("Failed to write IDENTITY.md: {}", e))?;
        eprintln!("[AGENT] IDENTITY.md written for: {}", request.name);
    }

    // Read existing SOUL.md and MEMORY.md for hashes (if they exist)
    let soul_relative = PaneFs::relative_from_names([workspace_path.as_str(), "SOUL.md"])
        .map_err(|e| format!("Invalid SOUL.md path: {}", e))?;
    let soul_path = pane_fs.authorize_write(PaneRoot::System, &soul_relative)
        .map_err(|e| format!("Unsafe SOUL.md path: {}", e))?;
    let memory_relative = PaneFs::relative_from_names([workspace_path.as_str(), "MEMORY.md"])
        .map_err(|e| format!("Invalid MEMORY.md path: {}", e))?;
    let memory_path = pane_fs.authorize_write(PaneRoot::System, &memory_relative)
        .map_err(|e| format!("Unsafe MEMORY.md path: {}", e))?;
    
    let soul_hash = if soul_path.as_path().exists() {
        let soul_path = pane_fs.authorize_existing(
            PaneRoot::System,
            &soul_relative,
            ExistingKind::File,
        ).map_err(|e| format!("Unsafe existing SOUL.md path: {}", e))?;
        std::fs::read_to_string(soul_path.as_path()).ok()
            .map(|content| compute_hash(&content))
    } else {
        None
    };
    
    let memory_hash = if memory_path.as_path().exists() {
        let memory_path = pane_fs.authorize_existing(
            PaneRoot::System,
            &memory_relative,
            ExistingKind::File,
        ).map_err(|e| format!("Unsafe existing MEMORY.md path: {}", e))?;
        std::fs::read_to_string(memory_path.as_path()).ok()
            .map(|content| compute_hash(&content))
    } else {
        None
    };
    
    let now = chrono::Utc::now().to_rfc3339();
    
    let agent_id = {
        let conn = db.0.lock().unwrap();
        
        // Check if agent with this slug exists
        let existing_id: Option<String> = conn.query_row(
            "SELECT id FROM ai_agents WHERE slug = ?1",
            params![&slug],
            |row| row.get(0),
        ).optional()
        .map_err(|e| format!("Failed to check existing agent: {}", e))?;
        
        if let Some(id) = existing_id {
            // Update existing agent. runtime_type is intentionally NOT touched here —
            // ADR-0063 D1: an agent's runtime is assigned at creation and immutable
            // (locked), so an update to an existing row must never overwrite it.
            conn.execute(
                "UPDATE ai_agents 
                 SET name = ?1, role = ?2, description = ?3, model = ?4, is_default = ?5,
                     workspace_path = ?6, soul_hash = ?7, memory_hash = ?8, updated_at = ?9
                 WHERE id = ?10",
                params![
                    &request.name,
                    &role,
                    &request.description,
                    &request.model,
                    if is_default { 1 } else { 0 },
                    &workspace_path,
                    &soul_hash,
                    &memory_hash,
                    &now,
                    &id
                ],
            ).map_err(|e| format!("Failed to update agent: {}", e))?;
            
            id
        } else {
            // Insert new agent. ADR-0063 D1: runtime_type must be non-NULL on every
            // new row (backend enforcement, build-scope 1d) — runtime_ref stays NULL,
            // this is always a local (non-OC-imported) agent from this call path.
            // #2629 (D7 fix): prefer the caller's explicit runtime_type (e.g. Onboarding's
            // connectProvider) over ambient keychain inference; falls back to the prior
            // inference behavior when the caller doesn't supply one.
            let id = uuid::Uuid::new_v4().to_string();
            let runtime_type = resolve_runtime_type_for_upsert(&conn, &request.runtime_type)?;
            
            conn.execute(
                "INSERT INTO ai_agents 
                 (id, name, slug, role, description, model, is_default, workspace_path, 
                  soul_hash, memory_hash, source, runtime_type, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
                    &id,
                    &request.name,
                    &slug,
                    &role,
                    &request.description,
                    &request.model,
                    if is_default { 1 } else { 0 },
                    &workspace_path,
                    &soul_hash,
                    &memory_hash,
                    &source,
                    &runtime_type,
                    &now,
                    &now
                ],
            ).map_err(|e| format!("Failed to insert agent: {}", e))?;
            
            id
        }
    };
    
    get_agent(agent_id, db)
}

/// ADR-0063 D4.3 / #2522 Stage 7c: legacy-pin write — the ONE legal way to write a
/// non-NULL `runtime_type` onto a pre-existing migration-legacy row (`Agent.runtime_type`
/// doc comment). Only succeeds when the row's CURRENT `runtime_type` is NULL; rejected
/// once a value is already set, since every other creation path treats `runtime_type` as
/// immutable-after-creation (see the `UPDATE ai_agents` comment in `update_agent` below).
/// Split out from `update_agent` (mirrors the existing `_impl` split used by
/// `set_default_agent_impl` / `check_name_collision_impl`) so this validation is directly
/// unit-testable against an in-memory `Connection`, without needing a `tauri::State`.
pub fn set_legacy_runtime_type_impl(conn: &Connection, agent_id: &str, new_runtime_type: &str) -> Result<(), String> {
    if !matches!(new_runtime_type, "openclaw" | "openrouter" | "anthropic" | "ollama" | "venice") {
        return Err(format!("Invalid runtime_type: '{}'", new_runtime_type));
    }
    let current: Option<String> = conn.query_row(
        "SELECT runtime_type FROM ai_agents WHERE id = ?1",
        params![agent_id],
        |row| row.get(0),
    ).map_err(|e| format!("Failed to read agent for legacy-pin: {}", e))?;
    if current.is_some() {
        return Err("This agent already has a provider set — runtime_type is immutable after it is first assigned.".to_string());
    }
    conn.execute(
        "UPDATE ai_agents SET runtime_type = ?1, updated_at = ?2 WHERE id = ?3",
        params![new_runtime_type, chrono::Utc::now().to_rfc3339(), agent_id],
    ).map_err(|e| format!("Failed to set runtime_type: {}", e))?;
    eprintln!("[UPDATE_AGENT] legacy-pin: agent {} runtime_type NULL -> {}", agent_id, new_runtime_type);
    Ok(())
}

/// Update agent metadata
#[tauri::command]
pub fn update_agent(request: UpdateAgentRequest, db: State<DbState>) -> Result<Agent, String> {
    eprintln!("[UPDATE_AGENT] called with: agent_id={}, name={:?}, runtime_type={:?}", request.agent_id, request.name, request.runtime_type);
    let agent_id = request.agent_id.clone();

    // ADR-0063 D4.3 / #2522 Stage 7c: legacy-pin write. Validate + write BEFORE anything
    // else touches the row (see set_legacy_runtime_type_impl doc comment above).
    if let Some(new_runtime_type) = &request.runtime_type {
        let conn = db.0.lock().unwrap();
        set_legacy_runtime_type_impl(&conn, &agent_id, new_runtime_type)?;
    }

    // If name is being changed, handle folder rename first
    if request.name.is_some() {
        let conn = db.0.lock().unwrap();
        
        // Get current agent data
        let old_agent: Agent = conn.query_row(
            "SELECT id, name, slug, role, description, model, is_default, is_active, workspace_path, 
                    soul_hash, memory_hash, config_json, owner_id, runtime_ref, source, created_at, updated_at, runtime_type
             FROM ai_agents WHERE id = ?1",
            params![&agent_id],
            |row| Ok(Agent {
                id: row.get(0)?,
                name: row.get(1)?,
                slug: row.get(2)?,
                role: row.get(3)?,
                description: row.get(4)?,
                model: row.get(5)?,
                is_default: row.get(6)?,
                is_active: row.get(7)?,
                workspace_path: row.get(8)?,
                soul_hash: row.get(9)?,
                memory_hash: row.get(10)?,
                config_json: row.get(11)?,
                owner_id: row.get(12)?,
                runtime_ref: row.get(13)?,
                source: row.get(14)?,
                created_at: row.get(15)?,
                updated_at: row.get(16)?,
                runtime_type: row.get(17)?,
            })
        ).map_err(|e| format!("Failed to get agent: {}", e))?;
        
        // Block default agent rename on an OpenClaw-runtime agent — symlink safety (§251).
        // ADR-0063 D1/D8: the global `ai_settings.provider` key this gate used to read is
        // deleted by the v79 migration (D8's key-retirement) — the per-row `runtime_type`
        // is the same fact, now stored per-agent instead of globally, so this reads the
        // row's own runtime_type rather than the (post-migration, absent) global setting.
        if old_agent.is_default && old_agent.runtime_type.as_deref() == Some(crate::ai::provider::PROVIDER_OPENCLAW) {
            return Err("This agent is linked to an external system (OpenClaw). Rename is not available while the link is active.".to_string());
        }
        
        // workspace_path is IMMUTABLE after creation - rename only changes the display name
        // No folder rename - folder stays at workspace_path (UUID)
        // This ensures sync and file paths never break when renaming

        // ADR-0039 Decision 7: Write IDENTITY.md BEFORE DB update.
        // If IDENTITY.md write fails, abort — do not update ai_agents.name without the file.
        let new_name = request.name.as_deref().unwrap();
        PaneFs::validate_name(&old_agent.workspace_path)
            .map_err(|e| format!("Invalid agent workspace path in database: {}", e))?;
        let pane_fs = PaneFs::for_runtime()
            .map_err(|e| format!("Filesystem policy error: {}", e))?;
        let identity_relative = PaneFs::relative_from_names([
            old_agent.workspace_path.as_str(),
            "IDENTITY.md",
        ]).map_err(|e| format!("Invalid IDENTITY.md path: {}", e))?;
        let identity_path = pane_fs.authorize_write(PaneRoot::System, &identity_relative)
            .map_err(|e| format!("Unsafe IDENTITY.md destination: {}", e))?;
        eprintln!("[AGENT] Updating IDENTITY.md on rename: {} -> {}", old_agent.name, new_name);
        let identity_content = format!("# {}\n\nYour name is {}.\n", new_name, new_name);
        crate::file_ops::atomic_write(
            identity_path.as_path(),
            &identity_content,
            None,
            crate::file_ops::WriteOrigin::Local,
        ).map_err(|e| format!("Failed to update IDENTITY.md during rename: {}", e))?;
        eprintln!("[AGENT] IDENTITY.md updated for rename to: {}", new_name);
    }
    
    {
        let conn = db.0.lock().unwrap();
        
        let now = chrono::Utc::now().to_rfc3339();
        
        // Build dynamic update query based on provided fields
        let mut update_parts = vec![];
        
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(now.clone())];
        let mut param_idx = 2;
        
        if let Some(name) = &request.name {
            update_parts.push(format!("name = ?{}", param_idx));
            params_vec.push(Box::new(name.clone()));
            param_idx += 1;
            
            // Also update slug when name changes
            let new_slug = generate_slug(name);
            update_parts.push(format!("slug = ?{}", param_idx));
            params_vec.push(Box::new(new_slug));
            param_idx += 1;
        }
        
        if let Some(desc) = &request.description {
            update_parts.push(format!("description = ?{}", param_idx));
            params_vec.push(Box::new(desc.clone()));
            param_idx += 1;
        }
        
        if let Some(model) = &request.model {
            update_parts.push(format!("model = ?{}", param_idx));
            params_vec.push(Box::new(model.clone()));
            param_idx += 1;
        }
        
        if let Some(is_active) = request.is_active {
            update_parts.push(format!("is_active = ?{}", param_idx));
            params_vec.push(Box::new(if is_active { 1 } else { 0 }));
            param_idx += 1;
        }
        
        params_vec.push(Box::new(agent_id.clone()));
        
        let mut all_updates = vec!["updated_at = ?1".to_string()];
        all_updates.extend(update_parts);
        
        let query = format!(
            "UPDATE ai_agents SET {} WHERE id = ?{}",
            all_updates.join(", "),
            param_idx
        );
        
        conn.execute(
            &query,
            rusqlite::params_from_iter(params_vec.iter().map(|b| b.as_ref())),
        ).map_err(|e| format!("Failed to update agent: {}", e))?;
    }
    
    get_agent(agent_id, db)
}

/// Reactivate agent
#[tauri::command]
pub fn reactivate_agent(agent_id: String, db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    
    let now = chrono::Utc::now().to_rfc3339();
    
    conn.execute(
        "UPDATE ai_agents SET is_active = 1, updated_at = ?1 WHERE id = ?2",
        params![now, agent_id],
    ).map_err(|e| format!("Failed to reactivate agent: {}", e))?;
    
    Ok(())
}

/// Update agent file hashes (called after file sync).
///
/// Retained as a public sync integration seam even when no in-crate caller is enabled.
#[allow(dead_code)]
pub fn update_agent_hashes(
    conn: &Connection,
    agent_id: &str,
    soul_hash: Option<String>,
    memory_hash: Option<String>,
) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    
    conn.execute(
        "UPDATE ai_agents SET soul_hash = ?1, memory_hash = ?2, updated_at = ?3 WHERE id = ?4",
        params![soul_hash, memory_hash, now, agent_id],
    ).map_err(|e| format!("Failed to update agent hashes: {}", e))?;
    
    Ok(())
}

/// Get system username for default agent naming
#[tauri::command]
pub fn get_system_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "My Agent".to_string())
}

/// Internal: check name collision against the live connection.
/// Checks ALL agents including OC-imported and inactive ones (§24.10.1).
fn check_name_collision_impl(conn: &Connection, name: &str) -> Result<bool, String> {
    let slug = generate_slug(name);
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ai_agents WHERE slug = ?1",
        params![slug],
        |row| row.get(0),
    ).map_err(|e| format!("Failed to check name collision: {}", e))?;
    Ok(count > 0)
}

/// Check if an agent name would collide with an existing agent's slug.
/// Checks ALL agents including OC-imported and inactive ones (§24.10.1).
/// Returns true if collision exists, false if name is available.
#[tauri::command]
pub fn check_name_collision(name: String, db: State<DbState>) -> Result<bool, String> {
    let conn = db.0.lock().unwrap();
    check_name_collision_impl(&conn, &name)
}

/// Internal: create a local agent. Returns the new agent_id on success.
/// agent_dir, agent_id, and workspace_path are provided for testability (avoids calling get_pane_root()
/// in tests) and to guarantee the filesystem path and DB workspace_path come from the SAME UUID.
/// NEVER call Uuid::new_v4() inside this function — callers own UUID generation (Fix A, audit 2026-07-26).
fn create_local_agent_impl(
    conn: &Connection,
    request: &CreateLocalAgentRequest,
    agent_dir: &std::path::Path,
    agent_id: &str,
    workspace_path: &str,
) -> Result<String, String> {
    if request.name.trim().is_empty() {
        return Err("Agent name cannot be empty".to_string());
    }

    // #2522 Stage 6a: runtime_type now arrives from the caller instead of being resolved
    // internally (that resolution — resolve_runtime_type_for_new_local_agent — is kept
    // available as a temporary seam for AgentCreateOverlay until 6b's real picker lands).
    // Still validate it here: never trust a network-origin string straight into an INSERT.
    if !matches!(request.runtime_type.as_str(), "openclaw" | "openrouter" | "anthropic" | "ollama" | "venice") {
        return Err(format!("Invalid runtime_type: '{}'", request.runtime_type));
    }

    let slug = generate_slug(&request.name);

    // Step 1: Check collision before touching the filesystem
    if check_name_collision_impl(conn, &request.name)? {
        return Err(format!("An agent named '{}' already exists — pick a different name", request.name));
    }

    // agent_id and workspace_path are passed in from create_local_agent — both derived from a
    // SINGLE UUID so the filesystem path and DB workspace_path always match (§4.2, Fix A 2026-07-26).
    // Do NOT generate any UUID here.

    // Step 2: Create filesystem folder + skeleton files (before DB write, per ADR-0039 D7)
    std::fs::create_dir_all(agent_dir)
        .map_err(|e| format!("Failed to create agent directory: {}", e))?;

    // Create logs/ subdirectory — new local agents are always promoted to main (§24.7A, Item 3 fix)
    std::fs::create_dir_all(agent_dir.join("logs"))
        .map_err(|e| format!("Failed to create logs directory: {}", e))?;

    // Write IDENTITY.md atomically (ADR-0039 D7: filesystem before DB)
    let identity_content = format!("# {}\n\nYour name is {}.\n", request.name, request.name);
    let identity_path = agent_dir.join("IDENTITY.md");
    let tmp_identity = agent_dir.join(".IDENTITY.md.tmp");
    std::fs::write(&tmp_identity, &identity_content)
        .map_err(|e| format!("Failed to write IDENTITY.md (temp): {}", e))?;
    std::fs::rename(&tmp_identity, &identity_path)
        .map_err(|e| format!("Failed to finalize IDENTITY.md: {}", e))?;

    // Write empty SOUL.md + MEMORY.md skeletons
    let soul_path = agent_dir.join("SOUL.md");
    if !soul_path.exists() {
        std::fs::write(&soul_path, "")
            .map_err(|e| format!("Failed to write SOUL.md: {}", e))?;
    }
    let memory_path = agent_dir.join("MEMORY.md");
    if !memory_path.exists() {
        std::fs::write(&memory_path, "")
            .map_err(|e| format!("Failed to write MEMORY.md: {}", e))?;
    }

    // Step 3: Single transaction — demote current default, INSERT new agent as default
    // §24.7A: single tx for demote-then-promote, never two independent UPDATEs
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute_batch("BEGIN EXCLUSIVE")
        .map_err(|e| format!("Failed to begin transaction: {}", e))?;

    // ADR-0063 D1: every creation path writes a non-NULL runtime_type. Locally-created
    // agents always have runtime_ref = NULL (no runtime-side registry ref); runtime_type
    // is now supplied by the caller (#2522 Stage 6a) rather than resolved internally.
    let runtime_type = &request.runtime_type;

    let tx_result = (|| -> Result<(), String> {
        // Demote current default
        conn.execute(
            "UPDATE ai_agents SET is_default = 0, updated_at = ?1 WHERE is_default = 1",
            params![&now],
        ).map_err(|e| format!("Failed to demote current default: {}", e))?;

        // Insert new agent as default (source = 'ai_created', runtime_ref = NULL).
        // #2567 B6: role='local' at birth — a user-created agent is never a
        // subagent, structurally. Prior to this fix this literal was 'sub',
        // relying on the runtime_ref=NULL workaround predicate to disambiguate
        // locally-created agents from real OC subagents (see B5/#2577). See
        // migrate_v81 for the one-time data repair of pre-existing rows.
        conn.execute(
            "INSERT INTO ai_agents 
             (id, name, slug, role, description, model, is_default, is_active,
              workspace_path, config_json, source, runtime_ref, runtime_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'local', ?4, ?5, 1, 1, ?6, '{}', 'ai_created', NULL, ?7, ?8, ?8)",
            params![
                agent_id,
                &request.name,
                &slug,
                &request.description,
                &request.model,
                workspace_path,
                runtime_type,
                &now,
            ],
        ).map_err(|e| format!("Failed to insert agent: {}", e))?;

        Ok(())
    })();

    if let Err(e) = tx_result {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e);
    }

    conn.execute_batch("COMMIT")
        .map_err(|e| format!("Failed to commit transaction: {}", e))?;

    Ok(agent_id.to_string())
}

/// Create a new locally-created agent.
/// Implements ADR-0059 D4 atomicity guarantee:
///   1. Validate name + slug uniqueness
///   2. Generate ONE UUID → both agent_id and workspace_path (§4.2, Fix A audit 2026-07-26)
///   3. Create filesystem folder + IDENTITY.md at .pane/system/{workspace_path}
///   4. Single transaction: demote current default, INSERT new agent as is_default=1
/// Returns error (no DB write) if filesystem or transaction step fails.
/// ADR-0063 D1: runtime_type is always written non-NULL (backend enforcement, build-scope 1d).
/// #2522 Stage 6a: runtime_type is now an explicit param on `request` — the caller picks it
/// (frontend picker is 6b's scope; `resolve_runtime_type_for_new_local_agent` remains public
/// as the temporary seam callers use until then) rather than this command resolving it itself.
#[tauri::command]
pub fn create_local_agent(request: CreateLocalAgentRequest, db: State<DbState>) -> Result<Agent, String> {
    // Generate ONE UUID — used for both id (hyphenated) and workspace_path (simple, uppercase).
    // This guarantees the filesystem directory and the DB workspace_path column are always in sync.
    let agent_uuid = uuid::Uuid::new_v4();
    let agent_id = agent_uuid.to_string();
    let workspace_path = agent_uuid.simple().to_string().to_uppercase();
    let pane_root = get_pane_root();
    let agent_dir = pane_root.join(".pane").join("system").join(&workspace_path);
    let conn = db.0.lock().unwrap();
    let returned_id = create_local_agent_impl(&conn, &request, &agent_dir, &agent_id, &workspace_path)?;
    drop(conn);
    get_agent(returned_id, db)
}

/// Internal: promote an agent to default (demote current, promote target), single transaction.
/// Renamed from `set_main_agent_impl` (ADR-0063 D3: "Set as Main" → "Set as Default";
/// `set_main_agent_impl` renamed accordingly). Transaction body preserved verbatim
/// (ADR-0045 D1/D2 demote-then-promote shape, per D3's explicit "preserved under the
/// new name" instruction) — only column/index names changed.
fn set_default_agent_impl(conn: &Connection, agent_id: &str) -> Result<(), String> {
    // Validate target agent is active and eligible (#2567: restore the role='sub'
    // exclusion that the old "Set as Main" buttons enforced, #2271; that predicate
    // lived only in Settings.svelte and was dropped when DefaultAgentPickerModal
    // replaced those buttons — UI filters are not enforcement, so the guard belongs
    // at the command level where every caller (modal, future callers) is covered).
    let row: Option<(i32, Option<String>, String)> = conn.query_row(
        "SELECT is_active, runtime_ref, role FROM ai_agents WHERE id = ?1",
        params![agent_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional()
    .map_err(|e| format!("Failed to look up agent: {}", e))?;

    match row {
        None => return Err(format!("Agent '{}' not found", agent_id)),
        Some((is_active, _, _)) if is_active == 0 => {
            return Err(format!("Agent '{}' is not active and cannot be promoted to default", agent_id));
        }
        // #B5 (Reed's PR #2577 audit): role='sub' alone is too broad — locally-created
        // agents share role='sub' but have runtime_ref = NULL (they aren't OC subagents).
        // Only reject when the agent is BOTH role='sub' AND has a runtime_ref, i.e. an
        // actual imported OC subagent.
        Some((_, ref runtime_ref, ref role)) if role == "sub" && runtime_ref.is_some() => {
            return Err("Subagents cannot be set as default".to_string());
        }
        _ => {}
    }

    let now = chrono::Utc::now().to_rfc3339();

    conn.execute_batch("BEGIN EXCLUSIVE")
        .map_err(|e| format!("Failed to begin transaction: {}", e))?;

    let tx_result = (|| -> Result<(), String> {
        // Demote current default
        conn.execute(
            "UPDATE ai_agents SET is_default = 0, updated_at = ?1 WHERE is_default = 1",
            params![&now],
        ).map_err(|e| format!("Failed to demote current default: {}", e))?;

        // Promote target agent
        let rows_updated = conn.execute(
            "UPDATE ai_agents SET is_default = 1, updated_at = ?1 WHERE id = ?2 AND is_active = 1",
            params![&now, agent_id],
        ).map_err(|e| format!("Failed to promote agent: {}", e))?;

        if rows_updated == 0 {
            return Err(format!("Agent '{}' could not be promoted (not found or not active)", agent_id));
        }

        Ok(())
    })();

    if let Err(e) = tx_result {
        let _ = conn.execute_batch("ROLLBACK");
        return Err(e);
    }

    conn.execute_batch("COMMIT")
        .map_err(|e| format!("Failed to commit transaction: {}", e))?;

    Ok(())
}

/// Set an agent as the current default agent.
/// Implements ADR-0059 D2 / ADR-0063 D3: single transaction demote-then-promote.
/// ADR-0063 D3: the class gate (ADR-0059 D2's OC-agent-promotion guard) dies with the
/// class — "Set as Default" is offered for any active agent of any runtime.
/// Command name kept as `set_main_agent` (frontend `invoke('set_main_agent', ...)`
/// call site is out of this stage's rename scope — mechanical types.ts/Onboarding.svelte
/// renames only, per build-scope 1f).
#[tauri::command]
pub fn set_main_agent(agent_id: String, db: State<DbState>) -> Result<Agent, String> {
    {
        let conn = db.0.lock().unwrap();
        set_default_agent_impl(&conn, &agent_id)?;
    }
    get_agent(agent_id, db)
}

// seed_agent_uploads / seed_agent_interview_impl / seed_agent_interview deleted
// (#2288 PR 3, §24.10.1c "generate, never transcribe"): these wrote raw
// filename lists / verbatim Q&A dumps to MEMORY.md/SOUL.md with no model
// call. AgentCreateOverlay.svelte now calls the shared
// src/lib/onboarding/seedGeneration.ts pipeline (same synthesis path
// onboarding uses) and writes files itself via write_system_file.

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn make_test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("
            PRAGMA foreign_keys = ON;
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
            CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_default ON ai_agents(is_default) WHERE is_default = 1;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_runtime_unique ON ai_agents(runtime_type, runtime_ref);
            CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY NOT NULL,
                value TEXT NOT NULL
            );
            CREATE TABLE chat_sessions (
                id          TEXT PRIMARY KEY,
                project_id  TEXT,
                agent_id    TEXT REFERENCES ai_agents(id),
                is_active   INTEGER NOT NULL DEFAULT 1
            );
        ").expect("setup schema");
        conn
    }

    fn seed_agent(conn: &Connection, id: &str, name: &str, is_default: bool, is_active: bool, runtime_ref: Option<&str>) {
        // Default role is 'main' — most callers are exercising promotion/collision/import
        // logic unrelated to the role='sub' guard (#2567). Tests exercising the sub-role
        // guard specifically use seed_agent_with_role below.
        seed_agent_with_role(conn, id, name, is_default, is_active, runtime_ref, "main");
    }

    fn seed_agent_with_role(conn: &Connection, id: &str, name: &str, is_default: bool, is_active: bool, runtime_ref: Option<&str>, role: &str) {
        let slug = generate_slug(name);
        let now = chrono::Utc::now().to_rfc3339();
        // OC-imported (runtime_ref present) rows get runtime_type='openclaw'; local rows
        // (runtime_ref NULL) default to 'openrouter' — same convention as
        // resolve_runtime_type_for_new_local_agent's own fallback, kept simple/explicit
        // here since these are hand-seeded fixture rows, not exercising that helper.
        let runtime_type = if runtime_ref.is_some() { "openclaw" } else { "openrouter" };
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, config_json, source, runtime_ref, runtime_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '{}', 'import', ?8, ?9, ?10, ?10)",
            params![
                id, name, slug, role,
                if is_default { 1 } else { 0 },
                if is_active { 1 } else { 0 },
                format!("WP{}", id),
                runtime_ref,
                runtime_type,
                now
            ],
        ).expect("seed agent");
    }

    // --- check_name_collision_impl ---

    #[test]
    fn test_check_name_collision_no_collision() {
        let conn = make_test_conn();
        let result = check_name_collision_impl(&conn, "Totally New Name")
            .expect("should not error");
        assert!(!result, "brand-new name should return false");
    }

    #[test]
    fn test_check_name_collision_existing() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-1", "Sam Smith", false, true, None);
        let result = check_name_collision_impl(&conn, "Sam Smith")
            .expect("should not error");
        assert!(result, "existing name should return true");
    }

    #[test]
    fn test_check_name_collision_slug_match() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-2", "Sam Smith", false, true, None);
        // Differently-cased version that produces same slug
        let result = check_name_collision_impl(&conn, "SAM SMITH")
            .expect("should not error");
        assert!(result, "slug-equivalent name should collide");
    }

    #[test]
    fn test_check_name_collision_checks_inactive_agents() {
        let conn = make_test_conn();
        seed_agent(&conn, "inactive-1", "Inactive One", false, false, None);
        let result = check_name_collision_impl(&conn, "Inactive One")
            .expect("should not error");
        assert!(result, "collision check must include inactive agents (§24.10.1)");
    }

    // --- runtime_ref in SELECT (renamed from oc_agent_id, #2522 ADR-0063 D1) ---

    #[test]
    fn test_get_agents_select_includes_runtime_ref() {
        let conn = make_test_conn();
        seed_agent(&conn, "local-agent", "Local One", true, true, None);
        seed_agent(&conn, "oc-agent", "OC One", false, true, Some("oc-remote-id-123"));

        // Directly verify the column is returned in the SELECT
        let mut stmt = conn.prepare(
            "SELECT id, runtime_ref FROM ai_agents ORDER BY name"
        ).unwrap();
        let rows: Vec<(String, Option<String>)> = stmt.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?))
        }).unwrap().collect::<Result<_, _>>().unwrap();

        assert_eq!(rows.len(), 2);
        let local = rows.iter().find(|(id, _)| id == "local-agent").unwrap();
        assert!(local.1.is_none(), "local agent must have NULL runtime_ref");
        let oc = rows.iter().find(|(id, _)| id == "oc-agent").unwrap();
        assert_eq!(oc.1.as_deref(), Some("oc-remote-id-123"));
    }

    // --- create_local_agent_impl ---

    #[test]
    fn test_create_local_agent_happy_path() {
        let conn = make_test_conn();
        seed_agent(&conn, "existing-main", "Old Main", true, true, None);
        let tmp = TempDir::new().unwrap();
        let agent_uuid = uuid::Uuid::new_v4();
        let agent_id = agent_uuid.to_string();
        let workspace_path = agent_uuid.simple().to_string().to_uppercase();
        let agent_dir = tmp.path().join(&workspace_path);

        let req = CreateLocalAgentRequest {
            name: "New Agent".to_string(),
            model: Some("gpt-4".to_string()),
            description: None,
            runtime_type: "openrouter".to_string(),
        };
        let id = create_local_agent_impl(&conn, &req, &agent_dir, &agent_id, &workspace_path)
            .expect("create should succeed");

        // New agent must be default
        let new_default_id: String = conn.query_row(
            "SELECT id FROM ai_agents WHERE is_default = 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(new_default_id, id, "new agent must be promoted to default");

        // Old agent must be demoted
        let old_default: i32 = conn.query_row(
            "SELECT is_default FROM ai_agents WHERE id = 'existing-main'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(old_default, 0, "old default must be demoted");

        // source must be 'ai_created', runtime_ref must be NULL, runtime_type must be non-NULL,
        // role must be 'local' (#2567 B6: a user-created agent is never a subagent,
        // structurally — was 'sub' prior to this fix).
        let (source, runtime_ref, runtime_type, role): (String, Option<String>, Option<String>, String) = conn.query_row(
            "SELECT source, runtime_ref, runtime_type, role FROM ai_agents WHERE id = ?1",
            params![&id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ).unwrap();
        assert_eq!(source, "ai_created");
        assert!(runtime_ref.is_none(), "locally created agent must have NULL runtime_ref");
        assert_eq!(runtime_type.as_deref(), Some("openrouter"), "#2522 Stage 6a: runtime_type must equal what the caller passed in");
        assert_eq!(role, "local", "#2567 B6: create_local_agent_impl must write role='local' at birth, not 'sub'");

        // IDENTITY.md must exist
        assert!(agent_dir.join("IDENTITY.md").exists(), "IDENTITY.md must be created");
    }

    /// #2522 Stage 6a: create_local_agent_impl writes whatever valid runtime_type the caller
    /// passes in — it no longer resolves this itself. Exercise a non-default value to prove
    /// the param actually flows through end to end, not just the "openrouter" fixture default.
    #[test]
    fn test_create_local_agent_persists_caller_supplied_runtime_type() {
        let conn = make_test_conn();
        let tmp = TempDir::new().unwrap();
        let agent_uuid = uuid::Uuid::new_v4();
        let agent_id = agent_uuid.to_string();
        let workspace_path = agent_uuid.simple().to_string().to_uppercase();
        let agent_dir = tmp.path().join(&workspace_path);

        let req = CreateLocalAgentRequest {
            name: "Anthropic Agent".to_string(),
            model: None,
            description: None,
            runtime_type: "anthropic".to_string(),
        };
        let id = create_local_agent_impl(&conn, &req, &agent_dir, &agent_id, &workspace_path)
            .expect("create should succeed");

        let runtime_type: Option<String> = conn.query_row(
            "SELECT runtime_type FROM ai_agents WHERE id = ?1",
            params![&id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(runtime_type.as_deref(), Some("anthropic"));
    }

    /// #2522 Stage 6a: an invalid/unknown runtime_type string must be rejected before any
    /// filesystem or DB write — defensive validation per SOUL.md §5 (no unwrap on
    /// network-origin data; validate inputs).
    #[test]
    fn test_create_local_agent_invalid_runtime_type_returns_error_no_writes() {
        let conn = make_test_conn();
        let tmp = TempDir::new().unwrap();
        let agent_uuid = uuid::Uuid::new_v4();
        let agent_id = agent_uuid.to_string();
        let workspace_path = agent_uuid.simple().to_string().to_uppercase();
        let agent_dir = tmp.path().join(&workspace_path);

        let req = CreateLocalAgentRequest {
            name: "Bad Runtime Agent".to_string(),
            model: None,
            description: None,
            runtime_type: "totally-not-a-runtime".to_string(),
        };
        let err = create_local_agent_impl(&conn, &req, &agent_dir, &agent_id, &workspace_path)
            .expect_err("invalid runtime_type should error");
        assert!(err.contains("Invalid runtime_type"), "error should name the bad field: {}", err);

        assert!(!agent_dir.exists(), "agent directory must not be created when runtime_type is invalid");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM ai_agents", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0, "invalid runtime_type must prevent DB write");
    }

    /// Fix A regression guard: DB workspace_path must match the actual filesystem directory name.
    /// This is the gap that allowed the UUID triple-generation bug to go undetected.
    /// Ref: audit 2026-07-26-pr-2264-agent-ui-audit.md Finding 2.
    #[test]
    fn test_create_local_agent_workspace_path_matches_filesystem_dir() {
        let conn = make_test_conn();
        seed_agent(&conn, "existing-main", "Old Main", true, true, None);
        let tmp = TempDir::new().unwrap();

        // Simulate exactly what create_local_agent() does: one UUID, two derived values.
        let agent_uuid = uuid::Uuid::new_v4();
        let agent_id = agent_uuid.to_string();
        let workspace_path = agent_uuid.simple().to_string().to_uppercase();
        let agent_dir = tmp.path().join(&workspace_path);

        let req = CreateLocalAgentRequest {
            name: "Path Match Test".to_string(),
            model: None,
            description: None,
            runtime_type: "openrouter".to_string(),
        };
        let returned_id = create_local_agent_impl(&conn, &req, &agent_dir, &agent_id, &workspace_path)
            .expect("create should succeed");

        // The DB-stored workspace_path must equal the folder name where files were actually created.
        let db_workspace_path: String = conn.query_row(
            "SELECT workspace_path FROM ai_agents WHERE id = ?1",
            params![&returned_id],
            |r| r.get(0),
        ).unwrap();

        let fs_dir_name = agent_dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("agent_dir must have a filename component");

        assert_eq!(
            db_workspace_path, fs_dir_name,
            "DB workspace_path '{}' must match filesystem directory name '{}' — \
             mismatch means agent files are permanently unreachable (Fix A, audit 2026-07-26)",
            db_workspace_path, fs_dir_name
        );

        // Also confirm the files physically exist at that path
        assert!(agent_dir.join("IDENTITY.md").exists(),
            "IDENTITY.md must exist at the path that matches workspace_path");
    }

    #[test]
    fn test_create_local_agent_collision_returns_error_no_db_write() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-existing", "Sam Smith", true, true, None);
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("collision-dir");

        let req = CreateLocalAgentRequest {
            name: "Sam Smith".to_string(), // same slug
            model: None,
            description: None,
            runtime_type: "openrouter".to_string(),
        };
        let agent_uuid = uuid::Uuid::new_v4();
        let agent_id = agent_uuid.to_string();
        let workspace_path = agent_uuid.simple().to_string().to_uppercase();
        let err = create_local_agent_impl(&conn, &req, &agent_dir, &agent_id, &workspace_path)
            .expect_err("should error on collision");
        assert!(err.contains("already exists"), "error should mention collision: {}", err);

        // No new row should have been inserted
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_agents WHERE source = 'ai_created'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0, "collision must prevent DB write");

        // Directory must not have been created (collision detected before FS ops)
        assert!(!agent_dir.exists(), "agent directory must not be created on collision");
    }

    #[test]
    fn test_create_local_agent_empty_name_returns_error() {
        let conn = make_test_conn();
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("empty-name-dir");

        let req = CreateLocalAgentRequest {
            name: "   ".to_string(),
            model: None,
            description: None,
            runtime_type: "openrouter".to_string(),
        };
        let agent_uuid = uuid::Uuid::new_v4();
        let agent_id = agent_uuid.to_string();
        let workspace_path = agent_uuid.simple().to_string().to_uppercase();
        let err = create_local_agent_impl(&conn, &req, &agent_dir, &agent_id, &workspace_path)
            .expect_err("empty name should error");
        assert!(err.contains("cannot be empty"), "error should mention empty name: {}", err);
    }

    // --- set_default_agent_impl (renamed from set_main_agent_impl, #2522 ADR-0063 D3) ---

    #[test]
    fn test_set_default_agent_promotes_and_demotes() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-a", "Agent A", true, true, None);  // current default
        seed_agent(&conn, "agent-b", "Agent B", false, true, None); // will be promoted

        set_default_agent_impl(&conn, "agent-b")
            .expect("set_default_agent_impl should succeed");

        // Verify exactly one default
        let default_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ai_agents WHERE is_default = 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(default_count, 1, "exactly one is_default=1 row must exist after set_default_agent");

        let new_default_id: String = conn.query_row(
            "SELECT id FROM ai_agents WHERE is_default = 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(new_default_id, "agent-b");

        // Old default must be demoted
        let old_is_default: i32 = conn.query_row(
            "SELECT is_default FROM ai_agents WHERE id = 'agent-a'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(old_is_default, 0, "previous default must be demoted");
    }

    #[test]
    fn test_set_default_agent_rejects_inactive() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-a", "Agent A", true, true, None);
        seed_agent(&conn, "inactive-x", "Inactive X", false, false, None);

        let err = set_default_agent_impl(&conn, "inactive-x")
            .expect_err("should error for inactive agent");
        assert!(err.contains("not active"), "error should mention not active: {}", err);

        // Original default must still be default
        let default_id: String = conn.query_row(
            "SELECT id FROM ai_agents WHERE is_default = 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(default_id, "agent-a", "original default must be preserved");
    }

    #[test]
    fn test_set_default_agent_rollback_on_missing_target() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-a", "Agent A", true, true, None);

        let err = set_default_agent_impl(&conn, "does-not-exist")
            .expect_err("should error for missing agent");
        assert!(!err.is_empty(), "should have an error message");

        // Original default must still be default (transaction rolled back)
        let default_id: String = conn.query_row(
            "SELECT id FROM ai_agents WHERE is_default = 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(default_id, "agent-a", "original default must be preserved on rollback");
    }

    #[test]
    fn test_set_default_agent_rejects_subagent() {
        // #2567: restore the role='sub' exclusion (#2271) at the command level. The old
        // "Set as Main" buttons filtered role='sub' agents in the UI (Settings.svelte);
        // that filter was dropped when DefaultAgentPickerModal replaced them. The guard
        // now lives in set_default_agent_impl itself so every caller is covered.
        //
        // #B5 (Reed's PR #2577 audit): this fixture must use a REAL subagent shape —
        // role='sub' AND a non-null runtime_ref (i.e. imported from OC). A role='sub'
        // row with runtime_ref=NULL is a locally-created agent, not a real subagent,
        // and must NOT be rejected (see test_set_default_agent_allows_local_sub_role).
        let conn = make_test_conn();
        seed_agent(&conn, "agent-a", "Agent A", true, true, None); // main, current default
        seed_agent_with_role(&conn, "sub-1", "Reed", false, true, Some("oc-remote-sub-1"), "sub");

        let err = set_default_agent_impl(&conn, "sub-1")
            .expect_err("should error when target agent has role='sub'");
        assert!(
            err.contains("Subagents cannot be set as default"),
            "error should explain subagents are ineligible: {}", err
        );

        // Original default must be untouched
        let default_id: String = conn.query_row(
            "SELECT id FROM ai_agents WHERE is_default = 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(default_id, "agent-a", "original default must be preserved when rejecting a subagent");
    }

    #[test]
    fn test_set_default_agent_allows_local_sub_role() {
        // #B5 (Reed's PR #2577 audit): role='sub' alone is too broad a predicate —
        // locally-created agents can share role='sub' but have runtime_ref = NULL
        // (they were never imported from OC, so they aren't real subagents). The
        // guard must only reject role='sub' agents that ALSO have a runtime_ref set.
        // This test seeds a locally-created shape (role='sub', runtime_ref=NULL) and
        // asserts promotion succeeds.
        let conn = make_test_conn();
        seed_agent(&conn, "agent-a", "Agent A", true, true, None); // main, current default
        seed_agent_with_role(&conn, "local-sub-1", "Local Sub", false, true, None, "sub");

        set_default_agent_impl(&conn, "local-sub-1")
            .expect("set_default_agent_impl should succeed for locally-created role='sub' agent with no runtime_ref");

        let default_id: String = conn.query_row(
            "SELECT id FROM ai_agents WHERE is_default = 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(default_id, "local-sub-1", "locally-created role='sub' agent must be promotable");
    }

    #[test]
    fn test_set_default_agent_allows_role_local() {
        // #2567 B6: role='local' is the new birth label for locally-created agents
        // (was role='sub' + runtime_ref=NULL workaround, see the test above). This
        // proves the B5 guard doesn't block the new role value either — role='local'
        // never matches the `role == "sub"` rejection arm, so no runtime_ref conjunct
        // is even needed for this case.
        let conn = make_test_conn();
        seed_agent(&conn, "agent-a", "Agent A", true, true, None); // main, current default
        seed_agent_with_role(&conn, "local-1", "Local One", false, true, None, "local");

        set_default_agent_impl(&conn, "local-1")
            .expect("set_default_agent_impl should succeed for role='local' agent");

        let default_id: String = conn.query_row(
            "SELECT id FROM ai_agents WHERE is_default = 1",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(default_id, "local-1", "role='local' agent must be promotable");
    }

    // --- Item 3: eager logs/ creation ---

    #[test]
    fn test_create_local_agent_creates_logs_dir() {
        // Regression guard: create_local_agent_impl must create logs/ because the new agent
        // is always promoted to main (Item 3 fix, #2030).
        let conn = make_test_conn();
        let tmp = TempDir::new().unwrap();
        let agent_uuid = uuid::Uuid::new_v4();
        let agent_id = agent_uuid.to_string();
        let workspace_path = agent_uuid.simple().to_string().to_uppercase();
        let agent_dir = tmp.path().join(&workspace_path);

        let req = CreateLocalAgentRequest {
            name: "Logs Dir Test".to_string(),
            model: None,
            description: None,
            runtime_type: "openrouter".to_string(),
        };
        create_local_agent_impl(&conn, &req, &agent_dir, &agent_id, &workspace_path)
            .expect("create should succeed");

        assert!(
            agent_dir.join("logs").is_dir(),
            "logs/ directory must be created alongside the agent (Item 3 fix)"
        );
    }

    // seed_agent_uploads / seed_agent_interview_impl tests deleted alongside
    // their functions (#2288 PR 3) — dead code, no test surface to keep.

    // --- set_legacy_runtime_type_impl (ADR-0063 D4.3 / #2522 Stage 7c) ---

    /// Seeds a legacy-unpinned agent row: runtime_ref NULL AND runtime_type NULL,
    /// the exact D4.3 shape migrate_v79 can leave behind on old local agents.
    fn seed_legacy_agent(conn: &Connection, id: &str, name: &str) {
        let slug = generate_slug(name);
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path, config_json, source, runtime_ref, runtime_type, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'sub', 0, 1, ?4, '{}', 'import', NULL, NULL, ?5, ?5)",
            params![id, name, slug, format!("WP{}", id), now],
        ).expect("seed legacy agent");
    }

    #[test]
    fn test_set_legacy_runtime_type_pins_null_row() {
        let conn = make_test_conn();
        seed_legacy_agent(&conn, "legacy-1", "Murphy Brown");

        set_legacy_runtime_type_impl(&conn, "legacy-1", "anthropic")
            .expect("pinning a NULL runtime_type should succeed");

        let runtime_type: Option<String> = conn.query_row(
            "SELECT runtime_type FROM ai_agents WHERE id = 'legacy-1'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(runtime_type.as_deref(), Some("anthropic"));
    }

    #[test]
    fn test_set_legacy_runtime_type_rejects_already_pinned() {
        let conn = make_test_conn();
        // seed_agent's non-legacy fixture always writes a non-NULL runtime_type
        // (see its own doc comment) — exactly the "already pinned" case here.
        seed_agent(&conn, "pinned-1", "Already Pinned", false, true, None);

        let err = set_legacy_runtime_type_impl(&conn, "pinned-1", "ollama")
            .expect_err("pinning an already-pinned row must be rejected");
        assert!(err.contains("immutable"), "error should explain immutability: {}", err);

        // Original value (from seed_agent: 'openrouter', runtime_ref None) must be untouched
        let runtime_type: Option<String> = conn.query_row(
            "SELECT runtime_type FROM ai_agents WHERE id = 'pinned-1'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert_eq!(runtime_type.as_deref(), Some("openrouter"), "already-pinned runtime_type must not change");
    }

    #[test]
    fn test_set_legacy_runtime_type_rejects_invalid_value() {
        let conn = make_test_conn();
        seed_legacy_agent(&conn, "legacy-2", "Bad Value Agent");

        let err = set_legacy_runtime_type_impl(&conn, "legacy-2", "not-a-real-runtime")
            .expect_err("invalid runtime_type string must be rejected");
        assert!(err.contains("Invalid runtime_type"), "error should name the bad field: {}", err);

        let runtime_type: Option<String> = conn.query_row(
            "SELECT runtime_type FROM ai_agents WHERE id = 'legacy-2'",
            [],
            |row| row.get(0),
        ).unwrap();
        assert!(runtime_type.is_none(), "row must stay NULL when the value is rejected");
    }

    // --- check_bound_agent_active (D21, #2649) ---

    #[test]
    fn test_check_bound_agent_active_noop_when_agent_active() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-1", "Active Agent", true, true, None);
        conn.execute(
            "INSERT INTO chat_sessions (id, agent_id) VALUES ('session-1', 'agent-1')",
            [],
        ).unwrap();

        let result = check_bound_agent_active(&conn, "session-1");
        assert!(result.is_ok(), "an active bound agent must not error: {:?}", result);
    }

    #[test]
    fn test_check_bound_agent_active_errors_when_agent_inactive() {
        let conn = make_test_conn();
        seed_agent(&conn, "agent-1", "Locked Agent", false, false, None);
        conn.execute(
            "INSERT INTO chat_sessions (id, agent_id) VALUES ('session-1', 'agent-1')",
            [],
        ).unwrap();

        let err = check_bound_agent_active(&conn, "session-1")
            .expect_err("a session bound to an is_active=0 agent must error");
        assert!(
            err.contains("Locked Agent") && err.contains("inactive"),
            "error must name the bound agent and state it's inactive, got: {}",
            err
        );
    }

    #[test]
    fn test_check_bound_agent_active_noop_when_agent_id_null() {
        let conn = make_test_conn();
        conn.execute(
            "INSERT INTO chat_sessions (id, agent_id) VALUES ('session-1', NULL)",
            [],
        ).unwrap();

        let result = check_bound_agent_active(&conn, "session-1");
        assert!(result.is_ok(), "NULL agent_id is self-heal's job, not this guard's: {:?}", result);
    }

    #[test]
    fn test_check_bound_agent_active_noop_when_bound_agent_row_missing() {
        let conn = make_test_conn();
        // Disable FK enforcement for this one insert only — this scenario models a
        // dangling reference (parent row deleted out from under the session), which
        // the real schema permits since chat_sessions.agent_id has no ON DELETE
        // action; hand-rolling it here without a real delete keeps the fixture simple.
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute(
            "INSERT INTO chat_sessions (id, agent_id) VALUES ('session-1', 'ghost-agent')",
            [],
        ).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        let result = check_bound_agent_active(&conn, "session-1");
        assert!(
            result.is_ok(),
            "a dangling agent_id (row deleted) is out of this guard's scope, downstream resolution surfaces its own error: {:?}",
            result
        );
    }

    /// R6 grep-gate (#2522, ADR-0062 D1/D3): `chat_sessions.agent_id` is
    /// immutable once bound. `resolve_and_selfheal_session_agent` (this file)
    /// is the *one* sanctioned post-INSERT write path for that column — every
    /// other `UPDATE chat_sessions SET ... agent_id ...` in the source tree
    /// (outside migrations, which set it once at row-creation-adjacent
    /// backfill time, and tests) would be a binding-mutation regression.
    /// This mechanically enforces the invariant instead of relying on
    /// convention: any future code that reintroduces a second write path
    /// fails this test, not just code review.
    #[test]
    fn test_r6_agent_id_has_single_sanctioned_write_path() {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set during `cargo test`");
        let src_dir = std::path::Path::new(&manifest_dir).join("src");

        // Allowlist: files permitted to contain an `agent_id`-setting UPDATE/SET
        // against chat_sessions, and why.
        //   - agents.rs: resolve_and_selfheal_session_agent, the sanctioned
        //     self-heal write path itself (this file).
        //   - migrations.rs: migrate_v78's one-time backfill of the newly
        //     added column on upgrade — not a runtime mutation of an already-
        //     bound session.
        let allowlisted_files = ["agents.rs", "migrations.rs"];

        // Pattern is intentionally loose (matches the SET clause, not a full
        // statement) so reformatting the SQL string doesn't silently defeat
        // the gate. Case-sensitive on SQL keywords per this codebase's
        // existing convention (uppercase SQL).
        let pattern_a = "chat_sessions";
        let pattern_b = "SET agent_id";

        let mut violations: Vec<String> = Vec::new();

        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }

        let mut rs_files = Vec::new();
        walk(&src_dir, &mut rs_files);

        for path in rs_files {
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if allowlisted_files.contains(&file_name) {
                continue;
            }
            // Test modules and files are allowed to build hand-rolled fixture
            // rows for their own scenarios — they don't touch a real session's
            // permanent binding.
            if file_name.starts_with("test_") || file_name.ends_with("_tests.rs") {
                continue;
            }

            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            // Strip line comments so commented-out examples don't trip the gate.
            let content_no_comments: String = content
                .lines()
                .map(|line| match line.find("//") {
                    Some(idx) => &line[..idx],
                    None => line,
                })
                .collect::<Vec<_>>()
                .join("\n");

            if content_no_comments.contains(pattern_a) && content_no_comments.contains(pattern_b) {
                violations.push(path.display().to_string());
            }
        }

        assert!(
            violations.is_empty(),
            "Found chat_sessions.agent_id write(s) outside the sanctioned self-heal \
             path (agents.rs) and migration backfill (migrations.rs) in: {:?}. \
             Per ADR-0062 D1/D3, agent_id is immutable once bound - route new \
             writes through resolve_and_selfheal_session_agent, or add the file \
             to this test's allowlist with a comment explaining why it's safe.",
            violations
        );
    }

    // ── #2629 (D7 fix): upsert_agent honors an explicit runtime_type ──
    // Full end-to-end coverage through the real #[tauri::command] handler (not just
    // the inner logic) via tauri::test::mock_app + app.manage(DbState), on a
    // fresh-migrated DB with a mock keychain holding BOTH anthropic and openrouter
    // keys present (the exact repro state from Reed's audit — leftover-key state IS
    // the repro, so it is never mocked clean). Asserts explicit runtime_type/model
    // win outright over ambient keychain inference, and that omitting runtime_type
    // preserves the pre-#2629 inference fallback (backward compat for every other
    // caller enumerated in the PR description).
    mod upsert_agent_explicit_runtime_type_tests {
        use super::*;
        use crate::keychain::test_support::KeychainGuard;
        use serial_test::serial;
        use tempfile::TempDir;
        use tauri::Manager;

        /// Build a real tauri::State<DbState> backed by a fresh-migrated in-memory
        /// DB, with PANE_TEST_ROOT pointed at a scratch filesystem dir (upsert_agent
        /// writes IDENTITY.md/SOUL.md/MEMORY.md under .pane/system before the DB
        /// write — ADR-0039 D7 — so a real filesystem root is required, not just a DB).
        /// Returns the TempDir guards alongside the app so callers can keep them
        /// alive for the duration of the test (dropping deletes the scratch dir).
        fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
            let fs_dir = TempDir::new().expect("failed to create scratch fs dir");
            std::env::set_var("PANE_TEST_ROOT", fs_dir.path());

            let conn = Connection::open_in_memory().expect("in-memory db");
            crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

            let app = tauri::test::mock_app();
            app.manage(crate::database::DbState::new(conn));
            (app, fs_dir)
        }

        fn teardown() {
        }

        fn seed_both_provider_keys(conn: &Connection) {
            // The repro state from Reed's audit: BOTH keys present simultaneously
            // (e.g. a freshly-validated Anthropic key alongside a leftover OpenRouter
            // key from a prior install, D8 #2631). Left deliberately dirty — NOT
            // mocked clean — because that leftover-key state is the whole point:
            // the test must prove explicit runtime_type wins even here.
            crate::ai::credential_migration::write_provider_key(conn, "anthropic", "sk-ant-fresh-choice")
                .expect("seed anthropic key");
            crate::ai::credential_migration::write_provider_key(conn, "openrouter", "sk-or-leftover-from-prior-install")
                .expect("seed openrouter key");
        }

        #[test]
        #[serial(pane_test_root, keychain_override)]
        fn test_explicit_anthropic_runtime_type_wins_over_openrouter_tie_break() {
            let _kc = KeychainGuard::set();
            let (app, _fs_dir) = setup();

            {
                let conn_state = app.state::<crate::database::DbState>();
                let conn = conn_state.0.lock().unwrap();
                seed_both_provider_keys(&conn);
                // Sanity: without the fix, resolve_runtime_type_for_new_local_agent would
                // tie-break both-present straight to openrouter (Matt's kept tie-break) —
                // confirming the ambient state really would misfire if not overridden.
                assert_eq!(
                    resolve_runtime_type_for_new_local_agent(&conn),
                    "openrouter",
                    "sanity: ambient inference must still tie-break to openrouter when both keys exist"
                );
            }

            let db_state = app.state::<crate::database::DbState>();
            let result = upsert_agent(
                CreateAgentRequest {
                    name: "Main Agent".to_string(),
                    role: Some("main".to_string()),
                    description: None,
                    model: Some("claude-opus-4-6".to_string()), // CI-ALLOW: hardcoded-model
                    is_default: Some(true),
                    workspace_path: None,
                    source: Some("onboarding".to_string()),
                    runtime_type: Some("anthropic".to_string()),
                },
                db_state,
            )
            .expect("upsert_agent should succeed");

            assert_eq!(
                result.runtime_type.as_deref(),
                Some("anthropic"),
                "explicit runtime_type ('anthropic', the user's actual choice) must win over \
                 ambient keychain inference (which would have produced 'openrouter'), even \
                 though BOTH provider keys are present in the keychain"
            );
            assert_eq!(
                result.model.as_deref(),
                Some("claude-opus-4-6"), // CI-ALLOW: hardcoded-model
                "explicit model must be written to the agent row, not left NULL and not \
                 read back from the retired ai_settings.model zombie key"
            );

            teardown();
        }

        #[test]
        #[serial(pane_test_root, keychain_override)]
        fn test_explicit_openrouter_runtime_type_persists_with_openrouter_model() {
            let _kc = KeychainGuard::set();
            let (app, _fs_dir) = setup();

            {
                let conn_state = app.state::<crate::database::DbState>();
                let conn = conn_state.0.lock().unwrap();
                seed_both_provider_keys(&conn);
            }

            let db_state = app.state::<crate::database::DbState>();
            let result = upsert_agent(
                CreateAgentRequest {
                    name: "Main Agent".to_string(),
                    role: Some("main".to_string()),
                    description: None,
                    model: Some("anthropic/claude-3.5-sonnet".to_string()), // CI-ALLOW: hardcoded-model
                    is_default: Some(true),
                    workspace_path: None,
                    source: Some("onboarding".to_string()),
                    runtime_type: Some("openrouter".to_string()),
                },
                db_state,
            )
            .expect("upsert_agent should succeed");

            assert_eq!(
                result.runtime_type.as_deref(),
                Some("openrouter"),
                "explicit runtime_type must be honored for the openrouter provider too \
                 (this test guards against a fix that only special-cases anthropic)"
            );
            assert_eq!(
                result.model.as_deref(),
                Some("anthropic/claude-3.5-sonnet"), // CI-ALLOW: hardcoded-model
                "explicit model (an OpenRouter-style model slug) must be written to the agent row"
            );

            teardown();
        }

        #[test]
        #[serial(pane_test_root, keychain_override)]
        fn test_none_runtime_type_falls_back_to_inference_backward_compat() {
            // Backward compat: a caller that doesn't yet know its runtime (every existing
            // caller enumerated in the PR description) must see IDENTICAL behavior to
            // before #2629 — i.e. still inferred from ambient keychain state.
            let _kc = KeychainGuard::set();
            let (app, _fs_dir) = setup();

            {
                let conn_state = app.state::<crate::database::DbState>();
                let conn = conn_state.0.lock().unwrap();
                // Only an anthropic key present this time — unambiguous inference case.
                crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-only-key")
                    .expect("seed anthropic key");
            }

            let db_state = app.state::<crate::database::DbState>();
            let result = upsert_agent(
                CreateAgentRequest {
                    name: "Imported Agent".to_string(),
                    role: Some("local".to_string()),
                    description: None,
                    model: None,
                    is_default: Some(false),
                    workspace_path: None,
                    source: Some("import".to_string()),
                    runtime_type: None,
                },
                db_state,
            )
            .expect("upsert_agent should succeed");

            assert_eq!(
                result.runtime_type.as_deref(),
                Some("anthropic"),
                "runtime_type: None must fall back to resolve_runtime_type_for_new_local_agent's \
                 keychain-presence inference — unchanged pre-#2629 behavior for callers that \
                 don't yet pass an explicit runtime_type"
            );

            teardown();
        }

        #[test]
        #[serial(pane_test_root, keychain_override)]
        fn test_invalid_explicit_runtime_type_is_rejected_no_write() {
            let _kc = KeychainGuard::set();
            let (app, _fs_dir) = setup();

            let db_state = app.state::<crate::database::DbState>();
            let err = upsert_agent(
                CreateAgentRequest {
                    name: "Bad Agent".to_string(),
                    role: Some("main".to_string()),
                    description: None,
                    model: None,
                    is_default: Some(true),
                    workspace_path: None,
                    source: Some("onboarding".to_string()),
                    runtime_type: Some("not-a-real-provider".to_string()),
                },
                db_state,
            )
            .expect_err("an invalid explicit runtime_type must be rejected, never silently inferred instead");
            assert!(err.contains("Invalid runtime_type"), "unexpected error message: {}", err);

            {
                let conn_state = app.state::<crate::database::DbState>();
                let conn = conn_state.0.lock().unwrap();
                let count: i64 = conn
                    .query_row("SELECT COUNT(*) FROM ai_agents WHERE name = 'Bad Agent'", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 0, "no row should be written when the explicit runtime_type is invalid");
            }

            teardown();
        }

        /// #2649 (PR 3C) D19 closure regression test — mechanical guard version.
        ///
        /// D19's root cause (see the `runOcSync` doc comment above and PR #2649's
        /// description) was Onboarding calling `ensureMainAgent()` — which invokes
        /// this same `upsert_agent` command — BEFORE the OC import ran, minting a
        /// `runtime_type='openclaw', runtime_ref=NULL` row that never collided with
        /// the import's own `runtime_ref='main'` row (two "main" agents, D19's bug).
        ///
        /// PR 3A/3B removed every known live call site that reached `upsert_agent`
        /// with `runtime_type: 'openclaw'`, but "no live caller does this" is
        /// discipline, not a mechanism — it proves the shape is currently unreached,
        /// not that it's blocked. PR 3C closes that gap: `upsert_agent` now rejects
        /// `runtime_type: 'openclaw'` outright, before any DB write or filesystem
        /// side effect. OC agent rows are created exclusively by
        /// `sync_initial_import` / `sync_re_import` (both in `sync_client.rs` /
        /// `ai/commands.rs`, which INSERT `ai_agents` rows with `runtime_ref` set
        /// directly), never through `upsert_agent`.
        ///
        /// This test proves the guard is mechanical: calling `upsert_agent` directly
        /// with `runtime_type: Some("openclaw")` returns an error, and zero rows are
        /// written to `ai_agents` — the rejection happens before the phantom
        /// `runtime_ref=NULL` row can ever be minted, regardless of what any future
        /// caller does or doesn't do.
        #[test]
        #[serial(pane_test_root, keychain_override)]
        fn test_upsert_agent_rejects_openclaw_runtime_type_before_any_write() {
            let _kc = KeychainGuard::set();
            let (app, _fs_dir) = setup();

            let db_state = app.state::<crate::database::DbState>();
            let err = upsert_agent(
                CreateAgentRequest {
                    name: "Phantom OC Agent".to_string(),
                    role: Some("main".to_string()),
                    description: None,
                    model: None,
                    is_default: Some(true),
                    workspace_path: None,
                    source: Some("onboarding".to_string()),
                    runtime_type: Some("openclaw".to_string()),
                },
                db_state,
            )
            .expect_err(
                "upsert_agent must mechanically reject runtime_type: 'openclaw' — OC agent rows \
                 come only from sync_initial_import/sync_re_import, never from upsert_agent \
                 (D19, #2649)",
            );

            assert!(
                err.contains("openclaw") && err.contains("upsert_agent"),
                "unexpected error message: {}",
                err
            );

            {
                let conn_state = app.state::<crate::database::DbState>();
                let conn = conn_state.0.lock().unwrap();
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM ai_agents WHERE name = 'Phantom OC Agent'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    count, 0,
                    "the guard must fire before any DB write — zero rows should exist for the \
                     rejected agent, proving the phantom runtime_ref=NULL row was never minted"
                );
            }

            teardown();
        }
    }

    // ── #3074 part 1: update_agent's `model` field, through the real command ──
    // Reed's scope audit (2026-10-24-scope-3074-agent-model-edit.md) found no existing
    // Rust test exercises `update_agent`'s `model` field at all — the backend half the
    // frontend model-editable gate (`isAgentModelEditable`, src/lib/utils/agentDisplay.ts)
    // depends on was unguarded. Builds its fixture agent via the real `create_local_agent`
    // command (never raw INSERT, same discipline the `upsert_agent_explicit_runtime_type_tests`
    // module above follows), then calls the real `update_agent` command and reads the
    // result straight back — proving the write path specifically for `model`, not just
    // that the two halves (write + read) compose correctly because each is tested alone.
    mod update_agent_model_field_tests {
        use super::*;
        use tempfile::TempDir;
        use tauri::Manager;
        use serial_test::serial;

        /// Same shape as `upsert_agent_explicit_runtime_type_tests::setup` above — a real
        /// `tauri::State<DbState>` via `tauri::test::mock_app`, fresh-migrated in-memory DB,
        /// PANE_TEST_ROOT pointed at a scratch dir (`create_local_agent` writes IDENTITY.md/
        /// SOUL.md/MEMORY.md under `.pane/system` before the DB write, ADR-0039 D7, so a real
        /// filesystem root is required even though this test's assertions are DB-only).
        fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
            let fs_dir = TempDir::new().expect("failed to create scratch fs dir");
            std::env::set_var("PANE_TEST_ROOT", fs_dir.path());

            let conn = Connection::open_in_memory().expect("in-memory db");
            crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

            let app = tauri::test::mock_app();
            app.manage(crate::database::DbState::new(conn));
            (app, fs_dir)
        }

        fn teardown() {
        }

        #[test]
        #[serial(pane_test_root)]
        fn test_update_agent_persists_model_field_through_real_command() {
            let (app, _fs_dir) = setup();
            let db_state = app.state::<crate::database::DbState>();

            // Create via the real command — an API-runtime agent (one of the four
            // `isAgentModelEditable` allows), with an initial model.
            let created = create_local_agent(
                CreateLocalAgentRequest {
                    name: "Model Edit Test Agent".to_string(),
                    model: Some("anthropic/claude-sonnet-4-5".to_string()), // CI-ALLOW: hardcoded-model
                    description: None,
                    runtime_type: "openrouter".to_string(),
                },
                db_state,
            )
            .expect("create_local_agent should succeed");
            assert_eq!(
                created.model.as_deref(),
                Some("anthropic/claude-sonnet-4-5"), // CI-ALLOW: hardcoded-model
                "sanity: initial model must be whatever create_local_agent was given"
            );

            // Update via the real command — model only, same explicit-payload shape
            // the frontend's eventual saveAgentModel() must use (never a spread of
            // the full agent; see the scope audit's §2 discipline note).
            let db_state = app.state::<crate::database::DbState>();
            let updated = update_agent(
                UpdateAgentRequest {
                    agent_id: created.id.clone(),
                    name: None,
                    description: None,
                    model: Some("anthropic/claude-opus-4-6".to_string()), // CI-ALLOW: hardcoded-model
                    is_active: None,
                    runtime_type: None,
                },
                db_state,
            )
            .expect("update_agent should succeed");

            assert_eq!(
                updated.model.as_deref(),
                Some("anthropic/claude-opus-4-6"), // CI-ALLOW: hardcoded-model
                "update_agent's model field must persist and be readable back through the real command round-trip"
            );
            assert_eq!(
                updated.runtime_type.as_deref(),
                Some("openrouter"),
                "updating model must never change runtime_type (ADR-0063 D1: immutable after creation)"
            );
            assert_eq!(
                updated.id, created.id,
                "update_agent must return the same agent, not a new row"
            );

            teardown();
        }

        // ── #3074 part 2: a model change takes effect on the very next dispatch ──
        // Proves the two halves (write via update_agent, read via
        // resolve_runtime_config_for_agent_id) compose end-to-end on the same
        // connection, rather than trusting each is correct in isolation because
        // each has its own passing test elsewhere. Reed's scope audit confirmed
        // resolve_runtime_config_for_agent_id runs a fresh SELECT with no cache
        // layer in front of it — this is the test that proves that claim, not
        // just reads the doc comment that states it.
        #[test]
        #[serial(pane_test_root, keychain_override)]
        fn test_update_agent_model_change_is_read_by_next_dispatch_resolution() {
            use crate::keychain::test_support::KeychainGuard;
            let _kc = KeychainGuard::set();
            let (app, _fs_dir) = setup();

            {
                let conn_state = app.state::<crate::database::DbState>();
                let conn = conn_state.0.lock().unwrap();
                crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-test-key")
                    .expect("seed openrouter key");
            }

            let db_state = app.state::<crate::database::DbState>();
            let created = create_local_agent(
                CreateLocalAgentRequest {
                    name: "Dispatch Read Test Agent".to_string(),
                    model: Some("anthropic/claude-sonnet-4-5".to_string()), // CI-ALLOW: hardcoded-model
                    description: None,
                    runtime_type: "openrouter".to_string(),
                },
                db_state,
            )
            .expect("create_local_agent should succeed");

            let db_state = app.state::<crate::database::DbState>();
            update_agent(
                UpdateAgentRequest {
                    agent_id: created.id.clone(),
                    name: None,
                    description: None,
                    model: Some("anthropic/claude-opus-4-6".to_string()), // CI-ALLOW: hardcoded-model
                    is_active: None,
                    runtime_type: None,
                },
                db_state,
            )
            .expect("update_agent should succeed");

            let conn_state = app.state::<crate::database::DbState>();
            let conn = conn_state.0.lock().unwrap();
            let cfg = crate::ai::runtime_resolution::resolve_runtime_config_for_agent_id(
                &conn,
                &created.id,
            )
            .expect("dispatch-time resolution should succeed for a credentialed openrouter agent");

            assert_eq!(
                cfg.model, "anthropic/claude-opus-4-6", // CI-ALLOW: hardcoded-model
                "the very next dispatch resolution must read the NEW model, not the one the \
                 agent was created with — proving no cache sits between update_agent's write \
                 and resolve_runtime_config_for_agent_id's read"
            );

            drop(conn);
            teardown();
        }

        // ── #3074 part 2: the backend has no runtime_type gate — the frontend's ──
        // isAgentModelEditable() is load-bearing, not cosmetic.
        //
        // D9 (ADR-0063): for an 'openclaw' agent, ai_agents.model is a write-TARGET for
        // write_back_agent_model_if_changed's post-dispatch cache, not a dispatch-time
        // input the OC gateway honors — the gateway resolves its own model from its own
        // registry. If a model picker were ever shown for an openclaw-runtime agent, the
        // write would succeed here (nothing in update_agent gates on runtime_type) but
        // be dispatch-inert and then silently reverted on the next clean OC-path turn.
        // This test proves the backend accepts the write anyway, which is exactly why
        // the frontend gate must exist and must not be "fixed" away as redundant.
        #[test]
        #[serial(pane_test_root)]
        fn test_update_agent_accepts_model_write_for_openclaw_runtime_backend_has_no_gate() {
            let (app, _fs_dir) = setup();
            let db_state = app.state::<crate::database::DbState>();

            let created = create_local_agent(
                CreateLocalAgentRequest {
                    name: "OpenClaw Model Write Test Agent".to_string(),
                    model: Some("openclaw-pending".to_string()),
                    description: None,
                    runtime_type: "openclaw".to_string(),
                },
                db_state,
            )
            .expect("create_local_agent should succeed for runtime_type='openclaw'");

            let db_state = app.state::<crate::database::DbState>();
            let updated = update_agent(
                UpdateAgentRequest {
                    agent_id: created.id.clone(),
                    name: None,
                    description: None,
                    model: Some("anthropic/claude-opus-4-6".to_string()), // CI-ALLOW: hardcoded-model
                    is_active: None,
                    runtime_type: None,
                },
                db_state,
            )
            .expect(
                "update_agent has no runtime_type gate today — this write must succeed at the \
                 backend. The reason a user is never allowed to trigger this is the frontend's \
                 isAgentModelEditable() gate (agentDisplay.ts), not anything here.",
            );

            assert_eq!(
                updated.model.as_deref(),
                Some("anthropic/claude-opus-4-6"), // CI-ALLOW: hardcoded-model
                "backend accepted the model write for an openclaw-runtime agent — confirms the \
                 frontend gate is the only thing standing between this call and a self-correcting \
                 -into-wrong UX (D9)"
            );
            assert_eq!(
                updated.runtime_type.as_deref(),
                Some("openclaw"),
                "runtime_type must remain untouched regardless of the model write"
            );

            teardown();
        }
    }

    // ── #2564 B1 fix (Path B), item #2: resolve_runtime_type_for_new_local_agent ──
    // Re-pointed off the retired `ai_settings.provider` scalar onto a keychain +
    // active-agent-row signal. These tests seed via the real keychain seam
    // (MockKeychain) rather than `ai_settings.provider`, confirming the function
    // resolves correctly with `provider` completely absent.

    mod resolve_runtime_type_for_new_local_agent_tests {
        use super::*;
        use crate::keychain::test_support::KeychainGuard;
        use serial_test::serial;

        #[test]
        #[serial(keychain_override)]
        fn test_no_credentials_no_agents_defaults_to_openrouter() {
            let _kc = KeychainGuard::set();
            let conn = make_test_conn();
            assert_eq!(resolve_runtime_type_for_new_local_agent(&conn), "openrouter");
        }

        #[test]
        #[serial(keychain_override)]
        fn test_provider_key_present_no_active_agents_defaults_to_openrouter() {
            let _kc = KeychainGuard::set();
            let conn = make_test_conn();
            crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-test-key").unwrap();
            assert_eq!(resolve_runtime_type_for_new_local_agent(&conn), "openrouter");
        }

        #[test]
        #[serial(keychain_override)]
        fn test_provider_key_present_active_anthropic_agent_picks_anthropic() {
            let _kc = KeychainGuard::set();
            let conn = make_test_conn();
            crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-test").unwrap();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_type, is_active, created_at, updated_at) \
                 VALUES ('a1', 'Main', 'main', 'WP1', 'anthropic', 1, '', '')",
                [],
            ).unwrap();
            assert_eq!(resolve_runtime_type_for_new_local_agent(&conn), "anthropic");
        }

        #[test]
        #[serial(keychain_override)]
        fn test_provider_key_present_active_openrouter_agent_picks_openrouter() {
            let _kc = KeychainGuard::set();
            let conn = make_test_conn();
            crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-test").unwrap();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_type, is_active, created_at, updated_at) \
                 VALUES ('a1', 'Main', 'main', 'WP1', 'openrouter', 1, '', '')",
                [],
            ).unwrap();
            assert_eq!(resolve_runtime_type_for_new_local_agent(&conn), "openrouter");
        }

        /// B3 (#2565) rewrite: the old "both providers active + one SHARED keychain
        /// slot" state is no longer representable now that anthropic/openrouter
        /// each have their own slot. This test now covers the genuinely-new
        /// equivalent state — BOTH providers have their OWN independent keys
        /// configured — which still needs a deterministic tie-break for "which
        /// runtime does a brand-new agent land on", per Matt's ruling to keep the
        /// default-to-openrouter tie-break for this case.
        #[test]
        #[serial(keychain_override)]
        fn test_both_provider_keys_present_defaults_to_openrouter() {
            let _kc = KeychainGuard::set();
            let conn = make_test_conn();
            crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-independent").unwrap();
            crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-independent").unwrap();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_type, is_active, created_at, updated_at) \
                 VALUES ('a1', 'Main', 'main', 'WP1', 'anthropic', 1, '', '')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_type, is_active, created_at, updated_at) \
                 VALUES ('a2', 'Second', 'second', 'WP2', 'openrouter', 1, '', '')",
                [],
            ).unwrap();
            assert_eq!(
                resolve_runtime_type_for_new_local_agent(&conn),
                "openrouter",
                "both-configured case must default to openrouter (Matt's ruling: keep the tie-break)"
            );

            // Confirm both keys are genuinely independent — this is the state
            // that was previously unrepresentable with a shared slot.
            assert_eq!(
                crate::ai::credential_migration::read_provider_key(&conn, "anthropic").as_deref(),
                Some("sk-ant-independent")
            );
            assert_eq!(
                crate::ai::credential_migration::read_provider_key(&conn, "openrouter").as_deref(),
                Some("sk-or-independent")
            );
        }

        #[test]
        #[serial(keychain_override)]
        fn test_no_provider_key_active_ollama_agent_picks_ollama() {
            let _kc = KeychainGuard::set();
            let conn = make_test_conn();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_type, is_active, created_at, updated_at) \
                 VALUES ('a1', 'Local', 'local', 'WP1', 'ollama', 1, '', '')",
                [],
            ).unwrap();
            assert_eq!(resolve_runtime_type_for_new_local_agent(&conn), "ollama");
        }

        #[test]
        #[serial(keychain_override)]
        fn test_resolves_without_provider_key_in_ai_settings_when_openclaw_agent_active() {
            // Regression guard: 'provider' must never be read from ai_settings at all
            // — confirm this by inserting an active openclaw agent (no credential
            // signal applies to openclaw here since it's not one of the two
            // credential-gated arms) and asserting the function still returns a sane,
            // non-openclaw default without erroring or touching ai_settings.
            let _kc = KeychainGuard::set();
            let conn = make_test_conn();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_type, is_active, created_at, updated_at) \
                 VALUES ('a1', 'OC', 'oc', 'WP1', 'openclaw', 1, '', '')",
                [],
            ).unwrap();
            // ai_settings.provider is never written anywhere in this test — table
            // stays empty, which is exactly the post-fix production state.
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_settings", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "sanity: ai_settings must stay untouched");
            assert_eq!(resolve_runtime_type_for_new_local_agent(&conn), "openrouter");
        }
    }
}
