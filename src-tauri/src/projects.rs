// Pane v9.0.0 — Project/Folder CRUD Commands
// Implements project and folder operations (§17.2)

use crate::database::{get_projects_path, DbState, Project};
use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::State;

/// §1.7.1 (#2874 item 50a): the seed-time name for a project's auto-created first
/// folder, and the folderless-project upgrade fallback name (v90→v91 migration,
/// `migrations.rs::migrate_v91`). One constant — every creation site
/// (`create_project_internal` here, the v90→v91 migration backfill, `watcher.rs`,
/// `import.rs`) references this instead of repeating the literal.
pub const DEFAULT_FOLDER_NAME: &str = "General";

/// §1.7.2 (#2874 item 50a): the one shared folder-name-uniqueness check — called
/// from every path that creates, renames, or moves a folder (sidebar
/// `create_folder`/`update_folder`, agent `execute_create_folder`/
/// `execute_move_folder`, `move_folder_internal` itself, and import/watcher's
/// folder-creation branches). Case-insensitive, scoped to siblings under
/// `parent_id`.
///
/// `exclude_id` is the folder's own id for a rename or move (so a folder never
/// collides with itself), or `""` — never a real `projects.id` — for a create,
/// where there is no "own id" yet. CRR tables can't carry a `UNIQUE` index (#493),
/// so this is the app-code enforcement the schema comment on `projects` already
/// calls for.
///
/// Enforces going forward only (§1.7.2): pre-existing duplicate siblings from
/// before this check existed are left as-is, never backfilled/de-duplicated here.
/// The agent-facing "name both colliding folders by ID, retry by ID" error shape
/// is #2874 item 50b (ai-integration.md §4.1) — out of scope for this PR; callers
/// here just propagate this function's `DUPLICATE_FOLDER:` error as-is.
pub(crate) fn check_folder_name_unique(
    conn: &rusqlite::Connection,
    parent_id: &str,
    name: &str,
    exclude_id: &str,
) -> Result<(), String> {
    let collision: Option<String> = conn
        .query_row(
            "SELECT id FROM projects WHERE parent_id = ?1 AND LOWER(name) = LOWER(?2) AND id != ?3 LIMIT 1",
            params![parent_id, name, exclude_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e| format!("Failed to check folder name uniqueness: {}", e))?;

    if let Some(existing_id) = collision {
        return Err(format!(
            "DUPLICATE_FOLDER:{}:A folder named '{}' already exists in this project.",
            existing_id, name
        ));
    }
    Ok(())
}

// === Request Types ===

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
    pub description: Option<String>,
    pub color: Option<String>,
    pub category: Option<String>,
    pub first_folder_name: Option<String>,
    pub is_favorite: Option<bool>,
    /// ADR-0062 D2 amendment (2026-08-17): the agent the project's born-active
    /// first session binds to. Required, not optional — both production callers
    /// (the New Project dialog's explicit picker, and the `create_project` tool's
    /// acting-agent-from-session resolution) always have a value to supply by the
    /// time this struct is constructed; there is no "no agent chosen yet" state
    /// on this path the way there might be transiently in a UI form field.
    pub agent_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateFolderRequest {
    pub project_id: String,
    pub name: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateProjectRequest {
    pub project_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub color: Option<String>,
    /// #2874 item 9: `Some(folder_id)` re-points this project's default
    /// folder (validated by `validate_default_folder_choice` below before
    /// the write); `None` leaves it untouched, matching `name`/`description`/
    /// `color`'s existing "absent = don't touch" convention on this request.
    pub default_folder_id: Option<String>,
}

/// #2874 item 9 (rulings 3-4): validates a candidate `default_folder_id`
/// before `update_project` writes it. The canonical visibility bar is the
/// same one `list_visible_folders` (ai/commands.rs, #2874 item 50b) already
/// established for agent folder resolution: `is_system`/`is_hidden` straight
/// off the `projects` row — no hand-typed system-folder list, no second
/// definition of "system"/"hidden" to drift out of sync with that one.
///
/// Three refusals, each its own branch so the error names which rule fired:
/// - the folder doesn't exist, or exists in a DIFFERENT project (`parent_id`
///   must equal `project_id` exactly — same shape as
///   `check_folder_name_unique`'s sibling-scoping and
///   `resolve_folder_branch_a`'s cross-project ID refusal)
/// - the folder is a system folder
/// - the folder is a hidden folder
pub(crate) fn validate_default_folder_choice(
    conn: &rusqlite::Connection,
    project_id: &str,
    folder_id: &str,
) -> Result<(), String> {
    let row: Option<(Option<String>, i32, i32)> = conn
        .query_row(
            "SELECT parent_id, COALESCE(is_system, 0), COALESCE(is_hidden, 0) FROM projects WHERE id = ?1",
            params![folder_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|e| format!("Failed to look up folder '{folder_id}': {e}"))?;

    let Some((parent_id, is_system, is_hidden)) = row else {
        return Err(format!("Folder '{folder_id}' does not exist."));
    };

    if parent_id.as_deref() != Some(project_id) {
        return Err(format!(
            "Folder '{folder_id}' does not belong to project '{project_id}' — cannot set it as the default folder."
        ));
    }
    if is_system != 0 {
        return Err("Cannot set a system folder as the default folder.".to_string());
    }
    if is_hidden != 0 {
        return Err("Cannot set a hidden folder as the default folder.".to_string());
    }
    Ok(())
}

/// #2874 item 9 ruling 5 / item 65 ruling 4: the one place that answers "is
/// this folder its project's CURRENT default folder" — shared by
/// `delete_folder_internal`'s existing "cannot delete the default folder"
/// guard and `toggle_folder_hidden`'s new "cannot hide the default folder"
/// guard below, so the two guards read the exact same pointer and can never
/// drift apart on what "default" means.
pub(crate) fn project_default_folder_id(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Option<String> {
    conn.query_row(
        "SELECT default_folder_id FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .unwrap_or(None)
}

// === Tauri Commands ===

/// Internal helper function that creates a project with all its folders
/// Used by both the Tauri command and the AI tool executor
pub fn create_project_internal(
    request: &CreateProjectRequest,
    db: &DbState,
) -> Result<Project, String> {
    let conn = db.0.lock().unwrap();
    
    let first_folder_name = request.first_folder_name.as_deref().unwrap_or(DEFAULT_FOLDER_NAME);
    PaneFs::validate_name(&request.name).map_err(|e| format!("Invalid project name: {e}"))?;
    PaneFs::validate_name(first_folder_name)
        .map_err(|e| format!("Invalid first folder name: {e}"))?;
    let fs_policy = PaneFs::for_runtime().map_err(|e| format!("Filesystem policy error: {e}"))?;
    let project_relative = PaneFs::relative_from_names([request.name.as_str()])
        .map_err(|e| format!("Invalid project path: {e}"))?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    
    // Create directory on disk
    fs_policy
        .create_dir_all(PaneRoot::Projects, &project_relative)
        .map_err(|e| format!("Failed to create project directory: {e}"))?;
    
    // Create Assets subdirectory
    let assets_relative = project_relative.join("Assets");
    fs_policy
        .create_dir_all(PaneRoot::Projects, &assets_relative)
        .map_err(|e| format!("Failed to create Assets directory: {e}"))?;
    
    // Insert into database
    conn.execute(
        "INSERT INTO projects (id, name, description, color, category, owner_id, is_favorite, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            &request.name,
            &request.description,
            &request.color,
            &request.category,
            "user_local",
            request.is_favorite.unwrap_or(false) as i32,
            &now,
            &now,
        ],
    ).map_err(|e| format!("Failed to insert project: {}", e))?;
    
    // Auto-create first folder (default "General", customizable)
    let folder_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![folder_id, first_folder_name, &id, "user_local", &now, &now],
    )
    .map_err(|e| format!("Failed to create {} folder: {}", first_folder_name, e))?;

    // §1.7.1: the first folder a project gets is always its default folder,
    // found by ID from here on — never by matching the name "General" again.
    conn.execute(
        "UPDATE projects SET default_folder_id = ?1 WHERE id = ?2",
        params![folder_id, &id],
    )
    .map_err(|e| format!("Failed to set default_folder_id: {}", e))?;

    // Create first folder directory on disk
    let first_folder_relative = project_relative.join(first_folder_name);
    fs_policy
        .create_dir_all(PaneRoot::Projects, &first_folder_relative)
        .map_err(|e| format!("Failed to create {} directory: {e}", first_folder_name))?;
    
    // Auto-create project Memory.md file and notes row (per ADR-0020)
    let memory_relative = project_relative.join("Memory.md");
    let memory_file_path = fs_policy
        .authorize_write(PaneRoot::Projects, &memory_relative)
        .map_err(|e| format!("Failed to authorize Memory.md: {e}"))?;
    let memory_header = format!("# Memory — {}\n", &request.name);
    fs::write(memory_file_path.as_path(), memory_header)
        .map_err(|e| format!("Failed to create Memory.md: {}", e))?;
    
    let memory_note_id = uuid::Uuid::new_v4().to_string();
    let memory_db_path = format!("Projects/{}/Memory.md", &request.name);
    conn.execute(
        "INSERT INTO notes (id, title, content_preview, file_path, project_id, folder_id, note_type, tags_json, owner_id, created_at, updated_at)
         VALUES (?1, 'Memory', '', ?2, ?3, NULL, 'memory', '[]', 'user_local', ?4, ?5)",
        params![memory_note_id, memory_db_path, &id, &now, &now],
    ).map_err(|e| format!("Failed to create Memory note: {}", e))?;
    
    // Born-active hook (§30.2.6): every new project gets an active chat session on creation.
    // Uses the same conn already held — no new lock acquisition needed.
    // ADR-0062 D1/D2 amendment: the born-active session binds to request.agent_id
    // (the New Project dialog's picker selection on the user-initiated path, or
    // the acting agent resolved from the calling session on the tool-initiated path).
    let session_id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO chat_sessions (id, project_id, title, origin, created_at, last_active_at, is_active, agent_id) \
         VALUES (?1, ?2, '', 'user', datetime('now'), datetime('now'), 1, ?3)",
        params![session_id, &id, &request.agent_id],
    ).map_err(|e| format!("Failed to create born-active session: {}", e))?;
    eprintln!(
        "[create_project_internal] Born-active session id={} created for project={}, agent_id={}",
        session_id, id, request.agent_id
    );

    // Return created project
    get_project(&conn, &id)
}

/// #2874 item 4 (docs/audits/2026-10-16-scope-1060-test-failures.md item 4,
/// "New project's General folder missing until 2nd folder exists"):
/// `create_project_internal` creates the project row AND its auto-created
/// default folder row in one transaction — both exist in the DB
/// immediately — but the caller used to emit only ONE `pane-data-changed`
/// event, for the project's own id. The Sidebar's incremental single-entity
/// patch handler (`handleDataChangedEvent`) only learns a row exists when
/// ITS OWN id is announced, so the folder stayed invisible until something
/// else (e.g. creating a second folder) happened to trigger a full reload.
///
/// Shared by BOTH places that create a project (Matt's ruling): the
/// Sidebar's `create_project` Tauri command below, and the AI agent's
/// `create_project` tool (`ai::tools::emit_entity_change_for_tool`'s
/// special case — see that function's doc comment for why `create_project`
/// can't go through the ordinary one-tuple `tool_entity_change` path). One
/// function emits both events, so neither caller can emit the project
/// without its folder, or invent a different entity_type/reason pair for
/// either — §34's existing constants only, per Matt's ruling.
pub fn emit_project_created_events<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    project_id: &str,
    default_folder_id: Option<&str>,
) {
    crate::refresh_event::emit_data_changed(
        app,
        project_id,
        crate::refresh_event::ENTITY_PROJECT,
        crate::refresh_event::REASON_CREATED,
    );
    if let Some(folder_id) = default_folder_id {
        crate::refresh_event::emit_data_changed(
            app,
            folder_id,
            crate::refresh_event::ENTITY_FOLDER,
            crate::refresh_event::REASON_CREATED,
        );
    }
}

#[tauri::command]
pub fn create_project(
    app: tauri::AppHandle,
    request: CreateProjectRequest,
    db: State<DbState>,
) -> Result<Project, String> {
    eprintln!(
        "[create_project] Creating project: name={}, category={:?}, first_folder={:?}",
        request.name, request.category, request.first_folder_name
    );
    let project = create_project_internal(&request, &db)?;
    // #2972 stage 1 (§34.1.1): migrated from `pane-project-changed` to the
    // single entity-identified `pane-data-changed` event. #2874 item 4:
    // now routes through `emit_project_created_events` (above) so the
    // auto-created default folder is announced too, not just the project.
    emit_project_created_events(&app, &project.id, project.default_folder_id.as_deref());
    eprintln!(
        "[create_project] Emitted {} for project={} and default_folder={:?}",
        crate::refresh_event::EVENT_DATA_CHANGED,
        project.id,
        project.default_folder_id
    );
    Ok(project)
}

#[tauri::command]
pub fn get_projects(db: State<DbState>) -> Result<Vec<Project>, String> {
    let conn = db.0.lock().unwrap();
    
    let mut stmt = conn.prepare(
        "SELECT id, name, description, color, parent_id, sort_order, owner_id, created_at, updated_at, category, is_system, is_favorite, default_folder_id
         FROM projects
         ORDER BY sort_order, name"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    let projects = stmt
        .query_map([], |row| {
        Ok(Project {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            color: row.get(3)?,
            parent_id: row.get(4)?,
            sort_order: row.get(5)?,
            owner_id: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            category: row.get(9)?,
            is_system: row.get::<_, i32>(10)? != 0,
            is_favorite: row.get::<_, i32>(11)? != 0,
            default_folder_id: row.get(12)?,
        })
    })
    .map_err(|e| format!("Failed to query projects: {}", e))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("Failed to collect projects: {}", e))?;
    
    Ok(projects)
}

#[tauri::command]
pub fn update_project(
    request: UpdateProjectRequest,
    db: State<DbState>,
) -> Result<Project, String> {
    let conn = db.0.lock().unwrap();
    
    let now = chrono::Utc::now().to_rfc3339();
    
    let mut updates = Vec::new();
    let mut update_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut renamed_project_directory = None;
    
    if let Some(name) = &request.name {
        PaneFs::validate_name(name).map_err(|e| format!("Invalid project name: {e}"))?;
        // Check if this is a top-level project (needs directory rename), and validate the
        // existing DB display name before any filesystem or database mutation.
        let (old_name, parent_id): (String, Option<String>) = conn
            .query_row(
                "SELECT name, parent_id FROM projects WHERE id = ?1",
            params![request.project_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| format!("Failed to get existing project: {e}"))?;
        PaneFs::validate_name(&old_name)
            .map_err(|e| format!("Unsafe project name in database: {e}"))?;
        
        if parent_id.is_none() && old_name != *name {
            // Top-level project - rename directory
            let old_relative = PaneFs::relative_from_names([old_name.as_str()])
                .map_err(|e| format!("Invalid existing project path: {e}"))?;
            let new_relative = PaneFs::relative_from_names([name.as_str()])
                .map_err(|e| format!("Invalid new project path: {e}"))?;

            if projects_path_exists(&old_relative)? {
                let fs_policy =
                    PaneFs::for_runtime().map_err(|e| format!("Filesystem policy error: {e}"))?;
                fs_policy
                    .rename(PaneRoot::Projects, &old_relative, &new_relative)
                    .map_err(|e| format!("Failed to rename project directory: {e}"))?;

                renamed_project_directory =
                    Some((old_relative, new_relative, old_name.clone(), name.clone()));
            }
        }
        
        updates.push("name = ?");
        update_params.push(Box::new(name.clone()));
    }
    
    if let Some(description) = &request.description {
        updates.push("description = ?");
        update_params.push(Box::new(description.clone()));
    }
    
    if let Some(color) = &request.color {
        updates.push("color = ?");
        update_params.push(Box::new(color.clone()));
    }

    if let Some(default_folder_id) = &request.default_folder_id {
        eprintln!(
            "[update_project] default_folder_id change requested: project={} folder={}",
            request.project_id, default_folder_id
        );
        validate_default_folder_choice(&conn, &request.project_id, default_folder_id)?;
        updates.push("default_folder_id = ?");
        update_params.push(Box::new(default_folder_id.clone()));
    }
    
    if !updates.is_empty() {
        updates.push("updated_at = ?");
        update_params.push(Box::new(now.clone()));
        
        let query = format!("UPDATE projects SET {} WHERE id = ?", updates.join(", "));
        update_params.push(Box::new(request.project_id.clone()));
        
        let params_refs: Vec<&dyn rusqlite::ToSql> = update_params
            .iter()
            .map(|b| b.as_ref() as &dyn rusqlite::ToSql)
            .collect();

        if let Some((old_relative, new_relative, old_name, new_name)) = &renamed_project_directory {
            let db_result = (|| -> Result<(), String> {
                let transaction = conn
                    .unchecked_transaction()
                    .map_err(|e| format!("Failed to start project rename transaction: {e}"))?;
                transaction
                    .execute(
                        "UPDATE notes SET file_path = replace(file_path, ?1, ?2) WHERE project_id = ?3",
                        params![
                            format!("Projects/{}/", old_name),
                            format!("Projects/{}/", new_name),
                            request.project_id
                        ],
                    )
                    .map_err(|e| format!("Failed to update note file paths: {e}"))?;
                transaction
                    .execute(
                        "UPDATE attachments SET project_name = ?1 WHERE project_id = ?2 AND project_name != ''",
                        params![new_name, request.project_id],
                    )
                    .map_err(|e| {
                        format!("Failed to update attachment project_name on rename: {e}")
                    })?;
                transaction
                    .execute(&query, &params_refs[..])
                    .map_err(|e| format!("Failed to update project: {e}"))?;
                transaction
                    .commit()
                    .map_err(|e| format!("Failed to commit project rename: {e}"))
            })();

            if let Err(error) = db_result {
                let rollback_error = PaneFs::for_runtime()
                    .and_then(|fs_policy| {
                        fs_policy.rename(PaneRoot::Projects, new_relative, old_relative)
                    })
                    .err();
                return Err(match rollback_error {
                    Some(rollback_error) => format!(
                        "{error}; also failed to restore project directory: {rollback_error}"
                    ),
                    None => error,
                });
            }
        
            eprintln!(
                "[PROJECTS] Updated attachments.project_name: '{}' → '{}' for project {}",
                old_name, new_name, request.project_id
            );
        } else {
        conn.execute(&query, &params_refs[..])
                .map_err(|e| format!("Failed to update project: {e}"))?;
        }
    }
    
    get_project(&conn, &request.project_id)
}

/// #3027 ruling 1: a trash failure on the project directory stops the delete
/// entirely — nothing is removed from disk OR the database, and the caller
/// gets an `Err` naming Trash. No `remove_dir_all` fallback exists anywhere
/// on this path (there used to be one — that was the bug).
///
/// #3027 ruling 2 (disk and DB must never diverge), chosen ordering: every
/// DB write below (the `attachments` cleanup and the `projects` row delete)
/// happens inside ONE SQL transaction that is NOT committed until after the
/// directory has already been successfully moved to the OS trash.
///   - **Trash fails:** the closure below returns `Err` before `commit()` is
///     ever called. `transaction` then drops without committing, and
///     rusqlite rolls back every DB write made inside it automatically —
///     exactly the same "drop = rollback" mechanism `delete_folder_internal`
///     (§2A.6, #2874 item 65) already relies on. Disk: untouched, because
///     `trash()` itself never completed. Net effect: nothing happened on
///     either side. This is the only ordering that can satisfy ruling 1 — if
///     the DB write happened (committed) first and trash failed afterward,
///     the row would already be gone while the directory sat on disk,
///     Err or not.
///   - **Trash succeeds, `commit()` then fails:** the one residual risk this
///     ordering cannot remove — SQLite's own commit failing (disk full,
///     I/O error) after an OS-level move that has no transactional
///     rollback of its own (the `trash` crate exposes no "undo", and
///     #3027's scope is the destructive-fallback bug, not building a new
///     restore-from-trash primitive). Logged as CRITICAL below; the DB row
///     survives (uncommitted write rolls back) so the project still shows
///     up in the UI, pointing at a directory that is now in the OS trash
///     instead of `Projects/`. Exceedingly rare in practice — a local
///     SQLite `COMMIT` on an already-open, already-validated connection —
///     and strictly smaller than the window the pre-#3027 code carried
///     (which committed the DB row unconditionally with no trash
///     involvement in the ordering at all).
///   - **Both succeed:** committed together, normal case.
///
/// Internal helper for project deletion — takes a live Connection so tests can call it
/// without needing a Tauri State wrapper.
pub fn delete_project_internal(
    project_id: &str,
    conn: &rusqlite::Connection,
) -> Result<(), String> {
    eprintln!(
        "[DELETE_PROJECT_INTERNAL] called with project_id: {}",
        project_id
    );

    PaneFs::validate_name(project_id).map_err(|e| format!("Invalid project id: {e}"))?;

    // Block deletion of system projects/folders
    let is_system: bool = conn
        .query_row(
        "SELECT COALESCE(is_system, 0) FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get::<_, i32>(0),
        )
        .map(|v| v != 0)
        .unwrap_or(false);

    if is_system {
        return Err("Cannot delete system folder".to_string());
    }

    // Check if this is a top-level project
    let (name, parent_id): (String, Option<String>) = conn
        .query_row(
        "SELECT name, parent_id FROM projects WHERE id = ?1",
        params![project_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("Project not found: {}", e))?;

    PaneFs::validate_name(&name).map_err(|e| format!("Unsafe project name in database: {e}"))?;
    let attachment_relative = PaneFs::relative_from_names([project_id])
        .map_err(|e| format!("Invalid legacy attachment path: {e}"))?;
    let legacy_attachments_dir = crate::database::get_pane_root()
        .join(".pane/attachments")
        .join(&attachment_relative);
    let legacy_attachments_exist = path_entry_exists(&legacy_attachments_dir)?;
    let fs_policy = PaneFs::for_runtime().map_err(|e| format!("Filesystem policy error: {e}"))?;
    if legacy_attachments_exist {
        fs_policy
            .authorize_existing(
                PaneRoot::Attachments,
                &attachment_relative,
                ExistingKind::Directory,
            )
            .map_err(|e| format!("Unsafe legacy attachment directory: {e}"))?;
    }

    // Top-level project only: resolve the directory path up front. Not inside
    // the transaction below — this is pure path computation, nothing mutates
    // yet, and a bad path should fail before we even open a transaction.
    let project_relative = if parent_id.is_none() {
        Some(
            PaneFs::relative_from_names([name.as_str()])
                .map_err(|e| format!("Invalid project path: {e}"))?,
        )
    } else {
        None
    };

    // #3027: one SQL transaction, held open (not committed) while the
    // directory trash is attempted — see the doc comment above this function
    // for the ordering rationale and every failure branch.
    let transaction = conn
        .unchecked_transaction()
        .map_err(|e| format!("Failed to start project delete transaction: {e}"))?;

    // Delete from database (CASCADE will delete notes and folders). Staged in
    // the open transaction only — not durable until `commit()` below.
    transaction
        .execute(
            "DELETE FROM attachments WHERE project_id = ?1",
            params![project_id],
        )
        .ok();
    transaction
        .execute("DELETE FROM projects WHERE id = ?1", params![project_id])
        .map_err(|e| format!("Failed to delete project: {}", e))?;

    // Now attempt the irreversible disk operation. If this fails, `return`
    // drops `transaction` without `commit()`, and rusqlite rolls back both
    // DELETEs above automatically — the project row is restored exactly as
    // it was. Ruling 1: nothing is deleted on disk OR in the DB.
    if let Some(project_relative) = &project_relative {
        if projects_path_exists(project_relative)? {
            eprintln!(
                "[PROJECTS] Moving project directory to trash: {}",
                get_projects_path().join(project_relative).display()
            );
            if let Err(e) =
                fs_policy.trash(PaneRoot::Projects, project_relative, ExistingKind::Directory)
            {
                eprintln!(
                    "[PROJECTS] trash failed ({e}); project delete aborted, nothing removed"
                );
                return Err(format!(
                    "Could not move project '{name}' to Trash ({e}). The project was not deleted."
                ));
            }
        }
    }

    if let Err(e) = transaction.commit() {
        // The directory is already in the OS trash at this point — the one
        // residual risk documented above (commit failing after a successful
        // trash). The DB row itself was never actually committed (this
        // branch IS the commit failing), so it is not lost from the
        // caller's perspective — but it is now pointing at a directory no
        // longer in `Projects/`. Loud because this should be exceedingly rare.
        eprintln!(
            "[PROJECTS] CRITICAL: project directory moved to trash but the DB transaction \
             failed to commit ({e}) — project '{name}' ({project_id}) may now be out of sync \
             between disk and database"
        );
        return Err(format!("Failed to commit project delete: {e}"));
    }

    // Clean up legacy attachment files (.pane/attachments/{project_id}/,
    // pre-v67). Best-effort and after the committed delete above — this is a
    // separate, already-authorized (checked above) legacy location, not the
    // main project directory ruling 1 governs. #3027 sweep: this used to be
    // a raw `remove_dir_all` (permanent, no trash) — now routed through
    // `trash()` like everything else that touches content a user could have
    // put there. Still non-fatal: a pre-v67 leftover failing to clean up
    // does not block (or need to roll back) a delete that already committed.
    if legacy_attachments_exist {
        match fs_policy.trash(
            PaneRoot::Attachments,
            &attachment_relative,
            ExistingKind::Directory,
        ) {
            Ok(()) => eprintln!(
                "[PROJECTS] Moved legacy attachment dir to trash: {}",
                legacy_attachments_dir.display()
            ),
            Err(error) => {
                eprintln!("[PROJECTS] Failed to trash authorized legacy attachment dir: {error}")
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub fn delete_project(project_id: String, db: State<DbState>) -> Result<(), String> {
    eprintln!("[DELETE_PROJECT] called with project_id: {}", project_id);
    let conn = db.0.lock().unwrap();
    delete_project_internal(&project_id, &conn)
}

pub(crate) fn move_folder_internal(
    conn: &rusqlite::Connection,
    folder_id: &str,
    target_project_id: &str,
) -> Result<(), String> {
    if folder_id == target_project_id {
        return Err("Cannot move a folder into itself".to_string());
    }

    let (_, source_parent_id): (String, Option<String>) = conn
        .query_row(
            "SELECT name, parent_id FROM projects WHERE id = ?1",
            params![folder_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("Folder not found: {e}"))?;
    let source_parent_id = source_parent_id
        .ok_or_else(|| "Cannot move a top-level project as a folder".to_string())?;

    // §1.7.1 ruling 4 (mirrors delete_folder_internal's identical default-folder guard
    // below): the default folder can be renamed, but never moved — moving it anywhere,
    // even within its own project, would leave `default_folder_id` pointing at a folder
    // that §1.7.1 requires to remain one of the project's own folders.
    let is_default: bool = conn
        .query_row(
            "SELECT default_folder_id FROM projects WHERE id = ?1",
            params![source_parent_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .unwrap_or(None)
        .as_deref()
        == Some(folder_id);
    if is_default {
        return Err("Cannot move the default folder".to_string());
    }

    // Resolve the complete source ancestry before any mutation. This validates every
    // DB-derived path component and rejects malformed parent cycles.
    let old_relative = get_project_relative_path(conn, folder_id, None)?;
    let folder_name = old_relative
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Invalid source folder path".to_string())?;

    let target_parent_id: Option<String> = conn
        .query_row(
            "SELECT parent_id FROM projects WHERE id = ?1",
            params![target_project_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Target project not found: {e}"))?;
    if target_parent_id.is_some() {
        return Err("Target must be a top-level project".to_string());
    }
    let target_relative =
        get_project_relative_path(conn, target_project_id, Some(target_project_id))?;
    let new_relative = target_relative.join(folder_name);

    // §1.7.2: the destination must not already have a sibling with this folder's
    // name. `target_project_id` is the destination parent (not the folder's current
    // parent) and `folder_name` is the folder's own unchanged name — the move variant
    // of the same create/rename/move check.
    check_folder_name_unique(conn, target_project_id, folder_name, folder_id)?;

    let fs_policy = PaneFs::for_runtime().map_err(|e| format!("Filesystem policy error: {e}"))?;
    fs_policy
        .authorize_existing(PaneRoot::Projects, &old_relative, ExistingKind::Directory)
        .map_err(|e| format!("Source folder directory is missing or unsafe: {e}"))?;

    if old_relative == new_relative {
        debug_assert_eq!(source_parent_id, target_project_id);
        return Ok(());
    }

    fs_policy
        .rename(PaneRoot::Projects, &old_relative, &new_relative)
        .map_err(|e| format!("Failed to move folder directory: {e}"))?;

    let old_path_prefix = format!(
        "{}/",
        Path::new("Projects").join(&old_relative).to_string_lossy()
    );
    let new_path_prefix = format!(
        "{}/",
        Path::new("Projects").join(&new_relative).to_string_lossy()
    );
    let now = chrono::Utc::now().to_rfc3339();

    let db_result = (|| -> Result<(), String> {
        let transaction = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start folder move transaction: {e}"))?;
        transaction
            .execute(
                "UPDATE projects SET parent_id = ?1, updated_at = ?2 WHERE id = ?3",
                params![target_project_id, now, folder_id],
            )
            .map_err(|e| format!("Failed to update folder: {e}"))?;
        transaction
            .execute(
                "WITH RECURSIVE folder_tree(id) AS (
                    SELECT ?1
                    UNION ALL
                    SELECT p.id FROM projects p JOIN folder_tree ft ON p.parent_id = ft.id
                 )
                 UPDATE notes
                 SET project_id = ?2, updated_at = ?3
                 WHERE folder_id IN (SELECT id FROM folder_tree)",
                params![folder_id, target_project_id, now],
            )
            .map_err(|e| format!("Failed to update notes in moved folder: {e}"))?;
        transaction
            .execute(
                "UPDATE notes
                 SET file_path = ?1 || substr(file_path, length(?2) + 1), updated_at = ?3
                 WHERE substr(file_path, 1, length(?2)) = ?2",
                params![new_path_prefix, old_path_prefix, now],
            )
            .map_err(|e| format!("Failed to update moved note file paths: {e}"))?;
        transaction
            .commit()
            .map_err(|e| format!("Failed to commit folder move: {e}"))
    })();

    if let Err(error) = db_result {
        let rollback_error = fs_policy
            .rename(PaneRoot::Projects, &new_relative, &old_relative)
            .err();
        return Err(match rollback_error {
            Some(rollback_error) => {
                format!("{error}; also failed to restore folder directory: {rollback_error}")
            }
            None => error,
        });
    }

    Ok(())
}

/// Shared logic for the Sidebar's "New Folder" command, split out from the
/// `#[tauri::command]` wrapper below so unit tests can call it directly —
/// same reasoning as `create_project`/`create_project_internal`: a real
/// `tauri::AppHandle` (`AppHandle<Wry>`) cannot be constructed from
/// `tauri::test::mock_app()`'s `AppHandle<MockRuntime>`, so anything a test
/// needs to call directly must not require one. This function does not
/// emit; the command wrapper does, once, after this succeeds.
pub fn create_folder_internal(
    request: CreateFolderRequest,
    db: State<DbState>,
) -> Result<Project, String> {
    let conn = db.0.lock().unwrap();
    
    PaneFs::validate_name(&request.name).map_err(|e| format!("Invalid folder name: {e}"))?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    
    // Get parent project/folder path
    let parent_id = request.parent_id.as_ref().unwrap_or(&request.project_id);

    // #2874 item 65 ruling 6: folders never nest — a folder's parent must be a
    // top-level project (parent_id IS NULL on the parent row), never another
    // folder. Mirrors `move_folder_internal`'s identical "Target must be a
    // top-level project" guard (projects.rs) exactly, applied here to create
    // instead of move.
    let parent_parent_id: Option<String> = conn
        .query_row(
            "SELECT parent_id FROM projects WHERE id = ?1",
            params![parent_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Parent not found: {e}"))?;
    if parent_parent_id.is_some() {
        return Err("Parent must be a top-level project — folders cannot nest".to_string());
    }

    // §1.7.2: no sibling may already have this name. Sentinel exclude_id ("")
    // because this is a create — there is no "own id" yet to exclude.
    check_folder_name_unique(&conn, parent_id, &request.name, "")?;

    let parent_relative = get_project_relative_path(&conn, parent_id, Some(&request.project_id))?;
    
    // Create directory on disk
    let folder_relative = parent_relative.join(&request.name);
    let fs_policy = PaneFs::for_runtime().map_err(|e| format!("Filesystem policy error: {e}"))?;
    fs_policy
        .create_dir_all(PaneRoot::Projects, &folder_relative)
        .map_err(|e| format!("Failed to create folder directory: {e}"))?;
    
    // Insert into database
    conn.execute(
        "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, request.name, parent_id, "user_local", now, now,],
    )
    .map_err(|e| format!("Failed to insert folder: {}", e))?;

    // Return created folder
    get_project(&conn, &id)
}

/// `#[tauri::command]` wrapper for `create_folder_internal` — the human
/// Sidebar "New Folder" path (`Sidebar.svelte:452` -> `api.createFolder()`
/// -> this command).
///
/// #2972 stage 1 (§34.1.4, ruling 7 — "the sidebar's New Folder" is one of
/// the three explicitly-named required emit sources): confirmed at base,
/// this path dispatched no event of any kind before this change — Sidebar
/// only self-refreshed via its own manual `$projects` re-fetch, and no
/// sibling component learned a folder was created. This closes that gap.
#[tauri::command]
pub fn create_folder(
    app: tauri::AppHandle,
    request: CreateFolderRequest,
    db: State<DbState>,
) -> Result<Project, String> {
    let folder = create_folder_internal(request, db)?;
    crate::refresh_event::emit_data_changed(
        &app,
        &folder.id,
        crate::refresh_event::ENTITY_FOLDER,
        crate::refresh_event::REASON_CREATED,
    );
    Ok(folder)
}

#[tauri::command]
pub fn get_folders(project_id: String, db: State<DbState>) -> Result<Vec<Project>, String> {
    let conn = db.0.lock().unwrap();
    
    let mut stmt = conn.prepare(
        "WITH RECURSIVE folder_tree AS (
            SELECT id, name, description, color, parent_id, sort_order, owner_id, created_at, updated_at, category, is_system, is_favorite, default_folder_id
            FROM projects
            WHERE parent_id = ?1
            UNION ALL
            SELECT p.id, p.name, p.description, p.color, p.parent_id, p.sort_order, p.owner_id, p.created_at, p.updated_at, p.category, p.is_system, p.is_favorite, p.default_folder_id
            FROM projects p
            JOIN folder_tree ft ON p.parent_id = ft.id
        )
        SELECT id, name, description, color, parent_id, sort_order, owner_id, created_at, updated_at, category, is_system, is_favorite, default_folder_id
        FROM folder_tree
        ORDER BY sort_order, name"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    // §24: Check if this is a system project
    let is_system_project = conn
        .query_row(
        "SELECT COALESCE(is_system, 0) FROM projects WHERE id = ?1",
        params![&project_id],
        |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);

    let folders = stmt
        .query_map([project_id], |row| {
        Ok(Project {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            color: row.get(3)?,
            parent_id: row.get(4)?,
            sort_order: row.get(5)?,
            owner_id: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
            category: row.get(9)?,
            is_system: row.get::<_, i32>(10)? != 0,
            is_favorite: row.get::<_, i32>(11)? != 0,
            default_folder_id: row.get(12)?,
        })
    })
    .map_err(|e| format!("Failed to query folders: {}", e))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("Failed to collect folders: {}", e))?;
    
    // §24: Hide "memory" subfolders inside system project agent folders
    if is_system_project {
        let folders = folders.into_iter().filter(|f| f.name != "memory").collect();
        return Ok(folders);
    }
    
    Ok(folders)
}

/// #2972 stage 2 (§34.5 WARN closure — Reed's 2026-10-11 audit of PR #3016,
/// Matt's sign-off: "fix before merge"). Sidebar, CalendarView and
/// NotesBrowser need to patch a single changed project-or-folder in place
/// on a `pane-data-changed` folder/project event instead of re-fetching
/// `get_projects()`/`get_folders()`/`syncProjectsWithNotebooks()` wholesale
/// — this is the single-entity-by-id read that gap was missing. A folder IS
/// a row in this same `projects` table (distinguished only by a non-null
/// `parent_id`), so one command serves both entity types; the id alone is
/// enough to find the row either way.
///
/// Returns the **exact same `Project` shape** as `get_projects` (above) and
/// `get_folders` (above) — same struct, same field population, same error
/// style — by delegating to the one existing `get_project` row-mapper
/// (`Helper Functions` section below) instead of re-deriving the mapping a
/// third time. That helper already backs both `create_project`'s and
/// `create_folder`'s "return what I just created" read, so this command's
/// shape is provably identical to what a list fetch would have produced for
/// the same row.
#[tauri::command]
pub fn get_project_or_folder(id: String, db: State<DbState>) -> Result<Project, String> {
    let conn = db.0.lock().unwrap();
    get_project(&conn, &id)
}

#[tauri::command]
pub fn update_folder(
    folder_id: String,
    name: Option<String>,
    db: State<DbState>,
) -> Result<Project, String> {
    let conn = db.0.lock().unwrap();
    
    // Prevent renaming system (AI) folders
    let is_sys: bool = conn
        .query_row(
        "SELECT COALESCE(is_system, 0) FROM projects WHERE id = ?1",
        rusqlite::params![folder_id],
        |row| row.get::<_, i32>(0),
        )
        .unwrap_or(0)
        != 0;
    if is_sys {
        return Err("Cannot rename system folder".to_string());
    }

    let now = chrono::Utc::now().to_rfc3339();
    
    if let Some(new_name) = &name {
        PaneFs::validate_name(new_name).map_err(|e| format!("Invalid folder name: {e}"))?;
        // Get old name and path
        let old_name: String = conn
            .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            params![folder_id],
            |row| row.get(0),
            )
            .map_err(|e| format!("Failed to get old name: {}", e))?;

        // §1.7.2: no sibling may already have the new name. Own id excluded so a
        // same-name "rename" (or a case-only change) never collides with itself.
        if old_name.to_lowercase() != new_name.to_lowercase() {
            let rename_parent_id: Option<String> = conn
                .query_row(
                    "SELECT parent_id FROM projects WHERE id = ?1",
                    params![folder_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to get parent for uniqueness check: {}", e))?;
            let rename_parent_id = rename_parent_id
                .ok_or_else(|| "Cannot rename a top-level project through update_folder".to_string())?;
            check_folder_name_unique(&conn, &rename_parent_id, new_name, &folder_id)?;
        }
        
        PaneFs::validate_name(&old_name)
            .map_err(|e| format!("Unsafe folder name in database: {e}"))?;
        let old_relative = get_project_relative_path(&conn, &folder_id, None)?;
        let parent_relative = old_relative
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| "Cannot rename the Projects root".to_string())?;
        let new_relative = parent_relative.join(new_name);
        
        // Rename directory
        if old_name != *new_name && projects_path_exists(&old_relative)? {
            let fs_policy =
                PaneFs::for_runtime().map_err(|e| format!("Filesystem policy error: {e}"))?;
            fs_policy
                .rename(PaneRoot::Projects, &old_relative, &new_relative)
                .map_err(|e| format!("Failed to rename folder directory: {e}"))?;
            
            // Update file_path for all notes in this folder
            let old_path_prefix = Path::new("Projects")
                .join(&old_relative)
                .to_string_lossy()
                .to_string();
            let new_path_prefix = Path::new("Projects")
                .join(&new_relative)
                .to_string_lossy()
                .to_string();
            
            let old_path_prefix = format!("{}/", old_path_prefix);
            let new_path_prefix = format!("{}/", new_path_prefix);
            let db_result = (|| -> Result<(), String> {
                let transaction = conn
                    .unchecked_transaction()
                    .map_err(|e| format!("Failed to start folder rename transaction: {e}"))?;
                transaction
                    .execute(
                        "UPDATE notes SET file_path = ?1 || substr(file_path, length(?2) + 1) WHERE substr(file_path, 1, length(?2)) = ?2",
                        params![new_path_prefix, old_path_prefix],
                    )
                    .map_err(|e| format!("Failed to update note file paths: {e}"))?;
                transaction
                    .execute(
                        "UPDATE projects SET name = ?1, updated_at = ?2 WHERE id = ?3",
                        params![new_name, now, folder_id],
                    )
                    .map_err(|e| format!("Failed to update folder: {e}"))?;
                transaction
                    .commit()
                    .map_err(|e| format!("Failed to commit folder rename: {e}"))
            })();
        
            if let Err(error) = db_result {
                let rollback_error = fs_policy
                    .rename(PaneRoot::Projects, &new_relative, &old_relative)
                    .err();
                return Err(match rollback_error {
                    Some(rollback_error) => format!(
                        "{error}; also failed to restore folder directory: {rollback_error}"
                    ),
                    None => error,
                });
            }
        } else {
            // Same-name renames are filesystem no-ops but still refresh metadata.
        conn.execute(
            "UPDATE projects SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_name, now, folder_id],
            )
            .map_err(|e| format!("Failed to update folder: {e}"))?;
        }
    }
    
    get_project(&conn, &folder_id)
}

/// Outcome of a successful `delete_folder_internal` call (#2874 item 65) —
/// every note/sheet that was moved out of the deleted folder into the
/// project's default folder, so the `#[tauri::command]` wrapper below can
/// emit the exact §34 event counts (one `moved` per note/sheet, one
/// `deleted` for the folder) without re-querying the DB after the fact.
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteFolderOutcome {
    pub moved: Vec<(String, &'static str)>,
}

/// Internal helper — testable without the Tauri `State` wrapper (mirrors
/// `delete_project_internal`'s split, so unit tests can drive it directly against an
/// in-memory/temp-dir connection without going through the trash crate's event loop
/// requirement, same rationale documented on `test_delete_project_removes_from_db`).
///
/// #2874 item 65 (Matt's ruling): deleting a folder no longer sends its
/// contents to the OS trash. Every note and sheet filed under it —
/// INCLUDING items already in Pane's own trash (`deleted_at IS NOT NULL`,
/// so a restore after this still lands correctly) — is moved into the
/// parent project's default folder first, through the SAME
/// `move_note_internal` path every other move in this codebase uses (no
/// parallel move implementation). Only the resulting EMPTY directory is
/// ever removed from disk; no note/sheet file is handed to the OS trash.
///
/// All-or-nothing (ruling 2), chosen ordering: every DB write below (each
/// note's move, the collapse-state cleanup, the folder row delete) happens
/// inside ONE SQL transaction, never committed until the very end — any
/// `Err` return below drops the transaction uncommitted, which rusqlite
/// rolls back automatically. The filesystem half (file renames) is NOT
/// inside that transaction — SQLite cannot cover the disk — so it is
/// tracked by hand: each note's `(old_file_path, new_file_path)` is
/// recorded in `relocated_paths` right after `move_note_internal` performs
/// it. If ANYTHING later fails — another note's move, the cleanup, the
/// folder delete, or `commit()` itself — every recorded rename is undone,
/// in reverse order, by a direct rename straight back to the original path
/// (bypassing the DB entirely, since the DB side is independently restored
/// by the transaction's own rollback — see `undo_relocated_paths`'s doc
/// comment for exactly why a plain rename, not another `move_note_internal`
/// call, is required there). Disk and DB are two separate rollback
/// mechanisms that both always run on any failure, not one depending on the
/// other's timing.
pub fn delete_folder_internal(
    folder_id: &str,
    conn: &rusqlite::Connection,
) -> Result<DeleteFolderOutcome, String> {
    // Prevent deleting system (AI) folders
    let is_system: bool = conn
        .query_row(
        "SELECT COALESCE(is_system, 0) FROM projects WHERE id = ?1",
        rusqlite::params![folder_id],
        |row| row.get::<_, i32>(0),
        )
        .unwrap_or(0)
        != 0;
    if is_system {
        return Err("Cannot delete system folder".to_string());
    }

    let parent_id: Option<String> = conn
        .query_row(
            "SELECT parent_id FROM projects WHERE id = ?1",
            params![folder_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Folder not found: {e}"))?;
    if parent_id.is_none() {
        return Err("Cannot delete a project through delete_folder".to_string());
    }

    // §1.7.1 ruling 4: the default folder can be renamed, but never deleted —
    // mirrors the is_system guard above exactly (same shape: read a flag off the
    // parent, short-circuit with an equivalent error message). There is no flow
    // yet to re-point default_folder_id at a different folder first, so refusing
    // here is the only thing keeping the pointer valid.
    let parent_project_id = parent_id
        .as_deref()
        .expect("checked above: parent_id is Some for a folder");
    let default_folder_id: Option<String> = project_default_folder_id(conn, parent_project_id);
    if default_folder_id.as_deref() == Some(folder_id) {
        return Err("Cannot delete the default folder".to_string());
    }
    let default_folder_id = default_folder_id
        .ok_or_else(|| format!("Project {parent_project_id} has no default_folder_id set"))?;

    // Get folder path. Every DB-derived component is validated before filesystem or DB mutation.
    let folder_relative = get_project_relative_path(conn, folder_id, None)?;

    // #2874 item 65 ruling 1: every note/sheet row currently filed under this
    // folder — no `deleted_at` filter, so items already sitting in Pane's
    // own trash are included and still resolve correctly after a later
    // restore.
    let mut stmt = conn
        .prepare("SELECT id, COALESCE(note_type, '') FROM notes WHERE folder_id = ?1 ORDER BY id")
        .map_err(|e| format!("Failed to list folder contents: {e}"))?;
    let notes_to_move: Vec<(String, String)> = stmt
        .query_map(params![folder_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| format!("Failed to list folder contents: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to list folder contents: {e}"))?;
    drop(stmt);

    let transaction = conn
        .unchecked_transaction()
        .map_err(|e| format!("Failed to start folder delete transaction: {e}"))?;

    let mut moved: Vec<(String, &'static str)> = Vec::new();
    // (old_file_path, new_file_path) for every note that had a real on-disk
    // file relocated — used to undo the FILESYSTEM half on failure,
    // independently of the SQL transaction's own (automatic) DB rollback.
    let mut relocated_paths: Vec<(String, String)> = Vec::new();

    let result: Result<(), String> = (|| -> Result<(), String> {
        for (note_id, note_type) in &notes_to_move {
            let before = crate::notes::get_note_meta(&transaction, note_id)
                .map_err(|e| format!("Failed to read note {note_id} before moving it: {e}"))?;

            // #2874: `enforce_system_guard = false` — this is a forced
            // relocation-to-safety ahead of deleting the folder underneath
            // these notes, not a user-initiated move. It must keep working
            // even for a system note that an older build (before this
            // guard existed) left sitting in a non-default folder; see
            // `reject_system_note_move`'s doc comment in notes.rs for the
            // full reasoning.
            crate::notes::move_note_internal(
                &transaction,
                note_id,
                parent_project_id,
                Some(default_folder_id.as_str()),
                false,
            )
            .map_err(|e| format!("Failed to move '{note_id}' to the default folder: {e}"))?;

            let after = crate::notes::get_note_meta(&transaction, note_id)
                .map_err(|e| format!("Failed to read note {note_id} after moving it: {e}"))?;
            if !before.file_path.is_empty() && before.file_path != after.file_path {
                relocated_paths.push((before.file_path, after.file_path));
            }

            let entity_type = if note_type == "sheet" {
                crate::refresh_event::ENTITY_SHEET
            } else {
                crate::refresh_event::ENTITY_NOTE
            };
            moved.push((note_id.clone(), entity_type));
        }

        // #2252: Clean up orphaned folder-view collapse-state rows before
        // deleting the folder row itself. ai_settings has no FK relationship
        // to projects (it's a flat KV table, not in CRR_TABLES), so nothing
        // cascades this automatically — left as-is these rows would
        // accumulate forever (harmless but permanent). Scoped by exact
        // folder_id prefix match so a sibling folder or a project-root scope
        // row (keyed by project id, see NotesBrowser.svelte
        // fvColumnScopeId()) is never touched. Now inside the same
        // transaction as everything else above and below.
        transaction
            .execute(
                "DELETE FROM ai_settings WHERE key LIKE 'folder:' || ?1 || ':%:collapsed'",
                params![folder_id],
            )
            .map_err(|e| format!("Failed to clean up folder collapse-state settings: {e}"))?;

        // Delete from database. CASCADE would delete any remaining notes too,
        // but there are none left under this folder_id — every one was just
        // moved out above.
        transaction
            .execute("DELETE FROM projects WHERE id = ?1", params![folder_id])
            .map_err(|e| format!("Failed to delete folder: {e}"))?;

        Ok(())
    })();

    if let Err(error) = result {
        undo_relocated_paths(&relocated_paths);
        // `transaction` drops here without `commit()` — rusqlite rolls back
        // every DB write made above automatically.
        return Err(error);
    }

    if let Err(error) = transaction.commit() {
        // The transaction's own Drop already rolled the DB back to its
        // pre-delete state (rusqlite rolls back on Drop unless committed) —
        // only the filesystem half still needs undoing.
        undo_relocated_paths(&relocated_paths);
        return Err(format!("Failed to commit folder delete transaction: {error}"));
    }

    // Reed's WARN closure (#2874 item 65, 2026-10-12 audit): every
    // DB-tracked note/sheet was already moved out above, but the on-disk
    // directory can still hold something Pane never tracked — a file a
    // user dropped in through Finder. Matt's ruling: "nothing Pane didn't
    // move is ever permanently deleted." So before removing the directory:
    //   1. Every remaining entry (anything NOT already moved out by the
    //      loop above) goes to the OS trash via the existing `PaneFs::trash`
    //      path (`database::delete_note_file`'s same underlying call —
    //      no parallel trash implementation) — recoverable, never a raw
    //      `fs::remove_file`/`remove_dir_all`.
    //   2. Only once the directory is actually empty is it removed, and
    //      only non-recursively (`remove_dir`, never `remove_dir_all`) — if
    //      it still isn't empty (a leftover's trash call failed), removal
    //      is skipped rather than forced.
    //   3. Best-effort throughout: any failure here (trashing a leftover,
    //      or the final directory removal) is logged and leaves the
    //      leftover/directory in place. It never turns into an `Err`
    //      return from this function and never rolls back the moves — the
    //      DB transaction already committed above.
    if projects_path_exists(&folder_relative).unwrap_or(false) {
        match PaneFs::for_runtime() {
            Ok(fs_policy) => {
                let absolute_dir = get_projects_path().join(&folder_relative);
                let mut all_leftovers_cleared = true;
                match fs::read_dir(&absolute_dir) {
                    Ok(entries) => {
                        for entry in entries {
                            let entry = match entry {
                                Ok(entry) => entry,
                                Err(e) => {
                                    eprintln!(
                                        "[PROJECTS] Folder '{}' leftover directory entry could not be read, leaving it in place: {}",
                                        folder_id, e
                                    );
                                    all_leftovers_cleared = false;
                                    continue;
                                }
                            };
                            let entry_relative = folder_relative.join(entry.file_name());
                            if let Err(e) =
                                fs_policy.trash(PaneRoot::Projects, &entry_relative, ExistingKind::Any)
                            {
                                eprintln!(
                                    "[PROJECTS] Folder '{}' had an untracked leftover '{}' that could not be moved to the OS trash; it has been left in place: {}",
                                    folder_id,
                                    entry_relative.display(),
                                    e
                                );
                                all_leftovers_cleared = false;
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "[PROJECTS] Folder '{}' directory could not be listed to check for leftovers, leaving it in place: {}",
                            folder_id, e
                        );
                        all_leftovers_cleared = false;
                    }
                }

                // Only ever non-recursive: if a leftover's trash call above
                // failed, the directory is still non-empty on purpose, and
                // `remove_dir` will itself refuse (logged below) rather
                // than this code forcing a recursive delete.
                if all_leftovers_cleared {
                    if let Err(e) = fs_policy.remove_dir(PaneRoot::Projects, &folder_relative) {
                        eprintln!(
                            "[PROJECTS] Folder '{}' deleted from DB but its now-empty directory could not be removed: {}",
                            folder_id, e
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!(
                    "[PROJECTS] Folder '{}' deleted from DB but its directory could not be cleaned up (filesystem policy error): {}",
                    folder_id, e
                );
            }
        }
    }

    Ok(DeleteFolderOutcome { moved })
}

/// Undoes `relocated_paths` (`(old, new)` pairs, in the order they were
/// recorded) in REVERSE, by renaming each file straight back from `new` to
/// `old` — directly, bypassing the DB and `move_note_internal` entirely.
/// This is deliberate, not a shortcut: by the time this runs, the enclosing
/// SQL transaction is either already dropped uncommitted or has already
/// rolled back via `commit()`'s own failure path, so every note's DB row
/// already reads its ORIGINAL `file_path`/`folder_id` again — calling
/// `move_note_internal` again here would see `current == target` and
/// no-op, leaving the disk file stranded at the new location. A plain
/// rename is the only operation that still needs to happen.
fn undo_relocated_paths(relocated_paths: &[(String, String)]) {
    for (old_path, new_path) in relocated_paths.iter().rev() {
        if let Err(rollback_error) = crate::database::rename_note_file(new_path, old_path) {
            eprintln!(
                "[PROJECTS] CRITICAL: failed to roll back a relocated file from '{}' back to '{}' after a failed folder delete: {}",
                new_path, old_path, rollback_error
            );
        }
    }
}

/// Emits the exact #2874 item 65 ruling 7 event set for a successful
/// `delete_folder_internal` call: one `moved` per note/sheet actually
/// relocated, plus exactly one `deleted` for the folder itself. Split out
/// from the `#[tauri::command]` wrapper below — generic over `R: Runtime`
/// (same reasoning as `emit_data_changed` itself) so a unit test can drive
/// it against `tauri::test::mock_app()`'s `AppHandle<MockRuntime>` and
/// assert the real emitted event COUNT/shape, not just that the wrapper's
/// source text calls `emit_data_changed` the right number of times.
fn emit_delete_folder_events<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    folder_id: &str,
    outcome: &DeleteFolderOutcome,
) {
    for (note_id, entity_type) in &outcome.moved {
        crate::refresh_event::emit_data_changed(
            app,
            note_id,
            entity_type,
            crate::refresh_event::REASON_MOVED,
        );
    }
    crate::refresh_event::emit_data_changed(
        app,
        folder_id,
        crate::refresh_event::ENTITY_FOLDER,
        crate::refresh_event::REASON_DELETED,
    );
}

#[tauri::command]
pub fn delete_folder(
    folder_id: String,
    app: tauri::AppHandle,
    db: State<DbState>,
) -> Result<(), String> {
    eprintln!("[DELETE_FOLDER] called with folder_id: {}", folder_id);
    let outcome = {
        let conn = db.0.lock().unwrap();
        delete_folder_internal(&folder_id, &conn)?
    };
    emit_delete_folder_events(&app, &folder_id, &outcome);
    Ok(())
}

/// Preview shown in the delete-folder confirmation dialog (#2874 item 65
/// ruling 4) — computed fresh at display time, never cached, so a rename of
/// the default folder or a note/sheet added right before the click is
/// always reflected. `default_folder_name` is read by ID
/// (`default_folder_id`), never by matching the literal "General" (§1.7.1).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct FolderDeletePreview {
    pub note_count: i64,
    pub sheet_count: i64,
    pub default_folder_name: String,
}

#[tauri::command]
pub fn get_folder_delete_preview(
    folder_id: String,
    db: State<DbState>,
) -> Result<FolderDeletePreview, String> {
    let conn = db.0.lock().unwrap();

    let parent_project_id: String = conn
        .query_row(
            "SELECT parent_id FROM projects WHERE id = ?1",
            params![folder_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Folder not found: {e}"))?;

    // No `deleted_at` filter — the preview counts everything that will
    // actually move, including items already in Pane's own trash (ruling 1).
    let note_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notes WHERE folder_id = ?1 AND COALESCE(note_type, '') != 'sheet'",
            params![folder_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to count notes: {e}"))?;
    let sheet_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notes WHERE folder_id = ?1 AND note_type = 'sheet'",
            params![folder_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to count sheets: {e}"))?;

    let default_folder_id: String = conn
        .query_row(
            "SELECT default_folder_id FROM projects WHERE id = ?1",
            params![parent_project_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Project {parent_project_id} has no default_folder_id set: {e}"))?;
    let default_folder_name: String = conn
        .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            params![default_folder_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Default folder not found: {e}"))?;

    Ok(FolderDeletePreview {
        note_count,
        sheet_count,
        default_folder_name,
    })
}

// === Category Commands ===

#[derive(Debug, Serialize, Deserialize)]
pub struct CategoryInfo {
    pub category: String,
    pub color: String,
}

#[tauri::command]
pub fn list_categories(db: State<DbState>) -> Result<Vec<CategoryInfo>, String> {
    eprintln!("[list_categories] called");
    let conn = db.0.lock().unwrap();
    
    // Ensure category_colors table exists.
    // A2 fix: do NOT silence this error with .ok() — if the CREATE fails (e.g. DB busy),
    // the UNION query below will also fail, leading to a silent "No categories" state.
    // Let the error propagate so callers see a real failure, not an empty list.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS category_colors (category TEXT NOT NULL DEFAULT '' PRIMARY KEY, color TEXT NOT NULL DEFAULT '#6B7280')",
        [],
    ).map_err(|e| format!("[list_categories] Failed to ensure category_colors table: {}", e))?;
    
    // Get distinct categories from both projects and category_colors tables
    let mut stmt = conn.prepare(
        "SELECT DISTINCT category FROM (
            SELECT category FROM projects WHERE category IS NOT NULL AND category != '' AND parent_id IS NULL
            UNION
            SELECT category FROM category_colors
        ) combined ORDER BY category"
    ).map_err(|e| format!("{}", e))?;
    
    let mut cats = stmt
        .query_map([], |row| {
        Ok(CategoryInfo {
            category: row.get(0)?,
            color: String::new(),
        })
        })
        .map_err(|e| format!("{}", e))?
    .filter_map(|r| r.ok())
    .collect::<Vec<_>>();
    
    // Get colors from category_colors table
    let color_map: std::collections::HashMap<String, String> = conn
        .prepare("SELECT category, color FROM category_colors")
        .ok()
    .and_then(|mut stmt| {
            stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
    })
    .unwrap_or_default();
    
    // Assign colors
    for cat in &mut cats {
        cat.color = color_map.get(&cat.category).cloned().unwrap_or_default();
    }
    
    Ok(cats)
}

#[tauri::command]
pub fn set_project_category(
    project_id: String,
    category: Option<String>,
    db: State<DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    conn.execute(
        "UPDATE projects SET category = ?1 WHERE id = ?2",
        params![category, project_id],
    )
    .map_err(|e| format!("{}", e))?;
    Ok(())
}

#[tauri::command]
pub fn set_category_color(
    category: String,
    color: String,
    db: State<DbState>,
) -> Result<(), String> {
    eprintln!(
        "[set_category_color] called: category={}, color={}",
        category, color
    );
    let conn = db.0.lock().unwrap();
    
    // Create table if it doesn't exist
    conn.execute(
        "CREATE TABLE IF NOT EXISTS category_colors (category TEXT NOT NULL DEFAULT '' PRIMARY KEY, color TEXT NOT NULL DEFAULT '#6B7280')",
        [],
    ).map_err(|e| format!("[set_category_color] Failed to ensure table: {}", e))?;
    
    // Insert or update color (create / color-update path only — use rename_category for renames)
    conn.execute(
        "INSERT OR REPLACE INTO category_colors (category, color) VALUES (?1, ?2)",
        params![category, color],
    )
    .map_err(|e| format!("[set_category_color] Failed to upsert: {}", e))?;
    
    Ok(())
}

/// Rename a category atomically.
///
/// A3 fix: updating a category name must propagate to all projects that reference it.
/// This command wraps both the `category_colors` rename and the `projects.category`
/// update in a single transaction so they cannot diverge.
///
/// Semantics:
/// - `old_category` = current name in `category_colors`
/// - `new_category` = desired new name
/// - `color`        = new color (may be same as old)
///
/// If `old_category == new_category`, this is a color-only update (no-op for project rows).
///
/// #3018 (§34.1/§34.2): rename is a bulk `UPDATE projects.category` across
/// every project that carried `old_category` — there is no single
/// `entity_id` for that bulk change. Matt's ruling: return the affected
/// project IDs from this same transaction (read them before the UPDATE
/// touches the rows, so the read and the write can never disagree about
/// which projects were affected) so the caller (`api.ts::renameCategory`)
/// can emit one `project`/`edited` event per affected project — not worked
/// out from frontend state, which could be stale or incomplete.
#[tauri::command]
pub fn rename_category(
    old_category: String,
    new_category: String,
    color: String,
    db: State<DbState>,
) -> Result<Vec<String>, String> {
    eprintln!(
        "[rename_category] called: '{}' → '{}' color={}",
        old_category, new_category, color
    );
    let conn = db.0.lock().unwrap();
    
    conn.execute("BEGIN", [])
        .map_err(|e| format!("[rename_category] BEGIN failed: {}", e))?;
    
    let result = (|| -> Result<Vec<String>, String> {
        // 1. Collect the IDs of every project this rename will touch, BEFORE
        // the UPDATE below changes them — same transaction, so this read is
        // guaranteed consistent with the write.
        let affected_ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM projects WHERE category = ?1")
                .map_err(|e| format!("[rename_category] Failed to prepare affected-ids query: {}", e))?;
            let rows = stmt
                .query_map(params![old_category], |row| row.get::<_, String>(0))
                .map_err(|e| format!("[rename_category] Failed to query affected ids: {}", e))?;
            rows.collect::<Result<Vec<String>, _>>()
                .map_err(|e| format!("[rename_category] Failed to read affected ids: {}", e))?
        };

        // 2. Rename (or update color) in category_colors
        conn.execute(
            "UPDATE category_colors SET category = ?2, color = ?3 WHERE category = ?1",
            params![old_category, new_category, color],
        )
        .map_err(|e| format!("[rename_category] Failed to update category_colors: {}", e))?;
        
        // 3. Update all projects that carried the old category name
        conn.execute(
            "UPDATE projects SET category = ?2 WHERE category = ?1",
            params![old_category, new_category],
        )
        .map_err(|e| {
            format!(
                "[rename_category] Failed to update projects.category: {}",
                e
            )
        })?;
        
        Ok(affected_ids)
    })();
    
    match result {
        Ok(affected_ids) => {
            conn.execute("COMMIT", [])
                .map_err(|e| format!("[rename_category] COMMIT failed: {}", e))?;
            eprintln!(
                "[rename_category] committed, {} project(s) affected",
                affected_ids.len()
            );
            Ok(affected_ids)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            eprintln!("[rename_category] ROLLBACK: {}", e);
            Err(e)
        }
    }
}

/// #3018 (§34.1/§34.2): same rationale as `rename_category` above — a bulk
/// `UPDATE projects.category` (cleared to NULL here, not renamed) with no
/// single `entity_id`. Returns the affected project IDs from the same
/// transaction so the caller can emit one `project`/`edited` event per
/// affected project. Previously ran as two bare sequential `execute()`
/// calls with no transaction at all; now wrapped in one so the affected-ids
/// read and the clear can never disagree (and so the two statements commit
/// or roll back together, which they never actually guaranteed before).
#[tauri::command]
pub fn delete_category(category: String, db: State<DbState>) -> Result<Vec<String>, String> {
    eprintln!("[delete_category] called: category={}", category);
    let conn = db.0.lock().unwrap();

    conn.execute("BEGIN", [])
        .map_err(|e| format!("[delete_category] BEGIN failed: {}", e))?;

    let result = (|| -> Result<Vec<String>, String> {
        // 1. Collect the IDs of every project this delete will touch, BEFORE
        // the clear below changes them.
        let affected_ids: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT id FROM projects WHERE category = ?1")
                .map_err(|e| format!("[delete_category] Failed to prepare affected-ids query: {}", e))?;
            let rows = stmt
                .query_map(params![category], |row| row.get::<_, String>(0))
                .map_err(|e| format!("[delete_category] Failed to query affected ids: {}", e))?;
            rows.collect::<Result<Vec<String>, _>>()
                .map_err(|e| format!("[delete_category] Failed to read affected ids: {}", e))?
        };

        // 2. Remove category from all projects
        conn.execute(
            "UPDATE projects SET category = NULL WHERE category = ?1",
            params![category],
        )
        .map_err(|e| format!("[delete_category] Failed to clear projects.category: {}", e))?;

        // 3. Remove color entry
        conn.execute(
            "DELETE FROM category_colors WHERE category = ?1",
            params![category],
        )
        .map_err(|e| {
            format!(
                "[delete_category] Failed to delete category_colors row: {}",
                e
            )
        })?;

        Ok(affected_ids)
    })();

    match result {
        Ok(affected_ids) => {
            conn.execute("COMMIT", [])
                .map_err(|e| format!("[delete_category] COMMIT failed: {}", e))?;
            eprintln!(
                "[delete_category] committed, {} project(s) affected",
                affected_ids.len()
            );
            Ok(affected_ids)
        }
        Err(e) => {
            let _ = conn.execute("ROLLBACK", []);
            eprintln!("[delete_category] ROLLBACK: {}", e);
            Err(e)
        }
    }
}

// === Helper Functions ===

fn get_project(conn: &rusqlite::Connection, project_id: &str) -> Result<Project, String> {
    conn.query_row(
        "SELECT id, name, description, color, parent_id, sort_order, owner_id, created_at, updated_at, category, is_system, is_favorite, default_folder_id
         FROM projects WHERE id = ?1",
        params![project_id],
        |row| {
            Ok(Project {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                color: row.get(3)?,
                parent_id: row.get(4)?,
                sort_order: row.get(5)?,
                owner_id: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
                category: row.get(9)?,
                is_system: row.get::<_, i32>(10)? != 0,
                is_favorite: row.get::<_, i32>(11)? != 0,
                default_folder_id: row.get(12)?,
            })
        },
    ).optional()
    .map_err(|e| format!("Failed to query project: {}", e))?
    .ok_or_else(|| "Project not found".to_string())
}

pub(crate) fn get_project_relative_path(
    conn: &rusqlite::Connection,
    project_id: &str,
    expected_project_id: Option<&str>,
) -> Result<PathBuf, String> {
    // Build a relative path by traversing up the parent chain. Reject malformed names and
    // cycles before returning anything that can be used for a filesystem mutation.
    let mut path_parts = Vec::new();
    let mut current_id = project_id.to_string();
    let mut visited = std::collections::HashSet::new();

    loop {
        if !visited.insert(current_id.clone()) {
            return Err("Invalid project hierarchy: parent cycle detected".to_string());
        }
        let (name, parent_id): (String, Option<String>) = conn
            .query_row(
                "SELECT name, parent_id FROM projects WHERE id = ?1",
                params![current_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| format!("Failed to get project info: {}", e))?;
        PaneFs::validate_name(&name)
            .map_err(|e| format!("Unsafe project or folder name in database: {e}"))?;
        path_parts.push(name);

        if let Some(pid) = parent_id {
            current_id = pid;
        } else {
            if let Some(expected_project_id) = expected_project_id {
                if current_id != expected_project_id {
                    return Err(format!(
                        "Folder does not belong to target project {expected_project_id}"
                    ));
                }
            }
            break;
        }
    }

    path_parts.reverse();
    PaneFs::relative_from_names(path_parts.iter().map(String::as_str))
        .map_err(|e| format!("Invalid project path: {e}"))
}

fn path_entry_exists(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "Failed to inspect path {}: {error}",
            path.display()
        )),
    }
}

fn projects_path_exists(relative: &Path) -> Result<bool, String> {
    path_entry_exists(&get_projects_path().join(relative))
}

#[tauri::command]
pub fn toggle_folder_hidden(
    folder_id: String,
    hidden: bool,
    db: State<DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();

    // #2874 item 9 ruling 5: the inverse of "cannot delete the default
    // folder" (delete_folder_internal, #2874 item 65 ruling 4) — hiding the
    // default folder would leave `default_folder_id` pointing at a folder
    // excluded from `list_visible_folders`'s agent-facing listing and from
    // `validate_default_folder_choice`'s own "not hidden" bar, and from the
    // Project Settings dropdown that sources its options the same way.
    // Only checked when HIDING (`hidden == true`) — un-hiding is always fine.
    if hidden {
        let parent_id: Option<String> = conn
            .query_row(
                "SELECT parent_id FROM projects WHERE id = ?1",
                params![&folder_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Folder not found: {e}"))?;
        if let Some(parent_id) = parent_id {
            if project_default_folder_id(&conn, &parent_id).as_deref() == Some(folder_id.as_str())
            {
                return Err("Cannot hide the default folder. Choose a different default in Project Settings first.".to_string());
            }
        }
    }

    conn.execute(
        "UPDATE projects SET is_hidden = ? WHERE id = ?",
        params![hidden as i64, folder_id],
    )
    .map_err(|e| format!("Failed to toggle folder hidden: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn get_hidden_folder_ids(db: State<DbState>) -> Result<Vec<String>, String> {
    let conn = db.0.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT id FROM projects WHERE is_hidden = 1")
        .map_err(|e| format!("Failed to query hidden folders: {}", e))?;

    let ids: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Failed to read hidden folders: {}", e))?
        .filter_map(|r| r.ok())
        .collect();
    
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    // ═══════════════════════════════════════════════════════════════════════════
    // Integration Test: Bug #800 - Project Memory auto-creation
    // ═══════════════════════════════════════════════════════════════════════════
    
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_project_memory_auto_created() {
        // Test that create_project_internal creates Memory.md and notes row
        let temp_dir = TempDir::new().unwrap();
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        
        // Create test database
        let db_path = temp_dir.path().join(".pane").join("pane.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        
        // Create minimal schema
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                color TEXT,
                category TEXT,
                owner_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                parent_id TEXT,
                sort_order INTEGER NOT NULL DEFAULT 0,
                default_model TEXT,
                smart_routing_enabled INTEGER,
                router_model TEXT,
                simple_tier_model TEXT,
                is_system INTEGER NOT NULL DEFAULT 0,
                is_hidden INTEGER NOT NULL DEFAULT 0,
                is_favorite INTEGER NOT NULL DEFAULT 0,
                default_folder_id TEXT
            );
            
            CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                content_preview TEXT NOT NULL DEFAULT '',
                file_path TEXT NOT NULL,
                project_id TEXT,
                folder_id TEXT,
                note_type TEXT,
                tags_json TEXT NOT NULL DEFAULT '[]',
                owner_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS chat_sessions (
                id             TEXT NOT NULL PRIMARY KEY,
                project_id     TEXT NOT NULL,
                title          TEXT NOT NULL DEFAULT '',
                origin         TEXT NOT NULL DEFAULT 'user',
                created_at     TEXT NOT NULL DEFAULT (datetime('now')),
                last_active_at TEXT NOT NULL DEFAULT (datetime('now')),
                hidden_at      TEXT,
                is_active      INTEGER NOT NULL DEFAULT 0,
                agent_id       TEXT
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_active_per_surface
                ON chat_sessions(project_id) WHERE is_active = 1;
        ",
        )
        .unwrap();
        
        // Create DbState
        let db_state = DbState::new(conn);
        
        // Create a project
        let request = CreateProjectRequest {
            name: "Test Project".to_string(),
            description: Some("A test project".to_string()),
            color: Some("#FF5733".to_string()),
            category: Some("work".to_string()),
            first_folder_name: Some("General".to_string()),
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        
        let project = create_project_internal(&request, &db_state).unwrap();
        
        // Verify Memory.md file exists
        let projects_path = temp_dir.path().join("Projects");
        let memory_file = projects_path.join("Test Project").join("Memory.md");
        assert!(memory_file.exists(), "Memory.md should be created");
        
        // Verify Memory.md has correct header
        let content = std::fs::read_to_string(&memory_file).unwrap();
        assert!(
            content.contains("# Memory — Test Project"),
            "Memory.md should have correct header"
        );
        
        // Verify notes row exists
        let conn = db_state.0.lock().unwrap();
        let note_count: i64 = conn
            .query_row(
            "SELECT COUNT(*) FROM notes WHERE project_id = ?1 AND note_type = 'memory'",
            rusqlite::params![project.id],
            |row| row.get(0),
            )
            .unwrap();
        assert_eq!(note_count, 1, "Should have exactly one memory note");
        
        // Verify note has correct attributes
        let (note_file_path, folder_id): (String, Option<String>) = conn.query_row(
            "SELECT file_path, folder_id FROM notes WHERE project_id = ?1 AND note_type = 'memory'",
            rusqlite::params![project.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).unwrap();
        
        assert_eq!(
            note_file_path, "Projects/Test Project/Memory.md",
            "Note file_path should match ADR-0020 canonical path"
        );
        assert!(
            folder_id.is_none(),
            "Memory note should have folder_id = NULL (project root)"
        );

    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Integration Test: #2142 — delete_project removes the project from DB
    // ═══════════════════════════════════════════════════════════════════════════

    /// Helper: minimal schema shared between #2142 tests.
    fn create_test_db(temp_dir: &TempDir) -> rusqlite::Connection {
        let db_path = temp_dir.path().join(".pane").join("pane.db");
        std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                color TEXT,
                category TEXT,
                owner_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                parent_id TEXT,
                sort_order INTEGER NOT NULL DEFAULT 0,
                default_model TEXT,
                smart_routing_enabled INTEGER,
                router_model TEXT,
                simple_tier_model TEXT,
                is_system INTEGER NOT NULL DEFAULT 0,
                is_hidden INTEGER NOT NULL DEFAULT 0,
                is_favorite INTEGER NOT NULL DEFAULT 0,
                default_folder_id TEXT
            );
            CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                content_preview TEXT NOT NULL DEFAULT '',
                file_path TEXT NOT NULL,
                project_id TEXT,
                folder_id TEXT,
                note_type TEXT,
                tags_json TEXT NOT NULL DEFAULT '[]',
                owner_id TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS attachments (
                id TEXT PRIMARY KEY,
                project_id TEXT
            );
            CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chat_sessions (
                id             TEXT NOT NULL PRIMARY KEY,
                project_id     TEXT NOT NULL,
                title          TEXT NOT NULL DEFAULT '',
                origin         TEXT NOT NULL DEFAULT 'user',
                created_at     TEXT NOT NULL DEFAULT (datetime('now')),
                last_active_at TEXT NOT NULL DEFAULT (datetime('now')),
                hidden_at      TEXT,
                is_active      INTEGER NOT NULL DEFAULT 0,
                agent_id       TEXT
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_active_per_surface
                ON chat_sessions(project_id) WHERE is_active = 1;
        ",
        )
        .unwrap();
        conn
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_delete_project_removes_from_db() {
        // #2142: Verify that delete_project_internal hard-deletes the project row.
        eprintln!("[TEST] test_delete_project_removes_from_db");
        let temp_dir = TempDir::new().unwrap();
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());

        let conn = create_test_db(&temp_dir);
        let db_state = DbState::new(conn);

        // Create a project via the internal helper
        let request = CreateProjectRequest {
            name: "To Delete".to_string(),
            description: None,
            color: None,
            category: None,
            first_folder_name: None,
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        let project = create_project_internal(&request, &db_state).unwrap();
        let project_id = project.id.clone();

        // Verify it exists
        {
            let conn = db_state.0.lock().unwrap();
            let count: i64 = conn
                .query_row(
                "SELECT COUNT(*) FROM projects WHERE id = ?1",
                rusqlite::params![&project_id],
                |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                count, 1,
                "Should have exactly one project row (no auto-created folders)"
            );
        }

        // Pre-remove the directory from disk so trash::delete is not called in tests.
        // (trash crate requires a macOS event loop which hangs in unit test runners)
        {
            let projects_path = crate::database::get_projects_path();
            let project_dir = projects_path.join("To Delete");
            if project_dir.exists() {
                std::fs::remove_dir_all(&project_dir).unwrap();
            }
        }

        // Delete via internal helper (directory already removed above — DB delete only)
        {
            let conn = db_state.0.lock().unwrap();
            delete_project_internal(&project_id, &conn).unwrap();
        }

        // Verify it is gone from DB (CASCADE deletes folder rows too)
        {
            let conn = db_state.0.lock().unwrap();
            let count: i64 = conn
                .query_row(
                "SELECT COUNT(*) FROM projects WHERE id = ?1",
                rusqlite::params![&project_id],
                |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 0, "Project row should be deleted from DB");
        }

    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Integration Test: #3027 — a trash failure stops the project delete
    // ═══════════════════════════════════════════════════════════════════════════

    /// Ruling 1, proven for real and deterministically, no dependence on the
    /// real OS Trash's own success/failure (same technique as the folder-delete
    /// sibling test `test_delete_folder_leaves_untrashable_leftover_in_place_on_trash_failure`,
    /// #2874 item 65): the project's directory ITSELF is a symlink, which
    /// `PaneFs::authorize_existing` rejects unconditionally ("symlink
    /// rejected") before `trash::delete` is ever reached. The real content
    /// lives at the symlink's target, outside `Projects/` entirely, so this
    /// test never touches the real OS trash.
    ///
    /// Fails before the fix, passes after, for real: the pre-fix code's
    /// `if let Err(e) = fs_policy.trash(...)` branch caught exactly this
    /// "symlink rejected" error and fell through to
    /// `fs_policy.remove_dir_all(...)` — which does not care whether its
    /// target is a symlink, and would have deleted the real content
    /// directory outright (the symlink's target), losing `note.md` for
    /// good, and then gone on to delete the DB row too. Under the fix,
    /// `delete_project_internal` returns `Err` before ever opening the
    /// directory, and the DB transaction it was staged in is dropped
    /// uncommitted (automatic rollback) — the project row, the note row,
    /// and the file are all exactly as they were.
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_delete_project_trash_failure_leaves_project_and_files_intact() {
        eprintln!("[TEST] test_delete_project_trash_failure_leaves_project_and_files_intact");
        let temp_dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("PANE_TEST_ROOT", temp_dir.path()) };

        let conn = create_test_db(&temp_dir);
        let db_state = DbState::new(conn);

        let request = CreateProjectRequest {
            name: "Symlink Swap Test".to_string(),
            description: None,
            color: None,
            category: None,
            first_folder_name: None,
            is_favorite: None,
            agent_id: "test-agent".to_string(),
        };
        let project = create_project_internal(&request, &db_state).unwrap();
        let project_id = project.id.clone();

        // A real, Pane-tracked note in this project — proves "files intact"
        // means DB-tracked content too, not just the bare directory.
        {
            let conn = db_state.0.lock().unwrap();
            conn.execute(
                "INSERT INTO notes (id, title, file_path, project_id, created_at, updated_at) \
                 VALUES ('note-trash-failure', 'Real Note', 'Projects/Trash Failure Test/note.md', ?1, datetime('now'), datetime('now'))",
                rusqlite::params![&project_id],
            )
            .unwrap();
        }

        let projects_path = crate::database::get_projects_path();
        let project_dir = projects_path.join("Symlink Swap Test");
        assert!(
            project_dir.exists(),
            "sanity: create_project_internal must have created a real directory"
        );

        // Real content lives OUTSIDE Projects/ entirely; the project
        // directory itself is replaced with a symlink pointing at it.
        let real_content_dir = temp_dir.path().join("real-content-outside-projects");
        std::fs::create_dir_all(&real_content_dir).unwrap();
        std::fs::write(
            real_content_dir.join("note.md"),
            "ORIGINAL CONTENT — MUST SURVIVE A TRASH FAILURE",
        )
        .unwrap();
        std::fs::remove_dir_all(&project_dir).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_content_dir, &project_dir).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real_content_dir, &project_dir).unwrap();

        let result = {
            let conn = db_state.0.lock().unwrap();
            delete_project_internal(&project_id, &conn)
        };

        assert!(
            result.is_err(),
            "a trash failure must stop the delete with an Err, not succeed: {:?}",
            result
        );
        let err = result.unwrap_err();
        eprintln!("[TEST] actual error returned: {}", err);
        assert!(
            err.contains("Trash"),
            "error must say the project couldn't be moved to Trash, got: {}",
            err
        );

        // (a) DB row present
        {
            let conn = db_state.0.lock().unwrap();
            let project_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM projects WHERE id = ?1",
                    rusqlite::params![&project_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                project_count, 1,
                "project row must still be present after a trash failure (ruling 1: nothing deleted in the DB)"
            );

            let note_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM notes WHERE id = 'note-trash-failure'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                note_count, 1,
                "note row must still be present — CASCADE must not have run (project row was never deleted)"
            );
        }

        // (b) files present, with intact contents
        assert!(
            project_dir.exists(),
            "project directory (the symlink) must still exist after a trash failure"
        );
        let content = std::fs::read_to_string(real_content_dir.join("note.md")).unwrap();
        assert_eq!(
            content, "ORIGINAL CONTENT — MUST SURVIVE A TRASH FAILURE",
            "file content must be byte-identical and untouched — no remove_dir_all fallback ran"
        );

    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Integration Test: #2252 — folder-view collapse rail persistence + cleanup
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_delete_folder_cleans_up_ai_settings_collapse_rows() {
        // #2252: delete_folder_internal must remove the two folder:{id}:{col}:collapsed
        // rows for the deleted folder, and MUST NOT touch a sibling folder's rows.
        eprintln!("[TEST] test_delete_folder_cleans_up_ai_settings_collapse_rows");
        let temp_dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("PANE_TEST_ROOT", temp_dir.path()) };

        let conn = create_test_db(&temp_dir);
        let db_state = DbState::new(conn);

        // Create a project (auto-creates one "General" folder).
        let request = CreateProjectRequest {
            name: "Collapse Test".to_string(),
            description: None,
            color: None,
            category: None,
            first_folder_name: Some("General".to_string()),
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };
        let project = create_project_internal(&request, &db_state).unwrap();

        // §1.7.1: this is the project's auto-created, auto-default folder —
        // create_project_internal points default_folder_id at it, so
        // delete_folder_internal now refuses to delete it (ruling 4). It plays the
        // SURVIVOR role in this test instead (renamed from the original `folder_id`).
        let default_folder_id: String = {
            let conn = db_state.0.lock().unwrap();
            conn.query_row(
                "SELECT id FROM projects WHERE parent_id = ?1",
                rusqlite::params![&project.id],
                |row| row.get(0),
            )
            .unwrap()
        };

        // Create a second, sibling folder under the same project. This is NOT the
        // default folder, so it's the one actually deleted below (the target).
        let sibling_id = uuid::Uuid::new_v4().to_string();
        {
            let conn = db_state.0.lock().unwrap();
            conn.execute(
                "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                 VALUES (?1, 'Sibling', ?2, 'user_local', datetime('now'), datetime('now'))",
                rusqlite::params![sibling_id, &project.id],
            )
            .unwrap();

            // Seed collapse-state rows for both the default folder and the sibling
            // (deletion target).
            conn.execute(
                "INSERT INTO ai_settings (key, value) VALUES (?1, 'true')",
                rusqlite::params![format!("folder:{}:cabinet:collapsed", default_folder_id)],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ai_settings (key, value) VALUES (?1, 'false')",
                rusqlite::params![format!("folder:{}:tasks:collapsed", default_folder_id)],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO ai_settings (key, value) VALUES (?1, 'true')",
                rusqlite::params![format!("folder:{}:cabinet:collapsed", sibling_id)],
            )
            .unwrap();
        }

        // Pre-remove the on-disk directory so delete_folder_internal's trash call is a
        // no-op (trash crate requires a macOS event loop that hangs in unit test runners —
        // same workaround as test_delete_project_removes_from_db).
        {
            let projects_path = crate::database::get_projects_path();
            let folder_dir = projects_path.join("Collapse Test").join("Sibling");
            if folder_dir.exists() {
                std::fs::remove_dir_all(&folder_dir).unwrap();
            }
        }

        {
            let conn = db_state.0.lock().unwrap();
            delete_folder_internal(&sibling_id, &conn).unwrap();
        }

        let conn = db_state.0.lock().unwrap();

        // Deleted (sibling) folder's rows are gone.
        let remaining_for_deleted: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ai_settings WHERE key LIKE 'folder:' || ?1 || ':%:collapsed'",
                rusqlite::params![&sibling_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            remaining_for_deleted, 0,
            "Deleted folder's ai_settings collapse rows must be cleaned up"
        );

        // Default folder's rows must survive untouched.
        let default_value: String = conn
            .query_row(
                "SELECT value FROM ai_settings WHERE key = ?1",
                rusqlite::params![format!("folder:{}:cabinet:collapsed", default_folder_id)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            default_value, "true",
            "Default folder's ai_settings row must NOT be deleted"
        );

        // Deleted (sibling) folder row itself is gone (existing delete_folder behavior,
        // unaffected by #2252).
        let folder_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM projects WHERE id = ?1",
                rusqlite::params![&sibling_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(folder_count, 0, "Folder row should be deleted from DB");

    }

    #[test]
    fn test_set_ai_setting_roundtrip_for_folder_collapse_keys() {
        // #2252: verifies the ai_settings KV roundtrip that NotesBrowser.svelte's
        // setColumnCollapsed()/load-effect rely on, using the exact key format
        // ("folder:{id}:{column}:collapsed") and 'true'/'false' string convention.
        // Goes red if the key format or string-boolean convention ever drifts.
        eprintln!("[TEST] test_set_ai_setting_roundtrip_for_folder_collapse_keys");
        let temp_dir = TempDir::new().unwrap();
        let conn = create_test_db(&temp_dir);

        let folder_id = "folder-abc-123";
        let cabinet_key = format!("folder:{}:cabinet:collapsed", folder_id);
        let tasks_key = format!("folder:{}:tasks:collapsed", folder_id);

        // No saved state yet — both keys must read back as None (defaults to expanded
        // in the frontend loader).
        assert!(crate::database::get_ai_setting(&conn, &cabinet_key).is_none());
        assert!(crate::database::get_ai_setting(&conn, &tasks_key).is_none());

        // Save: Cabinet collapsed, Tasks expanded.
        crate::database::set_ai_setting(&conn, &cabinet_key, "true").unwrap();
        crate::database::set_ai_setting(&conn, &tasks_key, "false").unwrap();

        assert_eq!(
            crate::database::get_ai_setting(&conn, &cabinet_key),
            Some("true".to_string())
        );
        assert_eq!(
            crate::database::get_ai_setting(&conn, &tasks_key),
            Some("false".to_string())
        );

        // Toggle back: Cabinet expanded.
        crate::database::set_ai_setting(&conn, &cabinet_key, "false").unwrap();
        assert_eq!(
            crate::database::get_ai_setting(&conn, &cabinet_key),
            Some("false".to_string())
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // Integration Test: #2142 — initial_project_created flag prevents re-creation
    // ═══════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_initial_project_created_flag_persists() {
        // #2142: Once 'initial_project_created' is written to ai_settings it can be read
        // back, proving the guard in resolveWorkshopProjectId() will fire on the next run.
        eprintln!("[TEST] test_initial_project_created_flag_persists");
        let temp_dir = TempDir::new().unwrap();
        let conn = create_test_db(&temp_dir);

        // Flag should not exist initially
        let before: Option<String> = conn
            .query_row(
            "SELECT value FROM ai_settings WHERE key = 'initial_project_created'",
            [],
            |row| row.get(0),
            )
            .ok();
        assert!(before.is_none(), "Flag must not exist on a fresh DB");

        // Simulate the Svelte code writing the flag when it first creates 'My Notes'
        conn.execute(
            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('initial_project_created', 'true')",
            [],
        ).unwrap();

        // Flag must now be readable as 'true'
        let after: String = conn
            .query_row(
            "SELECT value FROM ai_settings WHERE key = 'initial_project_created'",
            [],
            |row| row.get(0),
            )
            .unwrap();
        assert_eq!(after, "true", "Flag must persist as 'true' after being set");

        // Simulate a second run: if we found projects = [] AND flag = 'true', we must
        // NOT insert another 'My Notes'. We model this as a guard-check assertion.
        let flag_set = after == "true";
        let user_projects: i64 = conn.query_row(
            "SELECT COUNT(*) FROM projects WHERE (is_system IS NULL OR is_system = 0) AND parent_id IS NULL",
            [],
            |row| row.get(0),
        ).unwrap();
        // User deleted all projects; flag is set — the guard should kick in.
        assert_eq!(user_projects, 0, "No user projects in this scenario");
        assert!(
            flag_set,
            "Flag is set — guard must prevent auto-creation of 'My Notes'"
        );
        // If we were in JS: !flag_set would be false, so the create branch is skipped.
        let would_create = !flag_set;
        assert!(
            !would_create,
            "'My Notes' must NOT be auto-created when flag is set and projects = 0"
        );
    }

    fn create_folder_move_test_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                parent_id TEXT,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                file_path TEXT NOT NULL,
                project_id TEXT,
                folder_id TEXT,
                deleted_at TEXT,
                updated_at TEXT NOT NULL
            );
            INSERT INTO projects VALUES
                ('source', 'Source', NULL, 'old'),
                ('target', 'Target', NULL, 'old'),
                ('parent', 'Parent', 'source', 'old'),
                ('child', 'Child', 'parent', 'old');
            INSERT INTO notes VALUES
                ('active', 'Projects/Source/Parent/active.md', 'source', 'parent', NULL, 'old'),
                ('deleted', 'Projects/Source/Parent/Child/deleted.md', 'source', 'child', '2026-01-01', 'old'),
                ('path-only', 'Projects/Source/Parent/orphan.md', 'source', NULL, NULL, 'old');
            ",
        )
        .unwrap();
        conn
    }

    fn create_folder_move_directories(temp_dir: &TempDir) {
        let source = temp_dir.path().join("Projects/Source/Parent/Child");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(temp_dir.path().join("Projects/Target")).unwrap();
        std::fs::write(source.join("deleted.md"), "deleted").unwrap();
        std::fs::write(
            temp_dir.path().join("Projects/Source/Parent/active.md"),
            "active",
        )
        .unwrap();
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn move_folder_moves_nested_tree_and_updates_deleted_note_paths() {
        let temp_dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("PANE_TEST_ROOT", temp_dir.path()) };
        create_folder_move_directories(&temp_dir);
        let conn = create_folder_move_test_db();

        move_folder_internal(&conn, "parent", "target").unwrap();

        let parent_id: String = conn
            .query_row(
                "SELECT parent_id FROM projects WHERE id = 'parent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent_id, "target");

        let (project_id, file_path, deleted_at): (String, String, Option<String>) = conn
            .query_row(
                "SELECT project_id, file_path, deleted_at FROM notes WHERE id = 'deleted'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(project_id, "target");
        assert_eq!(file_path, "Projects/Target/Parent/Child/deleted.md");
        assert!(deleted_at.is_some());

        let (path_only_project, path_only_path): (String, String) = conn
            .query_row(
                "SELECT project_id, file_path FROM notes WHERE id = 'path-only'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(path_only_project, "source");
        assert_eq!(path_only_path, "Projects/Target/Parent/orphan.md");
        assert!(!temp_dir.path().join("Projects/Source/Parent").exists());
        assert!(temp_dir
            .path()
            .join("Projects/Target/Parent/Child/deleted.md")
            .exists());

    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn move_folder_rolls_back_directory_and_database_on_sql_failure() {
        let temp_dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("PANE_TEST_ROOT", temp_dir.path()) };
        create_folder_move_directories(&temp_dir);
        let conn = create_folder_move_test_db();
        conn.execute_batch(
            "CREATE TRIGGER reject_note_move BEFORE UPDATE ON notes
             BEGIN SELECT RAISE(ABORT, 'forced note update failure'); END;",
        )
        .unwrap();

        let error = move_folder_internal(&conn, "parent", "target").unwrap_err();

        assert!(error.contains("forced note update failure"));
        let parent_id: String = conn
            .query_row(
                "SELECT parent_id FROM projects WHERE id = 'parent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let (project_id, file_path): (String, String) = conn
            .query_row(
                "SELECT project_id, file_path FROM notes WHERE id = 'deleted'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(parent_id, "source");
        assert_eq!(project_id, "source");
        assert_eq!(file_path, "Projects/Source/Parent/Child/deleted.md");
        assert!(temp_dir
            .path()
            .join("Projects/Source/Parent/Child/deleted.md")
            .exists());
        assert!(!temp_dir.path().join("Projects/Target/Parent").exists());

    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn move_folder_fails_when_source_directory_is_missing() {
        let temp_dir = TempDir::new().unwrap();
        unsafe { std::env::set_var("PANE_TEST_ROOT", temp_dir.path()) };
        std::fs::create_dir_all(temp_dir.path().join("Projects/Target")).unwrap();
        let conn = create_folder_move_test_db();

        let error = move_folder_internal(&conn, "parent", "target").unwrap_err();

        assert!(error.contains("Source folder directory is missing"));
        let parent_id: String = conn
            .query_row(
                "SELECT parent_id FROM projects WHERE id = 'parent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(parent_id, "source");

    }

    // ════════════════════════════════════════════════════════════════════════
    // #2874 item 50a, §1.7.1/§1.7.2: default-folder delete refusal + the shared
    // folder-name-uniqueness check, exercised through the REAL #[tauri::command]
    // entry points (`create_folder`, `update_folder`) via `tauri::test::mock_app` +
    // `app.manage(DbState)`, the same pattern `agents.rs`'s
    // `upsert_agent_explicit_runtime_type_tests` already established in this
    // codebase for testing a command through its real State<DbState> signature
    // instead of only the inner logic.
    // ════════════════════════════════════════════════════════════════════════
    mod default_folder_and_duplicate_name_tests {
        use super::*;
        use tauri::Manager;

        /// Real tauri::State<DbState> backed by a fresh-migrated in-memory DB (the
        /// current canonical schema, including `default_folder_id`), with
        /// PANE_TEST_ROOT pointed at a scratch filesystem dir — `create_folder`/
        /// `update_folder`/`create_project_internal` all do real `PaneFs` disk
        /// operations, so a real filesystem root is required.
        fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
            let fs_dir = TempDir::new().expect("scratch fs dir");
            unsafe { std::env::set_var("PANE_TEST_ROOT", fs_dir.path()) };
            let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
            crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
            // create_project_internal creates a born-active chat_session tied to
            // agent_id (ADR-0062 D2) — needs a real ai_agents row to satisfy the
            // foreign-key-shaped lookup, same as every other full-schema test
            // fixture in this codebase (e.g. agents.rs's seed_agent helper).
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path) \
                 VALUES ('test-agent', 'Test Agent', 'test-agent', 'main', 1, 1, 'test-agent')",
                [],
            )
            .expect("seed test agent");
            let app = tauri::test::mock_app();
            app.manage(crate::database::DbState::new(conn));
            (app, fs_dir)
        }

        fn teardown() {
        }

        fn make_project(db_state: &State<DbState>, name: &str) -> Project {
            create_project_internal(
                &CreateProjectRequest {
                    name: name.to_string(),
                    description: None,
                    color: None,
                    category: None,
                    first_folder_name: None,
                    is_favorite: None,
                    agent_id: "test-agent".to_string(),
                },
                db_state,
            )
            .unwrap()
        }

        /// §1.7.1 ruling 4: the default folder can be renamed, but deleting it must
        /// always be refused — mirrors the pre-existing is_system guard's shape
        /// exactly (same error-message style: "Cannot delete ... folder").
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_folder_internal_refuses_to_delete_default_folder() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Delete Guard Test");

            let default_folder_id: String = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row(
                    "SELECT default_folder_id FROM projects WHERE id = ?1",
                    params![&project.id],
                    |row| row.get(0),
                )
                .unwrap()
            };

            // Before: the folder row exists.
            let exists_before: bool = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) > 0 FROM projects WHERE id = ?1",
                    params![&default_folder_id],
                    |row| row.get(0),
                )
                .unwrap()
            };
            assert!(exists_before, "before: default folder row must exist");

            // After: deletion is refused with the expected message, and the row
            // still exists (nothing was deleted).
            let result = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&default_folder_id, &conn)
            };
            assert_eq!(
                result,
                Err("Cannot delete the default folder".to_string()),
                "deleting the default folder must be refused with this exact message"
            );

            let exists_after: bool = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) > 0 FROM projects WHERE id = ?1",
                    params![&default_folder_id],
                    |row| row.get(0),
                )
                .unwrap()
            };
            assert!(exists_after, "after: default folder row must still exist (refused, not deleted)");

            teardown();
        }

        /// §1.7.2: create_folder (the sidebar's real Tauri command) must refuse a
        /// second folder with the same name (case-insensitive) in the same project.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_create_folder_rejects_duplicate_name() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Create Dup Test");

            let first = create_folder_internal(
                CreateFolderRequest {
                    project_id: project.id.clone(),
                    name: "Notes".to_string(),
                    parent_id: None,
                },
                db_state.clone(),
            );
            assert!(first.is_ok(), "first 'Notes' folder must succeed: {:?}", first.err());

            // Before fix: this second create would also succeed, producing two
            // same-named siblings. After fix: it must be refused.
            let second = create_folder_internal(
                CreateFolderRequest {
                    project_id: project.id.clone(),
                    name: "notes".to_string(), // case-insensitive collision
                    parent_id: None,
                },
                db_state.clone(),
            );
            assert!(
                second.is_err(),
                "second, case-insensitively duplicate 'notes' folder must be refused"
            );
            assert!(
                second.unwrap_err().starts_with("DUPLICATE_FOLDER:"),
                "duplicate rejection must use the DUPLICATE_FOLDER: error shape"
            );

            let sibling_count: i64 = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM projects WHERE parent_id = ?1 AND LOWER(name) = 'notes'",
                    params![&project.id],
                    |row| row.get(0),
                )
                .unwrap()
            };
            assert_eq!(sibling_count, 1, "only the first 'Notes' folder must exist — no duplicate was created");

            teardown();
        }

        /// §1.7.2: update_folder (the sidebar's real rename command) must refuse
        /// renaming a folder to a name an existing sibling already has.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_update_folder_rejects_duplicate_name_on_rename() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Rename Dup Test");

            create_folder_internal(
                CreateFolderRequest { project_id: project.id.clone(), name: "Notes".to_string(), parent_id: None },
                db_state.clone(),
            ).unwrap();
            let docs = create_folder_internal(
                CreateFolderRequest { project_id: project.id.clone(), name: "Docs".to_string(), parent_id: None },
                db_state.clone(),
            ).unwrap();

            let docs_name_before: String = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row("SELECT name FROM projects WHERE id = ?1", params![&docs.id], |row| row.get(0)).unwrap()
            };
            assert_eq!(docs_name_before, "Docs", "before: the second folder is still named 'Docs'");

            // Before fix: renaming 'Docs' -> 'Notes' would succeed, producing two
            // same-named siblings. After fix: it must be refused.
            let result = update_folder(docs.id.clone(), Some("Notes".to_string()), db_state.clone());
            assert!(result.is_err(), "renaming 'Docs' to the existing 'Notes' name must be refused");
            assert!(
                result.unwrap_err().starts_with("DUPLICATE_FOLDER:"),
                "duplicate rejection must use the DUPLICATE_FOLDER: error shape"
            );

            let docs_name_after: String = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row("SELECT name FROM projects WHERE id = ?1", params![&docs.id], |row| row.get(0)).unwrap()
            };
            assert_eq!(docs_name_after, "Docs", "after: the refused rename must leave the folder's name unchanged");

            teardown();
        }

        /// §1.7.2: move_folder_internal (the shared move primitive, used by both the
        /// agent's execute_move_folder and the Tauri move_folder command) must
        /// refuse a move whose DESTINATION already has a sibling with that name.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_move_folder_internal_rejects_duplicate_name_on_destination() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let source_project = make_project(&db_state, "Move Dup Source");
            let target_project = make_project(&db_state, "Move Dup Target");

            let moving = create_folder_internal(
                CreateFolderRequest { project_id: source_project.id.clone(), name: "Shared".to_string(), parent_id: None },
                db_state.clone(),
            ).unwrap();
            create_folder_internal(
                CreateFolderRequest { project_id: target_project.id.clone(), name: "Shared".to_string(), parent_id: None },
                db_state.clone(),
            ).unwrap();

            let moving_parent_before: String = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row("SELECT parent_id FROM projects WHERE id = ?1", params![&moving.id], |row| row.get(0)).unwrap()
            };
            assert_eq!(moving_parent_before, source_project.id, "before: the folder being moved is still under its source project");

            // Before fix: this move would succeed, leaving target_project with two
            // 'Shared' folders. After fix: it must be refused.
            let result = {
                let conn = db_state.0.lock().unwrap();
                move_folder_internal(&conn, &moving.id, &target_project.id)
            };
            assert!(result.is_err(), "moving 'Shared' into a project that already has a 'Shared' folder must be refused");
            assert!(
                result.unwrap_err().starts_with("DUPLICATE_FOLDER:"),
                "duplicate rejection must use the DUPLICATE_FOLDER: error shape"
            );

            let moving_parent_after: String = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row("SELECT parent_id FROM projects WHERE id = ?1", params![&moving.id], |row| row.get(0)).unwrap()
            };
            assert_eq!(moving_parent_after, source_project.id, "after: the refused move must leave the folder under its original source project");

            teardown();
        }

        /// §1.7.1 (#2874 item 50b): `move_folder_internal` must refuse to move a
        /// project's default folder — mirrors `delete_folder_internal`'s identical
        /// default-folder guard exactly, including the error-message style
        /// ("Cannot move ... folder"). Covers both a move to a different project
        /// AND a move "within its own project" (same source and target project) —
        /// the spec is explicit that moving the default folder is refused
        /// unconditionally, not just cross-project.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_move_folder_internal_refuses_to_move_default_folder() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Move Default Guard Test");
            let other_project = make_project(&db_state, "Move Default Guard Other");

            let default_folder_id: String = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row(
                    "SELECT default_folder_id FROM projects WHERE id = ?1",
                    params![&project.id],
                    |row| row.get(0),
                )
                .unwrap()
            };

            // Before: the default folder is parented under its own project.
            let parent_before: String = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row(
                    "SELECT parent_id FROM projects WHERE id = ?1",
                    params![&default_folder_id],
                    |row| row.get(0),
                )
                .unwrap()
            };
            assert_eq!(parent_before, project.id, "before: default folder is parented under its own project");

            // Attempt 1: move to a different project — refused.
            let cross_project_result = {
                let conn = db_state.0.lock().unwrap();
                move_folder_internal(&conn, &default_folder_id, &other_project.id)
            };
            assert_eq!(
                cross_project_result,
                Err("Cannot move the default folder".to_string()),
                "moving the default folder to a different project must be refused with this exact message"
            );

            // Attempt 2: move "within its own project" (same project as both
            // source and target) — refused identically. §1.7.1 requires the
            // default folder to remain one of the project's own folders at all
            // times; even a same-project move is refused unconditionally.
            let same_project_result = {
                let conn = db_state.0.lock().unwrap();
                move_folder_internal(&conn, &default_folder_id, &project.id)
            };
            assert_eq!(
                same_project_result,
                Err("Cannot move the default folder".to_string()),
                "moving the default folder within its own project must also be refused with this exact message"
            );

            // After: parent_id is unchanged by either refused attempt.
            let parent_after: String = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row(
                    "SELECT parent_id FROM projects WHERE id = ?1",
                    params![&default_folder_id],
                    |row| row.get(0),
                )
                .unwrap()
            };
            assert_eq!(parent_after, project.id, "after: both refused moves must leave the default folder's parent unchanged");

            teardown();
        }
    }

    // ============================================================================
    // Lane 1 PR 3 (#2874 item 9): the default-folder dropdown's backend —
    // `UpdateProjectRequest.default_folder_id` on `update_project`, validated
    // by `validate_default_folder_choice` (cross-project / system / hidden
    // all refused, each its own test), plus the inverse guard on
    // `toggle_folder_hidden` (hiding the current default folder refused).
    // ============================================================================
    mod default_folder_dropdown_tests {
        use super::*;
        use tauri::Manager;

        fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
            let fs_dir = TempDir::new().expect("scratch fs dir");
            unsafe { std::env::set_var("PANE_TEST_ROOT", fs_dir.path()) };
            let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
            crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path) \
                 VALUES ('test-agent', 'Test Agent', 'test-agent', 'main', 1, 1, 'test-agent')",
                [],
            )
            .expect("seed test agent");
            let app = tauri::test::mock_app();
            app.manage(crate::database::DbState::new(conn));
            (app, fs_dir)
        }

        fn teardown() {
        }

        fn make_project(db_state: &State<DbState>, name: &str) -> Project {
            create_project_internal(
                &CreateProjectRequest {
                    name: name.to_string(),
                    description: None,
                    color: None,
                    category: None,
                    first_folder_name: None,
                    is_favorite: None,
                    agent_id: "test-agent".to_string(),
                },
                db_state,
            )
            .unwrap()
        }

        fn default_folder_id_of(conn: &rusqlite::Connection, project_id: &str) -> String {
            conn.query_row(
                "SELECT default_folder_id FROM projects WHERE id = ?1",
                params![project_id],
                |row| row.get(0),
            )
            .unwrap()
        }

        /// Ruling 4 (cross-project): a real folder id that belongs to a
        /// DIFFERENT project must be refused, not silently accepted because
        /// it happens to be a real `projects.id`.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_update_project_refuses_cross_project_default_folder() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project_a = make_project(&db_state, "Project A");
            let project_b = make_project(&db_state, "Project B");
            let b_folder_id = {
                let conn = db_state.0.lock().unwrap();
                default_folder_id_of(&conn, &project_b.id)
            };

            let result = update_project(
                UpdateProjectRequest {
                    project_id: project_a.id.clone(),
                    name: None,
                    description: None,
                    color: None,
                    default_folder_id: Some(b_folder_id.clone()),
                },
                db_state.clone(),
            );

            assert!(result.is_err(), "a folder belonging to a different project must be refused");
            assert!(result.unwrap_err().contains("does not belong to project"));

            // After: project A's own default_folder_id is untouched.
            let conn = db_state.0.lock().unwrap();
            let unchanged = default_folder_id_of(&conn, &project_a.id);
            assert_ne!(unchanged, b_folder_id, "a refused change must not write anything");
            drop(conn);
            teardown();
        }

        /// Ruling 4 (system folder): a system folder can never be chosen as
        /// the default, even though it's a real folder in this exact project.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_update_project_refuses_system_folder_as_default() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "System Folder Test");
            let system_folder_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, is_system, created_at, updated_at) \
                     VALUES (?1, 'memory', ?2, 'user_local', 1, datetime('now'), datetime('now'))",
                    params![&system_folder_id, &project.id],
                )
                .unwrap();
            }

            let result = update_project(
                UpdateProjectRequest {
                    project_id: project.id.clone(),
                    name: None,
                    description: None,
                    color: None,
                    default_folder_id: Some(system_folder_id),
                },
                db_state.clone(),
            );

            assert!(result.is_err(), "a system folder must be refused as the default folder");
            assert!(result.unwrap_err().contains("system folder"));
            teardown();
        }

        /// Ruling 4 (hidden folder): a hidden folder can never be chosen as
        /// the default — it would become a folder the dropdown itself can't
        /// even show as "currently selected" without un-hiding it first.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_update_project_refuses_hidden_folder_as_default() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Hidden Folder Test");
            let hidden_folder = create_folder_internal(
                CreateFolderRequest {
                    project_id: project.id.clone(),
                    name: "Hidden".to_string(),
                    parent_id: None,
                },
                db_state.clone(),
            )
            .unwrap();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "UPDATE projects SET is_hidden = 1 WHERE id = ?1",
                    params![&hidden_folder.id],
                )
                .unwrap();
            }

            let result = update_project(
                UpdateProjectRequest {
                    project_id: project.id.clone(),
                    name: None,
                    description: None,
                    color: None,
                    default_folder_id: Some(hidden_folder.id),
                },
                db_state.clone(),
            );

            assert!(result.is_err(), "a hidden folder must be refused as the default folder");
            assert!(result.unwrap_err().contains("hidden folder"));
            teardown();
        }

        /// Ruling 5 (the inverse guard): hiding the CURRENT default folder
        /// must itself be refused by `toggle_folder_hidden` — found by
        /// walking to where hiding happens, per the task's instruction, not
        /// by re-deriving the default-folder check a third time.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_toggle_folder_hidden_refuses_to_hide_the_default_folder() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Hide Default Test");
            let default_folder_id = {
                let conn = db_state.0.lock().unwrap();
                default_folder_id_of(&conn, &project.id)
            };

            let result = toggle_folder_hidden(default_folder_id.clone(), true, db_state.clone());

            assert!(result.is_err(), "hiding the current default folder must be refused");
            assert!(result.unwrap_err().contains("default folder"));

            // After: the folder's is_hidden flag was never written.
            let conn = db_state.0.lock().unwrap();
            let is_hidden: i64 = conn
                .query_row(
                    "SELECT COALESCE(is_hidden, 0) FROM projects WHERE id = ?1",
                    params![&default_folder_id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(is_hidden, 0, "a refused hide must not write is_hidden");
            drop(conn);
            teardown();
        }

        /// A valid change: choosing a different, ordinary (non-system,
        /// non-hidden) folder in the SAME project succeeds, and every other
        /// consumer of `default_folder_id` (here: a NEW note created with no
        /// explicit folder) now resolves to the NEW default, not the old one.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_update_project_sets_default_folder_and_new_default_is_used() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Valid Change Test");
            let old_default_folder_id = {
                let conn = db_state.0.lock().unwrap();
                default_folder_id_of(&conn, &project.id)
            };
            let new_folder = create_folder_internal(
                CreateFolderRequest {
                    project_id: project.id.clone(),
                    name: "New Default".to_string(),
                    parent_id: None,
                },
                db_state.clone(),
            )
            .unwrap();

            let updated = update_project(
                UpdateProjectRequest {
                    project_id: project.id.clone(),
                    name: None,
                    description: None,
                    color: None,
                    default_folder_id: Some(new_folder.id.clone()),
                },
                db_state.clone(),
            )
            .expect("a valid same-project, non-system, non-hidden folder must be accepted");

            assert_eq!(
                updated.default_folder_id.as_deref(),
                Some(new_folder.id.as_str()),
                "the returned project row must reflect the new default_folder_id"
            );
            assert_ne!(new_folder.id, old_default_folder_id);

            // A new note created with no explicit folder_id now resolves to
            // the NEW default — proving the pointer is actually USED by a
            // real downstream consumer (create_note_internal), not just
            // stored.
            let conn = db_state.0.lock().unwrap();
            let note = crate::notes::create_note_internal(
                &conn,
                crate::notes::CreateNoteRequest {
                    project_id: project.id.clone(),
                    folder_id: None,
                    title: "Lands In New Default".to_string(),
                    content: Some("hi".to_string()),
                    tags: None,
                    status: Some("cabinet".to_string()),
                    priority: None,
                    due_date: None,
                    parent_id: None,
                },
            )
            .expect("create_note_internal must succeed");
            assert_eq!(
                note.folder_id.as_deref(),
                Some(new_folder.id.as_str()),
                "a note created with no explicit folder must land in the NEW default folder"
            );

            drop(conn);
            teardown();
        }
    }

    // ============================================================================
    // #2874 item 65: deleting a folder moves its content to the default
    // folder (DB + disk, including trashed items), filename clashes are
    // renamed never overwritten, a failed move leaves the folder intact,
    // folders never nest, and exact §34 event counts fire.
    // ============================================================================
    mod folder_delete_moves_content_tests {
        use super::*;
        use tauri::{Listener, Manager};

        fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
            let fs_dir = TempDir::new().expect("scratch fs dir");
            unsafe { std::env::set_var("PANE_TEST_ROOT", fs_dir.path()) };
            let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
            crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path) \
                 VALUES ('test-agent', 'Test Agent', 'test-agent', 'main', 1, 1, 'test-agent')",
                [],
            )
            .expect("seed test agent");
            let app = tauri::test::mock_app();
            app.manage(crate::database::DbState::new(conn));
            (app, fs_dir)
        }

        fn teardown() {
        }

        fn make_project(db_state: &State<DbState>, name: &str) -> Project {
            create_project_internal(
                &CreateProjectRequest {
                    name: name.to_string(),
                    description: None,
                    color: None,
                    category: None,
                    first_folder_name: None,
                    is_favorite: None,
                    agent_id: "test-agent".to_string(),
                },
                db_state,
            )
            .unwrap()
        }

        fn default_folder_id_of(conn: &rusqlite::Connection, project_id: &str) -> String {
            conn.query_row(
                "SELECT default_folder_id FROM projects WHERE id = ?1",
                params![project_id],
                |row| row.get(0),
            )
            .unwrap()
        }

        /// Inserts a real note/sheet row directly (full migrated schema), with
        /// an explicit chosen `id` (not a random UUID) so multi-row ordering in
        /// tests below is deterministic (`delete_folder_internal`'s internal
        /// `ORDER BY id` query is otherwise at the mercy of UUID randomness).
        fn insert_note_row(
            conn: &rusqlite::Connection,
            id: &str,
            title: &str,
            file_path: &str,
            project_id: &str,
            folder_id: &str,
            note_type: Option<&str>,
            deleted_at: Option<&str>,
        ) {
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO notes (id, title, file_path, project_id, folder_id, note_type, deleted_at, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![id, title, file_path, project_id, folder_id, note_type, deleted_at, &now],
            )
            .unwrap();
        }

        /// Ruling 1 + 7: a folder holding a real note, a sheet, and a TRASHED
        /// note ends up with all three in the default folder — on disk AND in
        /// the DB — and the source folder (including its on-disk directory) is
        /// gone. Content is asserted, not just paths (a move that truncates
        /// must fail this test). "Nothing goes to the OS trash" is structural
        /// here: this path never calls the `trash` crate for any note/sheet
        /// file — only a plain rename (via `move_note_internal`) moves file
        /// content, and only an empty, already-emptied directory is ever
        /// removed.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_folder_moves_notes_sheets_and_trashed_note_to_default_folder() {
            let (app, fs_dir) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Delete Move Test");
            let projects_root = crate::database::get_projects_path();

            let default_folder_id = {
                let conn = db_state.0.lock().unwrap();
                default_folder_id_of(&conn, &project.id)
            };
            let sibling_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                     VALUES (?1, 'ToDelete', ?2, 'user_local', datetime('now'), datetime('now'))",
                    params![&sibling_id, &project.id],
                )
                .unwrap();
            }

            let sibling_dir = projects_root.join("Delete Move Test").join("ToDelete");
            std::fs::create_dir_all(&sibling_dir).unwrap();
            std::fs::write(sibling_dir.join("note-a.md"), "NOTE A CONTENT").unwrap();
            std::fs::write(sibling_dir.join("note-b-trashed.md"), "TRASHED NOTE CONTENT").unwrap();

            {
                let conn = db_state.0.lock().unwrap();
                insert_note_row(
                    &conn, "note-a", "Note A",
                    "Projects/Delete Move Test/ToDelete/note-a.md",
                    &project.id, &sibling_id, None, None,
                );
                insert_note_row(
                    &conn, "note-b-trashed", "Trashed Note",
                    "Projects/Delete Move Test/ToDelete/note-b-trashed.md",
                    &project.id, &sibling_id, None, Some("2026-10-01T00:00:00Z"),
                );
                insert_note_row(
                    &conn, "sheet-c", "Sheet C",
                    "",
                    &project.id, &sibling_id, Some("sheet"), None,
                );
            }

            let outcome = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&sibling_id, &conn).expect("delete must succeed")
            };

            let mut moved_sorted = outcome.moved.clone();
            moved_sorted.sort();
            assert_eq!(
                moved_sorted,
                vec![
                    ("note-a".to_string(), crate::refresh_event::ENTITY_NOTE),
                    ("note-b-trashed".to_string(), crate::refresh_event::ENTITY_NOTE),
                    ("sheet-c".to_string(), crate::refresh_event::ENTITY_SHEET),
                ],
                "exactly these three items must be reported moved, with the right entity types"
            );

            let conn = db_state.0.lock().unwrap();

            // Folder row itself is gone.
            let folder_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", params![&sibling_id], |r| r.get(0))
                .unwrap();
            assert_eq!(folder_count, 0, "after: the deleted folder's row must be gone");

            // Every item's DB row now points at the default folder.
            for id in ["note-a", "note-b-trashed", "sheet-c"] {
                let folder_id: Option<String> = conn
                    .query_row("SELECT folder_id FROM notes WHERE id = ?1", params![id], |r| r.get(0))
                    .unwrap();
                assert_eq!(
                    folder_id.as_deref(),
                    Some(default_folder_id.as_str()),
                    "after: '{id}' must be in the default folder (by ID)"
                );
            }

            // The trashed note is STILL trashed (the move must not touch
            // deleted_at either way).
            let trashed_deleted_at: Option<String> = conn
                .query_row("SELECT deleted_at FROM notes WHERE id = 'note-b-trashed'", [], |r| r.get(0))
                .unwrap();
            assert!(trashed_deleted_at.is_some(), "after: the trashed note must remain trashed");

            // Disk: both real files physically exist in the default folder,
            // with their CONTENT intact (not just their path moved/truncated).
            let default_dir = projects_root.join("Delete Move Test").join("General");
            assert_eq!(
                std::fs::read_to_string(default_dir.join("note-a.md")).expect("note-a must exist in default folder"),
                "NOTE A CONTENT"
            );
            assert_eq!(
                std::fs::read_to_string(default_dir.join("note-b-trashed.md")).expect("trashed note must exist in default folder"),
                "TRASHED NOTE CONTENT"
            );

            // Disk: the deleted folder's directory (and its old file paths) are
            // entirely gone — no leftover empty directory, no stray files.
            assert!(!sibling_dir.exists(), "after: the deleted folder's directory must be gone");

            drop(conn);
            drop(fs_dir);
            teardown();
        }

        /// Lane 1 PR 3 (#2874 items 65/3a + the 2026-10-16 audit's "testing-rule
        /// gap"): the SAME scenario as the test immediately above, rebuilt
        /// through the REAL command path instead of a raw `INSERT` + manual
        /// `fs::write` fixture. The old fixture manually set `deleted_at` AND
        /// wrote the file to disk in the same step — a state the real
        /// `delete_note` command never actually produced before #3037 (it used
        /// to ALSO move the file to the OS trash at soft-delete time, so by the
        /// time a real trashed note reached `delete_folder`, its file was
        /// already gone and the move failed). #3037 is merged as of this PR's
        /// base (soft delete is DB-only, file stays in place) — this test
        /// proves `delete_folder` tolerates a note trashed through the REAL
        /// `delete_note` path cleanly, not an inferred/hand-built one.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_folder_with_note_trashed_via_real_delete_note_command_succeeds() {
            let (app, fs_dir) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Real Trash Delete Test");
            let pane_root = crate::database::get_pane_root();

            let default_folder_id = {
                let conn = db_state.0.lock().unwrap();
                default_folder_id_of(&conn, &project.id)
            };

            // Real folder, created the same way the UI's "New Folder" does.
            let folder = create_folder_internal(
                CreateFolderRequest {
                    project_id: project.id.clone(),
                    name: "ToDelete".to_string(),
                    parent_id: None,
                },
                db_state.clone(),
            )
            .expect("create_folder_internal must succeed");

            // Real note, created through create_note_internal — the same
            // function both `create_note` (the Tauri command) and the AI
            // `create_note` tool call.
            let note = {
                let conn = db_state.0.lock().unwrap();
                crate::notes::create_note_internal(
                    &conn,
                    crate::notes::CreateNoteRequest {
                        project_id: project.id.clone(),
                        folder_id: Some(folder.id.clone()),
                        title: "Real Note".to_string(),
                        content: Some("REAL NOTE CONTENT".to_string()),
                        tags: None,
                        status: Some("cabinet".to_string()),
                        priority: None,
                        due_date: None,
                        parent_id: None,
                    },
                )
                .expect("create_note_internal must succeed")
            };

            // Real soft-delete — the exact shared function the `delete_note`
            // Tauri command calls. Post-#3037: DB-only tombstone, file stays
            // in place (this is the behavior item 1 of the 2026-10-16 audit
            // fixed; #3037 is merged in this PR's base).
            {
                let conn = db_state.0.lock().unwrap();
                crate::notes::soft_delete_note_internal(&conn, &note.id)
                    .map_err(|_| ())
                    .expect("soft_delete_note_internal must succeed");
            }

            // Precondition: the file must still be on disk after a real soft
            // delete (if #3037 ever regressed, THIS assertion should fail
            // first, not the delete_folder one below).
            let note_file_path: String = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row(
                    "SELECT file_path FROM notes WHERE id = ?1",
                    params![&note.id],
                    |r| r.get(0),
                )
                .unwrap()
            };
            assert!(
                pane_root.join(&note_file_path).exists(),
                "precondition: a soft-deleted note's file must still exist on disk (#3037)"
            );

            // The real command under test: delete the folder holding the
            // now-trashed note.
            let outcome = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&folder.id, &conn)
            };
            assert!(
                outcome.is_ok(),
                "deleting a folder holding a trashed note must now succeed cleanly, got: {:?}",
                outcome.err()
            );

            let conn = db_state.0.lock().unwrap();
            let (new_folder_id, deleted_at, new_file_path): (String, Option<String>, String) = conn
                .query_row(
                    "SELECT folder_id, deleted_at, file_path FROM notes WHERE id = ?1",
                    params![&note.id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert_eq!(
                new_folder_id,
                default_folder_id,
                "after: the trashed note must land in the default folder"
            );
            assert!(
                deleted_at.is_some(),
                "after: the note must remain trashed (deleted_at preserved by the move)"
            );
            assert_eq!(
                std::fs::read_to_string(pane_root.join(&new_file_path))
                    .expect("relocated file must exist with content intact"),
                "REAL NOTE CONTENT"
            );
            let folder_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM projects WHERE id = ?1",
                    params![&folder.id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(folder_count, 0, "after: the deleted folder's row must be gone");

            drop(conn);
            drop(fs_dir);
            teardown();
        }

        /// #2874: `delete_folder_internal`'s forced relocation-to-the-default-
        /// folder must keep working even for a SYSTEM-managed note caught in
        /// a folder being deleted. This is reachable on an upgraded install —
        /// before this PR's guard existed, nothing stopped a system note's
        /// `folder_id` from being pointed at an arbitrary, later-deletable
        /// folder via `move_note`/`update_note`/the AI move tool, and that
        /// historical data must not become permanently stuck (an
        /// unconditional guard on `move_note_internal` would make this
        /// folder undeletable forever). `move_note_internal`'s
        /// `enforce_system_guard = false` at this call site (see the comment
        /// right above it) is exactly what keeps this working — this test
        /// seeds one note of EVERY `SYSTEM_NOTE_TYPES` entry (plus one plain
        /// note) into the same folder and proves `delete_folder_internal`
        /// relocates all of them, with content intact.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_folder_relocates_every_system_note_type_to_default_folder() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "System Note Delete Test");
            let projects_root = crate::database::get_projects_path();

            let default_folder_id = {
                let conn = db_state.0.lock().unwrap();
                default_folder_id_of(&conn, &project.id)
            };
            let sibling_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                     VALUES (?1, 'ToDelete', ?2, 'user_local', datetime('now'), datetime('now'))",
                    params![&sibling_id, &project.id],
                )
                .unwrap();
            }

            let sibling_dir = projects_root.join("System Note Delete Test").join("ToDelete");
            std::fs::create_dir_all(&sibling_dir).unwrap();

            // One note per system note_type, simulating a legacy/pre-guard
            // move that already happened — plus one plain note as a control.
            let mut note_ids: Vec<(String, &str)> = Vec::new();
            {
                let conn = db_state.0.lock().unwrap();
                for note_type in crate::notes::SYSTEM_NOTE_TYPES {
                    let note_id = format!("sys-note-{}", note_type);
                    let file_name = format!("{}.md", note_type);
                    let content = format!("SENTINEL CONTENT FOR {}", note_type);
                    std::fs::write(sibling_dir.join(&file_name), &content).unwrap();
                    insert_note_row(
                        &conn,
                        &note_id,
                        note_type,
                        &format!("Projects/System Note Delete Test/ToDelete/{}", file_name),
                        &project.id,
                        &sibling_id,
                        Some(note_type),
                        None,
                    );
                    note_ids.push((note_id, note_type));
                }
                std::fs::write(sibling_dir.join("plain.md"), "PLAIN CONTENT").unwrap();
                insert_note_row(
                    &conn,
                    "plain-note",
                    "Plain",
                    "Projects/System Note Delete Test/ToDelete/plain.md",
                    &project.id,
                    &sibling_id,
                    None,
                    None,
                );
            }

            let outcome = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&sibling_id, &conn)
                    .expect("delete must succeed despite system notes in the folder")
            };
            assert_eq!(
                outcome.moved.len(),
                note_ids.len() + 1,
                "every system note plus the plain note must be reported moved"
            );

            let conn = db_state.0.lock().unwrap();

            // Folder row itself is gone — the guard did not block the delete.
            let folder_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", params![&sibling_id], |r| r.get(0))
                .unwrap();
            assert_eq!(folder_count, 0, "after: the deleted folder's row must be gone");

            let default_dir = projects_root.join("System Note Delete Test").join("General");
            for (note_id, note_type) in &note_ids {
                let folder_id: Option<String> = conn
                    .query_row("SELECT folder_id FROM notes WHERE id = ?1", params![note_id], |r| r.get(0))
                    .unwrap();
                assert_eq!(
                    folder_id.as_deref(),
                    Some(default_folder_id.as_str()),
                    "after: system note_type '{}' must be in the default folder (by ID)",
                    note_type
                );
                let expected_content = format!("SENTINEL CONTENT FOR {}", note_type);
                let on_disk = std::fs::read_to_string(default_dir.join(format!("{}.md", note_type)))
                    .unwrap_or_else(|e| panic!("system note_type '{}' must exist in default folder on disk: {}", note_type, e));
                assert_eq!(
                    on_disk, expected_content,
                    "system note_type '{}': relocated file content must be unchanged",
                    note_type
                );
            }
            let plain_folder_id: Option<String> = conn
                .query_row("SELECT folder_id FROM notes WHERE id = 'plain-note'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(plain_folder_id.as_deref(), Some(default_folder_id.as_str()), "after: the plain note must also be in the default folder");
            assert_eq!(
                std::fs::read_to_string(default_dir.join("plain.md")).unwrap(),
                "PLAIN CONTENT",
                "after: the plain note's content must be unchanged"
            );

            assert!(!sibling_dir.exists(), "after: the deleted folder's directory must be gone");

            drop(conn);
            teardown();
        }

        /// Ruling 1: restoring a note that was trashed BEFORE its folder was
        /// deleted must land it in the default folder — proven through the
        /// real `restore_note` command, not a raw SQL assertion.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_restore_trashed_note_after_folder_delete_lands_in_default_folder() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Restore After Delete Test");
            let projects_root = crate::database::get_projects_path();

            let default_folder_id = {
                let conn = db_state.0.lock().unwrap();
                default_folder_id_of(&conn, &project.id)
            };
            let sibling_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                     VALUES (?1, 'ToDelete', ?2, 'user_local', datetime('now'), datetime('now'))",
                    params![&sibling_id, &project.id],
                )
                .unwrap();
            }

            let sibling_dir = projects_root.join("Restore After Delete Test").join("ToDelete");
            std::fs::create_dir_all(&sibling_dir).unwrap();
            std::fs::write(sibling_dir.join("trashed.md"), "TRASHED CONTENT").unwrap();
            {
                let conn = db_state.0.lock().unwrap();
                insert_note_row(
                    &conn, "trashed-note", "Trashed",
                    "Projects/Restore After Delete Test/ToDelete/trashed.md",
                    &project.id, &sibling_id, None, Some("2026-10-01T00:00:00Z"),
                );
            }

            {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&sibling_id, &conn).expect("delete must succeed");
            }

            let restored = crate::notes::restore_note("trashed-note".to_string(), db_state.clone())
                .expect("restore must succeed");
            assert_eq!(
                restored.meta.folder_id.as_deref(),
                Some(default_folder_id.as_str()),
                "after: restoring a note whose folder was deleted must land it in the default folder"
            );
            assert!(restored.meta.deleted_at.is_none(), "after: restore must clear deleted_at");

            teardown();
        }

        /// Ruling 3: a filename clash in the default folder must never
        /// overwrite — proven at the full `delete_folder_internal` level (not
        /// just `move_note_internal` directly), with both files' CONTENT
        /// asserted.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_folder_filename_clash_renames_never_overwrites() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Clash Test");
            let projects_root = crate::database::get_projects_path();

            let default_dir = projects_root.join("Clash Test").join("General");
            std::fs::create_dir_all(&default_dir).unwrap();
            std::fs::write(default_dir.join("shared.md"), "PRE-EXISTING DEFAULT CONTENT").unwrap();

            let sibling_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                     VALUES (?1, 'ToDelete', ?2, 'user_local', datetime('now'), datetime('now'))",
                    params![&sibling_id, &project.id],
                )
                .unwrap();
            }
            let sibling_dir = projects_root.join("Clash Test").join("ToDelete");
            std::fs::create_dir_all(&sibling_dir).unwrap();
            std::fs::write(sibling_dir.join("shared.md"), "INCOMING MOVED CONTENT").unwrap();
            {
                let conn = db_state.0.lock().unwrap();
                insert_note_row(
                    &conn, "clash-note", "Shared Name",
                    "Projects/Clash Test/ToDelete/shared.md",
                    &project.id, &sibling_id, None, None,
                );
            }

            {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&sibling_id, &conn).expect("delete must succeed despite the clash");
            }

            assert_eq!(
                std::fs::read_to_string(default_dir.join("shared.md")).unwrap(),
                "PRE-EXISTING DEFAULT CONTENT",
                "after: the pre-existing default-folder file must be untouched"
            );
            assert_eq!(
                std::fs::read_to_string(default_dir.join("shared (2).md")).unwrap(),
                "INCOMING MOVED CONTENT",
                "after: the moved note's own content must survive intact at a non-colliding path"
            );

            let conn = db_state.0.lock().unwrap();
            let after_file_path: String = conn
                .query_row("SELECT file_path FROM notes WHERE id = 'clash-note'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(after_file_path, "Projects/Clash Test/General/shared (2).md");

            drop(conn);
            teardown();
        }

        /// Ruling 2: a failed move leaves the folder intact and loses nothing.
        /// Real injected failure (not a simulated boundary): the SECOND note's
        /// underlying file is deleted out from under Pane before the delete
        /// call, so its own move hits a genuine `authorize_existing` I/O
        /// failure mid-loop. The FIRST note, already successfully relocated by
        /// that point, must be rolled all the way back — on disk AND in the
        /// DB — to its exact original location and content.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_failed_move_leaves_folder_intact_and_rolls_back_prior_moves() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Failure Test");
            let projects_root = crate::database::get_projects_path();

            let sibling_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                     VALUES (?1, 'ToDelete', ?2, 'user_local', datetime('now'), datetime('now'))",
                    params![&sibling_id, &project.id],
                )
                .unwrap();
            }
            let sibling_dir = projects_root.join("Failure Test").join("ToDelete");
            std::fs::create_dir_all(&sibling_dir).unwrap();
            std::fs::write(sibling_dir.join("note-a.md"), "NOTE A ORIGINAL CONTENT").unwrap();
            std::fs::write(sibling_dir.join("note-b.md"), "NOTE B CONTENT").unwrap();
            {
                let conn = db_state.0.lock().unwrap();
                // Deliberately chosen ids so `ORDER BY id` processes "note-a"
                // before "note-b" — deterministic, not left to UUID chance.
                insert_note_row(
                    &conn, "note-a", "Note A",
                    "Projects/Failure Test/ToDelete/note-a.md",
                    &project.id, &sibling_id, None, None,
                );
                insert_note_row(
                    &conn, "note-b", "Note B",
                    "Projects/Failure Test/ToDelete/note-b.md",
                    &project.id, &sibling_id, None, None,
                );
            }

            // REAL injected failure: note-b's file is gone from disk, but its
            // DB row still claims it exists there — a genuine desync, not a
            // mocked/simulated error at some abstraction boundary.
            std::fs::remove_file(sibling_dir.join("note-b.md")).unwrap();

            let result = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&sibling_id, &conn)
            };
            assert!(result.is_err(), "the delete must fail because note-b's file is genuinely missing");

            let conn = db_state.0.lock().unwrap();

            // The folder itself must still exist — refused, not partially
            // completed.
            let folder_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", params![&sibling_id], |r| r.get(0))
                .unwrap();
            assert_eq!(folder_count, 1, "after a failed delete: the folder row must still exist");

            // note-a (which DID successfully move before note-b failed) must be
            // rolled all the way back: same folder_id, same file_path as before.
            let (note_a_folder, note_a_path): (Option<String>, String) = conn
                .query_row(
                    "SELECT folder_id, file_path FROM notes WHERE id = 'note-a'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                note_a_folder.as_deref(), Some(sibling_id.as_str()),
                "after rollback: note-a's folder_id must be back to the (still-existing) original folder"
            );
            assert_eq!(
                note_a_path, "Projects/Failure Test/ToDelete/note-a.md",
                "after rollback: note-a's file_path must be back to its original path"
            );

            // And the physical file must ACTUALLY be back at the original path,
            // with its content intact — not stranded in the default folder.
            assert_eq!(
                std::fs::read_to_string(sibling_dir.join("note-a.md")).expect("note-a's file must be back at its original path"),
                "NOTE A ORIGINAL CONTENT",
                "after rollback: note-a's content must be fully intact, nothing lost"
            );
            let default_dir = projects_root.join("Failure Test").join("General");
            assert!(
                !default_dir.join("note-a.md").exists(),
                "after rollback: note-a must NOT be left stranded in the default folder"
            );

            drop(conn);
            teardown();
        }

        /// Ruling 6: `create_folder_internal` must refuse a parent that is
        /// itself a folder (not a top-level project) — folders never nest.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_create_folder_refuses_a_folder_as_parent() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Nest Guard Test");

            let existing_folder = create_folder_internal(
                CreateFolderRequest {
                    project_id: project.id.clone(),
                    name: "Existing".to_string(),
                    parent_id: None,
                },
                db_state.clone(),
            )
            .expect("creating a top-level-parented folder must succeed");

            // Attempt to create a folder whose parent is ANOTHER folder, not
            // the top-level project.
            let result = create_folder_internal(
                CreateFolderRequest {
                    project_id: project.id.clone(),
                    name: "Nested".to_string(),
                    parent_id: Some(existing_folder.id.clone()),
                },
                db_state.clone(),
            );
            assert!(result.is_err(), "a folder-as-parent must be refused");
            assert_eq!(
                result.unwrap_err(),
                "Parent must be a top-level project — folders cannot nest"
            );

            let child_count: i64 = {
                let conn = db_state.0.lock().unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM projects WHERE parent_id = ?1",
                    params![&existing_folder.id],
                    |row| row.get(0),
                )
                .unwrap()
            };
            assert_eq!(child_count, 0, "after: the refused create must leave no child folder under 'Existing'");

            teardown();
        }

        /// Ruling 5: the default folder must stay undeletable — kept/confirmed
        /// here too (in addition to the pre-existing
        /// `test_delete_folder_internal_refuses_to_delete_default_folder`),
        /// using the NEW `DeleteFolderOutcome`-returning signature, so a future
        /// signature change to `delete_folder_internal` can't silently drop
        /// this guard without a type error surfacing here.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_default_folder_still_refused_with_new_return_type() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Default Guard Retained Test");
            let default_folder_id = {
                let conn = db_state.0.lock().unwrap();
                default_folder_id_of(&conn, &project.id)
            };

            let result = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&default_folder_id, &conn)
            };
            assert_eq!(result, Err("Cannot delete the default folder".to_string()));

            teardown();
        }

        /// Ruling 7: exact §34 event counts — one `moved` per note/sheet, plus
        /// exactly one `deleted` for the folder. Drives the REAL
        /// `delete_folder_internal` (two notes + one sheet) and the REAL
        /// `emit_delete_folder_events` helper the `#[tauri::command]` wrapper
        /// calls — against `tauri::test::mock_app()`'s `AppHandle<MockRuntime>`
        /// with a live listener, asserting the actual emitted event count and
        /// shape, not a source-text call-count proxy.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_folder_emits_exact_event_counts() {
            let (app, _fs) = setup();
            let handle = app.handle();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Event Count Test");
            let projects_root = crate::database::get_projects_path();

            let sibling_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                     VALUES (?1, 'ToDelete', ?2, 'user_local', datetime('now'), datetime('now'))",
                    params![&sibling_id, &project.id],
                )
                .unwrap();
            }
            let sibling_dir = projects_root.join("Event Count Test").join("ToDelete");
            std::fs::create_dir_all(&sibling_dir).unwrap();
            std::fs::write(sibling_dir.join("note-a.md"), "A").unwrap();
            std::fs::write(sibling_dir.join("note-b.md"), "B").unwrap();
            {
                let conn = db_state.0.lock().unwrap();
                insert_note_row(&conn, "note-a", "A", "Projects/Event Count Test/ToDelete/note-a.md", &project.id, &sibling_id, None, None);
                insert_note_row(&conn, "note-b", "B", "Projects/Event Count Test/ToDelete/note-b.md", &project.id, &sibling_id, None, None);
                insert_note_row(&conn, "sheet-c", "C", "", &project.id, &sibling_id, Some("sheet"), None);
            }

            let fired: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let fired2 = fired.clone();
            handle.listen(crate::refresh_event::EVENT_DATA_CHANGED, move |event| {
                let payload: serde_json::Value = serde_json::from_str(event.payload()).unwrap();
                fired2.lock().unwrap().push(payload);
            });

            let outcome = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&sibling_id, &conn).expect("delete must succeed")
            };
            emit_delete_folder_events(handle, &sibling_id, &outcome);

            let events = fired.lock().unwrap();
            assert_eq!(
                events.len(), 4,
                "must fire exactly 3 moved + 1 deleted = 4 events, got: {:?}", *events
            );

            let moved_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["reason"] == "moved").collect();
            assert_eq!(moved_events.len(), 3, "exactly 3 'moved' events, one per note/sheet");
            let deleted_events: Vec<&serde_json::Value> = events.iter().filter(|e| e["reason"] == "deleted").collect();
            assert_eq!(deleted_events.len(), 1, "exactly 1 'deleted' event, for the folder");
            assert_eq!(deleted_events[0]["entity_id"], sibling_id);
            assert_eq!(deleted_events[0]["entity_type"], "folder");

            let mut moved_ids: Vec<String> = moved_events.iter().map(|e| e["entity_id"].as_str().unwrap().to_string()).collect();
            moved_ids.sort();
            assert_eq!(moved_ids, vec!["note-a".to_string(), "note-b".to_string(), "sheet-c".to_string()]);

            let sheet_event = moved_events.iter().find(|e| e["entity_id"] == "sheet-c").unwrap();
            assert_eq!(sheet_event["entity_type"], "sheet", "the sheet must emit entity_type 'sheet', not 'note'");
            let note_event = moved_events.iter().find(|e| e["entity_id"] == "note-a").unwrap();
            assert_eq!(note_event["entity_type"], "note");

            teardown();
        }

        // ════════════════════════════════════════════════════════════════
        // Reed's WARN closure (#2874 item 65, 2026-10-12 audit): before this
        // fix, `delete_folder_internal` called `fs_policy.remove_dir_all`
        // on the folder directory unconditionally — recursive, and it never
        // checked the directory was actually empty first. Anything a user
        // dropped into that on-disk folder via Finder that Pane never put
        // a `notes` row for (a PDF, an image, anything untracked) was
        // destroyed permanently, with no OS trash step. Matt's ruling:
        // "nothing Pane didn't move is ever permanently deleted." The fix:
        // every leftover entry is handed to the existing `PaneFs::trash`
        // path first; only once the directory is actually empty is it
        // removed, and only non-recursively (`remove_dir`).
        // ════════════════════════════════════════════════════════════════

        /// Item 1: an untracked file left in the folder goes to the OS
        /// trash, not the void — and a tracked note sharing the same
        /// folder still moves correctly, proving the new leftover handling
        /// runs ALONGSIDE the existing move logic, not instead of it.
        ///
        /// This drives the REAL, unmocked `delete_folder_internal` end to
        /// end, including the real `fs_policy.trash()` call for the
        /// leftover — the file is NOT pre-removed from disk to dodge it
        /// (the dodge Reed flagged — and ruled out of scope for THIS
        /// function — in the pre-existing `delete_project_internal` test
        /// at the time of her audit). A successful run of this test really
        /// does move one small, clearly-named disposable file into this
        /// machine's real OS Trash; that is disclosed, not hidden.
        ///
        /// **Why this test does not itself assert "found in the OS Trash
        /// with byte-identical content," and does not differ in its own
        /// pass/fail outcome between the pre-fix and post-fix code** (full
        /// honesty, per the brief's own allowance for exactly this): both
        /// `remove_dir_all` (pre-fix) and `trash()` + `remove_dir`
        /// (post-fix) make the leftover file disappear from its original
        /// path and both let `delete_folder_internal` return `Ok` for this
        /// single-trashable-leftover scenario — so "gone from source" and
        /// "call succeeds" cannot, by themselves, distinguish "moved to
        /// Trash" from "destroyed". Proving *where* it went for real would
        /// require reading the OS Trash's contents, and every avenue for
        /// that was tried and rejected:
        ///   - the `trash` crate's `os_limited::list()`/metadata API, which
        ///     would otherwise let a test enumerate+read trashed items
        ///     portably, is explicitly `#[cfg]`-excluded on macOS (it only
        ///     exists for Windows/Freedesktop-Trash platforms);
        ///   - directly reading `~/.Trash` via `std::fs` is denied
        ///     (`Operation not permitted`) in this sandboxed dev
        ///     environment, confirmed from a plain compiled Rust binary,
        ///     not just an interactive shell — so there's no guarantee a
        ///     CI runner would be able to either;
        ///   - Finder/AppleScript automation CAN see into the real Trash
        ///     by name and read a specific item's content (confirmed
        ///     manually while developing this fix — see the PR/report for
        ///     the quoted transcript) but deliberately is NOT wired into
        ///     this committed test: a follow-up "permanently remove this
        ///     one test item from the Trash" AppleScript call hung
        ///     indefinitely in this same environment, and embedding ANY
        ///     Finder automation risks a first-run "osascript wants to
        ///     control Finder" permission prompt interrupting whoever's
        ///     screen runs this suite next — an unacceptable cost for a
        ///     routinely-run test, Trash cleanup or not.
        ///
        /// What IS asserted for real, instead: the leftover is gone from
        /// its original path, `delete_folder_internal` succeeds (meaning
        /// the best-effort "leave it in place" failure branch was NOT
        /// taken — see the sibling test below for that branch proven for
        /// real), and the tracked note's move is unaffected. Combined with
        /// `test_delete_folder_leaves_untrashable_leftover_in_place_on_trash_failure`
        /// below — which DOES show a real, quoted, differing-before/after
        /// result, because its failure is forced deterministically at the
        /// `PaneFs` policy layer rather than depending on OS Trash
        /// internals — the mechanism (trash leftovers, best-effort, never
        /// force a recursive delete) is proven for real without that one
        /// specific sub-claim resting on an environment-dependent check.
        ///
        /// #3027 ruling 4: this test writes a real, disposable file into the
        /// DEVELOPER'S REAL `~/.Trash` every time it runs — including in CI
        /// — exactly the problem Matt flagged on this test after #3024
        /// shipped it. Manual-only from here on; the symlink-based failure
        /// test right below this one stays always-on (fully deterministic,
        /// never touches the real OS trash). Run it by hand with:
        ///
        /// ```sh
        /// cargo test --package pane --lib \
        ///   projects::tests::folder_delete_moves_content_tests::test_delete_folder_trashes_untracked_leftover_file_instead_of_deleting_it \
        ///   -- --exact --ignored
        /// ```
        #[test]
        #[ignore = "writes a real file into the developer's ~/.Trash on every run (#3027 ruling 4) — run manually, see doc comment for the exact command"]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_folder_trashes_untracked_leftover_file_instead_of_deleting_it() {
            let (app, fs_dir) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Leftover Trash Test");
            let projects_root = crate::database::get_projects_path();

            let sibling_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                     VALUES (?1, 'ToDelete', ?2, 'user_local', datetime('now'), datetime('now'))",
                    params![&sibling_id, &project.id],
                )
                .unwrap();
            }

            let sibling_dir = projects_root.join("Leftover Trash Test").join("ToDelete");
            std::fs::create_dir_all(&sibling_dir).unwrap();

            // A real, Pane-tracked note sharing the folder — proves the
            // leftover handling doesn't interfere with the normal move.
            std::fs::write(sibling_dir.join("tracked-note.md"), "TRACKED NOTE CONTENT").unwrap();
            {
                let conn = db_state.0.lock().unwrap();
                insert_note_row(
                    &conn, "tracked-note", "Tracked",
                    "Projects/Leftover Trash Test/ToDelete/tracked-note.md",
                    &project.id, &sibling_id, None, None,
                );
            }

            // The untracked leftover: something a user dropped into the
            // on-disk folder via Finder that Pane never created a `notes`
            // row for. Unique name so it's unmistakably a disposable test
            // artifact if it is ever seen in a real Trash listing.
            let leftover_name = format!("pane-test-untracked-leftover-{}.pdf", uuid::Uuid::new_v4());
            let leftover_path = sibling_dir.join(&leftover_name);
            std::fs::write(&leftover_path, "UNTRACKED FINDER-DROPPED CONTENT").unwrap();
            assert!(leftover_path.exists(), "sanity: leftover must exist before delete");

            let result = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&sibling_id, &conn)
            };
            assert!(
                result.is_ok(),
                "delete must still succeed with an untracked leftover present (best-effort, never a hard failure): {:?}",
                result
            );

            let conn = db_state.0.lock().unwrap();
            let folder_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", params![&sibling_id], |r| r.get(0))
                .unwrap();
            assert_eq!(folder_count, 0, "after: the deleted folder's row must be gone");
            drop(conn);

            // The tracked note still moved correctly, content intact.
            let default_dir = projects_root.join("Leftover Trash Test").join("General");
            assert_eq!(
                std::fs::read_to_string(default_dir.join("tracked-note.md"))
                    .expect("tracked note must exist in default folder"),
                "TRACKED NOTE CONTENT"
            );

            // The untracked leftover is gone from its original path (moved
            // to the OS trash, not left behind unprocessed).
            assert!(
                !leftover_path.exists(),
                "after: the untracked leftover must be gone from its original path"
            );

            // Directory is now empty and was removed.
            assert!(!sibling_dir.exists(), "after: the now-empty directory must be removed");

            drop(fs_dir);
            teardown();
        }

        /// Item 2: a folder with nothing left behind (every tracked
        /// note/sheet moved out, no untracked leftovers at all) just has
        /// its now-empty directory removed — nothing is ever handed to
        /// `fs_policy.trash()` in this case, because the leftover-scanning
        /// loop finds zero entries to act on.
        ///
        /// Honesty note: unlike the leftover-file test above, this
        /// scenario produces the IDENTICAL observable outcome under the
        /// pre-fix code too (`remove_dir_all` on an already-empty
        /// directory succeeds exactly the same way `remove_dir` does) —
        /// there is no bug to reproduce here, so this is a regression-
        /// pinning test for the fixed implementation's chosen code path
        /// (confirming `remove_dir`, never `remove_dir_all`, is reachable
        /// and correct for the common case), not a "fails before, passes
        /// after" test. Said plainly rather than manufactured.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_folder_removes_empty_directory_non_recursively_without_trashing_anything() {
            let (app, fs_dir) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Empty Dir Test");
            let projects_root = crate::database::get_projects_path();

            let sibling_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                     VALUES (?1, 'ToDelete', ?2, 'user_local', datetime('now'), datetime('now'))",
                    params![&sibling_id, &project.id],
                )
                .unwrap();
            }

            let sibling_dir = projects_root.join("Empty Dir Test").join("ToDelete");
            std::fs::create_dir_all(&sibling_dir).unwrap();
            std::fs::write(sibling_dir.join("only-note.md"), "ONLY NOTE CONTENT").unwrap();
            {
                let conn = db_state.0.lock().unwrap();
                insert_note_row(
                    &conn, "only-note", "Only",
                    "Projects/Empty Dir Test/ToDelete/only-note.md",
                    &project.id, &sibling_id, None, None,
                );
            }
            // Directory now holds exactly one entry (only-note.md); the
            // move loop relocates it before the leftover-sweep runs, so by
            // the time cleanup happens the directory is genuinely empty —
            // zero leftovers for the sweep to ever call `trash()` on.

            let result = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&sibling_id, &conn)
            };
            assert!(result.is_ok(), "delete must succeed: {:?}", result);

            let default_dir = projects_root.join("Empty Dir Test").join("General");
            assert_eq!(
                std::fs::read_to_string(default_dir.join("only-note.md")).unwrap(),
                "ONLY NOTE CONTENT"
            );
            assert!(
                !sibling_dir.exists(),
                "after: the now-empty directory must be removed non-recursively"
            );

            drop(fs_dir);
            teardown();
        }

        /// Ruling 3 ("it stays best-effort: a failure is logged and leaves
        /// the leftovers in place. Do not fail the delete, do not roll
        /// back the moves"), proven for real and deterministically — no
        /// dependence on the real OS Trash's own success/failure.
        ///
        /// The leftover here is a symlink, which `PaneFs::authorize_existing`
        /// rejects unconditionally ("symlink rejected") BEFORE ever
        /// reaching the real `trash::delete` call — so this test never
        /// touches the real OS Trash at all, and is fully deterministic
        /// and portable. This gives the genuine "fails before the fix,
        /// passes after" evidence the brief asks for: under the pre-fix
        /// code, `remove_dir_all` doesn't care about symlinks or emptiness
        /// — it deletes the whole directory (including the dangling
        /// symlink entry) unconditionally and `delete_folder_internal`
        /// returns `Ok` with the directory gone. Under the fix, the
        /// symlink's `trash()` call fails, the directory is deliberately
        /// LEFT IN PLACE (never forced via a recursive delete), the error
        /// is only logged, and — critically, per the ordering rule —
        /// nothing about the already-committed note moves is undone.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_delete_folder_leaves_untrashable_leftover_in_place_on_trash_failure() {
            let (app, fs_dir) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Untrashable Leftover Test");
            let projects_root = crate::database::get_projects_path();

            let sibling_id = uuid::Uuid::new_v4().to_string();
            {
                let conn = db_state.0.lock().unwrap();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) \
                     VALUES (?1, 'ToDelete', ?2, 'user_local', datetime('now'), datetime('now'))",
                    params![&sibling_id, &project.id],
                )
                .unwrap();
            }

            let sibling_dir = projects_root.join("Untrashable Leftover Test").join("ToDelete");
            std::fs::create_dir_all(&sibling_dir).unwrap();
            std::fs::write(sibling_dir.join("tracked-note.md"), "TRACKED NOTE CONTENT").unwrap();
            {
                let conn = db_state.0.lock().unwrap();
                insert_note_row(
                    &conn, "tracked-note", "Tracked",
                    "Projects/Untrashable Leftover Test/ToDelete/tracked-note.md",
                    &project.id, &sibling_id, None, None,
                );
            }

            // A symlink leftover — `authorize_existing` rejects symlinks
            // outright, so `fs_policy.trash()` deterministically fails on
            // it without ever calling the real `trash` crate.
            let symlink_target = projects_root.join("Untrashable Leftover Test").join("symlink-target.txt");
            std::fs::write(&symlink_target, "symlink target content").unwrap();
            let symlink_path = sibling_dir.join("untrashable-leftover-symlink");
            std::os::unix::fs::symlink(&symlink_target, &symlink_path).unwrap();

            let result = {
                let conn = db_state.0.lock().unwrap();
                delete_folder_internal(&sibling_id, &conn)
            };
            assert!(
                result.is_ok(),
                "delete must still succeed even though a leftover couldn't be trashed (best-effort, never a hard failure): {:?}",
                result
            );

            let conn = db_state.0.lock().unwrap();
            let folder_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", params![&sibling_id], |r| r.get(0))
                .unwrap();
            assert_eq!(folder_count, 0, "after: the deleted folder's DB row must still be gone (fs cleanup failure doesn't block the DB delete)");
            drop(conn);

            // The tracked note's move is NOT undone by the later fs
            // cleanup failure — ordering rule: the move already committed.
            let default_dir = projects_root.join("Untrashable Leftover Test").join("General");
            assert_eq!(
                std::fs::read_to_string(default_dir.join("tracked-note.md"))
                    .expect("tracked note must still be in the default folder"),
                "TRACKED NOTE CONTENT"
            );

            // The untrashable leftover is left exactly in place...
            assert!(
                symlink_path.symlink_metadata().is_ok(),
                "after: the untrashable leftover symlink must be left in place, not deleted"
            );
            // ...and because the directory is therefore NOT empty, it is
            // deliberately NOT removed (never a forced recursive delete).
            assert!(
                sibling_dir.exists(),
                "after: the directory must be left in place since it still isn't empty"
            );

            drop(fs_dir);
            teardown();
        }
    }

    // ════════════════════════════════════════════════════════════════════════
    // #2972 stage 2 (§34.5 WARN closure, PR #3016 / Reed's 2026-10-11 audit):
    // `get_project_or_folder` — the single-entity-by-id read Sidebar,
    // CalendarView and NotesBrowser now patch with instead of refetching a
    // whole list. Exercised through the real `#[tauri::command]` entry point
    // via `tauri::test::mock_app` + `app.manage(DbState)`, same pattern as
    // `default_folder_and_duplicate_name_tests` above — this command's own
    // signature takes no `AppHandle`, so (unlike `create_folder`/
    // `update_folder`) it CAN be invoked directly, not just source-scanned.
    // ════════════════════════════════════════════════════════════════════════
    mod get_project_or_folder_tests {
        use super::*;
        use tauri::Manager;

        fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
            let fs_dir = TempDir::new().expect("scratch fs dir");
            unsafe { std::env::set_var("PANE_TEST_ROOT", fs_dir.path()) };
            let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
            crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path) \
                 VALUES ('test-agent', 'Test Agent', 'test-agent', 'main', 1, 1, 'test-agent')",
                [],
            )
            .expect("seed test agent");
            let app = tauri::test::mock_app();
            app.manage(crate::database::DbState::new(conn));
            (app, fs_dir)
        }

        fn teardown() {
        }

        fn make_project(db_state: &State<DbState>, name: &str) -> Project {
            create_project_internal(
                &CreateProjectRequest {
                    name: name.to_string(),
                    description: None,
                    color: None,
                    category: None,
                    first_folder_name: None,
                    is_favorite: None,
                    agent_id: "test-agent".to_string(),
                },
                db_state,
            )
            .unwrap()
        }

        /// (a) returns a project — same shape `get_projects` would have produced
        /// for this row (field-for-field, since both delegate to the same
        /// `get_project` row-mapper).
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_get_project_or_folder_returns_a_project() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Single Read Project");

            let result = get_project_or_folder(project.id.clone(), app.state::<DbState>()).unwrap();

            assert_eq!(result.id, project.id);
            assert_eq!(result.name, "Single Read Project");
            assert_eq!(result.parent_id, None, "a top-level project has no parent_id");
            assert_eq!(result.is_system, project.is_system);
            assert_eq!(result.is_favorite, project.is_favorite);

            teardown();
        }

        /// (b) returns a folder — a folder is a row in the same `projects` table
        /// with a non-null `parent_id`; the same command, same id lookup, finds
        /// it identically.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_get_project_or_folder_returns_a_folder() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = make_project(&db_state, "Folder Parent Project");
            let folder = create_folder_internal(
                CreateFolderRequest {
                    project_id: project.id.clone(),
                    name: "Single Read Folder".to_string(),
                    parent_id: None,
                },
                app.state::<DbState>(),
            )
            .unwrap();

            let result = get_project_or_folder(folder.id.clone(), app.state::<DbState>()).unwrap();

            assert_eq!(result.id, folder.id);
            assert_eq!(result.name, "Single Read Folder");
            assert_eq!(result.parent_id, Some(project.id.clone()), "a folder's parent_id must point at its parent project");

            teardown();
        }

        /// (c) errors on an unknown id — same "not found" error style as the
        /// private `get_project` helper this command delegates to (and as
        /// `create_project`/`create_folder`'s own post-create read, which hits
        /// the identical error path if the row it just inserted somehow wasn't
        /// there).
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_get_project_or_folder_errors_on_unknown_id() {
            let (app, _fs) = setup();

            let result = get_project_or_folder("does-not-exist".to_string(), app.state::<DbState>());

            assert_eq!(result.unwrap_err(), "Project not found".to_string());

            teardown();
        }
    }

    // #2972 stage 1 (§34.1.4): confirms the Sidebar `create_folder` Tauri
    // command wrapper itself calls `emit_data_changed` exactly once — a
    // source scan because a real `tauri::AppHandle` (`AppHandle<Wry>`)
    // cannot be constructed from `tauri::test::mock_app()`'s
    // `AppHandle<MockRuntime>`, so the wrapper cannot be invoked directly in
    // a unit test (same constraint documented throughout this crate's
    // existing tests, e.g. `notes.rs`'s `test_update_note_title_only_...`).
    // The underlying `create_folder_internal` + `emit_data_changed`
    // BEHAVIOR (the real folder gets created, the real event fires with the
    // right id/type/reason) is covered by
    // `ai::tools::stage1_backend_emit_tests::test_sidebar_create_folder_path_fires_data_changed_exactly_once`
    // — this test only proves the wrapper's wiring to that behavior.
    #[test]
    fn test_create_folder_command_wrapper_emits_data_changed_exactly_once() {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set during `cargo test`");
        let path = std::path::Path::new(&manifest_dir).join("src/projects.rs");
        let content = std::fs::read_to_string(&path).expect("read projects.rs");

        let fn_start = content
            .find("pub fn create_folder(\n    app: tauri::AppHandle,")
            .expect("create_folder command wrapper must exist with this exact signature");
        let after_fn = &content[fn_start..];
        let body_end = after_fn
            .find("\n}\n")
            .expect("create_folder command wrapper body must close with '\\n}\\n'");
        let body = &after_fn[..body_end];

        let occurrences = body.matches("emit_data_changed(").count();
        assert_eq!(
            occurrences, 1,
            "create_folder command wrapper must call emit_data_changed exactly once, found {} call(s)",
            occurrences
        );
        assert!(
            body.contains("create_folder_internal(request, db)"),
            "create_folder command wrapper must delegate to create_folder_internal"
        );
    }

    // ═══════════════════════════════════════════════════════════════════════════
    // #3018 (§34.1/§34.2): rename_category/delete_category return affected
    // project IDs from the same transaction, so the frontend can emit one
    // `project`/`edited` event per affected project without working the
    // list out from frontend state.
    // ═══════════════════════════════════════════════════════════════════════════

    fn setup_category_app() -> tauri::App<tauri::test::MockRuntime> {
        use tauri::Manager;
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    fn insert_category_project(conn: &rusqlite::Connection, id: &str, category: Option<&str>) {
        conn.execute(
            "INSERT INTO projects (id, name, category, owner_id, is_favorite, created_at, updated_at)
             VALUES (?1, ?1, ?2, 'user_local', 0, datetime('now'), datetime('now'))",
            params![id, category],
        )
        .unwrap();
    }

    #[test]
    fn test_rename_category_returns_exactly_the_affected_project_ids() {
        use tauri::Manager;
        let app = setup_category_app();
        let db_state = app.state::<DbState>();
        {
            let conn = db_state.0.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO category_colors (category, color) VALUES ('Work', '#AABBCC')",
                [],
            )
            .unwrap();
            insert_category_project(&conn, "p1", Some("Work"));
            insert_category_project(&conn, "p2", Some("Work"));
            insert_category_project(&conn, "p3", Some("Other"));
            insert_category_project(&conn, "p4", None);
        }

        let mut affected = rename_category(
            "Work".to_string(),
            "Office".to_string(),
            "#AABBCC".to_string(),
            app.state::<DbState>(),
        )
        .expect("rename_category should succeed");
        affected.sort();

        assert_eq!(
            affected,
            vec!["p1".to_string(), "p2".to_string()],
            "rename_category must return exactly the project IDs whose category changed, got {:?}",
            affected
        );

        let conn = db_state.0.lock().unwrap();
        for (id, expected) in &[("p1", "Office"), ("p2", "Office"), ("p3", "Other")] {
            let cat: String = conn
                .query_row(
                    "SELECT category FROM projects WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(&cat, expected);
        }
    }

    #[test]
    fn test_rename_category_returns_empty_vec_when_no_projects_affected() {
        use tauri::Manager;
        let app = setup_category_app();
        let db_state = app.state::<DbState>();
        {
            let conn = db_state.0.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO category_colors (category, color) VALUES ('Lonely', '#AABBCC')",
                [],
            )
            .unwrap();
            // No project ever assigned 'Lonely' — zero projects affected.
        }

        let affected = rename_category(
            "Lonely".to_string(),
            "StillLonely".to_string(),
            "#AABBCC".to_string(),
            app.state::<DbState>(),
        )
        .expect("rename_category should succeed even with zero affected projects");

        assert_eq!(
            affected,
            Vec::<String>::new(),
            "zero projects used this category — affected list must be exactly empty, not merely non-matching"
        );
    }

    #[test]
    fn test_delete_category_returns_exactly_the_affected_project_ids_and_clears_them() {
        use tauri::Manager;
        let app = setup_category_app();
        let db_state = app.state::<DbState>();
        {
            let conn = db_state.0.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO category_colors (category, color) VALUES ('Archive', '#112233')",
                [],
            )
            .unwrap();
            insert_category_project(&conn, "a1", Some("Archive"));
            insert_category_project(&conn, "a2", Some("Archive"));
            insert_category_project(&conn, "a3", Some("Keep"));
        }

        let mut affected = delete_category("Archive".to_string(), app.state::<DbState>())
            .expect("delete_category should succeed");
        affected.sort();

        assert_eq!(
            affected,
            vec!["a1".to_string(), "a2".to_string()],
            "delete_category must return exactly the project IDs that were cleared, got {:?}",
            affected
        );

        let conn = db_state.0.lock().unwrap();
        let a1_cat: Option<String> = conn
            .query_row("SELECT category FROM projects WHERE id = 'a1'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(a1_cat, None, "deleted category's projects must be cleared to NULL");

        let a3_cat: Option<String> = conn
            .query_row("SELECT category FROM projects WHERE id = 'a3'", [], |row| row.get(0))
            .unwrap();
        assert_eq!(a3_cat, Some("Keep".to_string()), "unrelated project's category must be untouched");

        let color_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM category_colors WHERE category = 'Archive'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!color_exists, "category_colors row must be deleted");
    }

    #[test]
    fn test_delete_category_returns_empty_vec_when_no_projects_affected() {
        use tauri::Manager;
        let app = setup_category_app();
        let db_state = app.state::<DbState>();
        {
            let conn = db_state.0.lock().unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO category_colors (category, color) VALUES ('Unused', '#000000')",
                [],
            )
            .unwrap();
        }

        let affected = delete_category("Unused".to_string(), app.state::<DbState>())
            .expect("delete_category should succeed even with zero affected projects");

        assert_eq!(
            affected,
            Vec::<String>::new(),
            "zero projects used this category — affected list must be exactly empty"
        );
    }

    // ══════════════════════════════════════════════════════════════════════════════════
    // #2874 item 4 (docs/audits/2026-10-16-scope-1060-test-failures.md item
    // 4, "New project's General folder missing until 2nd folder exists"):
    // `emit_project_created_events` is the one shared helper BOTH the
    // `create_project` Tauri command and the AI agent's `create_project`
    // tool now call (the agent-side test lives in
    // `ai::tools::stage1_backend_emit_tests`, since that's where
    // `execute_create_project`/`emit_entity_change_for_tool` live). The
    // command wrapper itself takes a concrete `tauri::AppHandle` and can't
    // be invoked directly in a unit test (same constraint as
    // `create_folder` above) — so, same split as that test: a source scan
    // proves the WRAPPER calls the shared helper exactly once, and a real
    // `mock_app` + `app.listen()` test proves the shared helper itself
    // emits BOTH events correctly.
    // ══════════════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_create_project_command_wrapper_calls_emit_project_created_events_exactly_once() {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR must be set during `cargo test`");
        let path = std::path::Path::new(&manifest_dir).join("src/projects.rs");
        let content = std::fs::read_to_string(&path).expect("read projects.rs");

        let fn_start = content
            .find("pub fn create_project(\n    app: tauri::AppHandle,")
            .expect("create_project command wrapper must exist with this exact signature");
        let after_fn = &content[fn_start..];
        let body_end = after_fn
            .find("\n}\n")
            .expect("create_project command wrapper body must close with '\\n}\\n'");
        let body = &after_fn[..body_end];

        let occurrences = body.matches("emit_project_created_events(").count();
        assert_eq!(
            occurrences, 1,
            "create_project command wrapper must call emit_project_created_events exactly once, found {} call(s)",
            occurrences
        );
        assert!(
            !body.contains("emit_data_changed("),
            "create_project command wrapper must route through emit_project_created_events, \
             not call emit_data_changed directly (that's the old single-event bug #2874 item 4 fixes)"
        );
        assert!(
            body.contains("create_project_internal(&request, &db)"),
            "create_project command wrapper must delegate to create_project_internal"
        );
    }

    mod emit_project_created_events_tests {
        use super::*;
        use tauri::{Listener, Manager};

        fn setup() -> (tauri::App<tauri::test::MockRuntime>, TempDir) {
            let fs_dir = TempDir::new().expect("scratch fs dir");
            unsafe { std::env::set_var("PANE_TEST_ROOT", fs_dir.path()) };
            let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
            crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, is_active, workspace_path) \
                 VALUES ('test-agent', 'Test Agent', 'test-agent', 'main', 1, 1, 'test-agent')",
                [],
            )
            .expect("seed test agent");
            let app = tauri::test::mock_app();
            app.manage(DbState::new(conn));
            (app, fs_dir)
        }

        fn teardown() {
        }

        struct EventCapture {
            events: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        }

        impl EventCapture {
            fn install<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Self {
                let events: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
                    std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
                let events2 = events.clone();
                app.listen(crate::refresh_event::EVENT_DATA_CHANGED, move |event| {
                    let payload: serde_json::Value = serde_json::from_str(event.payload())
                        .expect("pane-data-changed payload must be valid JSON");
                    events2.lock().unwrap().push(payload);
                });
                EventCapture { events }
            }

            fn events(&self) -> Vec<serde_json::Value> {
                self.events.lock().unwrap().clone()
            }
        }

        /// Proves the real precondition this item depends on —
        /// `create_project_internal` really does set a real
        /// `default_folder_id` on every project it creates, immediately,
        /// in the same transaction — before testing that the emit helper
        /// announces it.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_create_project_internal_always_sets_a_default_folder_id() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = create_project_internal(
                &CreateProjectRequest {
                    name: "Precondition Project".to_string(),
                    description: None,
                    color: None,
                    category: None,
                    first_folder_name: None,
                    is_favorite: None,
                    agent_id: "test-agent".to_string(),
                },
                &db_state,
            )
            .expect("create_project_internal should succeed");

            assert!(
                project.default_folder_id.is_some(),
                "create_project_internal must always set a default_folder_id"
            );

            let conn = db_state.0.lock().unwrap();
            let folder_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM projects WHERE id = ?1 AND parent_id = ?2",
                    params![project.default_folder_id.as_ref().unwrap(), &project.id],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(folder_exists, "the default_folder_id must point at a real folder row belonging to this project");
            teardown();
        }

        /// The fix itself: a single `create_project_internal` call's result,
        /// fed through `emit_project_created_events`, fires EXACTLY TWO
        /// `pane-data-changed` events — the project (`created`) and its
        /// default folder (`created`) — not one. Before this fix, only the
        /// project's event existed at all; this is the regression test for
        /// the gap item 4 describes (the Sidebar's single-entity patch
        /// handler never learning the folder's id).
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_emits_both_project_and_default_folder_created_events() {
            let (app, _fs) = setup();
            let db_state = app.state::<DbState>();
            let project = create_project_internal(
                &CreateProjectRequest {
                    name: "Two Event Project".to_string(),
                    description: None,
                    color: None,
                    category: None,
                    first_folder_name: None,
                    is_favorite: None,
                    agent_id: "test-agent".to_string(),
                },
                &db_state,
            )
            .expect("create_project_internal should succeed");
            let default_folder_id = project.default_folder_id.clone().expect("precondition test above proves this is always Some");

            let capture = EventCapture::install(app.handle());
            emit_project_created_events(app.handle(), &project.id, Some(default_folder_id.as_str()));

            let events = capture.events();
            assert_eq!(
                events.len(),
                2,
                "creating a project must fire exactly two pane-data-changed events (project + default folder), got: {:?}",
                events
            );
            assert_eq!(events[0]["entity_id"], project.id, "event 0 must be the project's own id");
            assert_eq!(events[0]["entity_type"], "project");
            assert_eq!(events[0]["reason"], "created");

            assert_eq!(events[1]["entity_id"], default_folder_id, "event 1 must be the default folder's own id, not the project's");
            assert_eq!(events[1]["entity_type"], "folder");
            assert_eq!(events[1]["reason"], "created");

            teardown();
        }

        /// Defensive: a project somehow created with no default folder (not
        /// possible via `create_project_internal` today, per the
        /// precondition test above, but the helper's signature accepts
        /// `Option<&str>` deliberately) must still emit the project's own
        /// event — and must NOT emit a bogus/empty folder event.
        #[test]
        #[serial_test::serial(pane_test_root)]
        fn test_no_default_folder_id_emits_only_the_project_event() {
            let (app, _fs) = setup();
            let capture = EventCapture::install(app.handle());

            emit_project_created_events(app.handle(), "standalone-project-id", None);

            let events = capture.events();
            assert_eq!(events.len(), 1, "with no default_folder_id, only the project event must fire, got: {:?}", events);
            assert_eq!(events[0]["entity_id"], "standalone-project-id");
            assert_eq!(events[0]["entity_type"], "project");
            assert_eq!(events[0]["reason"], "created");

            teardown();
        }
    }
}
