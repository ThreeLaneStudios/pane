// Memory Router — §5.5.1
// Deterministic routing of unrouted facts to memory files (no LLM)

use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::Emitter;

/// Global router mutex — ensures only one router run at a time
static ROUTER_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

/// Per-file mutexes — ensures atomic writes to each memory file.
/// Pub so consolidator (§5.5.3) shares the same locks.
static FILE_MUTEXES: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

fn get_router_mutex() -> &'static Mutex<()> {
    ROUTER_MUTEX.get_or_init(|| Mutex::new(()))
}

/// Acquire a per-file mutex for memory file writes.
/// Shared between router (§5.5.1) and consolidator (§5.5.3) to prevent
/// concurrent writes to the same memory file.
pub fn get_file_mutex(file_path: &str) -> Arc<Mutex<()>> {
    let mutexes = FILE_MUTEXES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = mutexes.lock().unwrap();
    map.entry(file_path.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

#[derive(Debug, Clone)]
pub(crate) struct UnroutedFact {
    pub id: String,
    pub category: String,
    pub project_id: Option<String>,
    pub fact: String,
}

#[derive(Debug, Clone, PartialEq)]
enum DestinationType {
    AgentMemory,
    ProjectMemory(String), // project_id
    DailyLogOnly,
}

/// Route a fact to its destination based on category and project_id (§5.5.1 routing table)
fn determine_destination(category: &str, project_id: &Option<String>) -> DestinationType {
    // Normalize: "workspace" is not a real project ID — treat as no project.
    // Facts from home-screen conversations carry project_id = "workspace" by default.
    // #3167 Commit 5: moved onto the shared `tools::normalize_home_sentinel`
    // predicate (Reed's #3168 delta re-audit found this inline comparison
    // duplicated at three OTHER sites and named it as the risk a fourth
    // copy would appear) — this was the FIRST of the pre-existing copies,
    // same logic, no behavior change.
    let effective_pid = project_id
        .as_deref()
        .and_then(crate::ai::tools::normalize_home_sentinel)
        .map(|pid| pid.to_string());

    match (category, &effective_pid) {
        // identity/preference/style → always agent MEMORY.md
        ("identity", _) | ("preference", _) | ("style", _) => DestinationType::AgentMemory,

        // goal → agent if no project, project if has project
        ("goal", None) => DestinationType::AgentMemory,
        ("goal", Some(pid)) => DestinationType::ProjectMemory(pid.clone()),

        // fact/decision → project if has project, agent MEMORY.md if no project
        // (workspace-scoped facts have no project context so they route to agent memory)
        ("fact", Some(pid)) | ("decision", Some(pid)) => {
            DestinationType::ProjectMemory(pid.clone())
        }
        ("fact", None) | ("decision", None) => DestinationType::AgentMemory,

        // deadline/reflection → daily log only
        ("deadline", _) | ("reflection", _) => DestinationType::DailyLogOnly,

        // unknown category → daily log only (defensive)
        _ => {
            eprintln!(
                "[ROUTER] Unknown category: {} — routing to daily log only",
                category
            );
            DestinationType::DailyLogOnly
        }
    }
}

#[derive(Debug, Clone)]
struct MemoryFile {
    root: PaneRoot,
    relative: PathBuf,
}

fn policy_error(context: &str, error: impl std::fmt::Display) -> String {
    format!("{}: {}", context, error)
}

/// Get the authorized relative path for agent MEMORY.md
fn get_agent_memory_path(conn: &Connection) -> Result<MemoryFile, String> {
    // Query main agent workspace_path from ai_agents
    let workspace_path: String = conn
        .query_row(
            "SELECT workspace_path FROM ai_agents WHERE is_default = 1 AND is_active = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to query main agent: {}", e))?
        .ok_or_else(|| "No main agent found".to_string())?;
    PaneFs::validate_name(&workspace_path)
        .map_err(|e| policy_error("Unsafe agent workspace path in database", e))?;

    let relative = PaneFs::relative_from_names([workspace_path.as_str(), "MEMORY.md"])
        .map_err(|e| policy_error("Unsafe agent memory path", e))?;

    Ok(MemoryFile {
        root: PaneRoot::System,
        relative,
    })
}

/// Get the authorized relative path for a project's Memory note
fn get_project_memory_path(
    conn: &Connection,
    project_id: &str,
) -> Result<Option<MemoryFile>, String> {
    // Query project name (projects table has no deleted_at column)
    let project_name: Option<String> = conn
        .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to query project: {}", e))?;

    if let Some(name) = project_name {
        PaneFs::validate_name(&name)
            .map_err(|e| policy_error("Unsafe project name in database", e))?;
        let relative = PaneFs::relative_from_names([name.as_str(), "Memory.md"])
            .map_err(|e| policy_error("Unsafe project memory path", e))?;
        Ok(Some(MemoryFile {
            root: PaneRoot::Projects,
            relative,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
fn memory_file_from_absolute(file_path: &Path) -> Result<MemoryFile, String> {
    // #3008: get_pane_root() now resolves PANE_TEST_ROOT to its real
    // (symlink-free) path, but `file_path` here is whatever the TEST built
    // — typically joined directly onto the RAW `PANE_TEST_ROOT` value (the
    // established convention elsewhere in this codebase), which no longer
    // shares a literal prefix with the now-canonicalized `pane_root` below.
    // Rewrite file_path's ROOT portion (if it was built from the raw
    // PANE_TEST_ROOT) onto the canonical root, but leave everything AFTER
    // that root untouched — deliberately NOT a blanket `fs::canonicalize`
    // of the whole path, which would also silently resolve away an
    // intentional symlink placed deeper in the tree (e.g.
    // `test_router_rejects_symlinked_memory_path`'s `Projects/Linked
    // Project` symlink), hiding exactly the kind of escape PaneFs's own
    // authorize_* calls further down this call chain exist to catch.
    let file_path_owned = std::env::var("PANE_TEST_ROOT")
        .ok()
        .map(std::path::PathBuf::from)
        .and_then(|raw_root| file_path.strip_prefix(&raw_root).ok().map(|rel| (raw_root, rel.to_path_buf())))
        .map(|(_, relative)| crate::database::get_pane_root().join(relative))
        .unwrap_or_else(|| file_path.to_path_buf());
    let file_path: &Path = &file_path_owned;

    let pane_root = crate::database::get_pane_root();
    let roots = [
        (PaneRoot::System, pane_root.join(".pane").join("system")),
        (PaneRoot::Projects, pane_root.join("Projects")),
    ];

    for (root, root_path) in roots {
        if let Ok(relative) = file_path.strip_prefix(&root_path) {
            PaneFs::validate_relative(relative)
                .map_err(|e| policy_error("Unsafe memory file path", e))?;
            return Ok(MemoryFile {
                root,
                relative: relative.to_path_buf(),
            });
        }
    }

    Err(format!(
        "Memory file is outside authorized roots: {}",
        file_path.display()
    ))
}

/// Check if exact bullet text already exists in content (idempotent append)
fn bullet_exists(content: &str, bullet_text: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        // Match bullets starting with -, *, or •
        if let Some(stripped) = trimmed
            .strip_prefix('-')
            .or_else(|| trimmed.strip_prefix('*'))
            .or_else(|| trimmed.strip_prefix('•'))
        {
            stripped.trim() == bullet_text.trim()
        } else {
            false
        }
    })
}

/// Append bullet to content if not already present
fn append_bullet_if_new(content: &str, bullet_text: &str) -> (String, bool) {
    if bullet_exists(content, bullet_text) {
        (content.to_string(), false) // No change, bullet already exists
    } else {
        let new_content = if content.trim().is_empty() {
            format!("- {}\n", bullet_text)
        } else if content.ends_with('\n') {
            format!("{}- {}\n", content, bullet_text)
        } else {
            format!("{}\n- {}\n", content, bullet_text)
        };
        (new_content, true) // Content changed, bullet added
    }
}

/// Remove exact bullet text from content
fn remove_bullet(content: &str, bullet_text: &str) -> (String, bool) {
    let mut modified = false;
    let new_content = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if let Some(stripped) = trimmed
                .strip_prefix('-')
                .or_else(|| trimmed.strip_prefix('*'))
                .or_else(|| trimmed.strip_prefix('•'))
            {
                if stripped.trim() == bullet_text.trim() {
                    modified = true;
                    return false; // Filter out this line
                }
            }
            true // Keep this line
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Preserve trailing newline if original had one
    let final_content = if content.ends_with('\n') && !new_content.ends_with('\n') {
        format!("{}\n", new_content)
    } else {
        new_content
    };

    (final_content, modified)
}

/// Count words in text
pub(crate) fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Route unrouted facts to appropriate memory files
pub fn route_memory_facts(conn: &Connection, app_handle: tauri::AppHandle) -> Result<(), String> {
    eprintln!("[ROUTER] Starting memory routing...");

    // Acquire router mutex to serialize router runs
    let _router_guard = get_router_mutex().lock().unwrap();

    // Query all unrouted facts
    let mut stmt = conn.prepare(
        "SELECT id, category, project_id, fact FROM ai_memory WHERE routed_at IS NULL AND is_active = 1"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;

    let facts: Vec<UnroutedFact> = stmt
        .query_map([], |row| {
            Ok(UnroutedFact {
                id: row.get(0)?,
                category: row.get(1)?,
                project_id: row.get(2)?,
                fact: row.get(3)?,
            })
        })
        .map_err(|e| format!("Failed to query facts: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect facts: {}", e))?;

    if facts.is_empty() {
        eprintln!("[ROUTER] No unrouted facts to route");
        return Ok(());
    }

    eprintln!("[ROUTER] Found {} unrouted fact(s) to route", facts.len());

    // Group facts by destination
    let mut agent_facts: Vec<UnroutedFact> = Vec::new();
    let mut project_facts: HashMap<String, Vec<UnroutedFact>> = HashMap::new();
    let mut daily_only_facts: Vec<UnroutedFact> = Vec::new();

    for fact in facts {
        match determine_destination(&fact.category, &fact.project_id) {
            DestinationType::AgentMemory => agent_facts.push(fact),
            DestinationType::ProjectMemory(pid) => {
                project_facts.entry(pid).or_insert_with(Vec::new).push(fact);
            }
            DestinationType::DailyLogOnly => daily_only_facts.push(fact),
        }
    }

    let pane_fs = PaneFs::for_runtime().map_err(|e| policy_error("Filesystem policy", e))?;

    // Track modified files for word count check
    let mut modified_files: Vec<MemoryFile> = Vec::new();

    // Route agent facts to MEMORY.md
    if !agent_facts.is_empty() {
        eprintln!(
            "[ROUTER] Routing {} fact(s) to agent MEMORY.md",
            agent_facts.len()
        );
        let memory_path = get_agent_memory_path(conn)?;

        if let Err(e) = route_facts_to_memory_file(conn, &pane_fs, &memory_path, &agent_facts) {
            eprintln!("[ROUTER] Failed to route agent facts: {}", e);
        } else {
            modified_files.push(memory_path);
        }
    }

    // Route project facts to project Memory notes
    for (project_id, facts) in project_facts {
        eprintln!(
            "[ROUTER] Routing {} fact(s) to project {}",
            facts.len(),
            project_id
        );

        match get_project_memory_path(conn, &project_id) {
            Ok(Some(memory_path)) => {
                if let Err(e) = route_facts_to_memory_file(conn, &pane_fs, &memory_path, &facts) {
                    eprintln!(
                        "[ROUTER] Failed to route project {} facts: {}",
                        project_id, e
                    );
                } else {
                    modified_files.push(memory_path);
                }
            }
            Ok(None) => {
                eprintln!("[ROUTER] Project {} Memory note not found — marking facts as routed but skipping file write", project_id);
                // Mark as routed even though file doesn't exist (defensive — avoid repeated routing attempts)
                for fact in &facts {
                    let _ = mark_fact_routed(conn, &fact.id);
                }
            }
            Err(e) => {
                eprintln!(
                    "[ROUTER] Failed to get project {} memory path: {}",
                    project_id, e
                );
            }
        }
    }

    // Mark daily-only facts as routed (no file write)
    if !daily_only_facts.is_empty() {
        eprintln!(
            "[ROUTER] Marking {} daily-log-only fact(s) as routed",
            daily_only_facts.len()
        );
        for fact in daily_only_facts {
            if let Err(e) = mark_fact_routed(conn, &fact.id) {
                eprintln!("[ROUTER] Failed to mark fact {} as routed: {}", fact.id, e);
            }
        }
    }

    // Check word counts and trigger consolidator if needed
    for memory_file in modified_files {
        if let Ok(file_path) =
            pane_fs.authorize_existing(memory_file.root, &memory_file.relative, ExistingKind::File)
        {
            if let Ok(content) = std::fs::read_to_string(file_path.as_path()) {
                let word_count = count_words(&content);
                eprintln!(
                    "[ROUTER] File {} has {} words",
                    file_path.as_path().display(),
                    word_count
                );

                if word_count > 2000 {
                    eprintln!("[ROUTER] File exceeds 2000 words — triggering consolidator");
                    // Emit consolidator trigger event
                    if let Err(e) = app_handle.emit("trigger-consolidator", ()) {
                        eprintln!("[ROUTER] Failed to emit consolidator trigger: {}", e);
                    }
                    // Note: Only trigger once per router run, not per file
                    break;
                }
            }
        }
    }

    eprintln!("[ROUTER] Routing complete");
    Ok(())
}

/// Route facts to a specific file (with per-file mutex)
#[cfg(test)]
pub(crate) fn route_facts_to_file(
    conn: &Connection,
    file_path: &PathBuf,
    facts: &[UnroutedFact],
) -> Result<(), String> {
    let pane_fs = PaneFs::for_runtime().map_err(|e| policy_error("Filesystem policy", e))?;
    let memory_file = memory_file_from_absolute(file_path)?;
    route_facts_to_memory_file(conn, &pane_fs, &memory_file, facts)
}

fn route_facts_to_memory_file(
    conn: &Connection,
    pane_fs: &PaneFs,
    memory_file: &MemoryFile,
    facts: &[UnroutedFact],
) -> Result<(), String> {
    let mut file_path = pane_fs
        .authorize_write(memory_file.root, &memory_file.relative)
        .map_err(|e| policy_error("Unsafe memory file destination", e))?;

    if !file_path.as_path().exists() {
        // Create parent directory if it doesn't exist
        let parent = memory_file
            .relative
            .parent()
            .ok_or_else(|| "Memory file path has no parent".to_string())?;
        pane_fs
            .create_dir_all(memory_file.root, parent)
            .map_err(|e| policy_error("Failed to create memory directory", e))?;
        file_path = pane_fs
            .authorize_write(memory_file.root, &memory_file.relative)
            .map_err(|e| policy_error("Unsafe memory file destination", e))?;
    }

    // Acquire per-file mutex
    let file_path_str = file_path.as_path().to_string_lossy().to_string();
    let file_mutex = get_file_mutex(&file_path_str);
    let _file_guard = file_mutex.lock().unwrap();

    // Read current content
    let current_content = if file_path.as_path().exists() {
        let readable = pane_fs
            .authorize_existing(memory_file.root, &memory_file.relative, ExistingKind::File)
            .map_err(|e| policy_error("Unsafe memory file read", e))?;
        std::fs::read_to_string(readable.as_path())
            .map_err(|e| format!("Failed to read {}: {}", readable.as_path().display(), e))?
    } else {
        String::new()
    };

    // Append facts that don't already exist
    let mut new_content = current_content.clone();
    let mut facts_added = 0;

    for fact in facts {
        let (updated_content, changed) = append_bullet_if_new(&new_content, &fact.fact);
        if changed {
            new_content = updated_content;
            facts_added += 1;
            eprintln!("[ROUTER] Added bullet: {}", fact.fact);
        } else {
            eprintln!("[ROUTER] Bullet already exists (skipped): {}", fact.fact);
        }
    }

    // Write to file if changes were made (Local: Pane-originated memory update)
    if facts_added > 0 {
        crate::file_ops::atomic_write(
            file_path.as_path(),
            &new_content,
            Some(conn),
            crate::file_ops::WriteOrigin::Local,
        )
        .map_err(|e| format!("Failed to write {}: {}", file_path.as_path().display(), e))?;
        eprintln!(
            "[ROUTER] Wrote {} new fact(s) to {}",
            facts_added,
            file_path.as_path().display()
        );
    }

    // Mark all facts as routed (even if they already existed)
    for fact in facts {
        mark_fact_routed(conn, &fact.id)?;
    }

    Ok(())
}

/// Mark a fact as routed
pub(crate) fn mark_fact_routed(conn: &Connection, fact_id: &str) -> Result<(), String> {
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "UPDATE ai_memory SET routed_at = ?1 WHERE id = ?2",
        params![now, fact_id],
    )
    .map_err(|e| format!("Failed to mark fact as routed: {}", e))?;
    Ok(())
}

/// Remove bullets for inactive facts (called when is_active = 0)
pub fn remove_inactive_facts(conn: &Connection) -> Result<(), String> {
    eprintln!("[ROUTER] Checking for inactive facts to remove from files...");

    // Acquire router mutex
    let _router_guard = get_router_mutex().lock().unwrap();

    // Query inactive facts that were previously routed
    let mut stmt = conn.prepare(
        "SELECT id, category, project_id, fact FROM ai_memory WHERE is_active = 0 AND routed_at IS NOT NULL"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;

    let facts: Vec<UnroutedFact> = stmt
        .query_map([], |row| {
            Ok(UnroutedFact {
                id: row.get(0)?,
                category: row.get(1)?,
                project_id: row.get(2)?,
                fact: row.get(3)?,
            })
        })
        .map_err(|e| format!("Failed to query facts: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect facts: {}", e))?;

    if facts.is_empty() {
        eprintln!("[ROUTER] No inactive facts to remove");
        return Ok(());
    }

    eprintln!("[ROUTER] Found {} inactive fact(s) to remove", facts.len());

    // Group facts by destination
    let mut agent_facts: Vec<UnroutedFact> = Vec::new();
    let mut project_facts: HashMap<String, Vec<UnroutedFact>> = HashMap::new();

    for fact in facts {
        match determine_destination(&fact.category, &fact.project_id) {
            DestinationType::AgentMemory => agent_facts.push(fact),
            DestinationType::ProjectMemory(pid) => {
                project_facts.entry(pid).or_insert_with(Vec::new).push(fact);
            }
            DestinationType::DailyLogOnly => {
                // Daily log only facts don't have file entries, skip
            }
        }
    }

    let pane_fs = PaneFs::for_runtime().map_err(|e| policy_error("Filesystem policy", e))?;

    // Remove agent facts from MEMORY.md
    if !agent_facts.is_empty() {
        eprintln!(
            "[ROUTER] Removing {} fact(s) from agent MEMORY.md",
            agent_facts.len()
        );
        let memory_path = get_agent_memory_path(conn)?;

        if let Err(e) = remove_facts_from_file(conn, &pane_fs, &memory_path, &agent_facts) {
            eprintln!("[ROUTER] Failed to remove agent facts: {}", e);
        }
    }

    // Remove project facts from project Memory notes
    for (project_id, facts) in project_facts {
        eprintln!(
            "[ROUTER] Removing {} fact(s) from project {}",
            facts.len(),
            project_id
        );

        match get_project_memory_path(conn, &project_id) {
            Ok(Some(memory_path)) => {
                if let Err(e) = remove_facts_from_file(conn, &pane_fs, &memory_path, &facts) {
                    eprintln!(
                        "[ROUTER] Failed to remove project {} facts: {}",
                        project_id, e
                    );
                }
            }
            Ok(None) => {
                eprintln!(
                    "[ROUTER] Project {} Memory note not found — skipping removal",
                    project_id
                );
            }
            Err(e) => {
                eprintln!(
                    "[ROUTER] Failed to get project {} memory path: {}",
                    project_id, e
                );
            }
        }
    }

    eprintln!("[ROUTER] Removal complete");
    Ok(())
}

/// Remove facts from a specific file (with per-file mutex)
fn remove_facts_from_file(
    conn: &Connection,
    pane_fs: &PaneFs,
    memory_file: &MemoryFile,
    facts: &[UnroutedFact],
) -> Result<(), String> {
    let destination = pane_fs
        .authorize_write(memory_file.root, &memory_file.relative)
        .map_err(|e| policy_error("Unsafe memory file destination", e))?;
    if !destination.as_path().exists() {
        eprintln!(
            "[ROUTER] File does not exist (skipping removal): {}",
            destination.as_path().display()
        );
        return Ok(());
    }

    let file_path = pane_fs
        .authorize_existing(memory_file.root, &memory_file.relative, ExistingKind::File)
        .map_err(|e| policy_error("Unsafe memory file read", e))?;

    // Acquire per-file mutex
    let file_path_str = file_path.as_path().to_string_lossy().to_string();
    let file_mutex = get_file_mutex(&file_path_str);
    let _file_guard = file_mutex.lock().unwrap();

    // Read current content
    let current_content = std::fs::read_to_string(file_path.as_path())
        .map_err(|e| format!("Failed to read {}: {}", file_path.as_path().display(), e))?;

    // Remove facts
    let mut new_content = current_content.clone();
    let mut facts_removed = 0;

    for fact in facts {
        let (updated_content, changed) = remove_bullet(&new_content, &fact.fact);
        if changed {
            new_content = updated_content;
            facts_removed += 1;
            eprintln!("[ROUTER] Removed bullet: {}", fact.fact);
        } else {
            eprintln!("[ROUTER] Bullet not found (already removed): {}", fact.fact);
        }
    }

    // Write to file if changes were made (Local: Pane-originated memory removal)
    if facts_removed > 0 {
        crate::file_ops::atomic_write(
            file_path.as_path(),
            &new_content,
            Some(conn),
            crate::file_ops::WriteOrigin::Local,
        )
        .map_err(|e| format!("Failed to write {}: {}", file_path.as_path().display(), e))?;
        eprintln!(
            "[ROUTER] Removed {} fact(s) from {}",
            facts_removed,
            file_path.as_path().display()
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::LazyLock;
    use std::sync::Mutex;
    use tempfile::TempDir;

    /// Tests that use PANE_TEST_ROOT env var must serialize to avoid race conditions.
    /// Rust runs tests in parallel; set_var is process-global.
    static ENV_MUTEX: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

    fn set_test_root(path: &Path) {
        // Tests serialize access through ENV_MUTEX, so mutating the process environment is safe here.
        unsafe { std::env::set_var("PANE_TEST_ROOT", path) };
    }

    #[test]
    fn test_determine_destination() {
        // identity/preference/style → always agent
        assert_eq!(
            determine_destination("identity", &None),
            DestinationType::AgentMemory
        );
        assert_eq!(
            determine_destination("identity", &Some("proj1".to_string())),
            DestinationType::AgentMemory
        );
        assert_eq!(
            determine_destination("preference", &None),
            DestinationType::AgentMemory
        );
        assert_eq!(
            determine_destination("style", &Some("proj1".to_string())),
            DestinationType::AgentMemory
        );

        // goal → agent if no project, project if has project
        assert_eq!(
            determine_destination("goal", &None),
            DestinationType::AgentMemory
        );
        assert_eq!(
            determine_destination("goal", &Some("proj1".to_string())),
            DestinationType::ProjectMemory("proj1".to_string())
        );

        // fact/decision → project if has project, agent MEMORY.md if no project
        assert_eq!(
            determine_destination("fact", &Some("proj1".to_string())),
            DestinationType::ProjectMemory("proj1".to_string())
        );
        assert_eq!(
            determine_destination("fact", &None),
            DestinationType::AgentMemory
        );
        assert_eq!(
            determine_destination("decision", &Some("proj2".to_string())),
            DestinationType::ProjectMemory("proj2".to_string())
        );
        assert_eq!(
            determine_destination("decision", &None),
            DestinationType::AgentMemory
        );

        // "workspace" project_id normalizes to None → routes as no-project
        assert_eq!(
            determine_destination("fact", &Some("workspace".to_string())),
            DestinationType::AgentMemory
        );
        assert_eq!(
            determine_destination("fact", &Some("".to_string())),
            DestinationType::AgentMemory
        );
        assert_eq!(
            determine_destination("decision", &Some("workspace".to_string())),
            DestinationType::AgentMemory
        );
        assert_eq!(
            determine_destination("identity", &Some("workspace".to_string())),
            DestinationType::AgentMemory
        );
        assert_eq!(
            determine_destination("goal", &Some("workspace".to_string())),
            DestinationType::AgentMemory
        );
        // Real UUIDs still route to project memory
        assert_eq!(
            determine_destination("fact", &Some("real-uuid-001".to_string())),
            DestinationType::ProjectMemory("real-uuid-001".to_string())
        );

        // deadline/reflection → daily log only
        assert_eq!(
            determine_destination("deadline", &None),
            DestinationType::DailyLogOnly
        );
        assert_eq!(
            determine_destination("deadline", &Some("proj1".to_string())),
            DestinationType::DailyLogOnly
        );
        assert_eq!(
            determine_destination("reflection", &None),
            DestinationType::DailyLogOnly
        );
        assert_eq!(
            determine_destination("reflection", &Some("proj1".to_string())),
            DestinationType::DailyLogOnly
        );

        // unknown category → daily log only
        assert_eq!(
            determine_destination("unknown", &None),
            DestinationType::DailyLogOnly
        );
    }

    #[test]
    fn test_bullet_exists() {
        let content = "# Memory\n\n- First fact\n- Second fact\n- Third fact\n";

        assert!(bullet_exists(content, "First fact"));
        assert!(bullet_exists(content, "Second fact"));
        assert!(bullet_exists(content, "Third fact"));
        assert!(!bullet_exists(content, "Fourth fact"));

        // Test with different bullet markers
        let content2 = "* Starred fact\n• Bullet point\n- Dash fact\n";
        assert!(bullet_exists(content2, "Starred fact"));
        assert!(bullet_exists(content2, "Bullet point"));
        assert!(bullet_exists(content2, "Dash fact"));
    }

    #[test]
    fn test_append_bullet_if_new() {
        // Empty content
        let (result, changed) = append_bullet_if_new("", "New fact");
        assert!(changed);
        assert_eq!(result, "- New fact\n");

        // Content without trailing newline
        let content = "# Memory\n\n- Existing fact";
        let (result, changed) = append_bullet_if_new(content, "New fact");
        assert!(changed);
        assert_eq!(result, "# Memory\n\n- Existing fact\n- New fact\n");

        // Content with trailing newline
        let content = "# Memory\n\n- Existing fact\n";
        let (result, changed) = append_bullet_if_new(content, "New fact");
        assert!(changed);
        assert_eq!(result, "# Memory\n\n- Existing fact\n- New fact\n");

        // Duplicate fact
        let content = "# Memory\n\n- Existing fact\n";
        let (result, changed) = append_bullet_if_new(content, "Existing fact");
        assert!(!changed);
        assert_eq!(result, content);
    }

    #[test]
    fn test_remove_bullet() {
        let content = "# Memory\n\n- First fact\n- Second fact\n- Third fact\n";

        // Remove middle bullet
        let (result, changed) = remove_bullet(content, "Second fact");
        assert!(changed);
        assert_eq!(result, "# Memory\n\n- First fact\n- Third fact\n");

        // Remove first bullet
        let (result, changed) = remove_bullet(content, "First fact");
        assert!(changed);
        assert_eq!(result, "# Memory\n\n- Second fact\n- Third fact\n");

        // Remove last bullet
        let (result, changed) = remove_bullet(content, "Third fact");
        assert!(changed);
        assert_eq!(result, "# Memory\n\n- First fact\n- Second fact\n");

        // Try to remove non-existent bullet
        let (result, changed) = remove_bullet(content, "Nonexistent fact");
        assert!(!changed);
        assert_eq!(result, content);
    }

    #[test]
    fn test_count_words() {
        assert_eq!(count_words(""), 0);
        assert_eq!(count_words("Hello world"), 2);
        assert_eq!(count_words("This is a test"), 4);
        assert_eq!(count_words("  Multiple   spaces   "), 2);
        // Note: split_whitespace counts everything separated by whitespace, including bullet markers
        assert_eq!(
            count_words("# Heading\n\n- Bullet point\n- Another bullet"),
            8
        );
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_router_idempotent_append() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Integration test: verify router doesn't duplicate bullets
        let temp_dir = TempDir::new().unwrap();
        set_test_root(temp_dir.path());

        // Create test database
        let db_path = temp_dir.path().join(".pane").join("pane.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();

        // Create minimal schema
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                workspace_path TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE ai_memory (
                id TEXT PRIMARY KEY,
                category TEXT NOT NULL,
                project_id TEXT,
                fact TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                routed_at TEXT
            );

            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                deleted_at TEXT
            );
        ",
        )
        .unwrap();

        // Insert test agent
        conn.execute(
            "INSERT INTO ai_agents (id, workspace_path, is_default, is_active) VALUES ('agent1', 'test-agent', 1, 1)",
            [],
        ).unwrap();

        // Create agent MEMORY.md
        let memory_dir = temp_dir
            .path()
            .join(".pane")
            .join("system")
            .join("test-agent");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let memory_path = memory_dir.join("MEMORY.md");
        std::fs::write(&memory_path, "# Memory\n\n- Existing fact\n").unwrap();

        // Insert unrouted facts (one new, one duplicate)
        conn.execute(
            "INSERT INTO ai_memory (id, category, project_id, fact, is_active, routed_at) VALUES ('fact1', 'preference', NULL, 'New preference', 1, NULL)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO ai_memory (id, category, project_id, fact, is_active, routed_at) VALUES ('fact2', 'preference', NULL, 'Existing fact', 1, NULL)",
            [],
        ).unwrap();

        // Test the core routing logic directly (no AppHandle needed)
        let memory_path_test = temp_dir
            .path()
            .join(".pane")
            .join("system")
            .join("test-agent")
            .join("MEMORY.md");
        let facts = vec![
            UnroutedFact {
                id: "fact1".to_string(),
                category: "preference".to_string(),
                project_id: None,
                fact: "New preference".to_string(),
            },
            UnroutedFact {
                id: "fact2".to_string(),
                category: "preference".to_string(),
                project_id: None,
                fact: "Existing fact".to_string(),
            },
        ];

        route_facts_to_file(&conn, &memory_path_test, &facts).unwrap();

        // Verify file content
        let content = std::fs::read_to_string(&memory_path_test).unwrap();
        eprintln!("Final content:\n{}", content);

        // Count bullets
        let bullet_count = content
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                trimmed.starts_with('-') || trimmed.starts_with('*') || trimmed.starts_with('•')
            })
            .count();

        // Should have 2 bullets: "Existing fact" (already there) and "New preference" (newly added)
        assert_eq!(
            bullet_count, 2,
            "Should have exactly 2 bullets (no duplicates)"
        );
        assert!(
            content.contains("- Existing fact"),
            "Should contain existing fact"
        );
        assert!(
            content.contains("- New preference"),
            "Should contain new preference"
        );

        // Verify both facts are marked as routed
        let routed1: Option<String> = conn
            .query_row(
                "SELECT routed_at FROM ai_memory WHERE id = 'fact1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(routed1.is_some(), "fact1 should be marked as routed");

        let routed2: Option<String> = conn
            .query_row(
                "SELECT routed_at FROM ai_memory WHERE id = 'fact2'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            routed2.is_some(),
            "fact2 should be marked as routed even though it was a duplicate"
        );
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_remove_inactive_facts_integration() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Integration test: verify router removes bullets when facts are marked inactive
        let temp_dir = TempDir::new().unwrap();
        set_test_root(temp_dir.path());

        // Create test database
        let db_path = temp_dir.path().join(".pane").join("pane.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();

        // Create minimal schema
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                workspace_path TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE ai_memory (
                id TEXT PRIMARY KEY,
                category TEXT NOT NULL,
                project_id TEXT,
                fact TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                routed_at TEXT
            );
        ",
        )
        .unwrap();

        // Insert test agent
        conn.execute(
            "INSERT INTO ai_agents (id, workspace_path, is_default, is_active) VALUES ('agent1', 'test-agent', 1, 1)",
            [],
        ).unwrap();

        // Create agent MEMORY.md with facts
        let memory_dir = temp_dir
            .path()
            .join(".pane")
            .join("system")
            .join("test-agent");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let memory_path = memory_dir.join("MEMORY.md");
        std::fs::write(
            &memory_path,
            "# Memory\n\n- Keep this fact\n- Remove this fact\n- Keep this too\n",
        )
        .unwrap();

        // Insert facts (one inactive, two active)
        conn.execute(
            "INSERT INTO ai_memory (id, category, project_id, fact, is_active, routed_at) VALUES ('fact1', 'preference', NULL, 'Keep this fact', 1, datetime('now'))",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO ai_memory (id, category, project_id, fact, is_active, routed_at) VALUES ('fact2', 'preference', NULL, 'Remove this fact', 0, datetime('now'))",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO ai_memory (id, category, project_id, fact, is_active, routed_at) VALUES ('fact3', 'preference', NULL, 'Keep this too', 1, datetime('now'))",
            [],
        ).unwrap();

        // Remove inactive facts
        remove_inactive_facts(&conn).unwrap();

        // Verify file content
        let content = std::fs::read_to_string(&memory_path).unwrap();
        eprintln!("Final content after removal:\n{}", content);

        // Should have 2 bullets (removed the inactive one)
        let bullet_count = content
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                trimmed.starts_with('-') || trimmed.starts_with('*') || trimmed.starts_with('•')
            })
            .count();

        assert_eq!(
            bullet_count, 2,
            "Should have exactly 2 bullets after removal"
        );
        assert!(
            content.contains("- Keep this fact"),
            "Should contain first active fact"
        );
        assert!(
            content.contains("- Keep this too"),
            "Should contain second active fact"
        );
        assert!(
            !content.contains("- Remove this fact"),
            "Should NOT contain inactive fact"
        );
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_daily_log_only_routing() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Test that daily-log-only categories are marked as routed but don't touch files
        let temp_dir = TempDir::new().unwrap();
        set_test_root(temp_dir.path());

        // Create test database
        let db_path = temp_dir.path().join(".pane").join("pane.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();

        // Create minimal schema
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                workspace_path TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE ai_memory (
                id TEXT PRIMARY KEY,
                category TEXT NOT NULL,
                project_id TEXT,
                fact TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                routed_at TEXT
            );
        ",
        )
        .unwrap();

        // Insert test agent
        conn.execute(
            "INSERT INTO ai_agents (id, workspace_path, is_default, is_active) VALUES ('agent1', 'test-agent', 1, 1)",
            [],
        ).unwrap();

        // Create agent MEMORY.md (should not be modified)
        let memory_dir = temp_dir
            .path()
            .join(".pane")
            .join("system")
            .join("test-agent");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let memory_path = memory_dir.join("MEMORY.md");
        let original_content = "# Memory\n\n- Existing fact\n";
        std::fs::write(&memory_path, original_content).unwrap();

        // Insert daily-log-only facts (deadline/reflection only — fact/None now routes to AgentMemory)
        conn.execute(
            "INSERT INTO ai_memory (id, category, project_id, fact, is_active, routed_at) VALUES ('fact1', 'deadline', NULL, 'Project deadline next week', 1, NULL)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO ai_memory (id, category, project_id, fact, is_active, routed_at) VALUES ('fact2', 'reflection', NULL, 'User seemed happy today', 1, NULL)",
            [],
        ).unwrap();

        // We can't easily create a real AppHandle in tests, so we'll test the marking logic directly
        // Mark facts as routed manually (simulating what route_memory_facts would do)
        for fact_id in ["fact1", "fact2"] {
            mark_fact_routed(&conn, fact_id).unwrap();
        }

        // Verify facts are marked as routed
        for fact_id in ["fact1", "fact2"] {
            let routed: Option<String> = conn
                .query_row(
                    "SELECT routed_at FROM ai_memory WHERE id = ?1",
                    params![fact_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(routed.is_some(), "{} should be marked as routed", fact_id);
        }

        // Verify MEMORY.md was NOT modified
        let final_content = std::fs::read_to_string(&memory_path).unwrap();
        assert_eq!(
            final_content, original_content,
            "MEMORY.md should not be modified for daily-log-only facts"
        );
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_remove_inactive_facts_for_routed_fact() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // #674: Verify that deleting a routed fact removes its bullet from the memory file
        let temp_dir = TempDir::new().unwrap();
        set_test_root(temp_dir.path());

        // Create test database
        let db_path = temp_dir.path().join(".pane").join("pane.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        // Create minimal schema
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                workspace_path TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE ai_memory (
                id TEXT PRIMARY KEY,
                category TEXT NOT NULL,
                project_id TEXT,
                fact TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                routed_at TEXT
            );
        ",
        )
        .unwrap();

        // Insert test agent
        conn.execute(
            "INSERT INTO ai_agents (id, workspace_path, is_default, is_active) VALUES ('agent1', 'test-agent', 1, 1)",
            [],
        ).unwrap();

        // Create agent MEMORY.md
        let memory_dir = temp_dir
            .path()
            .join(".pane")
            .join("system")
            .join("test-agent");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let memory_path = memory_dir.join("MEMORY.md");
        let initial_content = "# Memory\n\n- This is a routed fact\n- Another fact\n";
        std::fs::write(&memory_path, initial_content).unwrap();

        // Insert a routed fact (using 'preference' category which routes to agent MEMORY.md)
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active, routed_at) VALUES (?1, ?2, ?3, 1, datetime('now'))",
            params!["fact1", "This is a routed fact", "preference"],
        ).unwrap();

        // Now mark it inactive (simulating delete_memory)
        conn.execute(
            "UPDATE ai_memory SET is_active = 0 WHERE id = ?1",
            params!["fact1"],
        )
        .unwrap();

        // Call remove_inactive_facts
        remove_inactive_facts(&conn).unwrap();

        // Verify the bullet was removed from the file
        let final_content = std::fs::read_to_string(&memory_path).unwrap();
        assert!(
            !final_content.contains("This is a routed fact"),
            "Bullet should be removed from memory file"
        );
        assert!(
            final_content.contains("Another fact"),
            "Other bullets should remain"
        );
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_remove_inactive_facts_skips_unrouted() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // #674: Verify that remove_inactive_facts skips facts that were never routed
        let temp_dir = TempDir::new().unwrap();
        set_test_root(temp_dir.path());

        // Create test database
        let db_path = temp_dir.path().join(".pane").join("pane.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        // Create minimal schema
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                workspace_path TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE ai_memory (
                id TEXT PRIMARY KEY,
                category TEXT NOT NULL,
                project_id TEXT,
                fact TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                routed_at TEXT
            );
        ",
        )
        .unwrap();

        // Insert test agent
        conn.execute(
            "INSERT INTO ai_agents (id, workspace_path, is_default, is_active) VALUES ('agent1', 'test-agent', 1, 1)",
            [],
        ).unwrap();

        // Create agent MEMORY.md
        let memory_dir = temp_dir
            .path()
            .join(".pane")
            .join("system")
            .join("test-agent");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let memory_path = memory_dir.join("MEMORY.md");
        let initial_content = "# Memory\n\n- Existing fact\n";
        std::fs::write(&memory_path, initial_content).unwrap();

        // Insert an unrouted fact (routed_at IS NULL)
        conn.execute(
            "INSERT INTO ai_memory (id, fact, category, is_active, routed_at) VALUES (?1, ?2, ?3, 1, NULL)",
            params!["fact1", "Unrouted fact", "preference"],
        ).unwrap();

        // Mark it inactive
        conn.execute(
            "UPDATE ai_memory SET is_active = 0 WHERE id = ?1",
            params!["fact1"],
        )
        .unwrap();

        // Call remove_inactive_facts
        remove_inactive_facts(&conn).unwrap();

        // Verify the file was not modified (since the fact was never routed)
        let final_content = std::fs::read_to_string(&memory_path).unwrap();
        assert_eq!(
            final_content, initial_content,
            "File should not be modified for unrouted facts"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Integration Test: Bug #1238 - Workspace project_id routing
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    #[serial(pane_test_root)]
    fn test_workspace_facts_route_to_agent_memory() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Integration test: facts with project_id = "workspace" must land in agent MEMORY.md
        let temp_dir = TempDir::new().unwrap();
        set_test_root(temp_dir.path());

        let db_path = temp_dir.path().join(".pane").join("pane.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                workspace_path TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1
            );
            CREATE TABLE ai_memory (
                id TEXT PRIMARY KEY,
                category TEXT NOT NULL,
                project_id TEXT,
                fact TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                routed_at TEXT
            );
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL
            );
        ",
        )
        .unwrap();

        conn.execute(
            "INSERT INTO ai_agents (id, workspace_path, is_default, is_active) VALUES ('agent1', 'test-agent', 1, 1)",
            [],
        ).unwrap();

        // Create agent MEMORY.md
        let memory_dir = temp_dir
            .path()
            .join(".pane")
            .join("system")
            .join("test-agent");
        std::fs::create_dir_all(&memory_dir).unwrap();
        let memory_path = memory_dir.join("MEMORY.md");
        std::fs::write(&memory_path, "# Memory\n\n").unwrap();

        // Route facts with project_id = "workspace" directly (simulating route_memory_facts)
        let facts = vec![
            UnroutedFact {
                id: "fact1".to_string(),
                category: "fact".to_string(),
                project_id: Some("workspace".to_string()),
                fact: "User prefers dark mode".to_string(),
            },
            UnroutedFact {
                id: "fact2".to_string(),
                category: "fact".to_string(),
                project_id: Some("".to_string()),
                fact: "User is in EST timezone".to_string(),
            },
        ];

        // Verify both route to AgentMemory
        assert_eq!(
            determine_destination("fact", &Some("workspace".to_string())),
            DestinationType::AgentMemory,
            "workspace project_id should route to AgentMemory"
        );
        assert_eq!(
            determine_destination("fact", &Some("".to_string())),
            DestinationType::AgentMemory,
            "empty project_id should route to AgentMemory"
        );

        // Insert into DB
        for f in &facts {
            conn.execute(
                "INSERT INTO ai_memory (id, category, project_id, fact, is_active, routed_at) VALUES (?1, ?2, ?3, ?4, 1, NULL)",
                params![f.id, f.category, f.project_id, f.fact],
            ).unwrap();
        }

        // Route them to file (using the normalized path via agent memory)
        route_facts_to_file(&conn, &memory_path, &facts).unwrap();

        // Verify facts appear in MEMORY.md
        let content = std::fs::read_to_string(&memory_path).unwrap();
        assert!(
            content.contains("User prefers dark mode"),
            "workspace fact should be in agent MEMORY.md"
        );
        assert!(
            content.contains("User is in EST timezone"),
            "empty-pid fact should be in agent MEMORY.md"
        );

        // Verify both are marked routed
        for f in &facts {
            let routed_at: Option<String> = conn
                .query_row(
                    "SELECT routed_at FROM ai_memory WHERE id = ?1",
                    params![f.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(routed_at.is_some(), "{} should be marked as routed", f.id);
        }
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Integration Test: Bug #796 - Router project-memory write path
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    #[serial(pane_test_root)]
    fn test_router_writes_project_memory() {
        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        // Test that router can write to project Memory.md after #796 fix
        let temp_dir = TempDir::new().unwrap();
        set_test_root(temp_dir.path());

        // Create test database
        let db_path = temp_dir.path().join(".pane").join("pane.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();

        // Create schema with projects table (no deleted_at column)
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                id TEXT PRIMARY KEY,
                workspace_path TEXT NOT NULL,
                is_default INTEGER NOT NULL DEFAULT 0,
                is_active INTEGER NOT NULL DEFAULT 1
            );

            CREATE TABLE ai_memory (
                id TEXT PRIMARY KEY,
                category TEXT NOT NULL,
                project_id TEXT,
                fact TEXT NOT NULL,
                is_active INTEGER NOT NULL DEFAULT 1,
                routed_at TEXT
            );

            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL
            );
        ",
        )
        .unwrap();

        // Insert test project
        let project_id = "test-project-001";
        conn.execute(
            "INSERT INTO projects (id, name) VALUES (?1, ?2)",
            params![project_id, "Test Project"],
        )
        .unwrap();

        // Create project directory and Memory.md
        let projects_path = temp_dir.path().join("Projects");
        let project_dir = projects_path.join("Test Project");
        std::fs::create_dir_all(&project_dir).unwrap();
        let memory_path = project_dir.join("Memory.md");
        std::fs::write(&memory_path, "# Memory — Test Project\n\n").unwrap();

        // Insert unrouted project fact
        conn.execute(
            "INSERT INTO ai_memory (id, category, project_id, fact, is_active, routed_at) VALUES (?1, ?2, ?3, ?4, 1, NULL)",
            params!["fact1", "fact", project_id, "This is a project fact"],
        ).unwrap();

        // Route the fact
        let facts = vec![UnroutedFact {
            id: "fact1".to_string(),
            category: "fact".to_string(),
            project_id: Some(project_id.to_string()),
            fact: "This is a project fact".to_string(),
        }];

        // Get project memory path using the fixed query
        let resolved_path = get_project_memory_path(&conn, project_id).unwrap();
        assert!(
            resolved_path.is_some(),
            "Should resolve project memory path"
        );

        // Route facts to file
        route_facts_to_file(&conn, &memory_path, &facts).unwrap();

        // Verify file contains the fact
        let content = std::fs::read_to_string(&memory_path).unwrap();
        assert!(
            content.contains("This is a project fact"),
            "Memory.md should contain the routed fact"
        );

        // Verify fact is marked as routed
        let routed_at: Option<String> = conn
            .query_row(
                "SELECT routed_at FROM ai_memory WHERE id = ?1",
                params!["fact1"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(routed_at.is_some(), "Fact should be marked as routed");
    }

    #[test]
    fn test_db_derived_memory_names_must_be_single_components() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE ai_agents (
                workspace_path TEXT NOT NULL,
                is_default INTEGER NOT NULL,
                is_active INTEGER NOT NULL
            );
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL
            );
            ",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO ai_agents (workspace_path, is_default, is_active) VALUES ('../escape', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (id, name) VALUES ('project-1', '../escape')",
            [],
        )
        .unwrap();

        assert!(get_agent_memory_path(&conn).is_err());
        assert!(get_project_memory_path(&conn, "project-1").is_err());
    }

    #[cfg(unix)]
    #[test]
    #[serial(pane_test_root)]
    fn test_router_rejects_symlinked_memory_path() {
        use std::os::unix::fs::symlink;

        let _env_guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let temp_dir = TempDir::new().unwrap();
        set_test_root(temp_dir.path());

        let projects_dir = temp_dir.path().join("Projects");
        let outside_dir = temp_dir.path().join("outside");
        std::fs::create_dir_all(&projects_dir).unwrap();
        std::fs::create_dir_all(&outside_dir).unwrap();
        symlink(&outside_dir, projects_dir.join("Linked Project")).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        let memory_path = projects_dir.join("Linked Project").join("Memory.md");
        let error = route_facts_to_file(&conn, &memory_path, &[]).unwrap_err();

        assert!(error.contains("symlink"), "unexpected error: {}", error);
        assert!(!outside_dir.join("Memory.md").exists());
    }
}
