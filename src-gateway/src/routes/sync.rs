use crate::auth::TenantContext;
use crate::error::GatewayError;
use crate::sync_queue::PendingChange;
use axum::{
    extract::{Query, State},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::path::PathBuf;
use std::sync::Arc;

/// Maximum file size for sync push (10 MiB = 10,485,760 bytes)
pub const SYNC_PUSH_MAX_BYTES: usize = 10 * 1024 * 1024;

/// POST /v1/sync/push request
#[derive(Debug, Deserialize)]
pub struct PushRequest {
    pub oc_agent_id: String,
    pub filename: String,
    pub content: String,
    pub checksum: String,
}

/// POST /v1/sync/push response
#[derive(Debug, Serialize)]
pub struct PushResponse {
    pub written_at: DateTime<Utc>,
    pub checksum: String,
}

/// GET /v1/sync/pull query params
#[derive(Debug, Deserialize)]
pub struct PullQuery {
    pub oc_agent_id: String,
}

/// GET /v1/sync/pull response
#[derive(Debug, Serialize)]
pub struct PullResponse {
    pub pending_changes: Vec<PendingChange>,
}

/// DELETE /v1/sync/pull/confirm request
#[derive(Debug, Deserialize)]
pub struct ConfirmRequest {
    pub oc_agent_id: String,
    pub processed_ids: Vec<String>,
}

/// DELETE /v1/sync/pull/confirm response
#[derive(Debug, Serialize)]
pub struct ConfirmResponse {
    pub confirmed: usize,
}

/// GET /v1/sync/initial response
///
/// ADR-0067 D5 / `docs/spec/sync.md` §22C.6.2: contract is
/// `{ agents: [{ oc_id, name, is_default, files: [...] }] }` — `is_default` forwards
/// OC's per-agent `isDefault` (tier 1 of the three-step default-resolution order).
#[derive(Debug, Serialize)]
pub struct InitialSyncResponse {
    pub agents: Vec<AgentFiles>,
}

#[derive(Debug, Serialize)]
pub struct AgentFiles {
    pub oc_id: String,
    pub name: String,
    /// OC's per-agent `isDefault` flag (ADR-0067 D5). Always `false` on OC 2026.8.2
    /// (§22C.10) — correct-but-dormant tier 1 input for the app's default resolution.
    pub is_default: bool,
    pub files: Vec<FileContent>,
}

#[derive(Debug, Serialize)]
pub struct FileContent {
    pub filename: String,
    pub content: String,
    pub checksum: String,
}

/// POST /v1/sync/push - Push a single file to an agent's workspace
pub async fn push(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Json(request): Json<PushRequest>,
) -> Result<Json<PushResponse>, GatewayError> {
    eprintln!(
        "[SYNC] Push request: tenant={}, agent={}, file={}",
        tenant.tenant_id, request.oc_agent_id, request.filename
    );

    // Validate content size
    if request.content.len() > SYNC_PUSH_MAX_BYTES {
        eprintln!(
            "[SYNC] Content too large: {} bytes (max {})",
            request.content.len(),
            SYNC_PUSH_MAX_BYTES
        );
        return Err(GatewayError::PayloadTooLarge { max_bytes: SYNC_PUSH_MAX_BYTES });
    }

    // ADR-0040: resolve workspace from agent registry (agents.list workspace field)
    // Fetch agents (cache is used when available; force_refresh=false for push hot path)
    let agents = state
        .agent_registry
        .get_agents(&tenant.tenant_id, false)
        .await?;
    let agent = agents
        .iter()
        .find(|a| a.oc_agent_id == request.oc_agent_id)
        .ok_or_else(|| {
            eprintln!(
                "[SYNC] Push: agent '{}' not found in registry for tenant '{}'",
                request.oc_agent_id, tenant.tenant_id
            );
            GatewayError::Config(format!(
                "Agent '{}' not found in agent registry",
                request.oc_agent_id
            ))
        })?;
    let workspace_path = get_agent_workspace(agent)?;

    // ADR-0041 §22C.8.5: enforce allowlist on push-receive (Door B)
    let is_main = request.oc_agent_id == "main";
    if !gateway_is_allowed_sync_file(&request.filename, is_main) {
        eprintln!(
            "[SYNC] Push rejected: file '{}' not in allowlist for agent '{}' (is_main={})",
            request.filename, request.oc_agent_id, is_main
        );
        return Err(GatewayError::InvalidRequest(format!(
            "File '{}' is not in the sync allowlist for this agent role",
            request.filename
        )));
    }

    // Ensure parent directories exist
    let file_path = workspace_path.join(&request.filename);
    if let Some(parent) = file_path.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| {
            eprintln!("[SYNC] Failed to create directories: {}", e);
            GatewayError::Io(e)
        })?;
    }

    // Write to temp file first (atomic write pattern)
    let temp_path = file_path.with_extension("tmp");
    tokio::fs::write(&temp_path, &request.content)
        .await
        .map_err(|e| {
            eprintln!("[SYNC] Failed to write temp file {}: {}", temp_path.display(), e);
            GatewayError::Io(e)
        })?;

    // Read back and compute checksum (verify write integrity)
    let written = match tokio::fs::read(&temp_path).await {
        Ok(data) => data,
        Err(e) => {
            let _ = tokio::fs::remove_file(&temp_path).await;
            eprintln!("[SYNC] Failed to read back temp file {}: {}", temp_path.display(), e);
            return Err(GatewayError::Io(e));
        }
    };

    let mut hasher = sha2::Sha256::new();
    hasher.update(&written);
    let computed_checksum = hex::encode(hasher.finalize());

    // Verify against client checksum
    if computed_checksum != request.checksum {
        let _ = tokio::fs::remove_file(&temp_path).await;
        eprintln!(
            "[SYNC] Checksum mismatch: expected {}, got {}",
            request.checksum, computed_checksum
        );
        return Err(GatewayError::InvalidRequest(format!(
            "Checksum mismatch: expected {}, got {}",
            request.checksum, computed_checksum
        )));
    }

    // Atomic rename to final location
    if let Err(e) = tokio::fs::rename(&temp_path, &file_path).await {
        let _ = tokio::fs::remove_file(&temp_path).await;
        eprintln!("[SYNC] Failed to rename {} to {}: {}", temp_path.display(), file_path.display(), e);
        return Err(GatewayError::Io(e));
    }

    eprintln!("[SYNC] File written successfully with verified checksum: {}", file_path.display());

    Ok(Json(PushResponse {
        written_at: Utc::now(),
        checksum: request.checksum,
    }))
}

/// GET /v1/sync/pull - Get pending changes for an agent
pub async fn pull(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Query(query): Query<PullQuery>,
) -> Result<Json<PullResponse>, GatewayError> {
    eprintln!(
        "[SYNC] Pull request: tenant={}, agent={}",
        tenant.tenant_id, query.oc_agent_id
    );

    let pending_changes = state
        .sync_queue
        .get_pending(&tenant.tenant_id, &query.oc_agent_id)
        .await;

    eprintln!("[SYNC] Returning {} pending changes", pending_changes.len());

    Ok(Json(PullResponse { pending_changes }))
}

/// DELETE /v1/sync/pull/confirm - Confirm processed changes
pub async fn confirm(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
    Json(request): Json<ConfirmRequest>,
) -> Result<Json<ConfirmResponse>, GatewayError> {
    eprintln!(
        "[SYNC] Confirm request: tenant={}, agent={}, count={}",
        tenant.tenant_id,
        request.oc_agent_id,
        request.processed_ids.len()
    );

    let confirmed = state
        .sync_queue
        .confirm(
            &tenant.tenant_id,
            &request.oc_agent_id,
            request.processed_ids,
        )
        .await;

    eprintln!("[SYNC] Confirmed {} changes", confirmed);

    Ok(Json(ConfirmResponse { confirmed }))
}

/// GET /v1/sync/initial - Initial bulk fetch of all agent workspace files
pub async fn initial(
    State(state): State<Arc<crate::server::SharedState>>,
    Extension(tenant): Extension<TenantContext>,
) -> Result<Json<InitialSyncResponse>, GatewayError> {
    eprintln!("[SYNC] Initial sync request: tenant={}", tenant.tenant_id);

    // Get all agents for this tenant
    let agents = state
        .agent_registry
        .get_agents(&tenant.tenant_id, true)
        .await?;

    let mut agent_files = Vec::new();

    for agent in agents {
        eprintln!(
            "[SYNC] Fetching files for agent: {}",
            agent.oc_agent_id
        );

        // ADR-0040: resolve workspace from agent registry entry (agents.list workspace field)
        let workspace_path = match get_agent_workspace(&agent) {
            Ok(path) => path,
            Err(e) => {
                eprintln!(
                    "[SYNC] Failed to get workspace for agent {}: {}",
                    agent.oc_agent_id, e
                );
                // ADR-0040 Decision 4: missing workspace = surface error, not silent skip
                return Err(e);
            }
        };

        // Read allowlisted files from workspace (ADR-0041 §22C.8.5)
        let is_main = agent.oc_agent_id == "main";
        let files = read_workspace_files(&workspace_path, is_main).await?;

        agent_files.push(AgentFiles {
            oc_id: agent.oc_agent_id,
            name: agent.name,
            // ADR-0067 D5: forward OC's per-agent isDefault straight through.
            is_default: agent.is_default,
            files,
        });
    }

    eprintln!("[SYNC] Initial sync complete: {} agents", agent_files.len());

    Ok(Json(InitialSyncResponse {
        agents: agent_files,
    }))
}

/// Resolve the workspace directory path for an agent from its registry entry.
///
/// ADR-0040: workspace is sourced exclusively from OC's `agents.list` response
/// (§22C.6.4). No hardcoded paths. No `openclaw.json` reads (ADR-0015).
/// Missing or empty workspace → structured error + 500.
fn get_agent_workspace(agent: &crate::agents::AgentInfo) -> Result<PathBuf, GatewayError> {
    match &agent.workspace {
        Some(ws) if !ws.trim().is_empty() => {
            let path = PathBuf::from(ws);
            eprintln!("[SYNC] Agent workspace path for {}: {}", agent.oc_agent_id, path.display());
            Ok(path)
        }
        _ => {
            eprintln!(
                "[SYNC] ERROR: workspace missing or empty for agent '{}' (ADR-0040 Decision 4). \
                Check agents.list response from OC. OC may be outdated or misconfigured.",
                agent.oc_agent_id
            );
            Err(GatewayError::Config(format!(
                "Agent '{}' has no workspace path in agents.list response. \
                Cannot resolve workspace without OC-provided path (ADR-0040).",
                agent.oc_agent_id
            )))
        }
    }
}

/// Identity files synced for ALL agents (ADR-0041 §22C.8.5).
/// Cross-ref: src-tauri/src/sync_client.rs IDENTITY_WHITELIST
/// Both lists must stay in sync — see ADR-0041 Decision 5.
const GATEWAY_IDENTITY_WHITELIST: &[&str] = &[
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

/// Sub-agent 6-file set (ADR-0041 §22C.8.5).
/// Cross-ref: src-tauri/src/sync_client.rs SUB_AGENT_WHITELIST
const GATEWAY_SUB_AGENT_WHITELIST: &[&str] = &[
    "SOUL.md",
    "MEMORY.md",
    "IDENTITY.md",
    "AGENTS.md",
    "USER.md",
    "TOOLS.md",
];

/// Returns true if `filename` is of the form `prefix<name>.md` with exactly
/// one path component after the prefix (depth-1, no nested directories).
///
/// ADR-0041 Decision 6: depth-1 is a reader contract (OC dreaming reader
/// reads memory/*.md flat only).
#[allow(dead_code)] // used in tests and via gateway_is_allowed_sync_file
fn gateway_is_depth1_md(prefix: &str, filename: &str) -> bool {
    if let Some(rest) = filename.strip_prefix(prefix) {
        rest.ends_with(".md") && !rest.contains('/')
    } else {
        false
    }
}

/// Check if `filename` is allowed to sync for the given agent role.
///
/// Main agent: GATEWAY_IDENTITY_WHITELIST + memory/*.md depth-1 + logs/*.md depth-1.
/// Sub-agents: GATEWAY_SUB_AGENT_WHITELIST only.
///
/// ADR-0041 §22C.8.5
fn gateway_is_allowed_sync_file(filename: &str, is_main: bool) -> bool {
    if is_main {
        GATEWAY_IDENTITY_WHITELIST.contains(&filename)
            || gateway_is_depth1_md("memory/", filename)
            || gateway_is_depth1_md("logs/", filename)
    } else {
        GATEWAY_SUB_AGENT_WHITELIST.contains(&filename)
    }
}

/// Read allowlisted files from a workspace directory (ADR-0041 §22C.8.5).
///
/// Replaces the previous full recursive walk. Only files on the sync allowlist
/// are read; `memory/` and `logs/` are depth-1 only (reader contract).
async fn read_workspace_files(workspace_path: &PathBuf, is_main: bool) -> Result<Vec<FileContent>, GatewayError> {
    let mut files = Vec::new();

    // Check if workspace exists
    if !workspace_path.exists() {
        eprintln!(
            "[SYNC] Workspace does not exist: {}",
            workspace_path.display()
        );
        return Ok(files);
    }

    // 1. Identity whitelist: read flat files from workspace root.
    let whitelist = if is_main {
        GATEWAY_IDENTITY_WHITELIST
    } else {
        GATEWAY_SUB_AGENT_WHITELIST
    };

    for &fname in whitelist {
        let file_path = workspace_path.join(fname);
        if !file_path.exists() {
            continue;
        }
        match tokio::fs::read_to_string(&file_path).await {
            Ok(content) => {
                let checksum = calculate_checksum(&content);
                files.push(FileContent {
                    filename: fname.to_string(),
                    content,
                    checksum,
                });
            }
            Err(e) => {
                eprintln!("[SYNC] Failed to read {}: {}", file_path.display(), e);
            }
        }
    }

    // 2. For main agent: read memory/*.md and logs/*.md depth-1.
    if is_main {
        for dir_prefix in &["memory", "logs"] {
            let dir_path = workspace_path.join(dir_prefix);
            if !dir_path.exists() {
                continue;
            }
            let mut dir_entries = match tokio::fs::read_dir(&dir_path).await {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("[SYNC] Failed to read dir {}: {}", dir_path.display(), e);
                    continue;
                }
            };
            while let Some(entry) = dir_entries.next_entry().await.map_err(GatewayError::Io)? {
                let entry_path = entry.path();
                let meta = entry.metadata().await.map_err(GatewayError::Io)?;
                if !meta.is_file() {
                    // Skip subdirectories — depth-1 contract (ADR-0041 Decision 6)
                    continue;
                }
                let filename = format!("{}/{}",
                    dir_prefix,
                    entry_path.file_name().unwrap_or_default().to_string_lossy()
                );
                if !filename.ends_with(".md") {
                    continue;
                }
                match tokio::fs::read_to_string(&entry_path).await {
                    Ok(content) => {
                        let checksum = calculate_checksum(&content);
                        files.push(FileContent { filename, content, checksum });
                    }
                    Err(e) => {
                        eprintln!("[SYNC] Failed to read {}: {}", entry_path.display(), e);
                    }
                }
            }
        }
    }

    eprintln!("[SYNC] Read {} allowlisted file(s) from workspace (is_main={})", files.len(), is_main);
    Ok(files)
}

// read_dir_recursive removed: ADR-0041 replaces the recursive walk with an
// allowlist-based read in read_workspace_files. No other callers.

/// Calculate a simple checksum for content
/// Using SHA256 hash
fn calculate_checksum(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    hex::encode(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentInfo;
    use chrono::Utc;

    fn make_agent(oc_agent_id: &str, workspace: Option<&str>) -> AgentInfo {
        AgentInfo {
            oc_agent_id: oc_agent_id.to_string(),
            name: oc_agent_id.to_string(),
            model: None,
            role: None,
            workspace: workspace.map(|s| s.to_string()),
            is_default: false,
            registered_at: Utc::now(),
        }
    }

    /// ADR-0067 D5: the `/v1/sync/initial` response's `AgentFiles.is_default` must
    /// forward whatever `AgentInfo.is_default` the registry produced — this is the
    /// exact mapping `initial()` performs (routes/sync.rs above), tested directly here
    /// since `initial()` itself requires a live `SharedState`/OC connection to invoke.
    #[test]
    fn test_agent_files_forwards_is_default_true() {
        let agent = AgentInfo {
            oc_agent_id: "main".to_string(),
            name: "Brit".to_string(),
            model: None,
            role: None,
            workspace: Some("/workspace".to_string()),
            is_default: true,
            registered_at: Utc::now(),
        };
        let agent_files = AgentFiles {
            oc_id: agent.oc_agent_id.clone(),
            name: agent.name.clone(),
            is_default: agent.is_default,
            files: vec![],
        };
        assert!(agent_files.is_default, "AgentFiles.is_default must forward true from AgentInfo");
    }

    #[test]
    fn test_agent_files_forwards_is_default_false() {
        let agent = make_agent("coder", Some("/workspace-coder"));
        let agent_files = AgentFiles {
            oc_id: agent.oc_agent_id.clone(),
            name: agent.name.clone(),
            is_default: agent.is_default,
            files: vec![],
        };
        assert!(!agent_files.is_default, "AgentFiles.is_default must forward false from AgentInfo");
    }

    // ADR-0040: agent with valid workspace returns correct path
    #[test]
    fn test_get_agent_workspace_valid_path() {
        let agent = make_agent("main", Some("/Users/owner/.openclaw/workspace"));
        let result = get_agent_workspace(&agent);
        assert!(result.is_ok(), "valid workspace should succeed");
        assert_eq!(result.unwrap(), std::path::PathBuf::from("/Users/owner/.openclaw/workspace"));
    }

    // ADR-0040 Decision 4: missing workspace field → error (no fallback)
    #[test]
    fn test_get_agent_workspace_missing_returns_error() {
        let agent = make_agent("main", None);
        let result = get_agent_workspace(&agent);
        assert!(result.is_err(), "missing workspace must return error");
    }

    // ADR-0040 Decision 4: empty string workspace → error (same as missing)
    #[test]
    fn test_get_agent_workspace_empty_string_returns_error() {
        let agent = make_agent("coder", Some(""));
        // Note: empty strings are filtered to None during registry population,
        // but we test the function directly here for defense-in-depth
        let result = get_agent_workspace(&agent);
        assert!(result.is_err(), "empty workspace must return error");
    }

    // ADR-0040 Decision 5: sub-agent uses same resolution mechanism as main agent
    #[test]
    fn test_get_agent_workspace_sub_agent_same_mechanism() {
        let agents = vec![
            make_agent("main", Some("/Users/owner/.openclaw/workspace")),
            make_agent("coder", Some("/Users/owner/.openclaw/workspace-coder")),
            make_agent("researcher", Some("/Users/owner/.openclaw/workspace-researcher")),
        ];
        for agent in &agents {
            let result = get_agent_workspace(agent);
            assert!(result.is_ok(), "sub-agent {} workspace should resolve", agent.oc_agent_id);
        }
        // Each gets its own path
        assert_ne!(
            get_agent_workspace(&agents[0]).unwrap(),
            get_agent_workspace(&agents[1]).unwrap()
        );
    }

    // ADR-0040 Decision 4: whitespace-only workspace → error
    #[test]
    fn test_get_agent_workspace_whitespace_only_returns_error() {
        let mut agent = make_agent("main", Some("   "));
        // Simulate what registry stores (whitespace filtered to None in agents.rs)
        // but also test direct call defensively
        let result = get_agent_workspace(&agent);
        assert!(result.is_err(), "whitespace-only workspace must return error");
        // Simulate after registry filtering
        agent.workspace = None;
        let result2 = get_agent_workspace(&agent);
        assert!(result2.is_err(), "None workspace must return error");
    }

    #[test]
    fn test_checksum_calculation() {
        let content = "test content";
        let checksum1 = calculate_checksum(content);
        let checksum2 = calculate_checksum(content);

        // Same content should produce same checksum
        assert_eq!(checksum1, checksum2);

        // Different content should produce different checksum
        let checksum3 = calculate_checksum("different content");
        assert_ne!(checksum1, checksum3);
    }

    #[tokio::test]
    async fn test_push_validates_size() {
        // This is a unit test for the size validation logic
        let oversized_content = "x".repeat(SYNC_PUSH_MAX_BYTES + 1);
        assert!(oversized_content.len() > SYNC_PUSH_MAX_BYTES);

        // We would need to create a full test harness to test the endpoint
        // For now, we verify the constant is set correctly
        assert_eq!(SYNC_PUSH_MAX_BYTES, 10 * 1024 * 1024);
    }

    #[tokio::test]
    async fn test_atomic_write_checksum_verification() {
        use sha2::{Digest, Sha256};
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.md");
        let content = "# Test content";
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let checksum = hex::encode(hasher.finalize());
        let temp_path = file_path.with_extension("tmp");
        tokio::fs::write(&temp_path, content).await.unwrap();
        let written = tokio::fs::read(&temp_path).await.unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&written);
        let computed_checksum = hex::encode(hasher.finalize());
        assert_eq!(computed_checksum, checksum);
        tokio::fs::rename(&temp_path, &file_path).await.unwrap();
        assert!(file_path.exists());
        let final_content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(final_content, content);
        assert!(!temp_path.exists());
    }

    #[tokio::test]
    async fn test_checksum_mismatch_cleanup() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.md");
        let temp_path = file_path.with_extension("tmp");
        let content = "# Test content";
        tokio::fs::write(&temp_path, content).await.unwrap();
        assert!(temp_path.exists());
        let wrong_checksum = "deadbeef";
        let correct_checksum = calculate_checksum(content);
        if wrong_checksum != correct_checksum {
            let _ = tokio::fs::remove_file(&temp_path).await;
        }
        assert!(!temp_path.exists());
        assert!(!file_path.exists());
    }

    // ── ADR-0041 allowlist helpers ──────────────────────────────────────────

    #[test]
    fn test_gateway_is_depth1_md_passes() {
        assert!(gateway_is_depth1_md("logs/", "logs/2026-05-28.md"));
        assert!(gateway_is_depth1_md("memory/", "memory/2026-05-28.md"));
    }

    #[test]
    fn test_gateway_is_depth1_md_fails_nested() {
        assert!(!gateway_is_depth1_md("logs/", "logs/subdir/file.md"));
    }

    #[test]
    fn test_gateway_is_depth1_md_fails_non_md() {
        assert!(!gateway_is_depth1_md("logs/", "logs/file.txt"));
    }

    #[test]
    fn test_gateway_allowed_main_identity_files_pass() {
        for f in GATEWAY_IDENTITY_WHITELIST {
            assert!(gateway_is_allowed_sync_file(f, true), "{} should pass for main", f);
        }
    }

    #[test]
    fn test_gateway_allowed_main_memory_depth1_passes() {
        assert!(gateway_is_allowed_sync_file("memory/2026-05-28.md", true));
    }

    #[test]
    fn test_gateway_allowed_main_memory_nested_fails() {
        assert!(!gateway_is_allowed_sync_file("memory/subdir/note.md", true));
    }

    #[test]
    fn test_gateway_allowed_main_scratchpad_fails() {
        assert!(!gateway_is_allowed_sync_file("scratchpad-123.md", true));
    }

    #[test]
    fn test_gateway_allowed_sub_core_6_pass() {
        for f in GATEWAY_SUB_AGENT_WHITELIST {
            assert!(gateway_is_allowed_sync_file(f, false), "{} should pass for sub-agent", f);
        }
    }

    #[test]
    fn test_gateway_allowed_sub_heartbeat_fails() {
        assert!(!gateway_is_allowed_sync_file("HEARTBEAT.md", false));
    }

    #[test]
    fn test_gateway_allowed_sub_bootstrap_fails() {
        assert!(!gateway_is_allowed_sync_file("BOOTSTRAP.md", false));
    }

    #[test]
    fn test_gateway_allowed_sub_memory_fails() {
        assert!(!gateway_is_allowed_sync_file("memory/2026-05-28.md", false));
    }

    #[test]
    fn test_gateway_allowed_sub_logs_fails() {
        assert!(!gateway_is_allowed_sync_file("logs/2026-05-28.md", false));
    }

    // Integration: read_workspace_files with a real temp directory.
    // Verifies only allowlisted files are returned and non-allowlisted are excluded.
    #[tokio::test]
    async fn test_read_workspace_files_main_agent() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let ws = dir.path().to_path_buf();

        // Create allowlisted files
        tokio::fs::write(ws.join("SOUL.md"), "soul content").await.unwrap();
        tokio::fs::write(ws.join("MEMORY.md"), "memory content").await.unwrap();
        tokio::fs::write(ws.join("BOOTSTRAP.md"), "bootstrap").await.unwrap();
        std::fs::create_dir_all(ws.join("memory")).unwrap();
        tokio::fs::write(ws.join("memory/2026-05-28.md"), "daily").await.unwrap();
        std::fs::create_dir_all(ws.join("logs")).unwrap();
        tokio::fs::write(ws.join("logs/2026-05-28.md"), "log").await.unwrap();

        // Create non-allowlisted files
        tokio::fs::write(ws.join("scratchpad.md"), "junk").await.unwrap();
        tokio::fs::write(ws.join("archive.md"), "junk").await.unwrap();
        std::fs::create_dir_all(ws.join("memory/subdir")).unwrap();
        tokio::fs::write(ws.join("memory/subdir/note.md"), "nested").await.unwrap();
        tokio::fs::write(ws.join("file.json"), "{}").await.unwrap();

        let files = read_workspace_files(&ws, true).await.unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.filename.as_str()).collect();

        assert!(names.contains(&"SOUL.md"), "SOUL.md must be included");
        assert!(names.contains(&"MEMORY.md"), "MEMORY.md must be included");
        assert!(names.contains(&"BOOTSTRAP.md"), "BOOTSTRAP.md must be included");
        assert!(names.contains(&"memory/2026-05-28.md"), "memory depth-1 must be included");
        assert!(names.contains(&"logs/2026-05-28.md"), "logs depth-1 must be included");
        assert!(!names.contains(&"scratchpad.md"), "scratchpad must be excluded");
        assert!(!names.contains(&"archive.md"), "archive must be excluded");
        assert!(!names.contains(&"memory/subdir/note.md"), "nested memory must be excluded");
        assert!(!names.contains(&"file.json"), "json must be excluded");
    }

    // ── ADR-0041 Door B: push-receive allowlist enforcement ─────────────────

    /// B-a: pushing a non-allowlisted file returns an error for main agent.
    #[test]
    fn test_push_receive_rejects_non_allowlisted_file_main() {
        let is_main = true;
        assert!(
            !gateway_is_allowed_sync_file("scratchpad.txt", is_main),
            "scratchpad.txt must not be in main allowlist"
        );
    }

    /// B-b: pushing an allowlisted file succeeds (passes the filter) for main agent.
    #[test]
    fn test_push_receive_allows_soul_md_main() {
        let is_main = true;
        assert!(
            gateway_is_allowed_sync_file("SOUL.md", is_main),
            "SOUL.md must be allowed for main agent push"
        );
    }

    /// B-c: sub-agent pushing HEARTBEAT.md is rejected.
    #[test]
    fn test_push_receive_rejects_heartbeat_sub_agent() {
        let is_main = false;
        assert!(
            !gateway_is_allowed_sync_file("HEARTBEAT.md", is_main),
            "HEARTBEAT.md must not be allowed for sub-agent push"
        );
    }

    /// B-d: sub-agent pushing SOUL.md is allowed.
    #[test]
    fn test_push_receive_allows_soul_md_sub_agent() {
        let is_main = false;
        assert!(
            gateway_is_allowed_sync_file("SOUL.md", is_main),
            "SOUL.md must be allowed for sub-agent push"
        );
    }

    /// B-e: main agent pushing memory/2026-05-28.md (depth-1) is allowed.
    #[test]
    fn test_push_receive_allows_memory_depth1_main() {
        let is_main = true;
        assert!(
            gateway_is_allowed_sync_file("memory/2026-05-28.md", is_main),
            "memory depth-1 .md must be allowed for main agent push"
        );
    }

    /// B-f: sub-agent pushing a memory file is rejected.
    #[test]
    fn test_push_receive_rejects_memory_sub_agent() {
        let is_main = false;
        assert!(
            !gateway_is_allowed_sync_file("memory/2026-05-28.md", is_main),
            "memory/ files must not be allowed for sub-agent push"
        );
    }

    #[tokio::test]
    async fn test_read_workspace_files_sub_agent() {
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let ws = dir.path().to_path_buf();

        // Create files
        for f in GATEWAY_IDENTITY_WHITELIST {
            tokio::fs::write(ws.join(f), "content").await.unwrap();
        }
        std::fs::create_dir_all(ws.join("memory")).unwrap();
        tokio::fs::write(ws.join("memory/2026-05-28.md"), "daily").await.unwrap();

        let files = read_workspace_files(&ws, false).await.unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.filename.as_str()).collect();

        // Sub-agent 6-file set passes
        for f in GATEWAY_SUB_AGENT_WHITELIST {
            assert!(names.contains(f), "{} must be included for sub-agent", f);
        }
        // HEARTBEAT, BOOTSTRAP, RULES excluded
        assert!(!names.contains(&"HEARTBEAT.md"), "HEARTBEAT must be excluded for sub-agent");
        assert!(!names.contains(&"BOOTSTRAP.md"), "BOOTSTRAP must be excluded for sub-agent");
        assert!(!names.contains(&"RULES.md"), "RULES must be excluded for sub-agent");
        // memory/ excluded entirely
        assert!(!names.contains(&"memory/2026-05-28.md"), "memory must be excluded for sub-agent");
    }
}
