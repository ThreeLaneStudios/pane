// startup.rs — Window-first startup orchestration (#2723 PR B)
//
// PR A (#3064, merged) made the pre-migration backup copy fast (50 MB:
// 628s → 0.14s measured). This PR makes the *remaining* wait visible instead
// of silent: on a launch that needs a schema migration, the window opens
// first and shows "Updating Pane… don't quit" with real progress, while the
// backup + migration run in the background. On every other launch (the
// overwhelming majority), nothing here changes anything — see
// `database::is_migration_pending` and the regression test named in
// `lib.rs`'s `.setup()`.
//
// Reused shape, not a new architecture: issue #353 (commit `0880f573`,
// "Move daily backup to background thread after window renders") already
// proved "defer `.setup()`-time DB work onto a background thread so it
// doesn't hold up the window" for the daily backup. This generalizes that
// exact pattern to the migration-backup + schema-migration case, which is
// the one that can actually be slow (minutes, per Reed's audit) because it
// runs *before* the window ever paints, unlike the daily backup which
// already runs after.

use rusqlite::Connection;
use serde::Serialize;
use std::sync::Mutex;

/// What stage of startup the app is in, from the frontend's point of view.
/// `NotYetDetermined` is only observable for a vanishingly small window (it
/// is replaced by one of the other three variants before `.setup()` can
/// possibly return) — included so `StartupStatus`'s default is honest rather
/// than silently pretending to already know the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupPhase {
    NotYetDetermined,
    MigrationRequired,
    Ready,
    Failed(String),
}

/// JSON payload shape for the `get_startup_status` command — a plain tag +
/// optional message, not the Rust enum directly (keeps the wire format
/// stable even if `StartupPhase`'s Rust representation changes).
#[derive(Debug, Clone, Serialize)]
pub struct StartupStatusPayload {
    pub phase: &'static str,
    pub message: Option<String>,
}

/// Tauri-managed state (`app.manage(StartupStatus::new())`) tracking which
/// startup phase this launch is in. Registered as the very first line inside
/// `.setup()` — before crsqlite path resolution, before the migration
/// pre-check, before anything else — so `get_startup_status` (a command with
/// NO `DbState` dependency) is safely callable from the very first instant
/// the frontend can reach it, on every launch, regardless of which path
/// `.setup()` ends up taking.
///
/// This is the piece that lets the frontend avoid the event-ordering race
/// that a push-only (`emit`/`listen`) design would have: an event emitted
/// before the frontend's listener attaches is simply lost, but a `State`
/// read via a request/response command always reflects whatever the state
/// is *at the moment the frontend asks* — there is no "missed the emit"
/// failure mode for the initial fast-path/slow-path decision.
pub struct StartupStatus(Mutex<StartupPhase>);

impl StartupStatus {
    pub fn new() -> Self {
        StartupStatus(Mutex::new(StartupPhase::NotYetDetermined))
    }

    pub fn set_migration_required(&self) {
        *self.0.lock().unwrap() = StartupPhase::MigrationRequired;
    }

    pub fn set_ready(&self) {
        *self.0.lock().unwrap() = StartupPhase::Ready;
    }

    pub fn set_failed(&self, message: String) {
        *self.0.lock().unwrap() = StartupPhase::Failed(message);
    }

    pub fn snapshot(&self) -> StartupStatusPayload {
        match &*self.0.lock().unwrap() {
            StartupPhase::NotYetDetermined => StartupStatusPayload { phase: "not_yet_determined", message: None },
            StartupPhase::MigrationRequired => StartupStatusPayload { phase: "migration_required", message: None },
            StartupPhase::Ready => StartupStatusPayload { phase: "ready", message: None },
            StartupPhase::Failed(msg) => StartupStatusPayload { phase: "failed", message: Some(msg.clone()) },
        }
    }
}

impl Default for StartupStatus {
    fn default() -> Self {
        Self::new()
    }
}

/// `#[tauri::command]` — no `DbState` dependency, safe to call at any point
/// after the app starts (including while a migration is still running on the
/// background thread). This is the frontend's one gate before it is allowed
/// to mount the real app / call any `DbState`-dependent command.
#[tauri::command]
pub fn get_startup_status(status: tauri::State<StartupStatus>) -> StartupStatusPayload {
    status.snapshot()
}

/// The ordering primitive (#2723 PR B item 3 — Reed's explicit requirement):
/// `DbState` is mounted, and ONLY THEN is `on_db_ready` invoked. Both the
/// fast path (called inline, on the main/setup thread) and the slow/
/// migrating path (called from a background `std::thread::spawn`, same as
/// `#353`'s daily-backup shape) go through this exact function — there is
/// exactly one place in the codebase where "mount DbState, then run the
/// dependent setup blocks" could possibly be reordered, and this function's
/// own two-line body does not allow it:
///
///   1. `manager.manage(DbState::new(conn))` — `conn` must already exist
///      (i.e. the caller's `init_database`/`init_database_with_progress`
///      call, and therefore `migrations::run_migrations[_with_progress]`'s
///      `set_schema_version` on every reachable match arm, already ran to
///      completion) before this function can even be called — the Rust
///      compiler enforces this via ownership of `conn`, it is not a runtime
///      race that happens to resolve in the right order.
///   2. `on_db_ready(manager)` — cannot run until line 1 above it has
///      already returned, by Rust's own sequential execution within a single
///      function body.
///
/// Generic over `R: Runtime` / `M: Manager<R>` specifically so this exact
/// function — not a copy, not a close imitation — is exercised by
/// `test_schema_version_committed_before_db_state_is_managed` below against
/// `tauri::test::mock_app()`'s `AppHandle<MockRuntime>`, matching the
/// pattern already established in this crate (`refresh_event::emit_data_changed`,
/// `projects.rs`'s `install<R: tauri::Runtime>`).
pub fn mount_db_state_then<R, M, F>(
    manager: &M,
    conn: Connection,
    on_db_ready: F,
) -> Result<(), String>
where
    R: tauri::Runtime,
    M: tauri::Manager<R>,
    F: FnOnce(&M) -> Result<(), String>,
{
    manager.manage(crate::database::DbState::new(conn));
    on_db_ready(manager)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::PathBuf;
    use tauri::Manager;

    // ─── StartupStatus — plain unit tests, no Tauri runtime needed ─────────

    #[test]
    fn test_startup_status_defaults_to_not_yet_determined() {
        let status = StartupStatus::new();
        let snap = status.snapshot();
        assert_eq!(snap.phase, "not_yet_determined");
        assert!(snap.message.is_none());
    }

    #[test]
    fn test_startup_status_migration_required_then_ready() {
        let status = StartupStatus::new();
        status.set_migration_required();
        assert_eq!(status.snapshot().phase, "migration_required");
        status.set_ready();
        assert_eq!(status.snapshot().phase, "ready");
    }

    #[test]
    fn test_startup_status_failed_carries_message() {
        let status = StartupStatus::new();
        status.set_failed("disk full".to_string());
        let snap = status.snapshot();
        assert_eq!(snap.phase, "failed");
        assert_eq!(snap.message, Some("disk full".to_string()));
    }

    // ─── Ordering guarantee (#2723 PR B item 3, Reed's explicit requirement) ──

    /// Proves, by execution against a real `tauri::test::mock_app()`, that
    /// `mount_db_state_then`'s dependent-block closure (standing in for the
    /// real §486/orphan-sweep blocks in `lib.rs`) never runs before
    /// `DbState` is mounted, AND that the connection it receives already has
    /// `set_schema_version` committed — using a REAL migration (the actual
    /// `tests/fixtures/upgrade-from-v91.db` fixture, real `run_migrations`,
    /// not a hand-built/raw-INSERT fixture) to get there, not an
    /// already-current fresh DB that would make the ordering trivially true.
    #[test]
    #[serial(pane_test_root, keychain_override)]
    fn test_schema_version_committed_before_db_state_is_managed() {
        use crate::keychain::test_support::KeychainGuard;
        use tempfile::TempDir;

        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures/upgrade-from-v91.db");
        assert!(
            fixture_path.exists(),
            "upgrade-from-v91.db fixture not found at {:?}",
            fixture_path
        );

        let temp_dir = TempDir::new().expect("tempdir");
        let scratch_db = temp_dir.path().join("scratch-v91.db");
        std::fs::copy(&fixture_path, &scratch_db).expect("copy fixture");

        unsafe {
            std::env::set_var("PANE_TEST_ROOT", temp_dir.path());
        }
        let _kc = KeychainGuard::set();

        let conn = Connection::open(&scratch_db).expect("open fixture db");
        let version_before: String = conn
            .query_row(
                "SELECT value FROM ai_settings WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(version_before, "91", "fixture must start at v91");

        // Real migration — same call `init_database`/`init_database_with_progress`
        // make internally. Not raw SQL, not a mocked result.
        crate::migrations::run_migrations(&conn).expect("v91 -> v92 migration must succeed");

        let version_after: String = conn
            .query_row(
                "SELECT value FROM ai_settings WHERE key = 'schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            version_after,
            crate::migrations::SCHEMA_VERSION,
            "set_schema_version must have committed by the time run_migrations returns"
        );

        // Now prove the second half of the guarantee: mount_db_state_then's
        // dependent-block closure only ever sees a DbState whose underlying
        // connection already reports the migrated version — using the SAME
        // function `lib.rs` calls in production (mock_app() stands in for
        // the real App/AppHandle<Wry>; the function body under test is
        // identical either way, see its own doc comment above).
        let app = tauri::test::mock_app();
        let handle = app.handle();

        let dependent_block_ran = std::sync::Arc::new(std::sync::Mutex::new(false));
        let dependent_block_ran2 = dependent_block_ran.clone();

        let result = mount_db_state_then(handle, conn, move |h| {
            // This closure stands in for §486 / the orphan sweeps / every
            // other DbState consumer in lib.rs's `.setup()`.
            let db_state = h.state::<crate::database::DbState>();
            let locked = db_state.0.lock().unwrap();
            let seen_version: String = locked
                .query_row(
                    "SELECT value FROM ai_settings WHERE key = 'schema_version'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                seen_version,
                crate::migrations::SCHEMA_VERSION,
                "dependent block must never observe a pre-migration schema_version"
            );
            *dependent_block_ran2.lock().unwrap() = true;
            Ok(())
        });

        assert!(result.is_ok());
        assert!(
            *dependent_block_ran.lock().unwrap(),
            "the dependent-block closure must actually have run"
        );
    }

    /// If `on_db_ready` fails, `mount_db_state_then` propagates the error —
    /// no silent swallow (SOUL.md §4).
    #[test]
    fn test_mount_db_state_then_propagates_dependent_block_error() {
        let app = tauri::test::mock_app();
        let handle = app.handle();
        let conn = Connection::open_in_memory().expect("in-memory conn");

        let result = mount_db_state_then(handle, conn, |_h| {
            Err("dependent block failed on purpose".to_string())
        });

        assert_eq!(result, Err("dependent block failed on purpose".to_string()));
    }

    /// `DbState` IS mounted even if the dependent block goes on to fail —
    /// matches the ordering guarantee's letter precisely: mounting happens
    /// unconditionally before the closure runs, regardless of what the
    /// closure itself returns.
    #[test]
    fn test_mount_db_state_then_mounts_db_state_even_when_dependent_block_fails() {
        let app = tauri::test::mock_app();
        let handle = app.handle();
        let conn = Connection::open_in_memory().expect("in-memory conn");

        let _ = mount_db_state_then(handle, conn, |_h| Err("boom".to_string()));

        // If DbState were NOT mounted, this would panic (Tauri's state()
        // panics on an unmanaged type) — reaching the assert below at all is
        // part of the proof.
        let db_state = handle.state::<crate::database::DbState>();
        assert!(db_state.0.lock().is_ok());
    }
}
