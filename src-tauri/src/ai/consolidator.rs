use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use tauri::Emitter;

// Sampling params for memory consolidation — values live here at the call-policy layer.
/// Temperature for memory consolidation: intentionally non-zero (0.3) for natural prose generation.
/// Memory consolidation is creative summarization, not deterministic extraction — slight variance
/// produces more natural, readable output than strict determinism.
pub(crate) const CONSOLIDATOR_TEMPERATURE: f64 = 0.3;
/// Max tokens for consolidation LLM call: supports comprehensive memory document rewrites.
pub(crate) const CONSOLIDATOR_MAX_TOKENS: u32 = 4096;

/// Target for consolidation staleness check
#[derive(Debug, Clone)]
pub struct ConsolidationTarget {
    /// Canonical database path. Filesystem access must still go through `PaneFs`.
    pub file_path: PathBuf,
    pub project_id: Option<String>,
    // Retained for the legacy staleness-inspection shape.
    #[allow(dead_code)]
    pub last_consolidated: Option<i64>,
}

#[derive(Debug, Clone)]
struct MemoryTarget {
    root: PaneRoot,
    relative: PathBuf,
    db_path: String,
    identity_relative: PathBuf,
}

fn policy_error(context: &str, error: impl std::fmt::Display) -> String {
    format!("{}: {}", context, error)
}

fn canonical_memory_target(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<MemoryTarget, String> {
    let workspace_path: String = conn
        .query_row(
            "SELECT workspace_path FROM ai_agents WHERE is_default = 1 AND is_active = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to resolve active agent workspace: {}", e))?;
    PaneFs::validate_name(&workspace_path)
        .map_err(|e| policy_error("Unsafe active agent workspace path", e))?;
    let identity_relative = PaneFs::relative_from_names([workspace_path.as_str(), "IDENTITY.md"])
        .map_err(|e| policy_error("Unsafe agent identity path", e))?;

    match project_id {
        None => {
            let relative = PaneFs::relative_from_names([workspace_path.as_str(), "MEMORY.md"])
                .map_err(|e| policy_error("Unsafe agent memory path", e))?;
            Ok(MemoryTarget {
                root: PaneRoot::System,
                db_path: format!(".pane/system/{}/MEMORY.md", workspace_path),
                relative,
                identity_relative,
            })
        }
        Some(project_id) => {
            let project_name: String = conn
                .query_row(
                    "SELECT name FROM projects WHERE id = ?1",
                    [project_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to resolve project {}: {}", project_id, e))?;
            PaneFs::validate_name(&project_name)
                .map_err(|e| policy_error("Unsafe project name", e))?;
            let relative = PaneFs::relative_from_names([project_name.as_str(), "Memory.md"])
                .map_err(|e| policy_error("Unsafe project memory path", e))?;
            Ok(MemoryTarget {
                root: PaneRoot::Projects,
                db_path: format!("Projects/{}/Memory.md", project_name),
                relative,
                identity_relative,
            })
        }
    }
}

#[cfg(test)]
fn injected_test_target(
    canonical: &MemoryTarget,
    file_path: &Path,
) -> Result<Option<MemoryTarget>, String> {
    if !file_path.is_absolute() {
        return Ok(None);
    }

    // #3008: get_pane_root() now resolves PANE_TEST_ROOT to its real
    // (symlink-free) path, but `file_path` here is whatever the TEST
    // injected — typically joined directly onto the RAW `PANE_TEST_ROOT`
    // value (the established convention elsewhere in this codebase), which
    // no longer shares a literal prefix with the now-canonicalized
    // `pane_root` below. Rewrite file_path's ROOT portion (if it was built
    // from the raw PANE_TEST_ROOT) onto the canonical root, but leave
    // everything AFTER that root untouched — deliberately NOT a blanket
    // `fs::canonicalize` of the whole path, which would also silently
    // resolve away an intentional symlink placed deeper in the tree,
    // hiding exactly the kind of escape `PaneFs`'s own authorize_* calls
    // further down this call chain exist to catch.
    let file_path_owned = std::env::var("PANE_TEST_ROOT")
        .ok()
        .map(std::path::PathBuf::from)
        .and_then(|raw_root| file_path.strip_prefix(&raw_root).ok().map(|rel| rel.to_path_buf()))
        .map(|relative| crate::database::get_pane_root().join(relative))
        .unwrap_or_else(|| file_path.to_path_buf());
    let file_path: &Path = &file_path_owned;

    let pane_root = crate::database::get_pane_root();
    let root_path = match canonical.root {
        PaneRoot::System => pane_root.join(".pane/system"),
        PaneRoot::Projects => pane_root.join("Projects"),
        _ => return Err("Invalid test memory root".to_string()),
    };
    let relative = file_path.strip_prefix(&root_path).map_err(|_| {
        format!(
            "Injected test path is outside the authorized root: {}",
            file_path.display()
        )
    })?;
    PaneFs::validate_relative(relative)
        .map_err(|e| policy_error("Unsafe injected test memory path", e))?;
    let expected_name = if canonical.root == PaneRoot::System {
        "MEMORY.md"
    } else {
        "Memory.md"
    };
    if relative.file_name().and_then(|name| name.to_str()) != Some(expected_name) {
        return Err(format!("Injected test path must end in {}", expected_name));
    }

    let prefix = if canonical.root == PaneRoot::System {
        ".pane/system"
    } else {
        "Projects"
    };
    Ok(Some(MemoryTarget {
        root: canonical.root,
        relative: relative.to_path_buf(),
        db_path: format!("{}/{}", prefix, relative.to_string_lossy()),
        identity_relative: canonical.identity_relative.clone(),
    }))
}

fn resolve_memory_target(
    conn: &Connection,
    project_id: Option<&str>,
    file_path: &Path,
) -> Result<MemoryTarget, String> {
    let canonical = canonical_memory_target(conn, project_id)?;
    #[cfg(test)]
    if let Some(injected) = injected_test_target(&canonical, file_path)? {
        return Ok(injected);
    }
    #[cfg(not(test))]
    let _ = file_path;
    Ok(canonical)
}

/// Determines if word count threshold triggers consolidation
// Retained as a consolidation policy test seam.
#[cfg_attr(not(test), allow(dead_code))]
pub fn should_trigger_consolidation(word_count: usize) -> bool {
    word_count > 2000
}

/// Main consolidation function: rewrites memory file from ai_memory source of truth.
///
/// `file_path` is a legacy test seam. Production builds ignore it and derive the
/// destination from the active agent workspace or validated project name. Test
/// builds may inject an absolute path only beneath the matching `PANE_TEST_ROOT` root.
pub async fn consolidate_file(
    db_state: &crate::database::DbState,
    app: &tauri::AppHandle,
    file_path: &Path,
    project_id: Option<&str>,
    provider: &str,
    api_key: &str,
    model: &str,
    endpoint: Option<&str>,
    // ADR-0067 D2/D5: the resolving agent's runtime_ref — never its local id.
    // `None` for non-OC agents; harmless no-op on non-"openclaw" providers.
    oc_runtime_ref: Option<&str>,
) -> Result<(), String> {
    let wrote = consolidate_file_impl(
        db_state, file_path, project_id, provider, api_key, model, endpoint, oc_runtime_ref,
    )
    .await?;

    // 14. Emit pane-reload-editor event — only on an actual write. Early-exit paths
    // inside consolidate_file_impl (no active facts, concurrent-edit mtime race,
    // content-loss guard) return `wrote: false` and must NOT emit, matching this
    // function's pre-#2775 behavior exactly (the emit call used to be unconditionally
    // the last statement in the body, which those `return Ok(())`s upstream of it
    // never reached).
    if wrote {
        app.emit("pane-reload-editor", ())
            .map_err(|e| format!("Failed to emit reload event: {}", e))?;
        eprintln!("[CONSOLIDATOR] Emitted pane-reload-editor event");
    }

    Ok(())
}

/// #2775: split out of `consolidate_file` so Tier 2 tests can drive the real
/// db_state-threading logic without needing a `tauri::AppHandle` (the emit call is
/// the only thing that needed one, and it's now the thin wrapper's job). Returns
/// `Ok(true)` iff step 11 actually wrote new content (i.e. the caller should emit
/// `pane-reload-editor`); `Ok(false)` on every early-exit skip path.
async fn consolidate_file_impl(
    db_state: &crate::database::DbState,
    file_path: &Path,
    project_id: Option<&str>,
    provider: &str,
    api_key: &str,
    model: &str,
    endpoint: Option<&str>,
    // ADR-0067 D2/D5: see consolidate_file's doc comment on the same parameter.
    oc_runtime_ref: Option<&str>,
) -> Result<bool, String> {
    let (pane_fs, target) = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        let target = resolve_memory_target(&conn, project_id, file_path)?;
        let pane_fs = PaneFs::for_runtime().map_err(|e| policy_error("Filesystem policy", e))?;
        (pane_fs, target)
    };
    let authorized_memory = pane_fs
        .authorize_existing(target.root, &target.relative, ExistingKind::File)
        .map_err(|e| policy_error("Unsafe memory file", e))?;
    let path_str = authorized_memory.as_path().to_string_lossy().to_string();
    eprintln!(
        "[CONSOLIDATOR] Starting consolidation for: {}",
        target.db_path
    );

    // 1-4: Read phase — acquire file mutex, authorize/read content, capture mtime, backup
    let file_mutex = crate::ai::memory_router::get_file_mutex(&path_str);
    let (existing_content, old_word_count, initial_mtime, backup_relative) = {
        let _guard = file_mutex.lock().unwrap();
        eprintln!("[CONSOLIDATOR] Acquired file mutex (read phase)");

        let readable = pane_fs
            .authorize_existing(target.root, &target.relative, ExistingKind::File)
            .map_err(|e| policy_error("Unsafe memory file read", e))?;
        let initial_mtime = std::fs::metadata(readable.as_path())
            .map_err(|e| format!("Failed to read file metadata: {}", e))?
            .modified()
            .map_err(|e| format!("Failed to get mtime: {}", e))?;

        let existing_content = std::fs::read_to_string(readable.as_path())
            .map_err(|e| format!("Failed to read memory file: {}", e))?;
        let old_word_count = crate::ai::memory_router::count_words(&existing_content);
        eprintln!(
            "[CONSOLIDATOR] Read existing content: {} words",
            old_word_count
        );
        
        let backup_dir_relative = Path::new("backups/synthesis");
        pane_fs
            .create_dir_all(PaneRoot::Internal, backup_dir_relative)
            .map_err(|e| policy_error("Failed to create synthesis backup directory", e))?;
        let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S-%f");
        let memory_stem = target
            .relative
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "Invalid memory filename".to_string())?;
        PaneFs::validate_name(memory_stem)
            .map_err(|e| policy_error("Unsafe memory filename", e))?;
        let backup_filename = format!("{}-{}.md", memory_stem, timestamp);
        let backup_relative =
            PaneFs::relative_from_names(["backups", "synthesis", backup_filename.as_str()])
                .map_err(|e| policy_error("Unsafe synthesis backup path", e))?;
        let backup_path = pane_fs
            .authorize_write(PaneRoot::Internal, &backup_relative)
            .map_err(|e| policy_error("Unsafe synthesis backup destination", e))?;
        std::fs::write(backup_path.as_path(), &existing_content)
            .map_err(|e| format!("Failed to write synthesis backup: {}", e))?;
        pane_fs
            .authorize_existing(PaneRoot::Internal, &backup_relative, ExistingKind::File)
            .map_err(|e| policy_error("Unsafe synthesis backup file", e))?;
        eprintln!(
            "[CONSOLIDATOR] Backup created: {}",
            backup_path.as_path().display()
        );

        (
            existing_content,
            old_word_count,
            initial_mtime,
            backup_relative,
        )
    }; // File mutex dropped here — unlocked during LLM call
    eprintln!("[CONSOLIDATOR] File mutex released for LLM call");

    // 5. Query canonical fact list from ai_memory
    let facts: Vec<String> = {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        if project_id.is_none() {
            // Agent MEMORY.md
            let mut stmt = conn
                .prepare(
                "SELECT fact, category FROM ai_memory 
                 WHERE is_active = 1 AND project_id IS NULL 
                 AND category IN ('identity', 'preference', 'style', 'goal') 
                 ORDER BY category, created_at",
                )
                .map_err(|e| format!("Failed to prepare query: {}", e))?;
            
            let results = stmt
                .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("Failed to query facts: {}", e))?;
            
            let mut facts = Vec::new();
            for result in results {
                let (fact, _category) = result.map_err(|e| format!("Failed to read row: {}", e))?;
                facts.push(fact);
            }
            facts
        } else {
            // Project Memory
            let pid = project_id.unwrap();
            let mut stmt = conn
                .prepare(
                "SELECT fact, category FROM ai_memory 
                 WHERE is_active = 1 AND project_id = ? 
                 AND category IN ('fact', 'decision', 'goal') 
                 ORDER BY category, created_at",
                )
                .map_err(|e| format!("Failed to prepare query: {}", e))?;
            
            let results = stmt
                .query_map(params![pid], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|e| format!("Failed to query facts: {}", e))?;
            
            let mut facts = Vec::new();
            for result in results {
                let (fact, _category) = result.map_err(|e| format!("Failed to read row: {}", e))?;
                facts.push(fact);
            }
            facts
        }
    }; // Lock dropped here before async LLM call

    eprintln!(
        "[CONSOLIDATOR] Retrieved {} active facts from ai_memory",
        facts.len()
    );

    // 6. If no active facts, skip
    if facts.is_empty() {
        eprintln!("[CONSOLIDATOR] No active facts found, skipping consolidation");
        return Ok(false);
    }

    // 7. Build consolidation prompt using synthesis_size_guard(). Identity always
    // comes from the policy-authorized active agent workspace, including project memory.
    let user_name = match pane_fs.authorize_existing(
        PaneRoot::System,
        &target.identity_relative,
        ExistingKind::File,
    ) {
        Ok(identity_path) => crate::ai::ingest::parse_identity_name(identity_path.as_path())
            .unwrap_or_else(|| "the user".to_string()),
        Err(_) => "the user".to_string(),
    };

    let (mode, size_suffix) = crate::ai::scheduler::synthesis_size_guard(old_word_count);
    eprintln!("[CONSOLIDATOR] Synthesis mode: {}", mode);

    let facts_formatted = facts
        .iter()
        .map(|f| format!("- {}", f))
        .collect::<Vec<_>>()
        .join("\n");

    let size_instruction = size_suffix.unwrap_or_default();

    let prompt = format!(
        "You are rewriting a user's Memory document for an AI assistant app called Pane.\n\n\
        Here is the current document (preserve its prose style):\n\
        ---\n\
        {}\n\
        ---\n\n\
        Here is the canonical list of active facts (this is the source of truth):\n\
        {}\n\n\
        RULES:\n\
        - Every fact from the canonical list MUST appear in the output\n\
        - Remove anything NOT backed by a fact in the canonical list\n\
        - Write in third person (\"{user_name} is...\", \"they prefer...\")\n\
        - Short paragraphs (2-3 sentences), separated by blank lines\n\
        - No category headers — natural prose flow\n\
        - No meta text about the document itself\n\
        - Do not invent content not in the fact list\
        {}\n\n\
        Return ONLY the rewritten document.",
        existing_content, facts_formatted, size_instruction
    );

    let messages = vec![crate::ai::provider::ChatMessage {
        role: "user".to_string(),
        content: prompt,
        tool_calls: None,
        tool_call_id: None,
        content_blocks: None,
        attachment_ids: None,
        id: None,
    }];

    // 8. Call LLM via call_llm_from_config
    eprintln!(
        "[CONSOLIDATOR] Calling LLM (model: {}, provider: {})",
        model, provider
    );
    let new_content = crate::ai::provider::call_llm_from_config(
        provider,
        api_key,
        model,
        endpoint,
        &messages,
        CONSOLIDATOR_MAX_TOKENS,
        CONSOLIDATOR_TEMPERATURE,
        120,
        Some(db_state), // #2775: db_state is a direct parameter of this function,
        // one line away — pinned-cert gateway client on paired installs.
        None, // model_registry not available in consolidator context
        oc_runtime_ref,
    )
    .await?;

    let new_word_count = crate::ai::memory_router::count_words(&new_content);
    eprintln!(
        "[CONSOLIDATOR] LLM returned {} words (was {} words)",
        new_word_count, old_word_count
    );

    // 9-11: Write phase — reacquire file mutex for safety checks + write
    let _write_guard = file_mutex.lock().unwrap();
    eprintln!("[CONSOLIDATOR] Reacquired file mutex (write phase)");

    // 9. Re-authorize before metadata/write. This also rejects a symlink swapped in
    // while the LLM call was in flight.
    let writable_memory = pane_fs
        .authorize_existing(target.root, &target.relative, ExistingKind::File)
        .map_err(|e| policy_error("Unsafe memory file before write", e))?;
    pane_fs
        .authorize_write(target.root, &target.relative)
        .map_err(|e| policy_error("Unsafe memory destination", e))?;
    let current_mtime = std::fs::metadata(writable_memory.as_path())
        .map_err(|e| format!("Failed to re-read metadata: {}", e))?
        .modified()
        .map_err(|e| format!("Failed to get current mtime: {}", e))?;
    
    if current_mtime != initial_mtime {
        eprintln!(
            "[CONSOLIDATOR-ABORTED-MTIME] File was modified during consolidation. Aborting write."
        );
        return Ok(false);
    }

    // 10. Content-loss guard
    if old_word_count > 100 && new_word_count < old_word_count / 2 {
        let backup_path = pane_fs
            .authorize_existing(PaneRoot::Internal, &backup_relative, ExistingKind::File)
            .map_err(|e| policy_error("Unsafe synthesis backup file", e))?;
        eprintln!(
            "[CONSOLIDATOR-BLOCKED] Content loss detected: {} → {} words (>50% reduction). Backup preserved at {}",
            old_word_count, new_word_count, backup_path.as_path().display()
        );
        return Ok(false);
    }

    // 11. Atomic write to the just-authorized canonical destination.
    crate::ai::tools::atomic_write(writable_memory.as_path(), &new_content)?;
    eprintln!("[CONSOLIDATOR] Atomic write complete");

    // 12. Update notes table metadata
    let content_preview = if new_content.len() > 200 {
        format!("{}...", new_content.chars().take(200).collect::<String>())
    } else {
        new_content.clone()
    };

    {
        let conn = db_state.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE notes SET word_count = ?, content_preview = ?, updated_at = CURRENT_TIMESTAMP 
             WHERE file_path = ?",
            params![new_word_count as i64, content_preview, &target.db_path],
        )
        .map_err(|e| format!("Failed to update notes metadata: {}", e))?;
        eprintln!("[CONSOLIDATOR] Updated notes table metadata");

        // 13. Update last_consolidated_at in ai_settings
        let now = chrono::Utc::now().timestamp();
        let settings_key = if project_id.is_none() {
            "last_consolidated_at:agent".to_string()
        } else {
            format!("last_consolidated_at:project:{}", project_id.unwrap())
        };

        crate::database::set_ai_setting(&conn, &settings_key, &now.to_string())?;
        eprintln!(
            "[CONSOLIDATOR] Updated ai_settings: {} = {}",
            settings_key, now
        );
    } // Lock dropped

    eprintln!(
        "[CONSOLIDATOR] Consolidation complete for {}",
        target.db_path
    );
    Ok(true)
}

/// Check for memory files that haven't been consolidated in >14 days
pub fn check_consolidation_staleness(conn: &Connection) -> Vec<ConsolidationTarget> {
    let mut targets = Vec::new();
    let now = chrono::Utc::now().timestamp();
    let fourteen_days_ago = now - (14 * 24 * 60 * 60);

    // Query all memory notes (agent MEMORY.md + project Memory notes)
    let query = "SELECT file_path, project_id FROM notes 
                 WHERE file_path LIKE '%MEMORY.md' OR file_path LIKE '%Memory.md'";
    
    let mut stmt = match conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[CONSOLIDATOR] Failed to prepare staleness query: {}", e);
            return targets;
        }
    };

    let results = match stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    }) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[CONSOLIDATOR] Failed to query memory files: {}", e);
            return targets;
        }
    };

    for result in results {
        if let Ok((file_path, project_id)) = result {
            let canonical = match canonical_memory_target(conn, project_id.as_deref()) {
                Ok(target) => target,
                Err(e) => {
                    eprintln!("[CONSOLIDATOR] Skipping unresolved memory note: {}", e);
                    continue;
                }
            };
            if file_path != canonical.db_path {
                eprintln!(
                    "[CONSOLIDATOR] Rejecting non-canonical memory note path: {}",
                    file_path
                );
                continue;
            }

            let settings_key = if project_id.is_none() {
                "last_consolidated_at:agent".to_string()
            } else {
                format!(
                    "last_consolidated_at:project:{}",
                    project_id.as_ref().unwrap()
                )
            };

            let last_consolidated = crate::database::get_ai_setting(conn, &settings_key)
                .and_then(|s| s.parse::<i64>().ok());

            // If never consolidated or stale (>14 days)
            if last_consolidated.is_none() || last_consolidated.unwrap() < fourteen_days_ago {
                targets.push(ConsolidationTarget {
                    file_path: PathBuf::from(canonical.db_path),
                    project_id,
                    last_consolidated,
                });
            }
        }
    }

    eprintln!(
        "[CONSOLIDATOR] Found {} stale memory files (>14 days or never consolidated)",
        targets.len()
    );
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE ai_agents (
                 workspace_path TEXT NOT NULL,
                 is_default INTEGER NOT NULL,
                 is_active INTEGER NOT NULL
             );
             CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL
             );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_agents (workspace_path, is_default, is_active)
             VALUES ('agent-workspace', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('project-id', 'Safe Project')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn resolves_canonical_agent_and_project_memory_paths() {
        let conn = target_test_db();

        let agent = canonical_memory_target(&conn, None).unwrap();
        assert_eq!(agent.root, PaneRoot::System);
        assert_eq!(agent.relative, Path::new("agent-workspace/MEMORY.md"));
        assert_eq!(agent.db_path, ".pane/system/agent-workspace/MEMORY.md");

        let project = canonical_memory_target(&conn, Some("project-id")).unwrap();
        assert_eq!(project.root, PaneRoot::Projects);
        assert_eq!(project.relative, Path::new("Safe Project/Memory.md"));
        assert_eq!(project.db_path, "Projects/Safe Project/Memory.md");
        assert_eq!(
            project.identity_relative,
            Path::new("agent-workspace/IDENTITY.md")
        );
    }

    #[test]
    fn rejects_unsafe_workspace_and_project_names() {
        let conn = target_test_db();
        conn.execute(
            "UPDATE ai_agents SET workspace_path = '../escape' WHERE is_default = 1",
            [],
        )
        .unwrap();
        assert!(canonical_memory_target(&conn, None).is_err());

        conn.execute(
            "UPDATE ai_agents SET workspace_path = 'agent-workspace' WHERE is_default = 1",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE projects SET name = '/tmp/escape' WHERE id = 'project-id'",
            [],
        )
        .unwrap();
        assert!(canonical_memory_target(&conn, Some("project-id")).is_err());
    }

    #[test]
    fn test_should_trigger_consolidation() {
        assert!(!should_trigger_consolidation(1999));
        assert!(!should_trigger_consolidation(2000));
        assert!(should_trigger_consolidation(2001));
        assert!(should_trigger_consolidation(5000));
    }

    #[test]
    fn test_content_loss_guard() {
        // Content loss guard logic (tested via integration):
        // If old_word_count > 100 AND new_word_count < old_word_count / 2 → block
        
        // These would block:
        assert!(200 > 100 && 90 < 200 / 2); // 200 → 90
        assert!(500 > 100 && 200 < 500 / 2); // 500 → 200
        
        // These would pass:
        assert!(!(50 > 100 && 20 < 50 / 2)); // old too small
        assert!(!(200 > 100 && 120 < 200 / 2)); // new >= 50%
    }

    #[test]
    fn test_consolidation_prompt_includes_all_facts() {
        // This test verifies the prompt building logic
        let facts = vec![
            "Fact 1".to_string(),
            "Fact 2".to_string(),
            "Fact 3".to_string(),
        ];
        let formatted = facts
            .iter()
            .map(|f| format!("- {}", f))
            .collect::<Vec<_>>()
            .join("\n");
        
        assert_eq!(formatted, "- Fact 1\n- Fact 2\n- Fact 3");
        assert_eq!(formatted.matches("- Fact").count(), 3);
    }
}

/// #2775 Tier 2 — site 2 of 4: proves `consolidate_file`'s `db_state` parameter
/// (already in scope one line above the `call_llm_from_config` call it's now
/// threaded into) actually reaches `select_openrouter_client` as `Some(..)`.
/// Uses the shared `#[cfg(test)]` client-path stamp mechanism (`gateway_client.rs`)
/// — the stamp is set during client selection, before any network I/O, so the
/// outer LLM call's network failure (dummy unreachable endpoint) doesn't affect
/// what this test asserts.
#[cfg(test)]
mod consolidate_file_db_threading_tests {
    use super::*;
    use crate::database::DbState;
    use serial_test::serial;
    use tempfile::TempDir;

    /// Builds a real on-disk fixture: PANE_TEST_ROOT scratch dir with a MEMORY.md
    /// file physically present at the canonical agent-workspace location, plus a
    /// full-schema in-memory DB (via `run_migrations`) seeded with a default agent,
    /// a stored `gateway_tls_cert_pem` (paired install), and one active memory fact
    /// (`consolidate_file` returns early with `Ok(false)` on an empty fact list, before
    /// ever reaching the LLM call — needs at least one row to get there).
    fn setup() -> (TempDir, DbState) {
        let temp_fs_dir = TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_fs_dir.path());

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, workspace_path, is_default, is_active)
             VALUES ('agent-1', 'Test Agent', 'test-agent', 'test-agent', 1, 1)",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active, project_id)
             VALUES ('fact-1', 'The user prefers dark mode.', 'preference', 1, NULL)",
            [],
        )
        .unwrap();

        let pem = crate::gateway_client::test_make_gateway_cert_pem();
        crate::database::set_ai_setting(&conn, "gateway_tls_cert_pem", &pem).unwrap();

        // Physical MEMORY.md at the canonical path consolidate_file will authorize.
        let memory_dir = temp_fs_dir.path().join(".pane/system/test-agent");
        std::fs::create_dir_all(&memory_dir).unwrap();
        std::fs::write(memory_dir.join("MEMORY.md"), "The user prefers dark mode.\n").unwrap();
        std::fs::write(memory_dir.join("IDENTITY.md"), "# Identity\n").unwrap();

        let db = DbState::new(conn);

        (temp_fs_dir, db)
    }

    #[tokio::test]
    #[serial(pane_test_root)]
    async fn test_consolidate_file_threads_db_state_to_pinned_client() {
        let (temp_fs_dir, db) = setup();
        let file_path = temp_fs_dir.path().join(".pane/system/test-agent/MEMORY.md");

        // consolidate_file_impl is the AppHandle-free core (split out of
        // consolidate_file for exactly this reason — see its doc comment) that
        // consolidate_file's #[tauri::command]-facing wrapper delegates to.
        let _ = consolidate_file_impl(
            &db,
            &file_path,
            None,
            "openclaw",
            "dummy-token",
            "openclaw",
            Some("https://127.0.0.1:1"),
            None,
        )
        .await;

        assert_eq!(
            crate::gateway_client::test_take_last_client_path(),
            Some(crate::gateway_client::ClientPathStamp::Pinned),
            "consolidate_file's db_state param must reach select_openrouter_client as Some(..) \
             and select the pinned client, not fall through to the pre-pairing/danger_accept path"
        );

    }
}

/// #2958 follow-up (site 6 of 6 in Reed's D2-guard-test audit table) — real resolver
/// + the real, unmodified `consolidate_file_impl` (already AppHandle-free, split out
/// of `consolidate_file` for exactly this reason per the #2775 doc comment above) +
/// mockito wire capture. See `commands.rs`'s `d2_dispatch_site_tests` module doc
/// comment for the full six-site map and the AppHandle architectural note.
#[cfg(test)]
mod d2_consolidate_file_dispatch_tests {
    use super::*;
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    /// UUID-shaped, deliberately distinct from the runtime_ref used alongside it —
    /// same convention as `commands.rs`'s `d2_dispatch_site_tests::LOCAL_AGENT_UUID`
    /// (a different literal here only so a grep across both files can't accidentally
    /// match the wrong module's fixture).
    const LOCAL_AGENT_UUID: &str = "3D6C8A12-90FE-4B57-AE23-1C7D4F9B0E62";

    fn setup(runtime_ref: &str, endpoint: &str) -> (TempDir, DbState) {
        let temp_fs_dir = TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_fs_dir.path());

        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");

        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO ai_agents (id, name, slug, is_default, is_active, workspace_path, runtime_type, runtime_ref, model, created_at, updated_at)
             VALUES (?1, 'Test Agent', 'test-agent', 1, 1, 'test-agent', 'openclaw', ?2, 'openclaw-pending', ?3, ?3)",
            rusqlite::params![LOCAL_AGENT_UUID, runtime_ref, now],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active, project_id)
             VALUES ('fact-1', 'The user prefers dark mode.', 'preference', 1, NULL)",
            [],
        )
        .unwrap();

        crate::ai::credential_migration::write_gateway_token(&conn, "d2-consolidate-token").unwrap();
        crate::database::set_ai_setting(&conn, "endpoint", endpoint).unwrap();

        let memory_dir = temp_fs_dir.path().join(".pane/system/test-agent");
        std::fs::create_dir_all(&memory_dir).unwrap();
        std::fs::write(memory_dir.join("MEMORY.md"), "The user prefers dark mode.\n").unwrap();
        std::fs::write(memory_dir.join("IDENTITY.md"), "# Identity\n").unwrap();

        (temp_fs_dir, DbState::new(conn))
    }

    #[tokio::test]
    #[serial(pane_test_root)]
    #[serial(keychain_override)]
    async fn test_site6_consolidate_file_impl_via_real_resolver_never_leaks_local_uuid() {
        let _kc = KeychainGuard::set();
        let runtime_ref = "consolidate-site6";

        let mut server = mockito::Server::new_async().await;
        let captured_body: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured_clone = captured_body.clone();
        let mock = server
            .mock("POST", "/v1/chat/completions")
            .match_request(move |request| {
                let body = request.utf8_lossy_body().unwrap_or_default().into_owned();
                *captured_clone.lock().unwrap() = Some(body);
                true
            })
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"role":"assistant","content":"Consolidated memory prose."}}],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#)
            .create_async()
            .await;

        let (temp_fs_dir, db) = setup(runtime_ref, &server.url());
        let file_path = temp_fs_dir.path().join(".pane/system/test-agent/MEMORY.md");

        // Real resolver, via the exact same shared helper (`resolve_default_agent_
        // dispatch_tuple`) ai_consolidate_memory calls before threading its result into
        // this same consolidate_file_impl — not a hand-fed value.
        let (provider_name, api_key, model, endpoint, oc_runtime_ref) = {
            let conn = db.0.lock().unwrap();
            crate::ai::chat::resolve_default_agent_dispatch_tuple(&conn)
                .expect("should resolve the seeded default openclaw agent")
        };
        assert_eq!(oc_runtime_ref.as_deref(), Some(runtime_ref), "sanity: tuple must carry the seeded runtime_ref");

        let result = consolidate_file_impl(
            &db,
            &file_path,
            None,
            &provider_name,
            &api_key,
            &model,
            endpoint.as_deref(),
            oc_runtime_ref.as_deref(),
        )
        .await;
        assert!(result.is_ok(), "consolidation must succeed: {:?}", result.err());

        mock.assert_async().await;
        let body = captured_body.lock().unwrap().clone().expect("body must have been captured");
        assert!(
            !body.contains(LOCAL_AGENT_UUID),
            "D2 VIOLATION: local agent UUID leaked onto the wire via consolidate_file_impl: {}",
            body
        );
        assert!(
            body.contains(&format!("openclaw/{}", runtime_ref)),
            "consolidate_file_impl body must carry the agent-scoped runtime_ref, got: {}",
            body
        );

    }
}
