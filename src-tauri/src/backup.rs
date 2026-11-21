// backup.rs — Automated Database Backup System (§25)
// Built: 2026-04-04 (v0.17.71, issue #325)
//
// Architecture:
// - Daily automatic backups on first app launch each day
// - Pre-migration backups before schema changes
// - Manual backup/restore commands for Settings UI
// - Rolling retention: 7 daily + 4 weekly backups
// - Integrity checks on all backups
// - Flat file backup of ~/Pane/.pane/system/ alongside DB

use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use chrono::{DateTime, Local, Datelike, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::Emitter;

const DAILY_RETENTION_COUNT: usize = 7;
const WEEKLY_RETENTION_COUNT: usize = 4;

// ─── Concurrency Guard (R7 / #1921) ──────────────────────────────────────────

/// Prevents concurrent manual backups. Only the manual path acquires this guard;
/// pre-restore, migration, and daily callers are already serialized by their
/// calling context.
static BACKUP_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

struct BackupGuard;

impl BackupGuard {
    fn try_acquire() -> Result<Self, String> {
        if BACKUP_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("Backup already in progress".to_string());
        }
        Ok(BackupGuard)
    }
}

impl Drop for BackupGuard {
    fn drop(&mut self) {
        BACKUP_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

// ─── Copy Pacing (#2723) ──────────────────────────────────────────────────────
//
// The SQLite backup-API copy used to always run at a single, unexplained pace
// (5 pages / 250ms, ~80 KB/s) regardless of caller. That constant was introduced
// in b06459df (#325, 2026-04-04) with no comment, commit message, or doc
// reference, and was never revisited. Per Reed's audit
// (docs/audits/2026-10-23-scope-slow-launch-updating-screen.md §2), that pacing
// only matters when a *live* app could contend with the backup for the source
// connection or disk I/O. Every caller must now say explicitly which situation
// it is in — there is no default.

/// How fast the SQLite backup-API copy step (inside `create_backup`) should run.
/// Every caller of `create_backup` supplies this explicitly; it is derived from
/// the caller's actual situation, not a fresh hardcoded pair (#2723).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupPacing {
    /// No live reader/writer to protect against: pre-migration (no window has
    /// opened yet — `lib.rs`'s `.setup()` hasn't returned), daily (runs on a
    /// background thread per #353, but nothing else holds `source_conn`), and
    /// the pre-restore safety backup (the live `DbState` connection is already
    /// closed per ADR-0056 R2 before this call happens). Copies every
    /// remaining page in one step, no sleep.
    FullSpeed,
    /// A live app may be actively reading/writing the DB concurrently
    /// (Settings → manual backup, user-initiated while the app is in normal
    /// use). Preserves the pre-#2723 pacing exactly — 5 pages / 250ms,
    /// ~80 KB/s. This is NOT a newly-invented number: Matt's explicit
    /// instruction was not to touch live-app pacing, and the original
    /// rationale for *why* 5/250 specifically cannot be recovered (one
    /// commit, b06459df, no comment — see audit §2, "why does the throttle
    /// exist at all"). Named and documented here rather than left as a bare
    /// magic pair; revisit if/when a measured I/O-fairness budget replaces it.
    ThrottledForLiveApp,
}

impl BackupPacing {
    /// (pages_per_step, pause_between_pages) for `Backup::run_to_completion`.
    /// `-1` is the SQLite backup API's own convention for "copy every
    /// remaining page in a single step" — not a new magic number, it is the
    /// documented meaning of a negative `nPage` argument to
    /// `sqlite3_backup_step`.
    fn step_params(self) -> (i32, std::time::Duration) {
        match self {
            BackupPacing::FullSpeed => (-1, std::time::Duration::from_millis(0)),
            BackupPacing::ThrottledForLiveApp => (5, std::time::Duration::from_millis(250)),
        }
    }
}

// ─── Output Staging (#2723 item 3 / #3049 lesson) ──────────────────────────────
//
// Reuses the pattern already proven in `restore_backup_internal` (write to a
// staging path on the same filesystem, then atomic-rename into place) so a
// quit mid-write can never leave a half-written `.db` or `.tar.gz` sitting at
// the exact filename a complete backup would use. The DB itself was already
// safe (Reed's audit §5); this closes the gap on the backup's *output* files.

/// Staging path for a backup output file: `name.db` → `name.db-staging`,
/// `name.tar.gz` → `name.tar.gz-staging`. Mirrors the suffix convention
/// already used by restore (`db_path.with_extension("db-restoring")`).
fn staging_path_for(final_path: &Path) -> PathBuf {
    let ext = final_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    final_path.with_extension(format!("{}-staging", ext))
}

/// Remove a stale staging file left behind by a prior crashed/killed backup run,
/// mirroring the cleanup already done for restore staging (`backup.rs` restore
/// path, "Removed stale staging file").
fn clear_stale_staging(staging_path: &Path) {
    if staging_path.exists() {
        let _ = fs::remove_file(staging_path);
        eprintln!("[BACKUP] Removed stale staging file: {:?}", staging_path.file_name());
    }
}

/// Atomic-rename a staged backup output into its final, listed filename.
/// #3049 lesson: the rename's `Result` must propagate all the way out — a
/// failed rename is not success just because the staged write succeeded.
fn publish_staged(staging_path: &Path, final_path: &Path) -> Result<(), String> {
    fs::rename(staging_path, final_path).map_err(|e| {
        format!(
            "Failed to publish {:?} (staged write succeeded but rename failed): {}",
            final_path.file_name(),
            e
        )
    })
}

// ─── Progress Event Helper (R7 / #1921) ──────────────────────────────────────

/// Emit a `backup://progress` event to the frontend. No-op when `app_handle` is `None`.
fn emit_progress(
    app_handle: Option<&tauri::AppHandle>,
    phase: &str,
    message: &str,
    pct: Option<u8>,
) {
    if let Some(handle) = app_handle {
        let _ = handle.emit(
            "backup://progress",
            serde_json::json!({
                "phase": phase,
                "message": message,
                "pct": pct
            }),
        );
    }
}

/// All sidecar suffixes created alongside each .db backup.
/// This is the single source of truth for deletion — `delete_backup_and_sidecars()`
/// loops over this list.  If you add a new sidecar, add its suffix here.
const SIDECAR_SUFFIXES: &[&str] = &[
    "_system.tar.gz",
    "_install-uuid.json",
    "_keychain.json",
    "_metadata.json",
    "_assets.tar.gz",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupInfo {
    pub filename: String,
    pub path: String,
    pub timestamp: String,
    pub size_bytes: u64,
    pub backup_type: BackupType,
    pub has_flat_files: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    pub app_version: String,
    pub backup_timestamp: String,
    pub backup_type: String,
    pub account_email: Option<String>,
    pub install_uuid: String,
    pub pane_version_schema: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BackupType {
    Daily,
    Weekly,
    Migration,
    Manual,
    /// Automatic pre-restore safety snapshot (ADR-0056 R3).
    /// Produces `pane_pre-restore-TIMESTAMP.db`; excluded from rotation.
    PreRestore,
}

/// Non-fatal warning from a restore operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreWarning {
    pub class: String,      // e.g. "KeychainRestorePartial", "SystemFilesRestoreFailed"
    pub message: String,
}

/// Summary returned on successful restore
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RestoreSummary {
    pub warnings: Vec<RestoreWarning>,
    pub backup_metadata: Option<BackupMetadata>,
}

/// Fatal restore error
#[derive(Debug)]
pub enum RestoreError {
    SchemaTooNew { backup: i64, current: i64 },
    SchemaOlderConfirmationRequired { backup: i64, current: i64 },
    /// Retained so restore callers can represent an explicit cancellation outcome.
    #[allow(dead_code)]
    UserCancelled,
    DbRestoreFailed(String),
    InstallUuidRestoreFailed(String),
}

impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SchemaTooNew { backup, current } => 
                write!(f, "Backup schema {} is newer than current {}", backup, current),
            Self::SchemaOlderConfirmationRequired { backup, current } =>
                write!(f, "Backup schema {} is older than current {}", backup, current),
            Self::UserCancelled => write!(f, "User cancelled restore"),
            Self::DbRestoreFailed(msg) => write!(f, "DB restore failed: {}", msg),
            Self::InstallUuidRestoreFailed(msg) => write!(f, "Install UUID restore failed: {}", msg),
        }
    }
}

/// Get the backups directory path
pub fn get_backups_dir() -> PathBuf {
    crate::database::get_pane_root().join(".pane").join("backups")
}

/// Get the system files directory path (for flat file backup)
fn get_system_dir() -> PathBuf {
    crate::database::get_pane_root().join(".pane").join("system")
}

/// Get real schema version from the `ai_settings` table (R4 — replaces dead `PRAGMA user_version`).
///
/// `PRAGMA user_version` is always 0 under cr-sqlite; the real version lives in
/// `ai_settings.schema_version` (same source as `migrations.rs:361–367`).
/// Returns 0 when the `ai_settings` table is absent (fresh / pre-schema DB).
///
/// ADR-0056 R4; `backup.md` — schema guard reader fix.
fn get_real_schema_version(db_path: &Path) -> Result<i64, String> {
    let conn = Connection::open(db_path)
        .map_err(|e| format!("Failed to open database for schema version: {}", e))?;

    // Guard: ai_settings may not exist on very old / fresh DBs.
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        return Ok(0);
    }

    let version = crate::database::get_ai_setting(&conn, "schema_version")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);

    Ok(version)
}

/// Get account email from database (best effort)
fn get_account_email(db_path: &Path) -> Option<String> {
    let conn = Connection::open(db_path).ok()?;
    
    // Try to get email from users table
    let email: Option<String> = conn
        .query_row("SELECT email FROM users WHERE email IS NOT NULL LIMIT 1", [], |row| row.get(0))
        .ok();
    
    email
}

/// Get known keychain accounts for backup export (ADR-0026, ADR-0044).
///
/// Returns a statically enumerated list of all known keychain account labels:
/// - Supabase auth tokens (email-keyed, queried from DB)
/// - AI credentials (fixed constants, ADR-0044 Decision 4)
///
/// The AI credential accounts are fixed string constants so the exporter
/// can enumerate them without runtime derivation.
fn get_known_keychain_accounts(db_path: &Path) -> Vec<String> {
    let mut accounts = Vec::new();
    
    // ADR-0044 Decision 4: always include the three AI credential accounts
    // These are fixed string constants — no runtime derivation required.
    for account in crate::ai::credential_migration::ALL_AI_KEYCHAIN_ACCOUNTS {
        accounts.push(account.to_string());
    }

    // ADR-0044 Addendum A (#1498): sync master key account — single fixed label,
    // not email-scoped. Must be included so keychain export carries the encryption root.
    // Without it, a restored machine holds synced ciphertext it cannot decrypt.
    accounts.push(crate::sync::keystore::KEYCHAIN_ACCOUNT.to_string());

    // Supabase session tokens (#1008) — re-login-recoverable, but included for backup
    // completeness so a restored machine retains an active session without forcing
    // immediate re-authentication. Fixed labels, not email-scoped.
    accounts.push("supabase_access_token".to_string());
    accounts.push("supabase_refresh_token".to_string());

    // Also include Supabase email-keyed entries (existing behavior)
    if let Ok(conn) = Connection::open(db_path) {
        if let Ok(mut stmt) = conn.prepare("SELECT DISTINCT email FROM users WHERE email IS NOT NULL") {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let Ok(email) = row.get::<_, String>(0) {
                        accounts.push(email);
                    }
                }
            }
        }
    }
    
    accounts
}

/// Ensure backups directory exists
fn ensure_backups_dir() -> Result<PathBuf, String> {
    let dir = get_backups_dir();
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create backups directory: {}", e))?;
    Ok(dir)
}

/// Create a timestamped backup of the database
/// Returns the path to the created backup
pub fn create_backup(
    db_path: &Path,
    backup_type: BackupType,
    version: Option<&str>,
    app_handle: Option<&tauri::AppHandle>,
    pacing: BackupPacing,
) -> Result<PathBuf, String> {
    let start = std::time::Instant::now();
    
    // Check if DB exists and has data
    if !db_path.exists() {
        return Err("Database file does not exist".to_string());
    }
    
    let file_size = fs::metadata(db_path)
        .map(|m| m.len())
        .map_err(|e| format!("Failed to read DB metadata: {}", e))?;
    
    if file_size < 4096 {
        return Err("Database too small (likely empty)".to_string());
    }
    
    ensure_backups_dir()?;
    
    // Generate backup filename
    let timestamp = Local::now().format("%Y%m%d_%H%M%S");
    let filename = match backup_type {
        BackupType::Migration => {
            let ver = version.unwrap_or("unknown");
            format!("pane_pre-migration-{}.db", ver)
        },
        BackupType::Manual => {
            format!("pane_manual-{}.db", timestamp)
        },
        BackupType::PreRestore => {
            format!("pane_pre-restore-{}.db", timestamp)
        },
        _ => {
            format!("pane_{}.db", timestamp)
        }
    };
    
    let backup_path = get_backups_dir().join(&filename);
    
    // Phase: backup_started (0%) — about to begin
    eprintln!("[BACKUP] Phase: backup_started — {:?} backup of {:?}", backup_type, db_path.file_name());
    emit_progress(app_handle, "backup_started", "Backup started", Some(0));

    // Phase: backup_flushing (5%) — flushing watcher debounce events
    eprintln!("[BACKUP] Phase: backup_flushing — flushing pending watcher changes");
    emit_progress(app_handle, "backup_flushing", "Flushing pending changes", Some(5));

    // Open source DB and use SQLite backup API for consistency
    let source_conn = Connection::open(db_path)
        .map_err(|e| format!("Failed to open source database: {}", e))?;

    // R5 (#1923): Flush pending file-watcher debounce events so notes typed within the
    // 2-second debounce window are imported to SQLite before the snapshot is taken.
    // This is a no-op if the watcher is not running (migration, restore, test paths).
    crate::watcher::flush_pending_watcher_events();

    // Phase: backup_checkpointing (10%) — WAL checkpoint
    eprintln!("[BACKUP] Phase: backup_checkpointing — WAL checkpoint");
    emit_progress(app_handle, "backup_checkpointing", "Checkpointing WAL", Some(10));

    // R6 (#1923): Checkpoint WAL and inspect result tuple (busy, log, checkpointed).
    // A partial/blocked checkpoint is non-fatal — proceed to backup per spec.
    let (wal_busy, wal_log, wal_checkpointed): (i64, i64, i64) = source_conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| format!("WAL checkpoint failed: {}", e))?;
    if wal_busy != 0 || wal_checkpointed < wal_log {
        eprintln!(
            "[BACKUP] WAL checkpoint partial: busy={}, log={}, checkpointed={}",
            wal_busy, wal_log, wal_checkpointed
        );
    } else {
        eprintln!("[BACKUP] WAL checkpoint complete: log={}, checkpointed={}", wal_log, wal_checkpointed);
    }

    // Phase: backup_copying (15%) — SQLite backup API copy
    eprintln!(
        "[BACKUP] Phase: backup_copying — pacing={:?} ({} pages/step, {:?} pause)",
        pacing,
        pacing.step_params().0,
        pacing.step_params().1
    );
    emit_progress(app_handle, "backup_copying", "Copying database", Some(15));

    // Stage the copy at a temp name on the same filesystem, so a kill mid-copy can never
    // leave a half-written `.db` visible at the final, listed filename (#2723 item 3;
    // reuses the pattern already shipped for restore at `restore_backup_internal`).
    let staging_db_path = staging_path_for(&backup_path);
    clear_stale_staging(&staging_db_path);

    // Use SQLite's backup API (handles WAL correctly)
    {
        let mut dest_conn = Connection::open(&staging_db_path)
            .map_err(|e| format!("Failed to create backup database: {}", e))?;
        {
            let backup = rusqlite::backup::Backup::new(&source_conn, &mut dest_conn)
                .map_err(|e| format!("Failed to initialize backup: {}", e))?;

            match pacing {
                BackupPacing::FullSpeed => {
                    // rusqlite's `run_to_completion` rejects a negative
                    // `pages_per_step` outright (it asserts `> 0`), but the
                    // underlying SQLite C API's own convention for "copy
                    // every remaining page right now" IS a negative count
                    // (`sqlite3_backup_step`, nPage < 0). Use `step()`
                    // directly to get that behavior. `Busy`/`Locked` are
                    // transient per rusqlite's own docs and only expected
                    // when a live connection contends with the backup —
                    // which, by construction, none of FullSpeed's callers
                    // have — so retrying immediately (no sleep) is
                    // defensive, not a reintroduced throttle.
                    use rusqlite::backup::StepResult;
                    loop {
                        match backup.step(-1).map_err(|e| format!("Backup execution failed: {}", e))? {
                            StepResult::Done => break,
                            StepResult::More | StepResult::Busy | StepResult::Locked => continue,
                            other => return Err(format!("Backup execution returned an unexpected result: {:?}", other)),
                        }
                    }
                }
                BackupPacing::ThrottledForLiveApp => {
                    let (pages_per_step, pause_between_pages) = pacing.step_params();
                    backup.run_to_completion(pages_per_step, pause_between_pages, None)
                        .map_err(|e| format!("Backup execution failed: {}", e))?;
                }
            }
        } // backup dropped here, releasing borrows
        // dest_conn closes here
    }

    // Close source connection
    drop(source_conn);

    // Phase: backup_verifying (75%) — integrity check (verify the STAGED file, before
    // it is ever visible at the final filename — a corrupt copy is caught before publish)
    eprintln!("[BACKUP] Phase: backup_verifying — integrity check on staged copy");
    emit_progress(app_handle, "backup_verifying", "Verifying integrity", Some(75));

    verify_backup_integrity(&staging_db_path)?;

    // Publish: atomic-rename the verified staged copy into its final, listed name.
    publish_staged(&staging_db_path, &backup_path)?;
    eprintln!("[BACKUP] Published database backup: {:?}", backup_path.file_name());

    let backup_size = fs::metadata(&backup_path)
        .map(|m| m.len())
        .unwrap_or(0);

    // Phase: backup_archiving (85%) — flat files
    // Skipped entirely for BackupType::Migration: no migration through the current
    // schema touches .pane/system/ (MIGRATION-DISK-OPS convention, migrations.rs:6-20),
    // and restore already treats a missing archive as valid (backup.rs restore STEP 5,
    // soft-fail / "older backup format" log line) — confirmed by execution, see test
    // `test_restore_migration_backup_without_system_archive`. Manual/Daily/Weekly/
    // PreRestore are unaffected and still get the archive unconditionally.
    if backup_type == BackupType::Migration {
        eprintln!("[BACKUP] Phase: backup_archiving — skipped for Migration backup (#2723 item 2; no migration touches .pane/system/)");
    } else {
        eprintln!("[BACKUP] Phase: backup_archiving — archiving system files");
        emit_progress(app_handle, "backup_archiving", "Archiving system files", Some(85));

        // Backup flat files (system directory)
        // NOTE: If adding a new sidecar, also add its suffix to SIDECAR_SUFFIXES (used by delete_backup_and_sidecars).
        let _flat_files_success = backup_flat_files(&filename, &timestamp.to_string()).is_ok();
    }

    // Media/Assets archive is intentionally NEVER produced for any backup type (#1915).
    // Assets trees grow to hundreds of GB / TB; users are expected to rely on their own
    // media backup solution (Time Machine, iCloud, etc.).
    // backup_attachment_files() is retained (deprecated) for follow-up cleanup only.
    eprintln!("[BACKUP] Skipping assets archive — media is user-managed (see #1915)");
    
    // Get install UUID (create if doesn't exist)
    let install_uuid = crate::install_uuid::get_or_create_install_uuid()
        .unwrap_or_else(|e| {
            eprintln!("[BACKUP] Failed to get install UUID: {}", e);
            "unknown".to_string()
        });
    
    // Phase: backup_finalizing (95%) — keychain export / UUID sidecar
    eprintln!("[BACKUP] Phase: backup_finalizing — keychain export + metadata sidecars");
    emit_progress(app_handle, "backup_finalizing", "Exporting keychain and metadata", Some(95));

    // Copy install-uuid.json to backup directory
    // NOTE: If adding a new sidecar, also add its suffix to SIDECAR_SUFFIXES (used by delete_backup_and_sidecars).
    let install_uuid_src = crate::install_uuid::get_install_uuid_path();
    if install_uuid_src.exists() {
        let install_uuid_dest = get_backups_dir().join(format!("{}_install-uuid.json", 
                                                                filename.trim_end_matches(".db")));
        if let Err(e) = fs::copy(&install_uuid_src, &install_uuid_dest) {
            eprintln!("[BACKUP] Failed to copy install-uuid.json: {}", e);
        } else {
            eprintln!("[BACKUP] Copied install-uuid.json to backup");
        }
    }
    
    // Export keychain items
    // NOTE: If adding a new sidecar, also add its suffix to SIDECAR_SUFFIXES (used by delete_backup_and_sidecars).
    let keychain_accounts = get_known_keychain_accounts(db_path);
    let keychain_export = crate::keychain::export_all_keychain_items(&keychain_accounts)
        .unwrap_or_else(|e| {
            eprintln!("[BACKUP] Failed to export keychain: {}", e);
            HashMap::new()
        });
    
    let keychain_filename = format!("{}_keychain.json", filename.trim_end_matches(".db"));
    let keychain_path = get_backups_dir().join(&keychain_filename);
    if let Ok(keychain_json) = serde_json::to_string_pretty(&keychain_export) {
        if let Err(e) = fs::write(&keychain_path, keychain_json) {
            eprintln!("[BACKUP] Failed to write keychain export: {}", e);
        } else {
            eprintln!("[BACKUP] Exported {} keychain items", keychain_export.len());
        }
    }
    
    // Create backup metadata
    // NOTE: If adding a new sidecar, also add its suffix to SIDECAR_SUFFIXES (used by delete_backup_and_sidecars).
    // R4: use real schema version from ai_settings (not dead PRAGMA user_version).
    let schema_version = get_real_schema_version(db_path).unwrap_or(0);
    let account_email = get_account_email(db_path);
    
    let metadata = BackupMetadata {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        backup_timestamp: timestamp.to_string(),
        backup_type: match backup_type {
            BackupType::Daily => "daily".to_string(),
            BackupType::Weekly => "weekly".to_string(),
            BackupType::Migration => "migration".to_string(),
            BackupType::Manual => "manual".to_string(),
            BackupType::PreRestore => "pre-restore".to_string(),
        },
        account_email,
        install_uuid,
        pane_version_schema: schema_version,
    };
    
    let metadata_filename = format!("{}_metadata.json", filename.trim_end_matches(".db"));
    let metadata_path = get_backups_dir().join(&metadata_filename);
    if let Ok(metadata_json) = serde_json::to_string_pretty(&metadata) {
        if let Err(e) = fs::write(&metadata_path, metadata_json) {
            eprintln!("[BACKUP] Failed to write metadata: {}", e);
        } else {
            eprintln!("[BACKUP] Created backup metadata");
        }
    }
    
    let duration = start.elapsed();
    eprintln!(
        "[BACKUP] Created {} backup: {} ({} bytes, {:.2}s)",
        match backup_type {
            BackupType::Daily => "daily",
            BackupType::Weekly => "weekly",
            BackupType::Migration => "migration",
            BackupType::Manual => "manual",
            BackupType::PreRestore => "pre-restore",
        },
        filename,
        backup_size,
        duration.as_secs_f64()
    );

    // Phase: backup_complete (100%)
    eprintln!("[BACKUP] Phase: backup_complete — {:.2}s total", duration.as_secs_f64());
    emit_progress(app_handle, "backup_complete", "Backup complete", Some(100));

    Ok(backup_path)
}

/// Verify backup integrity using SQLite's integrity_check
fn verify_backup_integrity(backup_path: &Path) -> Result<(), String> {
    let conn = Connection::open(backup_path)
        .map_err(|e| format!("Failed to open backup for verification: {}", e))?;
    
    let result: String = conn
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|e| format!("Integrity check query failed: {}", e))?;
    
    if result != "ok" {
        // Retry once
        eprintln!("[BACKUP] Integrity check failed, retrying...");
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        let retry_result: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(|e| format!("Integrity check retry failed: {}", e))?;
        
        if retry_result != "ok" {
            return Err(format!("Backup integrity check failed: {}", retry_result));
        }
    }

    eprintln!("[BACKUP] Integrity check passed");

    // === FTS5 notes_fts integrity-check (#3072) ===
    //
    // Same staged copy, same retry shape as the whole-DB check above — this
    // runs BEFORE the staged file is published, so a drifted/corrupt
    // external-content FTS5 index (notes_fts) is caught here, never visible
    // at the backup's final filename, matching the existing guarantee for
    // whole-DB corruption. Deliberately NOT added as a separate call site
    // (e.g. inline in the startup thread) — this reuses the retry/staging
    // logic this function already has for free.
    //
    // Unlike `PRAGMA integrity_check`, FTS5's `integrity-check` special
    // command raises an error (Err) on failure rather than returning a
    // non-"ok" row — handle that shape explicitly.
    //
    // `rank=1` is required, not optional: per the FTS5 docs (§6.5), for an
    // EXTERNAL CONTENT table, 'integrity-check' only compares the index
    // against the content table (`notes`) when rank=1 is supplied — without
    // it, only the index's own internal structure is checked and drift
    // against `notes` (the actual bug class this PR targets) would never be
    // caught.
    //
    // Does NOT touch `BackupPacing::FullSpeed` or the migration backup's
    // file-copy step — #3064's 11-minute-launch fix stays untouched. This
    // check only runs inside `create_backup` (Migration/Manual/Daily/Weekly/
    // PreRestore), never on every app launch.
    //
    // Skip gracefully if `notes_fts` doesn't exist on this DB at all (e.g.
    // many of this file's own lightweight test fixtures use a minimal
    // `test`-table-only schema with no `notes` or FTS tables at all) —
    // "nothing to check" is not the same claim as "check failed." Every real
    // Pane DB has `notes_fts` unconditionally from schema init
    // (migrations.rs::create_fts_virtual_tables), so this never skips in
    // production.
    let notes_fts_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'notes_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if notes_fts_exists {
        if let Err(e) = conn.execute("INSERT INTO notes_fts(notes_fts, rank) VALUES('integrity-check', 1)", []) {
            eprintln!("[BACKUP] notes_fts integrity-check failed ({}), retrying...", e);
            std::thread::sleep(std::time::Duration::from_millis(500));

            if let Err(retry_err) = conn.execute("INSERT INTO notes_fts(notes_fts, rank) VALUES('integrity-check', 1)", []) {
                return Err(format!("Backup notes_fts integrity-check failed: {}", retry_err));
            }
        }
        eprintln!("[BACKUP] notes_fts integrity-check passed");
    } else {
        eprintln!("[BACKUP] notes_fts table not present on this DB — skipping FTS5 integrity-check");
    }

    Ok(())
}

/// Backup flat files (system directory) to tar.gz
fn backup_flat_files(db_filename: &str, _timestamp: &str) -> Result<(), String> {
    let system_dir = get_system_dir();
    if !system_dir.exists() {
        return Ok(()); // No system files to backup
    }
    
    let backup_dir = get_backups_dir();
    let tar_name = db_filename.replace(".db", "_system.tar.gz");
    let tar_path = backup_dir.join(&tar_name);

    // Stage the archive at a temp name, then atomic-rename into place — same
    // reasoning as the `.db` staging above: a kill mid-archive must not leave a
    // half-written `.tar.gz` visible at the final, listed filename (#2723 item 3).
    let staging_tar_path = staging_path_for(&tar_path);
    clear_stale_staging(&staging_tar_path);

    // Use tar command to create archive
    let output = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&staging_tar_path)
        .arg("-C")
        .arg(system_dir.parent().unwrap())
        .arg("system")
        .output()
        .map_err(|e| format!("Failed to execute tar: {}", e))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let _ = fs::remove_file(&staging_tar_path);
        return Err(format!("tar failed: {}", stderr));
    }

    publish_staged(&staging_tar_path, &tar_path)?;

    let tar_size = fs::metadata(&tar_path)
        .map(|m| m.len())
        .unwrap_or(0);
    
    eprintln!("[BACKUP] Flat files archived: {} ({} bytes)", tar_name, tar_size);
    Ok(())
}

/// Create a daily backup (called on first app launch each day)
pub fn create_daily_backup(db_path: &Path) -> Result<PathBuf, String> {
    // Check if we already have a backup from today
    if has_backup_from_today()? {
        return Err("Already have a backup from today".to_string());
    }
    
    let now = Local::now();
    let backup_type = if is_week_boundary(&now) {
        BackupType::Weekly
    } else {
        BackupType::Daily
    };
    
    // FullSpeed (#2723): daily runs on a background thread, after the window
    // has already rendered (#353) — pacing here was never protecting the UI
    // thread, and there is no other holder of `source_conn` to be polite to.
    let backup_path = create_backup(db_path, backup_type, None, None, BackupPacing::FullSpeed)?;

    // Rotate old backups
    rotate_backups()?;

    Ok(backup_path)
}

/// Create a pre-migration backup.
/// FullSpeed (#2723 PR A): runs before any window exists and before any other
/// code has opened the DB — there is nothing else to be polite to. This is
/// the launch/migration backup that #2723 is about.
///
/// `app_handle` (#2723 PR B): `None` on the fast (unchanged, synchronous)
/// startup path — there is no window yet to show progress on, matching every
/// pre-PR-B call. `Some(handle)` on the window-first path, once `.setup()`
/// has already returned and the "Updating Pane…" screen is up — forwarded
/// straight to `create_backup`'s existing `backup://progress` phase events
/// (backup.rs:306–470), no new event name needed for the backup half.
pub fn create_migration_backup(
    db_path: &Path,
    version: &str,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<PathBuf, String> {
    create_backup(db_path, BackupType::Migration, Some(version), app_handle, BackupPacing::FullSpeed)
}

/// Create a manual backup (for Settings UI).
/// Acquires the RAII concurrency guard — rejects concurrent manual backup attempts.
/// ThrottledForLiveApp (#2723): the one caller where a user may actively be
/// using the app (typing, saving, syncing) while this runs. Matt's explicit
/// instruction: live-app callers keep their existing pacing.
pub fn create_manual_backup(
    db_path: &Path,
    app_handle: Option<&tauri::AppHandle>,
) -> Result<PathBuf, String> {
    let _guard = BackupGuard::try_acquire()?;
    let backup_path = create_backup(db_path, BackupType::Manual, None, app_handle, BackupPacing::ThrottledForLiveApp)?;
    rotate_backups()?;
    Ok(backup_path)
}

/// Check if we already have a backup from today
fn has_backup_from_today() -> Result<bool, String> {
    let backups = list_backups()?;
    let today = Local::now().format("%Y-%m-%d").to_string();
    
    // Check if ANY backup (daily, weekly, migration, manual) was created today
    // This prevents double backups when a migration backup already ran
    Ok(backups.iter().any(|b| b.timestamp.starts_with(&today)))
}

/// Check if this is a week boundary (Sunday)
fn is_week_boundary(dt: &DateTime<Local>) -> bool {
    dt.weekday() == chrono::Weekday::Sun
}

/// Parse timestamp from backup filename
fn parse_backup_timestamp(filename: &str) -> Option<DateTime<Local>> {
    // Extract timestamp portion (YYYYMMDD_HHMMSS)
    let timestamp_str = if filename.contains("pre-migration") {
        return None; // Migration backups don't have regular timestamps
    } else {
        filename
            .trim_start_matches("pane_")
            .trim_start_matches("manual-")
            .trim_end_matches(".db")
    };
    
    // Parse YYYYMMDD_HHMMSS
    if timestamp_str.len() >= 15 {
        let year = timestamp_str[0..4].parse::<i32>().ok()?;
        let month = timestamp_str[4..6].parse::<u32>().ok()?;
        let day = timestamp_str[6..8].parse::<u32>().ok()?;
        let hour = timestamp_str[9..11].parse::<u32>().ok()?;
        let minute = timestamp_str[11..13].parse::<u32>().ok()?;
        let second = timestamp_str[13..15].parse::<u32>().ok()?;
        
        Local.with_ymd_and_hms(year, month, day, hour, minute, second).earliest()
    } else {
        None
    }
}

/// Rotate backups: keep last 7 daily + last 4 weekly
fn rotate_backups() -> Result<(), String> {
    let backup_dir = get_backups_dir();
    
    let mut backups: Vec<_> = fs::read_dir(&backup_dir)
        .map_err(|e| format!("Failed to read backups directory: {}", e))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("pane_") && n.ends_with(".db"))
                .unwrap_or(false)
        })
        .collect();
    
    // Don't delete migration or manual backups
    backups.retain(|e| {
        let name = e.file_name();
        let name_str = name.to_str().unwrap_or("");
        !name_str.contains("pre-migration") && !name_str.contains("manual")
    });
    
    // Sort by modification time (oldest first)
    backups.sort_by_key(|e| e.metadata().ok().and_then(|m| m.modified().ok()));
    
    // Separate into daily and weekly
    let mut daily_backups = Vec::new();
    let mut weekly_backups = Vec::new();
    
    for entry in &backups {
        let filename = entry.file_name();
        let filename_str = filename.to_str().unwrap_or("");
        
        if let Some(dt) = parse_backup_timestamp(filename_str) {
            if is_week_boundary(&dt) {
                weekly_backups.push(entry);
            } else {
                daily_backups.push(entry);
            }
        }
    }
    
    // Delete old daily backups (keep last 7)
    if daily_backups.len() > DAILY_RETENTION_COUNT {
        for old in &daily_backups[..daily_backups.len() - DAILY_RETENTION_COUNT] {
            delete_backup_and_sidecars(old)?;
        }
    }
    
    // Delete old weekly backups (keep last 4)
    if weekly_backups.len() > WEEKLY_RETENTION_COUNT {
        for old in &weekly_backups[..weekly_backups.len() - WEEKLY_RETENTION_COUNT] {
            delete_backup_and_sidecars(old)?;
        }
    }
    
    Ok(())
}

/// Delete a backup .db and all associated sidecar files.
///
/// Sidecars are enumerated by `SIDECAR_SUFFIXES` — the single authoritative
/// list shared between creation and deletion. All suffix types (tar, keychain,
/// install-uuid, metadata) are cleaned up in one loop, preventing orphaned
/// credential files past their retention window (#1007).
fn delete_backup_and_sidecars(entry: &fs::DirEntry) -> Result<(), String> {
    let db_path = entry.path();
    let filename = entry.file_name();
    let filename_str = filename.to_str().unwrap_or("");
    
    // Delete DB file
    fs::remove_file(&db_path)
        .map_err(|e| format!("Failed to delete backup {}: {}", filename_str, e))?;
    
    eprintln!("[BACKUP] Rotated old backup: {}", filename_str);
    
    // Delete all associated sidecar files (tar, keychain, install-uuid, metadata)
    let backup_dir = get_backups_dir();
    for suffix in SIDECAR_SUFFIXES {
        let sidecar_name = filename_str.replace(".db", suffix);
        let sidecar_path = backup_dir.join(&sidecar_name);
        if sidecar_path.exists() {
            if let Err(e) = fs::remove_file(&sidecar_path) {
                eprintln!("[BACKUP] Warning: failed to delete sidecar {}: {}", sidecar_name, e);
            } else {
                eprintln!("[BACKUP] Rotated sidecar: {}", sidecar_name);
            }
        }
    }
    
    Ok(())
}

/// List all available backups
pub fn list_backups() -> Result<Vec<BackupInfo>, String> {
    let backup_dir = get_backups_dir();
    
    if !backup_dir.exists() {
        return Ok(Vec::new());
    }
    
    let mut backups: Vec<BackupInfo> = fs::read_dir(&backup_dir)
        .map_err(|e| format!("Failed to read backups directory: {}", e))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with("pane") && n.ends_with(".db"))
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            let filename = entry.file_name();
            let filename_str = filename.to_str()?.to_string();
            let path = entry.path();
            let path_str = path.to_str()?.to_string();
            
            let metadata = fs::metadata(&path).ok()?;
            let size_bytes = metadata.len();
            
            // Determine backup type
            let backup_type = if filename_str.contains("pre-migration") {
                BackupType::Migration
            } else if filename_str.contains("pre-restore") {
                BackupType::PreRestore
            } else if filename_str.contains("manual") {
                BackupType::Manual
            } else if let Some(dt) = parse_backup_timestamp(&filename_str) {
                if is_week_boundary(&dt) {
                    BackupType::Weekly
                } else {
                    BackupType::Daily
                }
            } else {
                return None;
            };
            
            // Extract timestamp for display — convert YYYYMMDD_HHMMSS to ISO 8601
            let timestamp = if filename_str.contains("pre-migration") {
                // Migration backups: use file modification time
                metadata.modified().ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| {
                        let dt = chrono::DateTime::from_timestamp(d.as_secs() as i64, 0)
                            .unwrap_or_default();
                        dt.format("%Y-%m-%dT%H:%M:%S").to_string()
                    })
                    .unwrap_or_else(|| "Unknown".to_string())
            } else if filename_str.contains("pre-restore") {
                // Pre-restore safety snapshots: pane_pre-restore-YYYYMMDD_HHMMSS.db
                let raw = filename_str
                    .replace("pane_pre-restore-", "")
                    .replace(".db", "");
                if raw.len() >= 15 {
                    format!("{}-{}-{}T{}:{}:{}",
                        &raw[0..4], &raw[4..6], &raw[6..8],
                        &raw[9..11], &raw[11..13], &raw[13..15])
                } else {
                    raw
                }
            } else {
                let raw = filename_str
                    .replace("pane_", "")
                    .replace("manual-", "")
                    .replace(".db", "");
                // Convert YYYYMMDD_HHMMSS → YYYY-MM-DDTHH:MM:SS
                if raw.len() >= 15 {
                    format!("{}-{}-{}T{}:{}:{}",
                        &raw[0..4], &raw[4..6], &raw[6..8],
                        &raw[9..11], &raw[11..13], &raw[13..15])
                } else {
                    raw
                }
            };
            
            // Check if flat files exist
            let tar_name = filename_str.replace(".db", "_system.tar.gz");
            let tar_path = get_backups_dir().join(&tar_name);
            let has_flat_files = tar_path.exists();
            
            Some(BackupInfo {
                filename: filename_str,
                path: path_str,
                timestamp,
                size_bytes,
                backup_type,
                has_flat_files,
            })
        })
        .collect();
    
    // Sort by timestamp (newest first)
    backups.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    
    Ok(backups)
}

/// Restore from a backup (replaces current database).
/// Implements the restore error contract per spec §6.1.4.1 + ADR-0056.
/// Requires user confirmation if backup schema < current schema.
///
/// INVARIANT (ADR-0056): The live `DbState` connection is closed before any
/// file-level swap occurs and reopened after. WAL/SHM sidecars are cleared
/// unconditionally. The Mutex is held for the full quiescence window so no
/// concurrent caller observes a `None` connection (they block, not panic).
pub fn restore_backup(
    backup_path: &Path,
    db_path: &Path,
    db: &crate::database::DbState,
) -> Result<RestoreSummary, RestoreError> {
    restore_backup_internal(backup_path, db_path, db, false)
}

/// Restore from a backup with schema confirmation already obtained.
/// Use when the UI has already shown the schema-older confirmation dialog.
pub fn restore_backup_confirmed(
    backup_path: &Path,
    db_path: &Path,
    db: &crate::database::DbState,
) -> Result<RestoreSummary, RestoreError> {
    restore_backup_internal(backup_path, db_path, db, true)
}

/// Internal restore implementation — ADR-0056 quiescent restore sequence.
///
/// # Concurrent-access strategy
/// We hold the `DbState` Mutex guard for the full close→swap→clear→reopen
/// sequence. Any Tauri command that arrives during this window will block on
/// `db.0.lock()` and will be served the fresh connection once restore
/// completes. This is preferred over a `restore_in_progress` flag because
/// it is race-free: the only way to observe a `None` connection would require
/// acquiring the lock, which is impossible while we hold it.
fn restore_backup_internal(
    backup_path: &Path,
    db_path: &Path,
    db: &crate::database::DbState,
    skip_schema_confirmation: bool,
) -> Result<RestoreSummary, RestoreError> {
    eprintln!("[RESTORE] Starting restore from: {:?}", backup_path.file_name());

    if !backup_path.exists() {
        return Err(RestoreError::DbRestoreFailed("Backup file does not exist".to_string()));
    }

    let mut warnings = Vec::new();

    // ─ Phase A: Read-only preflight (no state mutation) ──────────────────────────────

    // A1: Read backup metadata
    let backup_filename = backup_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let metadata_backup = backup_filename.replace(".db", "_metadata.json");
    let metadata_path = get_backups_dir().join(&metadata_backup);

    let backup_metadata = if metadata_path.exists() {
        match fs::read_to_string(&metadata_path) {
            Ok(contents) => match serde_json::from_str::<BackupMetadata>(&contents) {
                Ok(metadata) => {
                    eprintln!(
                        "[RESTORE] Backup metadata: app_version={}, schema={}, install_uuid={}",
                        metadata.app_version, metadata.pane_version_schema, metadata.install_uuid
                    );
                    Some(metadata)
                }
                Err(e) => {
                    eprintln!("[RESTORE] Failed to parse metadata: {}", e);
                    None
                }
            },
            Err(e) => {
                eprintln!("[RESTORE] Failed to read metadata: {}", e);
                None
            }
        }
    } else {
        eprintln!("[RESTORE] No metadata file found");
        None
    };

    // A2: Schema compatibility check — R4: use real ai_settings version, not PRAGMA user_version
    if let Some(ref metadata) = backup_metadata {
        let current_schema = if db_path.exists() {
            get_real_schema_version(db_path).map_err(|e| {
                RestoreError::DbRestoreFailed(format!("Failed to read current schema: {}", e))
            })?
        } else {
            0 // Fresh install — no existing DB
        };

        let backup_schema = metadata.pane_version_schema;
        eprintln!("[RESTORE] Schema check: backup={}, current={}", backup_schema, current_schema);

        if backup_schema > current_schema {
            return Err(RestoreError::SchemaTooNew {
                backup: backup_schema,
                current: current_schema,
            });
        } else if backup_schema < current_schema && !skip_schema_confirmation {
            return Err(RestoreError::SchemaOlderConfirmationRequired {
                backup: backup_schema,
                current: current_schema,
            });
        }
    }

    // A3: Verify backup integrity before touching the live DB
    verify_backup_integrity(backup_path).map_err(|e| RestoreError::DbRestoreFailed(e))?;

    // ─ Phase B: Quiescent swap (ADR-0056 R1 + R2) ───────────────────────────────
    //
    // We acquire the Mutex here and hold it until reopen is complete (step B5).
    // Any concurrent command will block on lock() and be served the new connection
    // once we release. This is intentional — see function-level doc comment.

    // B1 — R2: Close the live connection before any file operation.
    //
    // `guard.0.take()` moves the Connection out of the Option, dropping it
    // immediately. SQLite closes the file handle on drop. guard remains held
    // so no other thread can acquire the lock during the swap.
    let mut guard = db.0.lock().map_err(|e| {
        RestoreError::DbRestoreFailed(format!("Failed to acquire DbState lock: {}", e))
    })?;
    let _old_conn = guard.0.take(); // drops Connection → SQLite closes file handle
    eprintln!("[RESTORE] Live connection closed (ADR-0056 R2)");

    // B2 — R3: Safety backup via create_backup() AFTER live connection is closed.
    //
    // create_backup() opens its own short-lived connection, checkpoints the WAL,
    // and copies via the SQLite backup API — producing a clean, WAL-free snapshot.
    // This is correct because there is no competing live connection at this point.
    // We do NOT raw-copy a live WAL-open DB (the old broken behavior at backup.rs:765-773).
    if db_path.exists() {
        let safety_name = format!("pane_pre-restore-{}.db", Local::now().format("%Y%m%d_%H%M%S"));
        eprintln!("[RESTORE] Creating safety backup: {}", safety_name);
        // FullSpeed (#2723): the live DbState connection was just closed above
        // (ADR-0056 R2) — by construction there is no concurrent reader/writer
        // of db_path at this point, so there is nothing to be polite to.
        match create_backup(db_path, BackupType::PreRestore, None, None, BackupPacing::FullSpeed) {
            Ok(p) => eprintln!("[RESTORE] Safety backup created: {:?}", p.file_name()),
            Err(e) => eprintln!("[RESTORE] Safety backup failed (non-fatal, continuing): {}", e),
        }
    }

    // B3: Stage the restored DB to a temp path, then atomic-rename into place.
    //
    // staging_path is on the same filesystem (∼/.pane/) so rename is a single
    // syscall on APFS — crash before rename leaves the live DB untouched;
    // crash after leaves a fully-restored DB.
    let staging_path = db_path.with_extension("db-restoring");
    // Clean up any stale staging file from a prior crashed restore.
    if staging_path.exists() {
        let _ = fs::remove_file(&staging_path);
        eprintln!("[RESTORE] Removed stale staging file");
    }

    fs::copy(backup_path, &staging_path).map_err(|e| {
        RestoreError::DbRestoreFailed(format!("Failed to stage backup: {}", e))
    })?;
    eprintln!("[RESTORE] Staged backup to: {:?}", staging_path.file_name());

    // B4 — R1: Atomic rename + clear WAL/SHM unconditionally.
    //
    // The rename is atomic on APFS (same filesystem). WAL/SHM clearing is
    // unconditional: a present WAL after a file-level swap causes SQLite to
    // replay pre-restore frames even if the backup is perfect (salt match).
    // ADR-0056 §Rationale; #1922 root cause.
    fs::rename(&staging_path, db_path).map_err(|e| {
        RestoreError::DbRestoreFailed(format!("Failed to atomic-rename restored DB: {}", e))
    })?;
    eprintln!("[RESTORE] Atomic rename complete: backup is now the live DB");

    // RESTORE-ONLY: raw DB swap site. Only `restore_backup_internal` may perform
    // fs::rename on the live .db path. All other code paths must go through
    // `create_backup()`. (ADR-0056; INVARIANTS.md — Restore / Database Swap.)

    let wal_path = db_path.with_extension("db-wal");
    let shm_path = db_path.with_extension("db-shm");
    let wal_removed = fs::remove_file(&wal_path);
    let shm_removed = fs::remove_file(&shm_path);
    match wal_removed {
        Ok(_) => eprintln!("[RESTORE] Cleared WAL sidecar (R1)"),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(ref e) => eprintln!("[RESTORE] Warning: could not remove WAL: {}", e),
    }
    match shm_removed {
        Ok(_) => eprintln!("[RESTORE] Cleared SHM sidecar (R1)"),
        Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(ref e) => eprintln!("[RESTORE] Warning: could not remove SHM: {}", e),
    }

    // B5 — R2: Reopen a fresh connection and restore it to DbState.
    //
    // Must reload the crsqlite extension (CRDT operations fail silently without it).
    // `get_crsqlite_path()` returns the path cached at startup by lib.rs.
    let crsqlite_path = crate::database::get_crsqlite_path();
    let new_conn = crate::database::open_connection(crsqlite_path).map_err(|e| {
        RestoreError::DbRestoreFailed(format!("Failed to reopen connection after restore: {}", e))
    })?;
    guard.0 = Some(new_conn);
    drop(guard); // Release Mutex — all blocked commands now proceed with the restored connection
    eprintln!("[RESTORE] Fresh connection reopened with crsqlite (ADR-0056 R2)");

    // ─ Phase C: Non-DB restore steps (install UUID, system files, keychain) ───

    eprintln!("[RESTORE] Database file swap complete. Restoring sidecars…");
    
    // STEP 4: Restore install UUID — FATAL on failure
    let install_uuid_backup = backup_filename.replace(".db", "_install-uuid.json");
    let install_uuid_path = get_backups_dir().join(&install_uuid_backup);
    
    if install_uuid_path.exists() {
        let contents = fs::read_to_string(&install_uuid_path)
            .map_err(|e| RestoreError::InstallUuidRestoreFailed(
                format!("Failed to read install UUID backup: {}", e)
            ))?;
        
        let uuid_data = serde_json::from_str::<crate::install_uuid::InstallUuid>(&contents)
            .map_err(|e| RestoreError::InstallUuidRestoreFailed(
                format!("Failed to parse install UUID: {}", e)
            ))?;
        
        crate::install_uuid::write_install_uuid(&uuid_data)
            .map_err(|e| RestoreError::InstallUuidRestoreFailed(e))?;
        
        eprintln!("[RESTORE] Restored install UUID: {}", uuid_data.install_uuid);
    } else {
        return Err(RestoreError::InstallUuidRestoreFailed(
            "Install UUID backup file not found".to_string()
        ));
    }
    
    // STEP 5: Restore .pane/system/ — SOFT on failure (re-syncs from gateway)
    let tar_name = backup_filename.replace(".db", "_system.tar.gz");
    let tar_path = get_backups_dir().join(&tar_name);
    
    if tar_path.exists() {
        if let Err(e) = restore_flat_files(&tar_path) {
            let warning = RestoreWarning {
                class: "SystemFilesRestoreFailed".to_string(),
                message: format!("Failed to restore system files: {}. Files will re-sync from gateway.", e),
            };
            eprintln!("[RESTORE] Warning: {}", warning.message);
            warnings.push(warning);
        } else {
            eprintln!("[RESTORE] System files restored successfully");
        }
    } else {
        eprintln!("[RESTORE] No system files archive found (older backup format)");
    }
    
    // STEP 5b: Restore attachment files from legacy archive — SOFT on failure
    // New backups (post-#1915) never produce _assets.tar.gz, so this path is a no-op for
    // them. For older backups that do carry the archive (pre-#1915), we still restore it
    // gracefully. restore_attachment_files() is deprecated but intentionally kept for this.
    let assets_tar_name = backup_filename.replace(".db", "_assets.tar.gz");
    let assets_tar_path = get_backups_dir().join(&assets_tar_name);

    #[allow(deprecated)]
    if assets_tar_path.exists() {
        if let Err(e) = restore_attachment_files(&assets_tar_path) {
            let warning = RestoreWarning {
                class: "AttachmentFilesRestoreFailed".to_string(),
                message: format!("Failed to restore attachment files: {}. Files may need to be re-synced.", e),
            };
            eprintln!("[RESTORE] Warning: {}", warning.message);
            warnings.push(warning);
        } else {
            eprintln!("[RESTORE] Attachment files restored successfully");
        }
    } else {
        eprintln!("[RESTORE] No attachment archive found (older backup format — skipping)");
    }

    // STEP 6: Restore keychain — SOFT on failure (recoverable via Settings)
    let keychain_backup = backup_filename.replace(".db", "_keychain.json");
    let keychain_path = get_backups_dir().join(&keychain_backup);
    
    if keychain_path.exists() {
        match fs::read_to_string(&keychain_path) {
            Ok(contents) => {
                match serde_json::from_str::<HashMap<String, String>>(&contents) {
                    Ok(keychain_items) => {
                        match crate::keychain::import_keychain_items(&keychain_items) {
                            Ok(_) => {
                                eprintln!("[RESTORE] Restored {} keychain items", keychain_items.len());
                            }
                            Err(e) => {
                                let warning = RestoreWarning {
                                    class: if e.contains("partial") { 
                                        "KeychainRestorePartial".to_string() 
                                    } else { 
                                        "KeychainRestoreFailed".to_string() 
                                    },
                                    message: format!("Keychain restore issue: {}. Re-enter credentials in Settings.", e),
                                };
                                eprintln!("[RESTORE] Warning: {}", warning.message);
                                warnings.push(warning);
                            }
                        }
                    }
                    Err(e) => {
                        let warning = RestoreWarning {
                            class: "KeychainRestoreFailed".to_string(),
                            message: format!("Failed to parse keychain backup: {}. Re-enter credentials in Settings.", e),
                        };
                        eprintln!("[RESTORE] Warning: {}", warning.message);
                        warnings.push(warning);
                    }
                }
            }
            Err(e) => {
                let warning = RestoreWarning {
                    class: "KeychainRestoreFailed".to_string(),
                    message: format!("Failed to read keychain backup: {}. Re-enter credentials in Settings.", e),
                };
                eprintln!("[RESTORE] Warning: {}", warning.message);
                warnings.push(warning);
            }
        }
    } else {
        eprintln!("[RESTORE] No keychain backup found (older backup format)");
    }
    
    // STEP 7: Return success summary with warnings
    eprintln!("[RESTORE] Restore completed with {} warning(s)", warnings.len());
    eprintln!("[RESTORE] App restart required for changes to take effect");
    
    Ok(RestoreSummary {
        warnings,
        backup_metadata,
    })
}

/// Archive attachment files from both storage locations:
/// 1. Projects/*/Assets/  — per-project attachments
/// 2. .pane/attachments/  — home-chat / global attachments
///
/// The archive is relative to pane_root so restore can unpack to the same base.
///
/// # Deprecated
///
/// As of #1915, no backup type produces `_assets.tar.gz`. This function is no longer called
/// by `create_backup()` or any production path. It is retained here for the follow-up cleanup
/// PR; do not add new call sites. See also `restore_attachment_files()` (similarly deprecated).
#[deprecated(
    note = "Backups never archive media as of #1915. Function retained for follow-up cleanup only."
)]
#[allow(dead_code)]
fn backup_attachment_files(db_filename: &str, _timestamp: &str) -> Result<(), String> {
    let pane_root = crate::database::get_pane_root();
    let projects_root = crate::database::get_projects_path();
    let global_attachments = pane_root.join(".pane").join("attachments");

    // Collect paths to include in the archive (relative to pane_root)
    let mut paths_to_archive: Vec<String> = Vec::new();

    // 1. Walk each project subdirectory looking for Assets/ folders
    if projects_root.exists() {
        match fs::read_dir(&projects_root) {
            Ok(entries) => {
                for entry in entries.filter_map(|e| e.ok()) {
                    let assets_dir = entry.path().join("Assets");
                    if assets_dir.exists() && assets_dir.is_dir() {
                        // Convert to a path relative to pane_root for archive
                        match assets_dir.strip_prefix(&pane_root) {
                            Ok(relative) => {
                                paths_to_archive.push(relative.to_string_lossy().into_owned());
                            }
                            Err(e) => {
                                eprintln!("[BACKUP] Failed to relativize Assets path {:?}: {}", assets_dir, e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("[BACKUP] Failed to read projects directory: {}", e);
            }
        }
    }

    // 2. Home-chat / global attachments (.pane/attachments/)
    if global_attachments.exists() && global_attachments.is_dir() {
        match global_attachments.strip_prefix(&pane_root) {
            Ok(relative) => {
                paths_to_archive.push(relative.to_string_lossy().into_owned());
            }
            Err(e) => {
                eprintln!("[BACKUP] Failed to relativize global attachments path: {}", e);
            }
        }
    }

    if paths_to_archive.is_empty() {
        eprintln!("[BACKUP] No attachment directories found — skipping assets archive");
        return Ok(());
    }

    // Ensure the backups directory exists before writing the archive
    let backup_dir = ensure_backups_dir()
        .map_err(|e| format!("Failed to create backups dir for assets: {}", e))?;

    let assets_filename = format!("{}_assets.tar.gz", db_filename.trim_end_matches(".db"));
    let assets_path = backup_dir.join(&assets_filename);

    // Build tar command: archive relative to pane_root so relative paths are preserved
    let mut cmd = std::process::Command::new("tar");
    cmd.arg("-czf")
        .arg(&assets_path)
        .arg("-C")
        .arg(&pane_root);
    for path in &paths_to_archive {
        cmd.arg(path);
    }

    let output = cmd.output()
        .map_err(|e| format!("Failed to execute tar for assets: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tar assets archive failed: {}", stderr));
    }

    let archive_size = fs::metadata(&assets_path)
        .map(|m| m.len())
        .unwrap_or(0);

    eprintln!(
        "[BACKUP] Attachment files archived: {} ({} bytes, {} director{})",
        assets_filename,
        archive_size,
        paths_to_archive.len(),
        if paths_to_archive.len() == 1 { "y" } else { "ies" }
    );

    Ok(())
}

/// Restore attachment files from _assets.tar.gz back to pane_root
///
/// # Deprecated
///
/// As of #1915, `create_backup()` no longer produces `_assets.tar.gz` for any backup type.
/// This function is still reachable from the restore path for legacy backups that carry the
/// archive (e.g., backups created before #1915). It is retained until the follow-up cleanup
/// PR removes it. Do not add new call sites.
#[deprecated(
    note = "Assets archive is no longer produced as of #1915. This function handles legacy archives only and will be removed in a follow-up PR."
)]
#[allow(dead_code)]
fn restore_attachment_files(tar_path: &Path) -> Result<(), String> {
    let pane_root = crate::database::get_pane_root();

    // Extract relative to pane_root so Projects/*/Assets/ and .pane/attachments/ land correctly
    let output = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(tar_path)
        .arg("-C")
        .arg(&pane_root)
        .output()
        .map_err(|e| format!("Failed to execute tar for asset restore: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tar asset extraction failed: {}", stderr));
    }

    eprintln!("[RESTORE] Attachment files restored from: {:?}", tar_path.file_name());
    Ok(())
}

/// Restore flat files from tar.gz
fn restore_flat_files(tar_path: &Path) -> Result<(), String> {
    let pane_dir = crate::database::get_pane_root().join(".pane");
    
    // Extract tar to .pane directory (overwrites existing system/)
    let output = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(tar_path)
        .arg("-C")
        .arg(&pane_dir)
        .output()
        .map_err(|e| format!("Failed to execute tar: {}", e))?;
    
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tar extraction failed: {}", stderr));
    }
    
    eprintln!("[BACKUP] Flat files restored from: {:?}", tar_path.file_name());
    Ok(())
}

/// Shared test helper for setting up an isolated `PANE_TEST_ROOT` in tests.
///
/// Only manages the `PANE_TEST_ROOT` env var (set on construction, RESTORED
/// via `Drop` — including on panic). Deliberately does NOT own the caller's
/// `TempDir` — callers keep managing their own temp directories exactly as
/// before; this guard just guarantees the env var is always cleaned up.
///
/// #3057 follow-up: `drop` used to unconditionally `remove_var`, which is
/// safe for THIS test but actively harmful to whichever test runs next in
/// the same process if it never sets its own `PANE_TEST_ROOT` and (like
/// ~225 pre-existing tests crate-wide, discovered when #3057's
/// `get_pane_root()` guard first landed) silently relied on inheriting an
/// ambient value. Restoring the PRIOR value (including "restore to
/// nothing" when there genuinely was none) instead of blindly clearing
/// means this guard can never be the thing that turns "ambient value
/// present" into "absent" out from under a later, differently-isolated
/// test.
#[cfg(test)]
pub(crate) mod test_support {
    pub struct TestRootGuard {
        prev: Option<String>,
    }

    impl TestRootGuard {
        /// Set `PANE_TEST_ROOT` to `path`, capturing whatever was there
        /// before so `Drop` can restore it exactly, rather than clearing
        /// unconditionally.
        pub fn set(path: impl AsRef<std::path::Path>) -> Self {
            let prev = std::env::var("PANE_TEST_ROOT").ok();
            std::env::set_var("PANE_TEST_ROOT", path.as_ref());
            TestRootGuard { prev }
        }
    }

    impl Drop for TestRootGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("PANE_TEST_ROOT", v),
                None => std::env::remove_var("PANE_TEST_ROOT"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_support::TestRootGuard;
    use crate::keychain::test_support::KeychainGuard;
    use std::fs;
    use tempfile::TempDir;
    
    fn create_test_db(path: &Path) -> Connection {
        let conn = Connection::open(path).unwrap();
        // Disable WAL mode and use synchronous=FULL for immediate, reliable writes
        conn.execute_batch("PRAGMA journal_mode=DELETE;").unwrap();
        conn.execute_batch("PRAGMA synchronous=FULL;").unwrap();
        conn.execute_batch(
            "CREATE TABLE test (id INTEGER PRIMARY KEY, value TEXT);
             INSERT INTO test (value) VALUES ('test data');"
        ).unwrap();
        conn
    }

    /// Create a test DB that also has `ai_settings` with the given schema version.
    /// Required for schema-guard tests (R4): `get_real_schema_version` reads from
    /// `ai_settings`, not `PRAGMA user_version` (which is always 0 under cr-sqlite).
    fn create_test_db_with_schema(path: &Path, schema_version: i64) -> Connection {
        let conn = create_test_db(path);
        conn.execute_batch(
            "CREATE TABLE ai_settings (key TEXT PRIMARY KEY, value TEXT);"
        ).unwrap();
        conn.execute(
            "INSERT INTO ai_settings (key, value) VALUES ('schema_version', ?1)",
            rusqlite::params![schema_version.to_string()],
        ).unwrap();
        conn
    }

    /// Helper: read schema_version from ai_settings, or 0 if table/key absent.
    fn read_test_schema_version(db_path: &Path) -> i64 {
        let conn = Connection::open(db_path).unwrap();
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_settings'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if !table_exists { return 0; }
        conn.query_row(
            "SELECT value FROM ai_settings WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0)
    }

    /// Create a dummy `DbState` suitable for passing to restore functions in tests.
    /// The connection slot starts as `None`; restore will open a fresh connection
    /// (without crsqlite, which is acceptable in test environments) and put it back.
    fn make_test_db_state() -> crate::database::DbState {
        crate::database::DbState::new_empty_for_test()
    }
    
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_backup_creates_valid_db() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            // Connection uses journal_mode=DELETE, so writes are immediate
            drop(conn);
        }
        
        // Verify data is readable before creating backup
        {
            let verify_conn = Connection::open(&db_path).unwrap();
            let count: i64 = verify_conn
                .query_row("SELECT COUNT(*) FROM test", [], |row| row.get(0))
                .unwrap();
            assert_eq!(count, 1, "Test data not found in source database before backup");
        }
        
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();
        
        assert!(backup_path.exists());
        
        // Verify backup can be opened and queried
        let backup_conn = Connection::open(&backup_path).unwrap();
        let count: i64 = backup_conn
            .query_row("SELECT COUNT(*) FROM test", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
    
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_backup_integrity_check() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        
        // Delay to ensure filesystem buffers are flushed and file is fully closed
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();
        
        // Should pass integrity check
        assert!(verify_backup_integrity(&backup_path).is_ok());
    }

    /// #3072: `verify_backup_integrity` must now also fail when `notes_fts`
    /// (an external-content FTS5 table) has drifted out of sync with
    /// `notes` — the exact corruption class `PRAGMA integrity_check` alone
    /// (the pre-existing whole-DB check, still run first, still passing
    /// here) does not catch. Drift is injected directly against the shadow
    /// index (test-harness-only fault injection, not a raw-INSERT substitute
    /// for a user action) rather than via `create_backup`'s full pipeline,
    /// to keep this test fast and focused on `verify_backup_integrity`
    /// itself, which is the only function this PR changes in `backup.rs`.
    #[test]
    fn test_backup_integrity_check_catches_notes_fts_drift() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("staged.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "
            CREATE TABLE notes (id TEXT PRIMARY KEY, title TEXT, body TEXT);
            CREATE VIRTUAL TABLE notes_fts USING fts5(
                title, body, content='notes', content_rowid='rowid', tokenize='porter unicode61'
            );
            CREATE TRIGGER notes_fts_insert AFTER INSERT ON notes BEGIN
                INSERT INTO notes_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
            END;
            INSERT INTO notes (rowid, id, title, body) VALUES (1, 'a', 'Alpha', 'alpha needle content');
            INSERT INTO notes (rowid, id, title, body) VALUES (2, 'b', 'Beta', 'beta needle content');
            ",
        )
        .unwrap();

        // Sanity: whole-DB integrity_check alone (the pre-#3072 behavior)
        // passes on a perfectly valid SQLite file — the drift below is
        // FTS5-external-content-specific, not file/page-level corruption.
        let whole_db_ok: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0)).unwrap();
        assert_eq!(whole_db_ok, "ok");

        // Simulate drift: remove rowid 1's postings from the shadow index
        // directly, leaving `notes` itself untouched — exactly the kind of
        // drift an external-content table can suffer (and the kind the old
        // unconditional startup rebuild's non-transactional DELETE+loop
        // window produced, #2352/#2566/#2610).
        conn.execute("DELETE FROM notes_fts WHERE rowid = 1", []).unwrap();
        drop(conn);

        let result = verify_backup_integrity(&db_path);
        assert!(
            result.is_err(),
            "verify_backup_integrity must fail when notes_fts has drifted from notes, even though PRAGMA integrity_check alone passes"
        );
    }

    #[serial_test::serial(pane_test_root)]
    #[test]
    fn test_backup_list_returns_all() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        // This test would need to set up the actual backups directory
        // For now, just verify the function doesn't crash
        let result = list_backups();
        assert!(result.is_ok());
    }
    
    #[serial_test::serial(pane_test_root)]
    #[test]
    fn test_parse_backup_timestamp() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        let dt = parse_backup_timestamp("pane_20260404_113000.db");
        assert!(dt.is_some());
        
        let dt = dt.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 4);
        assert_eq!(dt.day(), 4);
    }
    
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_backup_includes_metadata() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        // Set up test environment for install UUID
        fs::create_dir_all(temp.path().join("Pane")).unwrap();
        let _test_root_guard = TestRootGuard::set(temp.path().join("Pane"));
        
        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();
        let backup_filename = backup_path.file_name().unwrap().to_str().unwrap();
        
        // Check metadata file exists
        let metadata_filename = backup_filename.replace(".db", "_metadata.json");
        let metadata_path = get_backups_dir().join(&metadata_filename);
        assert!(metadata_path.exists(), "Metadata file should exist");
        
        // Verify metadata contents
        let contents = fs::read_to_string(&metadata_path).unwrap();
        let metadata: BackupMetadata = serde_json::from_str(&contents).unwrap();
        
        assert!(!metadata.app_version.is_empty());
        assert!(!metadata.backup_timestamp.is_empty());
        assert_eq!(metadata.backup_type, "manual");
        assert!(!metadata.install_uuid.is_empty());
        assert!(metadata.pane_version_schema >= 0);
    }
    
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_backup_includes_install_uuid() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        // Set up test environment for install UUID
        fs::create_dir_all(temp.path().join("Pane")).unwrap();
        let _test_root_guard = TestRootGuard::set(temp.path().join("Pane"));
        
        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        // Ensure install UUID directory and file exist before backup
        let uuid_path = crate::install_uuid::get_install_uuid_path();
        let uuid_dir = uuid_path.parent().unwrap();
        fs::create_dir_all(uuid_dir).unwrap();
        let uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();
        
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();
        let backup_filename = backup_path.file_name().unwrap().to_str().unwrap();
        
        // Check install-uuid.json copy exists
        let install_uuid_filename = backup_filename.replace(".db", "_install-uuid.json");
        let install_uuid_path = get_backups_dir().join(&install_uuid_filename);
        assert!(install_uuid_path.exists(), "Install UUID backup should exist");
        
        // Verify contents
        let contents = fs::read_to_string(&install_uuid_path).unwrap();
        let uuid_data: crate::install_uuid::InstallUuid = serde_json::from_str(&contents).unwrap();
        assert_eq!(uuid_data.install_uuid, uuid);
    }
    
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_restore_restores_install_uuid() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        // Set up test environment for install UUID
        fs::create_dir_all(temp.path().join("Pane")).unwrap();
        let _test_root_guard = TestRootGuard::set(temp.path().join("Pane"));
        
        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        // Create a backup with install UUID
        let original_uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();
        
        // Change the install UUID
        let new_uuid_data = crate::install_uuid::InstallUuid {
            install_uuid: "changed-uuid".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        crate::install_uuid::write_install_uuid(&new_uuid_data).unwrap();
        
        // Verify it changed
        let changed_uuid = crate::install_uuid::read_install_uuid().unwrap().unwrap().install_uuid;
        assert_eq!(changed_uuid, "changed-uuid");
        
        // Restore backup (ADR-0056: pass DbState for connection lifecycle management)
        let dummy_db = make_test_db_state();
        let summary = restore_backup(&backup_path, &db_path, &dummy_db).unwrap();
        
        // Verify no fatal errors
        assert!(summary.backup_metadata.is_some());
        
        // Verify install UUID was restored to original
        let restored_uuid = crate::install_uuid::read_install_uuid().unwrap().unwrap().install_uuid;
        assert_eq!(restored_uuid, original_uuid);
    }
    
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_restore_schema_too_new() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("Pane")).unwrap();
        let _test_root_guard = TestRootGuard::set(temp.path().join("Pane"));

        let db_path = temp.path().join("test.db");
        {
            // R4: write schema_version to ai_settings (not PRAGMA user_version which is always 0)
            let conn = create_test_db_with_schema(&db_path, 1);
            drop(conn);
        }

        std::thread::sleep(std::time::Duration::from_millis(500));

        // Create a backup — metadata.pane_version_schema will be 1 (from ai_settings)
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();

        // Manually edit the metadata to claim schema version 99 (newer than current=1)
        let backup_filename = backup_path.file_name().unwrap().to_str().unwrap();
        let metadata_filename = backup_filename.replace(".db", "_metadata.json");
        let metadata_path = get_backups_dir().join(&metadata_filename);

        let mut metadata: BackupMetadata =
            serde_json::from_str(&fs::read_to_string(&metadata_path).unwrap()).unwrap();
        metadata.pane_version_schema = 99;
        fs::write(&metadata_path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();

        // Attempt restore - should fail with SchemaTooNew (backup=99 > current=1)
        let dummy_db = make_test_db_state();
        let result = restore_backup(&backup_path, &db_path, &dummy_db);

        assert!(result.is_err());
        match result.unwrap_err() {
            RestoreError::SchemaTooNew { backup, current } => {
                assert_eq!(backup, 99);
                assert_eq!(current, 1); // ai_settings schema_version='1'
            }
            other => panic!("Expected SchemaTooNew, got: {:?}", other),
        }
    }
    
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_restore_uuid_fatal() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("Pane")).unwrap();
        let _test_root_guard = TestRootGuard::set(temp.path().join("Pane"));
        
        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        
        std::thread::sleep(std::time::Duration::from_millis(500));
        
        // Create a backup
        let _original_uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();
        
        // Delete the install UUID backup file to simulate missing UUID
        let backup_filename = backup_path.file_name().unwrap().to_str().unwrap();
        let install_uuid_backup = backup_filename.replace(".db", "_install-uuid.json");
        let install_uuid_path = get_backups_dir().join(&install_uuid_backup);
        fs::remove_file(&install_uuid_path).unwrap();
        
        // Attempt restore - should fail with InstallUuidRestoreFailed
        let dummy_db = make_test_db_state();
        let result = restore_backup(&backup_path, &db_path, &dummy_db);

        assert!(result.is_err());
        match result.unwrap_err() {
            RestoreError::InstallUuidRestoreFailed(msg) => {
                assert!(msg.contains("not found") || msg.contains("Failed to read"));
            }
            other => panic!("Expected InstallUuidRestoreFailed, got: {:?}", other),
        }
    }
    
    #[serial_test::serial(pane_test_root)]
    #[test]
    fn test_metadata_format() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        let metadata = BackupMetadata {
            app_version: "0.18.0".to_string(),
            backup_timestamp: "20260507_140000".to_string(),
            backup_type: "manual".to_string(),
            account_email: Some("test@example.com".to_string()),
            install_uuid: "test-uuid-123".to_string(),
            pane_version_schema: 42,
        };
        
        // Verify serialization
        let json = serde_json::to_string_pretty(&metadata).unwrap();
        assert!(json.contains("app_version"));
        assert!(json.contains("backup_timestamp"));
        assert!(json.contains("backup_type"));
        assert!(json.contains("account_email"));
        assert!(json.contains("install_uuid"));
        assert!(json.contains("pane_version_schema"));
        
        // Verify deserialization
        let parsed: BackupMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_version, metadata.app_version);
        assert_eq!(parsed.install_uuid, metadata.install_uuid);
        assert_eq!(parsed.pane_version_schema, 42);
    }
    
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_restore_schema_older_confirmed_round_trip() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("Pane")).unwrap();
        let _test_root_guard = TestRootGuard::set(temp.path().join("Pane"));
        
        let db_path = temp.path().join("test.db");
        
        // STEP 1: Create a test DB with schema version 5 and insert test data.
        // R4: write schema_version to ai_settings (not PRAGMA user_version).
        {
            let conn = create_test_db_with_schema(&db_path, 5);

            // Insert a note to verify data integrity after restore
            conn.execute(
                "CREATE TABLE IF NOT EXISTS notes (id INTEGER PRIMARY KEY, content TEXT);",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO notes (content) VALUES ('test note from schema 5');",
                [],
            ).unwrap();
            drop(conn);
        }

        std::thread::sleep(std::time::Duration::from_millis(500));

        // STEP 2: Create a backup — metadata.pane_version_schema = 5 (from ai_settings)
        let _original_uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();

        // STEP 3: Modify LIVE DB to simulate schema version 10 (newer than backup).
        // R4: update ai_settings.schema_version to '10'.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('schema_version', '10')",
                [],
            ).unwrap();
            // Add a column to simulate schema evolution
            conn.execute("ALTER TABLE notes ADD COLUMN updated_at TEXT;", []).unwrap();
            drop(conn);
        }

        std::thread::sleep(std::time::Duration::from_millis(500));

        // STEP 4: Call restore_backup() without confirmation → should require confirmation
        let dummy_db = make_test_db_state();
        let result = restore_backup(&backup_path, &db_path, &dummy_db);

        assert!(result.is_err(), "restore_backup should require confirmation for older schema");
        match result.unwrap_err() {
            RestoreError::SchemaOlderConfirmationRequired { backup, current } => {
                assert_eq!(backup, 5, "Backup schema should be 5");
                assert_eq!(current, 10, "Current schema should be 10");
            }
            other => panic!("Expected SchemaOlderConfirmationRequired, got: {:?}", other),
        }

        // STEP 5: Verify NO state mutation happened (DB should still be schema 10).
        // R4: read schema_version from ai_settings, not PRAGMA user_version.
        {
            let schema_after_failed = read_test_schema_version(&db_path);
            assert_eq!(schema_after_failed, 10, "Schema should still be 10 after failed restore");

            // Verify the updated_at column still exists (schema-10 state untouched)
            let conn = Connection::open(&db_path).unwrap();
            let has_updated_at: Result<String, _> = conn.query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='notes'",
                [],
                |row| row.get(0),
            );
            assert!(has_updated_at.is_ok(), "Notes table should still exist");
            let schema_sql = has_updated_at.unwrap();
            assert!(schema_sql.contains("updated_at"), "updated_at column should still exist");
        }

        // STEP 6: Call restore_backup_confirmed() → should succeed.
        let dummy_db2 = make_test_db_state();
        let summary = restore_backup_confirmed(&backup_path, &db_path, &dummy_db2).unwrap();

        assert!(summary.backup_metadata.is_some(), "Backup metadata should be present");
        let metadata = summary.backup_metadata.unwrap();
        assert_eq!(metadata.pane_version_schema, 5, "Restored metadata should show schema 5");

        // STEP 7: Verify data is intact post-restore.
        // R4: read schema_version from ai_settings.
        {
            let schema_after_restore = read_test_schema_version(&db_path);
            assert_eq!(schema_after_restore, 5, "Schema should be restored to 5");

            let conn = Connection::open(&db_path).unwrap();

            // Verify the note from step 1 is present
            let note_content: String = conn
                .query_row("SELECT content FROM notes WHERE id = 1", [], |row| row.get(0))
                .unwrap();
            assert_eq!(note_content, "test note from schema 5", "Original data should be intact");

            // Verify updated_at column no longer exists (reverted to schema 5)
            let schema_sql: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name='notes'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!schema_sql.contains("updated_at"), "updated_at column should be gone after restore");
        }
    }
    
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_restore_confirmed_still_blocks_schema_too_new() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        fs::create_dir_all(temp.path().join("Pane")).unwrap();
        let _test_root_guard = TestRootGuard::set(temp.path().join("Pane"));
        
        let db_path = temp.path().join("test.db");
        
        // R4: Create test DB with schema_version='5' in ai_settings
        {
            let conn = create_test_db_with_schema(&db_path, 5);
            drop(conn);
        }

        std::thread::sleep(std::time::Duration::from_millis(500));

        // Create a backup — metadata.pane_version_schema = 5
        let _original_uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();

        // Manually edit the metadata to claim schema version 99 (HIGHER than current 5)
        let backup_filename = backup_path.file_name().unwrap().to_str().unwrap();
        let metadata_filename = backup_filename.replace(".db", "_metadata.json");
        let metadata_path = get_backups_dir().join(&metadata_filename);

        let mut metadata: BackupMetadata =
            serde_json::from_str(&fs::read_to_string(&metadata_path).unwrap()).unwrap();
        metadata.pane_version_schema = 99;
        fs::write(&metadata_path, serde_json::to_string_pretty(&metadata).unwrap()).unwrap();

        // Attempt restore_backup_confirmed — should still fail with SchemaTooNew
        // Confirmation only bypasses older-schema check, NOT newer-schema check
        let dummy_db = make_test_db_state();
        let result = restore_backup_confirmed(&backup_path, &db_path, &dummy_db);

        assert!(result.is_err(), "restore_backup_confirmed should still block SchemaTooNew");
        match result.unwrap_err() {
            RestoreError::SchemaTooNew { backup, current } => {
                assert_eq!(backup, 99, "Backup schema should be 99");
                assert_eq!(current, 5, "Current schema should be 5"); // from ai_settings
            }
            other => panic!("Expected SchemaTooNew, got: {:?}", other),
        }
    }

    /// Regression guard for #1007: rotation must clean up ALL sidecar files,
    /// not just the .db. Orphaned _keychain.json files held exported credentials
    /// past their retention window; this test fails if any sidecar type is missed.
    ///
    /// spec-skip: internal backup behavior — no spec contract exists for rotation cleanup.
    #[serial_test::serial(pane_test_root)]
    #[test]
    fn test_rotation_cleans_all_sidecars() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        let backup_dir = get_backups_dir();
        fs::create_dir_all(&backup_dir).unwrap();

        // 9 daily backup stubs — weekday dates only so they all classify as daily
        // (Sunday 2026-06-07 is intentionally skipped to avoid weekly classification)
        let dates = [
            "20260601_100000", // Mon
            "20260602_100000", // Tue
            "20260603_100000", // Wed
            "20260604_100000", // Thu
            "20260605_100000", // Fri
            "20260606_100000", // Sat
            "20260608_100000", // Mon
            "20260609_100000", // Tue
            "20260610_100000", // Wed
        ];

        let mut created_bases: Vec<String> = Vec::new();

        for (i, date) in dates.iter().enumerate() {
            let db_name = format!("pane_{}.db", date);
            let db_path = backup_dir.join(&db_name);

            // Stub content — rotate_backups() reads DirEntry only, never opens the DB
            fs::write(&db_path, b"stub").unwrap();

            // Assign distinct mtimes so sort_by_key produces deterministic oldest-first order
            let mtime = std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(1_700_000_000 + i as u64 * 3600);
            let times = std::fs::FileTimes::new().set_modified(mtime);
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(&db_path)
                .unwrap();
            file.set_times(times).unwrap();
            drop(file);

            // Create all four sidecars for this backup
            let base = db_name.trim_end_matches(".db").to_string();
            for suffix in SIDECAR_SUFFIXES {
                let sidecar_path = backup_dir.join(format!("{}{}", base, suffix));
                fs::write(&sidecar_path, b"stub sidecar").unwrap();
            }

            created_bases.push(base);
        }

        // Rotate — 9 daily > DAILY_RETENTION_COUNT (7), so 2 oldest should be removed
        rotate_backups().unwrap();

        // Classify which backups remain vs. were deleted
        let mut retained: Vec<String> = Vec::new();
        let mut deleted: Vec<String> = Vec::new();
        for base in &created_bases {
            if backup_dir.join(format!("{}.db", base)).exists() {
                retained.push(base.clone());
            } else {
                deleted.push(base.clone());
            }
        }

        assert_eq!(
            retained.len(), 7,
            "Expected 7 daily backups retained after rotation, got {}",
            retained.len()
        );
        assert_eq!(
            deleted.len(), 2,
            "Expected 2 backups rotated out, got {}",
            deleted.len()
        );

        // Retained backups must have ALL sidecars intact (no collateral damage)
        for base in &retained {
            for suffix in SIDECAR_SUFFIXES {
                let sidecar_path = backup_dir.join(format!("{}{}", base, suffix));
                assert!(
                    sidecar_path.exists(),
                    "Retained backup '{}' is missing sidecar '{}' — deleted incorrectly",
                    base,
                    suffix
                );
            }
        }

        // Rotated-out backups must have ZERO orphaned sidecars (regression guard for #1007)
        for base in &deleted {
            for suffix in SIDECAR_SUFFIXES {
                let sidecar_path = backup_dir.join(format!("{}{}", base, suffix));
                assert!(
                    !sidecar_path.exists(),
                    "Orphaned sidecar for rotated backup '{}': '{}' still exists after rotation (#1007)",
                    base,
                    suffix
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Attachment backup/restore tests (PR4: #1775)
    // -----------------------------------------------------------------------

    /// Helper: create a small file at `path` with given content.
    fn write_file(path: &Path, content: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[serial_test::serial(pane_test_root)]
    #[test]
    #[allow(deprecated)] // testing the deprecated backup_attachment_files() function itself
    fn test_backup_attachment_files_creates_archive() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        // Set up a project with Assets/
        let assets_dir = pane_root.join("Projects").join("MyProject").join("Assets");
        fs::create_dir_all(&assets_dir).unwrap();
        write_file(&assets_dir.join("abc123.png"), b"fake png data");

        let db_filename = "pane_20260629_120000.db";
        let result = backup_attachment_files(db_filename, "20260629_120000");
        assert!(result.is_ok(), "backup_attachment_files should succeed: {:?}", result);

        let archive_path = get_backups_dir().join("pane_20260629_120000_assets.tar.gz");
        assert!(archive_path.exists(), "_assets.tar.gz sidecar should be created");

        // Verify the archive has content (> 0 bytes)
        let size = fs::metadata(&archive_path).unwrap().len();
        assert!(size > 0, "Archive should not be empty");
    }

    #[serial_test::serial(pane_test_root)]
    #[test]
    #[allow(deprecated)] // testing the deprecated backup_attachment_files() function itself
    fn test_backup_attachment_files_includes_global_attachments() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        // Set up only global attachments (no project assets)
        let global_dir = pane_root.join(".pane").join("attachments");
        fs::create_dir_all(&global_dir).unwrap();
        write_file(&global_dir.join("global").join("def456.jpg"), b"fake jpg data");

        let db_filename = "pane_20260629_130000.db";
        let result = backup_attachment_files(db_filename, "20260629_130000");
        assert!(result.is_ok(), "backup_attachment_files should succeed with global attachments: {:?}", result);

        let archive_path = get_backups_dir().join("pane_20260629_130000_assets.tar.gz");
        assert!(archive_path.exists(), "_assets.tar.gz should be created for global attachments");
    }

    #[serial_test::serial(pane_test_root)]
    #[test]
    #[allow(deprecated)] // testing the deprecated backup_attachment_files() function itself
    fn test_backup_attachment_files_no_attachments_is_ok() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        // No attachments at all — should succeed (skip gracefully)
        let db_filename = "pane_20260629_140000.db";
        let result = backup_attachment_files(db_filename, "20260629_140000");
        assert!(result.is_ok(), "backup_attachment_files should succeed with no attachments: {:?}", result);

        // No archive created when nothing to archive
        let archive_path = get_backups_dir().join("pane_20260629_140000_assets.tar.gz");
        assert!(!archive_path.exists(), "Archive should NOT be created when no attachments exist");
    }

    #[serial_test::serial(pane_test_root)]
    #[test]
    #[allow(deprecated)] // testing deprecated backup_attachment_files() / restore_attachment_files() directly
    fn test_restore_attachment_files_extracts_to_correct_location() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        // Create source attachment file
        let project_assets = pane_root.join("Projects").join("TestProj").join("Assets");
        fs::create_dir_all(&project_assets).unwrap();
        write_file(&project_assets.join("restore-test.png"), b"fake restore image");

        // Archive it
        let db_filename = "pane_20260629_150000.db";
        backup_attachment_files(db_filename, "20260629_150000").unwrap();

        // Delete the original
        fs::remove_file(&project_assets.join("restore-test.png")).unwrap();
        assert!(!project_assets.join("restore-test.png").exists());

        // Restore
        let archive_path = get_backups_dir().join("pane_20260629_150000_assets.tar.gz");
        let result = restore_attachment_files(&archive_path);
        assert!(result.is_ok(), "restore_attachment_files should succeed: {:?}", result);

        // Verify file is back
        assert!(
            project_assets.join("restore-test.png").exists(),
            "Restored attachment file should exist at original location"
        );
        let content = fs::read(&project_assets.join("restore-test.png")).unwrap();
        assert_eq!(content, b"fake restore image");
    }

    #[serial_test::serial(pane_test_root)]
    #[test]
    #[allow(deprecated)] // testing deprecated backup_attachment_files() / restore_attachment_files() directly
    fn test_attachment_backup_restore_round_trip() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        // --- Setup: create attachments in BOTH locations ---

        // Project Assets
        let project_assets = pane_root.join("Projects").join("ProjectA").join("Assets");
        fs::create_dir_all(&project_assets).unwrap();
        write_file(&project_assets.join("image1.png"), b"project image 1");
        write_file(&project_assets.join("doc1.pdf"), b"project pdf 1");

        // Second project
        let project_b_assets = pane_root.join("Projects").join("ProjectB").join("Assets");
        fs::create_dir_all(&project_b_assets).unwrap();
        write_file(&project_b_assets.join("image2.jpg"), b"project b image");

        // Global attachments
        let global_dir = pane_root.join(".pane").join("attachments");
        fs::create_dir_all(&global_dir).unwrap();
        write_file(&global_dir.join("global1.png"), b"global image 1");

        // --- Backup ---
        let db_filename = "pane_20260629_160000.db";
        backup_attachment_files(db_filename, "20260629_160000").unwrap();

        let archive_path = get_backups_dir().join("pane_20260629_160000_assets.tar.gz");
        assert!(archive_path.exists(), "Archive should be created");

        // --- Delete all attachment files ---
        fs::remove_file(&project_assets.join("image1.png")).unwrap();
        fs::remove_file(&project_assets.join("doc1.pdf")).unwrap();
        fs::remove_file(&project_b_assets.join("image2.jpg")).unwrap();
        fs::remove_file(&global_dir.join("global1.png")).unwrap();

        // Confirm deletion
        assert!(!project_assets.join("image1.png").exists());
        assert!(!project_assets.join("doc1.pdf").exists());
        assert!(!project_b_assets.join("image2.jpg").exists());
        assert!(!global_dir.join("global1.png").exists());

        // --- Restore ---
        restore_attachment_files(&archive_path).unwrap();

        // --- Verify all files are back with correct content ---
        let content = fs::read(&project_assets.join("image1.png")).unwrap();
        assert_eq!(content, b"project image 1", "image1.png content mismatch");

        let content = fs::read(&project_assets.join("doc1.pdf")).unwrap();
        assert_eq!(content, b"project pdf 1", "doc1.pdf content mismatch");

        let content = fs::read(&project_b_assets.join("image2.jpg")).unwrap();
        assert_eq!(content, b"project b image", "image2.jpg content mismatch");

        let content = fs::read(&global_dir.join("global1.png")).unwrap();
        assert_eq!(content, b"global image 1", "global1.png content mismatch");
    }

    /// Manual backup MUST NOT produce _assets.tar.gz (extended to all types by #1915).
    /// This test was previously named `test_create_backup_includes_assets_sidecar` and
    /// asserted the opposite. Inverted by #1915 — assets are now user-managed.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_manual_backup_skips_assets_archive() {
        // Use MockKeychain so this test does not race with keystore tests on the real macOS Keychain.
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        // Populate an Assets tree
        let project_assets = pane_root.join("Projects").join("TestProject").join("Assets");
        fs::create_dir_all(&project_assets).unwrap();
        write_file(&project_assets.join("test-file.png"), b"test attachment data");

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));

        let _ = crate::install_uuid::get_or_create_install_uuid().unwrap();
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();
        let stem = backup_path.file_stem().unwrap().to_str().unwrap();

        // _assets.tar.gz must NOT be produced (#1915 — assets are user-managed)
        let assets_path = get_backups_dir().join(format!("{}_assets.tar.gz", stem));
        assert!(
            !assets_path.exists(),
            "Manual backup must NOT produce _assets.tar.gz (fix #1915), but found: {:?}",
            assets_path
        );

        // DB copy must still exist
        assert!(backup_path.exists(), "DB backup file must exist");
        let db_size = fs::metadata(&backup_path).unwrap().len();
        assert!(db_size > 0, "DB backup must not be empty");

        // _system.tar.gz must be attempted (non-fatal; allow absent in test env)
        // _keychain.json must exist
        let keychain_path = get_backups_dir().join(format!("{}_keychain.json", stem));
        assert!(
            keychain_path.exists(),
            "Manual backup must produce _keychain.json (got absent: {:?})",
            keychain_path
        );

        // _metadata.json must exist
        let metadata_path = get_backups_dir().join(format!("{}_metadata.json", stem));
        assert!(
            metadata_path.exists(),
            "Manual backup must produce _metadata.json (got absent: {:?})",
            metadata_path
        );
        let meta_json = fs::read_to_string(&metadata_path).unwrap();
        let meta: BackupMetadata = serde_json::from_str(&meta_json).unwrap();
        assert_eq!(meta.backup_type, "manual", "Metadata backup_type should be 'manual'");
    }

    /// Daily backup MUST NOT produce _assets.tar.gz (#1915 — assets are user-managed).
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_daily_backup_skips_assets_archive() {
        // Use MockKeychain so this test does not race with keystore tests on the real macOS Keychain.
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        let project_assets = pane_root.join("Projects").join("DailyProject").join("Assets");
        fs::create_dir_all(&project_assets).unwrap();
        write_file(&project_assets.join("daily-file.png"), b"daily test attachment");

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));

        let _ = crate::install_uuid::get_or_create_install_uuid().unwrap();
        let backup_path = create_backup(&db_path, BackupType::Daily, None, None, BackupPacing::FullSpeed).unwrap();
        let stem = backup_path.file_stem().unwrap().to_str().unwrap();

        let assets_path = get_backups_dir().join(format!("{}_assets.tar.gz", stem));
        assert!(
            !assets_path.exists(),
            "Daily backup must NOT produce _assets.tar.gz (fix #1915), but found: {:?}",
            assets_path
        );
        assert!(backup_path.exists(), "DB backup file must exist");
        assert!(fs::metadata(&backup_path).unwrap().len() > 0, "DB backup must not be empty");
    }

    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_restore_handles_missing_assets_sidecar_gracefully() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));

        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();
        let backup_path = create_backup(&db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed).unwrap();

        // Explicitly remove _assets.tar.gz to simulate an older backup without this sidecar
        let backup_filename = backup_path.file_name().unwrap().to_str().unwrap();
        let assets_sidecar = backup_filename.replace(".db", "_assets.tar.gz");
        let assets_path = get_backups_dir().join(&assets_sidecar);
        if assets_path.exists() {
            fs::remove_file(&assets_path).unwrap();
        }

        // Restore should succeed without the assets sidecar (no warning expected for
        // a "normal" older backup — just a log message)
        let dummy_db = make_test_db_state();
        let result = restore_backup(&backup_path, &db_path, &dummy_db);
        assert!(result.is_ok(), "Restore should succeed even without _assets.tar.gz: {:?}", result);
    }

    // -----------------------------------------------------------------------
    // All-types assets-skip tests (fix #1915 — extends #1910 to every BackupType)
    // -----------------------------------------------------------------------

    /// All four BackupType variants must NOT produce _assets.tar.gz.
    /// Also verifies that the essential sidecars (DB, keychain, install-uuid, metadata)
    /// are still produced for every type.
    ///
    /// Renamed from `test_migration_backup_skips_assets_archive` (which tested Migration only).
    /// Generalised by #1915 to cover Daily, Weekly, Migration, and Manual.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_all_backup_types_skip_assets_archive() {
        // Use MockKeychain so this test does not race with keystore tests on the real macOS Keychain.
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        // Populate a realistic Assets tree.
        let assets_dir = pane_root.join("Projects").join("MyProject").join("Assets");
        fs::create_dir_all(&assets_dir).unwrap();
        write_file(&assets_dir.join("blob1.png"), b"fake png one");
        write_file(&assets_dir.join("blob2.jpg"), b"fake jpg two");
        write_file(&assets_dir.join("blob3.pdf"), b"fake pdf three");

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));

        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        let cases: &[(&str, BackupType, Option<&str>)] = &[
            ("migration", BackupType::Migration, Some("v68-to-v69")),
            ("manual",    BackupType::Manual,    None),
            ("daily",     BackupType::Daily,     None),
            ("weekly",    BackupType::Weekly,    None),
        ];

        for (label, btype, version) in cases {
            std::thread::sleep(std::time::Duration::from_millis(1100)); // unique timestamp per case
            let backup_path = create_backup(&db_path, btype.clone(), *version, None, BackupPacing::FullSpeed).unwrap();
            let stem = backup_path.file_stem().unwrap().to_str().unwrap();

            // --- Core assertion: _assets.tar.gz must NOT exist ---
            let assets_path = get_backups_dir().join(format!("{}_assets.tar.gz", stem));
            assert!(
                !assets_path.exists(),
                "{} backup must NOT produce _assets.tar.gz (fix #1915), but found: {:?}",
                label, assets_path
            );

            // --- Essential sidecars must still be present ---
            assert!(backup_path.exists(), "{} DB backup file must exist", label);
            assert!(fs::metadata(&backup_path).unwrap().len() > 0, "{} DB backup must not be empty", label);

            let keychain_path = get_backups_dir().join(format!("{}_keychain.json", stem));
            assert!(
                keychain_path.exists(),
                "{} backup must produce _keychain.json (got absent: {:?})",
                label, keychain_path
            );

            let uuid_path = get_backups_dir().join(format!("{}_install-uuid.json", stem));
            assert!(
                uuid_path.exists(),
                "{} backup must produce _install-uuid.json (got absent: {:?})",
                label, uuid_path
            );

            let metadata_path = get_backups_dir().join(format!("{}_metadata.json", stem));
            assert!(
                metadata_path.exists(),
                "{} backup must produce _metadata.json (got absent: {:?})",
                label, metadata_path
            );

            let meta_json = fs::read_to_string(&metadata_path).unwrap();
            let meta: BackupMetadata = serde_json::from_str(&meta_json).unwrap();
            assert_eq!(
                meta.backup_type, *label,
                "{} backup metadata backup_type mismatch",
                label
            );
        }
    }

    /// Regression guard: no backup type should EVER produce _assets.tar.gz.
    /// This is the single-line canary — if the gate at create_backup is accidentally
    /// re-added, this test catches it across ALL four variants.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_no_backup_type_produces_assets_archive() {
        // Use MockKeychain so this test does not race with keystore tests on the real macOS Keychain.
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        // Large enough assets to make a tar-gz if the branch runs
        let assets_dir = pane_root.join("Projects").join("GuardProject").join("Assets");
        fs::create_dir_all(&assets_dir).unwrap();
        for i in 0..5 {
            write_file(&assets_dir.join(format!("guard{}.png", i)), b"guard asset content");
        }

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        let variants: &[(&str, BackupType, Option<&str>)] = &[
            ("Migration", BackupType::Migration, Some("guard-test")),
            ("Manual",    BackupType::Manual,    None),
            ("Daily",     BackupType::Daily,     None),
            ("Weekly",    BackupType::Weekly,    None),
        ];

        for (label, btype, version) in variants {
            std::thread::sleep(std::time::Duration::from_millis(1100));
            let backup_path = create_backup(&db_path, btype.clone(), *version, None, BackupPacing::FullSpeed).unwrap();
            let stem = backup_path.file_stem().unwrap().to_str().unwrap();
            let assets_path = get_backups_dir().join(format!("{}_assets.tar.gz", stem));
            assert!(
                !assets_path.exists(),
                "[REGRESSION] {} backup produced _assets.tar.gz — this must never happen (#1915)",
                label
            );
        }
    }

    /// Keep this alias so the test name from #1910 still appears in CI output.
    /// The full generalised coverage is in `test_all_backup_types_skip_assets_archive`.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_migration_backup_skips_assets_archive() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        let assets_dir = pane_root.join("Projects").join("MigProject").join("Assets");
        fs::create_dir_all(&assets_dir).unwrap();
        write_file(&assets_dir.join("blob1.png"), b"fake png one");

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();
        let backup_path = create_backup(&db_path, BackupType::Migration, Some("v68-to-v69"), None, BackupPacing::FullSpeed).unwrap();
        let stem = backup_path.file_stem().unwrap().to_str().unwrap();
        let assets_path = get_backups_dir().join(format!("{}_assets.tar.gz", stem));
        assert!(
            !assets_path.exists(),
            "Migration backup must NOT produce _assets.tar.gz (fix #1910 / #1915)"
        );
    }

    // test_non_migration_backup_still_includes_assets DELETED by #1915:
    // it asserted the opposite of the new behavior (non-Migration types now also skip assets).
    // Coverage is replaced by test_no_backup_type_produces_assets_archive above.

    /// Performance guard: any backup with a populated Assets tree must complete quickly
    /// because the asset archive step is skipped for ALL types (#1915 — extends #1910).
    /// Uses a moderately sized fixture (~2 MB synthetic data) to prove the skip is real.
    ///
    /// Renamed from `test_migration_backup_completes_fast_with_large_assets`; now tests
    /// Manual and Daily in addition to Migration.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_all_backup_types_complete_fast_with_large_assets() {
        // Use MockKeychain so this test does not race with keystore tests on the real macOS Keychain.
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        // Create a moderately large fake assets tree (~2 MB total across 20 files × 100 KB each).
        // This is nowhere near 5.4 GB but is large enough that if tar-gzip were running
        // (even with compression), it would add measurable time vs. the skip path.
        let assets_dir = pane_root.join("Projects").join("BigProject").join("Assets");
        fs::create_dir_all(&assets_dir).unwrap();
        let fake_blob: Vec<u8> = vec![0xAB; 100 * 1024]; // 100 KB of 0xAB bytes
        for i in 0..20 {
            write_file(&assets_dir.join(format!("blob{:02}.bin", i)), &fake_blob);
        }

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));

        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        let perf_cases: &[(&str, BackupType, Option<&str>)] = &[
            ("Migration", BackupType::Migration, Some("v68-to-v69")),
            ("Manual",    BackupType::Manual,    None),
            ("Daily",     BackupType::Daily,     None),
        ];

        for (label, btype, version) in perf_cases {
            std::thread::sleep(std::time::Duration::from_millis(1100));
            let t0 = std::time::Instant::now();
            let backup_path = create_backup(&db_path, btype.clone(), *version, None, BackupPacing::FullSpeed).unwrap();
            let elapsed = t0.elapsed();

            // Each backup type must complete in under 5 seconds with a 2 MB Assets tree.
            // If the archive step were NOT skipped, tar-gzip over 2 MB would add measurable
            // time. On Matt's real 5.4 GB library it took ~2 min — this guard proves the
            // skip is real for every type.
            assert!(
                elapsed.as_secs() < 5,
                "{} backup with 2 MB assets took {:?} — expected < 5s (archive should be skipped, fix #1915)",
                label, elapsed
            );

            let stem = backup_path.file_stem().unwrap().to_str().unwrap();
            let assets_path = get_backups_dir().join(format!("{}_assets.tar.gz", stem));
            assert!(
                !assets_path.exists(),
                "_assets.tar.gz must not exist for {} backup (performance + correctness guard #1915)",
                label
            );
        }
    }

    // =========================================================================
    // #2723 PR A — full-speed migration backup
    // =========================================================================

    /// Manual timing harness — real before/after numbers for the PR description,
    /// at Matt's real DB sizes. Not run by a bare `cargo test` sweep (`#[ignore]`);
    /// invoke explicitly with `--ignored --exact --nocapture`. Builds via real
    /// commands (`run_migrations` + `create_note_internal`), times a single
    /// `FullSpeed` `create_backup()` call on each size, and prints the result.
    /// "Before" numbers are not re-measured here — Reed's audit already measured
    /// the identical throttled-copy call at both these sizes (15.19 MB/190.63s,
    /// 50.08 MB/628.83s) and cross-validated the 15 MB figure against a real
    /// production log (#2583); re-running a ~10.5-minute throttled copy here
    /// would reproduce the same number at real cost for no new information.
    #[test]
    #[ignore]
    fn timing_harness_full_speed_copy_15mb_and_50mb() {
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        let _kc = KeychainGuard::set();

        for target_mb in [15u64, 50u64] {
            let db_path = temp.path().join(format!("timing_{}mb.db", target_mb));
            build_realistic_db_via_real_commands(&db_path, target_mb * 1_000_000);
            let actual_bytes = fs::metadata(&db_path).unwrap().len();
            let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

            let t0 = std::time::Instant::now();
            let backup_path = create_backup(
                &db_path, BackupType::Migration, Some(&format!("timing-{}mb", target_mb)), None, BackupPacing::FullSpeed,
            ).unwrap();
            let elapsed = t0.elapsed();
            assert!(verify_backup_integrity(&backup_path).is_ok());

            eprintln!(
                "[TIMING-HARNESS] target={} MB actual={} bytes FullSpeed create_backup()={:.3}s",
                target_mb, actual_bytes, elapsed.as_secs_f64()
            );
        }
    }

    /// Build a real canonical-schema database and fill it with real content via
    /// the actual `create_note_internal` command — NOT raw `INSERT` — until it
    /// reaches at least `target_bytes`. Per the test-isolation brief: "Build data
    /// through real commands, never by inserting rows." Mirrors Reed's audit
    /// methodology ("real canonical schema via run_migrations, padded to size").
    /// Notes use `notes::WORKSHOP_SCOPE_ID` so they stay DB-only (no on-disk
    /// note files, per §24.10.1e) — this function only ever writes to `db_path`.
    fn build_realistic_db_via_real_commands(db_path: &Path, target_bytes: u64) {
        let conn = Connection::open(db_path).unwrap();
        crate::migrations::run_migrations(&conn)
            .expect("real schema creation via run_migrations");

        // ~10 KB of repeated, realistic prose per note — keeps the real-command
        // note count (and the 3 queries create_note_internal runs per note) in a
        // reasonable range even at 50 MB.
        let paragraph = "Pane is a local-first notes and tasks app for people who think in \
            lists, cabinets, and projects rather than folders. ".repeat(160);
        let mut i: u64 = 0;
        loop {
            let size = fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
            if size >= target_bytes {
                break;
            }
            let req = crate::notes::CreateNoteRequest {
                project_id: crate::notes::WORKSHOP_SCOPE_ID.to_string(),
                folder_id: None,
                title: format!("Perf test note {}", i),
                content: Some(paragraph.clone()),
                tags: None,
                status: Some("cabinet".to_string()),
                priority: None,
                due_date: None,
                parent_id: None,
            };
            crate::notes::create_note_internal(&conn, req)
                .expect("create_note_internal should succeed for real-command test data");
            i += 1;
        }
        eprintln!(
            "[TEST] build_realistic_db_via_real_commands: {} notes, {} bytes (target {})",
            i,
            fs::metadata(db_path).map(|m| m.len()).unwrap_or(0),
            target_bytes
        );
        drop(conn);
    }

    /// Create a minimal `.pane/system/` tree so `backup_flat_files` has something
    /// real to archive (mirrors the Assets-tree pattern already used above for
    /// `write_file`, just under `system/` instead of `Projects/.../Assets/`).
    fn populate_system_dir(pane_root: &Path) {
        let system_dir = pane_root.join(".pane").join("system");
        fs::create_dir_all(&system_dir).unwrap();
        write_file(&system_dir.join("settings.json"), b"{\"theme\":\"dark\"}");
        write_file(&system_dir.join("workshop.json"), b"{\"items\":[]}");
    }

    // ---- 1. Pacing mechanism (#2723 item 1) --------------------------------

    /// `BackupPacing`'s params must be explicit, named values — not a re-guessed
    /// magic pair. `FullSpeed` copies everything in one step (SQLite's own `-1`
    /// convention), `ThrottledForLiveApp` preserves the exact pre-#2723 constants.
    #[test]
    fn test_backup_pacing_step_params_are_explicit() {
        assert_eq!(
            BackupPacing::FullSpeed.step_params(),
            (-1, std::time::Duration::from_millis(0))
        );
        assert_eq!(
            BackupPacing::ThrottledForLiveApp.step_params(),
            (5, std::time::Duration::from_millis(250))
        );
    }

    /// The live-app (manual backup) caller must still pace noticeably slower than
    /// a full-speed copy of the same data — proving the throttle survives where
    /// Matt asked it to, not just that FullSpeed got faster. Uses a 1 MB real
    /// database (via `create_note_internal`) so `ThrottledForLiveApp` (~80 KB/s)
    /// takes a reliably-measurable ~12s while `FullSpeed` takes well under 1s.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_live_app_pacing_still_throttles_vs_full_speed() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        let db_path = temp.path().join("test.db");
        build_realistic_db_via_real_commands(&db_path, 1_000_000);
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        let t0 = std::time::Instant::now();
        let _full_speed_backup = create_backup(
            &db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed,
        ).unwrap();
        let full_speed_elapsed = t0.elapsed();

        // Distinct second-resolution timestamp so the second Manual backup doesn't
        // collide with the first's filename (`pane_manual-{timestamp}.db`).
        std::thread::sleep(std::time::Duration::from_millis(1100));

        let t1 = std::time::Instant::now();
        let _throttled_backup = create_backup(
            &db_path, BackupType::Manual, None, None, BackupPacing::ThrottledForLiveApp,
        ).unwrap();
        let throttled_elapsed = t1.elapsed();

        eprintln!(
            "[TEST] pacing comparison: full_speed={:?}, throttled={:?}",
            full_speed_elapsed, throttled_elapsed
        );

        assert!(
            full_speed_elapsed.as_secs_f64() < 2.0,
            "FullSpeed copy of a 1 MB DB should take well under 2s, took {:?}",
            full_speed_elapsed
        );
        assert!(
            throttled_elapsed.as_secs_f64() > 6.0,
            "ThrottledForLiveApp copy of a 1 MB DB should preserve pre-#2723 pacing \
             (~80 KB/s → >6s for 1 MB), took {:?} — the live-app throttle may have been \
             accidentally removed",
            throttled_elapsed
        );
        assert!(
            throttled_elapsed > full_speed_elapsed * 3,
            "Throttled pacing ({:?}) should be dramatically slower than full speed ({:?}) \
             for identical data — otherwise FullSpeed and ThrottledForLiveApp aren't \
             actually different",
            throttled_elapsed, full_speed_elapsed
        );
    }

    // ---- 2. Archive skip for Migration only (#2723 item 2) -----------------

    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_migration_backup_skips_system_archive() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        populate_system_dir(&pane_root);

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        let backup_path = create_backup(
            &db_path, BackupType::Migration, Some("v91-to-v92"), None, BackupPacing::FullSpeed,
        ).unwrap();

        let stem = backup_path.file_stem().unwrap().to_str().unwrap();
        let tar_path = get_backups_dir().join(format!("{}_system.tar.gz", stem));
        assert!(
            !tar_path.exists(),
            "Migration backup must NOT produce _system.tar.gz (#2723 item 2), but found: {:?}",
            tar_path
        );
        assert!(backup_path.exists(), "DB backup file must still exist");
        assert!(fs::metadata(&backup_path).unwrap().len() > 0, "DB backup must not be empty");
    }

    /// Regression guard: every OTHER backup type must still produce the system
    /// archive unconditionally — the skip is scoped to Migration only.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_non_migration_backups_still_archive_system_files() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        populate_system_dir(&pane_root);

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        let cases: &[(&str, BackupType, Option<&str>)] = &[
            ("manual", BackupType::Manual, None),
            ("daily", BackupType::Daily, None),
            ("weekly", BackupType::Weekly, None),
            ("pre-restore", BackupType::PreRestore, None),
        ];
        for (label, btype, version) in cases {
            let backup_path = create_backup(
                &db_path, btype.clone(), *version, None, BackupPacing::FullSpeed,
            ).unwrap();
            let stem = backup_path.file_stem().unwrap().to_str().unwrap();
            let tar_path = get_backups_dir().join(format!("{}_system.tar.gz", stem));
            assert!(
                tar_path.exists(),
                "{} backup must still produce _system.tar.gz (only Migration skips it), expected: {:?}",
                label, tar_path
            );
            std::thread::sleep(std::time::Duration::from_millis(1100)); // distinct timestamps for Daily/Weekly/PreRestore
        }
    }

    /// A Migration backup set with no `_system.tar.gz` must still be a valid
    /// restore source (#2723 item 2 caveat: "the backup must stay restorable").
    /// Confirmed by execution, not just by reading the soft-fail branch.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_restore_migration_backup_without_system_archive() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        populate_system_dir(&pane_root);

        let db_path = temp.path().join("test.db");
        // Baseline schema alone (via run_migrations) is already several hundred KB,
        // so the target must comfortably exceed that floor to guarantee real note
        // rows get created below.
        build_realistic_db_via_real_commands(&db_path, 900_000);
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        let note_count_before: i64 = {
            let conn = Connection::open(&db_path).unwrap();
            conn.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0)).unwrap()
        };
        assert!(note_count_before > 0, "fixture must have real note rows");

        let backup_path = create_backup(
            &db_path, BackupType::Migration, Some("v91-to-v92"), None, BackupPacing::FullSpeed,
        ).unwrap();

        // Confirm the archive really is absent — the restore below must succeed
        // DESPITE this, not because we forgot to check.
        let stem = backup_path.file_stem().unwrap().to_str().unwrap();
        let tar_path = get_backups_dir().join(format!("{}_system.tar.gz", stem));
        assert!(!tar_path.exists(), "precondition: no system archive for this Migration backup");

        // Restore into a fresh target path/DbState — both source and metadata report
        // schema_version=0 (create_test_db_with_schema not used; real DB has no
        // ai_settings yet at this point in a fresh canonical-schema install) so the
        // schema check (equal versions) does not require confirmation.
        let restore_target = temp.path().join("restored.db");
        fs::copy(&db_path, &restore_target).unwrap(); // start from a copy so source is untouched
        let dummy_db = make_test_db_state();
        let summary = restore_backup(&backup_path, &restore_target, &dummy_db)
            .expect("restore from a Migration backup with no system archive must succeed");

        // No SystemFilesRestoreFailed warning — a genuinely-absent archive on a
        // Migration backup is expected, not an error condition (soft-fail log only).
        assert!(
            !summary.warnings.iter().any(|w| w.class == "SystemFilesRestoreFailed"),
            "restoring a Migration backup with no archive must not raise a SystemFilesRestoreFailed \
             warning — absence is expected for this backup type, got: {:?}",
            summary.warnings
        );

        let note_count_after: i64 = {
            let conn = Connection::open(&restore_target).unwrap();
            conn.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0)).unwrap()
        };
        assert_eq!(note_count_after, note_count_before, "restored DB must have the same real note rows");
    }

    // ---- 3. Output staging (#2723 item 3 / #3049 lesson) -------------------

    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_stale_db_staging_file_cleaned_up_before_new_backup() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        // Migration backups use a fixed filename, so we can pre-create the exact
        // staging path a prior killed run would have left behind.
        ensure_backups_dir().unwrap();
        let final_db_path = get_backups_dir().join("pane_pre-migration-staging-test.db");
        let staging_db_path = staging_path_for(&final_db_path);
        fs::write(&staging_db_path, b"PARTIAL GARBAGE FROM A KILLED RUN").unwrap();
        assert!(staging_db_path.exists(), "precondition: stale staging file exists");

        let backup_path = create_backup(
            &db_path, BackupType::Migration, Some("staging-test"), None, BackupPacing::FullSpeed,
        ).expect("create_backup must succeed despite a stale staging file from a prior kill");

        assert_eq!(backup_path, final_db_path);
        assert!(!staging_db_path.exists(), "stale/leftover staging file must not survive a successful backup");
        assert!(backup_path.exists(), "final backup must be published");
        assert!(verify_backup_integrity(&backup_path).is_ok(), "published backup must be a real, valid DB (not the stale garbage)");
    }

    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_stale_tar_staging_file_cleaned_up_before_new_archive() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);
        populate_system_dir(&pane_root);

        let db_path = temp.path().join("test.db");
        {
            let conn = create_test_db(&db_path);
            drop(conn);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        ensure_backups_dir().unwrap();
        // Manual backups are timestamped, so build the exact filename a backup
        // started "now" would use, and pre-seed its tar staging path with garbage.
        let timestamp = Local::now().format("%Y%m%d_%H%M%S").to_string();
        let final_tar_path = get_backups_dir().join(format!("pane_manual-{}_system.tar.gz", timestamp));
        let staging_tar_path = staging_path_for(&final_tar_path);
        fs::write(&staging_tar_path, b"PARTIAL GARBAGE FROM A KILLED ARCHIVE RUN").unwrap();
        assert!(staging_tar_path.exists(), "precondition: stale tar staging file exists");

        let backup_path = create_backup(
            &db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed,
        ).expect("create_backup must succeed despite a stale tar staging file from a prior kill");

        let stem = backup_path.file_stem().unwrap().to_str().unwrap();
        let tar_path = get_backups_dir().join(format!("{}_system.tar.gz", stem));
        assert!(tar_path.exists(), "archive must still be published");
        assert!(!staging_tar_path.exists(), "stale tar staging file must not survive a successful archive");

        // Verify it's a REAL gzip archive (not the leftover garbage) by actually
        // listing its contents.
        let output = std::process::Command::new("tar")
            .arg("-tzf")
            .arg(&tar_path)
            .output()
            .unwrap();
        assert!(output.status.success(), "published archive must be a valid, listable tar.gz, got: {:?}", output);
        let listing = String::from_utf8_lossy(&output.stdout);
        assert!(listing.contains("settings.json"), "archive must contain the real system files, got listing: {}", listing);
    }

    /// Child-process half of the real SIGKILL quit-safety test below. Not run by
    /// a bare `cargo test` sweep (requires `--ignored --exact`); invoked directly
    /// by `test_quit_mid_copy_leaves_no_partial_output_at_final_name` via
    /// `std::env::current_exe()`. Deliberately uses `ThrottledForLiveApp` pacing
    /// (slow, ~80 KB/s) so the parent has a comfortable, reliable window to
    /// detect the in-progress staging file and kill this process mid-copy —
    /// this is a TEST-ONLY pacing choice to make the race deterministic; it does
    /// not reflect production Migration pacing (which is FullSpeed).
    #[test]
    #[ignore]
    fn kill_harness_slow_backup_copy() {
        let db_path = PathBuf::from(
            std::env::var("PANE_KILL_HARNESS_DB_PATH").expect("PANE_KILL_HARNESS_DB_PATH must be set"),
        );
        let result = create_backup(
            &db_path,
            BackupType::Migration,
            Some("kill-harness-test"),
            None,
            BackupPacing::ThrottledForLiveApp,
        );
        eprintln!("[KILL-HARNESS] create_backup returned: {:?}", result.is_ok());
    }

    /// Real quit-mid-copy proof (#2723 Tests requirement + Reed's audit item 5):
    /// actually SIGKILLs a child process while it is mid-way through a backup
    /// copy, then asserts (a) no half-written file is ever visible at the final,
    /// listed filename, and (b) the very next call to `create_backup` for the
    /// same target proceeds cleanly — no leftover staging file, no accumulation.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_quit_mid_copy_leaves_no_partial_output_at_final_name() {
        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        let db_path = temp.path().join("source.db");
        // ~1.2 MB — at ThrottledForLiveApp's ~80 KB/s this takes ~15s, giving the
        // parent a wide, reliable window to detect the staging file and kill.
        build_realistic_db_via_real_commands(&db_path, 1_200_000);
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        let final_db_path = get_backups_dir().join("pane_pre-migration-kill-harness-test.db");
        let staging_db_path = staging_path_for(&final_db_path);
        // Clean slate — a prior failed test run might have left either file behind.
        let _ = fs::remove_file(&final_db_path);
        let _ = fs::remove_file(&staging_db_path);

        // Capture the child's stderr to a real file (not a pipe — avoids any risk of
        // this process blocking on a full pipe buffer while the child is still
        // writing) so we can synchronize on its `eprintln!` phase markers instead
        // of on destination-file byte growth. (Measured on this box: SQLite's
        // backup-API destination file's apparent length stays at 0 for most of the
        // copy and only reflects real content very close to completion — not a
        // reliable "copy in progress" signal. The phase-log lines this PR added
        // for #2724 are.)
        let log_path = temp.path().join("kill_harness_child.log");
        let log_file = fs::File::create(&log_path).unwrap();

        let exe = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(&exe)
            .args([
                "backup::tests::kill_harness_slow_backup_copy",
                "--exact",
                "--ignored",
                "--test-threads=1",
                // Rust's default libtest output capture intercepts eprintln! too
                // (not just println!), discarding it unless the test fails or
                // --nocapture is passed. We need the child's phase-log lines live
                // to synchronize the kill, so --nocapture is required here.
                "--nocapture",
            ])
            .env("PANE_TEST_ROOT", &pane_root)
            .env("PANE_KILL_HARNESS_DB_PATH", &db_path)
            .stdout(std::process::Stdio::null())
            .stderr(log_file)
            .spawn()
            .expect("failed to spawn kill-harness child process");

        // Wait for the child to report it has actually started the throttled copy
        // step (the `backup_copying` phase line) — proof the race is against the
        // copy itself, not just process startup.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut saw_copying_phase = false;
        while std::time::Instant::now() < deadline {
            if let Ok(contents) = fs::read_to_string(&log_path) {
                if contents.contains("Phase: backup_copying") {
                    saw_copying_phase = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            saw_copying_phase,
            "child never reported starting the copy phase within 10s — test is not actually racing the copy. child log so far: {:?}, child.try_wait()={:?}",
            fs::read_to_string(&log_path).unwrap_or_default(),
            child.try_wait()
        );

        // At ThrottledForLiveApp's ~80 KB/s, a ~1.2 MB copy takes several seconds.
        // Sleep partway into that window — comfortably past the start, comfortably
        // before completion — then kill.
        std::thread::sleep(std::time::Duration::from_secs(5));

        // Confirm we are still genuinely mid-copy (the NEXT phase, backup_verifying,
        // has not been reached yet) before killing — otherwise this wouldn't be a
        // mid-copy kill at all.
        let log_before_kill = fs::read_to_string(&log_path).unwrap_or_default();
        assert!(
            !log_before_kill.contains("Phase: backup_verifying"),
            "copy already finished before the kill — not a valid mid-copy test (child log: {})",
            log_before_kill
        );

        // Kill now, mid-copy.
        child.kill().expect("failed to SIGKILL the kill-harness child");
        let _ = child.wait();

        // The final, listed filename must NEVER have existed during this window.
        assert!(
            !final_db_path.exists(),
            "a half-written backup must never appear at the final, listed filename after a kill mid-copy"
        );
        // The staging leftover is the expected artifact of a kill — confirm it's there
        // (otherwise the assertion above is vacuous).
        assert!(staging_db_path.exists(), "expected a leftover staging file after the kill");

        // Next launch: retry cleanly. FullSpeed here mirrors production Migration
        // pacing; the point under test is the staging/cleanup behavior, not speed.
        let retry_result = create_backup(
            &db_path, BackupType::Migration, Some("kill-harness-test"), None, BackupPacing::FullSpeed,
        );
        assert!(retry_result.is_ok(), "retry after a killed run must succeed cleanly: {:?}", retry_result);
        assert!(!staging_db_path.exists(), "stale staging file from the killed run must be gone after a clean retry");
        assert!(final_db_path.exists(), "retry must publish a valid backup at the final filename");
        assert!(verify_backup_integrity(&final_db_path).is_ok(), "retried backup must be a complete, valid DB");
    }

    /// Child-process half of the real SIGKILL archive-kill test below. Not run
    /// by a bare `cargo test` sweep (requires `--ignored --exact`); invoked
    /// directly by `test_quit_mid_archive_leaves_no_partial_output_at_final_name`
    /// via `std::env::current_exe()`. Uses `BackupType::Manual` — unlike the
    /// `.db` kill harness above (`BackupType::Migration`), because Migration
    /// now skips the archive step entirely (#2723 item 2); Manual is the one
    /// FullSpeed-eligible type that still always archives.
    #[test]
    #[ignore]
    fn kill_harness_slow_backup_archive() {
        let db_path = PathBuf::from(
            std::env::var("PANE_KILL_HARNESS_DB_PATH").expect("PANE_KILL_HARNESS_DB_PATH must be set"),
        );
        let result = create_backup(
            &db_path,
            BackupType::Manual,
            None,
            None,
            BackupPacing::FullSpeed,
        );
        eprintln!("[KILL-HARNESS] create_backup returned: {:?}", result.is_ok());
    }

    /// Real quit-mid-archive proof (Reed's audit §4 WARN on PR #3064: the
    /// `.tar.gz` archive step only had a simulated-garbage test, never a real
    /// SIGKILL of the actual `tar` process this code spawns). Mirrors
    /// `test_quit_mid_copy_leaves_no_partial_output_at_final_name` above, but
    /// for the archive step: actually SIGKILLs the real `tar` child process
    /// spawned by `backup_flat_files`, mid-write, via the real `create_backup`
    /// path, then asserts (a) no half-written archive is ever visible at the
    /// final, listed `_system.tar.gz` filename, and (b) the very next backup
    /// attempt completes and produces a valid, listable archive.
    ///
    /// `BackupType::Manual` is used (not `Migration`, which skips archiving
    /// entirely per #2723 item 2) — it's the one type that still always
    /// archives.
    ///
    /// `backup_flat_files` shells out to the real `tar` binary, so killing only
    /// the Rust harness process (as the `.db` test does with a plain
    /// `child.kill()`) would NOT kill `tar` — SIGKILL doesn't propagate to a
    /// process's children, so `tar` would be silently reparented and keep
    /// writing to completion in the background, undermining the "genuinely
    /// killed mid-archive" claim. The harness is therefore spawned in its own
    /// process group (`process_group(0)`); `tar` inherits that same group, and
    /// the kill targets the whole group (`kill -9 -- -<pgid>`) so both die
    /// together — verified empirically (real `tar` process confirmed gone,
    /// destination left genuinely incomplete) before landing this test.
    #[serial_test::serial(pane_test_root, keychain_override)]
    #[test]
    fn test_quit_mid_archive_leaves_no_partial_output_at_final_name() {
        use std::os::unix::process::CommandExt;
        use std::io::Write;
        use rand::RngCore;

        let _kc = KeychainGuard::set();
        let temp = TempDir::new().unwrap();
        let pane_root = temp.path().join("Pane");
        fs::create_dir_all(&pane_root).unwrap();
        let _test_root_guard = TestRootGuard::set(&pane_root);

        let db_path = temp.path().join("source.db");
        build_realistic_db_via_real_commands(&db_path, 50_000);
        let _uuid = crate::install_uuid::get_or_create_install_uuid().unwrap();

        // Real fixture for the system directory being archived: two 150 MB
        // files of incompressible (random) data, written from one reused 1 MB
        // random block — gzip's 32 KB window can't find matches across writes
        // 1 MB apart, so the compressed output ends up ~the same size as the
        // input (confirmed empirically before landing this test), without
        // paying the CPU cost of generating 300 MB of fresh randomness.
        // Measured on this box: `tar -czf` over this fixture takes ~3.5s — a
        // real, comfortable wall-clock window to kill inside, well above the
        // sub-second archive time of an actual install's tiny `.pane/system/`.
        let system_dir = pane_root.join(".pane").join("system");
        fs::create_dir_all(&system_dir).unwrap();
        let mut block = vec![0u8; 1_000_000];
        rand::thread_rng().fill_bytes(&mut block);
        for name in ["filler1.bin", "filler2.bin"] {
            let mut f = fs::File::create(system_dir.join(name)).unwrap();
            for _ in 0..150 {
                f.write_all(&block).unwrap();
            }
        }

        let log_path = temp.path().join("archive_kill_harness_child.log");
        let log_file = fs::File::create(&log_path).unwrap();

        let exe = std::env::current_exe().unwrap();
        let mut child = std::process::Command::new(&exe)
            .args([
                "backup::tests::kill_harness_slow_backup_archive",
                "--exact",
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("PANE_TEST_ROOT", &pane_root)
            .env("PANE_KILL_HARNESS_DB_PATH", &db_path)
            .stdout(std::process::Stdio::null())
            .stderr(log_file)
            // New process group (pgid = this child's own pid) so the `tar`
            // subprocess it spawns inherits the SAME group — letting us kill
            // both together below. Without this, `tar` would survive the
            // harness-only kill and finish writing in the background.
            .process_group(0)
            .spawn()
            .expect("failed to spawn archive-kill-harness child process");

        // Wait for the child to report it has started archiving (the
        // `backup_archiving` phase line) — same synchronization technique as
        // the `.db` copy test above: phase logs, not file-size polling.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut saw_archiving_phase = false;
        while std::time::Instant::now() < deadline {
            if let Ok(contents) = fs::read_to_string(&log_path) {
                if contents.contains("Phase: backup_archiving") {
                    saw_archiving_phase = true;
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            saw_archiving_phase,
            "child never reported starting the archiving phase within 10s — test is not actually racing the archive. child log so far: {:?}, child.try_wait()={:?}",
            fs::read_to_string(&log_path).unwrap_or_default(),
            child.try_wait()
        );

        // Pull the exact backup filename the child chose (logged just before
        // the archiving phase, when the `.db` half was published) — ground
        // truth for the final/staging tar paths, rather than re-predicting
        // `Local::now()` ourselves across a process spawn (which could drift
        // across a second boundary and silently check the wrong path).
        let log_after_start = fs::read_to_string(&log_path).unwrap_or_default();
        let db_filename = log_after_start
            .split("Published database backup: Some(\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .expect("expected a 'Published database backup: Some(\"...\")' log line before the archiving phase")
            .to_string();
        let tar_name = db_filename.replace(".db", "_system.tar.gz");
        let final_tar_path = get_backups_dir().join(&tar_name);
        let staging_tar_path = staging_path_for(&final_tar_path);
        // No stale-leftover cleanup here (unlike the `.db` test above, which
        // reuses a fixed filename run-to-run): this `pane_root` is a brand-new
        // `TempDir` and `Manual`'s timestamp-based filename is effectively
        // unique per run, so there is nothing to clean up — and critically,
        // unlinking `staging_tar_path` here (after archiving has already
        // started) would race the live `tar` process: POSIX unlink-while-open
        // leaves the open fd writing to an now-nameless inode, so `metadata()`
        // on this path would never see it again and the later `rename` in
        // `publish_staged` would fail with ENOENT — silently, since the
        // caller only checks `.is_ok()`. (Found exactly this by debugging an
        // early draft of this test.)

        // Now that archiving is confirmed underway, let the real `tar`
        // process make genuine progress into our ~3.5s fixture before
        // killing. The staging file's size growth IS a reliable mid-progress
        // signal for `tar`'s streaming gzip output (unlike the SQLite backup
        // API copy above, whose destination length Reed found stays near 0
        // until near completion) — using it here lets the kill point scale
        // with actual machine speed instead of a single blind sleep.
        let expected_total_bytes = 2 * 150_000_000u64;
        let kill_floor = expected_total_bytes / 4; // ~25% in — comfortably mid-write
        let kill_deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Ok(meta) = fs::metadata(&staging_tar_path) {
                if meta.len() >= kill_floor {
                    break;
                }
            }
            if std::time::Instant::now() >= kill_deadline {
                break; // the "not yet finished" check right below still guards correctness
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // Confirm we are still genuinely mid-archive (the child has not yet
        // logged its next phase, `backup_finalizing`, which only happens after
        // a successful archive) before killing — otherwise this wouldn't be a
        // valid mid-archive test at all.
        let log_before_kill = fs::read_to_string(&log_path).unwrap_or_default();
        assert!(
            !log_before_kill.contains("Phase: backup_finalizing"),
            "archive already finished before the kill — not a valid mid-archive test (child log: {})",
            log_before_kill
        );

        // Kill the whole process group (harness + its live `tar` child) at once.
        let pgid = child.id();
        let kill_status = std::process::Command::new("kill")
            .args(["-9", "--", &format!("-{}", pgid)])
            .status()
            .expect("failed to invoke kill(1) on the archive-kill-harness process group");
        assert!(kill_status.success(), "kill -9 against the harness process group failed: {:?}", kill_status);
        let _ = child.wait();

        // The final, listed archive filename must NEVER have existed during this window.
        assert!(
            !final_tar_path.exists(),
            "a half-written archive must never appear at the final, listed filename after a kill mid-archive"
        );
        // The staging leftover is the expected artifact of a kill — confirm
        // it's there (otherwise the assertion above is vacuous), and that
        // it's genuinely incomplete (smaller than a real finished archive of
        // this fixture would be), proving the kill actually landed mid-write
        // rather than after `tar` had already finished.
        let staging_meta = fs::metadata(&staging_tar_path)
            .expect("expected a leftover tar staging file after the kill");
        assert!(
            staging_meta.len() < expected_total_bytes,
            "staging archive looks complete ({} bytes) — the kill did not actually land mid-write",
            staging_meta.len()
        );
        eprintln!(
            "[TEST] killed mid-archive: staging file was {} bytes (of an expected ~{} byte complete archive)",
            staging_meta.len(), expected_total_bytes
        );

        // Next run: retry cleanly. Doesn't need to reuse the exact same
        // filename as the killed attempt — the point under test is that the
        // NEXT backup attempt for this target succeeds and produces a
        // genuinely valid archive, not a specific replay of the killed
        // filename (that stale-staging-cleanup path is already covered by
        // `test_stale_tar_staging_file_cleaned_up_before_new_archive`).
        let retry_result = create_backup(
            &db_path, BackupType::Manual, None, None, BackupPacing::FullSpeed,
        );
        assert!(retry_result.is_ok(), "retry after a killed archive run must succeed cleanly: {:?}", retry_result);
        let retry_backup_path = retry_result.unwrap();
        let retry_stem = retry_backup_path.file_stem().unwrap().to_str().unwrap();
        let retry_tar_path = get_backups_dir().join(format!("{}_system.tar.gz", retry_stem));
        assert!(retry_tar_path.exists(), "retry must publish a valid archive at its final filename");

        // Verify it's a REAL, listable gzip archive (not a half-written leftover).
        let output = std::process::Command::new("tar")
            .arg("-tzf")
            .arg(&retry_tar_path)
            .output()
            .unwrap();
        assert!(output.status.success(), "retried archive must be a valid, listable tar.gz, got: {:?}", output);
        let listing = String::from_utf8_lossy(&output.stdout);
        assert!(listing.contains("filler1.bin"), "archive must contain the real system files, got listing: {}", listing);
    }
}
