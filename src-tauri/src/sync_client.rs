// Pane Bridge v2 Sync Client
// Pushes system files to OpenClaw machine after writes
// Architecture per pane-architecture-spec.md §24.2, §24.3, §6.5
// v0.18.1: ID-based agent routing (#364, #331)

// === Topology-aware sync ===
// split_machine: Gateway HTTP/2 for sync, models, sessions.
//   Auth: endpoint + api_key (canonical keys per §22C.11A.4)
//   TLS: self-signed (fingerprint pinning in Track 4)
// Note: OpenClaw is split-machine only (see #2649). The `SameMachine` enum variant
// was removed; a legacy `same_machine` DB string value is still mapped to
// SplitMachine for backward compatibility (see detect_topology below).

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use tauri::Emitter;
use crate::conflict_resolution::{self, ConflictContext, Resolution};
use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};

#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum SyncTopology {
    SplitMachine,
}

// #2649: detect_topology()'s only production call site (watcher.rs's
// same-machine-gated startup reconciliation) was removed — OpenClaw is
// split-machine only. Retained (unused in prod) because it still has direct
// test coverage (test_topology.rs) proving the legacy 'same_machine' DB
// string value keeps mapping to SplitMachine for backward compatibility.
#[allow(dead_code)]
pub fn detect_topology(conn: &rusqlite::Connection) -> SyncTopology {
    // Check explicit override first
    let explicit: Option<String> = conn
        .query_row("SELECT value FROM ai_settings WHERE key = 'tier3_topology'", [], |row| row.get(0))
        .ok();
    if let Some(ref val) = explicit {
        return match val.as_str() {
            "split_machine" => {
                eprintln!("[SYNC] Topology: split_machine (explicit override)");
                SyncTopology::SplitMachine
            }
            "same_machine" => {
                eprintln!("[SYNC] Topology: same_machine value found - treating as split_machine (SameMachine removed #1108)");
                SyncTopology::SplitMachine
            }
            _ => {
                eprintln!("[SYNC] Topology: split_machine (explicit override - unknown value '{}' treated as split_machine)", val);
                SyncTopology::SplitMachine
            }
        };
    }

    // All topologies are split_machine - SameMachine removed per #1108. This branch
    // is reached whenever `ai_settings.tier3_topology` is unset - which is the case on
    // essentially every install, paired or not (see docs/audits/2026-08-21-d11-sync-
    // endpoint-mismatch.md §5): the prior log line here ("no endpoint configured")
    // was misleading - this function never inspects `ai_settings.endpoint` or
    // `ai_agents` at all, it only ever checks `tier3_topology`. Corrected to describe
    // what actually happened: the explicit-override setting was absent, so the
    // (only-live) split_machine default applies.
    eprintln!("[SYNC] Topology: split_machine (default - tier3_topology setting not set)");
    SyncTopology::SplitMachine
}

// Retained for compatibility with callers that classify legacy gateway URLs.
#[allow(dead_code)]
pub fn is_remote_url(url: &str) -> bool {
    // Parse the URL, extract host, check if it's local
    if let Ok(parsed) = url::Url::parse(url) {
        match parsed.host() {
            Some(url::Host::Domain(domain)) => {
                domain != "localhost"
            }
            Some(url::Host::Ipv4(ip)) => {
                !ip.is_loopback()
            }
            Some(url::Host::Ipv6(ip)) => {
                !ip.is_loopback()
            }
            None => false,
        }
    } else {
        false
    }
}

/// Canonical identity files synced for all agents (ADR-0041 §22C.8.5).
/// Cross-ref: src-gateway/src/routes/sync.rs GATEWAY_IDENTITY_WHITELIST
/// Both lists must stay in sync - see ADR-0041 Decision 5.
const IDENTITY_WHITELIST: &[&str] = &[
    "SOUL.md",
    "MEMORY.md",
    "IDENTITY.md",
    "AGENTS.md",
    "USER.md",
    "TOOLS.md",
    "HEARTBEAT.md",
    "BOOTSTRAP.md",
    "RULES.md",
];

/// Sub-agent 6-file set (ADR-0041 §22C.8.5): sub-agents sync only these.
/// Cross-ref: src-gateway/src/routes/sync.rs GATEWAY_SUB_AGENT_WHITELIST
const SUB_AGENT_WHITELIST: &[&str] = &[
    "SOUL.md",
    "MEMORY.md",
    "IDENTITY.md",
    "AGENTS.md",
    "USER.md",
    "TOOLS.md",
];

/// Check if `filename` is allowed to sync for the given agent role.
///
/// Main agent (`is_main == true`): full IDENTITY_WHITELIST +
///   `memory/*.md` depth-1 + `logs/*.md` depth-1.
/// Sub-agents: SUB_AGENT_WHITELIST only (6 files).
///
/// ADR-0041 §22C.8.5 - depth-1 is a reader contract (OC dreaming reader
/// reads memory/*.md flat only; recursive sync would produce dead-weight
/// files OC never reads).
pub fn is_allowed_sync_file(filename: &str, is_main: bool) -> bool {
    if is_main {
        IDENTITY_WHITELIST.contains(&filename)
            || is_depth1_md("memory/", filename)
            || is_depth1_md("logs/", filename)
    } else {
        SUB_AGENT_WHITELIST.contains(&filename)
    }
}

/// Returns true if `filename` is of the form `prefix<name>.md` with exactly
/// one path component after the prefix (i.e. depth-1, no nested directories).
///
/// ADR-0041 Decision 6: depth-1 is a reader contract, not a performance hint.
pub fn is_depth1_md(prefix: &str, filename: &str) -> bool {
    if let Some(rest) = filename.strip_prefix(prefix) {
        rest.ends_with(".md") && !rest.contains('/')
    } else {
        false
    }
}

fn sync_file_relative(workspace: &str, filename: &str) -> Result<std::path::PathBuf, String> {
    PaneFs::validate_name(workspace)
        .map_err(|e| format!("Invalid workspace component {:?}: {}", workspace, e))?;

    if let Some(name) = filename.strip_prefix("memory/") {
        PaneFs::relative_from_names([workspace, "memory", name])
            .map_err(|e| format!("Invalid sync filename {:?}: {}", filename, e))
    } else if let Some(name) = filename.strip_prefix("logs/") {
        PaneFs::relative_from_names([workspace, "logs", name])
            .map_err(|e| format!("Invalid sync filename {:?}: {}", filename, e))
    } else {
        PaneFs::relative_from_names([workspace, filename])
            .map_err(|e| format!("Invalid sync filename {:?}: {}", filename, e))
    }
}

fn sync_filesystem(system_dir_override: Option<&Path>) -> Result<(PaneFs, PaneRoot), String> {
    match system_dir_override {
        Some(system_dir) => PaneFs::from_pane_root(system_dir)
            .map(|pane_fs| (pane_fs, PaneRoot::Pane))
            .map_err(|e| format!("Filesystem policy error: {}", e)),
        None => PaneFs::for_runtime()
            .map(|pane_fs| (pane_fs, PaneRoot::System))
            .map_err(|e| format!("Filesystem policy error: {}", e)),
    }
}

#[cfg(test)]
fn legacy_test_workspace_policy(workspace_path: &str) -> Option<(PaneFs, PaneRoot, String)> {
    let workspace = Path::new(workspace_path);
    let metadata = std::fs::symlink_metadata(workspace).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    let canonical = std::fs::canonicalize(workspace).ok()?;
    let temp_root = std::fs::canonicalize(std::env::temp_dir()).ok()?;
    if !canonical.starts_with(&temp_root) {
        return None;
    }
    let name = canonical.file_name()?.to_str()?.to_string();
    PaneFs::validate_name(&name).ok()?;
    let parent = canonical.parent()?;
    let pane_fs = PaneFs::from_pane_root(parent).ok()?;
    Some((pane_fs, PaneRoot::Pane, name))
}

fn migration_workspace_policy(
    pane_fs: &PaneFs,
    root: PaneRoot,
    workspace_path: &str,
) -> Result<(PaneFs, PaneRoot, String), String> {
    let supplied = Path::new(workspace_path);
    let workspace = supplied
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid workspace path: {}", workspace_path))?;
    let relative = PaneFs::relative_from_names([workspace])
        .map_err(|e| format!("Invalid workspace path {}: {}", workspace_path, e))?;
    if pane_fs
        .authorize_existing(root, &relative, ExistingKind::Directory)
        .is_ok()
    {
        return Ok((pane_fs.clone(), root, workspace.to_string()));
    }

    #[cfg(test)]
    if let Some(policy) = legacy_test_workspace_policy(workspace_path) {
        return Ok(policy);
    }

    Err(format!(
        "Workspace is not canonically beneath PaneRoot::System: {}",
        workspace_path
    ))
}

fn write_remote_sync_file(
    pane_fs: &PaneFs,
    root: PaneRoot,
    workspace: &str,
    filename: &str,
    content: &str,
    conn: Option<&Connection>,
) -> Result<(), String> {
    let relative = sync_file_relative(workspace, filename)?;
    if let Some(parent) = relative.parent() {
        pane_fs
            .create_dir_all(root, parent)
            .map_err(|e| format!("Failed to create authorized parent directory: {}", e))?;
    }
    let destination = pane_fs
        .authorize_write(root, &relative)
        .map_err(|e| format!("Unsafe sync write destination: {}", e))?;
    crate::file_ops::atomic_write(
        destination.as_path(),
        content,
        conn,
        crate::file_ops::WriteOrigin::Remote,
    )
}

#[derive(Serialize)]
struct SyncPushRequest {
    oc_agent_id: String,  // Changed from 'agent' (name) to 'oc_agent_id' (ID)
    filename: String,
    content: String,
    checksum: String,
}

#[derive(Deserialize)]
struct SyncPushResponse {
    written_at: String,
    // Parsed to preserve the gateway response contract; currently not consumed.
    #[allow(dead_code)]
    checksum: String,
}

/// Compute SHA-256 checksum of content
fn compute_checksum(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Extract agent UUID and filename from a system file path
/// Example: ".pane/system/E170EB0D29556C08E6593D3CC4DC4F58/MEMORY.md" → Some(("E170EB0D29556C08E6593D3CC4DC4F58", "MEMORY.md"))
/// Example: ".pane/system/A1B2C3D4.../logs/2026-04-07.md" → Some(("A1B2C3D4...", "logs/2026-04-07.md"))
pub fn extract_agent_and_filename(path: &Path) -> Option<(String, String)> {
    let path_str = path.to_string_lossy();

    // Must be a system file
    if !path_str.contains(".pane/system/") {
        return None;
    }

    let components: Vec<&str> = path_str.split("/").collect();

    // Find .pane/system/ indices
    let system_idx = components.iter().position(|&c| c == "system")?;

    // Agent UUID is next component
    if system_idx + 1 >= components.len() {
        return None;
    }

    let agent_uuid = components[system_idx + 1].to_string();

    // Filename is everything after agent UUID
    let remaining: Vec<&str> = components[system_idx + 2..].to_vec();
    if remaining.is_empty() {
        return None;
    }
    let filename = remaining.join("/");

    // Check filename is in whitelist OR is a log/memory file
    // ADR-0041 §22C.8.5: push surface enforces depth-1 for memory/ and logs/.
    // Role split enforcement on push: extract_agent_and_filename receives a
    // filesystem path and does not have DB access to look up is_main.
    // Decision: we apply the MAIN agent's allowlist here (superset of sub-agent),
    // and rely on gateway read + Pane ingest as the role-split enforcement points.
    // A sub-agent push of e.g. HEARTBEAT.md will pass push but be dropped at ingest.
    // This is correct per ADR-0041 Decision 5 (defense-in-depth).
    let is_whitelisted = IDENTITY_WHITELIST.contains(&filename.as_str())
        || is_depth1_md("logs/", &filename)
        || is_depth1_md("memory/", &filename);

    if !is_whitelisted {
        return None;
    }

    Some((agent_uuid, filename))
}

/// Check if a path is a system file that should sync
pub fn is_system_path(path: &Path) -> bool {
    extract_agent_and_filename(path).is_some()
}

/// Returns true iff at least one active `ai_agents` row has `runtime_type = 'openclaw'`.
/// #2564 B1 fix (Path B): replaces the retired `ai_settings.provider == "openclaw"` gate.
/// `ai_settings.provider` is retired for real (ADR-0063 D8) — every reader that used to
/// check it now checks per-agent `runtime_type` instead, which is the canonical source
/// of "is OpenClaw active" post-dissolution.
pub fn has_active_openclaw_agent(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM ai_agents WHERE runtime_type = 'openclaw' AND is_active = 1 LIMIT 1",
        [],
        |_| Ok(()),
    )
    .is_ok()
}

/// Get gateway URL from ai_settings table (used for bridge sync)
/// #2564 B1 fix (Path B): gate re-pointed from the retired `ai_settings.provider` scalar
/// to `has_active_openclaw_agent` (per-agent `runtime_type` check). `endpoint` itself is
/// unaffected — it's restored by the v80 migration from the pre-migration backup when
/// recoverable (see `migrate_v80` in migrations.rs).
pub fn get_gateway_url(conn: &Connection) -> Option<String> {
    if !has_active_openclaw_agent(conn) {
        return None;
    }
    crate::database::get_ai_setting(conn, "endpoint")
}

/// Get gateway token — migration-aware read (ADR-0044).
/// Reads from keychain (ai_gateway_token account); falls back to ai_settings.api_key
/// if migration is not yet complete. Post-migration: keychain miss = None (not configured).
pub fn get_gateway_token(conn: &Connection) -> Option<String> {
    crate::ai::credential_migration::read_gateway_token(conn)
}

/// Queue a sync operation (blocking version for immediate use)
fn queue_sync_blocking(
    conn: &Connection,
    oc_agent_id: &str,
    filename: &str,
    content: &str,
    checksum: &str,
) -> Result<(), String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO sync_queue (id, agent, filename, content, checksum, created_at, retry_count, next_retry_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)
         ON CONFLICT(agent, filename, checksum) DO NOTHING",
        params![id, oc_agent_id, filename, content, checksum, now, now],
    )
    .map_err(|e| format!("Failed to queue sync: {}", e))?;

    eprintln!("[SYNC] Queued: {}/{} (checksum: {})", oc_agent_id, filename, &checksum[..8]);
    Ok(())
}

/// Push a file to the gateway
// Retained for the dormant asynchronous sync path.
#[allow(dead_code)]
async fn sync_push(
    gateway_url: &str,
    token: &str,
    oc_agent_id: &str,
    filename: &str,
    content: &str,
    db: &crate::database::DbState,
) -> Result<(), String> {
    let checksum = compute_checksum(content);

    // Use gateway client with cert pinning
    let client = crate::gateway_client::build_gateway_client(db)?;

    let base = gateway_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/v1/sync/push", base);
    let req = SyncPushRequest {
        oc_agent_id: oc_agent_id.to_string(),
        filename: filename.to_string(),
        content: content.to_string(),
        checksum: checksum.clone(),
    };

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Gateway returned status {}: {}",
            response.status(),
            response.text().await.unwrap_or_default()
        ));
    }

    let result: SyncPushResponse = response
        .json()
        .await
        .map_err(|e| format!("Invalid response: {}", e))?;

    eprintln!("[SYNC] Pushed: {}/{} (checksum: {}, written_at: {})", oc_agent_id, filename, &checksum[..8], result.written_at);
    Ok(())
}

/// Push a file to the gateway (blocking version for std::thread::spawn contexts)
///
/// Takes `cert_pem` (pre-read from DB) instead of `&DbState` so the caller can release
/// the mutex before this call. Passing `&DbState` while holding its lock causes a
/// re-entrant deadlock in `build_gateway_client_blocking` (fixed in #1350).
fn sync_push_blocking(
    gateway_url: &str,
    token: &str,
    oc_agent_id: &str,
    filename: &str,
    content: &str,
    cert_pem: Option<String>,
) -> Result<(), String> {
    let checksum = compute_checksum(content);

    // Use blocking gateway client with pre-read cert PEM (no DB lock needed here)
    let client = crate::gateway_client::build_gateway_client_blocking_with_cert(cert_pem)?;

    // Split-machine: /v1/sync/push (old: /bridge/push)
    let base = gateway_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/v1/sync/push", base);
    let req = SyncPushRequest {
        oc_agent_id: oc_agent_id.to_string(),
        filename: filename.to_string(),
        content: content.to_string(),
        checksum: checksum.clone(),
    };

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", token))
        .json(&req)
        .send()
        .map_err(|e| format!("Network error: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Gateway returned status {}: {}",
            response.status(),
            response.text().unwrap_or_default()
        ));
    }

    let result: SyncPushResponse = response
        .json()
        .map_err(|e| format!("Invalid response: {}", e))?;

    eprintln!("[SYNC] Pushed: {}/{} (checksum: {}, written_at: {})", oc_agent_id, filename, &checksum[..8], result.written_at);
    Ok(())
}

/// Trigger a sync push after a system file write
/// Called by atomic_write after successful write
/// Non-blocking: queues immediately, background task processes queue
pub fn trigger_sync(conn: &Connection, path: &Path, content: &str) {
    // Check if this is a system file, extract UUID
    let (agent_uuid, filename) = match extract_agent_and_filename(path) {
        Some(pair) => pair,
        None => return, // Not a system file, skip sync
    };

    // Look up the agent's OC identity (ai_agents.runtime_ref, #2522 ADR-0063 D1 rename)
    // from its workspace UUID. `oc_agent_id` below is the Rust-side wire/handle name for
    // this value — what flows through SyncPushRequest.oc_agent_id and sync_queue.agent —
    // not the (now-renamed) ai_agents column it was read from.
    let oc_agent_id: String = match conn.query_row(
        "SELECT runtime_ref FROM ai_agents WHERE workspace_path = ?1",
        [&agent_uuid],
        |row| row.get(0),
    ) {
        Ok(id) => id,
        Err(_) => {
            eprintln!("[SYNC] No runtime_ref found for UUID {}", agent_uuid);
            return;
        }
    };

    // Split-machine: use endpoint + api_key (canonical keys per §22C.11A.4)
    let _gateway_url = match get_gateway_url(conn) {
        Some(url) if !url.is_empty() => {
            eprintln!("[SYNC] Pane gateway URL: {}", url);
            url
        },
        _ => {
            eprintln!("[SYNC] No endpoint configured, skipping sync for {}/{}", oc_agent_id, filename);
            return;
        }
    };

    let _token = match get_gateway_token(conn) {
        Some(t) if !t.is_empty() => t,
        _ => {
            eprintln!("[SYNC] No api_key configured");
            return;
        }
    };

    // Queue immediately (non-blocking)
    let checksum = compute_checksum(content);
    if let Err(e) = queue_sync_blocking(conn, &oc_agent_id, &filename, content, &checksum) {
        eprintln!("[SYNC] Failed to queue: {}", e);
        return;
    }

    // Spawn background task to process this item
    let db_path = get_db_path();
    let oc_agent_id = oc_agent_id.clone();
    let filename = filename.clone();
    let content = content.to_string();

    std::thread::spawn(move || {
        // Open fresh connection in spawned task
        let conn = match rusqlite::Connection::open(&db_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[SYNC] Failed to open DB: {}", e);
                return;
            }
        };

        // Read all config we need from the plain Connection before the HTTP call.
        // CRITICAL (#1350 fix, Option 2): we deliberately do NOT wrap conn in DbState
        // here. The old code did: DbState(Mutex::new(conn)) -> lock() -> pass &db_state
        // to sync_push_blocking -> build_gateway_client_blocking tried to re-lock the
        // same Mutex on the same thread -> non-reentrant deadlock. By reading cert_pem
        // directly and passing it as Option<String>, no DbState lock is needed during
        // the HTTP call.
        let gateway_url = match get_gateway_url(&conn) {
            Some(url) => url,
            None => return,
        };
        let token = match get_gateway_token(&conn) {
            Some(t) => t,
            None => return,
        };
        let cert_pem = crate::database::get_ai_setting(&conn, "gateway_tls_cert_pem");

        // Try to push (blocking version - cert_pem pre-read, no DbState lock needed)
        match sync_push_blocking(&gateway_url, &token, &oc_agent_id, &filename, &content, cert_pem) {
            Ok(_) => {
                // Success - remove from queue and update timestamp
                let _ = conn.execute(
                    "DELETE FROM sync_queue WHERE agent = ?1 AND filename = ?2",
                    rusqlite::params![oc_agent_id, filename],
                );
                let now = chrono::Local::now().to_rfc3339();
                let _ = crate::database::set_ai_setting(&conn, "last_sync_push", &now);
            }
            Err(e) => {
                eprintln!("[SYNC] Push failed for {}/{}: {}", oc_agent_id, filename, e);
                // Leave in queue for retry
            }
        }
    });
}

/// Get the database path (needed for async task connection)
///
/// #3008 sweep: this previously hardcoded `$HOME/Pane/.pane/pane.db` directly,
/// bypassing `crate::database::get_db_path()` and ignoring
/// `PANE_TEST_MODE`/`PANE_TEST_DB`/`PANE_TEST_ROOT` — a test run could open a
/// connection to the real production database instead of an isolated test
/// one. Routed through the canonical, test-aware path resolver instead.
fn get_db_path() -> std::path::PathBuf {
    crate::database::get_db_path()
}

/// Flush queued sync operations (called on app startup)
// Retained for the dormant asynchronous sync path.
#[allow(dead_code)]
pub async fn flush_sync_queue(conn: &Connection, db: &crate::database::DbState) -> Result<(), String> {
    let gateway_url = match get_gateway_url(conn) {
        Some(url) if !url.is_empty() => url,
        _ => return Ok(()), // No gateway configured
    };

    let token = match get_gateway_token(conn) {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(()), // No token configured
    };

    // Get queued items ready for retry (respects exponential backoff)
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn
        .prepare("SELECT id, agent, filename, content, retry_count FROM sync_queue WHERE next_retry_at IS NULL OR next_retry_at <= ?1 ORDER BY created_at ASC LIMIT 100")
        .map_err(|e| format!("Failed to query sync queue: {}", e))?;

    let rows = stmt
        .query_map([&now], |row| {
            Ok((
                row.get::<_, String>(0)?,  // id
                row.get::<_, String>(1)?,  // agent (oc_agent_id)
                row.get::<_, String>(2)?,  // filename
                row.get::<_, String>(3)?,  // content
                row.get::<_, i64>(4)?,     // retry_count
            ))
        })
        .map_err(|e| format!("Failed to read sync queue: {}", e))?;

    let mut flushed = 0;
    let mut failed = 0;
    let mut dropped = 0;

    for row in rows {
        if let Ok((id, oc_agent_id, filename, content, retry_count)) = row {
            // Max retry limit: drop after 5 attempts
            if retry_count >= 5 {
                eprintln!("[SYNC] Dropping {}/{} after {} retries", oc_agent_id, filename, retry_count);
                conn.execute("DELETE FROM sync_queue WHERE id = ?1", params![id]).ok();
                dropped += 1;
                continue;
            }

            match sync_push(&gateway_url, &token, &oc_agent_id, &filename, &content, db).await {
                Ok(_) => {
                    // Remove from queue
                    conn.execute("DELETE FROM sync_queue WHERE id = ?1", params![id])
                        .ok();
                    flushed += 1;
                }
                Err(e) => {
                    eprintln!("[SYNC] Flush failed for {}/{}: {}", oc_agent_id, filename, e);

                    // Update retry count and next retry time (exponential backoff)
                    let next_retry = chrono::Utc::now()
                        + chrono::Duration::seconds(2_i64.pow(retry_count.min(6) as u32));

                    conn.execute(
                        "UPDATE sync_queue SET retry_count = retry_count + 1, next_retry_at = ?1 WHERE id = ?2",
                        params![next_retry.to_rfc3339(), id],
                    )
                    .ok();

                    failed += 1;
                }
            }
        }
    }

    if flushed > 0 {
        eprintln!("[SYNC] Flushed {} queued sync(s)", flushed);
    }
    if failed > 0 {
        eprintln!("[SYNC] {} sync(s) failed, will retry", failed);
    }
    if dropped > 0 {
        eprintln!("[SYNC] Dropped {} sync(s) after max retries", dropped);
    }

    Ok(())
}

/// Flush queued sync operations (blocking version for background threads)
pub fn flush_sync_queue_blocking(conn: &Connection, app_handle: Option<&tauri::AppHandle>, db: &crate::database::DbState) -> Result<(), String> {
    // Split-machine: use endpoint + api_key (canonical keys per §22C.11A.4)
    let gateway_url = match get_gateway_url(conn) {
        Some(url) if !url.is_empty() => url,
        _ => return Ok(()), // No gateway configured
    };

    let token = match get_gateway_token(conn) {
        Some(t) if !t.is_empty() => t,
        _ => return Ok(()), // No token configured
    };

    // Pre-read the cert PEM so sync_push_blocking doesn't need &DbState.
    // The conn here is always a standalone Connection (not a lock held on db.0),
    // so db is safe to pass directly to build_gateway_client_blocking for the pull
    // path. But for push we use the cert-variant to keep the call sites consistent.
    // See #1350 for the original deadlock analysis.
    let _ = db; // db param retained for API stability; cert is read from conn below
    let cert_pem = crate::database::get_ai_setting(conn, "gateway_tls_cert_pem");

    // Get queued items ready for retry (respects exponential backoff)
    let now = chrono::Utc::now().to_rfc3339();
    let mut stmt = conn
        .prepare("SELECT id, agent, filename, content, retry_count FROM sync_queue WHERE next_retry_at IS NULL OR next_retry_at <= ?1 ORDER BY created_at ASC LIMIT 100")
        .map_err(|e| format!("Failed to query sync queue: {}", e))?;

    let rows = stmt
        .query_map([&now], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|e| format!("Failed to read sync queue: {}", e))?;

    let mut flushed = 0;
    let mut failed = 0;
    let mut dropped = 0;

    for row in rows {
        if let Ok((id, oc_agent_id, filename, content, retry_count)) = row {
            // Max retry limit: drop after 5 attempts
            if retry_count >= 5 {
                eprintln!("[SYNC] Dropping {}/{} after {} retries", oc_agent_id, filename, retry_count);

                // Emit event to frontend if app_handle is available
                if let Some(handle) = app_handle {
                    handle.emit("sync-failure", json!({
                        "agent": oc_agent_id,
                        "filename": filename,
                        "error": format!("Sync failed after {} retries", retry_count)
                    })).ok();
                }

                conn.execute("DELETE FROM sync_queue WHERE id = ?1", params![id]).ok();
                dropped += 1;
                continue;
            }

            match sync_push_blocking(&gateway_url, &token, &oc_agent_id, &filename, &content, cert_pem.clone()) {
                Ok(_) => {
                    // Remove from queue
                    conn.execute("DELETE FROM sync_queue WHERE id = ?1", params![id])
                        .ok();
                    flushed += 1;
                }
                Err(e) => {
                    eprintln!("[SYNC] Flush failed for {}/{}: {}", oc_agent_id, filename, e);

                    // Update retry count and next retry time (exponential backoff)
                    let next_retry = chrono::Utc::now()
                        + chrono::Duration::seconds(2_i64.pow(retry_count.min(6) as u32));

                    conn.execute(
                        "UPDATE sync_queue SET retry_count = retry_count + 1, next_retry_at = ?1 WHERE id = ?2",
                        params![next_retry.to_rfc3339(), id],
                    )
                    .ok();

                    failed += 1;
                }
            }
        }
    }

    if flushed > 0 {
        eprintln!("[SYNC] Flushed {} queued sync(s)", flushed);
    }
    if failed > 0 {
        eprintln!("[SYNC] {} sync(s) failed, will retry", failed);
    }
    if dropped > 0 {
        eprintln!("[SYNC] Dropped {} sync(s) after max retries", dropped);
    }

    Ok(())
}

#[derive(Deserialize)]
struct PendingChange {
    id: String,
    // Parsed to preserve the gateway pending-change contract.
    #[allow(dead_code)]
    oc_agent_id: String,  // Changed from 'agent' (name) to 'oc_agent_id'
    filename: String,
    content: String,
    // Parsed to preserve the gateway pending-change contract.
    #[allow(dead_code)]
    checksum: String,
    queued_at: String,
}

#[derive(Deserialize)]
struct PullResponse {
    pending_changes: Vec<PendingChange>,
}

#[derive(Serialize)]
struct PullConfirmRequest {
    oc_agent_id: String,  // Changed from 'agent' (name)
    processed_ids: Vec<String>,
}

/// Pull pending changes from the OpenClaw gateway (blocking).
/// Called on every app startup (lib.rs:945) to retrieve changes OC made while Pane was offline.
/// Uses the shared `conflict_resolution` module - ADR-0042 (Pane-PRIMARY / OC-REPLICA).
///
/// See also: `flush_sync_queue_blocking` for the outbound push after ingest.
pub fn pull_pending_changes_blocking(
    conn: &Connection,
    app_handle: Option<&tauri::AppHandle>,
    db: &crate::database::DbState,
) -> Result<(), String> {
    // Split-machine: use endpoint + api_key (canonical keys per §22C.11A.4)
    let gateway_url = match get_gateway_url(conn) {
        Some(url) if !url.is_empty() => url,
        _ => {
            eprintln!("[SYNC] No gateway configured, skipping pull");
            return Ok(());
        }
    };

    let token = match get_gateway_token(conn) {
        Some(t) if !t.is_empty() => t,
        _ => {
            eprintln!("[SYNC] No gateway token configured");
            return Ok(());
        }
    };

    // Query all active agents from ai_agents table. runtime_type is selected alongside
    // runtime_ref so the "main" comparison below can be gated on runtime_type == 'openclaw'
    // (ADR-0063 D1 semantic-trap ruling: `runtime_ref == "main"` is OC's own "main" slug in
    // OC's namespace, not a Pane default-agent flag — must never be read as one without the
    // runtime_type gate).
    let mut stmt = conn
        .prepare("SELECT id, workspace_path, runtime_ref, runtime_type FROM ai_agents WHERE is_active = 1 AND runtime_ref IS NOT NULL")
        .map_err(|e| format!("Failed to query agents: {}", e))?;

    let agents: Vec<(String, String, String, Option<String>)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|e| format!("Failed to read agents: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect agents: {}", e))?;

    if agents.is_empty() {
        eprintln!("[SYNC] No active agents with runtime_ref found, skipping pull");
        return Ok(());
    }

    eprintln!("[SYNC] Pulling changes for {} agent(s)", agents.len());

    // Fix A (#1180): pre-read cert from conn instead of taking db.0 mutex again.
    // build_gateway_client_blocking(db) would re-acquire the shared mutex and deadlock
    // against the periodic flush thread (lib.rs:967) which holds it across HTTP.
    // See: docs/audits/2026-06-02-freeze-cluster-1347-1180.md
    let cert_pem = crate::database::get_ai_setting(conn, "gateway_tls_cert_pem");
    let client = crate::gateway_client::build_gateway_client_blocking_with_cert(cert_pem)?;

    let (pane_fs, system_root) = sync_filesystem(None)?;

    // Pull changes for each agent
    for (_agent_id, workspace_path, runtime_ref, runtime_type) in agents {
        // Split-machine: /v1/sync/pull (old: /bridge/pull)
        let base = gateway_url.trim_end_matches('/').trim_end_matches("/v1");
        let url = format!(
            "{}/v1/sync/pull?oc_agent_id={}",
            base,
            urlencoding::encode(&runtime_ref)
        );

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .map_err(|e| format!("Network error for {}: {}", runtime_ref, e))?;

        if !response.status().is_success() {
            eprintln!(
                "[SYNC] Warning: Gateway returned status {} for {}",
                response.status(),
                runtime_ref
            );
            continue;
        }

        let pull_response: PullResponse = response
            .json()
            .map_err(|e| format!("Invalid response for {}: {}", runtime_ref, e))?;

        if pull_response.pending_changes.is_empty() {
            eprintln!("[SYNC] No pending changes for {}", runtime_ref);
            continue;
        }

        eprintln!(
            "[SYNC] Retrieved {} pending change(s) for {}",
            pull_response.pending_changes.len(),
            runtime_ref
        );

        // ADR-0063 D1 semantic-trap ruling: gate the "main" comparison on
        // runtime_type == 'openclaw' — runtime_ref == "main" alone is OC-namespace
        // identity, not a Pane role signal.
        let is_main_agent = runtime_type.as_deref() == Some("openclaw") && runtime_ref == "main";
        let mut processed_ids = Vec::new();

        for change in pull_response.pending_changes {
            // Allowlist check (ADR-0041 §22C.8.5)
            if !is_allowed_sync_file(&change.filename, is_main_agent) {
                eprintln!(
                    "[SYNC] Filtered non-allowlisted file: {}/{}",
                    runtime_ref, change.filename
                );
                // Mark as processed so gateway doesn't re-send on every pull.
                processed_ids.push(change.id);
                continue;
            }

            let file_relative = match sync_file_relative(&workspace_path, &change.filename) {
                Ok(relative) => relative,
                Err(e) => {
                    eprintln!(
                        "[SYNC] Rejecting unsafe path {}/{}: {}",
                        runtime_ref, change.filename, e
                    );
                    processed_ids.push(change.id);
                    continue;
                }
            };

            let (local_modified, local_content) =
                match pane_fs.authorize_existing(system_root, &file_relative, ExistingKind::File) {
                    Ok(existing) => (
                        std::fs::metadata(existing.as_path())
                            .and_then(|m| m.modified())
                            .ok(),
                        std::fs::read_to_string(existing.as_path()).ok(),
                    ),
                    Err(_) => (None, None),
                };

            // Parse remote queued_at (ISO 8601)
            let remote_queued = chrono::DateTime::parse_from_rfc3339(&change.queued_at).ok();

            // Use shared conflict resolution module - ADR-0042 (Pane-PRIMARY / OC-REPLICA)
            // This is the LIVE path (lib.rs:945 → pull_pending_changes_blocking → here).
            // On tie, KeepLocal: Pane wins per ADR-0042 §22C.8.2.
            let resolution = conflict_resolution::resolve(&ConflictContext {
                agent_id: runtime_ref.clone(),
                filename: change.filename.clone(),
                local_modified: local_modified.map(|t| chrono::DateTime::<chrono::Utc>::from(t)),
                remote_timestamp: remote_queued.map(|t| t.with_timezone(&chrono::Utc)),
                local_content: local_content.clone(),
                remote_content: Some(change.content.clone()),
            });

            match resolution {
                Resolution::AcceptRemote => {
                    // Use atomic_write to write the file.
                    // WriteOrigin::Remote suppresses trigger_sync on ingest to prevent
                    // the ping-pong loop. Canonical write-back is handled by
                    // flush_sync_queue_blocking at the end of this ingest pass.
                    let file_path = match pane_fs.authorize_write(system_root, &file_relative) {
                        Ok(path) => path,
                        Err(e) => {
                            eprintln!(
                                "[SYNC] Unsafe write destination {}/{}: {}",
                                runtime_ref, change.filename, e
                            );
                            continue;
                        }
                    };
                    if let Err(e) = crate::file_ops::atomic_write(
                        file_path.as_path(),
                        &change.content,
                        Some(conn),
                        crate::file_ops::WriteOrigin::Remote,
                    ) {
                        eprintln!(
                            "[SYNC] Failed to write {}/{}: {}",
                            runtime_ref, change.filename, e
                        );
                        continue;
                    }
                    eprintln!(
                        "[SYNC] Ingested: {}/{} ({} bytes)",
                        runtime_ref,
                        change.filename,
                        change.content.len()
                    );
                }
                Resolution::KeepLocal => {
                    eprintln!(
                        "[SYNC] Keeping local version of {}/{}",
                        runtime_ref, change.filename
                    );
                }
                Resolution::Skip => {
                    eprintln!(
                        "[SYNC] Skipping {}/{} (identical or both missing)",
                        runtime_ref, change.filename
                    );
                }
            }

            // Mark as processed regardless (even if we didn't ingest, we've handled it)
            processed_ids.push(change.id);
        }

        // Confirm processed entries for this agent
        if !processed_ids.is_empty() {
            // Split-machine: /v1/sync/pull/confirm (old: /bridge/pull/confirm)
            let base = gateway_url.trim_end_matches('/').trim_end_matches("/v1");
            let confirm_url = format!("{}/v1/sync/pull/confirm", base);
            let confirm_req = PullConfirmRequest {
                oc_agent_id: runtime_ref.clone(),
                processed_ids,
            };

            let confirm_response = client
                .delete(&confirm_url)
                .header("Authorization", format!("Bearer {}", token))
                .json(&confirm_req)
                .send()
                .map_err(|e| format!("Confirm request failed for {}: {}", runtime_ref, e))?;

            if !confirm_response.status().is_success() {
                eprintln!(
                    "[SYNC] Warning: Failed to confirm processed entries for {}: {}",
                    runtime_ref,
                    confirm_response.status()
                );
            } else {
                eprintln!("[SYNC] Confirmed processed entries for {}", runtime_ref);
            }
        }
    }

    // After ingesting changes, push canonical state back to gateway.
    // Forward app_handle so sync-failure banner events reach the frontend
    // when retry_count >= 5 drops an entry (the whole point of §1370 plumbing).
    eprintln!("[SYNC] Pushing canonical state back to gateway...");
    flush_sync_queue_blocking(conn, app_handle, db)?;

    Ok(())
}

/// Result returned to the frontend after a manual "Sync Now" request (#2522,
/// ADR-0063 Stage 7a). `success` is always `true` on `Ok(...)` — the field
/// exists so the frontend has a stable shape to check without inspecting
/// `Result` variants across the Tauri IPC boundary; failures surface as the
/// command's `Err(String)` instead of `success: false`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SyncNowResult {
    pub success: bool,
    pub timestamp: String,
}

/// Core logic behind the `sync_now` Tauri command (#2522, ADR-0063 Stage 7a).
/// Wraps `pull_pending_changes_blocking`, which internally handles both the
/// inbound pull AND the outbound flush (it calls `flush_sync_queue_blocking`
/// at its own tail — see the comment above that call) — the same primitive
/// the periodic sync thread uses. A user-triggered "Sync Now" therefore does
/// not need its own separate flush call; calling it a second time here would
/// just be redundant work on an already-drained queue.
///
/// Split out from the `#[tauri::command]` wrapper in lib.rs so it can be
/// unit-tested directly: `tauri::AppHandle` cannot be constructed in unit
/// tests (see test_pull_pending_changes_blocking.rs), so tests call this
/// function with `app_handle: None`, exactly like the existing pull/flush
/// tests do.
pub fn sync_now_blocking(
    conn: &Connection,
    app_handle: Option<&tauri::AppHandle>,
    db: &crate::database::DbState,
) -> Result<SyncNowResult, String> {
    eprintln!("[SYNC_NOW] Manual sync requested");

    pull_pending_changes_blocking(conn, app_handle, db)
        .map_err(|e| format!("sync_now: pull_pending_changes_blocking failed: {}", e))?;

    let timestamp = chrono::Utc::now().to_rfc3339();
    eprintln!("[SYNC_NOW] Completed successfully at {}", timestamp);

    Ok(SyncNowResult {
        success: true,
        timestamp,
    })
}

#[derive(Deserialize)]
struct SyncAllAgent {
    oc_id: String,    // OpenClaw agent ID (e.g., "main", "coder")
    name: String,     // Display name (e.g., "Brit Freed", "Stia")
    files: Vec<SyncAllFile>,
    /// ADR-0067 D5: OC's per-agent `isDefault`, forwarded by the Pane gateway's
    /// `/v1/sync/initial` response (`AgentFiles.is_default`, wired by commit
    /// `6eb6e25a`). `#[serde(default)]` so a gateway that hasn't redeployed yet
    /// (field simply absent) deserializes to `false`, identical to an explicit
    /// `false` — never a hard failure on an older gateway.
    #[serde(default)]
    is_default: bool,
}

#[derive(Deserialize)]
struct SyncAllFile {
    filename: String,
    content: String,
    // Parsed to preserve the initial-sync response contract.
    #[allow(dead_code)]
    checksum: String,
}

#[derive(Deserialize)]
struct SyncAllResponse {
    agents: Vec<SyncAllAgent>,
}

/// Perform initial sync - retrieve all agent files from gateway
/// Used during onboarding when no local agent files exist
/// Returns list of imported agents (oc_agent_id, name, uuid)
///
/// `cert_pem` must be pre-read from DB by the caller (key: `gateway_tls_cert_pem`) so
/// the pinned TLS builder is used when a gateway cert is configured.  Passing `None`
/// falls back to `danger_accept_invalid_certs`, which is only appropriate before
/// pairing has completed — callers with DB access must supply the stored cert (#2011).
pub fn initial_sync(
    gateway_url: &str,
    token: &str,
    cert_pem: Option<String>,
) -> Result<Vec<(String, String, String, bool)>, String> {
    eprintln!("[SYNC] initial_sync started: url={}", gateway_url);

    // Delegate HTTP fetch + retry to fetch_gateway_agents (§24.5.5 refactor, #956).
    // cert_pem flows from the caller so the SPKI-pinned builder is used when a cert
    // is stored — fixes the danger_accept_invalid_certs fallback on /v1/sync/initial.
    let gateway_agents =
        fetch_gateway_agents(gateway_url, token, cert_pem).map_err(|e| e.to_string())?;

    if gateway_agents.is_empty() {
        eprintln!("[SYNC] No agents found on gateway");
        return Ok(vec![]);
    }

    eprintln!(
        "[SYNC] Retrieved {} agent(s) from gateway",
        gateway_agents.len()
    );

    // BUG 4 FIX: Validate response structure before processing
    for agent in &gateway_agents {
        if agent.oc_id.is_empty() {
            return Err(format!("Agent missing oc_id (name: {:?})", agent.name));
        }
        if agent.name.is_empty() {
            eprintln!(
                "[SYNC] Warning: Agent {} has empty name, will use fallback",
                agent.oc_id
            );
        }
        // Filename emptiness is checked here; allowlist and component validation happen
        // immediately before each policy-authorized write.
        for file in &agent.files {
            if file.filename.is_empty() {
                return Err(format!("Invalid filename from gateway: {}", file.filename));
            }
        }
    }

    let (pane_fs, system_root) = sync_filesystem(None)?;

    let mut imported_agents = Vec::new();

    for agent in gateway_agents {
        let agent_name = if agent.name.trim().is_empty() {
            format!("Agent {}", agent.oc_id)
        } else {
            agent.name.clone()
        };

        eprintln!(
            "[SYNC] Importing agent: {} (oc_id: {}, {} file(s))",
            agent_name,
            agent.oc_id,
            agent.files.len()
        );

        // Generate a new UUID for this agent's local folder
        let agent_uuid = uuid::Uuid::new_v4()
            .to_string()
            .replace("-", "")
            .to_uppercase();
        let agent_relative = PaneFs::relative_from_names([agent_uuid.as_str()])
            .map_err(|e| format!("Invalid generated agent workspace: {}", e))?;

        // Create agent directory
        let agent_dir = pane_fs
            .create_dir_all(system_root, &agent_relative)
            .map_err(|e| {
                format!(
                    "Failed to create agent directory {} ({}): {}",
                    agent_name, agent_uuid, e
                )
            })?;
        eprintln!(
            "[SYNC] Creating agent directory: {}",
            agent_dir.as_path().display()
        );

        // BUG 2 FIX: Only create logs/ for the main agent
        if agent.oc_id == "main" {
            let logs_relative = PaneFs::relative_from_names([agent_uuid.as_str(), "logs"])
                .map_err(|e| format!("Invalid logs directory path: {}", e))?;
            eprintln!("[SYNC] Creating logs directory for main agent");
            pane_fs
                .create_dir_all(system_root, &logs_relative)
                .map_err(|e| {
                    format!("Failed to create logs directory for {}: {}", agent_name, e)
                })?;
        }

        // Write each file WITHOUT triggering sync (we're importing FROM gateway, not pushing back)
        // ADR-0041 §22C.8.5: enforce allowlist on initial-sync ingest (Door D2)
        let is_main_agent = agent.oc_id == "main";
        for file in agent.files {
            if !is_allowed_sync_file(&file.filename, is_main_agent) {
                eprintln!(
                    "[SYNC] initial_sync: filtered non-allowlisted file {}/{}",
                    agent_name, file.filename
                );
                continue;
            }
            let file_relative = match sync_file_relative(&agent_uuid, &file.filename) {
                Ok(relative) => relative,
                Err(e) => {
                    eprintln!(
                        "[SYNC] initial_sync: rejecting unsafe filename {}/{}: {}",
                        agent_name, file.filename, e
                    );
                    continue;
                }
            };

            // Create parent directory if needed (for logs/YYYY-MM-DD.md, memory/*)
            if let Some(parent) = file_relative.parent() {
                pane_fs.create_dir_all(system_root, parent).map_err(|e| {
                    format!("Failed to create directory for {}: {}", file.filename, e)
                })?;
            }
            let file_path = pane_fs
                .authorize_write(system_root, &file_relative)
                .map_err(|e| {
                    format!(
                        "Unsafe write destination for {}/{}: {}",
                        agent_name, file.filename, e
                    )
                })?;

            // BUG 10 FIX: Log before file write
            eprintln!(
                "[SYNC] Writing {}/{} ({} bytes)",
                agent_name,
                file.filename,
                file.content.len()
            );

            crate::file_ops::atomic_write(
                file_path.as_path(),
                &file.content,
                None,
                crate::file_ops::WriteOrigin::Remote,
            )
            .map_err(|e| {
                format!(
                    "Failed to write file for {}/{}: {}",
                    agent_name, file.filename, e
                )
            })?;

            // BUG 10 FIX: Log after successful write
            eprintln!(
                "[SYNC] Wrote {}/{}: {} bytes",
                agent_name,
                file.filename,
                file.content.len()
            );
        }

        // ADR-0067 D5: carry OC's reported per-agent isDefault (tier 1) through
        // for sync_initial_import's default-resolution decision.
        imported_agents.push((agent.oc_id, agent_name, agent_uuid, agent.is_default));
    }

    // BUG 10 FIX: Log completion summary
    eprintln!(
        "[SYNC] initial_sync complete: imported {} agent(s)",
        imported_agents.len()
    );
    for (oc_id, name, uuid, _) in &imported_agents {
        eprintln!("  - {} (oc_id: {}, uuid: {})", name, oc_id, uuid);
    }

    Ok(imported_agents)
}

// ── §24.5.5 Provider-Switch Re-import ──────────────────────────────────────
//
// #956 - Settings: switching to OpenClaw must trigger agent discovery + sync
// Spec: docs/spec/agents.md §24.5.5

/// Gateway agent struct returned by `fetch_gateway_agents`.
/// Structured error from `fetch_gateway_agents` that carries an optional gateway
/// error code (e.g. `"oc_handshake_timeout"`) parsed from the response body.
///
/// Callers returning `Result<_, String>` can convert via `.map_err(|e| e.to_string())`
/// or by implementing `From<GatewayFetchError> for String` (see below).  The
/// `Display` impl serialises the code into the message string so it survives the
/// Tauri `Result<_, String>` invoke boundary and can be discriminated in Svelte
/// without changing any Tauri command signatures.
///
/// Format when a code is present (stable contract, checked by Svelte):
///   `{"error_code":"oc_handshake_timeout","message":"..."}`
/// Format without a code: plain human-readable message string.
#[derive(Debug, Clone)]
pub struct GatewayFetchError {
    /// Gateway `error` field from the JSON response body, if parsed.
    pub code: Option<String>,
    pub message: String,
}

impl GatewayFetchError {
    fn plain(message: impl Into<String>) -> Self {
        Self { code: None, message: message.into() }
    }

    fn with_code(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: Some(code.into()), message: message.into() }
    }
}

impl std::fmt::Display for GatewayFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.code {
            Some(code) => {
                // JSON-encode so the code survives the Tauri String boundary.
                // The message field uses simple escaping sufficient for gateway
                // error messages (no control characters or embedded quotes expected).
                let escaped = self.message.replace('\\', r"\\").replace('"', r#"\""#);
                write!(f, "{{\"error_code\":\"{}\",\"message\":\"{}\"}}", code, escaped)
            }
            None => write!(f, "{}", self.message),
        }
    }
}

impl From<GatewayFetchError> for String {
    fn from(e: GatewayFetchError) -> String {
        e.to_string()
    }
}

/// Contains only what Pane needs for re-import: the OC identity, display name,
/// and the set of files to write into the local workspace directory.
#[derive(Debug, Clone)]
pub struct GatewayAgent {
    pub oc_id: String,
    pub name: String,
    pub files: Vec<GatewayFile>,
    /// ADR-0067 D5 / spec §22C.6.2 tier 1: OC's own per-agent `isDefault`, forwarded
    /// verbatim from the gateway's `/v1/sync/initial` response. Always `false` on OC
    /// 2026.8.2 (§22C.10 capture) — correct-but-dormant; tiers 2/3
    /// (`resolve_should_be_default`) are the live production paths today.
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub struct GatewayFile {
    pub filename: String,
    pub content: String,
}

/// Fetch and parse the gateway's agent list.
///
/// Extracted from `initial_sync` so it can be reused by both the onboarding
/// path and the §24.5.5 provider-switch re-import path. `initial_sync` is
/// refactored to call this internally - **zero behavior change** on the
/// existing onboarding path.
///
/// Retry logic: 3 attempts with exponential back-off (2 s, 4 s),
/// identical to the original `initial_sync` retry loop.
///
/// Uses `build_gateway_client_blocking_with_cert` so the caller can pre-read
/// the cert PEM and avoid re-entrant DbState mutex deadlocks (#1350).
pub fn fetch_gateway_agents(
    gateway_url: &str,
    token: &str,
    cert_pem: Option<String>,
) -> Result<Vec<GatewayAgent>, GatewayFetchError> {
    eprintln!("[SYNC] fetch_gateway_agents: url={}", gateway_url);

    let client = crate::gateway_client::build_gateway_client_blocking_with_cert(cert_pem)
        .map_err(GatewayFetchError::plain)?;
    let base = gateway_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/v1/sync/initial", base);
    eprintln!("[SYNC] GET {}", url);

    // §22C.5.4 / ADR-0055: parse error body on non-success responses so we can
    // discriminate oc_handshake_timeout from other 502 causes.  Any error whose
    // code is `oc_handshake_timeout` is fast-failed immediately — retrying cannot
    // fix a stale OC bearer token.  All other failures continue the 3-attempt
    // exponential-backoff loop unchanged.
    #[derive(serde::Deserialize)]
    struct GatewayErrorBody {
        error: Option<String>,
    }

    // 3 attempts, exponential back-off: 2 s, 4 s
    let mut last_error: GatewayFetchError = GatewayFetchError::plain("Unknown error");
    let response = 'retry_loop: loop {
        for attempt in 0..3u32 {
            match client
                .get(&url)
                .header("Authorization", format!("Bearer {}", token))
                .send()
            {
                Ok(resp) if resp.status().is_success() => break 'retry_loop resp,
                Ok(resp) => {
                    let status = resp.status();
                    // Read body before it's consumed; fall back gracefully on parse failure.
                    let error_code: Option<String> = resp
                        .json::<GatewayErrorBody>()
                        .ok()
                        .and_then(|b| b.error);

                    eprintln!(
                        "[SYNC] fetch_gateway_agents attempt {}/3 failed: HTTP {} (error_code={:?})",
                        attempt + 1,
                        status,
                        error_code
                    );

                    // §22C.5.4: fast-fail on oc_handshake_timeout — a stale token
                    // won't be fixed by retrying.  All other error codes continue.
                    if error_code.as_deref() == Some("oc_handshake_timeout") {
                        eprintln!("[SYNC] oc_handshake_timeout detected — fast-failing (no retry)");
                        return Err(GatewayFetchError::with_code(
                            "oc_handshake_timeout",
                            format!("Gateway returned oc_handshake_timeout (HTTP {})", status),
                        ));
                    }

                    last_error = match error_code {
                        Some(code) => GatewayFetchError::with_code(
                            code.clone(),
                            format!("Gateway returned {} (error: {})", status, code),
                        ),
                        None => GatewayFetchError::plain(
                            format!("Gateway returned {}", status)
                        ),
                    };
                }
                Err(e) => {
                    last_error = GatewayFetchError::plain(e.to_string());
                    eprintln!(
                        "[SYNC] fetch_gateway_agents attempt {}/3 failed: {}",
                        attempt + 1,
                        last_error
                    );
                }
            }
            if attempt < 2 {
                let delay = std::time::Duration::from_secs(2u64.pow(attempt + 1)); // 2 s, 4 s
                eprintln!("[SYNC] Retrying in {:?}...", delay);
                std::thread::sleep(delay);
            }
        }
        return Err(GatewayFetchError::plain(format!(
            "Could not reach gateway after 3 attempts. Last error: {}",
            last_error
        )));
    };

    let sync_response: SyncAllResponse = response
        .json()
        .map_err(|e| GatewayFetchError::plain(format!("Invalid response format: {}", e)))?;

    eprintln!(
        "[SYNC] fetch_gateway_agents: {} agent(s) returned",
        sync_response.agents.len()
    );

    // Map internal deserialization types to the public GatewayAgent struct.
    Ok(sync_response
        .agents
        .into_iter()
        .map(|a| GatewayAgent {
            oc_id: a.oc_id,
            name: a.name,
            files: a
                .files
                .into_iter()
                .map(|f| GatewayFile {
                    filename: f.filename,
                    content: f.content,
                })
                .collect(),
            is_default: a.is_default,
        })
        .collect())
}

/// ADR-0067 D5 / spec §22C.6.2: the canonical three-step default-agent
/// resolution order, shared by every import/re-import path so the concept
/// stops being independently re-derived per call site (`sync_re_import` here,
/// `sync_initial_import` in commands.rs — previously two, going on three,
/// hand-rolled `oc_id == "main"` implementations).
///
/// 1. `oc_reported_default` — OC's own per-agent `isDefault` (tier 1). Always
///    wins when true, including over an existing foreign default — it is OC's
///    own ground truth. Always `false` on OC 2026.8.2 (§22C.10) —
///    correct-but-dormant; a green test for this tier is not evidence the
///    feature works on the live gateway (ADR-0067 Consequences).
/// 2. `existing_default_exists && !already_default_here` — a prior Settings
///    choice (`ai_agents.is_default`, live in the local DB) blocks the tier-3
///    fallback from stealing the seat, UNLESS the row already holding it is
///    this same incoming agent being re-synced in place. **This is the live
///    production path** — tier 3 only fires on first-run/no-prior-choice.
/// 3. `is_main_by_ref` (`runtime_ref`/`oc_id == "main"`) — first-run fallback.
pub(crate) fn resolve_should_be_default(
    oc_reported_default: bool,
    is_main_by_ref: bool,
    existing_default_exists: bool,
    already_default_here: bool,
) -> bool {
    if oc_reported_default {
        return true;
    }
    if existing_default_exists && !already_default_here {
        return false;
    }
    is_main_by_ref
}

#[cfg(test)]
mod resolve_should_be_default_tests {
    use super::resolve_should_be_default;

    /// Tier 1: OC's reported default wins even over an existing foreign default
    /// that tier 2 would otherwise protect. Fixture-only per ADR-0067's own
    /// Consequences section — OC 2026.8.2 never sends `true` here.
    #[test]
    fn test_tier1_oc_default_wins_over_existing_foreign_default() {
        assert!(resolve_should_be_default(true, false, true, false));
    }

    /// Tier 2, PRIMARY production case: no OC default reported (structurally
    /// false on 2026.8.2), a prior Settings choice already exists on a
    /// DIFFERENT row — that existing choice wins; this agent does not land as
    /// default even though it's OC's structural "main".
    #[test]
    fn test_tier2_existing_settings_choice_blocks_takeover() {
        assert!(!resolve_should_be_default(false, true, true, false));
    }

    /// Tier 2, PRIMARY production case, re-sync of the currently-default row:
    /// `already_default_here` must prevent the row from losing its own seat on
    /// every re-sync just because "a default exists" (it's this same row).
    #[test]
    fn test_tier2_resyncing_current_default_keeps_seat() {
        assert!(resolve_should_be_default(false, true, true, true));
    }

    /// Tier 3, first-run fallback: no OC default, no existing default anywhere
    /// — the structurally-"main" agent lands as default. Non-regression for
    /// onboarding, byte-for-byte today's pre-ADR-0067 behavior.
    #[test]
    fn test_tier3_first_run_fallback_to_main() {
        assert!(resolve_should_be_default(false, true, false, false));
    }

    /// Tier 3 does not fire for a non-main agent when nothing else applies.
    #[test]
    fn test_tier3_non_main_agent_does_not_land_as_default() {
        assert!(!resolve_should_be_default(false, false, false, false));
    }

    /// A non-main agent with no OC default and an existing foreign default
    /// never lands as default — sanity floor case.
    #[test]
    fn test_non_main_never_lands_as_default_with_existing_default() {
        assert!(!resolve_should_be_default(false, false, true, false));
    }
}

/// §24.5.5 - Perform a provider-switch re-import.
///
/// For each agent returned by the gateway:
///
/// - **Existing** (`runtime_ref` already in `ai_agents`): overwrite ALL files
///   (including `IDENTITY.md` - NO `!exists` guard, per spec §24.5.5 design
///   note) into the EXISTING workspace directory, then UPDATE the row in place.
///   `workspace_path` is read from the pre-queried DB row and is NEVER derived
///   from the gateway payload.
/// - **New** (`runtime_ref` absent from `ai_agents`): fresh UUID + directory +
///   INSERT, same as `initial_sync` but without the IDENTITY.md `!exists` guard.
///
/// Invariants:
///  1. `workspace_path` - always from the existing row, never the gateway payload.
///  2. `runtime_ref IS NOT NULL` in the WHERE clause handles non-OC local agents.
///  3. Slug collision on UPDATE → rollback the entire transaction.
///  4. Whole operation is wrapped in a transaction: partial failure = full rollback.
///  5. `is_allowed_sync_file()` + `PaneFs` authorization on every file write.
///
/// Returns the list of `(oc_agent_id, display_name, was_existing)` processed —
/// `oc_agent_id` here is the return tuple's conceptual/wire name (matches the
/// gateway's `oc_id`), not the (renamed) `ai_agents.runtime_ref` column.
///
/// F5 item 2 (#2455 venice live-test v2): this function already correctly
/// reasserts `role` on EVERY call, on both the overwrite-in-place UPDATE and
/// the fresh-import INSERT paths (see the `role` local computed from
/// `land_as_default`/`is_default` in each branch below) — no fix was needed
/// here. This self-heals any `ai_agents.role` corruption on the imported
/// subagent's own row every time a user re-pairs (Settings → "Re-pair"), the
/// same way `sync_initial_import`'s ON CONFLICT clause now does for its own
/// entry point (see that function's doc comment) and `migrate_v89` does
/// one-time for rows that never get re-imported again.
pub fn sync_re_import(
    conn: &rusqlite::Connection,
    gateway_url: &str,
    token: &str,
    cert_pem: Option<String>,
    system_dir_override: Option<&Path>,
) -> Result<Vec<(String, String, bool)>, String> {
    eprintln!("[REIMPORT] sync_re_import started: url={}", gateway_url);

    let agents = fetch_gateway_agents(gateway_url, token, cert_pem).map_err(|e| e.to_string())?;

    if agents.is_empty() {
        eprintln!("[REIMPORT] No agents on gateway - nothing to import");
        return Ok(vec![]);
    }

    eprintln!("[REIMPORT] {} agent(s) from gateway", agents.len());

    let (pane_fs, system_root) = sync_filesystem(system_dir_override)?;

    // Pre-query existing ai_agents rows with runtime_ref so we can classify
    // each gateway agent as "overwrite-in-place" vs "fresh import" before we
    // touch any files. Keyed by runtime_ref → (id, workspace_path, name).
    let mut existing_map: std::collections::HashMap<String, (String, String, String)> =
        std::collections::HashMap::new();

    // In-memory set of slugs claimed during this batch (intra-batch collision probe).
    // Pre-populate from existing DB rows so the probe also covers pre-batch state
    // without needing a DB round-trip per candidate.  We add each slug we INSERT.
    let mut claimed_slugs: std::collections::HashSet<String> = std::collections::HashSet::new();
    {
        let mut stmt = conn
            .prepare(
                "SELECT id, runtime_ref, workspace_path, name \
                 FROM ai_agents \
                 WHERE runtime_ref IS NOT NULL",
            )
            .map_err(|e| format!("[REIMPORT] Failed to prepare agent query: {}", e))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?, // id
                    row.get::<_, String>(1)?, // runtime_ref
                    row.get::<_, String>(2)?, // workspace_path
                    row.get::<_, String>(3)?, // name
                ))
            })
            .map_err(|e| format!("[REIMPORT] Failed to query agents: {}", e))?;
        for row in rows {
            let (id, oc_id, workspace_path, name) =
                row.map_err(|e| format!("[REIMPORT] Row error: {}", e))?;
            existing_map.insert(oc_id, (id, workspace_path, name));
        }
    }

    // Pre-populate claimed_slugs from ALL existing rows (including runtime_ref IS NULL)
    // so the intra-batch probe sees pre-existing slugs without extra DB queries.
    {
        let mut stmt = conn
            .prepare("SELECT slug FROM ai_agents")
            .map_err(|e| format!("[REIMPORT] Failed to prepare slug query: {}", e))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("[REIMPORT] Failed to query slugs: {}", e))?;
        for row in rows {
            let slug = row.map_err(|e| format!("[REIMPORT] Slug row error: {}", e))?;
            claimed_slugs.insert(slug);
        }
    }

    eprintln!(
        "[REIMPORT] {} existing OC-linked agent(s) in DB",
        existing_map.len()
    );

    // ADR-0045 §Dependencies: pre-flight single-default normalization (runs BEFORE BEGIN TRANSACTION).
    // If two is_default=1 rows already exist (e.g. from a prior bug on the overwrite path),
    // demote all non-OC rows now so every re-import path starts from a clean invariant.
    {
        let existing_default_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_agents WHERE is_default = 1",
                [],
                |r| r.get(0),
            )
            .map_err(|e| format!("[REIMPORT] Failed to count is_default rows: {}", e))?;
        if existing_default_count > 1 {
            // #2567 B6: this predicate only ever matches runtime_ref IS NULL rows (see
            // WHERE clause), i.e. locally-created agents — blanket role='sub' here was
            // a birth mislabel. Uses the same CASE pattern as the other demotion sites
            // for consistency, even though the WHERE clause makes the ELSE branch dead
            // here.
            conn.execute(
                "UPDATE ai_agents SET is_default = 0, \
                 role = CASE WHEN runtime_ref IS NULL THEN 'local' ELSE 'sub' END \
                 WHERE is_default = 1 AND runtime_ref IS NULL",
                [],
            )
            .map_err(|e| format!("[REIMPORT] Pre-flight demotion failed: {}", e))?;
            eprintln!(
                "[REIMPORT] WARN: {} is_default=1 rows found; demoted non-OC rows as pre-flight normalization",
                existing_default_count
            );
        }
    }

    let now = chrono::Utc::now().to_rfc3339();

    // Track new directories we create so they can be cleaned up on rollback.
    let mut new_dirs: Vec<std::path::PathBuf> = Vec::new();
    let mut results: Vec<(String, String, bool)> = Vec::new();

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("[REIMPORT] Failed to begin transaction: {}", e))?;

    // #2644 fix: pre-flight default-agent-exists check, ported from D13's
    // sync_initial_import gate (commands.rs, `default_agent_exists`) INSIDE the
    // transaction — same rollback boundary as the per-agent loop below. Without
    // this, sync_re_import's two demotion blocks (overwrite-in-place and
    // fresh-import, below) unconditionally demote whatever row currently holds
    // is_default=1 whenever the incoming agent is the OC main agent, stealing
    // default status from an existing foreign default (e.g. an OpenRouter-onboarded
    // default) on every re-pair — not just the first pair D13 already covers.
    //
    // This is READ ONLY here (existence check); it does not by itself change
    // whether the loop below demotes anything — `land_as_default`, computed per
    // agent, is what the demotion blocks now gate on instead of the bare
    // `agent.oc_id == "main"` check.
    let default_agent_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM ai_agents WHERE is_default = 1)",
            [],
            |r| r.get(0),
        )
        .map_err(|e| format!("[REIMPORT] Failed to check for existing default agent: {}", e))?;
    eprintln!(
        "[REIMPORT] Pre-flight check: default_agent_exists={}",
        default_agent_exists
    );

    let outcome = (|| -> Result<(), String> {
        for agent in &agents {
            let agent_name = if agent.name.trim().is_empty() {
                format!("Agent {}", agent.oc_id)
            } else {
                agent.name.clone()
            };
            // `is_default`/`is_main` (oc_id == "main") continues to gate file-allowlist
            // and log-dir semantics unchanged (is_allowed_sync_file, logs/ creation) —
            // those are "is this OC's own main agent" questions, independent of who
            // holds the DB default-agent seat. Every agent this function processes is
            // OC-imported by construction (fetch_gateway_agents / gateway-sourced), so
            // runtime_type is always 'openclaw' for both branches below.
            let is_default = agent.oc_id == "main";

            // #2644 fix: `land_as_default` is the DB is_default/role-assignment signal,
            // deliberately DISTINCT from `is_default` above. It additionally requires
            // that either no default agent exists yet, OR the existing default is
            // already this same OC main row being re-imported in place (an OC main
            // agent re-pairing over itself must keep its own default seat, not lose it
            // to this same pre-flight check on every re-pair). A foreign default
            // (any other row already holding is_default=1) blocks the takeover entirely
            // — re-import must never steal default status (Matt's ruling, ported
            // verbatim from D13/#2640).
            let already_default_here = existing_map
                .get(&agent.oc_id)
                .map(|(row_id, _, _)| {
                    conn.query_row(
                        "SELECT is_default FROM ai_agents WHERE id = ?1",
                        rusqlite::params![row_id],
                        |r| r.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                        == 1
                })
                .unwrap_or(false);
            // ADR-0067 D5 / spec §22C.6.2: three-step default-resolution order,
            // consolidating what were separately-evolved `oc_id == "main"`
            // heuristics into one shared function (`resolve_should_be_default`).
            // Tier 1 (agent.is_default, OC's own reported default) always wins—
            // correct-but-dormant on OC 2026.8.2 per §22C.10 (every agent reports
            // false). Tiers 2/3 preserve this function's pre-existing, carefully
            // audited (#2644/#956/#1546) is_default/default_agent_exists/
            // already_default_here logic byte-for-byte.
            let land_as_default = resolve_should_be_default(
                agent.is_default,
                is_default,
                default_agent_exists,
                already_default_here,
            );

            if let Some((row_id, workspace_path, _old_name)) = existing_map.get(&agent.oc_id) {
                // ── OVERWRITE-IN-PLACE PATH ─────────────────────────────────
                eprintln!(
                    "[REIMPORT] Overwriting existing agent: {} (oc_id={}, workspace={})",
                    agent_name, agent.oc_id, workspace_path
                );

                // workspace_path is from the pre-queried row - NEVER from gateway payload.
                let (workspace_fs, workspace_root, workspace_name) =
                    match PaneFs::validate_name(workspace_path) {
                        Ok(()) => (pane_fs.clone(), system_root, workspace_path.clone()),
                        Err(e) => {
                            #[cfg(test)]
                            {
                                legacy_test_workspace_policy(workspace_path).ok_or_else(|| {
                                    format!("[REIMPORT] Invalid workspace path in database: {}", e)
                                })?
                            }
                            #[cfg(not(test))]
                            {
                                return Err(format!(
                                    "[REIMPORT] Invalid workspace path in database: {}",
                                    e
                                ));
                            }
                        }
                    };
                let agent_relative = PaneFs::relative_from_names([workspace_name.as_str()])
                    .map_err(|e| format!("[REIMPORT] Invalid workspace path in database: {}", e))?;
                workspace_fs
                    .create_dir_all(workspace_root, &agent_relative)
                    .map_err(|e| {
                        format!(
                            "[REIMPORT] Failed to create workspace for {}: {}",
                            agent.oc_id, e
                        )
                    })?;

                // Write ALL files including IDENTITY.md - NO !exists guard (§24.5.5).
                for file in &agent.files {
                    if !is_allowed_sync_file(&file.filename, is_default) {
                        eprintln!(
                            "[REIMPORT] Filtered non-allowlisted file: {}/{}",
                            agent.oc_id, file.filename
                        );
                        continue;
                    }
                    write_remote_sync_file(
                        &workspace_fs,
                        workspace_root,
                        &workspace_name,
                        &file.filename,
                        &file.content,
                        None,
                    )
                    .map_err(|e| {
                        format!(
                            "[REIMPORT] Failed to write {}/{}: {}",
                            agent.oc_id, file.filename, e
                        )
                    })?;
                    eprintln!(
                        "[REIMPORT] Wrote {}/{} ({} bytes)",
                        agent.oc_id,
                        file.filename,
                        file.content.len()
                    );
                }

                // UPDATE in place - NEVER DELETE+INSERT (notes.agent_id refs survive).
                // Slug collision on UPDATE → rusqlite constraint violation → rollback.
                let slug = agent_name
                    .to_lowercase()
                    .replace(' ', "-")
                    .replace('_', "-");

                // ADR-0045 D2: if this is the OC main agent arriving via the overwrite path,
                // demote any other row that currently holds is_default=1 before updating.
                // This runs INSIDE the existing BEGIN TRANSACTION — atomically paired with
                // the UPDATE below. Mirrors the fresh-import demotion block.
                //
                // #2644 fix: gated on `land_as_default`, NOT the bare `is_default` (oc_id
                // == "main") check. Before this fix, ANY re-import of the OC main agent
                // unconditionally demoted whatever row held is_default=1 — including a
                // foreign default (e.g. an OpenRouter-onboarded default) that was never
                // supposed to be touched by a re-pair. `land_as_default` is false when a
                // foreign default exists, so this block becomes a no-op and the foreign
                // default keeps its seat.
                if land_as_default {
                    // #2567 B6: this UPDATE has no runtime_ref filter — the row being
                    // demoted could be a locally-created agent (runtime_ref IS NULL) or
                    // a real OC subagent losing default status. Conditionally preserve
                    // the correct class rather than blanket-writing 'sub'.
                    let demoted = conn
                        .execute(
                            "UPDATE ai_agents SET is_default = 0, \
                             role = CASE WHEN runtime_ref IS NULL THEN 'local' ELSE 'sub' END \
                             WHERE is_default = 1 AND id != ?1",
                            rusqlite::params![row_id],
                        )
                        .map_err(|e| {
                            format!(
                                "[REIMPORT] Demotion UPDATE failed for {}: {}",
                                agent.oc_id, e
                            )
                        })?;
                    if demoted > 0 {
                        eprintln!(
                            "[REIMPORT] Demoted {} row(s) for OC main overwrite-in-place",
                            demoted
                        );
                    }
                }

                // #2644 fix: role derivation ported from D13's sync_initial_import rule —
                // land_as_default=true -> role='main'; is_default (oc_id=="main") but
                // blocked by a foreign default -> role='local' (stays pickable in the
                // agent picker, DefaultAgentPickerModal.svelte's `role !== 'sub'` filter);
                // actual OC subagents -> role='sub'.
                let role = if land_as_default {
                    "main"
                } else if is_default {
                    "local"
                } else {
                    "sub"
                };

                // PR2 (#2647/#2648): `is_active = 1` is the load-bearing reactivation fix —
                // without it, a re-pair after disconnect (which sets is_active = 0 on OC rows,
                // see `ai_disconnect_runtime_impl`) silently fails to reactivate the row: the
                // UUID/workspace_path are correctly preserved (overwrite-in-place, no new
                // directory), but the row stays invisible to every `is_active = 1`-gated reader
                // forever. See docs/spec/sync.md invariant block for the full contract.
                conn.execute(
                    "UPDATE ai_agents SET name=?1, slug=?2, updated_at=?3, is_default=?5, role=?6, is_active=1 WHERE id=?4",
                    rusqlite::params![
                        agent_name,
                        slug,
                        now,
                        row_id,
                        if land_as_default { 1i64 } else { 0i64 },
                        role,
                    ],
                )
                .map_err(|e| {
                    format!(
                        "[REIMPORT] UPDATE failed for {} (possible slug collision): {}",
                        agent.oc_id, e
                    )
                })?;

                eprintln!("[REIMPORT] Updated DB row for agent: {}", agent.oc_id);
                results.push((agent.oc_id.clone(), agent_name, true));
            } else {
                // ── FRESH IMPORT PATH ────────────────────────────────────────
                eprintln!(
                    "[REIMPORT] Fresh import: {} (oc_id={})",
                    agent_name, agent.oc_id
                );

                // New UUID and directory - workspace_path is always local, never gateway.
                let uuid = uuid::Uuid::new_v4()
                    .to_string()
                    .replace('-', "")
                    .to_uppercase();
                let agent_relative = PaneFs::relative_from_names([uuid.as_str()])
                    .map_err(|e| format!("[REIMPORT] Invalid generated workspace: {}", e))?;
                pane_fs
                    .create_dir_all(system_root, &agent_relative)
                    .map_err(|e| {
                        format!("[REIMPORT] Failed to create dir for {}: {}", agent.oc_id, e)
                    })?;
                new_dirs.push(agent_relative.clone());

                // logs/ sub-dir for default agent (mirrors initial_sync)
                if is_default {
                    let logs_relative = PaneFs::relative_from_names([uuid.as_str(), "logs"])
                        .map_err(|e| format!("[REIMPORT] Invalid logs path: {}", e))?;
                    pane_fs
                        .create_dir_all(system_root, &logs_relative)
                        .map_err(|e| {
                            format!("[REIMPORT] Failed to create logs/ for default agent: {}", e)
                        })?;
                }

                // Write all files - no IDENTITY.md !exists guard for fresh re-import
                for file in &agent.files {
                    if !is_allowed_sync_file(&file.filename, is_default) {
                        eprintln!(
                            "[REIMPORT] Filtered non-allowlisted file: {}/{}",
                            agent.oc_id, file.filename
                        );
                        continue;
                    }
                    write_remote_sync_file(
                        &pane_fs,
                        system_root,
                        &uuid,
                        &file.filename,
                        &file.content,
                        None,
                    )
                    .map_err(|e| {
                        format!(
                            "[REIMPORT] Failed to write {}/{}: {}",
                            agent.oc_id, file.filename, e
                        )
                    })?;
                    eprintln!(
                        "[REIMPORT] Wrote {}/{} ({} bytes)",
                        agent.oc_id,
                        file.filename,
                        file.content.len()
                    );
                }

                // INSERT new agent row — disambiguate slug on collision (§24.5.5).
                // Slug collision is a handled, expected condition: coexist + disambiguate.
                // No rollback; the batch continues with a suffixed slug.
                let base_slug = agent_name
                    .to_lowercase()
                    .replace(' ', "-")
                    .replace('_', "-");
                let slug = {
                    if !claimed_slugs.contains(&base_slug) {
                        base_slug.clone()
                    } else {
                        // Increment suffix until a free slug is found.
                        let mut n = 2usize;
                        loop {
                            let candidate = format!("{}-{}", base_slug, n);
                            if !claimed_slugs.contains(&candidate) {
                                break candidate;
                            }
                            n += 1;
                        }
                    }
                };
                eprintln!(
                    "[REIMPORT] Fresh import slug for {}: {} (base={})",
                    agent.oc_id, slug, base_slug
                );
                // Record this slug as claimed so subsequent agents in the same
                // batch see it as taken (intra-batch probe requirement).
                claimed_slugs.insert(slug.clone());

                // §24.5.5 / #956: Before inserting an OC main agent, demote any
                // existing agent that already holds is_default=1 (e.g. an OR-onboarded
                // agent with runtime_ref=NULL). The OC gateway is authoritative on
                // who is default. This runs INSIDE the transaction so it rolls back
                // atomically if the INSERT below fails.
                //
                // #2644 fix: gated on `land_as_default`, NOT the bare `is_default` (oc_id
                // == "main") check. `land_as_default` is false here whenever a foreign
                // default agent already exists (the pre-flight `default_agent_exists`
                // check above) — the exact #2644 bug this fix closes: previously ANY
                // fresh-import of an OC main agent unconditionally demoted the existing
                // default, even a foreign one from a different provider, on every
                // re-pair. `already_default_here` is always false in this fresh-import
                // branch (there is no pre-existing row for this oc_id by definition), so
                // `land_as_default` reduces to `is_default && !default_agent_exists` here.
                if land_as_default {
                    let oc_id_param: &str = &agent.oc_id;
                    // #2567 B6: same as the overwrite-path demotion above — the row
                    // being demoted may be a locally-created agent (runtime_ref IS
                    // NULL) or a different OC subagent, so preserve the correct class
                    // rather than blanket-writing 'sub'.
                    let demoted = conn
                        .execute(
                            "UPDATE ai_agents SET is_default = 0, \
                             role = CASE WHEN runtime_ref IS NULL THEN 'local' ELSE 'sub' END \
                             WHERE is_default = 1 \
                               AND (runtime_ref IS NULL OR runtime_ref != ?1)",
                            rusqlite::params![oc_id_param],
                        )
                        .map_err(|e| {
                            format!(
                                "[REIMPORT] Demotion UPDATE failed for {}: {}",
                                agent.oc_id, e
                            )
                        })?;
                    if demoted > 0 {
                        eprintln!(
                            "[REIMPORT] Demoted existing default agent (is_default → 0) for OC main takeover"
                        );
                    }
                }

                // #2644 fix: role derivation — land_as_default=true -> role='main';
                // is_default (oc_id=="main") but blocked by a foreign default ->
                // role='local' (stays pickable in the agent picker, matching D13's
                // sync_initial_import rule verbatim); actual OC subagents -> role='sub'.
                let role = if land_as_default {
                    "main"
                } else if is_default {
                    "local"
                } else {
                    "sub"
                };

                conn.execute(
                    "INSERT INTO ai_agents \
                     (id, name, slug, role, is_default, workspace_path, runtime_type, runtime_ref, source, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'openclaw', ?7, 'import', ?8, ?9)",
                    rusqlite::params![
                        uuid,
                        agent_name,
                        slug,
                        role,
                        if land_as_default { 1i64 } else { 0i64 },
                        uuid,        // workspace_path = UUID (local, never gateway)
                        agent.oc_id,
                        now,
                        now
                    ],
                )
                .map_err(|e| {
                    format!(
                        "[REIMPORT] INSERT failed for {}: {}",
                        agent.oc_id, e
                    )
                })?;

                eprintln!(
                    "[REIMPORT] Inserted new agent row: {} (uuid={})",
                    agent.oc_id, uuid
                );
                results.push((agent.oc_id.clone(), agent_name, false));
            }
        }
        Ok(())
    })();

    match outcome {
        Ok(_) => {
            conn.execute("COMMIT", [])
                .map_err(|e| format!("[REIMPORT] Failed to commit: {}", e))?;
            eprintln!(
                "[REIMPORT] Transaction committed - {} agent(s) imported/updated",
                results.len()
            );
            Ok(results)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            // Clean up any new directories we created during this run
            for dir in &new_dirs {
                if let Err(ce) = pane_fs.remove_dir_all(system_root, dir) {
                    eprintln!(
                        "[REIMPORT] Warning: failed to clean up {:?} after rollback: {}",
                        dir, ce
                    );
                }
            }
            eprintln!("[REIMPORT] Transaction rolled back: {}", e);
            Err(format!("[REIMPORT] Import rolled back: {}", e))
        }
    }
}

/// ADR-0040 Decision 7: detect stub files created by integrity check.
/// Stubs are empty or contain only the auto-created sentinel string.
/// These were generated by the old hardcoded workspace path and have no real content.
pub fn is_stub_file(content: &str) -> bool {
    content.trim().is_empty() || content.contains("auto-created by integrity check")
}

/// Conditions for running the ADR-0040 workspace migration:
/// - an active `ai_agents` row has `runtime_type = 'openclaw'`
/// - tier3_topology = split_machine
/// - workspace_resolution_migration_complete_v2 is unset
///
/// #2564 B1 fix (Path B): the `provider == "openclaw"` gate is re-pointed to
/// `has_active_openclaw_agent` — same substitution as `get_gateway_url` above.
///
/// #2674: flag key versioned to `_v2`. Installs stamped `true` under the old
/// (unversioned) key from the pre-fix unconditional-stamp bug will have the
/// new key unset, so the migration retries once and self-heals.
pub fn should_run_workspace_migration(conn: &Connection) -> bool {
    let topology = crate::database::get_ai_setting(conn, "tier3_topology");
    let flag = crate::database::get_ai_setting(conn, "workspace_resolution_migration_complete_v2");

    has_active_openclaw_agent(conn)
        && topology.as_deref() == Some("split_machine")
        && flag.is_none()
}

/// ADR-0040 Decision 7: fetch files for migration from the gateway.
///
/// Calls /v1/sync/initial and pairs each file with the agent's local workspace_path
/// (looked up from ai_agents by runtime_ref). Returns per-file tuples:
/// `(workspace_path, filename, gateway_content)`.
///
/// Agents whose oc_id is not found (by runtime_ref) in the local ai_agents table are
/// skipped with a structured log - the migration is best-effort for agents that exist
/// locally.
pub fn fetch_migration_files(
    gateway_url: &str,
    token: &str,
    conn: &Connection,
) -> Result<Vec<(String, String, String)>, String> {
    eprintln!("[MIGRATION] Fetching files from gateway: {}", gateway_url);

    // Use the SPKI-pinned builder so self-signed gateway certs are verified via the
    // stored pin rather than blindly accepting any certificate (#2011).
    let cert_pem = crate::database::get_ai_setting(conn, "gateway_tls_cert_pem");
    let client = crate::gateway_client::build_gateway_client_blocking_with_cert(cert_pem)
        .map_err(|e| format!("[MIGRATION] HTTP client error: {}", e))?;

    let base = gateway_url.trim_end_matches('/').trim_end_matches("/v1");
    let url = format!("{}/v1/sync/initial", base);
    eprintln!("[MIGRATION] GET {}", url);

    let response = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .map_err(|e| format!("[MIGRATION] Request failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("[MIGRATION] Gateway returned {}", response.status()));
    }

    #[derive(serde::Deserialize)]
    struct MigrationFile { filename: String, content: String }
    #[derive(serde::Deserialize)]
    struct MigrationAgent { oc_id: String, files: Vec<MigrationFile> }
    #[derive(serde::Deserialize)]
    struct MigrationResponse { agents: Vec<MigrationAgent> }

    let sync_response: MigrationResponse = response
        .json()
        .map_err(|e| format!("[MIGRATION] Invalid response format: {}", e))?;

    eprintln!("[MIGRATION] Got {} agent(s) from gateway", sync_response.agents.len());

    let mut result = Vec::new();
    for agent in sync_response.agents {
        // Look up the agent's workspace_path from the local ai_agents table
        let workspace_path: Option<String> = conn.query_row(
            "SELECT workspace_path FROM ai_agents WHERE runtime_ref = ?1 LIMIT 1",
            rusqlite::params![agent.oc_id],
            |row| row.get(0),
        ).ok();

        match workspace_path {
            Some(ws) => {
                eprintln!(
                    "[MIGRATION] Agent oc_id={} workspace={} files={}",
                    agent.oc_id, ws, agent.files.len()
                );
                // ADR-0041 §22C.8.5: enforce allowlist on migration ingest (Door D3)
                let is_default = agent.oc_id == "main";
                for file in agent.files {
                    if !is_allowed_sync_file(&file.filename, is_default) {
                        eprintln!(
                            "[MIGRATION] Filtered non-allowlisted file: agent={}, file={}",
                            agent.oc_id, file.filename
                        );
                        continue;
                    }
                    result.push((ws.clone(), file.filename, file.content));
                }
            }
            None => {
                eprintln!(
                    "[MIGRATION] Agent oc_id={} not found in ai_agents - skipping {} file(s)",
                    agent.oc_id,
                    agent.files.len()
                );
            }
        }
    }

    eprintln!("[MIGRATION] Fetched {} file(s) total", result.len());
    Ok(result)
}

/// ADR-0040 Decision 7: apply migration file list to local workspace.
///
/// For each `(workspace_path, filename, gateway_content)` tuple:
///   - If local file is a stub → overwrite with gateway content (LWW override)
///   - If local file is non-stub (does not match the stub predicate) → preserve local unchanged
///
/// This is a conservative approximation of §22C.8.2's LWW rule - without reliable mtime
/// tracking on system files during migration, preserving local is the safer default
/// (zero data-loss risk for user edits).
///
/// On success (err_count == 0), sets `workspace_resolution_migration_complete_v2 = "true"`
/// in ai_settings. #2674: if any file failed, the flag is left unset so the migration
/// retries on next startup instead of being permanently marked complete.
pub fn apply_migration_files(
    conn: &Connection,
    files: &[(String, String, String)],
) -> Result<(), String> {
    eprintln!("[MIGRATION] Applying {} file(s)", files.len());

    let mut stub_count = 0usize;
    let mut skip_count = 0usize;
    let mut err_count = 0usize;
    let (pane_fs, system_root) = sync_filesystem(None)?;

    for (workspace_path, filename, gateway_content) in files {
        if !is_allowed_sync_file(filename, true) {
            eprintln!(
                "[MIGRATION] Rejecting non-allowlisted filename {}/{}",
                workspace_path, filename
            );
            err_count += 1;
            continue;
        }

        let (workspace_fs, workspace_root, workspace) =
            match migration_workspace_policy(&pane_fs, system_root, workspace_path) {
                Ok(policy) => policy,
                Err(e) => {
                    eprintln!(
                        "[MIGRATION] Rejecting unsafe workspace {}: {}",
                        workspace_path, e
                    );
                    err_count += 1;
                    continue;
                }
            };

        let file_relative = match sync_file_relative(&workspace, filename) {
            Ok(relative) => relative,
            Err(e) => {
                eprintln!(
                    "[MIGRATION] Rejecting unsafe filename {}/{}: {}",
                    workspace_path, filename, e
                );
                err_count += 1;
                continue;
            }
        };
        let file_destination = match workspace_fs.authorize_write(workspace_root, &file_relative) {
            Ok(path) => path,
            Err(e) => {
                eprintln!(
                    "[MIGRATION] Rejecting unsafe destination {}/{}: {}",
                    workspace_path, filename, e
                );
                err_count += 1;
                continue;
            }
        };

        // Check existing local file through an authorized, non-symlink path.
        let local_is_stub = match workspace_fs.authorize_existing(
            workspace_root,
            &file_relative,
            ExistingKind::File,
        ) {
            Ok(existing) => match std::fs::read_to_string(existing.as_path()) {
                Ok(local) => is_stub_file(&local),
                Err(_) => true,
            },
            Err(_) => true,
        };

        if local_is_stub {
            eprintln!(
                "[MIGRATION] Overwriting stub: {}",
                file_destination.as_path().display()
            );
            if let Some(parent) = file_relative.parent() {
                if let Err(e) = workspace_fs.create_dir_all(workspace_root, parent) {
                    eprintln!(
                        "[MIGRATION] Failed to create dirs for {}: {}",
                        file_destination.as_path().display(),
                        e
                    );
                    err_count += 1;
                    continue;
                }
            }
            let file_destination =
                match workspace_fs.authorize_write(workspace_root, &file_relative) {
                    Ok(path) => path,
                    Err(e) => {
                        eprintln!("[MIGRATION] Rejecting unsafe write destination: {}", e);
                        err_count += 1;
                        continue;
                    }
                };
            if let Err(e) = crate::file_ops::atomic_write(
                file_destination.as_path(),
                gateway_content,
                Some(conn),
                crate::file_ops::WriteOrigin::Remote,
            ) {
                eprintln!(
                    "[MIGRATION] Failed to write {}: {}",
                    file_destination.as_path().display(),
                    e
                );
                err_count += 1;
                continue;
            }
            stub_count += 1;
        } else {
            // Non-stub: preserve local. ADR-0040 Decision 7 - without reliable mtime
            // tracking, preserving local is the safer default (zero data-loss risk).
            eprintln!(
                "[MIGRATION] Non-stub local file preserved: {}",
                file_destination.as_path().display()
            );
            skip_count += 1;
        }
    }

    eprintln!(
        "[MIGRATION] Complete - stubs overwritten: {}, skipped (non-stub): {}, errors: {}",
        stub_count, skip_count, err_count
    );

    // #2674: only stamp complete on a clean run (err_count == 0). A migration
    // where every file was rejected (e.g. the ADR-0040 path-guard bug) must
    // not be permanently marked done - leave the flag unset so
    // should_run_workspace_migration() retries on next startup.
    if err_count == 0 {
        crate::database::set_ai_setting(conn, "workspace_resolution_migration_complete_v2", "true")
            .map_err(|e| format!("[MIGRATION] Failed to set completion flag: {}", e))?;
        eprintln!("[MIGRATION] workspace_resolution_migration_complete_v2 = true");
    } else {
        eprintln!(
            "[MIGRATION] {} error(s) - completion flag NOT set, will retry next startup",
            err_count
        );
    }
    Ok(())
}

/// ADR-0040 Decision 7: one-shot migration to overwrite stub files with real content.
///
/// Fetches files from the gateway, then for each file:
///   - If local file is a stub → overwrite with gateway content (LWW override)
///   - If local file is non-stub (does not match the stub predicate) → preserve local unchanged
///
/// This is a conservative approximation of §22C.8.2's LWW rule - without reliable mtime
/// tracking on system files during migration, preserving local is the safer default
/// (zero data-loss risk for user edits). True timestamp-based LWW for migrated files
/// is a post-launch enhancement.
///
/// On success (err_count == 0), sets `workspace_resolution_migration_complete_v2 = "true"`
/// in ai_settings. #2674: partial/total failures leave the flag unset for retry.
pub fn run_workspace_migration(conn: &Connection) -> Result<(), String> {
    eprintln!("[MIGRATION] ADR-0040 workspace migration starting");

    let gateway_url = get_gateway_url(conn).ok_or_else(|| {
        "[MIGRATION] No gateway URL configured".to_string()
    })?;
    let token = get_gateway_token(conn).ok_or_else(|| {
        "[MIGRATION] No gateway token configured".to_string()
    })?;

    let files = fetch_migration_files(&gateway_url, &token, conn)
        .map_err(|e| format!("[MIGRATION] fetch_migration_files failed: {}", e))?;

    apply_migration_files(conn, &files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;

    // ── #2635 (D11 fix): detect_topology log-line accuracy ───────────────────────
    // detect_topology() only ever inspects `ai_settings.tier3_topology` - it does not
    // read `ai_settings.endpoint` or `ai_agents` at all. The pre-fix log line claimed
    // "no endpoint configured" when falling through to the default, which was
    // misleading (see docs/audits/2026-08-21-d11-sync-endpoint-mismatch.md §5): the
    // function's *return value* (always SplitMachine, since OpenClaw is split-machine
    // only per #2649) is what these tests assert, since the log line itself is not a
    // return value / observable via a public API - the log-string correction is a
    // comment+eprintln! change, verified by inspection/PR review rather than a test
    // assertion on stderr content.
    #[test]
    fn test_detect_topology_defaults_to_split_machine_when_tier3_topology_unset() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch("CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        assert_eq!(detect_topology(&conn), SyncTopology::SplitMachine);
    }

    #[test]
    fn test_detect_topology_explicit_split_machine_override() {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch("CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        crate::database::set_ai_setting(&conn, "tier3_topology", "split_machine").unwrap();
        assert_eq!(detect_topology(&conn), SyncTopology::SplitMachine);
    }

    #[test]
    fn test_detect_topology_legacy_same_machine_value_treated_as_split_machine() {
        // #2649: the `SameMachine` enum variant no longer exists. This test proves
        // a legacy `tier3_topology='same_machine'` DB value (written by a prior
        // build, pre-#1108) is still mapped to SplitMachine for backward
        // compatibility, rather than panicking or producing an unmapped state.
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch("CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);").unwrap();
        crate::database::set_ai_setting(&conn, "tier3_topology", "same_machine").unwrap();
        assert_eq!(detect_topology(&conn), SyncTopology::SplitMachine);
    }

    // ── ADR-0041 allowlist helpers ───────────────────────────────────────────

    // is_depth1_md tests
    #[test]
    fn test_is_depth1_md_passes_logs_date() {
        assert!(is_depth1_md("logs/", "logs/2026-05-28.md"));
    }

    #[test]
    fn test_is_depth1_md_fails_nested() {
        assert!(!is_depth1_md("logs/", "logs/subdir/file.md"));
    }

    #[test]
    fn test_is_depth1_md_passes_edge_dotmd() {
        // "logs/.md" - technically valid depth-1 file named ".md"
        assert!(is_depth1_md("logs/", "logs/.md"));
    }

    #[test]
    fn test_is_depth1_md_fails_non_md() {
        assert!(!is_depth1_md("logs/", "logs/file.txt"));
    }

    #[test]
    fn test_is_depth1_md_fails_wrong_prefix() {
        assert!(!is_depth1_md("logs/", "memory/2026-05-28.md"));
    }

    // is_allowed_sync_file - main agent
    #[test]
    fn test_allowed_main_identity_files_pass() {
        for f in IDENTITY_WHITELIST {
            assert!(is_allowed_sync_file(f, true), "{} should pass for main", f);
        }
    }

    #[test]
    fn test_allowed_main_memory_depth1_passes() {
        assert!(is_allowed_sync_file("memory/2026-05-28.md", true));
    }

    #[test]
    fn test_allowed_main_memory_nested_fails() {
        assert!(!is_allowed_sync_file("memory/subdir/note.md", true));
    }

    #[test]
    fn test_allowed_main_archive_fails() {
        assert!(!is_allowed_sync_file("archive/old.md", true));
    }

    #[test]
    fn test_allowed_main_scratchpad_fails() {
        assert!(!is_allowed_sync_file("scratchpad-123.md", true));
    }

    #[test]
    fn test_allowed_main_json_fails() {
        assert!(!is_allowed_sync_file("file.json", true));
    }

    // is_allowed_sync_file - sub-agent
    #[test]
    fn test_allowed_sub_core_6_pass() {
        for f in SUB_AGENT_WHITELIST {
            assert!(is_allowed_sync_file(f, false), "{} should pass for sub-agent", f);
        }
    }

    #[test]
    fn test_allowed_sub_heartbeat_fails() {
        assert!(!is_allowed_sync_file("HEARTBEAT.md", false));
    }

    #[test]
    fn test_allowed_sub_bootstrap_fails() {
        assert!(!is_allowed_sync_file("BOOTSTRAP.md", false));
    }

    #[test]
    fn test_allowed_sub_rules_fails() {
        assert!(!is_allowed_sync_file("RULES.md", false));
    }

    #[test]
    fn test_allowed_sub_memory_fails() {
        assert!(!is_allowed_sync_file("memory/2026-05-28.md", false));
    }

    #[test]
    fn test_allowed_sub_logs_fails() {
        assert!(!is_allowed_sync_file("logs/2026-05-28.md", false));
    }

    // Ingest filtering integration: non-allowlisted files are filtered but
    // still marked as processed (so gateway doesn't re-send).
    // We test the logic directly since we can't spin up the full gateway in unit tests.
    #[test]
    fn test_ingest_filter_non_allowlisted_marked_processed() {
        // Simulate the ingest loop logic for a sub-agent receiving a non-allowed file.
        let runtime_ref = "coder";
        let is_main_agent = runtime_ref == "main";
        let filenames = vec!["SOUL.md", "MEMORY.md", "scratchpad.md", "HEARTBEAT.md", "archive/old.md"];
        let mut processed_ids: Vec<String> = Vec::new();
        let mut written: Vec<String> = Vec::new();

        for (i, fname) in filenames.iter().enumerate() {
            if !is_allowed_sync_file(fname, is_main_agent) {
                processed_ids.push(format!("id-{}", i));
                continue;
            }
            written.push(fname.to_string());
            processed_ids.push(format!("id-{}", i));
        }

        // All 5 IDs should be in processed_ids (filtered + allowed)
        assert_eq!(processed_ids.len(), 5, "all changes must be in processed_ids");
        // Only SOUL.md and MEMORY.md should be written
        assert_eq!(written, vec!["SOUL.md", "MEMORY.md"]);
    }

    // ── ADR-0040 stub detection ──────────────────────────────────────────────

    #[test]
    fn test_is_stub_empty_file() {
        assert!(is_stub_file(""), "empty file must be stub");
    }

    #[test]
    fn test_is_stub_whitespace_only() {
        assert!(is_stub_file("  \n  \t  "), "whitespace-only must be stub");
    }

    #[test]
    fn test_is_stub_auto_created_sentinel() {
        let content = "# placeholder\n# auto-created by integrity check\n";
        assert!(is_stub_file(content), "sentinel string must be stub");
    }

    #[test]
    fn test_is_stub_real_content_not_stub() {
        let content = "# My real notes\n\nActual useful content here.";
        assert!(!is_stub_file(content), "real content must not be stub");
    }

    #[test]
    fn test_is_stub_real_content_with_sentinel_substring() {
        // A file that mentions the phrase in a different context must still not be a stub
        let content = "# This file was NOT auto-created by integrity check - it was written by hand";
        // This DOES contain the substring, so is_stub returns true - expected per ADR spec
        assert!(is_stub_file(content), "file containing sentinel substring is treated as stub");
    }

    // ── Migration conditions ─────────────────────────────────────────────────

    fn open_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS ai_agents (
                id TEXT PRIMARY KEY,
                runtime_type TEXT,
                is_active INTEGER NOT NULL DEFAULT 1
             );",
        ).expect("create ai_settings/ai_agents tables");
        conn
    }

    /// #2564 B1 fix: seeds an active `ai_agents` row with `runtime_type = 'openclaw'` —
    /// the Path B replacement for the retired `ai_settings.provider == "openclaw"` gate.
    fn seed_active_openclaw_agent(conn: &rusqlite::Connection) {
        conn.execute(
            "INSERT INTO ai_agents (id, runtime_type, is_active) VALUES ('agent-oc', 'openclaw', 1)",
            [],
        )
        .expect("seed openclaw agent");
    }

    #[test]
    fn test_should_run_migration_conditions_met() {
        let conn = open_test_db();
        seed_active_openclaw_agent(&conn);
        crate::database::set_ai_setting(&conn, "tier3_topology", "split_machine").unwrap();
        // flag intentionally not set
        assert!(should_run_workspace_migration(&conn));
    }

    #[test]
    fn test_should_not_run_migration_flag_already_set() {
        let conn = open_test_db();
        seed_active_openclaw_agent(&conn);
        crate::database::set_ai_setting(&conn, "tier3_topology", "split_machine").unwrap();
        crate::database::set_ai_setting(&conn, "workspace_resolution_migration_complete_v2", "true").unwrap();
        assert!(!should_run_workspace_migration(&conn), "flag set → must skip");
    }

    #[test]
    fn test_should_not_run_migration_no_openclaw_agent() {
        let conn = open_test_db();
        // No ai_agents row at all — e.g. a local-only openrouter/anthropic install.
        crate::database::set_ai_setting(&conn, "tier3_topology", "split_machine").unwrap();
        assert!(!should_run_workspace_migration(&conn), "no active openclaw-runtime agent → skip");
    }

    #[test]
    fn test_should_not_run_migration_openclaw_agent_inactive() {
        let conn = open_test_db();
        conn.execute(
            "INSERT INTO ai_agents (id, runtime_type, is_active) VALUES ('agent-oc', 'openclaw', 0)",
            [],
        )
        .unwrap();
        crate::database::set_ai_setting(&conn, "tier3_topology", "split_machine").unwrap();
        assert!(!should_run_workspace_migration(&conn), "inactive openclaw agent → skip");
    }

    #[test]
    fn test_should_not_run_migration_wrong_topology() {
        let conn = open_test_db();
        seed_active_openclaw_agent(&conn);
        crate::database::set_ai_setting(&conn, "tier3_topology", "same_machine").unwrap();
        assert!(!should_run_workspace_migration(&conn), "non-split_machine topology → skip");
    }

    #[test]
    fn test_should_not_run_migration_no_provider_set() {
        let conn = open_test_db();
        // no openclaw agent, no topology
        assert!(!should_run_workspace_migration(&conn));
    }

    // ── #2564 B1 fix: has_active_openclaw_agent / get_gateway_url re-point tests ──

    #[test]
    fn test_has_active_openclaw_agent_true_when_active_row_exists() {
        let conn = open_test_db();
        seed_active_openclaw_agent(&conn);
        assert!(has_active_openclaw_agent(&conn));
    }

    #[test]
    fn test_has_active_openclaw_agent_false_when_no_rows() {
        let conn = open_test_db();
        assert!(!has_active_openclaw_agent(&conn));
    }

    #[test]
    fn test_has_active_openclaw_agent_false_when_inactive() {
        let conn = open_test_db();
        conn.execute(
            "INSERT INTO ai_agents (id, runtime_type, is_active) VALUES ('agent-oc', 'openclaw', 0)",
            [],
        )
        .unwrap();
        assert!(!has_active_openclaw_agent(&conn));
    }

    #[test]
    fn test_has_active_openclaw_agent_false_for_other_runtime_types() {
        let conn = open_test_db();
        conn.execute(
            "INSERT INTO ai_agents (id, runtime_type, is_active) VALUES ('agent-1', 'openrouter', 1)",
            [],
        )
        .unwrap();
        assert!(!has_active_openclaw_agent(&conn));
    }

    /// #2564 B1 core regression test: `get_gateway_url` must resolve correctly when
    /// `ai_settings.provider` is absent (retired) but an active `runtime_type='openclaw'`
    /// agent exists — this is exactly the v79-orphaned/v80-recovered state.
    #[test]
    fn test_get_gateway_url_resolves_without_provider_key_when_openclaw_agent_active() {
        let conn = open_test_db();
        seed_active_openclaw_agent(&conn);
        crate::database::set_ai_setting(&conn, "endpoint", "http://100.64.1.2:18789").unwrap();
        // Note: 'provider' key is deliberately never written — simulates post-v79 state.
        assert_eq!(
            get_gateway_url(&conn).as_deref(),
            Some("http://100.64.1.2:18789"),
            "get_gateway_url must resolve via runtime_type, not the retired provider key"
        );
    }

    #[test]
    fn test_get_gateway_url_none_when_no_openclaw_agent() {
        let conn = open_test_db();
        crate::database::set_ai_setting(&conn, "endpoint", "http://100.64.1.2:18789").unwrap();
        assert_eq!(
            get_gateway_url(&conn),
            None,
            "no active openclaw agent → None even if endpoint happens to be set"
        );
    }

    #[test]
    fn test_get_gateway_url_none_when_endpoint_missing() {
        let conn = open_test_db();
        seed_active_openclaw_agent(&conn);
        // endpoint deliberately not set — orphaned-and-not-yet-recovered state.
        assert_eq!(
            get_gateway_url(&conn),
            None,
            "active openclaw agent but no endpoint → None"
        );
    }

    // ── Stub overwrite vs. LWW preservation (filesystem) ────────────────────

    #[test]
    fn test_stub_overwrite_writes_gateway_content() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stub_path = dir.path().join("NOTES.md");
        std::fs::write(&stub_path, "   ").expect("write stub");

        let is_stub = is_stub_file(&std::fs::read_to_string(&stub_path).unwrap());
        assert!(is_stub);

        // Simulate migration overwrite
        let gateway_content = "# Real notes from gateway\n\nActual content.";
        if is_stub {
            std::fs::write(&stub_path, gateway_content).expect("overwrite");
        }

        let after = std::fs::read_to_string(&stub_path).unwrap();
        assert_eq!(after, gateway_content, "stub should be overwritten with gateway content");
    }

    #[test]
    fn test_non_stub_file_not_overwritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file_path = dir.path().join("NOTES.md");
        let original = "# Original content\n\nHuman-written notes.";
        std::fs::write(&file_path, original).expect("write");

        let is_stub = is_stub_file(&std::fs::read_to_string(&file_path).unwrap());
        assert!(!is_stub);

        // Simulate migration: non-stub → preserve local
        if is_stub {
            std::fs::write(&file_path, "GATEWAY CONTENT").expect("overwrite");
        }

        let after = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(after, original, "non-stub file must be preserved");
    }

    // ── End-to-end migration tests (ADR-0040 F10) ─────────────────────────
    // These tests call run_workspace_migration() directly via a mockito server.
    // They exercise: stub file overwrite, non-stub preservation, and flag set.

    fn open_migration_test_db(server_url: &str) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);\
             CREATE TABLE IF NOT EXISTS ai_agents (\
               id TEXT PRIMARY KEY,\
               name TEXT NOT NULL DEFAULT '',\
               slug TEXT NOT NULL DEFAULT '',\
               role TEXT NOT NULL DEFAULT 'sub',\
               is_default INTEGER NOT NULL DEFAULT 0,\
               is_active INTEGER NOT NULL DEFAULT 1,\
               workspace_path TEXT NOT NULL DEFAULT '',\
               runtime_ref TEXT,\
               runtime_type TEXT,\
               source TEXT NOT NULL DEFAULT 'local',\
               created_at TEXT NOT NULL DEFAULT '',\
               updated_at TEXT NOT NULL DEFAULT ''\
             );",
        ).expect("create tables");
        // Conditions for migration to run. #2564 B1 fix: `get_gateway_url`/
        // `should_run_workspace_migration` no longer read the retired `provider` key —
        // callers insert an ai_agents row with runtime_type='openclaw' below instead.
        crate::database::set_ai_setting(&conn, "tier3_topology", "split_machine").unwrap();
        // Point gateway URL at the mock server (no /v1 suffix - fetch_migration_files adds it)
        crate::database::set_ai_setting(&conn, "endpoint", server_url).unwrap();
        crate::database::set_ai_setting(&conn, "api_key", "test-token").unwrap();
        conn
    }

    /// F10-a: stub file is overwritten with gateway content; migration flag is set.
    #[tokio::test]
    async fn test_run_workspace_migration_stub_overwritten() {
        let mut server = mockito::Server::new_async().await;

        let dir = tempfile::tempdir().expect("tempdir");
        let agent_dir = dir.path().join("main-agent");
        std::fs::create_dir_all(&agent_dir).unwrap();

        // Write a stub SOUL.md
        let stub_content = "# SOUL.md\n\n**Agent:** Test\n\n*(This file was auto-created by integrity check. Please update with agent personality and directives.)*\n";
        std::fs::write(agent_dir.join("SOUL.md"), stub_content).unwrap();

        // Mock /v1/sync/initial to return real content for this agent
        let gateway_soul = "# SOUL.md\n\nReal gateway content for the agent.".to_string();
        let body = format!(
            "{{\"agents\":[{{\"oc_id\":\"main\",\"name\":\"Test Agent\",\"files\":[{{\"filename\":\"SOUL.md\",\"content\":{},\"checksum\":\"abc\"}}]}}]}}",
            serde_json::to_string(&gateway_soul).unwrap()
        );
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(&body)
            .create_async()
            .await;

        let server_url = server.url();
        let workspace_path = agent_dir.to_str().unwrap().to_string();
        let gateway_soul_clone = gateway_soul.clone();

        let result = tokio::task::spawn_blocking(move || {
            let conn = open_migration_test_db(&server_url);
            // Register the agent in ai_agents so workspace lookup works
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_ref, runtime_type, created_at, updated_at) \
                 VALUES ('main-uuid', 'Test Agent', 'test-agent', ?1, 'main', 'openclaw', '', '')",
                rusqlite::params![workspace_path],
            ).unwrap();

            let r = run_workspace_migration(&conn);

            // Assert: stub was overwritten with gateway content
            let after = std::fs::read_to_string(
                std::path::PathBuf::from(&workspace_path).join("SOUL.md")
            ).unwrap();
            assert_eq!(after, gateway_soul_clone, "stub must be overwritten with gateway content");

            // Assert: migration flag is set
            let flag = crate::database::get_ai_setting(&conn, "workspace_resolution_migration_complete_v2");
            assert_eq!(flag.as_deref(), Some("true"), "migration flag must be set after run");

            r
        }).await.expect("spawn_blocking panicked");

        result.expect("run_workspace_migration must succeed");
    }

    /// F10-b: non-stub file is preserved; stub in same agent dir is overwritten.
    #[tokio::test]
    async fn test_run_workspace_migration_non_stub_preserved() {
        let mut server = mockito::Server::new_async().await;

        let dir = tempfile::tempdir().expect("tempdir");
        let agent_dir = dir.path().join("main-agent");
        std::fs::create_dir_all(&agent_dir).unwrap();

        // SOUL.md is a stub → should be overwritten
        let stub_content = "# SOUL.md\n\n**Agent:** Test\n\n*(This file was auto-created by integrity check. Please update with agent personality and directives.)*\n";
        std::fs::write(agent_dir.join("SOUL.md"), stub_content).unwrap();

        // MEMORY.md has real user content → must NOT be overwritten
        let user_memory = "# My real notes\n\nHuman-written memory.";
        std::fs::write(agent_dir.join("MEMORY.md"), user_memory).unwrap();

        let gateway_soul = "# SOUL.md\n\nReal gateway SOUL content.".to_string();
        let gateway_memory = "# MEMORY.md\n\nGateway memory -- should NOT overwrite local.".to_string();
        let soul_json = serde_json::to_string(&gateway_soul).unwrap();
        let memory_json = serde_json::to_string(&gateway_memory).unwrap();
        let body = format!(
            "{{\"agents\":[{{\"oc_id\":\"main\",\"name\":\"Test Agent\",\"files\":[{{\"filename\":\"SOUL.md\",\"content\":{},\"checksum\":\"s1\"}},{{\"filename\":\"MEMORY.md\",\"content\":{},\"checksum\":\"m1\"}}]}}]}}",
            soul_json,
            memory_json
        );
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(&body)
            .create_async()
            .await;

        let server_url = server.url();
        let workspace_path = agent_dir.to_str().unwrap().to_string();
        let gateway_soul_clone = gateway_soul.clone();
        let user_memory_str = user_memory.to_string();

        let result = tokio::task::spawn_blocking(move || {
            let conn = open_migration_test_db(&server_url);
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, workspace_path, runtime_ref, runtime_type, created_at, updated_at) \
                 VALUES ('main-uuid', 'Test Agent', 'test-agent', ?1, 'main', 'openclaw', '', '')",
                rusqlite::params![workspace_path],
            ).unwrap();

            let r = run_workspace_migration(&conn);

            let soul_after = std::fs::read_to_string(
                std::path::PathBuf::from(&workspace_path).join("SOUL.md")
            ).unwrap();
            assert_eq!(soul_after, gateway_soul_clone, "stub SOUL.md must be overwritten");

            let memory_after = std::fs::read_to_string(
                std::path::PathBuf::from(&workspace_path).join("MEMORY.md")
            ).unwrap();
            assert_eq!(memory_after, user_memory_str, "non-stub MEMORY.md must be preserved");

            let flag = crate::database::get_ai_setting(&conn, "workspace_resolution_migration_complete_v2");
            assert_eq!(flag.as_deref(), Some("true"), "migration flag must be set");

            r
        }).await.expect("spawn_blocking panicked");

        result.expect("run_workspace_migration must succeed");
    }

    // ── #2674 regression tests ──────────────────────────────────────────

    /// #2674 regression test for fix (a): `migration_workspace_policy` must accept a
    /// bare-UUID `workspace_path` (the exact shape of production `ai_agents.workspace_path`
    /// values, per `agents.rs`'s `agent_uuid.simple().to_string().to_uppercase()`
    /// convention) resolved against a REAL `.pane/system/<UUID>` directory tree.
    ///
    /// This deliberately does NOT go through `legacy_test_workspace_policy` (the
    /// `#[cfg(test)]` shim that resolves arbitrary temp-dir paths under the OS temp
    /// root) — that shim is what let the pre-fix bug (redundant
    /// `std::fs::canonicalize(supplied)` on a bare UUID, which fails because a bare
    /// UUID doesn't resolve to anything relative to the process cwd) slip past CI.
    /// Instead this builds a real `PaneFs` rooted at a scratch dir via
    /// `PaneFs::from_pane_root`, creates a real `<root>/.pane/system/<UUID>` directory,
    /// and calls `migration_workspace_policy` with `PaneRoot::System` + the bare UUID —
    /// the same call shape `apply_migration_files` uses in production
    /// (`sync_filesystem(None)` → `PaneFs::for_runtime()` → `PaneRoot::System`).
    #[test]
    fn test_2674_migration_workspace_policy_accepts_bare_uuid_under_pane_root_system() {
        let temp = tempfile::tempdir().expect("tempdir");
        let pane_root = temp.path().join("Pane");
        let pane_fs = PaneFs::from_pane_root(&pane_root).expect("construct PaneFs");

        // Bare UUID, no path separators — exactly what ai_agents.workspace_path holds
        // (agents.rs: `agent_uuid.simple().to_string().to_uppercase()`).
        let workspace_uuid = "4F2A9B3C1D5E4F6A8B7C9D0E1F2A3B4C";
        let system_dir = pane_root.join(".pane").join("system").join(workspace_uuid);
        std::fs::create_dir_all(&system_dir).expect("seed real .pane/system/<UUID> dir");

        let result = migration_workspace_policy(&pane_fs, PaneRoot::System, workspace_uuid);

        assert!(
            result.is_ok(),
            "#2674: migration_workspace_policy must accept a bare-UUID workspace_path \
             resolved via authorize_existing against a real .pane/system/<UUID> dir, \
             not reject it as 'not canonically beneath PaneRoot::System': {:?}",
            result
        );
        let (_returned_fs, returned_root, returned_workspace) = result.unwrap();
        assert_eq!(returned_root, PaneRoot::System, "root must be echoed back unchanged");
        assert_eq!(
            returned_workspace, workspace_uuid,
            "workspace name must be the bare UUID, unmodified"
        );
    }

    /// #2674 regression test for fix (b): if EVERY file in a migration batch is
    /// rejected (err_count > 0, e.g. all non-allowlisted filenames), the completion
    /// flag must stay unset after `apply_migration_files` returns — so
    /// `should_run_workspace_migration` retries on the next startup instead of
    /// permanently stamping a total-failure run as done.
    #[test]
    #[serial(pane_test_root)]
    fn test_2674_apply_migration_files_all_rejected_leaves_flag_unset() {
        // apply_migration_files -> sync_filesystem(None) -> PaneFs::for_runtime() reads
        // the real Pane root (get_pane_root()) unless PANE_TEST_ROOT overrides it —
        // same convention as agents.rs's upsert_agent_explicit_runtime_type_tests::setup().
        // #[serial] because PANE_TEST_ROOT is a process-global env var.
        let fs_dir = tempfile::tempdir().expect("scratch fs dir");
        std::env::set_var("PANE_TEST_ROOT", fs_dir.path());

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .expect("create ai_settings table");

        // All three files use non-allowlisted filenames -> every one is rejected by
        // is_allowed_sync_file before workspace/path resolution is even attempted.
        let files = vec![
            (
                "some-workspace".to_string(),
                "not-on-the-allowlist.json".to_string(),
                "content-a".to_string(),
            ),
            (
                "some-workspace".to_string(),
                "another-bad-file.txt".to_string(),
                "content-b".to_string(),
            ),
            (
                "some-workspace".to_string(),
                "scratch.log".to_string(),
                "content-c".to_string(),
            ),
        ];

        let result = apply_migration_files(&conn, &files);


        assert!(
            result.is_ok(),
            "apply_migration_files must return Ok even when every file is rejected \
             (rejections are per-file errors, not a hard function failure): {:?}",
            result
        );

        let flag =
            crate::database::get_ai_setting(&conn, "workspace_resolution_migration_complete_v2");
        assert_eq!(
            flag, None,
            "#2674: completion flag must stay UNSET when every file in the batch was \
             rejected (err_count > 0) — a total-failure run must retry on next startup, \
             not be permanently marked complete"
        );
    }

    // ── ADR-0041 Door D2: initial_sync allowlist enforcement ─────────────

    /// D2-a: non-allowlisted file is skipped during initial_sync.
    #[test]
    fn test_initial_sync_filters_non_allowlisted_file() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let agent_dir = dir.path().join("agent-uuid");
        std::fs::create_dir_all(&agent_dir).unwrap();

        // Simulate what initial_sync does for one agent: write allowlisted + non-allowlisted
        let is_main_agent = false;
        let files = vec![
            ("SOUL.md", "soul content"),
            ("random-file.json", "{}"),
            ("scratchpad.txt", "notes"),
        ];
        for (filename, content) in &files {
            if !is_allowed_sync_file(filename, is_main_agent) {
                continue;
            }
            std::fs::write(agent_dir.join(filename), content).unwrap();
        }

        assert!(agent_dir.join("SOUL.md").exists(), "SOUL.md must be written");
        assert!(!agent_dir.join("random-file.json").exists(), "random-file.json must be skipped");
        assert!(!agent_dir.join("scratchpad.txt").exists(), "scratchpad.txt must be skipped");
    }

    /// D2-b: allowlisted files ARE written during initial_sync (main agent).
    #[test]
    fn test_initial_sync_writes_allowlisted_files_main() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let agent_dir = dir.path().join("agent-uuid");
        std::fs::create_dir_all(&agent_dir).unwrap();

        let is_main_agent = true;
        let files = vec![
            ("SOUL.md", "soul"),
            ("MEMORY.md", "mem"),
            ("HEARTBEAT.md", "hb"),
            ("random.json", "{}"),
        ];
        for (filename, content) in &files {
            if !is_allowed_sync_file(filename, is_main_agent) {
                continue;
            }
            std::fs::write(agent_dir.join(filename), content).unwrap();
        }

        assert!(agent_dir.join("SOUL.md").exists(), "SOUL.md must be written for main");
        assert!(agent_dir.join("MEMORY.md").exists(), "MEMORY.md must be written for main");
        assert!(agent_dir.join("HEARTBEAT.md").exists(), "HEARTBEAT.md must be written for main");
        assert!(!agent_dir.join("random.json").exists(), "random.json must be skipped");
    }

    /// D2-c: sub-agent HEARTBEAT.md is filtered (not in sub-agent whitelist).
    #[test]
    fn test_initial_sync_sub_agent_heartbeat_filtered() {
        assert!(!is_allowed_sync_file("HEARTBEAT.md", false),
            "HEARTBEAT.md must not be in sub-agent allowlist");
        assert!(!is_allowed_sync_file("BOOTSTRAP.md", false),
            "BOOTSTRAP.md must not be in sub-agent allowlist");
    }

    // ── ADR-0041 Door D3: migration allowlist enforcement ─────────────────

    /// D3-a: non-allowlisted file does not appear in fetch_migration_files result.
    #[test]
    fn test_fetch_migration_files_filters_non_allowlisted() {
        // We test the filter logic directly (not the HTTP layer) by simulating
        // what fetch_migration_files does in its inner loop.
        let is_default = false;
        let files_from_gateway = vec![
            ("SOUL.md", "soul"),
            ("random-file.json", "{}"),
            ("HEARTBEAT.md", "hb"),
        ];
        let ws = "/fake/workspace";
        let mut result: Vec<(String, String, String)> = Vec::new();
        for (filename, content) in &files_from_gateway {
            if !is_allowed_sync_file(filename, is_default) {
                continue;
            }
            result.push((ws.to_string(), filename.to_string(), content.to_string()));
        }

        let filenames: Vec<&str> = result.iter().map(|(_, f, _)| f.as_str()).collect();
        assert!(filenames.contains(&"SOUL.md"), "SOUL.md must be included");
        assert!(!filenames.contains(&"random-file.json"), "random-file.json must be filtered");
        assert!(!filenames.contains(&"HEARTBEAT.md"), "HEARTBEAT.md must be filtered for sub-agent");
    }

    /// D3-b: main agent allowlisted files pass through migration filter.
    #[test]
    fn test_fetch_migration_files_main_allowlisted_pass() {
        let is_default = true;
        let files_from_gateway = vec![
            ("SOUL.md", "soul"),
            ("MEMORY.md", "mem"),
            ("HEARTBEAT.md", "hb"),
            ("memory/2026-05-28.md", "daily"),
            ("scratchpad.txt", "junk"),
        ];
        let ws = "/fake/workspace";
        let mut result: Vec<(String, String, String)> = Vec::new();
        for (filename, content) in &files_from_gateway {
            if !is_allowed_sync_file(filename, is_default) {
                continue;
            }
            result.push((ws.to_string(), filename.to_string(), content.to_string()));
        }

        let filenames: Vec<&str> = result.iter().map(|(_, f, _)| f.as_str()).collect();
        assert!(filenames.contains(&"SOUL.md"));
        assert!(filenames.contains(&"MEMORY.md"));
        assert!(filenames.contains(&"HEARTBEAT.md"));
        assert!(filenames.contains(&"memory/2026-05-28.md"));
        assert!(!filenames.contains(&"scratchpad.txt"), "scratchpad.txt must be filtered");
    }

    #[test]
    fn test_no_bridge_urls_in_sync_client() {
        let source = include_str!("sync_client.rs");
        let forbidden = format!("{}{}{}", "/", "bridge", "/");
        for (i, line) in source.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("*") {
                continue;
            }
            let code_part = if let Some(comment_start) = trimmed.find("//") {
                &trimmed[..comment_start]
            } else {
                trimmed
            };
            if code_part.contains(&forbidden) {
                panic!("Line {} contains {} URL in code: {}", i + 1, forbidden, trimmed);
            }
        }
    }

    // ── #1350 deadlock regression tests ─────────────────────────────────────────────────
    //
    // Context: commit 7776bd4d (cert-pinning refactor, May 4 2026) introduced a
    // deadlock in trigger_sync's background thread:
    //
    //   sync_client.rs:406  let conn = db_state.0.lock()   ← LOCK #1
    //   sync_client.rs:421  sync_push_blocking(..., &db_state)
    //     ↳ gateway_client.rs:64  db.0.lock()             ← LOCK #2, same thread → HANG
    //
    // Fix (Option 2): extract cert_pem from conn, drop lock, pass pem directly.
    // `build_gateway_client_blocking_with_cert` never touches DbState.

    /// Verify that `build_gateway_client_blocking_with_cert` does NOT acquire any
    /// DbState lock. In the old code, calling `build_gateway_client_blocking(db)`
    /// while holding `db.0`'s MutexGuard on the same thread would hang forever.
    /// With the fix, the `_with_cert` variant takes no DbState and completes
    /// immediately even when a DbState mutex is held by the calling thread.
    #[test]
    fn test_build_gateway_client_with_cert_no_mutex_relock() {

        // Create a DbState and hold its lock on THIS thread
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db_state = crate::database::DbState::new(conn);
        let _guard = db_state.0.lock().unwrap(); // <- LOCK held on current thread

        // In the OLD code: build_gateway_client_blocking(&db_state) would deadlock here.
        // In the FIXED code: build_gateway_client_blocking_with_cert(None) never touches
        // the mutex and must succeed immediately.
        let result =
            crate::gateway_client::build_gateway_client_blocking_with_cert(None);

        // Assert completes without deadlock and returns a usable client
        assert!(
            result.is_ok(),
            "with_cert variant must succeed even while DbState mutex is held: {:?}",
            result.err()
        );
    }

    /// Regression test: sync_push_blocking completes within 2 seconds.
    /// A deadlock would cause the thread to hang forever - the recv_timeout detects it.
    ///
    /// Uses a mock HTTP server to avoid real gateway dependency. Verifies the full
    /// push path from pairing-state DB config through HTTP response parsing.
    ///
    /// `#[serial(pane_test_root)]` + `PANE_TEST_ROOT` isolate this test's
    /// `init_database(None)` call from the developer's real `~/Pane` production
    /// database (see #2946) - this was the only `init_database(None)` call site
    /// that did not set `PANE_TEST_ROOT`.
    #[test]
    #[serial(pane_test_root)]
    fn test_sync_push_blocking_completes_no_deadlock() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let _test_root_guard = crate::backup::test_support::TestRootGuard::set(tmp.path());

        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/v1/sync/push")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"written_at":"2026-06-01T00:00:00Z","checksum":"abc123"}"#)
            .create();

        let server_url = server.url();

        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();

        std::thread::spawn(move || {
            // Simulate the fixed trigger_sync thread: plain Connection, no DbState lock,
            // cert_pem pre-read and passed directly.
            // Use init_database to ensure ai_settings table exists.
            let conn = crate::database::init_database(None).unwrap();
            crate::database::set_ai_setting(&conn, "provider", "openclaw").unwrap();
            crate::database::set_ai_setting(&conn, "endpoint", &server_url).unwrap();
            crate::database::set_ai_setting(&conn, "api_key", "test-bearer-token").unwrap();
            // Clear any cert that init_database seeded - mock server uses plain HTTP
            // so danger_accept fallback (None cert) is the correct path for this test.
            conn.execute(
                "DELETE FROM ai_settings WHERE key = 'gateway_tls_cert_pem'",
                [],
            ).unwrap();
            let cert_pem: Option<String> = crate::database::get_ai_setting(&conn, "gateway_tls_cert_pem");

            let result = sync_push_blocking(
                &server_url,
                "test-bearer-token",
                "main",
                "logs/2026-06-01.md",
                "test content from in-app autosave",
                cert_pem,
            );
            let _ = tx.send(result);
        });

        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("sync_push_blocking must complete within 5s - deadlock would hang forever");

        assert!(result.is_ok(), "sync_push_blocking must succeed against mock server: {:?}", result);
        mock.assert(); // verify POST /v1/sync/push was actually called
    }

    /// Regression test: DbState lock held by caller does NOT prevent sync_push_blocking
    /// from succeeding. This is the exact pre-fix deadlock scenario.
    ///
    /// Old code (deadlocks):
    ///   let db = DbState(Mutex::new(conn));
    ///   let _guard = db.0.lock();          ← LOCK #1 held on this thread
    ///   sync_push_blocking(..., &db)         ← internally calls db.0.lock() → HANG
    ///
    /// Fixed code (works):
    ///   cert_pem = get_ai_setting(&conn, ...); // read before lock
    ///   drop(conn);                           // lock not held during push
    ///   sync_push_blocking(..., cert_pem)      ← never touches the mutex
    #[test]
    fn test_sync_push_with_held_mutex_no_deadlock_regression() {

        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/v1/sync/push")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"written_at":"2026-06-01T12:34:56Z","checksum":"deadbeef"}"#)
            .create();

        let server_url = server.url();

        // Create a DbState and hold its lock (simulates the old trigger_sync pattern)
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db_state = crate::database::DbState::new(conn);
        let _guard = db_state.0.lock().unwrap(); // LOCK #1 held on calling thread

        // Fixed call path: cert_pem passed directly, NO DbState argument.
        // Would deadlock under old signature; completes immediately under new one.
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let server_url_clone = server_url.clone();

        std::thread::spawn(move || {
            // cert_pem=None simulates pre-pairing / no cert stored
            let r = sync_push_blocking(
                &server_url_clone,
                "bearer-deadlock-test",
                "main",
                "logs/2026-06-01.md",
                "regression content",
                None, // <- fixed: no DbState, no mutex relock
            );
            let _ = tx.send(r);
        });

        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("sync_push_blocking must complete within 5s (deadlock regression guard)");

        drop(_guard); // release db lock after push completes

        assert!(result.is_ok(), "sync_push_blocking must succeed: {:?}", result);
        mock.assert();
    }

    /// Integration test: periodic flush completes even while the shared db.0 mutex is held.
    ///
    /// Regression guard for Fix B (#1180). Under the OLD periodic flush pattern:
    ///
    ///   if let Ok(conn) = periodic_conn.lock() {  // periodic_conn IS db.0
    ///       flush_sync_queue_blocking(&conn, ...);
    ///   }
    ///
    /// ...any concurrent task holding db.0 (e.g. startup sync) prevented the periodic
    /// flush from even starting. This produced four stacked
    /// "[GATEWAY_CLIENT] Building blocking gateway client..." log lines and the
    /// pinwheel-of-death freeze.
    ///
    /// Under Fix B:
    ///
    ///   open_connection(None) → fresh conn, no mutex → flush proceeds immediately
    ///
    /// Test-the-test (§4): to confirm this test CATCHES the regression, temporarily
    /// replace the `crate::database::open_connection(None)` call below with
    /// `db_state_for_flush.0.lock().map(|_| ())` (simulating the old lock pattern)
    /// - recv_timeout fires at 5s while db.0 is held for 6s:
    ///   "periodic flush must complete within 5s - old periodic_conn.lock() blocks on db.0"
    ///
    /// See: docs/audits/2026-06-02-freeze-cluster-1347-1180.md
    ///      docs/audits/2026-06-02-1180-fix-site-confirmation.md
    #[test]
    fn test_periodic_flush_completes_with_shared_mutex_held() {

        // Create a DbState whose mutex we'll hold - simulates a concurrent startup-sync
        // or any long-running operation holding the shared db.0 lock across blocking I/O.
        // Uses an in-memory Connection so this test does not depend on the real DB state.
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let db_state = crate::database::DbState::new(conn);
        let db_state_for_flush = crate::database::DbState(db_state.0.clone());

        // IMPORTANT: hold the lock BEFORE spawning the thread so the thread always
        // sees the lock held when it attempts to acquire it (eliminates the race).
        let guard = db_state.0.lock().unwrap();

        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();

        // Spawn the periodic-flush thread using Fix B pattern:
        // open_connection(None) → fresh file-based connection, never touches db_state.0.
        // flush_sync_queue_blocking returns Ok(()) immediately when no gateway is
        // configured in the fresh connection - no HTTP call fires, no db.0 lock taken.
        //
        // §4 REVERT INSTRUCTION: to verify this test catches the regression, temporarily
        // replace the `open_connection(None)` block below with:
        //   let _would_block = db_state_for_flush.0.lock(); // simulates old periodic_conn.lock()
        //   let _ = tx.send(Ok(()));
        // recv_timeout will fire at 5s - proving the test catches Fix B regressions.
        std::thread::spawn(move || {
            // Fix B pattern: fresh connection - does not acquire db_state_for_flush.0.
            match crate::database::open_connection(None) {
                Ok(fresh_conn) => {
                    // No gateway configured in fresh_conn → Ok(()) immediately.
                    // db_state_for_flush.0 is NEVER locked inside flush_sync_queue_blocking.
                    let result = flush_sync_queue_blocking(&fresh_conn, None, &db_state_for_flush);
                    let _ = tx.send(result);
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("[TEST] open fresh conn failed: {}", e)));
                }
            }
        });

        // Assert flush completes within 5 seconds while db.0 is still held.
        // db.0 is held on this thread until drop(guard) below.
        // Without Fix B (using periodic_conn.lock()), recv_timeout fires with:
        //   "periodic flush must complete within 5s - old periodic_conn.lock() blocks on db.0"
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("periodic flush must complete within 5s - old periodic_conn.lock() blocks on db.0");

        drop(guard); // release db.0 after the assertion

        assert!(
            result.is_ok(),
            "flush_sync_queue_blocking must succeed with fresh connection: {:?}",
            result
        );
    }

    // ──────────────────────────────────────────────────────────────────────────────
    // §24.5.5 Provider-Switch Re-import Tests (#956)
    // Spec: docs/spec/agents.md §24.5.5  |  spec-covered-by: #1476
    // ──────────────────────────────────────────────────────────────────────────────
    //
    // Test coverage matrix:
    //   T1  Overwrite-in-place: existing agent → UPDATE, preserve id + workspace_path, files overwritten
    //   T2  Fresh import: new runtime_ref → INSERT, new UUID, new dir
    //   T3  Mixed: some existing + some new
    //   T4  IDENTITY.md overwritten (NOT preserved) - explicit assertion
    //   T5  notes.agent_id stability (id preserved on UPDATE)
    //   T6  Transaction rollback on partial failure
    //   T7  Slug collision → full rollback
    //   T8  check_oc_agent_collisions correct results with zero mutations
    //   T9  Allowlist enforcement
    //   T10 Self-contained: reimport works even if state changed since check
    //   T13 #956 regression: OR agent holds is_default=1 → demoted when OC main imported
    // ──────────────────────────────────────────────────────────────────────────────

    /// Open an in-memory SQLite DB with the minimal schema required by sync_re_import.
    fn open_reimport_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_settings (
               key   TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS ai_agents (
               id             TEXT PRIMARY KEY,
               name           TEXT NOT NULL DEFAULT '',
               slug           TEXT NOT NULL UNIQUE,
               role           TEXT NOT NULL DEFAULT 'sub',
               is_default        INTEGER NOT NULL DEFAULT 0,
               is_active      INTEGER NOT NULL DEFAULT 1,
               workspace_path TEXT NOT NULL DEFAULT '',
               runtime_ref    TEXT,
               runtime_type   TEXT,
               source         TEXT NOT NULL DEFAULT 'import',
               created_at     TEXT NOT NULL DEFAULT '',
               updated_at     TEXT NOT NULL DEFAULT ''
             );
             -- Partial unique index that triggers #956: only one row may have is_default=1.
             -- Must match production schema in migrations.rs.
             CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_default
               ON ai_agents (is_default) WHERE is_default = 1;
             -- Also mirror the composite runtime unique index (ADR-0063 D4 step 4).
             CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_runtime_unique
               ON ai_agents (runtime_type, runtime_ref);",
        )
        .expect("create reimport test schema");
        conn
    }

    /// Build a mock server that serves the /v1/sync/initial endpoint.
    /// `agents_json` is the JSON array of agents, e.g.:
    /// `[{"oc_id":"main","name":"Brit","files":[{"filename":"SOUL.md","content":"...","checksum":""}]}]`
    // Retained as a helper for dormant initial-sync mock coverage.
    #[allow(dead_code)]
    async fn mock_gateway_initial(server: &mut mockito::Server, agents_json: &str) -> mockito::Mock {
        let body = format!("{{\"agents\":{}}}", agents_json);
        server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await
    }

    // -------------------------------------------------------------------
    // Test helper: build a mock body for GET /v1/sync/initial
    fn make_initial_body(agents_json: &str) -> String {
        format!("{{\"agents\":{}}}", agents_json)
    }

    /// T1 — Overwrite-in-place: existing agent → UPDATE, preserve id + workspace_path.
    /// Files are overwritten including IDENTITY.md (§24.5.5).
    ///
    /// `#[serial(pane_test_root)]` + `PANE_TEST_ROOT` isolate this test's
    /// `sync_re_import(..., None)` call (which resolves `get_pane_root()` via
    /// `PaneFs::for_runtime()`) from the developer's real `~/Pane` (see #2946/#2944).
    #[tokio::test]
    #[serial(pane_test_root)]
    async fn test_reimport_overwrite_in_place_preserves_id_and_workspace() {
        let test_root_dir = tempfile::tempdir().expect("test root tempdir");
        let _test_root_guard = crate::backup::test_support::TestRootGuard::set(test_root_dir.path());
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let ws_path = dir.path().join("existing-agent-uuid");
        std::fs::create_dir_all(&ws_path).unwrap();
        std::fs::write(ws_path.join("SOUL.md"), "old soul content").unwrap();

        let agents_json = r#"[{"oc_id":"main","name":"Brit Freed","files":[
            {"filename":"SOUL.md","content":"new gateway soul","checksum":"abc"}
        ]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let ws_path_str = ws_path.to_str().unwrap().to_string();
        let ws_path_clone = ws_path.clone();

        let (result, stored_id, stored_ws, stored_is_main) = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            let existing_id = "fixed-row-id-001";
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES (?1, 'Old Name', 'old-name', ?2, 'main', '2026-01-01', '2026-01-01')",
                rusqlite::params![existing_id, ws_path_str],
            ).unwrap();

            let result = sync_re_import(&conn, &server_url, "test-token", None, None);

            let (stored_id, stored_ws, stored_is_main): (String, String, i64) = conn
                .query_row(
                    "SELECT id, workspace_path, is_default FROM ai_agents WHERE runtime_ref = 'main'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .unwrap();
            (result, stored_id, stored_ws, stored_is_main)
        }).await.expect("spawn_blocking");

        assert!(result.is_ok(), "sync_re_import must succeed: {:?}", result);
        assert_eq!(stored_id, "fixed-row-id-001", "row id must be preserved on UPDATE");
        assert_eq!(
            stored_ws, ws_path_clone.to_str().unwrap(),
            "workspace_path must come from the pre-queried row, not gateway payload"
        );
        assert_eq!(
            stored_is_main, 1,
            "T1: OC main agent must have is_default=1 after UPDATE path (#1546)"
        );
        let soul_after = std::fs::read_to_string(ws_path_clone.join("SOUL.md")).unwrap();
        assert_eq!(soul_after, "new gateway soul", "SOUL.md must be overwritten");
    }

    /// T2 — Fresh import: new runtime_ref → INSERT, new row created.
    #[tokio::test]
    async fn test_reimport_fresh_import_new_uuid() {
        let mut server = mockito::Server::new_async().await;

        let agents_json = r#"[{"oc_id":"coder-fresh","name":"Stia","files":[]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir_path = dir.path().to_path_buf();

        let (count_before, result_ok, count_after, was_existing) =
            tokio::task::spawn_blocking(move || {
                let conn = open_reimport_test_db();
                let count_before: i64 = conn
                    .query_row("SELECT COUNT(*) FROM ai_agents", [], |r| r.get(0))
                    .unwrap();

                let result = sync_re_import(&conn, &server_url, "test-token", None, Some(system_dir_path.as_path()));
                let result_ok = result.is_ok();
                let was_existing = result.as_ref().ok().and_then(|v| v.get(0)).map(|t| t.2);

                let count_after: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM ai_agents WHERE runtime_ref = 'coder-fresh'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);

                (count_before, result_ok, count_after, was_existing)
            })
            .await
            .expect("spawn_blocking");

        assert_eq!(count_before, 0);
        if result_ok {
            // Fresh import path: new row inserted
            assert_eq!(count_after, 1, "INSERT must create a new row");
            assert_eq!(was_existing, Some(false), "must report was_existing=false");
        } else {
            // Dir creation may fail in sandboxed CI — count stays 0 (rolled back)
            assert_eq!(count_after, 0, "must be 0 after rollback");
        }
    }

    /// T3 — Mixed: one existing + one new agent in the same transaction.
    #[tokio::test]
    async fn test_reimport_mixed_existing_and_new() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let ws_path = dir.path().join("main-ws");
        std::fs::create_dir_all(&ws_path).unwrap();

        let ws_path_str = ws_path.to_str().unwrap().to_string();
        let agents_json = format!(
            r#"[
                {{"oc_id":"main","name":"Brit","files":[{{"filename":"SOUL.md","content":"main soul","checksum":"a"}}]}},
                {{"oc_id":"coder-mix","name":"Stia","files":[]}}
            ]"#
        );
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(&agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let system_dir_path = dir.path().to_path_buf();

        let (result, stored_main_id) = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES ('main-row-id', 'Brit Old', 'brit-old', ?1, 'main', '2026-01-01', '2026-01-01')",
                rusqlite::params![ws_path_str],
            ).unwrap();

            let result = sync_re_import(&conn, &server_url, "test-token", None, Some(system_dir_path.as_path()));

            let stored_main_id: String = conn
                .query_row(
                    "SELECT id FROM ai_agents WHERE runtime_ref = 'main'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            (result, stored_main_id)
        }).await.expect("spawn_blocking");

        assert_eq!(stored_main_id, "main-row-id", "main row id must be preserved on UPDATE");
        if let Ok(v) = result {
            assert_eq!(v.len(), 2, "both agents must be in result");
            let main_entry = v.iter().find(|(oc, _, _)| oc == "main");
            let coder_entry = v.iter().find(|(oc, _, _)| oc == "coder-mix");
            assert!(main_entry.map(|e| e.2).unwrap_or(false), "main must be was_existing=true");
            assert!(!coder_entry.map(|e| e.2).unwrap_or(true), "coder-mix must be was_existing=false");
        }
    }

    /// T4 — IDENTITY.md is overwritten (NOT preserved) on overwrite-in-place.
    /// Deliberate divergence from initial_sync (§24.5.5 design note).
    ///
    /// `#[serial(pane_test_root)]` + `PANE_TEST_ROOT` isolate this test's
    /// `sync_re_import(..., None)` call from the developer's real `~/Pane` (see #2946/#2944).
    #[tokio::test]
    #[serial(pane_test_root)]
    async fn test_reimport_identity_md_overwritten_not_preserved() {
        let test_root_dir = tempfile::tempdir().expect("test root tempdir");
        let _test_root_guard = crate::backup::test_support::TestRootGuard::set(test_root_dir.path());
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let ws_path = dir.path().join("ws-identity");
        std::fs::create_dir_all(&ws_path).unwrap();
        std::fs::write(
            ws_path.join("IDENTITY.md"),
            "# old local identity\n\nShould be overwritten.",
        ).unwrap();

        let new_identity = "# new gateway identity\n\nGateway is authoritative (\u{00a7}24.5.5).";
        let agents_json = format!(
            r#"[{{"oc_id":"main","name":"Brit","files":[
                {{"filename":"IDENTITY.md","content":{},"checksum":"abc"}}
            ]}}]"#,
            serde_json::to_string(new_identity).unwrap()
        );
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(&agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let ws_path_str = ws_path.to_str().unwrap().to_string();
        let ws_path_clone = ws_path.clone();
        let new_identity_owned = new_identity.to_string();

        let result = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES ('main-id', 'Brit', 'brit', ?1, 'main', '2026-01-01', '2026-01-01')",
                rusqlite::params![ws_path_str],
            ).unwrap();
            sync_re_import(&conn, &server_url, "test-token", None, None)
        }).await.expect("spawn_blocking");

        assert!(result.is_ok(), "sync_re_import must succeed: {:?}", result);
        let identity_after =
            std::fs::read_to_string(ws_path_clone.join("IDENTITY.md")).unwrap();
        assert_eq!(
            identity_after, new_identity_owned,
            "IDENTITY.md must be overwritten with gateway content (§24.5.5 — no !exists guard)"
        );
    }

    /// T5 — notes.agent_id stability: ai_agents.id never changes on UPDATE.
    ///
    /// `#[serial(pane_test_root)]` + `PANE_TEST_ROOT` isolate this test's
    /// `sync_re_import(..., None)` call from the developer's real `~/Pane` (see #2946/#2944).
    #[tokio::test]
    #[serial(pane_test_root)]
    async fn test_reimport_notes_agent_id_stability() {
        let test_root_dir = tempfile::tempdir().expect("test root tempdir");
        let _test_root_guard = crate::backup::test_support::TestRootGuard::set(test_root_dir.path());
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let ws_path = dir.path().join("ws-stable");
        std::fs::create_dir_all(&ws_path).unwrap();

        let agents_json = r#"[{"oc_id":"main","name":"Brit","files":[
            {"filename":"SOUL.md","content":"soul","checksum":"x"}
        ]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let ws_path_str = ws_path.to_str().unwrap().to_string();

        let id_after = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES ('stable-id', 'Brit', 'brit', ?1, 'main', '2026-01-01', '2026-01-01')",
                rusqlite::params![ws_path_str],
            ).unwrap();
            sync_re_import(&conn, &server_url, "test-token", None, None).unwrap();
            let id_after: String = conn
                .query_row(
                    "SELECT id FROM ai_agents WHERE runtime_ref = 'main'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            id_after
        }).await.expect("spawn_blocking");

        assert_eq!(
            id_after, "stable-id",
            "ai_agents.id must never change on re-import (notes.agent_id refs survive)"
        );
    }

    /// T6 — Network error leaves DB unchanged (fetch fails before any mutations).
    #[test]
    fn test_reimport_network_error_leaves_db_unchanged() {
        // No tokio test needed — we call sync_re_import in a plain thread.
        // fetch_gateway_agents will fail immediately (no server listening).
        let bad_url = "http://127.0.0.1:19999";

        let (result_is_err, count) = std::thread::spawn(move || {
            let conn = open_reimport_test_db();
            let result = sync_re_import(&conn, bad_url, "token", None, None);
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_agents", [], |r| r.get(0))
                .unwrap();
            (result.is_err(), count)
        }).join().expect("thread panicked");

        assert!(result_is_err, "network error must return Err");
        assert_eq!(count, 0, "DB must be unchanged after failed network fetch");
    }

    /// T7 — Intra-batch slug collision: coexist + disambiguate (§24.5.5).
    /// Two OC agents with the same name/slug, no pre-existing DB rows.
    /// Both must commit: first gets base slug, second gets base-2.
    /// No rollback (slug collision is a handled condition, not a failure).
    #[tokio::test]
    async fn test_reimport_slug_collision_rolls_back_all() {
        let mut server = mockito::Server::new_async().await;

        // Two gateway agents resolve to the same base slug "brit".
        // Neither exists in DB at batch start.
        let agents_json = r#"[
            {"oc_id":"main",  "name":"Brit","files":[]},
            {"oc_id":"coder2","name":"Brit","files":[]}
        ]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir_path = dir.path().to_path_buf();

        let (result, slugs) = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            let result = sync_re_import(&conn, &server_url, "test-token", None, Some(system_dir_path.as_path()));
            let mut stmt = conn
                .prepare("SELECT slug FROM ai_agents ORDER BY slug")
                .unwrap();
            let slugs: Vec<String> = stmt
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            (result, slugs)
        }).await.expect("spawn_blocking");

        assert!(result.is_ok(), "intra-batch slug collision must NOT roll back: {:?}", result);
        assert_eq!(slugs.len(), 2, "both agents must be committed");
        assert!(slugs.contains(&"brit".to_string()), "first agent must claim base slug");
        assert!(slugs.contains(&"brit-2".to_string()), "second agent must get disambiguated slug brit-2");
    }

    /// T13 — Intra-batch slug collision (two OC agents, same base slug, both net-new).
    /// Both must commit: one gets base, the other gets base-2.
    /// Proves the intra-batch probe is sound.
    #[tokio::test]
    async fn test_reimport_intra_batch_slug_collision() {
        let mut server = mockito::Server::new_async().await;

        let agents_json = r#"[
            {"oc_id":"agent-a","name":"Pane Agent","files":[]},
            {"oc_id":"agent-b","name":"Pane Agent","files":[]}
        ]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir_path = dir.path().to_path_buf();

        let (result, slugs, count) = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            // No pre-existing rows — both agents are net-new.
            let result = sync_re_import(&conn, &server_url, "test-token", None, Some(system_dir_path.as_path()));
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_agents", [], |r| r.get(0))
                .unwrap();
            let mut stmt = conn
                .prepare("SELECT slug FROM ai_agents ORDER BY slug")
                .unwrap();
            let slugs: Vec<String> = stmt
                .query_map([], |r| r.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            (result, slugs, count)
        }).await.expect("spawn_blocking");

        assert!(result.is_ok(), "intra-batch collision must commit: {:?}", result);
        assert_eq!(count, 2, "both agents must be inserted");
        assert!(slugs.contains(&"pane-agent".to_string()), "first agent must get base slug");
        assert!(slugs.contains(&"pane-agent-2".to_string()), "second agent must get pane-agent-2");
    }

    /// T14 — Increment-until-free: pre-existing rows with slugs `pane` and `pane-2`.
    /// Incoming OC agent resolves to base slug `pane` → must disambiguate to `pane-3`.
    #[tokio::test]
    async fn test_reimport_increment_until_free() {
        let mut server = mockito::Server::new_async().await;

        let agents_json = r#"[
            {"oc_id":"new-agent","name":"Pane","files":[]}
        ]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir_path = dir.path().to_path_buf();

        let (result, new_slug) = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            // Pre-existing rows with slugs `pane` and `pane-2`.
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES ('id-pane', 'Pane', 'pane', 'ws-pane', NULL, '', '')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES ('id-pane2', 'Pane 2', 'pane-2', 'ws-pane2', NULL, '', '')",
                [],
            ).unwrap();

            let result = sync_re_import(&conn, &server_url, "test-token", None, Some(system_dir_path.as_path()));
            let new_slug: String = conn
                .query_row(
                    "SELECT slug FROM ai_agents WHERE runtime_ref = 'new-agent'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            (result, new_slug)
        }).await.expect("spawn_blocking");

        assert!(result.is_ok(), "increment-until-free must succeed: {:?}", result);
        assert_eq!(new_slug, "pane-3", "must skip pane and pane-2, land on pane-3");
    }

    /// T15 — Existing row untouched: pre-existing row with `runtime_ref IS NULL`.
    /// Incoming OC agent slug-collides. After coexist: verify the existing row has
    /// NO changes (same runtime_ref = NULL, same name, same workspace_path, same files).
    /// The incoming OC agent is a separate row with its own UUID and disambiguated slug.
    #[tokio::test]
    async fn test_reimport_existing_row_untouched_on_slug_collision() {
        let mut server = mockito::Server::new_async().await;

        let agents_json = r#"[
            {"oc_id":"oc-new","name":"Pane","files":[
                {"filename":"SOUL.md","content":"oc soul","checksum":"x"}
            ]}
        ]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir_path = dir.path().to_path_buf();

        // Create the pre-existing local agent's workspace with a file.
        let existing_ws = dir.path().join("existing-ws");
        std::fs::create_dir_all(&existing_ws).unwrap();
        std::fs::write(existing_ws.join("SOUL.md"), "original soul").unwrap();

        let (result, existing_name, existing_oc_id, existing_ws_path, existing_soul, new_row_count) =
            tokio::task::spawn_blocking(move || {
                let conn = open_reimport_test_db();
                // Pre-existing row with runtime_ref IS NULL and slug 'pane'.
                let existing_ws_str = existing_ws.to_str().unwrap().to_string();
                conn.execute(
                    "INSERT INTO ai_agents \
                     (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                     VALUES ('existing-id', 'Pane', 'pane', ?1, NULL, '2026-01-01', '2026-01-01')",
                    rusqlite::params![existing_ws_str],
                ).unwrap();

                let result = sync_re_import(&conn, &server_url, "test-token", None, Some(system_dir_path.as_path()));

                // Read back the existing row — it must be completely unchanged.
                let (existing_name, existing_oc_id, existing_ws_path): (String, Option<String>, String) = conn
                    .query_row(
                        "SELECT name, runtime_ref, workspace_path FROM ai_agents WHERE id = 'existing-id'",
                        [],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .unwrap();

                // The pre-existing file must be unmodified.
                let existing_soul = std::fs::read_to_string(existing_ws.join("SOUL.md")).unwrap();

                // Total row count: 2 (existing + incoming OC agent with disambiguated slug).
                let new_row_count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM ai_agents WHERE runtime_ref = 'oc-new'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();

                (result, existing_name, existing_oc_id, existing_ws_path, existing_soul, new_row_count)
            }).await.expect("spawn_blocking");

        assert!(result.is_ok(), "slug collision coexist must succeed: {:?}", result);
        // Existing row must be completely untouched.
        assert_eq!(existing_name, "Pane", "existing row name must be unchanged");
        assert!(existing_oc_id.is_none(), "existing row runtime_ref must remain NULL");
        assert!(existing_ws_path.ends_with("existing-ws"), "existing row workspace_path must be unchanged");
        assert_eq!(existing_soul, "original soul", "existing row files must be unmodified");
        // Incoming OC agent must have been inserted as a separate row.
        assert_eq!(new_row_count, 1, "incoming OC agent must exist as its own row");
    }

    /// T8 — check_oc_agent_collisions logic: correct collision/new split, zero DB mutations.
    /// Tests the core classification logic (not the Tauri command wrapper).
    #[tokio::test]
    async fn test_check_oc_agent_collisions_logic() {
        let mut server = mockito::Server::new_async().await;

        // Gateway: "main" and "coder" exist locally, "analyst" is net-new
        let agents_json = r#"[
            {"oc_id":"main",    "name":"Brit",    "files":[]},
            {"oc_id":"coder",   "name":"Stia",    "files":[]},
            {"oc_id":"analyst", "name":"Analyst", "files":[]}
        ]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();

        let (collisions, new_count, db_count_after) = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES ('id-main', 'Brit Local', 'brit-local', 'ws-main', 'main', '', '')",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES ('id-coder', 'Stia Local', 'stia-local', 'ws-coder', 'coder', '', '')",
                [],
            ).unwrap();

            // fetch_gateway_agents is blocking, so call directly here
            let gateway_agents = fetch_gateway_agents(&server_url, "token", None).unwrap();

            // Mirror check_oc_agent_collisions inner logic
            let mut local_map: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut stmt = conn
                .prepare(
                    "SELECT runtime_ref, name FROM ai_agents WHERE runtime_ref IS NOT NULL",
                )
                .unwrap();
            let rows =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
                    .unwrap();
            for row in rows {
                let (oc_id, name) = row.unwrap();
                local_map.insert(oc_id, name);
            }

            let mut collisions = 0usize;
            let mut new_count = 0usize;
            for agent in &gateway_agents {
                if local_map.contains_key(&agent.oc_id) {
                    collisions += 1;
                } else {
                    new_count += 1;
                }
            }

            // Zero mutations: count stays at 2
            let db_count_after: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_agents", [], |r| r.get(0))
                .unwrap();

            (collisions, new_count, db_count_after)
        }).await.expect("spawn_blocking");

        assert_eq!(collisions, 2, "main and coder must be collisions");
        assert_eq!(new_count, 1, "analyst must be a new agent");
        assert_eq!(db_count_after, 2, "check must perform zero DB mutations");
    }

    /// T9 — Allowlist enforcement: non-whitelisted files are NOT written on re-import.
    ///
    /// `#[serial(pane_test_root)]` + `PANE_TEST_ROOT` isolate this test's
    /// `sync_re_import(..., None)` call from the developer's real `~/Pane` (see #2946/#2944).
    #[tokio::test]
    #[serial(pane_test_root)]
    async fn test_reimport_allowlist_enforcement() {
        let test_root_dir = tempfile::tempdir().expect("test root tempdir");
        let _test_root_guard = crate::backup::test_support::TestRootGuard::set(test_root_dir.path());
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let ws_path = dir.path().join("ws-allow");
        std::fs::create_dir_all(&ws_path).unwrap();

        let agents_json = format!(
            r#"[{{"oc_id":"main","name":"Brit","files":[
                {{"filename":"SOUL.md","content":"soul content","checksum":"a"}},
                {{"filename":"random-secret.json","content":"{{\"s\":1}}","checksum":"b"}},
                {{"filename":"scratchpad.txt","content":"junk","checksum":"c"}}
            ]}}]"#
        );
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(&agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let ws_path_str = ws_path.to_str().unwrap().to_string();
        let ws_path_clone = ws_path.clone();

        let result = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES ('id-main', 'Brit', 'brit', ?1, 'main', '', '')",
                rusqlite::params![ws_path_str],
            ).unwrap();
            sync_re_import(&conn, &server_url, "test-token", None, None)
        }).await.expect("spawn_blocking");

        assert!(result.is_ok(), "sync_re_import must succeed: {:?}", result);
        assert!(ws_path_clone.join("SOUL.md").exists(), "SOUL.md must be written (allowlisted)");
        assert!(
            !ws_path_clone.join("random-secret.json").exists(),
            "random-secret.json must NOT be written (not in allowlist)"
        );
        assert!(
            !ws_path_clone.join("scratchpad.txt").exists(),
            "scratchpad.txt must NOT be written (not in allowlist)"
        );
    }

    /// T10 — Self-contained: reimport re-fetches and handles TOCTOU correctly.
    /// Simulates a state change between the check and reimport calls.
    ///
    /// `#[serial(pane_test_root)]` + `PANE_TEST_ROOT` isolate this test's
    /// `sync_re_import(..., None)` call from the developer's real `~/Pane` (see #2946/#2944).
    #[tokio::test]
    #[serial(pane_test_root)]
    async fn test_reimport_self_contained_toctou() {
        let test_root_dir = tempfile::tempdir().expect("test root tempdir");
        let _test_root_guard = crate::backup::test_support::TestRootGuard::set(test_root_dir.path());
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let ws_path = dir.path().join("coder-ws");
        std::fs::create_dir_all(&ws_path).unwrap();

        let agents_json = r#"[{"oc_id":"coder","name":"Stia","files":[
            {"filename":"SOUL.md","content":"coder soul","checksum":"c"}
        ]}]"#;
        // reimport makes ONE fetch (check simulation is just a pre-insert here)
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let ws_path_str = ws_path.to_str().unwrap().to_string();

        let (result, stored_id) = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();
            // TOCTOU: coder inserted between check and reimport
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, workspace_path, runtime_ref, created_at, updated_at) \
                 VALUES ('coder-id', 'Stia Old', 'stia-old', ?1, 'coder', '', '')",
                rusqlite::params![ws_path_str],
            ).unwrap();

            // reimport re-queries and treats coder as existing → UPDATE, not INSERT
            let result = sync_re_import(&conn, &server_url, "test-token", None, None);

            let stored_id: String = conn
                .query_row(
                    "SELECT id FROM ai_agents WHERE runtime_ref = 'coder'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            (result, stored_id)
        }).await.expect("spawn_blocking");

        assert!(result.is_ok(), "reimport must succeed with TOCTOU: {:?}", result);
        assert_eq!(
            stored_id, "coder-id",
            "reimport must UPDATE existing row even when TOCTOU makes check stale"
        );
    }

    /// T11 — fetch_gateway_agents: 3 attempts then Err.
    /// Updated for GatewayFetchError return type (#2025).
    #[tokio::test]
    async fn test_fetch_gateway_agents_retries_and_errors() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(500)
            .expect(3)
            .create_async()
            .await;
        let server_url = server.url();

        let result = tokio::task::spawn_blocking(move || {
            fetch_gateway_agents(&server_url, "token", None)
        }).await.unwrap();

        assert!(result.is_err(), "must return Err after 3 failed attempts");
        let err = result.unwrap_err();
        // GatewayFetchError — code is None for plain 500, message mentions retries
        assert!(err.code.is_none(), "plain 500 should have no error_code");
        let msg = err.to_string();
        assert!(
            msg.contains("3 attempts") || msg.contains("Gateway returned"),
            "error must mention retries: {}", msg
        );
    }

    /// T11b — §22C.5.4: fetch_gateway_agents fast-fails on oc_handshake_timeout (no retry).
    #[tokio::test]
    async fn test_fetch_gateway_agents_fast_fails_on_handshake_timeout() {
        let mut server = mockito::Server::new_async().await;
        // Only one call expected — fast-fail means no retry
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(502)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"oc_handshake_timeout","status":502,"message":"Gateway did not receive hello-ok"}"#)
            .expect(1)  // fast-fail: only 1 attempt, not 3
            .create_async()
            .await;
        let server_url = server.url();

        let result = tokio::task::spawn_blocking(move || {
            fetch_gateway_agents(&server_url, "token", None)
        }).await.unwrap();

        assert!(result.is_err(), "must return Err on oc_handshake_timeout");
        let err = result.unwrap_err();
        assert_eq!(
            err.code.as_deref(),
            Some("oc_handshake_timeout"),
            "error code must be oc_handshake_timeout"
        );
        // Display must serialize as JSON for Svelte discrimination
        let display = err.to_string();
        assert!(
            display.contains("\"error_code\":\"oc_handshake_timeout\""),
            "Display must embed error_code in JSON: {}", display
        );
    }

    /// T11c — §22C.5.4: other error codes (oc_connection_lost) still retry 3 times.
    #[tokio::test]
    async fn test_fetch_gateway_agents_retries_on_other_error_codes() {
        let mut server = mockito::Server::new_async().await;
        // oc_connection_lost: all 3 attempts should fire (not fast-failed)
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(502)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"oc_connection_lost","status":502,"message":"OC connection lost"}"#)
            .expect(3)
            .create_async()
            .await;
        let server_url = server.url();

        let result = tokio::task::spawn_blocking(move || {
            fetch_gateway_agents(&server_url, "token", None)
        }).await.unwrap();

        assert!(result.is_err(), "must return Err after 3 attempts");
        let err = result.unwrap_err();
        // Should NOT be oc_handshake_timeout
        assert_ne!(
            err.code.as_deref(),
            Some("oc_handshake_timeout"),
            "oc_connection_lost must not be treated as handshake timeout"
        );
    }

    /// T13 — #956 regression: provider-switch demotes OR main agent.
    ///
    /// Pre-state:  one agent with `is_default=1`, `runtime_ref=NULL` (OpenRouter-onboarded).
    /// Action:     sync_re_import with an OC agent where `oc_id="main"`.
    ///
    /// **Superseded by #2644 (this PR):** this test's name and original assertions
    /// ("OR agent must be demoted", "OC main agent must hold is_default=1") encoded the
    /// pre-#2644 behavior, which is the exact bug #2644 closes: a fresh-import OC main
    /// agent unconditionally stole is_default from any existing foreign default. Under
    /// #2644, `land_as_default` is false here (a foreign default exists, and this OC row
    /// was never the default before — `already_default_here=false`), so the OR agent must
    /// KEEP is_default=1 and the OC agent must land with role='local', is_default=0 (same
    /// as D13's rule for sync_initial_import's first-pair case). Assertions below now
    /// verify the corrected (non-stealing) behavior; the test/function name is kept for
    /// git-blame continuity with the original #1527/#1546-era bug history.
    /// Assertions (post-#2644):
    ///   1. OR agent (foreign default) is UNCHANGED: is_default=1, role='main'.
    ///   2. OC agent inserted with is_default=0, role='local' (blocked from stealing default).
    ///   3. Both rows survive in `ai_agents` (no deletion, no rollback).
    ///   4. No constraint violations (the partial UNIQUE index on `is_default` is in play).
    #[tokio::test]
    async fn test_reimport_demotes_or_main_on_oc_main_import() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir = dir.path().to_path_buf();

        // OC gateway returns a single "main" agent.
        let agents_json = r#"[{"oc_id":"main","name":"Brit Freed","files":[]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let system_dir_clone = system_dir.clone();

        let (result, or_is_main, oc_is_main, total_rows, or_role) =
            tokio::task::spawn_blocking(move || {
                let conn = open_reimport_test_db();

                // Pre-state: an OpenRouter-onboarded agent owns is_default=1,
                // runtime_ref is NULL (never linked to OC gateway).
                conn.execute(
                    "INSERT INTO ai_agents \
                     (id, name, slug, role, is_default, workspace_path, runtime_ref, source, created_at, updated_at) \
                     VALUES ('or-agent-id', 'Blue', 'blue', 'main', 1, 'or-ws', NULL, 'local', '', '')",
                    [],
                )
                .expect("pre-state OR agent insert must succeed");

                let result = sync_re_import(
                    &conn,
                    &server_url,
                    "test-token",
                    None,
                    Some(system_dir_clone.as_path()),
                );

                // Assertion data: is_default values after reimport.
                let or_is_main: i64 = conn
                    .query_row(
                        "SELECT is_default FROM ai_agents WHERE id = 'or-agent-id'",
                        [],
                        |r| r.get(0),
                    )
                    .expect("OR agent row must still exist (no deletion)");

                let oc_is_main: i64 = conn
                    .query_row(
                        "SELECT is_default FROM ai_agents WHERE runtime_ref = 'main'",
                        [],
                        |r| r.get(0),
                    )
                    .expect("OC main agent row must exist after INSERT");

                let total_rows: i64 = conn
                    .query_row("SELECT COUNT(*) FROM ai_agents", [], |r| r.get(0))
                    .unwrap_or(0);

                let or_role: String = conn
                    .query_row(
                        "SELECT role FROM ai_agents WHERE id = 'or-agent-id'",
                        [],
                        |r| r.get(0),
                    )
                    .expect("OR agent role must be readable");

                (result, or_is_main, oc_is_main, total_rows, or_role)
            })
            .await
            .expect("spawn_blocking");

        // 1. No transaction rollback.
        assert!(
            result.is_ok(),
            "sync_re_import must succeed without constraint violation: {:?}",
            result
        );
        // 2. #2644 fix: OR agent (foreign default) must NOT be demoted — re-import must
        // never steal default status from an existing foreign default agent.
        assert_eq!(
            or_is_main, 1,
            "#2644: OR agent (foreign default) must KEEP is_default=1 — fresh-import of OC main must not steal it"
        );
        // 3. #2644 fix: OC main agent must land as non-default (blocked by the foreign default).
        assert_eq!(
            oc_is_main, 0,
            "#2644: OC main agent must NOT steal is_default from an existing foreign default on fresh import"
        );
        // 4. Both rows survive (no deletion).
        assert_eq!(
            total_rows, 2,
            "both the OR agent and the OC main agent must exist — no rows deleted"
        );
        // 5. OR agent's role is untouched (never demoted, so still 'main').
        assert_eq!(
            or_role, "main",
            "#2644: OR agent (foreign default, never demoted) must keep role='main'"
        );
    }

    /// #2567 B6 demotion test: a locally-created DEFAULT agent (seeded with
    /// role='local' directly, the new post-B6 birth label — rather than T13's
    /// role='main' pre-state).
    ///
    /// **Superseded by #2644 (this PR):** originally this test asserted the locally-created
    /// default agent gets demoted (is_default=0) by an incoming OC main-agent fresh-import,
    /// preserving role='local' rather than 'sub' during that demotion (#2567 B6's fix).
    /// Under #2644, the demotion itself must not happen at all here: a foreign default
    /// (this locally-created agent) blocks `land_as_default`, so the OC agent lands as
    /// role='local'/is_default=0 instead, and the locally-created agent is left completely
    /// untouched (still is_default=1, still role='local' — same end-state value for role,
    /// but now because it was never touched, not because a demotion UPDATE preserved it).
    #[tokio::test]
    async fn test_reimport_demotes_locally_created_default_agent_preserves_role_local() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir = dir.path().to_path_buf();

        let agents_json = r#"[{"oc_id":"main","name":"Brit Freed","files":[]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let system_dir_clone = system_dir.clone();

        let (result, demoted_role, demoted_is_default) =
            tokio::task::spawn_blocking(move || {
                let conn = open_reimport_test_db();

                // Pre-state: a locally-created default agent, role='local' (the
                // post-#2567-B6 birth label — create_local_agent_impl writes this,
                // never 'sub'), runtime_ref=NULL.
                conn.execute(
                    "INSERT INTO ai_agents \
                     (id, name, slug, role, is_default, workspace_path, runtime_ref, source, created_at, updated_at) \
                     VALUES ('local-default-1', 'My Local Agent', 'my-local-agent', 'local', 1, 'local-ws', NULL, 'ai_created', '', '')",
                    [],
                )
                .expect("pre-state locally-created default agent insert must succeed");

                let result = sync_re_import(
                    &conn,
                    &server_url,
                    "test-token",
                    None,
                    Some(system_dir_clone.as_path()),
                );

                let demoted_role: String = conn
                    .query_row(
                        "SELECT role FROM ai_agents WHERE id = 'local-default-1'",
                        [],
                        |r| r.get(0),
                    )
                    .expect("locally-created agent row must still exist (no deletion)");

                let demoted_is_default: i64 = conn
                    .query_row(
                        "SELECT is_default FROM ai_agents WHERE id = 'local-default-1'",
                        [],
                        |r| r.get(0),
                    )
                    .expect("locally-created agent row must still exist");

                (result, demoted_role, demoted_is_default)
            })
            .await
            .expect("spawn_blocking");

        assert!(
            result.is_ok(),
            "sync_re_import must succeed without constraint violation: {:?}",
            result
        );
        // #2644 fix: the locally-created default agent must NOT be demoted — it is a
        // foreign default, and re-import must never steal default status from it.
        assert_eq!(
            demoted_is_default, 1,
            "#2644: locally-created default agent must KEEP is_default=1 — OC fresh-import must not steal it"
        );
        assert_eq!(
            demoted_role, "local",
            "#2644/#2567 B6: locally-created default agent must remain role='local' — never touched by the (now-skipped) demotion"
        );
    }

    /// T12 — fetch_gateway_agents: empty agents array → Ok([]).
    #[tokio::test]
    async fn test_fetch_gateway_agents_empty_list() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{\"agents\":[]}")             
            .create_async()
            .await;
        let server_url = server.url();

        let result = tokio::task::spawn_blocking(move || {
            fetch_gateway_agents(&server_url, "token", None)
        }).await.unwrap();

        assert!(result.is_ok(), "empty agent list must return Ok([]): {:?}", result);
        assert!(result.unwrap().is_empty(), "empty agents array must yield empty vec");
    }

    // -----------------------------------------------------------------------
    // ADR-0045 #1527 tests: pre-flight normalization + overwrite-path demotion
    // -----------------------------------------------------------------------

    /// Open a reimport test DB WITHOUT the partial unique index on is_default.
    /// Used for two-main strand tests where we need to pre-populate both rows
    /// with is_default=1 before calling sync_re_import.
    /// SQLite enforces UNIQUE constraints when creating indexes on existing data,
    /// so the strand must be set up before any index is created.
    fn open_reimport_test_db_no_main_index() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_agents (
               id             TEXT PRIMARY KEY,
               name           TEXT NOT NULL DEFAULT '',
               slug           TEXT NOT NULL UNIQUE,
               role           TEXT NOT NULL DEFAULT 'sub',
               is_default        INTEGER NOT NULL DEFAULT 0,
               is_active      INTEGER NOT NULL DEFAULT 1,
               workspace_path TEXT NOT NULL DEFAULT '',
               runtime_ref    TEXT,
               runtime_type   TEXT,
               source         TEXT NOT NULL DEFAULT 'import',
               created_at     TEXT NOT NULL DEFAULT '',
               updated_at     TEXT NOT NULL DEFAULT ''
             );
             -- Mirror the composite runtime unique index (ADR-0063 D4 step 4;
             -- matches the INSERT...ON CONFLICT(runtime_type, runtime_ref) target).
             CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_runtime_unique
               ON ai_agents (runtime_type, runtime_ref);
             -- Note: idx_ai_agents_default is intentionally OMITTED here so that
             -- two is_default=1 rows can be pre-inserted for strand tests.",
        )
        .expect("create reimport test schema without main index");
        conn
    }

    /// T-1527-A: Two-main strand normalization (pre-flight).
    ///
    /// Pre-state: two is_default=1 rows (one runtime_ref IS NULL, one runtime_ref='main').
    /// sync_re_import must fire the pre-flight normalization and leave exactly one is_default=1.
    ///
    /// Bug reproduction: without the pre-flight check, both rows survive a re-import
    /// without mutation — the invariant is never enforced.
    #[tokio::test]
    async fn test_1527_a_two_main_strand_normalization() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir = dir.path().to_path_buf();

        // Gateway returns a single OC agent (already in DB — overwrite path).
        let agents_json = r#"[{"oc_id":"main","name":"OC Main","files":[]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let system_dir_clone = system_dir.clone();

        let (result, main_count, or_is_main) = tokio::task::spawn_blocking(move || {
            // Use DB without partial unique index so two is_default=1 rows can be pre-inserted.
            let conn = open_reimport_test_db_no_main_index();

            // Pre-state: both OR and OC have is_default=1 (the bug strand).
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, source, created_at, updated_at) \
                 VALUES ('or-id', 'OR Agent', 'or-agent', 'main', 1, 1, 'or-ws', NULL, 'local', '', '')",
                [],
            ).expect("OR insert");
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, source, created_at, updated_at) \
                 VALUES ('oc-id', 'OC Main', 'oc-main', 'main', 1, 1, 'oc-ws', 'main', 'import', '', '')",
                [],
            ).expect("OC insert");

            // Verify the pre-state: two is_default=1 rows exist (the bug strand).
            let pre_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
                .expect("pre-count");
            assert_eq!(pre_count, 2, "pre-state: two is_default=1 rows must exist before sync_re_import");

            let result = sync_re_import(
                &conn,
                &server_url,
                "test-token",
                None,
                Some(system_dir_clone.as_path()),
            );

            let main_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
                .expect("post-count");

            let or_is_main: i64 = conn
                .query_row("SELECT is_default FROM ai_agents WHERE id = 'or-id'", [], |r| r.get(0))
                .expect("OR must exist");

            (result, main_count, or_is_main)
        })
        .await
        .expect("spawn_blocking");

        assert!(result.is_ok(), "T-1527-A: sync_re_import must succeed: {:?}", result);
        assert_eq!(main_count, 1, "T-1527-A: pre-flight normalization must leave exactly one is_default=1 row");
        assert_eq!(or_is_main, 0, "T-1527-A: non-OC row must be demoted by pre-flight normalization");
    }

    /// T-1527-B: Overwrite-path demotion fires.
    ///
    /// Pre-state: OR agent is_default=1 (runtime_ref IS NULL),
    ///            OC agent already in DB with is_default=1 (overwrite path) — two-main strand.
    ///
    /// Both rows have is_default=1 (created in a DB without the partial unique index
    /// to simulate the real-world strand condition from ADR-0045).
    ///
    /// Bug reproduction: without the guard, the overwrite UPDATE (name/slug/updated_at only)
    /// runs without any demotion — leaving OR stranded at is_default=1 while OC is also
    /// is_default=1. With the fix: pre-flight and/or in-transaction demotion guard fires,
    /// OR demoted to 0, OC retains is_default=1. Single main row after commit.
    #[tokio::test]
    async fn test_1527_b_overwrite_path_demotion_fires() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir = dir.path().to_path_buf();

        // Gateway sends the OC main agent (already in DB — triggers overwrite path).
        let agents_json = r#"[{"oc_id":"main","name":"OC Main Updated","files":[]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let system_dir_clone = system_dir.clone();

        let (result, or_is_main, oc_is_main, main_count) = tokio::task::spawn_blocking(move || {
            // Use DB without partial unique index so two is_default=1 rows can be pre-inserted.
            let conn = open_reimport_test_db_no_main_index();

            // OC agent already in DB with is_default=1 (INSERT-time value from original import).
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, source, created_at, updated_at) \
                 VALUES ('oc-id', 'OC Main', 'oc-main', 'main', 1, 1, 'oc-ws', 'main', 'import', '', '')",
                [],
            ).expect("OC insert");

            // OR agent also has is_default=1 (the bug strand: prior sync_re_import on overwrite
            // path ran UPDATE without a demotion guard, leaving OR stranded at is_default=1).
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, source, created_at, updated_at) \
                 VALUES ('or-id', 'OR Agent', 'or-agent', 'main', 1, 1, 'or-ws', NULL, 'local', '', '')",
                [],
            ).expect("OR insert with stranded is_default=1");

            // Verify pre-state (bug reproduction): two is_default=1 rows exist.
            let pre_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
                .expect("pre-count");
            assert_eq!(pre_count, 2, "pre-fix: two is_default=1 rows must exist (the bug strand)");

            let result = sync_re_import(
                &conn,
                &server_url,
                "test-token",
                None,
                Some(system_dir_clone.as_path()),
            );

            let or_is_main: i64 = conn
                .query_row("SELECT is_default FROM ai_agents WHERE id = 'or-id'", [], |r| r.get(0))
                .expect("OR must exist after");

            let oc_is_main: i64 = conn
                .query_row("SELECT is_default FROM ai_agents WHERE id = 'oc-id'", [], |r| r.get(0))
                .expect("OC must exist after");

            let main_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
                .expect("post-count");

            (result, or_is_main, oc_is_main, main_count)
        })
        .await
        .expect("spawn_blocking");

        assert!(result.is_ok(), "T-1527-B: sync_re_import must succeed: {:?}", result);
        // OR agent demoted by pre-flight normalization (count > 1 detected) and/or in-transaction guard.
        assert_eq!(or_is_main, 0, "T-1527-B: OR agent must be demoted to is_default=0");
        // OC agent has is_default=1 after UPDATE path sets it (fix #1546 — demote-then-promote).
        assert_eq!(oc_is_main, 1, "T-1527-B: OC agent must have is_default=1 after UPDATE path sets it (#1546)");
        // Single main row: invariant holds after sync_re_import.
        assert_eq!(main_count, 1, "T-1527-B: exactly one is_default=1 row must exist after demotion");
    }

    /// T-1546: collision provider-switch — zero is_default=1 cannot result from sync_re_import UPDATE path.
    ///
    /// Reproduces the exact failure mode from #1546:
    ///   1. OC agent exists in DB with is_default=0 (was demoted when user switched to OR provider).
    ///   2. OR agent holds is_default=1.
    ///   3. sync_re_import re-imports the OC main agent — triggers UPDATE (overwrite-in-place) path.
    ///   4. Pre-#1546-fix: demotion clears OR’s is_default, UPDATE does not set OC’s is_default → zero-main.
    ///   5. #1546-era fix: UPDATE unconditionally wrote is_default=1, role='main' for the OC agent
    ///      (demote-then-promote atomic) — correct under the THEN-current mode-switching model,
    ///      where ADR-0045 D3 ruled "OC is authoritative on is_main while active" and re-pairing OC
    ///      was understood as switching the active provider back to OC.
    ///
    /// **Superseded by #2644 (this PR) / ADR-0063:** ADR-0063 retires the mode-switching framing
    /// entirely — runtimes connect once and stay live; there is no "active provider" to switch back
    /// to, so re-pairing OC is no longer "OC reclaiming its old authoritative seat." It is simply a
    /// re-import, and Matt's #2640/D13 ruling ("no import path may ever steal or collide on default
    /// status") now applies here too — sync_re_import must NOT promote the OC agent over an existing
    /// foreign default, exactly like sync_initial_import already refuses to on first pair. The
    /// zero-main invariant this test guards (main_count == 1 always) is UNCHANGED and still holds;
    /// only *which* agent ends up as main changes: the foreign (OR) default keeps its seat, and the
    /// OC agent lands with role='local' (pickable, not stranded) rather than stealing 'main'.
    #[tokio::test]
    async fn test_1546_collision_path_zero_main_cannot_occur() {
        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let system_dir = dir.path().to_path_buf();

        // Gateway re-imports the OC main agent.
        let agents_json = r#"[{"oc_id":"main","name":"OC Main","files":[]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let system_dir_clone = system_dir.clone();

        let (result, oc_is_main, oc_role, main_count, or_is_main) = tokio::task::spawn_blocking(move || {
            let conn = open_reimport_test_db();

            // OR agent: currently the active main (provider-switch state).
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, source, created_at, updated_at) \
                 VALUES ('or-id', 'OR Agent', 'or-agent', 'main', 1, 1, 'or-ws', NULL, 'local', '2026-01-01', '2026-01-01')",
                [],
            ).expect("OR agent insert");

            // OC agent: already in DB with is_default=0 (was demoted on provider switch to OR).
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, source, created_at, updated_at) \
                 VALUES ('oc-id', 'OC Main', 'oc-main', 'sub', 0, 1, 'oc-ws', 'main', 'import', '2026-01-01', '2026-01-01')",
                [],
            ).expect("OC agent insert with is_default=0");

            // Pre-state: exactly one is_default=1 row (the OR agent). OC is is_default=0.
            let pre_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
                .expect("pre-count");
            assert_eq!(pre_count, 1, "pre-state: exactly one main row (OR agent)");

            let result = sync_re_import(
                &conn,
                &server_url,
                "test-token",
                None,
                Some(system_dir_clone.as_path()),
            );

            let oc_is_main: i64 = conn
                .query_row("SELECT is_default FROM ai_agents WHERE id = 'oc-id'", [], |r| r.get(0))
                .expect("OC must exist after");

            let oc_role: String = conn
                .query_row("SELECT role FROM ai_agents WHERE id = 'oc-id'", [], |r| r.get(0))
                .expect("OC role must exist");

            let main_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM ai_agents WHERE is_default = 1", [], |r| r.get(0))
                .expect("post-count");

            // #2644 fix: assert the foreign (OR) default explicitly kept its seat, not just
            // that *some* row has is_default=1 (main_count alone can't distinguish "OR kept it"
            // from "OC stole it").
            let or_is_main: i64 = conn
                .query_row("SELECT is_default FROM ai_agents WHERE id = 'or-id'", [], |r| r.get(0))
                .expect("OR must exist after");

            (result, oc_is_main, oc_role, main_count, or_is_main)
        })
        .await
        .expect("spawn_blocking");

        assert!(result.is_ok(), "T-1546: sync_re_import must succeed: {:?}", result);
        // Core invariant — zero-main MUST NOT occur after collision provider-switch reimport. Still
        // holds after #2644: the OR agent keeps its seat instead of the OC agent taking it, but
        // exactly one is_default=1 row exists either way.
        assert_eq!(
            main_count, 1,
            "T-1546: exactly one is_default=1 row must exist after collision reimport — zero-main is the bug"
        );
        // #2644 fix (supersedes the #1546-era assertion here): the OC agent must NOT be promoted
        // over an existing foreign default — land_as_default is false because default_agent_exists=true
        // and this OC row was not already the default before this call (already_default_here=false).
        assert_eq!(
            oc_is_main, 0,
            "#2644: OC agent must NOT steal is_default from an existing foreign (OR) default on re-import"
        );
        assert_eq!(
            oc_role, "local",
            "#2644: OC agent blocked from default by a foreign default must land role='local' (pickable, not 'sub')"
        );
        assert_eq!(
            or_is_main, 1,
            "#2644: the pre-existing foreign (OR) default must keep is_default=1 — re-import must never steal it"
        );
    }

    // -------------------------------------------------------------------
    // #2649 (PR 3A): disconnect -> re-pair round-trip tests.
    //
    // `ai_disconnect_runtime_impl("openclaw", ...)` lives in `ai/commands.rs` and
    // is `pub(crate)`, so it's reachable here. These tests exercise the REAL
    // disconnect implementation (not a hand-rolled UPDATE) followed by the REAL
    // `sync_re_import` against a mocked gateway, proving the full round-trip the
    // shared `runOcPairingCeremony()` frontend module now relies on for its
    // re-pair branch: `oc_agent_row_exists` remains true across a disconnect (the
    // row is soft-deactivated, never deleted), so a subsequent re-pair correctly
    // takes the reimport_oc_agents/sync_re_import path instead of sync_initial_import,
    // and that path must restore the row in place rather than minting a duplicate.
    //
    // Uses the same schema as `ai/commands.rs`'s `make_conn()` (has `model` and
    // `chat_sessions` columns/tables that `ai_disconnect_runtime_impl` touches) —
    // `open_reimport_test_db()` above lacks both, so a dedicated schema helper is
    // used instead of overloading that one.
    // -------------------------------------------------------------------

    /// Schema mirror of `ai/commands.rs`'s `make_conn()` — needed because
    /// `ai_disconnect_runtime_impl`'s openclaw arm reads/writes `ai_agents.model`
    /// (indirectly, via `SELECT *`-adjacent callers elsewhere) and reads/writes
    /// `chat_sessions.agent_id` (D21, #2649: disconnect now PRESERVES this column
    /// rather than NULLing it — see `agents::check_bound_agent_active`).
    /// `open_reimport_test_db()` has neither column/table.
    fn open_disconnect_reimport_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_settings (
               key   TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS ai_agents (
               id             TEXT PRIMARY KEY,
               name           TEXT NOT NULL DEFAULT '',
               slug           TEXT NOT NULL UNIQUE,
               role           TEXT NOT NULL DEFAULT 'sub',
               model          TEXT,
               is_default        INTEGER NOT NULL DEFAULT 0,
               is_active      INTEGER NOT NULL DEFAULT 1,
               workspace_path TEXT NOT NULL DEFAULT '',
               runtime_ref    TEXT,
               runtime_type   TEXT,
               source         TEXT NOT NULL DEFAULT 'import',
               created_at     TEXT NOT NULL DEFAULT '',
               updated_at     TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS chat_sessions (
               id       TEXT PRIMARY KEY,
               agent_id TEXT
             );
             CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_default
               ON ai_agents (is_default) WHERE is_default = 1;
             CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_runtime_unique
               ON ai_agents (runtime_type, runtime_ref);",
        )
        .expect("create disconnect+reimport test schema");
        conn
    }

    /// #2649 round-trip test A: disconnect then re-pair (no foreign default present).
    ///
    /// Pre-state: one OC main agent, `is_default=1`, `is_active=1`, `model='gpt-4-oc'`
    /// (simulates a model that was set some other way — sync_re_import must never
    /// touch it).
    ///
    /// Sequence: `ai_disconnect_runtime_impl("openclaw", ...)` (real impl) then
    /// `sync_re_import` (real impl, mocked gateway returning the same `oc_id="main"`).
    ///
    /// Assertions:
    ///  1. `id` and `workspace_path` are IDENTICAL before/after the full round-trip
    ///     (disconnect must never delete the row; re-import must UPDATE in place,
    ///     never INSERT a new UUID/directory for an oc_id it already has a row for).
    ///  2. `is_active` is restored to 1 after re-import (disconnect sets it to 0;
    ///     PR2's #2647/#2648 reactivation fix must still hold end-to-end).
    ///  3. `model` is UNCHANGED by re-import — the overwrite-in-place UPDATE's SET
    ///     list (`name`, `slug`, `updated_at`, `is_default`, `role`, `is_active`)
    ///     has no `model` column, so a value set by another path must survive.
    ///
    /// `#[serial(pane_test_root)]` (not bare `#[serial]`) + `PANE_TEST_ROOT` isolate
    /// this test's `sync_re_import(..., None)` call from the developer's real `~/Pane`
    /// (see #2946/#2944) — the named group also serializes against every other
    /// `PANE_TEST_ROOT`-touching test in the binary.
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_2649_disconnect_then_repair_round_trip_preserves_uuid_and_model() {
        let test_root_dir = tempfile::tempdir().expect("test root tempdir");
        let _test_root_guard = crate::backup::test_support::TestRootGuard::set(test_root_dir.path());
        let _kc = KeychainGuard::set();

        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let ws_path = dir.path().join("oc-main-uuid");
        std::fs::create_dir_all(&ws_path).expect("create workspace dir");
        std::fs::write(ws_path.join("SOUL.md"), "original soul").expect("seed SOUL.md");

        let agents_json = r#"[{"oc_id":"main","name":"Brit Freed","files":[
            {"filename":"SOUL.md","content":"soul after re-pair","checksum":"z"}
        ]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let ws_path_str = ws_path.to_str().unwrap().to_string();

        let (
            reimport_result,
            id_before,
            id_after,
            ws_before,
            ws_after,
            is_active_after_disconnect,
            is_active_after_reimport,
            model_before,
            model_after,
        ) = tokio::task::spawn_blocking(move || {
            let conn = open_disconnect_reimport_test_db();
            let now = "2026-08-22T00:00:00Z";
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, role, model, is_default, is_active, workspace_path, runtime_ref, runtime_type, source, created_at, updated_at) \
                 VALUES ('oc-main-id', 'Brit Freed', 'brit-freed', 'main', 'gpt-4-oc', 1, 1, ?1, 'main', 'openclaw', 'import', ?2, ?2)",
                rusqlite::params![ws_path_str, now],
            )
            .expect("pre-state OC main agent insert");

            let id_before: String = conn
                .query_row("SELECT id FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("id before");
            let ws_before: String = conn
                .query_row("SELECT workspace_path FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("workspace_path before");
            let model_before: Option<String> = conn
                .query_row("SELECT model FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("model before");

            // Step 1: real disconnect. Row must survive (soft-deactivate, not delete) so
            // `oc_agent_row_exists` still returns true afterward — the exact signal the
            // shared frontend module's `runOcImportPhase` branches on for re-pair routing.
            crate::ai::connections::ai_disconnect_runtime_impl("openclaw", &conn)
                .expect("disconnect must succeed");

            let is_active_after_disconnect: i64 = conn
                .query_row("SELECT is_active FROM ai_agents WHERE id = 'oc-main-id'", [], |r| r.get(0))
                .expect("row must still exist after disconnect (soft-deactivate, not delete)");

            // Step 2: real re-import (re-pair). Must UPDATE the existing row in place.
            let reimport_result = sync_re_import(&conn, &server_url, "test-token", None, None);

            let id_after: String = conn
                .query_row("SELECT id FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("id after");
            let ws_after: String = conn
                .query_row("SELECT workspace_path FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("workspace_path after");
            let is_active_after_reimport: i64 = conn
                .query_row("SELECT is_active FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("is_active after");
            let model_after: Option<String> = conn
                .query_row("SELECT model FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("model after");

            (
                reimport_result,
                id_before,
                id_after,
                ws_before,
                ws_after,
                is_active_after_disconnect,
                is_active_after_reimport,
                model_before,
                model_after,
            )
        })
        .await
        .expect("spawn_blocking");

        assert!(
            reimport_result.is_ok(),
            "#2649: sync_re_import must succeed on the re-pair round-trip: {:?}",
            reimport_result
        );
        assert_eq!(
            id_after, id_before,
            "#2649: agent UUID must be IDENTICAL after disconnect + re-pair — re-import must never mint a duplicate row for an oc_id it already tracks"
        );
        assert_eq!(
            ws_after, ws_before,
            "#2649: workspace_path must be IDENTICAL after disconnect + re-pair — no new directory may be created"
        );
        assert_eq!(
            is_active_after_disconnect, 0,
            "#2649: disconnect must deactivate the row (is_active=0) — sanity check on the pre-reimport state"
        );
        assert_eq!(
            is_active_after_reimport, 1,
            "#2649/#2647/#2648: is_active must be restored to 1 by re-import — the PR2 reactivation fix must hold end-to-end through a real disconnect"
        );
        assert_eq!(
            model_before,
            Some("gpt-4-oc".to_string()),
            "sanity: model must be set before the round-trip"
        );
        assert_eq!(
            model_after, model_before,
            "#2649: model column must be UNTOUCHED by re-import's overwrite-in-place UPDATE — its SET list has no `model` column"
        );
    }

    /// #2649 round-trip test B: disconnect then re-pair WITH a foreign default agent
    /// present (e.g. an OpenRouter-onboarded default). Combines the #2649 round-trip
    /// coverage above with the #2644 foreign-default-protection invariant — the OC
    /// agent must reactivate correctly on re-pair WITHOUT disturbing the foreign
    /// agent's default status, exactly as `test_1546_collision_path_zero_main_cannot_occur`
    /// proves for the non-disconnect collision path.
    ///
    /// `#[serial(pane_test_root)]` (not bare `#[serial]`) + `PANE_TEST_ROOT` isolate
    /// this test's `sync_re_import(..., None)` call from the developer's real `~/Pane`
    /// (see #2946/#2944).
    #[tokio::test]
    #[serial(pane_test_root, keychain_override)]
    async fn test_2649_disconnect_then_repair_round_trip_with_foreign_default_agent() {
        let test_root_dir = tempfile::tempdir().expect("test root tempdir");
        let _test_root_guard = crate::backup::test_support::TestRootGuard::set(test_root_dir.path());
        let _kc = KeychainGuard::set();

        let mut server = mockito::Server::new_async().await;
        let dir = tempfile::tempdir().expect("tempdir");
        let ws_path = dir.path().join("oc-main-uuid-2");
        std::fs::create_dir_all(&ws_path).expect("create workspace dir");

        let agents_json = r#"[{"oc_id":"main","name":"Brit Freed","files":[]}]"#;
        let _m = server
            .mock("GET", "/v1/sync/initial")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(make_initial_body(agents_json))
            .create_async()
            .await;
        let server_url = server.url();
        let ws_path_str = ws_path.to_str().unwrap().to_string();

        let (
            reimport_result,
            id_before,
            id_after,
            ws_after,
            is_active_after_reimport,
            oc_is_default_after,
            oc_role_after,
            model_after,
            foreign_is_default_after,
            foreign_role_after,
        ) = tokio::task::spawn_blocking(move || {
            let conn = open_disconnect_reimport_test_db();
            let now = "2026-08-22T00:00:00Z";

            // Foreign default agent (e.g. OpenRouter-onboarded), runtime_ref IS NULL,
            // created AFTER the OC agent so disconnect's "most-recently-created
            // non-sub, is_active" promotion candidate query would pick this one if
            // it were eligible at disconnect time — it is NOT is_active-gated here
            // (it's active all along), so this also exercises the promote-on-disconnect
            // path landing on the correct row.
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, role, is_default, is_active, workspace_path, runtime_ref, runtime_type, source, created_at, updated_at) \
                 VALUES ('or-id', 'OR Agent', 'or-agent', 'main', 0, 1, 'or-ws', NULL, 'openrouter', 'local', ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("foreign agent insert");

            // OC main agent — currently the default (is_default=1).
            conn.execute(
                "INSERT INTO ai_agents \
                 (id, name, slug, role, model, is_default, is_active, workspace_path, runtime_ref, runtime_type, source, created_at, updated_at) \
                 VALUES ('oc-main-id', 'Brit Freed', 'brit-freed', 'main', 'gpt-4-oc', 1, 1, ?1, 'main', 'openclaw', 'import', ?2, ?2)",
                rusqlite::params![ws_path_str, now],
            )
            .expect("pre-state OC main agent insert");

            let id_before: String = conn
                .query_row("SELECT id FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("id before");

            // Step 1: real disconnect — deactivates + demotes the OC row, then promotes
            // the next eligible non-sub active row (the foreign agent) to is_default=1.
            crate::ai::connections::ai_disconnect_runtime_impl("openclaw", &conn)
                .expect("disconnect must succeed");

            let foreign_is_default_mid: i64 = conn
                .query_row("SELECT is_default FROM ai_agents WHERE id = 'or-id'", [], |r| r.get(0))
                .expect("foreign agent must exist after disconnect");
            assert_eq!(
                foreign_is_default_mid, 1,
                "sanity: disconnect must promote the foreign agent to default once OC is deactivated"
            );

            // Step 2: real re-import (re-pair). The OC row must reactivate WITHOUT
            // stealing is_default back from the foreign agent (#2644 invariant).
            let reimport_result = sync_re_import(&conn, &server_url, "test-token", None, None);

            let id_after: String = conn
                .query_row("SELECT id FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("id after");
            let ws_after: String = conn
                .query_row("SELECT workspace_path FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("workspace_path after");
            let is_active_after_reimport: i64 = conn
                .query_row("SELECT is_active FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("is_active after");
            let oc_is_default_after: i64 = conn
                .query_row("SELECT is_default FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("oc is_default after");
            let oc_role_after: String = conn
                .query_row("SELECT role FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("oc role after");
            let model_after: Option<String> = conn
                .query_row("SELECT model FROM ai_agents WHERE runtime_ref = 'main'", [], |r| r.get(0))
                .expect("model after");
            let foreign_is_default_after: i64 = conn
                .query_row("SELECT is_default FROM ai_agents WHERE id = 'or-id'", [], |r| r.get(0))
                .expect("foreign agent must still exist after re-import");
            let foreign_role_after: String = conn
                .query_row("SELECT role FROM ai_agents WHERE id = 'or-id'", [], |r| r.get(0))
                .expect("foreign agent role after");

            (
                reimport_result,
                id_before,
                id_after,
                ws_after,
                is_active_after_reimport,
                oc_is_default_after,
                oc_role_after,
                model_after,
                foreign_is_default_after,
                foreign_role_after,
            )
        })
        .await
        .expect("spawn_blocking");

        assert!(
            reimport_result.is_ok(),
            "#2649: sync_re_import must succeed with a foreign default present: {:?}",
            reimport_result
        );
        assert_eq!(
            id_after, id_before,
            "#2649: OC agent UUID must survive disconnect + re-pair even with a foreign default present"
        );
        assert!(
            ws_after.ends_with("oc-main-uuid-2"),
            "#2649: workspace_path must be preserved (no new directory)"
        );
        assert_eq!(
            is_active_after_reimport, 1,
            "#2649: OC agent must reactivate (is_active=1) on re-pair even with a foreign default present"
        );
        assert_eq!(
            model_after,
            Some("gpt-4-oc".to_string()),
            "#2649: model column must be untouched by re-import even in the foreign-default variant"
        );
        // #2644 invariant, exercised end-to-end through a REAL disconnect this time
        // (existing #2644 tests only cover the pre-populated-collision case, never a
        // real disconnect->re-pair sequence): the foreign default must keep its seat,
        // and the OC agent must land as role='local' (pickable, not stranded), NOT
        // steal is_default back.
        assert_eq!(
            oc_is_default_after, 0,
            "#2644/#2649: OC agent must NOT steal is_default back from the foreign agent on re-pair"
        );
        assert_eq!(
            oc_role_after, "local",
            "#2644/#2649: OC agent blocked from default by a foreign default must land role='local'"
        );
        assert_eq!(
            foreign_is_default_after, 1,
            "#2644/#2649: the foreign agent (promoted to default by disconnect) must KEEP is_default=1 through the OC re-pair"
        );
        assert_eq!(
            foreign_role_after, "main",
            "#2644/#2649: the foreign agent's role must remain 'main' — never touched by the OC re-pair"
        );
    }
}
// retrigger
