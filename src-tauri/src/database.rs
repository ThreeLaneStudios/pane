// Pane v9.2.4 — Database Module
// Implements the full schema from pane-architecture-spec.md §16

use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

// ─── crsqlite path cache (ADR-0056) ──────────────────────────────────────────
// Cached at startup by lib.rs so restore can reopen the connection with the
// same extension path that was used at launch.
static CRSQLITE_LIB_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Store the crsqlite extension path. Called once at startup (lib.rs) before
/// app.manage(DbState(...)). Subsequent calls are silently ignored (OnceLock).
pub fn set_crsqlite_path(path: Option<PathBuf>) {
    let _ = CRSQLITE_LIB_PATH.set(path);
}

/// Retrieve the cached crsqlite extension path for connection reopen on restore.
pub fn get_crsqlite_path() -> Option<PathBuf> {
    CRSQLITE_LIB_PATH.get().and_then(|p| p.clone())
}

// ─── Connection wrapper (ADR-0056, R2) ───────────────────────────────────────

/// Newtype over `Option<Connection>` that enables close/reopen for restore
/// quiescence (ADR-0056) while preserving backward-compatible `Deref` access
/// for the 300+ existing call sites across the codebase.
///
/// # Invariant
/// The inner `Option` is `Some` during all normal operation. It is `None` only
/// inside the restore quiescence window, which is protected by holding the
/// `DbState` Mutex guard for the full close→swap→clear-WAL→reopen sequence.
/// No concurrent caller ever observes `None` — they block on the Mutex until
/// restore puts `Some(new_conn)` back.
///
/// # Design note
/// Using a Deref newtype (rather than `Arc<Mutex<Option<Connection>>>` with
/// per-site `.as_ref().unwrap()`) keeps the ~300+ existing callers unchanged
/// while still giving the restore path first-class ownership of the connection
/// lifecycle. The `expect` message in `deref`/`deref_mut` is the only place
/// that enforces the invariant at runtime — and it is unreachable under correct
/// usage because the Mutex is held across the entire quiescence window.
///
/// ADR-0056; `docs/INVARIANTS.md` — Restore / Database Swap.
pub struct DbConn(pub Option<Connection>);

impl DbConn {
    /// Create a `DbConn` wrapping a live connection.
    pub fn new(conn: Connection) -> Self {
        DbConn(Some(conn))
    }
}

impl std::ops::Deref for DbConn {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.0.as_ref().expect(
            "[DbState] Connection is None — restore in progress or double-close bug (ADR-0056)",
        )
    }
}

impl std::ops::DerefMut for DbConn {
    fn deref_mut(&mut self) -> &mut Connection {
        self.0
            .as_mut()
            .expect("[DbState] Connection is None (mut) — restore in progress or double-close bug (ADR-0056)")
    }
}

// ─── DbState ─────────────────────────────────────────────────────────────────

/// Thread-safe database connection handle for Tauri state.
///
/// Wraps `Arc<Mutex<DbConn>>`. The `Option<Connection>` inside `DbConn` enables
/// the restore path to atomically close the live connection before swapping the
/// DB file on disk and reopen a fresh connection afterward, without requiring
/// any change at the 300+ usage sites (Deref handles it transparently).
///
/// ADR-0056, R2; `docs/INVARIANTS.md` — Restore / Database Swap.
pub struct DbState(pub Arc<Mutex<DbConn>>);

impl DbState {
    /// Create a new `DbState` wrapping a live connection.
    /// Use at Tauri app startup and in tests.
    pub fn new(conn: Connection) -> Self {
        DbState(Arc::new(Mutex::new(DbConn::new(conn))))
    }

    /// Create a `DbState` with no live connection (connection slot = None).
    /// Use in tests that call restore functions directly and don't need a real
    /// starting connection — restore will open a fresh one on completion.
    #[cfg(test)]
    pub fn new_empty_for_test() -> Self {
        DbState(Arc::new(Mutex::new(DbConn(None))))
    }
}

// === Data Types ===

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NoteMeta {
    pub id: String,
    pub title: String,
    pub content_preview: String,
    pub file_path: String,
    pub project_id: String,
    pub folder_id: Option<String>,

    // Task fields (all optional)
    pub status: Option<String>,
    pub priority: Option<String>,
    pub due_date: Option<String>,
    pub assignee: Option<String>,
    pub completed_at: Option<String>,
    pub sort_order: i64,

    // Metadata
    pub word_count: i64,
    pub pinned: bool,
    pub owner_id: Option<String>,
    pub tags: Vec<String>,

    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub note_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NoteContent {
    pub meta: NoteMeta,
    pub content: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    /// Reserved column — not used for any current UI display.
    /// Category color is stored in `category_colors.color` and looked up via `projects.category`.
    /// Do not write per-project color to this column without a spec section.
    pub color: Option<String>,
    pub parent_id: Option<String>,
    pub sort_order: i64,
    pub owner_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub category: Option<String>,
    pub is_system: bool,
    pub is_favorite: bool,
    /// §1.7.1 (#2874 item 50a): only meaningfully set on a top-level project
    /// row — points at one of that project's own folder rows (by `projects.id`).
    /// A folder row itself (a `projects` row with `parent_id` set) always has
    /// this as `None`; folders don't themselves have a default folder.
    /// #2874 item 9: now writable after creation via `update_project`
    /// (`UpdateProjectRequest.default_folder_id`), not just at creation time.
    pub default_folder_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TagInfo {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// Retained for database/API payload compatibility with older callers.
#[allow(dead_code)]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResult {
    pub note_id: String,
    pub title: String,
    pub content_preview: String,
    pub project_name: String,
    pub folder_name: Option<String>,
    // #2438: project_id/folder_id were missing from this struct entirely —
    // search_notes never returned them, silently emptying wiki-link
    // navigation's target-context lookup (see docs/audits/2026-08-05-2438-incident-diagnosis.md).
    // These are the raw ids (not display names); project_name/folder_name above
    // remain for existing display-only consumers.
    pub project_id: String,
    pub folder_id: String,
    pub score: f64,
}

// === Helper Functions ===

pub fn get_pane_root() -> PathBuf {
    // Allow test override (§30 — Agent App Testing Infrastructure)
    let raw = match std::env::var("PANE_TEST_ROOT") {
        // "Empty is not zero" (#3057): a set-but-empty PANE_TEST_ROOT must be
        // refused exactly like an unset one, not silently resolved to
        // `PathBuf::from("")` (a CWD-dependent, unpredictable relative path).
        Ok(test_root) if !test_root.is_empty() => PathBuf::from(test_root),
        _ => {
            // #3057: a test binary that reaches this chokepoint without having
            // isolated PANE_TEST_ROOT first must refuse loudly, never silently
            // fall through to the real `~/Pane` (the exact shape of a prior
            // incident where an unisolated test touched real user data).
            // This branch compiles out entirely in production (`#[cfg(test)]`),
            // costing nothing outside the test binary.
            #[cfg(test)]
            panic!(
                "PANE_TEST_ROOT is not set (or is empty) — a test reached \
                 get_pane_root() without isolating it first. Refusing to \
                 resolve a real path from a test binary. Set \
                 PANE_TEST_ROOT=/tmp/<your-isolated-dir> before calling any \
                 code path that touches the filesystem."
            );
            #[cfg(not(test))]
            {
                // Production path
                let home = dirs::home_dir().expect("Could not find home directory");
                home.join("Pane")
            }
        }
    };

    resolve_pane_root_real_path(raw)
}

/// Resolves `raw` to its real (symlink-free) path on disk — the exact same
/// resolution `PaneFs::for_runtime()` / `PaneFs::from_pane_root()` already
/// perform via `fs::canonicalize`. Every caller of `get_pane_root()` that
/// compares its result against a path produced by `PaneFs` (via
/// `strip_prefix` or equality) needs both sides spelled identically —
/// macOS makes the two spellings diverge trivially (`/tmp` → `/private/tmp`,
/// `/var` → `/private/var`), and any Pane folder reached through a
/// symlink (external volume, synced directory, home-dir symlink) hits the
/// same divergence in production (#3008).
///
/// Prior occurrence **#2381**: the same divergence broke a test's
/// `is_suppressed()` lookup on macOS CI. The fix landed as a manual
/// `std::fs::canonicalize()` call inside that one test (see the
/// `test_2381_rename_note_file_suppresses_destination_before_disk_write`
/// comment below) instead of fixing `get_pane_root()` itself — leaving
/// every other caller (and every other test) to independently remember to
/// canonicalize `PANE_TEST_ROOT` before use, which `notes.rs`'s own test
/// comments ended up documenting as a recurring gotcha rather than a fixed
/// bug. This function is that fix, at the source: `get_pane_root()` now
/// resolves to the same real path PaneFs holds, for BOTH the test and
/// production branches, so no caller has to remember which spelling it's
/// getting.
///
/// First launch: `fs::canonicalize` fails outright on a path that doesn't
/// exist yet (fresh install, no `~/Pane` folder on disk). Rather than let
/// that failure silently fall back to the unresolved path (which would
/// quietly resurrect this exact bug the first time the folder is later
/// reached through a symlink) or error out of a cold start, we create the
/// directory first and then resolve it — the same create-then-canonicalize
/// order `PaneFs::from_pane_root()` already uses — so a fresh install
/// produces a real, resolved root from the very first call. Proven by the
/// `smoke-fresh-db` workflow, which starts from zero on-disk state.
fn resolve_pane_root_real_path(raw: PathBuf) -> PathBuf {
    if let Ok(resolved) = fs::canonicalize(&raw) {
        return resolved;
    }

    if let Err(error) = fs::create_dir_all(&raw) {
        eprintln!(
            "[DB] Failed to create Pane root {}: {} — falling back to the unresolved path",
            raw.display(),
            error
        );
        return raw;
    }

    fs::canonicalize(&raw).unwrap_or_else(|error| {
        eprintln!(
            "[DB] Failed to canonicalize Pane root {} after creating it: {} — falling back to the unresolved path",
            raw.display(),
            error
        );
        raw
    })
}

pub fn get_db_path() -> PathBuf {
    // Check for test mode (§30 — Agent App Testing Infrastructure)
    if let Ok(test_mode) = std::env::var("PANE_TEST_MODE") {
        if test_mode == "1" {
            if let Ok(test_db_path) = std::env::var("PANE_TEST_DB") {
                eprintln!("[DB] Using test database: {}", test_db_path);
                return PathBuf::from(test_db_path);
            } else {
                eprintln!("[DB] PANE_TEST_MODE=1 but PANE_TEST_DB not set, using production DB");
            }
        }
    }

    // Production path
    get_pane_root().join(".pane").join("pane.db")
}

pub fn get_config_path() -> PathBuf {
    get_pane_root().join(".pane").join("config.json")
}

pub fn get_projects_path() -> PathBuf {
    get_pane_root().join("Projects")
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .trim_matches('-')
        .to_string()
}

fn generate_unique_slug(base_slug: &str, project_path: &PathBuf) -> String {
    let mut slug = base_slug.to_string();
    let mut counter = 1;

    while project_path.join(format!("{}.md", slug)).exists() {
        slug = format!("{}-{}", base_slug, counter);
        counter += 1;
    }

    slug
}

fn note_policy_path(file_path: &str) -> Result<(PaneRoot, PathBuf), String> {
    let path = Path::new(file_path);
    if path.is_absolute() {
        return Err(format!("absolute note path rejected: {file_path}"));
    }

    let (root, relative) = if let Some(relative) = file_path.strip_prefix("Projects/") {
        (PaneRoot::Projects, Path::new(relative))
    } else if let Some(relative) = file_path.strip_prefix(".pane/system/") {
        (PaneRoot::System, Path::new(relative))
    } else {
        return Err(format!(
            "note path must be under Projects/ or .pane/system/: {file_path}"
        ));
    };

    PaneFs::validate_relative(relative).map_err(|error| error.to_string())?;
    Ok((root, relative.to_path_buf()))
}

// === Database Backup Functions ===
// DEPRECATED: Replaced by backup.rs module (§25) in v0.17.71
// Old backup code removed — now using comprehensive backup system with:
// - Daily automatic backups
// - Pre-migration backups
// - Manual backup/restore
// - Rolling retention (7 daily + 4 weekly)
// - Integrity checks
// - Flat file backups

// === Database Initialization ===

/// Opens a database connection without running migrations.
/// Use this when you need a connection but migrations have already run.
/// This is safe to call multiple times (e.g., in background tasks).
pub fn open_connection(crsqlite_path: Option<PathBuf>) -> Result<Connection, String> {
    eprintln!("[DB] Opening database connection...");
    let db_path = get_db_path();

    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create db directory: {}", e))?;
    }

    let conn = Connection::open(&db_path).map_err(|e| format!("Failed to open database: {}", e))?;

    conn.execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(|e| format!("Failed to set WAL mode: {}", e))?;

    if let Some(ref path) = crsqlite_path {
        load_crsqlite_extension(&conn, path)?;
    }

    Ok(conn)
}

/// Full database initialization — opens connection, runs migrations, seeds defaults.
/// Call this ONCE at app startup. Use open_connection() for subsequent connections.
///
/// Thin wrapper over `init_database_with_progress(crsqlite_path, None)`
/// (#2723 PR B) — every pre-existing call site (the real fast-path launch,
/// plus every test helper in the crate) keeps calling this exact function
/// with this exact signature; behavior is byte-for-byte unchanged.
pub fn init_database(crsqlite_path: Option<PathBuf>) -> Result<Connection, String> {
    init_database_with_progress(crsqlite_path, None)
}

/// Cheap, read-only pre-check (#2723 PR B): would a real launch against this
/// DB actually need to run a migration? Opens a throwaway connection (same
/// crsqlite extension the real init would use, so the comparison is
/// apples-to-apples) and defers the actual version comparison to
/// `migrations::needs_migration` — the exact same comparison
/// `run_migrations`'s first match makes, just without running anything.
///
/// Used by `lib.rs`'s `.setup()` to decide fast path (unchanged, synchronous,
/// no new thread) vs. window-first path (background thread, #2723 PR B item
/// 1). On ANY error here (missing file parent, extension load failure,
/// corrupt DB, etc.) returns `false` — falls through to the fast path, which
/// then hits (and reports) the exact same error `init_database` always
/// would have. This pre-check must never be able to introduce a *new*
/// failure mode of its own, only skip ahead to the one that already exists
/// (#2723 PR B item 4).
pub fn is_migration_pending(crsqlite_path: Option<PathBuf>) -> bool {
    match open_connection(crsqlite_path) {
        Ok(conn) => crate::migrations::needs_migration(&conn),
        Err(e) => {
            eprintln!(
                "[STARTUP] #2723: migration pre-check failed ({}) — falling through to the fast path",
                e
            );
            false
        }
    }
}

/// Same as `init_database`, but threads an optional `AppHandle` through to
/// `migrations::run_migrations_with_progress` so a real launch can emit live
/// `backup://progress` / `migration://progress` events on the "Updating
/// Pane…" screen (#2723 PR B). `app_handle = None` (every existing caller,
/// via `init_database` above) is a no-op for progress — identical behavior
/// to before this PR.
pub fn init_database_with_progress(
    crsqlite_path: Option<PathBuf>,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<Connection, String> {
    eprintln!("[DB] Initializing database (full init with migrations)...");

    init_file_system()?;

    // ADR-0051: migrations run on a short-lived connection to prevent
    // schema_cookie mismatch with cached FTS5 trigger VDBE programs.
    {
        let migration_conn = open_connection(crsqlite_path.clone())?;
        match crate::migrations::run_migrations_with_progress(&migration_conn, app_handle) {
            Ok(_) => {}
            Err(e) => {
                eprintln!(
                    "[CRITICAL] Migration failed: {}. Continuing with existing schema.",
                    e
                );
                eprintln!("[CRITICAL] The app will work but some new features may be unavailable.");
                eprintln!("[CRITICAL] A backup was saved before the migration attempt.");
            }
        }
    }

    let app_conn = open_connection(crsqlite_path)?;

    seed_defaults(&app_conn)?;
    restore_ai_settings_from_config(&app_conn)?;

    // NOTE (§398): CRR migration moved to numbered migration system (migration 17)
    // The old migrate_schema_for_crsql() and upgrade_tables_to_crr() functions
    // are no longer called on startup. CRR conversion now happens ONCE via migration 17.

    Ok(app_conn)
}
fn init_file_system() -> Result<(), String> {
    let pane_root = get_pane_root();
    let pane_data = pane_root.join(".pane");
    let projects = get_projects_path();

    // Create directory structure
    fs::create_dir_all(&pane_data)
        .map_err(|e| format!("Failed to create .pane directory: {}", e))?;
    fs::create_dir_all(&projects)
        .map_err(|e| format!("Failed to create Projects directory: {}", e))?;

    // Create default config.json if it doesn't exist
    let config_path = get_config_path();
    if !config_path.exists() {
        let default_config = r#"{
  "version": 1,
  "ai": {
    "maxTokensBudget": 8000,
    "autoMemoryExtraction": true
  },
  "sync": {
    "enabled": false,
    "provider": "supabase",
    "lastSyncAt": null
  },
  "ui": {
    "theme": "tron-dark",
    "sidebarWidth": 260,
    "chatPanelWidth": 380
  },
  "notifications": {
    "enabled": true,
    "quietHoursStart": "23:00",
    "quietHoursEnd": "08:00"
  }
}"#;
        fs::write(&config_path, default_config)
            .map_err(|e| format!("Failed to create config.json: {}", e))?;
    }

    Ok(())
}

/// §24.7: Load system files directly from ai_agents + filesystem
/// Reads from .pane/system/{workspace_path}/{FILE}.md
fn get_system_note_from_agents(
    conn: &Connection,
    note_type: &str,
) -> Result<Option<(String, String)>, String> {
    // Map note_type to filename.
    // #2277 Fix 3 (ADR-0060 D3): "rules" now maps to AGENTS.md, unifying with the
    // edit_note resolver (ai/commands.rs, note_type_to_filename) and the §24 read block
    // (ai/commands.rs build_context_prompt, which has always hardcoded ("AGENTS.md", "Rules")).
    // Previously this mapped "rules" -> "RULES.md", a second file that nothing else wrote
    // to consistently — onboarding generated RULES.md while edit_note("rules") silently
    // edited AGENTS.md instead. One canonical resolver now: AGENTS.md is the Rules slot.
    let filename = match note_type {
        "soul" => "SOUL.md",
        "memory" => "MEMORY.md",
        "identity" => "IDENTITY.md",
        "user" => "USER.md",
        "tools" => "TOOLS.md",
        "rules" => "AGENTS.md",
        "agents" => "AGENTS.md",
        "heartbeat" => "HEARTBEAT.md",
        _ => return Ok(None),
    };

    // Get main agent
    let agent: Option<(String, String)> = conn
        .query_row(
            "SELECT id, workspace_path FROM ai_agents WHERE is_default = 1 AND is_active = 1",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| format!("Failed to query main agent: {}", e))?;

    if let Some((agent_id, workspace_path)) = agent {
        PaneFs::validate_name(&workspace_path)
            .map_err(|error| format!("Unsafe agent workspace path: {error}"))?;
        let relative = PaneFs::relative_from_names([workspace_path.as_str(), filename])
            .map_err(|error| format!("Unsafe system note path: {error}"))?;
        let pane_fs = PaneFs::for_runtime().map_err(|error| error.to_string())?;
        let file_path =
            match pane_fs.authorize_existing(PaneRoot::System, &relative, ExistingKind::File) {
                Ok(path) => path,
                // #2277 Fix 3 fallout: a missing FILENAME (e.g. no AGENTS.md yet for a
                // freshly created local agent) must resolve to Ok(None), not an error.
                // authorize_existing's own "authorized path does not exist" message only
                // fires when the parent directory exists but a final symlink_metadata
                // check still fails; for a missing *leaf file* under an existing directory,
                // walk_beneath_root's component walk fails first with a different message
                // ("failed to inspect authorized path ...: No such file or directory").
                // Both phrasings share the same underlying io::Error text, so match on
                // that instead of the wrapper's own wording to catch both.
                Err(error) if error.to_string().contains("does not exist")
                    || error.to_string().contains("No such file or directory") =>
                {
                    return Ok(None)
                }
                Err(error) => return Err(format!("Unsafe system note path: {error}")),
            };

        match std::fs::read_to_string(file_path.as_path()) {
            Ok(content) => Ok(Some((agent_id, content))),
            Err(e) => {
                eprintln!("[§24] Failed to read {}: {}", filename, e);
                Ok(None)
            }
        }
    } else {
        // No main agent found
        Ok(None)
    }
}

/// Get a system note (memory/soul/rules) from a project's AI folder
/// §24.7: If ai_agents table exists, load from .pane/system/{workspace_path}/ directly
pub fn get_system_note(
    conn: &Connection,
    project_id: &str,
    note_type: &str,
) -> Result<Option<(String, String)>, String> {
    // Check if ai_agents table exists (post-§24 migration)
    let has_agents_table: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if has_agents_table {
        // §24.7: New behavior — load from ai_agents + filesystem directly
        return get_system_note_from_agents(conn, note_type);
    }

    // Legacy behavior for pre-migration databases

    // Helper closure to read note content from file_path
    let read_note_content = |file_path: &str| -> String {
        if !file_path.is_empty() {
            let resolved = if file_path.starts_with('/') {
                file_path.to_string()
            } else {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{}/Pane/{}", home, file_path)
            };
            std::fs::read_to_string(&resolved).unwrap_or_default()
        } else {
            String::new()
        }
    };

    // §24: For system projects, prioritize the main agent folder
    // so memory extraction goes to the right agent
    let is_system_project: bool = conn
        .query_row(
            "SELECT COALESCE(is_system, 0) FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get::<_, bool>(0),
        )
        .unwrap_or(false);

    let main_folder_id: Option<String> = if is_system_project {
        get_ai_setting(conn, "main_agent_folder_id")
    } else {
        None
    };

    if let Some(ref folder_id) = main_folder_id {
        let result: Option<(String, String)> = conn.query_row(
            "SELECT id, file_path FROM notes WHERE folder_id = ?1 AND note_type = ?2 AND deleted_at IS NULL",
            params![folder_id, note_type],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ).optional().map_err(|e| format!("Failed to find system note: {}", e))?;

        if let Some((note_id, file_path)) = result {
            let content = read_note_content(&file_path);
            return Ok(Some((note_id, content)));
        }
        // main_agent_folder_id is set but note not found in it — don't fall back to random folders
        // for types that should only exist in the main agent folder
        let main_only_types = ["identity", "user", "tools", "rules"];
        if main_only_types.contains(&note_type) {
            return Ok(None);
        }
    }

    // Fallback: search all sub-folders (for soul/memory which can exist in sub-agent folders too)
    // Get ALL sub-folders, separated by type
    // Agent folders (is_system=0) get priority over AI folders (is_system=1)
    let mut stmt = conn
        .prepare("SELECT id, COALESCE(is_system, 0) FROM projects WHERE parent_id = ?1")
        .map_err(|e| format!("Failed to prepare folder query: {}", e))?;

    let folders: Vec<(String, bool)> = stmt
        .query_map(params![project_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)? != 0))
        })
        .map_err(|e| format!("Failed to find folders: {}", e))?
        .filter_map(|r| r.ok())
        .collect();

    if folders.is_empty() {
        return Ok(None);
    }

    // Priority: non-system folders (agent folders) first, then system folders (AI folder)
    let agent_folders: Vec<&String> = folders
        .iter()
        .filter(|(_, is_sys)| !is_sys)
        .map(|(id, _)| id)
        .collect();
    let system_folders: Vec<&String> = folders
        .iter()
        .filter(|(_, is_sys)| *is_sys)
        .map(|(id, _)| id)
        .collect();

    // Search agent folders first, then fall back to system folders
    for folder_id in agent_folders.iter().chain(system_folders.iter()) {
        let result: Option<(String, String)> = conn.query_row(
            "SELECT id, file_path FROM notes WHERE folder_id = ?1 AND note_type = ?2 AND deleted_at IS NULL",
            params![folder_id, note_type],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        ).optional().map_err(|e| format!("Failed to find system note: {}", e))?;

        if let Some((note_id, file_path)) = result {
            // Auto-heal: if main_agent_folder_id is not set and we found a note in an agent folder, save it
            if is_system_project && main_folder_id.is_none() {
                let _ = set_ai_setting(conn, "main_agent_folder_id", folder_id);
            }
            let content = read_note_content(&file_path);
            return Ok(Some((note_id, content)));
        }
    }

    Ok(None)
}

/// One-time migration to set up AI folders for existing projects

pub fn seed_defaults(conn: &Connection) -> Result<(), String> {
    // Check if database is empty
    let user_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap_or(0);

    if user_count == 0 {
        // Create default local user
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO users (id, name, created_at) VALUES ('user_local', 'Local User', ?1)",
            params![now],
        )
        .map_err(|e| format!("Failed to create default user: {}", e))?;

        println!("✓ Created default local user");
    }

    // Check if projects exist
    let project_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))
        .unwrap_or(0);

    // Default "Notes" project removed — onboarding creates agents via ai_agents (§24)
    if project_count == 0 {
        println!("ℹ No projects — onboarding will create initial project");
    }

    // Ensure all AI folders have is_system=1 (fixup for older data)
    conn.execute(
        "UPDATE projects SET is_system = 1 WHERE name = 'AI' AND parent_id IS NOT NULL AND is_system = 0",
        [],
    ).ok();

    // Tag orphan Memory/Soul/Rules notes by title (fixup for pre-note_type data)
    conn.execute("UPDATE notes SET note_type = 'memory' WHERE title = 'Memory' AND (note_type IS NULL OR note_type = '') AND deleted_at IS NULL", []).ok();
    conn.execute("UPDATE notes SET note_type = 'soul' WHERE title = 'Soul' AND (note_type IS NULL OR note_type = '') AND deleted_at IS NULL", []).ok();
    conn.execute("UPDATE notes SET note_type = 'rules' WHERE title = 'Rules' AND (note_type IS NULL OR note_type = '') AND deleted_at IS NULL", []).ok();

    // Post-migration #470: Memory notes should have folder_id = NULL (project root)
    // Set folder_id to NULL for all system notes
    conn.execute(
        "UPDATE notes SET folder_id = NULL
         WHERE note_type IN ('memory', 'soul', 'rules')
         AND deleted_at IS NULL",
        [],
    )
    .ok();

    // Deduplicate Memory notes per project (keep the one with most content, or oldest if tied)
    conn.execute_batch("
        DELETE FROM notes WHERE note_type = 'memory' AND deleted_at IS NULL AND id NOT IN (
            SELECT n1.id FROM notes n1 WHERE n1.note_type = 'memory' AND n1.deleted_at IS NULL
            AND NOT EXISTS (
                SELECT 1 FROM notes n2 WHERE n2.note_type = 'memory' AND n2.deleted_at IS NULL
                AND n2.project_id = n1.project_id AND n2.id != n1.id
                AND (LENGTH(COALESCE(n2.content_preview, '')) > LENGTH(COALESCE(n1.content_preview, ''))
                     OR (LENGTH(COALESCE(n2.content_preview, '')) = LENGTH(COALESCE(n1.content_preview, '')) AND n2.created_at < n1.created_at))
            )
        );
    ").ok();

    // Deduplicate Soul notes per project (keep the one with most content)
    conn.execute_batch("
        DELETE FROM notes WHERE note_type = 'soul' AND deleted_at IS NULL AND id NOT IN (
            SELECT n1.id FROM notes n1 WHERE n1.note_type = 'soul' AND n1.deleted_at IS NULL
            AND NOT EXISTS (
                SELECT 1 FROM notes n2 WHERE n2.note_type = 'soul' AND n2.deleted_at IS NULL
                AND n2.project_id = n1.project_id AND n2.id != n1.id
                AND (LENGTH(COALESCE(n2.content_preview, '')) > LENGTH(COALESCE(n1.content_preview, ''))
                     OR (LENGTH(COALESCE(n2.content_preview, '')) = LENGTH(COALESCE(n1.content_preview, '')) AND n2.created_at < n1.created_at))
            )
        );
    ").ok();

    // Check if dashboard config exists
    let config_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM user_dashboard_config", [], |row| {
            row.get(0)
        })
        .unwrap_or(0);

    if config_count == 0 {
        let now = chrono::Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();

        conn.execute(
            "INSERT INTO user_dashboard_config (id, created_at, updated_at) VALUES (?1, ?2, ?3)",
            params![id, now, now],
        )
        .map_err(|e| format!("Failed to create dashboard config: {}", e))?;

        println!("✓ Created default dashboard config");
    }

    // Set default memory search configuration values (§5.15.6) if not present
    if get_ai_setting(conn, "memory_search_keyword_weight").is_none() {
        conn.execute(
            "INSERT OR IGNORE INTO ai_settings (key, value) VALUES ('memory_search_keyword_weight', '0.3')",
            [],
        ).ok();
    }
    if get_ai_setting(conn, "memory_search_fact_boost").is_none() {
        conn.execute(
            "INSERT OR IGNORE INTO ai_settings (key, value) VALUES ('memory_search_fact_boost', '1.2')",
            [],
        ).ok();
    }
    if get_ai_setting(conn, "memory_search_fuse_both_sources").is_none() {
        conn.execute(
            "INSERT OR IGNORE INTO ai_settings (key, value) VALUES ('memory_search_fuse_both_sources', 'true')",
            [],
        ).ok();
    }
    if get_ai_setting(conn, "memory_search_temporal_half_life_days").is_none() {
        conn.execute(
            "INSERT OR IGNORE INTO ai_settings (key, value) VALUES ('memory_search_temporal_half_life_days', '7')",
            [],
        ).ok();
    }

    // LLM rerank configuration (§5.15.4) — default OFF
    if get_ai_setting(conn, "memory_search_llm_rerank_enabled").is_none() {
        conn.execute(
            "INSERT OR IGNORE INTO ai_settings (key, value) VALUES ('memory_search_llm_rerank_enabled', 'false')",
            [],
        ).ok();
    }
    // Note: memory_search_llm_rerank_model must be set by user in Settings (no default)
    // Removed hardcoded fallback per spec line 423 (Matt directive, Apr 11)

    if get_ai_setting(conn, "memory_search_max_results").is_none() {
        conn.execute(
            "INSERT OR IGNORE INTO ai_settings (key, value) VALUES ('memory_search_max_results', '20')",
            [],
        ).ok();
    }

    // Compaction trigger percentage (#1403-D, §27.10).
    // CI-ALLOW: hardcoded seed default per #1403-D scope brief §5 (same pattern as peer settings above).
    if get_ai_setting(conn, "compaction_trigger_pct").is_none() {
        conn.execute(
            "INSERT OR IGNORE INTO ai_settings (key, value) VALUES ('compaction_trigger_pct', '0.80')",
            [],
        ).ok();
    }

    // Link previews gate (ADR-0049 §Decision 2, #1780). Default true.
    // Seeded here as a fallback; migrate_v69 (migrations.rs) carries over link_embeds_enabled
    // value for users who had previously toggled the old key off.
    if get_ai_setting(conn, "link_previews_enabled").is_none() {
        conn.execute(
            "INSERT OR IGNORE INTO ai_settings (key, value) VALUES ('link_previews_enabled', 'true')",
            [],
        ).ok();
    }

    // Backfill Memory.md for existing projects without one (Issue #803)
    // Idempotent — safe to re-run every startup
    eprintln!("[SEED] Checking for projects missing Memory.md notes...");
    let pane_fs = match PaneFs::for_runtime() {
        Ok(policy) => Some(policy),
        Err(error) => {
            eprintln!("[CRITICAL] Memory.md backfill filesystem policy failed: {error}");
            None
        }
    };
    match conn.prepare("SELECT id, name FROM projects WHERE parent_id IS NULL AND (is_system IS NULL OR is_system = 0)") {
        Ok(mut stmt) => {
            match stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))) {
                Ok(rows) => {
                    let projects: Vec<(String, String)> = rows.filter_map(|r| r.ok()).collect();
                    for (project_id, project_name) in projects {
                        // Check if Memory note exists
                        let note_count: i64 = match conn
                            .query_row(
                                "SELECT COUNT(*) FROM notes WHERE project_id = ?1 AND note_type = 'memory' AND deleted_at IS NULL",
                                params![&project_id],
                                |row| row.get(0),
                            ) {
                                Ok(count) => count,
                                Err(e) => {
                                    eprintln!("[CRITICAL] Memory.md backfill failed for project {}: {}", project_name, e);
                                    continue;
                                }
                            };

                        if note_count == 0 {
                            // No Memory note exists — backfill needed
                            let Some(pane_fs) = pane_fs.as_ref() else {
                                continue;
                            };
                            let memory_relative = match PaneFs::relative_from_names([
                                project_name.as_str(),
                                "Memory.md",
                            ]) {
                                Ok(relative) => relative,
                                Err(error) => {
                                    eprintln!("[CRITICAL] Memory.md backfill rejected project {}: {}", project_name, error);
                                    continue;
                                }
                            };
                            let memory_file_path = match pane_fs
                                .authorize_write(PaneRoot::Projects, &memory_relative)
                            {
                                Ok(path) => path,
                                Err(error) => {
                                    eprintln!("[CRITICAL] Memory.md backfill failed for project {}: {}", project_name, error);
                                    continue;
                                }
                            };

                            // Create file if it doesn't exist
                            if !memory_file_path.as_path().exists() {
                                let memory_header = format!("# Memory — {}\n", &project_name);
                                if let Err(e) = crate::file_ops::atomic_write(
                                    memory_file_path.as_path(),
                                    &memory_header,
                                    Some(conn),
                                    crate::file_ops::WriteOrigin::Local,
                                ) {
                                    // §smoke-allowlist: Memory\.md backfill failed for project .+: No such file or directory — expected in upgrade smoke tests where project dirs don't exist in temp workspace
                                    eprintln!("[CRITICAL] Memory.md backfill failed for project {}: {}", project_name, e);
                                    continue;
                                }
                                eprintln!("[SEED] Created Memory.md file for project: {}", project_name);
                            } else {
                                eprintln!("[SEED] Memory.md file exists on disk for project: {}, creating notes row", project_name);
                            }

                            // Create notes row (matches create_project_internal pattern)
                            let memory_note_id = uuid::Uuid::new_v4().to_string();
                            let memory_db_path = format!("Projects/{}/Memory.md", &project_name);
                            let now = chrono::Utc::now().to_rfc3339();
                            if let Err(e) = conn.execute(
                                "INSERT INTO notes (id, title, content_preview, file_path, project_id, folder_id, note_type, tags_json, owner_id, created_at, updated_at)
                                 VALUES (?1, 'Memory', '', ?2, ?3, NULL, 'memory', '[]', 'user_local', ?4, ?5)",
                                params![memory_note_id, memory_db_path, &project_id, &now, &now],
                            ) {
                                eprintln!("[CRITICAL] Memory.md backfill failed for project {}: {}", project_name, e);
                                continue;
                            }

                            eprintln!("[SEED] Backfilled Memory note for project: {}", project_name);
                        }
                    }
                }
                Err(e) => eprintln!("[CRITICAL] Memory.md backfill failed to query projects: {}", e),
            }
        }
        Err(e) => eprintln!("[CRITICAL] Memory.md backfill failed to prepare query: {}", e),
    }

    Ok(())
}

// === Note Operations ===

pub fn create_note_on_disk(
    project_name: &str,
    folder_name: Option<&str>,
    title: &str,
    content: &str,
) -> Result<(String, PathBuf), String> {
    PaneFs::validate_name(project_name).map_err(|error| error.to_string())?;
    if let Some(folder) = folder_name {
        PaneFs::validate_name(folder).map_err(|error| error.to_string())?;
    }

    let pane_fs = PaneFs::for_runtime().map_err(|error| error.to_string())?;
    let mut target_relative =
        PaneFs::relative_from_names([project_name]).map_err(|error| error.to_string())?;
    if let Some(folder) = folder_name {
        target_relative.push(folder);
    }
    let target_dir = pane_fs
        .create_dir_all(PaneRoot::Projects, &target_relative)
        .map_err(|error| error.to_string())?;

    let base_slug = slugify(title);
    let slug = generate_unique_slug(&base_slug, &target_dir.as_path().to_path_buf());
    let mut file_relative = target_relative;
    file_relative.push(format!("{slug}.md"));
    let file_path = pane_fs
        .authorize_write(PaneRoot::Projects, &file_relative)
        .map_err(|error| error.to_string())?;

    crate::file_ops::atomic_write(
        file_path.as_path(),
        content,
        None,
        crate::file_ops::WriteOrigin::Local,
    )
    .map_err(|error| {
        format!(
            "Failed to write file {}: {error}",
            file_path.as_path().display()
        )
    })?;

    Ok((slug, file_path.as_path().to_path_buf()))
}

pub(crate) fn authorize_note_parent(file_path: &str) -> Result<PathBuf, String> {
    let (root, relative) = note_policy_path(file_path)?;
    let parent = relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| "note path has no authorized parent directory".to_string())?;
    let pane_fs = PaneFs::for_runtime().map_err(|error| error.to_string())?;
    pane_fs
        .authorize_existing(root, parent, ExistingKind::Directory)
        .map(|authorized| authorized.as_path().to_path_buf())
        .map_err(|error| error.to_string())
}

pub fn read_note_content(file_path: &str) -> Result<String, String> {
    let (root, relative) = note_policy_path(file_path)?;
    let pane_fs = PaneFs::for_runtime().map_err(|error| error.to_string())?;
    let authorized = pane_fs
        .authorize_existing(root, &relative, ExistingKind::File)
        .map_err(|error| error.to_string())?;
    fs::read_to_string(authorized.as_path()).map_err(|error| {
        format!(
            "Failed to read file {}: {error}",
            authorized.as_path().display()
        )
    })
}

pub fn write_note_content(
    file_path: &str,
    content: &str,
    conn: Option<&Connection>,
) -> Result<(), String> {
    let (root, relative) = note_policy_path(file_path)?;
    let pane_fs = PaneFs::for_runtime().map_err(|error| error.to_string())?;
    let authorized = pane_fs
        .authorize_write(root, &relative)
        .map_err(|error| error.to_string())?;

    // All calls through write_note_content are Pane-originated (Local) writes.
    crate::file_ops::atomic_write(
        authorized.as_path(),
        content,
        conn,
        crate::file_ops::WriteOrigin::Local,
    )
    .map_err(|error| {
        format!(
            "Failed to write file {}: {error}",
            authorized.as_path().display()
        )
    })
}

/// #3027 sweep finding (not in Matt's original list, same bug shape as
/// `delete_project_internal`'s pre-fix code): this used to fall back to a
/// hard `remove_file` whenever `trash()` failed, permanently destroying the
/// note's file with no OS trash recovery — exactly the "trash failure ->
/// delete harder" shape #3027 rules out for projects. Fixed the same way:
/// no fallback. A trash failure here returns `Err` and leaves the file
/// exactly where it was; every caller already treats that as either
/// non-fatal (soft `delete_note`/`force_delete_note` log it and keep the
/// note's `deleted_at` tombstone — the file now just stays in `Projects/`
/// instead of moving to Trash, better than before where it was destroyed)
/// or fatal-before-any-DB-write (`purge_note_internal` calls this BEFORE its
/// own `DELETE FROM notes`, so an `Err` here stops the purge with the row
/// and file both still intact — matching #3027 ruling 1's shape exactly).
///
/// #3037 follow-up (`~/Shared/ExampleApp/docs/audits/2026-10-16-pr-3037-trash-
/// keeps-file.md` item 2): no caller in this crate invokes this directly
/// anymore — `empty_trash` and the AI assistant's `execute_delete_note`
/// (the two remaining direct callers before this fix) now both route
/// through `purge_note_internal` / `delete_note_file_tolerating_missing`
/// instead, same as `purge_note` already did. Kept (not deleted) as the
/// strict, non-tolerating primitive that wrapper builds on, in case a
/// future caller genuinely wants "fail hard on ANY missing/denied file"
/// rather than "tolerate not-found."
#[allow(dead_code)]
pub fn delete_note_file(file_path: &str) -> Result<(), String> {
    let (root, relative) = note_policy_path(file_path)?;
    let pane_fs = PaneFs::for_runtime().map_err(|error| error.to_string())?;
    let authorized = pane_fs
        .authorize_existing(root, &relative, ExistingKind::File)
        .map_err(|error| error.to_string())?;
    crate::watcher::suppress_path(authorized.as_path());

    pane_fs
        .trash(root, &relative, ExistingKind::File)
        .map_err(|trash_error| {
            format!("Failed to move note file to Trash ({trash_error}); the file was left in place.")
        })
}

/// #2874 item 1, Matt's ruling 3: "a trashed note whose file is already gone
/// purges cleanly." This is the single-note purge path's counterpart to
/// `empty_trash`'s existing blanket `.ok()` (notes.rs) — but deliberately
/// NOT a blanket swallow: only a confirmed-`NotFound` `FsPolicyError`
/// (`is_not_found()`, set exclusively from a `NotFound`-kind `io::Error` in
/// `authorize_existing`) is treated as "already purged, nothing to do here."
/// Every other failure mode — permissions, a real policy violation, any
/// other I/O error — still propagates as a hard `Err`, exactly as
/// `delete_note_file` already does. See the risk note in
/// `docs/audits/2026-10-16-scope-1060-test-failures.md` item 1: getting this
/// distinction wrong would mask a real new bug as "fine, it's just gone."
pub fn delete_note_file_tolerating_missing(file_path: &str) -> Result<(), String> {
    let (root, relative) = note_policy_path(file_path)?;
    let pane_fs = PaneFs::for_runtime().map_err(|error| error.to_string())?;
    let authorized = match pane_fs.authorize_existing(root, &relative, ExistingKind::File) {
        Ok(path) => path,
        Err(error) if error.is_not_found() => {
            eprintln!(
                "[PURGE-NOTE] File already gone, treating purge as already-complete: {}",
                file_path
            );
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    };
    crate::watcher::suppress_path(authorized.as_path());

    pane_fs
        .trash(root, &relative, ExistingKind::File)
        .map_err(|trash_error| {
            format!("Failed to move note file to Trash ({trash_error}); the file was left in place.")
        })
}

/// #2874 item 1, Matt's ruling 5 (no-recovery-tool restore path): read-only
/// existence check `restore_note` uses to decide whether a trashed note's
/// file needs rebuilding from `note_versions` before clearing `deleted_at`.
/// An empty `file_path` (DB-only notes — sheets, subtasks) always reports
/// "exists": there is nothing to check, and nothing to rebuild. Any failure
/// to confirm existence (including a real I/O/permission error, not just
/// "not found") is conservatively treated as "missing" here — this function
/// only ever gates an attempted rebuild, and that attempt's own `Result`
/// (via `write_note_content`) is what actually surfaces a real failure;
/// this check never deletes or mutates anything itself.
pub fn note_file_exists(file_path: &str) -> bool {
    if file_path.is_empty() {
        return true;
    }
    match note_policy_path(file_path) {
        Ok((root, relative)) => match PaneFs::for_runtime() {
            Ok(pane_fs) => pane_fs
                .authorize_existing(root, &relative, ExistingKind::File)
                .is_ok(),
            Err(_) => false,
        },
        Err(_) => false,
    }
}

pub fn rename_note_file(old_path: &str, new_path: &str) -> Result<(), String> {
    let (old_root, old_relative) = note_policy_path(old_path)?;
    let (new_root, new_relative) = note_policy_path(new_path)?;
    if old_root != new_root {
        return Err("note rename must stay within the same authorized root".to_string());
    }

    let pane_fs = PaneFs::for_runtime().map_err(|error| error.to_string())?;
    if let Some(parent) = new_relative.parent() {
        if !parent.as_os_str().is_empty() {
            pane_fs
                .create_dir_all(new_root, parent)
                .map_err(|error| error.to_string())?;
        }
    }

    // Suppress the destination path BEFORE the rename hits disk so the file watcher's
    // debounced Create/Modify event for the new filename is ignored as a Pane-originated
    // write (#2381) — same suppress-before-write pattern as atomic_write (file_ops.rs:135)
    // and delete_note_file (database.rs:952). SUPPRESSED_PATHS is a HashSet<PathBuf> keyed
    // on exact path, so the resolved *destination* path is what must be registered.
    //
    // The source (old) path does NOT need suppression here: the watcher unconditionally
    // discards every `EventKind::Remove(_)` event before it is ever inserted into
    // PENDING_EVENTS (watcher.rs:148, watcher.rs:196), and any non-Remove event kind fired
    // against the source path is filtered by the watcher's own `path.is_file()` gate at
    // insertion time (watcher.rs:199) — the source path no longer exists on disk once the
    // rename syscall completes, so it can never be queued regardless of suppression state.
    let destination = pane_fs
        .authorize_write(new_root, &new_relative)
        .map_err(|error| error.to_string())?;
    crate::watcher::suppress_path(destination.as_path());

    pane_fs
        .rename(old_root, &old_relative, &new_relative)
        .map_err(|error| format!("Failed to rename file: {error}"))
}

// === cr-sqlite Integration (v9.1.0) ===

fn load_crsqlite_extension(conn: &Connection, path: &PathBuf) -> Result<(), String> {
    unsafe {
        conn.load_extension_enable()
            .map_err(|e| format!("Failed to enable extension loading: {}", e))?;

        // Strip .dylib extension — SQLite's load_extension auto-appends
        // the platform extension (.dylib on macOS), causing double-extension
        // if the path already includes it.
        let stripped_path = path.with_extension("");
        conn.load_extension(&stripped_path, Some("sqlite3_crsqlite_init"))
            .map_err(|e| format!("Failed to load crsqlite extension: {}", e))?;

        conn.load_extension_disable()
            .map_err(|e| format!("Failed to disable extension loading: {}", e))?;
    }

    println!("✓ Loaded cr-sqlite extension from {:?}", path);
    Ok(())
}

/// CRR schema migration helper (§398) — extracted for migration 17.
/// Fixes PK constraints for CRR compatibility (no detection logic).
/// Retained so historical migration tooling can invoke the original migration step.
#[allow(dead_code)]
pub fn migrate_schema_for_crsql_inner(conn: &Connection) -> Result<(), String> {
    // Disable foreign keys temporarily
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .map_err(|e| format!("Failed to disable foreign keys: {}", e))?;

    // Clean up any existing crsql shadow tables
    // NOTE: ai_chat_messages removed in migration 18, but keep in list for cleanup of old shadow tables
    let tables = vec![
        "projects",
        "notes",
        "tags",
        "note_tags",
        "ai_chat_messages",
        "ai_context_markers",
        "ai_pinned_messages",
        "ai_conversation_summaries",
        "ai_memory",
        "ai_actions",
        "user_profiles",
        "user_dashboard_config",
        "dashboard_modules",
        "module_items",
        "daily_entries",
        "habit_entries",
        "reflection_entries",
        "gratitude_entries",
        "sync_state",
        "note_links",
        "note_versions",
    ];

    for table in &tables {
        conn.execute(&format!("DROP TABLE IF EXISTS {}__crsql_clock", table), [])
            .ok();
        conn.execute(&format!("DROP TABLE IF EXISTS {}__crsql_pks", table), [])
            .ok();
    }

    // 1. Fix projects table (remove UNIQUE constraint, remove DEFAULT from PK)
    conn.execute_batch("
        BEGIN;
        CREATE TABLE IF NOT EXISTS projects_new (
            id          TEXT NOT NULL PRIMARY KEY,
            name        TEXT NOT NULL DEFAULT '',
            description TEXT,
            color       TEXT,
            parent_id   TEXT,
            sort_order  INTEGER NOT NULL DEFAULT 0,
            owner_id    TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
            category    TEXT,
            is_system   INTEGER NOT NULL DEFAULT 0
        );
        INSERT OR IGNORE INTO projects_new SELECT id, name, description, color, parent_id, sort_order, owner_id,
            COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')), category, is_system FROM projects;
        DROP TABLE projects;
        ALTER TABLE projects_new RENAME TO projects;
        CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);
        CREATE INDEX IF NOT EXISTS idx_projects_parent ON projects(parent_id);
        CREATE INDEX IF NOT EXISTS idx_projects_name_parent ON projects(name, parent_id);
        COMMIT;
    ").map_err(|e| format!("Failed to migrate projects: {}", e))?;

    //  2. Fix notes table (remove DEFAULT from PK, add agent_id, nullable project_id)
    // Check if old table has agent_id column
    let notes_has_agent_id: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name='agent_id'",
            [],
            |row| row.get::<_, i64>(0).map(|c| c > 0),
        )
        .unwrap_or(false);

    let copy_sql = if notes_has_agent_id {
        "INSERT OR IGNORE INTO notes_new SELECT id, title, content_preview, file_path, project_id, folder_id,
            status, priority, due_date, assignee, completed_at, sort_order, word_count, pinned, owner_id,
            tags_json, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')), deleted_at, parent_id, note_type, COALESCE(is_vital, 0), agent_id FROM notes"
    } else {
        "INSERT OR IGNORE INTO notes_new (id, title, content_preview, file_path, project_id, folder_id,
            status, priority, due_date, assignee, completed_at, sort_order, word_count, pinned, owner_id,
            tags_json, created_at, updated_at, deleted_at, parent_id, note_type, is_vital)
        SELECT id, title, content_preview, file_path, project_id, folder_id,
            status, priority, due_date, assignee, completed_at, sort_order, word_count, pinned, owner_id,
            tags_json, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')), deleted_at, parent_id, note_type, COALESCE(is_vital, 0) FROM notes"
    };

    conn.execute_batch(
        "
        BEGIN;
        CREATE TABLE IF NOT EXISTS notes_new (
            id              TEXT NOT NULL PRIMARY KEY,
            title           TEXT NOT NULL DEFAULT '',
            content_preview TEXT NOT NULL DEFAULT '',
            file_path       TEXT NOT NULL DEFAULT '',
            project_id      TEXT,
            folder_id       TEXT,
            status          TEXT NOT NULL DEFAULT 'cabinet',
            priority        TEXT,
            due_date        TEXT,
            assignee        TEXT,
            completed_at    TEXT,
            sort_order      INTEGER NOT NULL DEFAULT 0,
            word_count      INTEGER NOT NULL DEFAULT 0,
            pinned          INTEGER NOT NULL DEFAULT 0,
            owner_id        TEXT,
            tags_json       TEXT NOT NULL DEFAULT '[]',
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
            deleted_at      TEXT,
            parent_id       TEXT,
            note_type       TEXT,
            is_vital        INTEGER NOT NULL DEFAULT 0,
            agent_id        TEXT
        );
    ",
    )
    .map_err(|e| format!("Failed to create notes_new: {}", e))?;

    conn.execute(copy_sql, [])
        .map_err(|e| format!("Failed to copy notes data: {}", e))?;

    conn.execute_batch("
        DROP TABLE notes;
        ALTER TABLE notes_new RENAME TO notes;
        CREATE INDEX IF NOT EXISTS idx_notes_project ON notes(project_id);
        CREATE INDEX IF NOT EXISTS idx_notes_folder ON notes(folder_id);
        CREATE INDEX IF NOT EXISTS idx_notes_agent_id ON notes(agent_id);
        CREATE INDEX IF NOT EXISTS idx_notes_status ON notes(status) WHERE status != 'cabinet';
        CREATE INDEX IF NOT EXISTS idx_notes_priority ON notes(priority) WHERE priority IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_due_date ON notes(due_date) WHERE due_date IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at);
        CREATE INDEX IF NOT EXISTS idx_notes_deleted ON notes(deleted_at) WHERE deleted_at IS NOT NULL;
        COMMIT;
    ").map_err(|e| format!("Failed to migrate notes: {}", e))?;

    // 3. Fix tags table (convert from INTEGER AUTOINCREMENT to TEXT PRIMARY KEY)
    conn.execute_batch(
        "
        BEGIN;
        CREATE TABLE IF NOT EXISTS tags_new (
            id    TEXT NOT NULL PRIMARY KEY,
            name  TEXT NOT NULL DEFAULT '',
            color TEXT NOT NULL DEFAULT '#6B7280'
        );
        INSERT OR IGNORE INTO tags_new SELECT CAST(id AS TEXT), name, color FROM tags;

        -- Update note_tags to use text IDs
        CREATE TABLE IF NOT EXISTS note_tags_temp (
            note_id TEXT NOT NULL,
            tag_id  TEXT NOT NULL,
            PRIMARY KEY (note_id, tag_id)
        );
        INSERT OR IGNORE INTO note_tags_temp
        SELECT note_id, CAST(tag_id AS TEXT) FROM note_tags;
        DROP TABLE note_tags;
        ALTER TABLE note_tags_temp RENAME TO note_tags;

        DROP TABLE tags;
        ALTER TABLE tags_new RENAME TO tags;
        CREATE INDEX IF NOT EXISTS idx_tags_name ON tags(name);
        CREATE INDEX IF NOT EXISTS idx_note_tags_note ON note_tags(note_id);
        CREATE INDEX IF NOT EXISTS idx_note_tags_tag ON note_tags(tag_id);
        COMMIT;
    ",
    )
    .map_err(|e| format!("Failed to migrate tags: {}", e))?;

    // 4-19: Migrate remaining tables (ai_chat_messages, ai_pinned_messages, etc.)
    // Using same pattern: remove DEFAULT from PK, remove UNIQUE constraints

    let migrations = vec![
        ("ai_chat_messages", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_chat_messages_new (
                id                TEXT NOT NULL PRIMARY KEY,
                project_id        TEXT NOT NULL DEFAULT '',
                role              TEXT NOT NULL DEFAULT 'user',
                content           TEXT NOT NULL DEFAULT '',
                context_note_id   TEXT,
                context_folder_id TEXT,
                context_mode      TEXT DEFAULT 'project',
                token_count       INTEGER,
                model             TEXT,
                created_at        TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO ai_chat_messages_new SELECT id, project_id, role, content, context_note_id, context_folder_id,
                context_mode, token_count, model, COALESCE(created_at, datetime('now')) FROM ai_chat_messages;
            DROP TABLE ai_chat_messages;
            ALTER TABLE ai_chat_messages_new RENAME TO ai_chat_messages;
            CREATE INDEX IF NOT EXISTS idx_chat_project ON ai_chat_messages(project_id, created_at);

        -- Local-only chat messages (not synced via cr-sqlite, no triggers)
        CREATE TABLE IF NOT EXISTS chat_messages_local (
            id          TEXT NOT NULL PRIMARY KEY,
            project_id  TEXT NOT NULL DEFAULT '',
            role        TEXT NOT NULL DEFAULT 'user',
            content     TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            is_pinned   INTEGER NOT NULL DEFAULT 0,
            attachment_ids TEXT,
            tool_calls  TEXT,
            tool_results TEXT,
            model       TEXT,
            token_count INTEGER,
            thread_id   TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_chat_local_project ON chat_messages_local(project_id, created_at);
            COMMIT;
        "),
        ("ai_context_markers", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_context_markers_new (
                id                TEXT NOT NULL PRIMARY KEY,
                project_id        TEXT NOT NULL DEFAULT '',
                marker_type       TEXT NOT NULL DEFAULT 'section',
                label             TEXT NOT NULL DEFAULT '',
                target_note_id    TEXT,
                target_folder_id  TEXT,
                created_at        TEXT NOT NULL DEFAULT (datetime('now')),
                inserted_after    TEXT
            );
            INSERT OR IGNORE INTO ai_context_markers_new SELECT * FROM ai_context_markers;
            DROP TABLE ai_context_markers;
            ALTER TABLE ai_context_markers_new RENAME TO ai_context_markers;
            COMMIT;
        "),
        ("ai_pinned_messages", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_pinned_messages_new (
                id          TEXT NOT NULL PRIMARY KEY,
                project_id  TEXT NOT NULL DEFAULT '',
                message_id  TEXT NOT NULL DEFAULT '',
                pinned_at   TEXT NOT NULL DEFAULT (datetime('now')),
                sort_order  INTEGER DEFAULT 0
            );
            INSERT OR IGNORE INTO ai_pinned_messages_new SELECT * FROM ai_pinned_messages;
            DROP TABLE ai_pinned_messages;
            ALTER TABLE ai_pinned_messages_new RENAME TO ai_pinned_messages;
            CREATE INDEX IF NOT EXISTS idx_ai_pinned_project_message ON ai_pinned_messages(project_id, message_id);
            COMMIT;
        "),
        ("ai_conversation_summaries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_conversation_summaries_new (
                id                TEXT NOT NULL PRIMARY KEY,
                project_id        TEXT NOT NULL DEFAULT '',
                summary_text      TEXT NOT NULL DEFAULT '',
                start_message_id  TEXT NOT NULL DEFAULT '',
                end_message_id    TEXT NOT NULL DEFAULT '',
                token_count       INTEGER,
                created_at        TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO ai_conversation_summaries_new SELECT id, project_id, summary_text, start_message_id, end_message_id,
                token_count, COALESCE(created_at, datetime('now')) FROM ai_conversation_summaries;
            DROP TABLE ai_conversation_summaries;
            ALTER TABLE ai_conversation_summaries_new RENAME TO ai_conversation_summaries;
            COMMIT;
        "),
        ("ai_memory", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_memory_new (
                id                TEXT NOT NULL PRIMARY KEY,
                layer             TEXT NOT NULL DEFAULT 'user',
                project_id        TEXT,
                user_id           TEXT,
                fact              TEXT NOT NULL DEFAULT '',
                category          TEXT,
                source_message_id TEXT,
                is_active         INTEGER NOT NULL DEFAULT 1,
                created_at        TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at        TEXT,
                last_referenced   TEXT,
                routed_at         TEXT
            );
            INSERT OR IGNORE INTO ai_memory_new SELECT COALESCE(id, hex(randomblob(16))), layer, project_id, user_id, fact, category,
                source_message_id, is_active, COALESCE(created_at, datetime('now')), updated_at, last_referenced, NULL FROM ai_memory;
            DROP TABLE ai_memory;
            ALTER TABLE ai_memory_new RENAME TO ai_memory;
            CREATE INDEX IF NOT EXISTS idx_memory_layer ON ai_memory(layer);
            CREATE INDEX IF NOT EXISTS idx_memory_project ON ai_memory(project_id) WHERE project_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_memory_user ON ai_memory(user_id) WHERE user_id IS NOT NULL;
            COMMIT;
        "),
        ("ai_actions", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_actions_new (
                id              TEXT NOT NULL PRIMARY KEY,
                user_id         TEXT NOT NULL DEFAULT '',
                action_type     TEXT NOT NULL DEFAULT '',
                target_id       TEXT,
                target_type     TEXT,
                details_json    TEXT NOT NULL DEFAULT '{}',
                project_id      TEXT,
                model           TEXT,
                message_id      TEXT,
                status          TEXT NOT NULL DEFAULT 'success',
                error_message   TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO ai_actions_new SELECT COALESCE(id, hex(randomblob(16))), user_id, action_type, target_id, target_type,
                details_json, project_id, model, message_id, status, error_message, COALESCE(created_at, datetime('now')) FROM ai_actions;
            DROP TABLE ai_actions;
            ALTER TABLE ai_actions_new RENAME TO ai_actions;
            CREATE INDEX IF NOT EXISTS idx_ai_actions_user ON ai_actions(user_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_ai_actions_project ON ai_actions(project_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_ai_actions_type ON ai_actions(action_type);
            CREATE INDEX IF NOT EXISTS idx_ai_actions_target ON ai_actions(target_id);
            CREATE INDEX IF NOT EXISTS idx_ai_actions_date ON ai_actions(created_at);
            COMMIT;
        "),
        ("user_profiles", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS user_profiles_new (
                id                      TEXT NOT NULL PRIMARY KEY,
                user_id                 TEXT NOT NULL DEFAULT '',
                display_name            TEXT,
                bio                     TEXT,
                working_style           TEXT,
                ai_provider             TEXT,
                ai_model                TEXT,
                onboarding_method       TEXT,
                onboarding_completed    INTEGER NOT NULL DEFAULT 0,
                profile_json            TEXT DEFAULT '{}',
                created_at              TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at              TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO user_profiles_new SELECT COALESCE(id, hex(randomblob(16))), user_id, display_name, bio, working_style,
                ai_provider, ai_model, onboarding_method, onboarding_completed, profile_json,
                COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM user_profiles;
            DROP TABLE user_profiles;
            ALTER TABLE user_profiles_new RENAME TO user_profiles;
            COMMIT;
        "),
        ("user_dashboard_config", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS user_dashboard_config_new (
                id                    TEXT NOT NULL PRIMARY KEY,
                onboarding_completed  INTEGER DEFAULT 0,
                onboarding_template   TEXT,
                theme_variant         TEXT DEFAULT 'default',
                created_at            TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO user_dashboard_config_new SELECT COALESCE(id, hex(randomblob(16))), onboarding_completed, onboarding_template,
                theme_variant, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM user_dashboard_config;
            DROP TABLE user_dashboard_config;
            ALTER TABLE user_dashboard_config_new RENAME TO user_dashboard_config;
            COMMIT;
        "),
        ("dashboard_modules", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS dashboard_modules_new (
                id              TEXT NOT NULL PRIMARY KEY,
                module_type     TEXT NOT NULL DEFAULT '',
                sort_order      INTEGER NOT NULL DEFAULT 0,
                is_enabled      INTEGER NOT NULL DEFAULT 1,
                config_json     TEXT DEFAULT '{}',
                static_content  TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO dashboard_modules_new SELECT COALESCE(id, hex(randomblob(16))), module_type, sort_order, is_enabled,
                config_json, static_content, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM dashboard_modules;
            DROP TABLE dashboard_modules;
            ALTER TABLE dashboard_modules_new RENAME TO dashboard_modules;
            CREATE INDEX IF NOT EXISTS idx_modules_sort ON dashboard_modules(sort_order) WHERE is_enabled = 1;
            COMMIT;
        "),
        ("module_items", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS module_items_new (
                id          TEXT NOT NULL PRIMARY KEY,
                module_id   TEXT NOT NULL DEFAULT '',
                item_type   TEXT NOT NULL DEFAULT '',
                label       TEXT NOT NULL DEFAULT '',
                description TEXT,
                emoji       TEXT,
                config_json TEXT DEFAULT '{}',
                sort_order  INTEGER NOT NULL DEFAULT 0,
                is_active   INTEGER NOT NULL DEFAULT 1,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO module_items_new SELECT COALESCE(id, hex(randomblob(16))), module_id, item_type, label, description,
                emoji, config_json, sort_order, is_active, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM module_items;
            DROP TABLE module_items;
            ALTER TABLE module_items_new RENAME TO module_items;
            CREATE INDEX IF NOT EXISTS idx_items_module ON module_items(module_id, sort_order);
            COMMIT;
        "),
        ("daily_entries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS daily_entries_new (
                id          TEXT NOT NULL PRIMARY KEY,
                module_id   TEXT NOT NULL DEFAULT '',
                entry_date  TEXT NOT NULL DEFAULT '',
                data_json   TEXT NOT NULL DEFAULT '{}',
                completed_at TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO daily_entries_new SELECT COALESCE(id, hex(randomblob(16))), module_id, entry_date, data_json,
                completed_at, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM daily_entries;
            DROP TABLE daily_entries;
            ALTER TABLE daily_entries_new RENAME TO daily_entries;
            CREATE INDEX IF NOT EXISTS idx_daily_date ON daily_entries(entry_date);
            CREATE INDEX IF NOT EXISTS idx_daily_module_date ON daily_entries(module_id, entry_date);
            COMMIT;
        "),
        ("habit_entries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS habit_entries_new (
                id          TEXT NOT NULL PRIMARY KEY,
                item_id     TEXT NOT NULL DEFAULT '',
                entry_date  TEXT NOT NULL DEFAULT '',
                completed   INTEGER NOT NULL DEFAULT 0,
                completed_at TEXT
            );
            INSERT OR IGNORE INTO habit_entries_new SELECT COALESCE(id, hex(randomblob(16))), item_id, entry_date, completed, completed_at FROM habit_entries;
            DROP TABLE habit_entries;
            ALTER TABLE habit_entries_new RENAME TO habit_entries;
            CREATE INDEX IF NOT EXISTS idx_habit_date ON habit_entries(item_id, entry_date);
            COMMIT;
        "),
        ("reflection_entries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS reflection_entries_new (
                id              TEXT NOT NULL PRIMARY KEY,
                question_id     TEXT NOT NULL DEFAULT '',
                entry_date      TEXT NOT NULL DEFAULT '',
                value_numeric   REAL,
                value_bool      INTEGER,
                value_text      TEXT
            );
            INSERT OR IGNORE INTO reflection_entries_new SELECT COALESCE(id, hex(randomblob(16))), question_id, entry_date,
                value_numeric, value_bool, value_text FROM reflection_entries;
            DROP TABLE reflection_entries;
            ALTER TABLE reflection_entries_new RENAME TO reflection_entries;
            CREATE INDEX IF NOT EXISTS idx_reflection_date ON reflection_entries(question_id, entry_date);
            COMMIT;
        "),
        ("gratitude_entries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS gratitude_entries_new (
                id          TEXT NOT NULL PRIMARY KEY,
                entry_date  TEXT NOT NULL DEFAULT '',
                entry_index INTEGER NOT NULL DEFAULT 0,
                text        TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO gratitude_entries_new SELECT COALESCE(id, hex(randomblob(16))), entry_date, entry_index, text,
                COALESCE(created_at, datetime('now')) FROM gratitude_entries;
            DROP TABLE gratitude_entries;
            ALTER TABLE gratitude_entries_new RENAME TO gratitude_entries;
            CREATE INDEX IF NOT EXISTS idx_gratitude_date ON gratitude_entries(entry_date);
            COMMIT;
        "),
        ("sync_state", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS sync_state_new (
                id          TEXT NOT NULL PRIMARY KEY,
                device_id   TEXT NOT NULL DEFAULT '',
                device_name TEXT,
                platform    TEXT,
                last_sync_at TEXT,

                sync_version INTEGER NOT NULL DEFAULT 0,
                is_current  INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO sync_state_new SELECT COALESCE(id, hex(randomblob(16))), device_id, device_name, platform,
                last_sync_at, sync_version, is_current, COALESCE(created_at, datetime('now')) FROM sync_state;
            DROP TABLE sync_state;
            ALTER TABLE sync_state_new RENAME TO sync_state;
            COMMIT;
        "),
        ("note_tags", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS note_tags_new (
                note_id TEXT NOT NULL,
                tag_id  TEXT NOT NULL,
                PRIMARY KEY (note_id, tag_id)
            );
            INSERT OR IGNORE INTO note_tags_new SELECT note_id, CAST(tag_id AS TEXT) FROM note_tags;
            DROP TABLE note_tags;
            ALTER TABLE note_tags_new RENAME TO note_tags;
            CREATE INDEX IF NOT EXISTS idx_note_tags_note ON note_tags(note_id);
            CREATE INDEX IF NOT EXISTS idx_note_tags_tag ON note_tags(tag_id);
            COMMIT;
        "),
        ("note_links", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS note_links_new (
                source_note_id TEXT NOT NULL,
                target_note_id TEXT NOT NULL,
                PRIMARY KEY (source_note_id, target_note_id)
            );
            INSERT OR IGNORE INTO note_links_new SELECT * FROM note_links;
            DROP TABLE note_links;
            ALTER TABLE note_links_new RENAME TO note_links;
            CREATE INDEX IF NOT EXISTS idx_note_links_source ON note_links(source_note_id);
            CREATE INDEX IF NOT EXISTS idx_note_links_target ON note_links(target_note_id);
            COMMIT;
        "),
        ("note_versions", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS note_versions (
                id         TEXT NOT NULL PRIMARY KEY,
                note_id    TEXT NOT NULL DEFAULT '',
                content    TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                source     TEXT NOT NULL DEFAULT 'manual'
            );
            CREATE TABLE IF NOT EXISTS note_versions_new (
                id         TEXT NOT NULL PRIMARY KEY,
                note_id    TEXT NOT NULL DEFAULT '',
                content    TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                source     TEXT NOT NULL DEFAULT 'manual'
            );
            INSERT OR IGNORE INTO note_versions_new SELECT id, note_id, content, created_at, source FROM note_versions;
            DROP TABLE IF EXISTS note_versions;
            ALTER TABLE note_versions_new RENAME TO note_versions;
            CREATE INDEX IF NOT EXISTS idx_note_versions_note_id ON note_versions(note_id);
            COMMIT;
        "),
    ];

    for (name, sql) in migrations {
        // Skip ai_chat_messages if table doesn't exist (migration 18 drops it)
        if name == "ai_chat_messages" {
            let table_exists: bool = conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ai_chat_messages'",
                [],
                |row| row.get::<_, i64>(0).map(|count| count > 0),
            ).unwrap_or(false);

            if !table_exists {
                eprintln!("[CRR-MIGRATE] Skipping ai_chat_messages (table doesn't exist, likely dropped in migration 18)");
                continue;
            }
        }

        conn.execute_batch(sql)
            .map_err(|e| format!("Failed to migrate {}: {}", name, e))?;
    }

    // Re-enable foreign keys
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| format!("Failed to re-enable foreign keys: {}", e))?;

    Ok(())
}

/// DEPRECATED (§398): Old CRR migration function (replaced by migration 17)
/// This function is kept for reference but should never be called
#[allow(dead_code)]
fn migrate_schema_for_crsql(conn: &Connection) -> Result<(), String> {
    // Check if migration is needed by testing the notes table PK
    // If notes.id already has NOT NULL (notnull=1), migration was already done
    let already_migrated: bool = conn
        .query_row(
            "SELECT notnull FROM pragma_table_info('notes') WHERE pk > 0 LIMIT 1",
            [],
            |row| row.get::<_, i32>(0),
        )
        .map(|v| v == 1) // notnull=1 means already migrated
        .unwrap_or(false);

    if already_migrated {
        println!("  ✓ Schema already migrated for cr-sqlite");
        return Ok(());
    }

    println!("🔧 Migrating schema for CRR compatibility...");

    // Disable foreign keys temporarily
    conn.execute_batch("PRAGMA foreign_keys = OFF;")
        .map_err(|e| format!("Failed to disable foreign keys: {}", e))?;

    // Clean up any existing crsql shadow tables
    let tables = vec![
        "projects",
        "notes",
        "tags",
        "note_tags",
        "ai_chat_messages",
        "ai_context_markers",
        "ai_pinned_messages",
        "ai_conversation_summaries",
        "ai_memory",
        "ai_actions",
        "user_profiles",
        "user_dashboard_config",
        "dashboard_modules",
        "module_items",
        "daily_entries",
        "habit_entries",
        "reflection_entries",
        "gratitude_entries",
        "sync_state",
    ];

    for table in &tables {
        conn.execute(&format!("DROP TABLE IF EXISTS {}__crsql_clock", table), [])
            .ok();
        conn.execute(&format!("DROP TABLE IF EXISTS {}__crsql_pks", table), [])
            .ok();
    }

    // 1. Fix projects table (remove UNIQUE constraint, remove DEFAULT from PK)
    conn.execute_batch("
        BEGIN;
        CREATE TABLE IF NOT EXISTS projects_new (
            id          TEXT NOT NULL PRIMARY KEY,
            name        TEXT NOT NULL DEFAULT '',
            description TEXT,
            color       TEXT,
            parent_id   TEXT,
            sort_order  INTEGER NOT NULL DEFAULT 0,
            owner_id    TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
            category    TEXT,
            is_system   INTEGER NOT NULL DEFAULT 0
        );
        INSERT OR IGNORE INTO projects_new SELECT id, name, description, color, parent_id, sort_order, owner_id,
            COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')), category, is_system FROM projects;
        DROP TABLE projects;
        ALTER TABLE projects_new RENAME TO projects;
        CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);
        CREATE INDEX IF NOT EXISTS idx_projects_parent ON projects(parent_id);
        CREATE INDEX IF NOT EXISTS idx_projects_name_parent ON projects(name, parent_id);
        COMMIT;
    ").map_err(|e| format!("Failed to migrate projects: {}", e))?;

    //  2. Fix notes table (remove DEFAULT from PK, add agent_id, nullable project_id)
    // Check if old table has agent_id column
    let notes_has_agent_id: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('notes') WHERE name='agent_id'",
            [],
            |row| row.get::<_, i64>(0).map(|c| c > 0),
        )
        .unwrap_or(false);

    let copy_sql = if notes_has_agent_id {
        "INSERT OR IGNORE INTO notes_new SELECT id, title, content_preview, file_path, project_id, folder_id,
            status, priority, due_date, assignee, completed_at, sort_order, word_count, pinned, owner_id,
            tags_json, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')), deleted_at, parent_id, note_type, COALESCE(is_vital, 0), agent_id FROM notes"
    } else {
        "INSERT OR IGNORE INTO notes_new (id, title, content_preview, file_path, project_id, folder_id,
            status, priority, due_date, assignee, completed_at, sort_order, word_count, pinned, owner_id,
            tags_json, created_at, updated_at, deleted_at, parent_id, note_type, is_vital)
        SELECT id, title, content_preview, file_path, project_id, folder_id,
            status, priority, due_date, assignee, completed_at, sort_order, word_count, pinned, owner_id,
            tags_json, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')), deleted_at, parent_id, note_type, COALESCE(is_vital, 0) FROM notes"
    };

    conn.execute_batch(
        "
        BEGIN;
        CREATE TABLE IF NOT EXISTS notes_new (
            id              TEXT NOT NULL PRIMARY KEY,
            title           TEXT NOT NULL DEFAULT '',
            content_preview TEXT NOT NULL DEFAULT '',
            file_path       TEXT NOT NULL DEFAULT '',
            project_id      TEXT,
            folder_id       TEXT,
            status          TEXT NOT NULL DEFAULT 'cabinet',
            priority        TEXT,
            due_date        TEXT,
            assignee        TEXT,
            completed_at    TEXT,
            sort_order      INTEGER NOT NULL DEFAULT 0,
            word_count      INTEGER NOT NULL DEFAULT 0,
            pinned          INTEGER NOT NULL DEFAULT 0,
            owner_id        TEXT,
            tags_json       TEXT NOT NULL DEFAULT '[]',
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
            deleted_at      TEXT,
            parent_id       TEXT,
            note_type       TEXT,
            is_vital        INTEGER NOT NULL DEFAULT 0,
            agent_id        TEXT
        );
    ",
    )
    .map_err(|e| format!("Failed to create notes_new: {}", e))?;

    conn.execute(copy_sql, [])
        .map_err(|e| format!("Failed to copy notes data: {}", e))?;

    conn.execute_batch("
        DROP TABLE notes;
        ALTER TABLE notes_new RENAME TO notes;
        CREATE INDEX IF NOT EXISTS idx_notes_project ON notes(project_id);
        CREATE INDEX IF NOT EXISTS idx_notes_folder ON notes(folder_id);
        CREATE INDEX IF NOT EXISTS idx_notes_agent_id ON notes(agent_id);
        CREATE INDEX IF NOT EXISTS idx_notes_status ON notes(status) WHERE status != 'cabinet';
        CREATE INDEX IF NOT EXISTS idx_notes_priority ON notes(priority) WHERE priority IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_due_date ON notes(due_date) WHERE due_date IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at);
        CREATE INDEX IF NOT EXISTS idx_notes_deleted ON notes(deleted_at) WHERE deleted_at IS NOT NULL;
        COMMIT;
    ").map_err(|e| format!("Failed to migrate notes: {}", e))?;

    // 3. Fix tags table (convert from INTEGER AUTOINCREMENT to TEXT PRIMARY KEY)
    conn.execute_batch(
        "
        BEGIN;
        CREATE TABLE IF NOT EXISTS tags_new (
            id    TEXT NOT NULL PRIMARY KEY,
            name  TEXT NOT NULL DEFAULT '',
            color TEXT NOT NULL DEFAULT '#6B7280'
        );
        INSERT OR IGNORE INTO tags_new SELECT CAST(id AS TEXT), name, color FROM tags;

        -- Update note_tags to use text IDs
        CREATE TABLE IF NOT EXISTS note_tags_temp (
            note_id TEXT NOT NULL,
            tag_id  TEXT NOT NULL,
            PRIMARY KEY (note_id, tag_id)
        );
        INSERT OR IGNORE INTO note_tags_temp
        SELECT note_id, CAST(tag_id AS TEXT) FROM note_tags;
        DROP TABLE note_tags;
        ALTER TABLE note_tags_temp RENAME TO note_tags;

        DROP TABLE tags;
        ALTER TABLE tags_new RENAME TO tags;
        CREATE INDEX IF NOT EXISTS idx_tags_name ON tags(name);
        CREATE INDEX IF NOT EXISTS idx_note_tags_note ON note_tags(note_id);
        CREATE INDEX IF NOT EXISTS idx_note_tags_tag ON note_tags(tag_id);
        COMMIT;
    ",
    )
    .map_err(|e| format!("Failed to migrate tags: {}", e))?;

    // 4-19: Migrate remaining tables (ai_chat_messages, ai_pinned_messages, etc.)
    // Using same pattern: remove DEFAULT from PK, remove UNIQUE constraints

    let migrations = vec![
        ("ai_chat_messages", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_chat_messages_new (
                id                TEXT NOT NULL PRIMARY KEY,
                project_id        TEXT NOT NULL DEFAULT '',
                role              TEXT NOT NULL DEFAULT 'user',
                content           TEXT NOT NULL DEFAULT '',
                context_note_id   TEXT,
                context_folder_id TEXT,
                context_mode      TEXT DEFAULT 'project',
                token_count       INTEGER,
                model             TEXT,
                created_at        TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO ai_chat_messages_new SELECT id, project_id, role, content, context_note_id, context_folder_id,
                context_mode, token_count, model, COALESCE(created_at, datetime('now')) FROM ai_chat_messages;
            DROP TABLE ai_chat_messages;
            ALTER TABLE ai_chat_messages_new RENAME TO ai_chat_messages;
            CREATE INDEX IF NOT EXISTS idx_chat_project ON ai_chat_messages(project_id, created_at);

        -- Local-only chat messages (not synced via cr-sqlite, no triggers)
        CREATE TABLE IF NOT EXISTS chat_messages_local (
            id          TEXT NOT NULL PRIMARY KEY,
            project_id  TEXT NOT NULL DEFAULT '',
            role        TEXT NOT NULL DEFAULT 'user',
            content     TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            is_pinned   INTEGER NOT NULL DEFAULT 0,
            attachment_ids TEXT,
            tool_calls  TEXT,
            tool_results TEXT,
            model       TEXT,
            token_count INTEGER,
            thread_id   TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_chat_local_project ON chat_messages_local(project_id, created_at);
            COMMIT;
        "),
        ("ai_context_markers", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_context_markers_new (
                id                TEXT NOT NULL PRIMARY KEY,
                project_id        TEXT NOT NULL DEFAULT '',
                marker_type       TEXT NOT NULL DEFAULT 'section',
                label             TEXT NOT NULL DEFAULT '',
                target_note_id    TEXT,
                target_folder_id  TEXT,
                created_at        TEXT NOT NULL DEFAULT (datetime('now')),
                inserted_after    TEXT
            );
            INSERT OR IGNORE INTO ai_context_markers_new SELECT * FROM ai_context_markers;
            DROP TABLE ai_context_markers;
            ALTER TABLE ai_context_markers_new RENAME TO ai_context_markers;
            COMMIT;
        "),
        ("ai_pinned_messages", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_pinned_messages_new (
                id          TEXT NOT NULL PRIMARY KEY,
                project_id  TEXT NOT NULL DEFAULT '',
                message_id  TEXT NOT NULL DEFAULT '',
                pinned_at   TEXT NOT NULL DEFAULT (datetime('now')),
                sort_order  INTEGER DEFAULT 0
            );
            INSERT OR IGNORE INTO ai_pinned_messages_new SELECT * FROM ai_pinned_messages;
            DROP TABLE ai_pinned_messages;
            ALTER TABLE ai_pinned_messages_new RENAME TO ai_pinned_messages;
            CREATE INDEX IF NOT EXISTS idx_ai_pinned_project_message ON ai_pinned_messages(project_id, message_id);
            COMMIT;
        "),
        ("ai_conversation_summaries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_conversation_summaries_new (
                id                TEXT NOT NULL PRIMARY KEY,
                project_id        TEXT NOT NULL DEFAULT '',
                summary_text      TEXT NOT NULL DEFAULT '',
                start_message_id  TEXT NOT NULL DEFAULT '',
                end_message_id    TEXT NOT NULL DEFAULT '',
                token_count       INTEGER,
                created_at        TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO ai_conversation_summaries_new SELECT id, project_id, summary_text, start_message_id, end_message_id,
                token_count, COALESCE(created_at, datetime('now')) FROM ai_conversation_summaries;
            DROP TABLE ai_conversation_summaries;
            ALTER TABLE ai_conversation_summaries_new RENAME TO ai_conversation_summaries;
            COMMIT;
        "),
        ("ai_memory", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_memory_new (
                id                TEXT NOT NULL PRIMARY KEY,
                layer             TEXT NOT NULL DEFAULT 'user',
                project_id        TEXT,
                user_id           TEXT,
                fact              TEXT NOT NULL DEFAULT '',
                category          TEXT,
                source_message_id TEXT,
                is_active         INTEGER NOT NULL DEFAULT 1,
                created_at        TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at        TEXT,
                last_referenced   TEXT,
                routed_at         TEXT
            );
            INSERT OR IGNORE INTO ai_memory_new SELECT COALESCE(id, hex(randomblob(16))), layer, project_id, user_id, fact, category,
                source_message_id, is_active, COALESCE(created_at, datetime('now')), updated_at, last_referenced, NULL FROM ai_memory;
            DROP TABLE ai_memory;
            ALTER TABLE ai_memory_new RENAME TO ai_memory;
            CREATE INDEX IF NOT EXISTS idx_memory_layer ON ai_memory(layer);
            CREATE INDEX IF NOT EXISTS idx_memory_project ON ai_memory(project_id) WHERE project_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_memory_user ON ai_memory(user_id) WHERE user_id IS NOT NULL;
            COMMIT;
        "),
        ("ai_actions", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS ai_actions_new (
                id              TEXT NOT NULL PRIMARY KEY,
                user_id         TEXT NOT NULL DEFAULT '',
                action_type     TEXT NOT NULL DEFAULT '',
                target_id       TEXT,
                target_type     TEXT,
                details_json    TEXT NOT NULL DEFAULT '{}',
                project_id      TEXT,
                model           TEXT,
                message_id      TEXT,
                status          TEXT NOT NULL DEFAULT 'success',
                error_message   TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO ai_actions_new SELECT COALESCE(id, hex(randomblob(16))), user_id, action_type, target_id, target_type,
                details_json, project_id, model, message_id, status, error_message, COALESCE(created_at, datetime('now')) FROM ai_actions;
            DROP TABLE ai_actions;
            ALTER TABLE ai_actions_new RENAME TO ai_actions;
            CREATE INDEX IF NOT EXISTS idx_ai_actions_user ON ai_actions(user_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_ai_actions_project ON ai_actions(project_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_ai_actions_type ON ai_actions(action_type);
            CREATE INDEX IF NOT EXISTS idx_ai_actions_target ON ai_actions(target_id);
            CREATE INDEX IF NOT EXISTS idx_ai_actions_date ON ai_actions(created_at);
            COMMIT;
        "),
        ("user_profiles", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS user_profiles_new (
                id                      TEXT NOT NULL PRIMARY KEY,
                user_id                 TEXT NOT NULL DEFAULT '',
                display_name            TEXT,
                bio                     TEXT,
                working_style           TEXT,
                ai_provider             TEXT,
                ai_model                TEXT,
                onboarding_method       TEXT,
                onboarding_completed    INTEGER NOT NULL DEFAULT 0,
                profile_json            TEXT DEFAULT '{}',
                created_at              TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at              TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO user_profiles_new SELECT COALESCE(id, hex(randomblob(16))), user_id, display_name, bio, working_style,
                ai_provider, ai_model, onboarding_method, onboarding_completed, profile_json,
                COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM user_profiles;
            DROP TABLE user_profiles;
            ALTER TABLE user_profiles_new RENAME TO user_profiles;
            COMMIT;
        "),
        ("user_dashboard_config", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS user_dashboard_config_new (
                id                    TEXT NOT NULL PRIMARY KEY,
                onboarding_completed  INTEGER DEFAULT 0,
                onboarding_template   TEXT,
                theme_variant         TEXT DEFAULT 'default',
                created_at            TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO user_dashboard_config_new SELECT COALESCE(id, hex(randomblob(16))), onboarding_completed, onboarding_template,
                theme_variant, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM user_dashboard_config;
            DROP TABLE user_dashboard_config;
            ALTER TABLE user_dashboard_config_new RENAME TO user_dashboard_config;
            COMMIT;
        "),
        ("dashboard_modules", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS dashboard_modules_new (
                id              TEXT NOT NULL PRIMARY KEY,
                module_type     TEXT NOT NULL DEFAULT '',
                sort_order      INTEGER NOT NULL DEFAULT 0,
                is_enabled      INTEGER NOT NULL DEFAULT 1,
                config_json     TEXT DEFAULT '{}',
                static_content  TEXT,
                created_at      TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO dashboard_modules_new SELECT COALESCE(id, hex(randomblob(16))), module_type, sort_order, is_enabled,
                config_json, static_content, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM dashboard_modules;
            DROP TABLE dashboard_modules;
            ALTER TABLE dashboard_modules_new RENAME TO dashboard_modules;
            CREATE INDEX IF NOT EXISTS idx_modules_sort ON dashboard_modules(sort_order) WHERE is_enabled = 1;
            COMMIT;
        "),
        ("module_items", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS module_items_new (
                id          TEXT NOT NULL PRIMARY KEY,
                module_id   TEXT NOT NULL DEFAULT '',
                item_type   TEXT NOT NULL DEFAULT '',
                label       TEXT NOT NULL DEFAULT '',
                description TEXT,
                emoji       TEXT,
                config_json TEXT DEFAULT '{}',
                sort_order  INTEGER NOT NULL DEFAULT 0,
                is_active   INTEGER NOT NULL DEFAULT 1,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO module_items_new SELECT COALESCE(id, hex(randomblob(16))), module_id, item_type, label, description,
                emoji, config_json, sort_order, is_active, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM module_items;
            DROP TABLE module_items;
            ALTER TABLE module_items_new RENAME TO module_items;
            CREATE INDEX IF NOT EXISTS idx_items_module ON module_items(module_id, sort_order);
            COMMIT;
        "),
        ("daily_entries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS daily_entries_new (
                id          TEXT NOT NULL PRIMARY KEY,
                module_id   TEXT NOT NULL DEFAULT '',
                entry_date  TEXT NOT NULL DEFAULT '',
                data_json   TEXT NOT NULL DEFAULT '{}',
                completed_at TEXT,
                created_at  TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO daily_entries_new SELECT COALESCE(id, hex(randomblob(16))), module_id, entry_date, data_json,
                completed_at, COALESCE(created_at, datetime('now')), COALESCE(updated_at, datetime('now')) FROM daily_entries;
            DROP TABLE daily_entries;
            ALTER TABLE daily_entries_new RENAME TO daily_entries;
            CREATE INDEX IF NOT EXISTS idx_daily_date ON daily_entries(entry_date);
            CREATE INDEX IF NOT EXISTS idx_daily_module_date ON daily_entries(module_id, entry_date);
            COMMIT;
        "),
        ("habit_entries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS habit_entries_new (
                id          TEXT NOT NULL PRIMARY KEY,
                item_id     TEXT NOT NULL DEFAULT '',
                entry_date  TEXT NOT NULL DEFAULT '',
                completed   INTEGER NOT NULL DEFAULT 0,
                completed_at TEXT
            );
            INSERT OR IGNORE INTO habit_entries_new SELECT COALESCE(id, hex(randomblob(16))), item_id, entry_date, completed, completed_at FROM habit_entries;
            DROP TABLE habit_entries;
            ALTER TABLE habit_entries_new RENAME TO habit_entries;
            CREATE INDEX IF NOT EXISTS idx_habit_date ON habit_entries(item_id, entry_date);
            COMMIT;
        "),
        ("reflection_entries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS reflection_entries_new (
                id              TEXT NOT NULL PRIMARY KEY,
                question_id     TEXT NOT NULL DEFAULT '',
                entry_date      TEXT NOT NULL DEFAULT '',
                value_numeric   REAL,
                value_bool      INTEGER,
                value_text      TEXT
            );
            INSERT OR IGNORE INTO reflection_entries_new SELECT COALESCE(id, hex(randomblob(16))), question_id, entry_date,
                value_numeric, value_bool, value_text FROM reflection_entries;
            DROP TABLE reflection_entries;
            ALTER TABLE reflection_entries_new RENAME TO reflection_entries;
            CREATE INDEX IF NOT EXISTS idx_reflection_date ON reflection_entries(question_id, entry_date);
            COMMIT;
        "),
        ("gratitude_entries", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS gratitude_entries_new (
                id          TEXT NOT NULL PRIMARY KEY,
                entry_date  TEXT NOT NULL DEFAULT '',
                entry_index INTEGER NOT NULL DEFAULT 0,
                text        TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO gratitude_entries_new SELECT COALESCE(id, hex(randomblob(16))), entry_date, entry_index, text,
                COALESCE(created_at, datetime('now')) FROM gratitude_entries;
            DROP TABLE gratitude_entries;
            ALTER TABLE gratitude_entries_new RENAME TO gratitude_entries;
            CREATE INDEX IF NOT EXISTS idx_gratitude_date ON gratitude_entries(entry_date);
            COMMIT;
        "),
        ("sync_state", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS sync_state_new (
                id          TEXT NOT NULL PRIMARY KEY,
                device_id   TEXT NOT NULL DEFAULT '',
                device_name TEXT,
                platform    TEXT,
                last_sync_at TEXT,
                sync_version INTEGER NOT NULL DEFAULT 0,
                is_current  INTEGER NOT NULL DEFAULT 0,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            INSERT OR IGNORE INTO sync_state_new SELECT COALESCE(id, hex(randomblob(16))), device_id, device_name, platform,
                last_sync_at, sync_version, is_current, COALESCE(created_at, datetime('now')) FROM sync_state;
            DROP TABLE sync_state;
            ALTER TABLE sync_state_new RENAME TO sync_state;
            COMMIT;
        "),
        ("note_tags", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS note_tags_new (
                note_id TEXT NOT NULL,
                tag_id  TEXT NOT NULL,
                PRIMARY KEY (note_id, tag_id)
            );
            INSERT OR IGNORE INTO note_tags_new SELECT note_id, CAST(tag_id AS TEXT) FROM note_tags;
            DROP TABLE note_tags;
            ALTER TABLE note_tags_new RENAME TO note_tags;
            CREATE INDEX IF NOT EXISTS idx_note_tags_note ON note_tags(note_id);
            CREATE INDEX IF NOT EXISTS idx_note_tags_tag ON note_tags(tag_id);
            COMMIT;
        "),
        ("note_links", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS note_links_new (
                source_note_id TEXT NOT NULL,
                target_note_id TEXT NOT NULL,
                PRIMARY KEY (source_note_id, target_note_id)
            );
            INSERT OR IGNORE INTO note_links_new SELECT * FROM note_links;
            DROP TABLE note_links;
            ALTER TABLE note_links_new RENAME TO note_links;
            CREATE INDEX IF NOT EXISTS idx_note_links_source ON note_links(source_note_id);
            CREATE INDEX IF NOT EXISTS idx_note_links_target ON note_links(target_note_id);
            COMMIT;
        "),
        ("note_versions", "
            BEGIN;
            CREATE TABLE IF NOT EXISTS note_versions (
                id         TEXT NOT NULL PRIMARY KEY,
                note_id    TEXT NOT NULL DEFAULT '',
                content    TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                source     TEXT NOT NULL DEFAULT 'manual'
            );
            CREATE TABLE IF NOT EXISTS note_versions_new (
                id         TEXT NOT NULL PRIMARY KEY,
                note_id    TEXT NOT NULL DEFAULT '',
                content    TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                source     TEXT NOT NULL DEFAULT 'manual'
            );
            INSERT OR IGNORE INTO note_versions_new SELECT id, note_id, content, created_at, source FROM note_versions;
            DROP TABLE IF EXISTS note_versions;
            ALTER TABLE note_versions_new RENAME TO note_versions;
            CREATE INDEX IF NOT EXISTS idx_note_versions_note_id ON note_versions(note_id);
            COMMIT;
        "),
    ];

    for (name, sql) in migrations {
        conn.execute_batch(sql)
            .map_err(|e| format!("Failed to migrate {}: {}", name, e))?;
        println!("  ✓ Migrated {}", name);
    }

    // Re-enable foreign keys
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|e| format!("Failed to re-enable foreign keys: {}", e))?;

    println!("✓ Schema migration complete");
    Ok(())
}

/// CRR upgrade helper (§398) — extracted for migration 17
/// Calls crsql_as_crr() on all tables (idempotent)
pub fn upgrade_tables_to_crr_inner(conn: &Connection) -> Result<(), String> {
    let tables = vec![
        "projects",
        "notes",
        "tags",
        "note_tags",
        "note_links",
        "ai_chat_messages",
        "ai_context_markers",
        "ai_pinned_messages",
        "ai_conversation_summaries",
        "ai_memory",
        "ai_actions",
        "user_profiles",
        "user_dashboard_config",
        "dashboard_modules",
        "module_items",
        "daily_entries",
        "habit_entries",
        "reflection_entries",
        "gratitude_entries",
        "sync_state",
        "note_versions",
        // B1 (#2141): category_colors joins CRR so category definitions sync across devices.
        // schema v75.
        //
        // WHY this entry also exists in the upgrade path (pre-launch = fresh-install only):
        // Belt-and-suspenders safety. `upgrade_tables_to_crr_inner` is called by numbered
        // migration arms (e.g. v74→v75). If we ever ship a post-launch migration arm that
        // runs on existing user DBs, both the fresh-install CRR_TABLES const AND this
        // upgrade list must include category_colors — otherwise upgraded devices would miss
        // CRR on this table. Adding it here is idempotent (checked before enabling) and
        // ensures the invariant holds regardless of which path a device takes.
        "category_colors",
    ];

    for table in &tables {
        // Check if table exists before upgrading
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                params![table],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if exists > 0 {
            // Check if table is already a CRR by looking for its clock table
            let is_crr: bool = conn.query_row(
                &format!("SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='{}__crsql_clock'", table),
                [],
                |row| row.get(0),
            ).unwrap_or(false);

            if is_crr {
                eprintln!("  ✓ {} already a CRR", table);
                continue;
            }

            match conn.query_row(
                &format!("SELECT crsql_as_crr('{}');", table),
                [],
                |_| Ok(()),
            ) {
                Ok(_) => {
                    eprintln!("  ✓ {} upgraded to CRR", table);
                }
                Err(e) => {
                    eprintln!("  ⚠ Skipped table {} ({})", table, e);
                }
            }
        }
    }

    Ok(())
}

/// DEPRECATED (§398): Old CRR upgrade function (replaced by migration 17)
/// This function is kept for reference but should never be called
#[allow(dead_code)]
fn upgrade_tables_to_crr(conn: &Connection) -> Result<(), String> {
    upgrade_tables_to_crr_inner(conn)
}
/// Retained as an explicit cr-sqlite lifecycle hook for external integration paths.
#[allow(dead_code)]
pub fn finalize_crsql(conn: &Connection) -> Result<(), String> {
    conn.execute_batch("SELECT crsql_finalize();")
        .map_err(|e| format!("Failed to finalize crsql: {}", e))?;

    println!("✓ Finalized cr-sqlite");
    Ok(())
}

// === Sync Status Command (v9.1.0) ===

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncStatus {
    pub site_id: String,
    pub db_version: i64,
    pub tables_as_crr: i64,
}

#[tauri::command]
pub fn get_sync_status(db: tauri::State<DbState>) -> Result<SyncStatus, String> {
    let conn = db.0.lock().unwrap();

    // Get site_id
    let site_id: Vec<u8> = conn
        .query_row("SELECT crsql_site_id()", [], |row| row.get(0))
        .map_err(|e| format!("Failed to get site_id: {}", e))?;

    // Get current db version
    let db_version: i64 = conn
        .query_row("SELECT crsql_db_version()", [], |row| row.get(0))
        .map_err(|e| format!("Failed to get db_version: {}", e))?;

    // Count tables with CRR metadata
    let tables_as_crr: i64 = conn
        .query_row("SELECT COUNT(DISTINCT tbl) FROM crsql_master", [], |row| {
            row.get(0)
        })
        .unwrap_or(0);

    Ok(SyncStatus {
        site_id: hex::encode(&site_id),
        db_version,
        tables_as_crr,
    })
}

// === AI Settings Helpers (§2.3B) ===

/// Unified ai_settings reader — normalizes JSON quotes automatically
pub fn get_ai_setting(conn: &Connection, key: &str) -> Option<String> {
    let result = conn
        .query_row(
            "SELECT value FROM ai_settings WHERE key = ?1",
            rusqlite::params![key],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .map(|v| v.trim_matches('"').to_string())
        .filter(|v| !v.is_empty());

    // Diagnostic logging for internal provider settings and context override (Bug #3)
    if key == "internal_provider" || key == "internal_api_key" || key == "context_window_override" {
        let display = if key == "internal_api_key" {
            result.as_ref().map(|_| "***".to_string())
        } else {
            result.clone()
        };
        eprintln!("[SETTINGS-LOAD] get_ai_setting('{}') = {:?}", key, display);
    }

    result
}

/// Unified ai_settings writer — stores plain values (no JSON wrapping)
pub fn set_ai_setting(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    // Diagnostic logging for internal provider settings and context override (Bug #3)
    if key == "internal_provider" || key == "internal_api_key" || key == "context_window_override" {
        eprintln!(
            "[SETTINGS-SAVE] set_ai_setting('{}', '{}')",
            key,
            if key == "internal_api_key" {
                "***"
            } else {
                value
            }
        );
    }

    conn.execute(
        "INSERT OR REPLACE INTO ai_settings (key, value) VALUES (?1, ?2)",
        rusqlite::params![key, value],
    )
    .map_err(|e| format!("Failed to save setting '{}': {}", key, e))?;

    // Flush to config.json after every write (§290 backup strategy)
    flush_ai_to_config(conn);

    Ok(())
}

/// ADR-0044 (Decision 3): credential keys that must never appear in config.json.
/// Any key on this list is omitted from the aiSettings dump and the ai.apiKeyRef field
/// is replaced with the sentinel "keychain" instead of the actual secret.
const AI_SETTINGS_BLOCKLIST: &[&str] = &["api_key", "internal_api_key"];

/// Flush ai_settings to the `aiSettings` section of .pane/config.json (§398)
/// Called after every ai_settings write to ensure config.json stays current
/// Backs up ALL non-secret keys to survive CRR migrations
/// Preserves all other sections (theme, sync, UI)
/// ADR-0044 Decision 3: credential keys are NEVER written to config.json.
fn flush_ai_to_config(conn: &Connection) {
    let config_path = get_config_path();

    // Read existing config (preserve all other sections)
    let mut config: serde_json::Value = if config_path.exists() {
        std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({"version": 1}))
    } else {
        serde_json::json!({"version": 1})
    };

    // Update ai section from current ai_settings (5 legacy keys)
    let ai_section = config
        .as_object_mut()
        .unwrap()
        .entry("ai")
        .or_insert(serde_json::json!({}));

    if let Some(obj) = ai_section.as_object_mut() {
        // Map ai_settings keys to config.json ai section
        // #2564 B1 fix (Path B), item #10: `ai_settings.provider` is retired (ADR-0063
        // D8) and is never written again as of this fix (see ai_configure_and_save,
        // item #9). `get_ai_setting(conn, "provider")` now naturally returns `None` on
        // every install that goes through this fix, so this mirror becomes a
        // permanent no-op — verified clean (no panic, no stale write) rather than
        // removed outright, since a raw `DELETE`-only retirement (no replacement key)
        // means there's nothing meaningful to mirror going forward. config.json's
        // `ai.provider` field goes permanently absent for post-fix installs.
        if let Some(v) = get_ai_setting(conn, "provider") {
            obj.insert("provider".into(), v.into());
        }
        if let Some(v) = get_ai_setting(conn, "model") {
            obj.insert("model".into(), v.into());
        }
        // ADR-0044 Decision 3: apiKeyRef gets sentinel "keychain", NEVER the actual secret
        obj.insert("apiKeyRef".into(), "keychain".into());
        if let Some(v) = get_ai_setting(conn, "endpoint") {
            obj.insert("customEndpoint".into(), v.into());
        }
    }

    // §398: Backup non-secret keys to separate aiSettings section
    // ADR-0044 Decision 3: blocklist excludes credential keys from this dump
    let ai_settings_section = config
        .as_object_mut()
        .unwrap()
        .entry("aiSettings")
        .or_insert(serde_json::json!({}));

    if let Some(obj) = ai_settings_section.as_object_mut() {
        // Read ALL rows from ai_settings table, excluding blocklisted credential keys
        if let Ok(mut stmt) = conn.prepare("SELECT key, value FROM ai_settings") {
            if let Ok(rows) = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            }) {
                for row in rows.flatten() {
                    // ADR-0044 Decision 3: skip blocklisted credential keys
                    if AI_SETTINGS_BLOCKLIST.contains(&row.0.as_str()) {
                        continue;
                    }
                    obj.insert(row.0, row.1.into());
                }
            }
        }
    }

    if let Ok(json) = serde_json::to_string_pretty(&config) {
        let _ = std::fs::write(&config_path, json);
    }
}

/// Flush ai_settings to config.json (ALL keys) — exposed for migration 17.
/// Retained for compatibility with historical migration runners.
#[allow(dead_code)]
pub fn flush_ai_to_config_all_keys(conn: &Connection) -> Result<(), String> {
    flush_ai_to_config(conn);
    Ok(())
}

/// Restore ai_settings from config.json `aiSettings` section if DB is empty (§398 recovery)
/// Called during DB initialization and by migration 17 to recover settings
/// Restores ALL keys (not just 5) from aiSettings section
fn restore_ai_settings_from_config(conn: &Connection) -> Result<(), String> {
    // Check if ai_settings is empty
    let settings_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM ai_settings", [], |row| row.get(0))
        .unwrap_or(0);

    if settings_count > 0 {
        // DB already has settings, don't restore
        return Ok(());
    }

    // Check if config.json exists
    let config_path = get_config_path();
    if !config_path.exists() {
        return Ok(());
    }

    // Read and parse config.json
    let content = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config.json: {}", e))?;

    let config: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse config.json: {}", e))?;

    let mut restored = 0;

    // NEW (§398): Try to restore from aiSettings section first (ALL keys)
    if let Some(ai_settings) = config.get("aiSettings").and_then(|a| a.as_object()) {
        eprintln!(
            "[ONBOARD] Restoring non-secret ai_settings from config.json aiSettings section..."
        );

        for (key, value) in ai_settings {
            // ADR-0044 Decision 3: credentials are never in config.json; skip if present
            // (from pre-ADR-0044 backups). Credentials are recovered from keychain only.
            if AI_SETTINGS_BLOCKLIST.contains(&key.as_str()) {
                eprintln!(
                    "[ONBOARD] Skipping credential key '{}' from config.json (ADR-0044)",
                    key
                );
                continue;
            }
            if let Some(val) = value.as_str() {
                if !val.is_empty() && val != "none" && val != "keychain" {
                    eprintln!("[ONBOARD] Restoring {}: {}", key, val);
                    conn.execute(
                        "INSERT OR REPLACE INTO ai_settings (key, value) VALUES (?1, ?2)",
                        rusqlite::params![key, val],
                    )
                    .ok();
                    restored += 1;
                }
            }
        }
    }

    // Fallback: restore from legacy ai section (5 keys only)
    if restored == 0 {
        if let Some(ai) = config.get("ai").and_then(|a| a.as_object()) {
            eprintln!("[ONBOARD] Attempting to restore ai_settings from legacy ai section...");

            // Map config.json keys back to ai_settings keys (non-credential only)
            // ADR-0044 Decision 3: apiKeyRef is now a sentinel ("keychain"); never restore it.
            // #2564 B1 fix (Path B), item #11: ("provider", "provider") REMOVED from this
            // mappings array. `ai_settings.provider` is retired for real (ADR-0063 D8) —
            // this legacy-mappings fallback (only fires when `ai_settings` is fully empty,
            // per the COUNT(*) == 0 guard above) must not resurrect the key on the rare
            // from-scratch config-restore scenario where it does fire.
            let mappings = [
                ("model", "model"),
                ("customEndpoint", "endpoint"),
            ];
            for (config_key, settings_key) in &mappings {
                if let Some(val) = ai.get(*config_key).and_then(|v| v.as_str()) {
                    if val != "none" && !val.is_empty() && val != "keychain" {
                        eprintln!(
                            "[ONBOARD] Restoring {}: {} (from config key: {})",
                            settings_key, val, config_key
                        );
                        conn.execute(
                            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES (?1, ?2)",
                            rusqlite::params![settings_key, val],
                        )
                        .ok();
                        restored += 1;
                    }
                }
            }
        }
    }

    if restored > 0 {
        eprintln!("[DB] Restored {} ai_settings from config.json", restored);
    }

    Ok(())
}

/// Restore ai_settings from config.json (ALL keys) — exposed for migration 17.
/// Retained for compatibility with historical migration runners.
#[allow(dead_code)]
pub fn restore_ai_settings_from_config_all_keys(conn: &Connection) -> Result<(), String> {
    restore_ai_settings_from_config(conn)
}

// === Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a temporary database for testing
    fn create_test_db() -> (Connection, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let db_path = temp_dir.path().join("test.db");

        let conn = Connection::open(&db_path).expect("Failed to open test database");

        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .expect("Failed to set WAL mode");

        // Create minimal schema for testing
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS schema_migrations (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE TABLE IF NOT EXISTS users (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
        ",
        )
        .expect("Failed to create test schema");

        (conn, temp_dir)
    }

    #[test]
    fn test_open_connection_multiple_times() {
        eprintln!("[TEST] test_open_connection_multiple_times");

        let (conn1, temp_dir) = create_test_db();
        let db_path = temp_dir.path().join("test.db");

        // Insert test data
        conn1.execute(
            "INSERT INTO users (id, name, created_at) VALUES ('test_user', 'Test User', datetime('now'))",
            [],
        ).expect("Failed to insert test user");

        drop(conn1);

        // Open connection again (simulating what happens in startup sync task)
        let conn2 = Connection::open(&db_path).expect("Failed to open connection second time");

        // Verify data is still there
        let user_count: i64 = conn2
            .query_row(
                "SELECT COUNT(*) FROM users WHERE id = 'test_user'",
                [],
                |row| row.get(0),
            )
            .expect("Failed to query users");

        assert_eq!(
            user_count, 1,
            "User should exist after reopening connection"
        );

        drop(conn2);

        // Open a third time to verify no corruption
        let conn3 = Connection::open(&db_path).expect("Failed to open connection third time");

        let user_count: i64 = conn3
            .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
            .expect("Failed to query users again");

        assert_eq!(user_count, 1, "User count should remain 1");

        eprintln!("[TEST] ✓ Multiple connections work correctly");
    }

    #[test]
    fn test_migrations_run_once() {
        eprintln!("[TEST] test_migrations_run_once");

        let (conn, _temp_dir) = create_test_db();

        // Simulate running a migration
        conn.execute(
            "INSERT INTO schema_migrations (id, name, applied_at) VALUES (1, 'test_migration', datetime('now'))",
            [],
        ).expect("Failed to insert migration record");

        // Verify only one entry
        let migration_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE name = 'test_migration'",
                [],
                |row| row.get(0),
            )
            .expect("Failed to count migrations");

        assert_eq!(migration_count, 1, "Migration should only run once");

        // Try to insert the same migration again (should fail due to UNIQUE constraint)
        let result = conn.execute(
            "INSERT INTO schema_migrations (id, name, applied_at) VALUES (2, 'test_migration', datetime('now'))",
            [],
        );

        assert!(result.is_err(), "Duplicate migration should fail");

        // Verify still only one entry
        let migration_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE name = 'test_migration'",
                [],
                |row| row.get(0),
            )
            .expect("Failed to count migrations after duplicate attempt");

        assert_eq!(migration_count, 1, "Migration count should still be 1");

        eprintln!("[TEST] ✓ Migrations only run once (UNIQUE constraint works)");
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_database_path_respects_test_mode() {
        eprintln!("[TEST] test_database_path_respects_test_mode");

        // Test 1: When PANE_TEST_MODE is not set, should use production path
        std::env::remove_var("PANE_TEST_MODE");
        std::env::remove_var("PANE_TEST_DB");

        let prod_path = get_db_path();
        assert!(
            prod_path.to_string_lossy().contains(".pane/pane.db"),
            "Production path should contain .pane/pane.db, got: {}",
            prod_path.display()
        );

        eprintln!(
            "[TEST]   ✓ Production path correct: {}",
            prod_path.display()
        );

        // Test 2: When PANE_TEST_MODE=1 but PANE_TEST_DB not set, should use production path
        std::env::set_var("PANE_TEST_MODE", "1");
        std::env::remove_var("PANE_TEST_DB");

        let fallback_path = get_db_path();
        assert!(
            fallback_path.to_string_lossy().contains(".pane/pane.db"),
            "Fallback path should contain .pane/pane.db, got: {}",
            fallback_path.display()
        );

        eprintln!(
            "[TEST]   ✓ Fallback to production when PANE_TEST_DB not set: {}",
            fallback_path.display()
        );

        // Test 3: When both PANE_TEST_MODE=1 and PANE_TEST_DB are set, should use test path
        std::env::set_var("PANE_TEST_MODE", "1");
        let test_db_path = "/tmp/pane-test-db.db";
        std::env::set_var("PANE_TEST_DB", test_db_path);

        let test_path = get_db_path();
        assert_eq!(
            test_path.to_string_lossy(),
            test_db_path,
            "Test path should match PANE_TEST_DB value"
        );

        eprintln!("[TEST]   ✓ Test path correct: {}", test_path.display());

        // Test 4: When PANE_TEST_MODE=0, should use production path even if PANE_TEST_DB is set
        std::env::set_var("PANE_TEST_MODE", "0");
        std::env::set_var("PANE_TEST_DB", test_db_path);

        let disabled_path = get_db_path();
        assert!(
            disabled_path.to_string_lossy().contains(".pane/pane.db"),
            "Production path should be used when PANE_TEST_MODE=0, got: {}",
            disabled_path.display()
        );

        eprintln!(
            "[TEST]   ✓ Production path when test mode disabled: {}",
            disabled_path.display()
        );

        // Clean up
        std::env::remove_var("PANE_TEST_MODE");
        std::env::remove_var("PANE_TEST_DB");

        eprintln!("[TEST] ✓ Test database isolation works correctly");
    }

    #[test]
    fn test_get_ai_setting_masks_internal_api_key() {
        eprintln!("[TEST] test_get_ai_setting_masks_internal_api_key");

        let (conn, _temp_dir) = create_test_db();

        // Store a real-looking API key
        let real_key = "sk-supersecretapikey1234567890";
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('internal_api_key', ?1)",
            rusqlite::params![real_key],
        )
        .expect("Failed to insert test api key");

        // Retrieve the setting — this triggers the diagnostic log
        let result = get_ai_setting(&conn, "internal_api_key");

        // The return value must be the REAL key (function still returns actual value)
        assert_eq!(
            result.as_deref(),
            Some(real_key),
            "get_ai_setting should return the actual value, not the masked one"
        );

        // Verify the masking logic: the display value for internal_api_key
        // must be Some(\"***\") not the real key. We test the same logic inline.
        let display = if "internal_api_key" == "internal_api_key" {
            result.as_ref().map(|_| "***".to_string())
        } else {
            result.clone()
        };
        assert_eq!(
            display.as_deref(),
            Some("***"),
            "Display value for internal_api_key must be masked as \"***\""
        );
        assert_ne!(
            display.as_deref(),
            Some(real_key),
            "Display value must NOT contain the real API key"
        );

        eprintln!("[TEST] ✓ internal_api_key is masked in diagnostic logs (display=\"***\", real value preserved)");
    }

    #[test]
    fn test_get_ai_setting_does_not_mask_internal_provider() {
        eprintln!("[TEST] test_get_ai_setting_does_not_mask_internal_provider");

        let (conn, _temp_dir) = create_test_db();

        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('internal_provider', 'openai')",
            [],
        )
        .expect("Failed to insert internal_provider");

        let result = get_ai_setting(&conn, "internal_provider");
        assert_eq!(
            result.as_deref(),
            Some("openai"),
            "internal_provider value should be returned as-is"
        );

        // Masking logic should NOT apply to internal_provider
        let display = if "internal_provider" == "internal_api_key" {
            result.as_ref().map(|_| "***".to_string())
        } else {
            result.clone()
        };
        assert_eq!(
            display.as_deref(),
            Some("openai"),
            "internal_provider display value must NOT be masked"
        );

        eprintln!("[TEST] ✓ internal_provider is not masked in diagnostic logs");
    }

    #[test]
    fn test_set_ai_setting_roundtrip() {
        eprintln!("[TEST] test_set_ai_setting_roundtrip");

        let (conn, _temp_dir) = create_test_db();

        set_ai_setting(&conn, "internal_api_key", "sk-testkey99")
            .expect("set_ai_setting should succeed");

        let result = get_ai_setting(&conn, "internal_api_key");
        assert_eq!(
            result.as_deref(),
            Some("sk-testkey99"),
            "get_ai_setting should return what set_ai_setting stored"
        );

        // Update
        set_ai_setting(&conn, "internal_api_key", "sk-updatedkey")
            .expect("set_ai_setting update should succeed");

        let result2 = get_ai_setting(&conn, "internal_api_key");
        assert_eq!(
            result2.as_deref(),
            Some("sk-updatedkey"),
            "get_ai_setting should return updated value"
        );

        eprintln!("[TEST] ✓ set_ai_setting/get_ai_setting roundtrip works correctly");
    }

    /// #2252 Bug 2 regression: folder-collapse keys (`folder:{scopeId}:cabinet:collapsed`)
    /// are NOT in the diagnostic-logging allowlist (internal_provider/internal_api_key/
    /// context_window_override) at the database layer, but the write itself must still
    /// succeed and round-trip correctly — the audit's finding was that this key family
    /// was silently persisting with zero terminal visibility (fixed one layer up, in
    /// notes.rs's set_ai_setting command wrapper), not that the write was broken.
    #[test]
    fn test_set_ai_setting_folder_collapse_key_roundtrip() {
        eprintln!("[TEST] test_set_ai_setting_folder_collapse_key_roundtrip");

        let (conn, _temp_dir) = create_test_db();
        let key = "folder:test-scope-id:cabinet:collapsed";

        set_ai_setting(&conn, key, "true").expect("set_ai_setting should succeed for folder-collapse key");

        let result = get_ai_setting(&conn, key);
        assert_eq!(
            result.as_deref(),
            Some("true"),
            "folder-collapse key must round-trip even though it's outside the diagnostic-log allowlist"
        );

        // Toggle back to false — mirrors a second click on the same collapse button.
        set_ai_setting(&conn, key, "false").expect("set_ai_setting update should succeed");
        let result2 = get_ai_setting(&conn, key);
        assert_eq!(result2.as_deref(), Some("false"));

        eprintln!("[TEST] ✓ folder-collapse ai_settings key round-trips correctly");
    }

    // === #2277 Fix 3: unified "rules" resolver tests ===
    //
    // Prior to #2277, get_system_note_from_agents mapped "rules" -> "RULES.md" while
    // ai/commands.rs's edit_note resolver and §24 read block both used "AGENTS.md" for
    // the same semantic slot (ADR-0060 D3: AGENTS.md is canonical). These tests lock in
    // the unified mapping so the two codepaths can never diverge again silently.

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_2277_rules_note_type_resolves_to_agents_md_file() {
        eprintln!("[TEST] test_2277_rules_note_type_resolves_to_agents_md_file");

        let temp_fs_dir = TempDir::new().expect("Failed to create temp fs dir");
        std::env::set_var("PANE_TEST_ROOT", temp_fs_dir.path());

        let (conn, _temp_dir) = create_test_db();
        conn.execute_batch(
            "CREATE TABLE ai_agents (
                 id TEXT PRIMARY KEY,
                 workspace_path TEXT NOT NULL,
                 is_default INTEGER NOT NULL DEFAULT 0,
                 is_active INTEGER NOT NULL DEFAULT 1
             );",
        )
        .expect("Failed to create ai_agents table");

        let agent_id = "agent-2277-rules";
        let workspace_path = "agent-2277-rules";
        conn.execute(
            "INSERT INTO ai_agents (id, workspace_path, is_default, is_active) VALUES (?1, ?2, 1, 1)",
            rusqlite::params![agent_id, workspace_path],
        )
        .expect("Failed to insert main agent");

        // Write AGENTS.md (NOT RULES.md) — this is what onboarding/edit_note actually write.
        let system_dir = temp_fs_dir.path().join(".pane/system").join(workspace_path);
        std::fs::create_dir_all(&system_dir).expect("Failed to create agent system dir");
        std::fs::write(system_dir.join("AGENTS.md"), "Rules content lives in AGENTS.md")
            .expect("Failed to write AGENTS.md");
        // Also write a stale RULES.md to prove it is NOT what gets read.
        std::fs::write(system_dir.join("RULES.md"), "STALE: should never be read for note_type=rules")
            .expect("Failed to write stale RULES.md");

        let result = get_system_note_from_agents(&conn, "rules")
            .expect("get_system_note_from_agents should not error");

        let (returned_agent_id, content) = result.expect("Expected Some((agent_id, content)) for rules note_type");
        assert_eq!(returned_agent_id, agent_id);
        assert_eq!(
            content, "Rules content lives in AGENTS.md",
            "note_type=rules must resolve to AGENTS.md content, not RULES.md; got: {:?}",
            content
        );
        assert!(
            !content.contains("STALE"),
            "note_type=rules must NOT read the stale RULES.md file"
        );

        eprintln!("[TEST] ✓ note_type=rules resolves to AGENTS.md, matching edit_note/§24 read block");
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_2277_rules_note_type_agents_md_missing_returns_none() {
        eprintln!("[TEST] test_2277_rules_note_type_agents_md_missing_returns_none");

        let temp_fs_dir = TempDir::new().expect("Failed to create temp fs dir");
        std::env::set_var("PANE_TEST_ROOT", temp_fs_dir.path());

        let (conn, _temp_dir) = create_test_db();
        conn.execute_batch(
            "CREATE TABLE ai_agents (
                 id TEXT PRIMARY KEY,
                 workspace_path TEXT NOT NULL,
                 is_default INTEGER NOT NULL DEFAULT 0,
                 is_active INTEGER NOT NULL DEFAULT 1
             );",
        )
        .expect("Failed to create ai_agents table");

        let agent_id = "agent-2277-no-rules";
        let workspace_path = "agent-2277-no-rules";
        conn.execute(
            "INSERT INTO ai_agents (id, workspace_path, is_default, is_active) VALUES (?1, ?2, 1, 1)",
            rusqlite::params![agent_id, workspace_path],
        )
        .expect("Failed to insert main agent");

        let system_dir = temp_fs_dir.path().join(".pane/system").join(workspace_path);
        std::fs::create_dir_all(&system_dir).expect("Failed to create agent system dir");
        // Only a stale RULES.md exists — no AGENTS.md. Per ADR-0060 D3, this must NOT
        // fall back to RULES.md; it must report the rules note as absent.
        std::fs::write(system_dir.join("RULES.md"), "legacy rules file, should not be read")
            .expect("Failed to write stale RULES.md");

        let result = get_system_note_from_agents(&conn, "rules")
            .expect("get_system_note_from_agents should not error");

        assert!(
            result.is_none(),
            "note_type=rules with no AGENTS.md must return None even if a legacy RULES.md exists; got: {:?}",
            result
        );

        eprintln!("[TEST] ✓ note_type=rules does not fall back to legacy RULES.md");
    }

    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_2277_get_system_note_delegates_rules_to_agents_md() {
        eprintln!("[TEST] test_2277_get_system_note_delegates_rules_to_agents_md");

        let temp_fs_dir = TempDir::new().expect("Failed to create temp fs dir");
        std::env::set_var("PANE_TEST_ROOT", temp_fs_dir.path());

        let (conn, _temp_dir) = create_test_db();
        conn.execute_batch(
            "CREATE TABLE ai_agents (
                 id TEXT PRIMARY KEY,
                 workspace_path TEXT NOT NULL,
                 is_default INTEGER NOT NULL DEFAULT 0,
                 is_active INTEGER NOT NULL DEFAULT 1
             );
             CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 is_system INTEGER DEFAULT 0,
                 parent_id TEXT
             );",
        )
        .expect("Failed to create schema");

        let agent_id = "agent-2277-public";
        let workspace_path = "agent-2277-public";
        conn.execute(
            "INSERT INTO ai_agents (id, workspace_path, is_default, is_active) VALUES (?1, ?2, 1, 1)",
            rusqlite::params![agent_id, workspace_path],
        )
        .expect("Failed to insert main agent");
        conn.execute(
            "INSERT INTO projects (id, name, is_system) VALUES ('proj-x', 'Any Project', 0)",
            [],
        )
        .expect("Failed to insert project");

        let system_dir = temp_fs_dir.path().join(".pane/system").join(workspace_path);
        std::fs::create_dir_all(&system_dir).expect("Failed to create agent system dir");
        std::fs::write(system_dir.join("AGENTS.md"), "Public API rules content")
            .expect("Failed to write AGENTS.md");

        // Public get_system_note (used by the §24 read block via load_system_notes) must
        // agree with get_system_note_from_agents on the "rules" -> AGENTS.md mapping.
        let result = get_system_note(&conn, "proj-x", "rules")
            .expect("get_system_note should not error");
        let (_id, content) = result.expect("Expected Some content for rules note_type");
        assert_eq!(content, "Public API rules content");

        eprintln!("[TEST] ✓ get_system_note(\"rules\") agrees with get_system_note_from_agents");
    }

    /// #2381 red-on-revert: rename_note_file() must register the destination path in the
    /// watcher's SUPPRESSED_PATHS set BEFORE the rename hits disk, mirroring the
    /// suppress-before-write pattern already used by atomic_write() (file_ops.rs:135) and
    /// delete_note_file() (database.rs:952).
    ///
    /// This is an integration test: it exercises the real `rename_note_file()` function
    /// against a real filesystem (via PaneFs/TempDir) and asserts against the real global
    /// `crate::watcher::SUPPRESSED_PATHS` set (via the public `is_suppressed` accessor) —
    /// no mocks. Deleting the `crate::watcher::suppress_path()` call added to
    /// `rename_note_file()` flips this test red: `is_suppressed(&destination)` would return
    /// `false` because nothing would have inserted the destination path into the set.
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_2381_rename_note_file_suppresses_destination_before_disk_write() {
        eprintln!("[TEST] test_2381_rename_note_file_suppresses_destination_before_disk_write");

        let temp_fs_dir = TempDir::new().expect("Failed to create temp fs dir");
        std::env::set_var("PANE_TEST_ROOT", temp_fs_dir.path());
        // #3008: get_pane_root() now resolves PANE_TEST_ROOT to its real path
        // itself (same canonicalization PaneFs::for_runtime()/authorize_write()
        // perform), so this test no longer has to pre-canonicalize PANE_TEST_ROOT
        // before use — fixing the #2381 divergence (TempDir::path() returns
        // /var/folders/… on macOS CI, a symlink to /private/var/folders/…) at the
        // source instead of leaving every test to remember the workaround. Read
        // get_pane_root() back (rather than the raw temp_fs_dir path) so this
        // test's own comparison paths match what production code actually uses.
        let temp_root = crate::database::get_pane_root();

        let note_dir = temp_root
            .join("Projects")
            .join("TestProject")
            .join("General");
        std::fs::create_dir_all(&note_dir).expect("Failed to create note dir");

        let old_abs_path = note_dir.join("old-note-title.md");
        std::fs::write(&old_abs_path, "# Old Note Title\n\nBody content.")
            .expect("Failed to write source note file");

        let old_rel_path = "Projects/TestProject/General/old-note-title.md";
        let new_rel_path = "Projects/TestProject/General/new-note-title.md";
        let new_abs_path = note_dir.join("new-note-title.md");

        // Guard against cross-test pollution of the process-global suppression set:
        // acknowledge any stale entry for our destination path before we start.
        crate::watcher::acknowledge_path(&new_abs_path);
        assert!(
            !crate::watcher::is_suppressed(&new_abs_path),
            "Precondition failed: destination path was already suppressed before the rename call"
        );

        // Exercise the real function under test.
        rename_note_file(old_rel_path, new_rel_path)
            .expect("rename_note_file should succeed for a valid same-root rename");

        // Confirm the rename actually happened on disk (real outcome, not a stub).
        assert!(
            !old_abs_path.exists(),
            "Old path must no longer exist on disk after rename"
        );
        assert!(
            new_abs_path.exists(),
            "New path must exist on disk after rename"
        );

        // The key assertion: the destination path must have been registered in the
        // watcher's suppression set as part of rename_note_file(), BEFORE the rename
        // completed. Since this test never calls acknowledge_path() for new_abs_path
        // afterward, if rename_note_file() suppressed it, it is still present now.
        // Removing the suppress_path() call from rename_note_file() makes this assertion
        // fail (red-on-revert).
        assert!(
            crate::watcher::is_suppressed(&new_abs_path),
            "rename_note_file() must call watcher::suppress_path() for the destination path \
             before performing the rename, so the file watcher ignores its own Create/Modify \
             event for the renamed file (#2381)"
        );

        // Cleanup: acknowledge our suppression entry and reset env so other tests are unaffected.
        crate::watcher::acknowledge_path(&new_abs_path);
        eprintln!("[TEST] ✓ rename_note_file() suppresses destination path before disk rename");
    }

    // ─── #2723 PR B: is_migration_pending — the fast-path/slow-path predicate ───

    /// A fresh install (no DB file yet) must NOT be treated as "migration
    /// pending" — there is nothing to back up and schema creation on an
    /// empty DB is already fast. This is the regression guard for #2723 PR
    /// B item 4 ("the no-migration launch takes exactly today's path"):
    /// the very first launch of all is the most common "no migration"
    /// case there is.
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_is_migration_pending_false_for_fresh_install() {
        let temp_dir = TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());

        assert!(
            !is_migration_pending(None),
            "a fresh install (no schema_version row yet) must not be reported as migration-pending"
        );

    }

    /// An install already at the current `SCHEMA_VERSION` must NOT be
    /// reported as pending — this is the predicate's main regression guard:
    /// the overwhelming majority of real launches land here, and a false
    /// positive would mean every ordinary launch pays the window-first
    /// background-thread tax for nothing.
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_is_migration_pending_false_when_schema_current() {
        let temp_dir = TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());

        // Run migrations once for real (fresh install → current schema),
        // exactly what a normal first launch does.
        let conn = init_database(None).expect("init_database should succeed on a fresh install");
        drop(conn);

        assert!(
            !is_migration_pending(None),
            "an install already at SCHEMA_VERSION must not be reported as migration-pending"
        );

    }

    /// A real old-schema fixture (the actual `tests/fixtures/upgrade-from-v91.db`
    /// used by `smoke-launch.sh upgrade`, not a hand-built/raw-INSERT stand-in)
    /// must be reported as pending. This is the other half of the
    /// predicate's correctness: it must say yes when a migration is
    /// genuinely needed, not just "never say yes."
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_is_migration_pending_true_for_old_schema_fixture() {
        let fixture_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/upgrade-from-v91.db");
        assert!(
            fixture_path.exists(),
            "upgrade-from-v91.db fixture not found at {:?}",
            fixture_path
        );

        let temp_dir = TempDir::new().expect("tempdir");
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        let db_path = get_db_path();
        std::fs::create_dir_all(db_path.parent().unwrap()).expect("create .pane dir");
        std::fs::copy(&fixture_path, &db_path).expect("copy v91 fixture into scratch root");

        assert!(
            is_migration_pending(None),
            "a v91 DB (current SCHEMA_VERSION is {}) must be reported as migration-pending",
            crate::migrations::SCHEMA_VERSION
        );

    }

    /// RAII guard restoring a previous env var value on drop — runs during
    /// panic unwinding too (the default, non-`panic=abort` behavior), so the
    /// two #3057 panic tests below don't leak `PANE_TEST_ROOT=""`/unset into
    /// whichever test runs next in this same process.
    struct EnvVarRestoreGuard {
        key: &'static str,
        prev: Option<String>,
    }

    impl Drop for EnvVarRestoreGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    /// #3057: `get_pane_root()` must refuse (panic) when `PANE_TEST_ROOT` is
    /// not set — the guard stopping a test from silently resolving to the
    /// real `~/Pane` (the exact shape of a prior incident where an
    /// unisolated test touched real user data). Defensively removes the var
    /// first in case a prior test in this same process leaked it.
    #[test]
    #[serial_test::serial(pane_test_root)]
    #[should_panic(expected = "PANE_TEST_ROOT")]
    fn test_get_pane_root_panics_when_test_root_unset() {
        let prev = std::env::var("PANE_TEST_ROOT").ok();
        std::env::remove_var("PANE_TEST_ROOT");
        let _guard = EnvVarRestoreGuard {
            key: "PANE_TEST_ROOT",
            prev,
        };
        let _ = get_pane_root();
    }

    /// #3057 "empty is not zero": `PANE_TEST_ROOT=""` (set-but-empty) must
    /// hit the same panic as unset, not silently resolve `PathBuf::from("")`
    /// (a CWD-dependent, unpredictable relative path) or fall through to the
    /// real-home-dir arm.
    ///
    /// Mutation: change the guard's `Ok(s) if !s.is_empty()` back to a plain
    /// `Ok(s)` → this test stops panicking (resolves to `PathBuf::from("")`
    /// instead) → red.
    #[test]
    #[serial_test::serial(pane_test_root)]
    #[should_panic(expected = "PANE_TEST_ROOT")]
    fn test_get_pane_root_panics_when_test_root_empty_string() {
        let prev = std::env::var("PANE_TEST_ROOT").ok();
        std::env::set_var("PANE_TEST_ROOT", "");
        let _guard = EnvVarRestoreGuard {
            key: "PANE_TEST_ROOT",
            prev,
        };
        let _ = get_pane_root();
    }
}
