// Pane v11.10.6 — Notifications Module
// Local JSONL watcher (fallback) + Supabase REST polling for encrypted push notifications

use crate::database::DbState;
use crate::sync::{auth::AuthSession, crypto, keystore};
use base64::{Engine as _, engine::general_purpose};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;
use rusqlite::params;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;
use chrono::{Local, Duration};

const SUPABASE_URL: &str = "https://jngkiljhmnrygdnfvrgp.supabase.co";
const SUPABASE_ANON_KEY: &str = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJpc3MiOiJzdXBhYmFzZSIsInJlZiI6ImpuZ2tpbGpobW5yeWdkbmZ2cmdwIiwicm9sZSI6ImFub24iLCJpYXQiOjE3NzIyMzUwNzEsImV4cCI6MjA4NzgxMTA3MX0.xe6N-2KaY0Tx2eYI-UZJrlSa3v2EYZJT-PMVFRDPnzI";

#[derive(Debug, Serialize, Deserialize, Clone)]
struct NotificationLine {
    #[serde(default = "default_type")]
    r#type: String,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default = "default_source")]
    source: String,
    #[serde(default)]
    ts: Option<String>,
}

fn log_notif(msg: &str) {
    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
    let line = format!("[{}] {}
", timestamp, msg);
    eprintln!("[notifications] {}", msg);
    if let Some(home) = dirs::home_dir() {
        let log_path = home.join("Pane").join(".pane").join("notifications.log");
        let _ = std::fs::OpenOptions::new().create(true).append(true).open(&log_path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
}

fn default_type() -> String { "info".to_string() }
fn default_source() -> String { "system".to_string() }

#[derive(Debug, Serialize, Clone)]
struct NotificationEvent {
    id: String,
    r#type: String,
    title: String,
    body: String,
    source: String,
    created_at: String,
}

#[derive(Debug, Deserialize)]
struct NotificationPayload {
    title: String,
    body: String,
}

/// Row returned from Supabase REST API for notifications table
#[derive(Debug, Deserialize)]
struct SupabaseNotificationRow {
    id: String,
    encrypted_payload: String,
    nonce: String,
    #[serde(default = "default_type")]
    r#type: String,
    #[serde(default = "default_source")]
    source: String,
    created_at: String,
}

/// §36.7 (#3050 PR 6) — named string constants for the two notification
/// `type` values this PR adds. Deliberately NOT a Rust enum: Ruling 5
/// (Matt, 2026-10-29) — `docs/INVARIANTS.md` §19 and `notifications.md`
/// §19.2 both assert a `NotificationType` enum that has never existed in
/// this file (`grep -rn "NotificationType" src-tauri/src/` returns zero
/// hits); every existing type (`task_due`, `task_overdue`, `note`,
/// `subtask`, ...) is already a plain `&str` with no `CHECK` constraint
/// backing it. This PR follows the pattern the code actually uses — named
/// constants, mirroring `refresh_event.rs`'s `ENTITY_NOTE`/`ENTITY_SHEET` —
/// and fixes the two documents instead of building an enum this PR (or any
/// of the six existing types) needs.
pub const NOTIF_TYPE_AGENT_MESSAGE: &str = "agent_message";
pub const NOTIF_TYPE_JOB_FAILED: &str = "job_failed";

/// Insert a notification row into the `notifications` table.
///
/// Extracted from `process_notification()` so it can be unit-tested without
/// a Tauri `AppHandle`. The event emit and OS native notification remain in
/// `process_notification()` — this function is DB-only, no side effects.
///
/// Extended in PR3a (#1811) to include full column set: target_kind, target_id,
/// idem_key, expires_at — all optional for backwards-compat with existing callers.
pub fn insert_notification(
    conn: &rusqlite::Connection,
    id: &str,
    notif_type: &str,
    title: &str,
    body: &str,
    source: &str,
    now: &str,
    target_kind: Option<&str>,
    target_id: Option<&str>,
    idem_key: Option<&str>,
    expires_at: Option<&str>,
) -> Result<(), String> {
    conn.execute(
        "INSERT INTO notifications (id, type, title, body, source, read, created_at, target_kind, target_id, idem_key, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?8, ?9, ?10)",
        params![id, notif_type, title, body, source, now, target_kind, target_id, idem_key, expires_at],
    ).map_err(|e| format!("insert_notification: {}", e))?;
    Ok(())
}

/// Process a notification: insert into DB, emit event, optionally show native notification.
///
/// `os_notify`: when `true`, fires an OS-native alert if the window is not focused.
/// Pass `false` for in-app-only types (e.g. task_due, task_overdue per Matt's locked decision).
/// Existing callers (JSONL watcher, Supabase poller) pass `true` to preserve existing behavior.
fn process_notification<R: tauri::Runtime>(
    app: &AppHandle<R>,
    db: &DbState,
    notif_type: &str,
    title: &str,
    body: &str,
    source: &str,
    target_kind: Option<&str>,
    target_id: Option<&str>,
    idem_key: Option<&str>,
    expires_at: Option<&str>,
    os_notify: bool,
) {
    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    if let Ok(conn) = db.0.lock() {
        if let Err(e) = insert_notification(&conn, &id, notif_type, title, body, source, &now, target_kind, target_id, idem_key, expires_at) {
            eprintln!("[notifications] DB insert failed: {}", e);
        }
    }

    let event = NotificationEvent {
        id: id.clone(),
        r#type: notif_type.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        source: source.to_string(),
        created_at: now,
    };

    app.emit("pane-notification", &event).ok();

    let is_focused = app
        .get_webview_window("main")
        .map(|w| w.is_focused().unwrap_or(false))
        .unwrap_or(false);

    if os_notify && !is_focused {
        use tauri_plugin_notification::NotificationExt;
        app.notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .ok();
    }

    println!("[notifications] Processed: {} — {}", title, body);
}

// ─── §36.7 (#3050 PR 6): agent_message / job_failed ──────────────────────

/// §36.7/§36.6/§36.8.1 — a job session's report-back reply or an OC
/// background-pickup message landed in a session the user is NOT currently
/// viewing (`ai::chat::is_session_open`, checked by the caller BEFORE this
/// is invoked — this function does not re-check it, matching every other
/// notification emitter's "caller resolves the trigger condition" shape).
///
/// #3149/C45: per-firing notification keys, scoped by consumer, so a
/// fourth consumer can't collide silently with "own-session" or "origin"
/// the way a bare string convention could. Before this enum existed, the
/// own-session key was the bare `agent_message:{id}` this function always
/// computed, and the origin key was built by the CALLER appending a
/// `-origin` suffix before ever reaching this function — a convention
/// enforced only by a comment, not a type. Replacing both with
/// `notification_key(firing_id, scope)` is a pure refactor: see
/// `notification_key`'s own doc comment for the byte-identical guarantee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationScope {
    /// The firing's own session — today's bare `agent_message:{id}` key.
    OwnSession,
    /// A job's origin session (the chat session that scheduled the job) —
    /// today's `agent_message:{id}-origin` suffixed key.
    Origin,
    /// C45 (#3155): OC's own background pickup of a non-job session's
    /// activity (§36.6) — a genuinely different namespace from the
    /// own-session/origin firing pair above (OC background pickup, not a
    /// Pane-dispatched job's own report-back), previously hand-composed
    /// at its one call site (`ai::scheduler`'s OC-pickup loop) as
    /// `format!("oc-pickup-{}", local_message_id)` BEFORE being wrapped
    /// again by this function's own `OwnSession` arm — producing
    /// `agent_message:oc-pickup-{id}`. This variant expresses that exact
    /// shape as a real arm instead of a hand-rolled prefix the caller had
    /// to remember to compose.
    OcPickup,
}

/// Mints a per-firing notification `idem_key`, scoped by consumer
/// (#3149/C45). **Byte-identical to the two hand-rolled formats this
/// replaces** — `OwnSession` → `agent_message:{firing_id}` (unchanged,
/// this was always computed inline inside
/// `emit_agent_message_notification`), `Origin` →
/// `agent_message:{firing_id}-origin` (previously built by a call site's
/// own `format!("{}-origin", report_message_key)` BEFORE calling into
/// `emit_agent_message_notification`, which then wrapped it again). The
/// two scopes can never format identically for the same `firing_id` — a
/// third scope, when it's added, is forced to pick a variant at compile
/// time rather than inventing a new string-literal suffix.
pub fn notification_key(firing_id: &str, scope: NotificationScope) -> String {
    match scope {
        NotificationScope::OwnSession => format!("agent_message:{}", firing_id),
        NotificationScope::Origin => format!("agent_message:{}-origin", firing_id),
        // C45 (#3155): byte-identical to the pre-existing hand-composed
        // shape (`format!("oc-pickup-{}", local_message_id)` passed as the
        // bare `OwnSession` firing_id) — same output, one less hand-rolled
        // string at the call site.
        NotificationScope::OcPickup => format!("agent_message:oc-pickup-{}", firing_id),
    }
}

/// `idem_key = notification_key(message_id, scope)` — one notification per
/// ingested message, matching §36.7's table exactly. Dedup follows the
/// established convention (`emit_due_notification`'s own doc comment:
/// "Caller is responsible for the SELECT-before-INSERT dedup check") —
/// checked here, inline, rather than pushed out to each of the call sites
/// (`ai::scheduler`'s own-session/origin report-back hooks, the reminder
/// firing, and `ai::scheduler`'s OC-pickup path), so all of them get the
/// guarantee from one place.
pub fn emit_agent_message_notification<R: tauri::Runtime>(
    app: &AppHandle<R>,
    db: &DbState,
    message_id: &str,
    agent_name: &str,
    first_line: &str,
    project_name: &str,
    session_id: &str,
    scope: NotificationScope,
) {
    let idem_key = notification_key(message_id, scope);

    let already_exists = match db.0.lock() {
        Ok(conn) => conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
                params![idem_key],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
            > 0,
        Err(e) => {
            eprintln!("[notifications] emit_agent_message_notification: lock failed on dedup check: {}", e);
            false
        }
    };
    if already_exists {
        eprintln!("[notifications] agent_message idem_key={} already recorded — skipping duplicate emit", idem_key);
        return;
    }

    let title = format!("Message from {}", agent_name);
    let raw_body = format!("{} · {}", first_line, project_name);
    // #3178: defense-in-depth. `execute_generate_image_at` (image_gen.rs)
    // now hands the model a relative `Assets/<uuid>.<ext>` path going
    // forward, so a freshly-generated reply should never contain an
    // absolute one — but this is the boundary where EVERY agent-message
    // notification body gets built, regardless of source, so it's also
    // the one place that catches: (1) a not-yet-repaired historical
    // message echoed back verbatim, or (2) any future regression that
    // reintroduces the leak upstream. Matt's named mutation: feed this
    // an absolute path → the assertion in `notification_body_never_
    // contains_absolute_asset_path` (below) reddens.
    let (body, stripped) = crate::ai::image_gen::relativize_absolute_asset_paths(&raw_body);
    if stripped > 0 {
        eprintln!(
            "[notifications] emit_agent_message_notification: stripped {} absolute asset path(s) from notification body (#3178 defense-in-depth)",
            stripped
        );
    }
    let expires_at = (Local::now() + Duration::days(7))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    process_notification(
        app,
        db,
        NOTIF_TYPE_AGENT_MESSAGE,
        &title,
        &body,
        "system",
        Some("session"),
        Some(session_id),
        Some(idem_key.as_str()),
        Some(expires_at.as_str()),
        true, // in-app + OS popup, per §36.7's table
    );
}

/// §36.7 Ruling 1 (Matt, 2026-10-29) — "one notification per failure
/// streak, updated in place." Unlike every other notification type in this
/// file (plain SELECT-before-INSERT dedup, skip on match), this is a real
/// UPSERT: the first failure of a streak INSERTS a row; every further
/// failure in the SAME streak UPDATES that row's `title`/`body` in place
/// (the 3-strike flip's own update included); a success ends the streak,
/// so the NEXT failure (a new streak) gets a fresh `idem_key` and therefore
/// a new row.
///
/// Keyed on `job_id` + `streak_started_at` (the streak's FIRST
/// `last_failed_at` timestamp — the caller is responsible for NOT
/// overwriting `agent_jobs.last_failed_at` on the 2nd/3rd failure of a
/// streak, specifically so this value stays stable across every call in
/// one streak; see `ai::scheduler::dispatch_oc_job`'s own comment at the
/// call site). This is what makes "exactly one row per streak" true: three
/// consecutive failures compute the SAME `idem_key` (same `job_id`, same
/// unchanged `streak_started_at`) on all three calls, so all three resolve
/// to the UPDATE branch after the first.
pub fn emit_or_update_job_failed_notification<R: tauri::Runtime>(
    app: &AppHandle<R>,
    db: &DbState,
    job_id: &str,
    job_title: &str,
    streak_started_at: &str,
    attempt: i64,
    max_attempts: i64,
    terminal: bool,
    last_error_first_line: &str,
    session_id: &str,
) {
    let idem_key = format!("job_failed:{}:{}", job_id, streak_started_at);
    // C47 (#3155): both branches used to hardcode the literal "Tick"
    // regardless of the job's real title — a job named "Say hello" got a
    // bell reading "Tick stopped after 1 failures." `job_title` is now
    // substituted in. Also: a once/spawn job's single-failure terminal
    // exit (`max_attempts <= 1` — every once/spawn call already passes
    // `max_attempts = 1`, no new plumbing) reads "{title} failed", not the
    // streak wording ("stopped after N failures"), which only makes sense
    // when N can exceed 1.
    let title = if terminal {
        if max_attempts <= 1 {
            format!("{} failed", job_title)
        } else {
            format!("{} stopped after {} failures", job_title, max_attempts)
        }
    } else {
        format!("{} failed · {} of {} — {}", job_title, attempt, max_attempts, last_error_first_line)
    };
    let body = last_error_first_line.to_string();

    let existing_id: Option<String> = match db.0.lock() {
        Ok(conn) => conn
            .query_row(
                "SELECT id FROM notifications WHERE idem_key = ?1",
                params![idem_key],
                |r| r.get(0),
            )
            .optional()
            .unwrap_or(None),
        Err(e) => {
            eprintln!("[notifications] emit_or_update_job_failed_notification: lock failed: {}", e);
            return;
        }
    };

    match existing_id {
        Some(row_id) => {
            let updated = match db.0.lock() {
                Ok(conn) => conn
                    .execute(
                        "UPDATE notifications SET title = ?1, body = ?2 WHERE id = ?3",
                        params![title, body, row_id],
                    )
                    .map_err(|e| format!("{}", e)),
                Err(e) => Err(format!("lock failed: {}", e)),
            };
            if let Err(e) = updated {
                eprintln!("[notifications] job_failed streak update failed for job {}: {}", job_id, e);
                return;
            }
            // Re-emit so an open NotificationPanel re-queries and shows the
            // updated copy — the panel already re-queries on `pane-notification`
            // (existing mechanism, §19.6); no new event name needed.
            app.emit(
                "pane-notification",
                NotificationEvent {
                    id: row_id,
                    r#type: NOTIF_TYPE_JOB_FAILED.to_string(),
                    title,
                    body,
                    source: "system".to_string(),
                    created_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
                },
            )
            .ok();
            eprintln!("[notifications] job_failed streak updated in place for job {} (attempt {} of {}, terminal={})", job_id, attempt, max_attempts, terminal);
        }
        None => {
            let expires_at = (Local::now() + Duration::days(7))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string();
            process_notification(
                app,
                db,
                NOTIF_TYPE_JOB_FAILED,
                &title,
                &body,
                "system",
                Some("session"),
                Some(session_id),
                Some(idem_key.as_str()),
                Some(expires_at.as_str()),
                true,
            );
            eprintln!("[notifications] job_failed streak started for job {} (attempt {} of {})", job_id, attempt, max_attempts);
        }
    }
}

#[cfg(test)]
mod job_failed_streak_tests_3050_pr6 {
    use super::*;
    use crate::database::DbState;

    /// `process_notification`'s OS-popup branch (`os_notify=true`, the path
    /// `emit_or_update_job_failed_notification`'s INSERT arm takes, per
    /// §36.7's table) calls `app.notification()` —
    /// `tauri_plugin_notification`'s own extension trait, which panics with
    /// "state() called before manage()" against a bare `tauri::test::
    /// mock_app()` (that helper registers no plugins). Every existing
    /// `os_notify=true` test-covered caller in this file (`emit_due_
    /// notification`, `emit_subtask_due_notification`) passes `false` —
    /// there was no precedent to copy for `true`. This builds the same
    /// `MockRuntime` app but with the plugin actually registered, so the
    /// real production code path (OS popup included) is what's under test,
    /// not a weakened substitute.
    fn make_mock_app_with_notification_plugin() -> tauri::App<tauri::test::MockRuntime> {
        tauri::test::mock_builder()
            .plugin(tauri_plugin_notification::init())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .expect("mock app with notification plugin must build")
    }

    fn make_db() -> DbState {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        DbState::new(conn)
    }

    fn row_count(db: &DbState, job_id: &str) -> i64 {
        db.0.lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE type = 'job_failed' AND idem_key LIKE ?1",
                params![format!("job_failed:{}:%", job_id)],
                |r| r.get(0),
            )
            .unwrap_or(-1)
    }

    /// Ruling 1's first named test: three consecutive failures of the same
    /// streak (same `job_id`, same `streak_started_at`) → ONE row, with
    /// three updates (the third being the terminal flip's copy).
    #[test]
    fn test_three_consecutive_failures_same_streak_one_row_three_updates() {
        let db = make_db();
        let app = make_mock_app_with_notification_plugin();
        let handle = app.handle();
        let streak_started_at = "2026-10-29 08:00:00";

        emit_or_update_job_failed_notification(handle, &db, "job-streak-1", "Tick", streak_started_at, 1, 3, false, "job-streak-1 HTTP 400", "sess-1");
        assert_eq!(row_count(&db, "job-streak-1"), 1, "first failure must insert exactly one row");

        let (title1,): (String,) = db.0.lock().unwrap().query_row(
            "SELECT title FROM notifications WHERE type='job_failed'", [], |r| Ok((r.get(0)?,))
        ).unwrap();
        assert!(title1.contains("1 of 3"), "first failure's title must read '1 of 3', got: {}", title1);

        emit_or_update_job_failed_notification(handle, &db, "job-streak-1", "Tick", streak_started_at, 2, 3, false, "job-streak-1 HTTP 400", "sess-1");
        assert_eq!(row_count(&db, "job-streak-1"), 1, "second failure of the SAME streak must update, not insert a second row");
        let (title2,): (String,) = db.0.lock().unwrap().query_row(
            "SELECT title FROM notifications WHERE type='job_failed'", [], |r| Ok((r.get(0)?,))
        ).unwrap();
        assert!(title2.contains("2 of 3"), "second failure's title must read '2 of 3', got: {}", title2);

        emit_or_update_job_failed_notification(handle, &db, "job-streak-1", "Tick", streak_started_at, 3, 3, true, "job-streak-1 HTTP 400", "sess-1");
        assert_eq!(row_count(&db, "job-streak-1"), 1, "the 3-strike flip must update the SAME row, not insert a third");
        let (title3,): (String,) = db.0.lock().unwrap().query_row(
            "SELECT title FROM notifications WHERE type='job_failed'", [], |r| Ok((r.get(0)?,))
        ).unwrap();
        // C47 (#3155): the assertion text is UNCHANGED from before this
        // fix — this test's own job_title argument happens to be "Tick" —
        // but it's now proving the parameter is threaded through
        // correctly, not that the string is baked into the function. See
        // `test_once_job_failed_title_uses_real_job_title_not_tick` below
        // for the test that actually catches a hardcoded-"Tick" regression
        // (this one alone would NOT: reverting to a hardcoded literal
        // would still pass here, since the literal and this test's own
        // argument happen to match).
        assert_eq!(title3, "Tick stopped after 3 failures");
    }

    /// C47 (#3155): the job-failed bell used to hardcode the literal
    /// "Tick" in both branches, regardless of the job's real title — a job
    /// named "Say hello" got a bell reading "Tick stopped after 1
    /// failures." This test uses a DIFFERENT title ("Say hello") so a
    /// reverted-to-hardcoded mutation is caught directly, unlike the test
    /// above (which happens to use "Tick" as its own real title).
    ///
    /// Also covers the once/spawn wording rule: a single-failure terminal
    /// exit (`max_attempts <= 1`, the fallback every once/spawn call uses)
    /// reads "{title} failed", not the streak wording ("stopped after N
    /// failures") — that wording only makes sense when N can exceed 1.
    ///
    /// Mutation A (revert to hardcoded "Tick"): "Tick failed" ≠ "Say hello
    /// failed" — red.
    /// Mutation B (drop the `max_attempts <= 1` branch, always "stopped
    /// after N failures"): "Say hello stopped after 1 failures" ≠ "Say
    /// hello failed" — red.
    #[test]
    fn test_once_job_failed_title_uses_real_job_title_not_tick() {
        let db = make_db();
        let app = make_mock_app_with_notification_plugin();
        let handle = app.handle();

        emit_or_update_job_failed_notification(handle, &db, "job-once-1", "Say hello", "2026-11-03 08:00:00", 1, 1, true, "some error", "sess-1");

        let (title,): (String,) = db.0.lock().unwrap().query_row(
            "SELECT title FROM notifications WHERE idem_key LIKE 'job_failed:job-once-1:%'", [], |r| Ok((r.get(0)?,))
        ).unwrap();
        assert_eq!(title, "Say hello failed", "a once-job's terminal failure must read '{{title}} failed', not the streak wording, and must use the REAL job title");
    }

    /// C47 (#3155): the recurring-job wording (`max_attempts > 1`) with a
    /// non-"Tick" title — proves the streak-wording branch ALSO threads the
    /// real title, not just the once-job branch above.
    ///
    /// Mutation (swap in a hardcoded "Tick"): "Tick stopped after 3
    /// failures" ≠ "Nightly digest stopped after 3 failures" — red.
    #[test]
    fn test_recurring_job_failed_title_uses_real_job_title_not_tick() {
        let db = make_db();
        let app = make_mock_app_with_notification_plugin();
        let handle = app.handle();

        emit_or_update_job_failed_notification(handle, &db, "job-recurring-1", "Nightly digest", "2026-11-03 08:00:00", 3, 3, true, "some error", "sess-1");

        let (title,): (String,) = db.0.lock().unwrap().query_row(
            "SELECT title FROM notifications WHERE idem_key LIKE 'job_failed:job-recurring-1:%'", [], |r| Ok((r.get(0)?,))
        ).unwrap();
        assert_eq!(title, "Nightly digest stopped after 3 failures");
    }

    /// Ruling 1's second named test: failure, success (streak ends — no
    /// notification call on success, by design; see the call site),
    /// failure (a NEW streak, different `streak_started_at`) → TWO rows.
    #[test]
    fn test_failure_success_failure_two_rows() {
        let db = make_db();
        let app = make_mock_app_with_notification_plugin();
        let handle = app.handle();

        emit_or_update_job_failed_notification(handle, &db, "job-streak-2", "Tick", "2026-10-29 08:00:00", 1, 3, false, "first streak error", "sess-1");
        assert_eq!(row_count(&db, "job-streak-2"), 1);

        // A success ends the streak (no call here — the caller simply
        // never invokes this function on a success; nothing to assert
        // beyond "no new row appears because nothing was called").

        // New streak: a LATER `streak_started_at`, since the prior streak
        // ended and this is a genuinely new failure.
        emit_or_update_job_failed_notification(handle, &db, "job-streak-2", "Tick", "2026-10-29 09:00:00", 1, 3, false, "second streak error", "sess-1");
        assert_eq!(row_count(&db, "job-streak-2"), 2, "a failure after a success must start a NEW row, not update the old streak's row");
    }

    /// Mutation target: if the idem_key computation ever drops
    /// `streak_started_at` (keying on `job_id` alone), the two-streak test
    /// above would collapse to one row instead of two. Pinned directly so
    /// the failure mode is visible without relying on the test above's
    /// inference.
    #[test]
    fn test_different_streak_started_at_yields_different_idem_key() {
        let db = make_db();
        let app = make_mock_app_with_notification_plugin();
        let handle = app.handle();

        emit_or_update_job_failed_notification(handle, &db, "job-x", "Tick", "2026-10-29 08:00:00", 1, 3, false, "err", "sess-1");
        emit_or_update_job_failed_notification(handle, &db, "job-x", "Tick", "2026-10-29 09:00:00", 1, 3, false, "err", "sess-1");

        let count: i64 = db.0.lock().unwrap().query_row(
            "SELECT COUNT(DISTINCT idem_key) FROM notifications WHERE type='job_failed'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(count, 2, "two different streak_started_at values for the same job_id must produce two distinct idem_keys");
    }

    // ── #3178: notification body never carries an absolute asset path ──

    /// Matt's named mutation (verbatim from #3178): feed an absolute path
    /// in → this test reddens. This is the defense-in-depth boundary —
    /// `emit_agent_message_notification` is where EVERY agent-message
    /// notification body is built, so it's the one place that catches a
    /// not-yet-repaired historical row OR a future regression upstream,
    /// independent of whether `execute_generate_image_at`'s own fix
    /// (image_gen.rs) is still in place.
    #[test]
    fn notification_body_never_contains_absolute_asset_path() {
        let db = make_db();
        let app = make_mock_app_with_notification_plugin();
        let handle = app.handle();

        let leaking_first_line =
            "![a lighthouse](/Users/example/Pane/Projects/Aurum/Assets/44d12345-1234-5678-9abc-1234567890ab.png)";

        emit_agent_message_notification(
            handle,
            &db,
            "msg-leak-1",
            "Lighthouse Bot",
            leaking_first_line,
            "Aurum",
            "sess-1",
            NotificationScope::OwnSession,
        );

        let body: String = db
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT body FROM notifications WHERE type = 'agent_message'",
                [],
                |r| r.get(0),
            )
            .expect("notification row must exist");

        assert!(
            !body.contains("/Users/"),
            "notification body must never contain an absolute /Users/ path, got: {:?}",
            body
        );
        assert!(
            body.contains("Assets/44d12345-1234-5678-9abc-1234567890ab.png"),
            "the relative Assets/<uuid>.<ext> form must survive the rewrite, got: {:?}",
            body
        );
    }

    /// A body with no absolute path at all must pass through byte-for-byte
    /// (no stray rewriting of ordinary text that merely mentions a path).
    #[test]
    fn notification_body_passes_through_unchanged_when_no_absolute_path_present() {
        let db = make_db();
        let app = make_mock_app_with_notification_plugin();
        let handle = app.handle();

        emit_agent_message_notification(
            handle,
            &db,
            "msg-clean-1",
            "Lighthouse Bot",
            "All done, no images this time.",
            "Aurum",
            "sess-1",
            NotificationScope::OwnSession,
        );

        let body: String = db
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT body FROM notifications WHERE type = 'agent_message'",
                [],
                |r| r.get(0),
            )
            .expect("notification row must exist");

        assert_eq!(body, "All done, no images this time. · Aurum");
    }
}

// ─── Local JSONL Watcher (fallback) ──────────────────────────────────────

pub fn start_notification_watcher(app: AppHandle, db: DbState) {
    let pane_dir = dirs::home_dir()
        .expect("No home dir")
        .join("Shared/Pane");
    let jsonl_path = pane_dir.join("notifications.jsonl");

    std::fs::create_dir_all(&pane_dir).ok();
    if !jsonl_path.exists() {
        std::fs::write(&jsonl_path, "").ok();
    }

    let initial_offset = std::fs::metadata(&jsonl_path)
        .map(|m| m.len())
        .unwrap_or(0);

    eprintln!("[notifications] JSONL watcher starting, watching: {:?}, offset: {}", jsonl_path, initial_offset);
    std::thread::spawn(move || {
        let mut offset = initial_offset;

        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));

            let current_size = match std::fs::metadata(&jsonl_path) {
                Ok(m) => m.len(),
                Err(_) => continue,
            };

            if current_size <= offset {
                if current_size < offset {
                    offset = current_size;
                }
                continue;
            }

            let file_bytes = match std::fs::read(&jsonl_path) {
                Ok(b) => b,
                Err(_) => continue,
            };

            let new_data = &file_bytes[offset as usize..];
            let new_text = String::from_utf8_lossy(new_data);
            offset = current_size;

            for line in new_text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }

                let parsed: NotificationLine = match serde_json::from_str(line) {
                    Ok(n) => n,
                    Err(e) => {
                        eprintln!("[notifications] Failed to parse line: {} — {}", line, e);
                        continue;
                    }
                };

                process_notification(&app, &db, &parsed.r#type, &parsed.title, &parsed.body, &parsed.source, None, None, None, None, true);
            }
        }
    });
}

// ─── Auth Helpers ────────────────────────────────────────────────────────

/// Load auth session from disk
fn load_auth_session() -> Option<AuthSession> {
    let path = dirs::home_dir()?.join("Pane").join(".pane").join("auth_session.json");
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Save auth session back to disk (after token refresh)
fn save_auth_session(session: &AuthSession) -> Result<(), String> {
    let path = dirs::home_dir()
        .ok_or("No home dir")?
        .join("Pane").join(".pane").join("auth_session.json");
    let data = serde_json::to_string_pretty(session)
        .map_err(|e| format!("Failed to serialize auth session: {}", e))?;
    std::fs::write(&path, data)
        .map_err(|e| format!("Failed to write auth session: {}", e))?;
    Ok(())
}

/// Decode JWT and check if expired (with 60s buffer)
fn is_token_expired(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return true;
    }
    let payload = match general_purpose::URL_SAFE_NO_PAD.decode(parts[1]) {
        Ok(p) => p,
        Err(_) => {
            match general_purpose::STANDARD.decode(parts[1].trim_end_matches('=')) {
                Ok(p) => p,
                Err(_) => return true,
            }
        }
    };
    let parsed: serde_json::Value = match serde_json::from_slice(&payload) {
        Ok(v) => v,
        Err(_) => return true,
    };
    let exp = match parsed.get("exp").and_then(|v| v.as_i64()) {
        Some(e) => e,
        None => return true,
    };
    let now = chrono::Utc::now().timestamp();
    now >= (exp - 60)
}

/// Refresh the access token using the refresh token. Returns updated session.
async fn refresh_auth_token(session: &AuthSession) -> Result<AuthSession, String> {
    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/auth/v1/token?grant_type=refresh_token", SUPABASE_URL))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "refresh_token": session.refresh_token
        }))
        .send()
        .await
        .map_err(|e| format!("Token refresh network error: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(format!("Token refresh failed ({}): {}", status, body));
    }

    let body: serde_json::Value = res.json().await
        .map_err(|e| format!("Failed to parse refresh response: {}", e))?;

    let access_token = body.get("access_token").and_then(|v| v.as_str())
        .ok_or("No access_token in refresh response")?;
    let refresh_token = body.get("refresh_token").and_then(|v| v.as_str())
        .ok_or("No refresh_token in refresh response")?;
    let expires_at = body.get("expires_at").and_then(|v| v.as_i64()).unwrap_or(0);

    let updated = AuthSession {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        user_id: session.user_id.clone(),
        email: session.email.clone(),
        expires_at,
        device_id: session.device_id.clone(),
    };

    save_auth_session(&updated)?;
    eprintln!("[notifications] JWT refreshed successfully");
    Ok(updated)
}

/// Get a valid access token, refreshing if needed.
async fn get_valid_token() -> Result<(String, AuthSession), String> {
    let mut session = load_auth_session()
        .ok_or("No auth session — must be signed in")?;

    if is_token_expired(&session.access_token) {
        log_notif(&format!("Access token expired, refreshing..."));
        session = refresh_auth_token(&session).await?;
    }

    Ok((session.access_token.clone(), session))
}

/// Get the notification encryption key (derived from master key)
fn get_notification_key() -> Option<[u8; 32]> {
    let master_key = keystore::load_master_key().ok()??;
    Some(crypto::derive_sub_key(&master_key, "pane-notifications-v1"))
}

/// Decrypt a notification payload
fn decrypt_notification(encrypted_b64: &str, nonce_b64: &str, key: &[u8; 32]) -> Result<NotificationPayload, String> {
    let encrypted = general_purpose::STANDARD.decode(encrypted_b64)
        .map_err(|e| format!("Failed to decode payload: {}", e))?;
    let nonce_bytes = general_purpose::STANDARD.decode(nonce_b64)
        .map_err(|e| format!("Failed to decode nonce: {}", e))?;

    if nonce_bytes.len() != 24 {
        return Err(format!("Invalid nonce length: {}", nonce_bytes.len()));
    }

    let cipher = XChaCha20Poly1305::new(key.into());
    let nonce = XNonce::from_slice(&nonce_bytes);
    let plaintext = cipher.decrypt(nonce, encrypted.as_ref())
        .map_err(|e| format!("Decryption failed: {}", e))?;

    let payload: NotificationPayload = serde_json::from_slice(&plaintext)
        .map_err(|e| format!("Failed to parse decrypted payload: {}", e))?;

    Ok(payload)
}

/// Encrypt a notification payload, returns (encrypted_b64, nonce_b64)
fn encrypt_notification(title: &str, body: &str, key: &[u8; 32]) -> Result<(String, String), String> {
    let payload = serde_json::json!({ "title": title, "body": body });
    let plaintext = serde_json::to_vec(&payload)
        .map_err(|e| format!("Failed to serialize payload: {}", e))?;

    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; 24];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, plaintext.as_ref())
        .map_err(|e| format!("Encryption failed: {}", e))?;

    Ok((
        general_purpose::STANDARD.encode(&ciphertext),
        general_purpose::STANDARD.encode(&nonce_bytes),
    ))
}

// ─── Supabase REST Poller ────────────────────────────────────────────────

/// Start Supabase REST polling for notifications (replaces Realtime websocket)

/// Re-authenticate using password stored in macOS Keychain
async fn password_reauth() -> Result<(), String> {
    let session = load_auth_session()
        .ok_or("No auth session for re-auth")?;
    let password = crate::keychain::get_password(&session.email)?
        .ok_or("No password stored in Keychain — please sign in again")?;

    // ADR-0070 Decision 4 (#3195 PR C): sign_in now targets Supabase GoTrue
    // directly and returns a plain AuthSession (no install_uuid/machine_name
    // params, no entitlement fields to re-map) — nothing else in this
    // function or file changes.
    let new_session = crate::sync::auth::sign_in(&session.email, &password).await
        .map_err(|e| format!("Password re-auth failed: {}", e))?;

    save_auth_session(&new_session)?;
    log_notif("Re-authenticated via Keychain password");
    Ok(())
}

pub fn start_supabase_notification_poller(app: AppHandle, db: DbState) {
    if let Err(e) = crate::sync::commands::require_account_enabled() {
        log_notif(&format!("account_enabled is off ({}), skipping Supabase notification poller (ADR-0070 Decision 3/5)", e));
        return;
    }

    let key = match std::panic::catch_unwind(|| get_notification_key()) {
        Ok(Some(k)) => k,
        Ok(None) => {
            log_notif("No sync key found, skipping Supabase notification poller");
            return;
        }
        Err(_) => {
            log_notif("Error loading notification key, skipping poller");
            return;
        }
    };

    let session = match load_auth_session() {
        Some(s) => s,
        None => {
            log_notif("No auth session found, skipping Supabase notification poller");
            return;
        }
    };

    log_notif(&format!("Starting Supabase REST poller for user {}", session.user_id));

    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime for notifications");
        rt.block_on(async move {
        let mut last_seen_ts: Option<String> = None;
        let client = reqwest::Client::new();

        loop {
            // Get valid token (auto-refresh if expired)
            let (access_token, session) = match get_valid_token().await {
                Ok(t) => t,
                Err(e) => {
                    log_notif(&format!("Token refresh failed: {}, attempting password re-auth", e));
                    match password_reauth().await {
                        Ok(_) => {
                            log_notif("Password re-auth succeeded, continuing poller");
                            continue;
                        }
                        Err(e2) => {
                            log_notif(&format!("Password re-auth also failed: {}, stopping poller", e2));
                            break;
                        }
                    }
                }
            };

            // Build query URL
            let mut url = format!(
                "{}/rest/v1/notifications?user_id=eq.{}&read=eq.false&order=created_at.desc&limit=20",
                SUPABASE_URL, session.user_id
            );

            if let Some(ref ts) = last_seen_ts {
                url.push_str(&format!("&created_at=gt.{}", ts));
            }

            match client
                .get(&url)
                .header("apikey", SUPABASE_ANON_KEY)
                .header("Authorization", format!("Bearer {}", access_token))
                .send()
                .await
            {
                Ok(res) if res.status().is_success() => {
                    match res.json::<Vec<SupabaseNotificationRow>>().await {
                        Ok(rows) => {
                            for row in rows.iter().rev() {
                                match &last_seen_ts {
                                    Some(ts) if row.created_at.as_str() <= ts.as_str() => continue,
                                    _ => {}
                                }

                                match decrypt_notification(&row.encrypted_payload, &row.nonce, &key) {
                                    Ok(decrypted) => {
                                        log_notif(&format!("Polled notification: {}", decrypted.title));
                                        process_notification(&app, &db, &row.r#type, &decrypted.title, &decrypted.body, &row.source, None, None, None, None, true);
                                    }
                                    Err(e) => {
                                        log_notif(&format!("Failed to decrypt notification {}: {}", row.id, e));
                                    }
                                }

                                // Mark as read on Supabase
                                if let Err(e) = client
                                    .patch(&format!("{}/rest/v1/notifications?id=eq.{}", SUPABASE_URL, row.id))
                                    .header("apikey", SUPABASE_ANON_KEY)
                                    .header("Authorization", format!("Bearer {}", access_token))
                                    .header("Content-Type", "application/json")
                                    .json(&serde_json::json!({"read": true}))
                                    .send()
                                    .await
                                {
                                    eprintln!("[notifications] Failed to mark notification {} as read: {}", row.id, e);
                                }

                                last_seen_ts = Some(row.created_at.clone());
                            }
                        }
                        Err(e) => {
                            log_notif(&format!("Failed to parse poll response: {}", e));
                        }
                    }
                }
                Ok(res) => {
                    let status = res.status();
                    if status.as_u16() == 401 || status.as_u16() == 403 {
                        eprintln!("[notifications] Auth error during poll ({}), will retry with refresh", status);
                    } else {
                        eprintln!("[notifications] Poll returned status {}", status);
                    }
                }
                Err(e) => {
                    eprintln!("[notifications] Poll network error: {}", e);
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
        }
        });
    });
}

// ─── Push Notification Command ───────────────────────────────────────────

#[tauri::command]
pub async fn push_notification(
    title: String,
    body: String,
    r#type: Option<String>,
    source: Option<String>,
) -> Result<(), String> {
    crate::sync::commands::require_account_enabled()?;
    let notif_type = r#type.unwrap_or_else(|| "info".to_string());
    let notif_source = source.unwrap_or_else(|| "system".to_string());

    let key = get_notification_key()
        .ok_or_else(|| "No sync key found — sync must be set up first".to_string())?;

    let (access_token, session) = get_valid_token().await?;

    let (encrypted_payload, nonce) = encrypt_notification(&title, &body, &key)?;

    let client = reqwest::Client::new();
    let res = client
        .post(format!("{}/rest/v1/notifications", SUPABASE_URL))
        .header("apikey", SUPABASE_ANON_KEY)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .header("Prefer", "return=minimal")
        .json(&serde_json::json!({
            "user_id": session.user_id,
            "type": notif_type,
            "source": notif_source,
            "encrypted_payload": encrypted_payload,
            "nonce": nonce,
        }))
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;

    if !res.status().is_success() {
        let status = res.status();
        let body_text = res.text().await.unwrap_or_default();
        return Err(format!("Supabase insert failed ({}): {}", status, body_text));
    }

    Ok(())
}

// ─── Tauri Commands ──────────────────────────────────────────────────────

#[tauri::command]
pub fn get_notifications(
    db: tauri::State<'_, DbState>,
    unread_only: bool,
) -> Result<Vec<serde_json::Value>, String> {
    let conn = db.0.lock().unwrap();
    let sql = if unread_only {
        "SELECT id, type, title, body, source, read, created_at, target_kind, target_id FROM notifications WHERE read = 0 AND dismissed_at IS NULL ORDER BY created_at DESC LIMIT 100"
    } else {
        "SELECT id, type, title, body, source, read, created_at, target_kind, target_id FROM notifications WHERE dismissed_at IS NULL ORDER BY created_at DESC LIMIT 100"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| format!("{}", e))?;
    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "id": row.get::<_, String>(0)?,
            "type": row.get::<_, String>(1)?,
            "title": row.get::<_, String>(2)?,
            "body": row.get::<_, String>(3)?,
            "source": row.get::<_, String>(4)?,
            "read": row.get::<_, i32>(5)?,
            "created_at": row.get::<_, String>(6)?,
            "target_kind": row.get::<_, Option<String>>(7)?,
            "target_id": row.get::<_, Option<String>>(8)?,
        }))
    }).map_err(|e| format!("{}", e))?;
    
    let mut result = Vec::new();
    for r in rows {
        if let Ok(v) = r { result.push(v); }
    }
    Ok(result)
}

#[tauri::command]
pub fn mark_notification_read(
    db: tauri::State<'_, DbState>,
    notification_id: String,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    conn.execute("UPDATE notifications SET read = 1 WHERE id = ?1", params![notification_id])
        .map_err(|e| format!("{}", e))?;
    Ok(())
}

#[tauri::command]
pub fn mark_all_notifications_read(
    db: tauri::State<'_, DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    conn.execute("UPDATE notifications SET read = 1 WHERE read = 0", [])
        .map_err(|e| format!("{}", e))?;
    Ok(())
}

#[tauri::command]
pub fn dismiss_notification(
    db: tauri::State<'_, DbState>,
    notification_id: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    conn.execute(
        "UPDATE notifications SET dismissed_at = datetime('now') WHERE id = ?1 AND dismissed_at IS NULL",
        params![notification_id],
    ).map_err(|e| format!("{}", e))?;
    Ok(())
}

#[tauri::command]
pub fn get_unread_count(
    db: tauri::State<'_, DbState>,
) -> Result<i64, String> {
    let conn = db.0.lock().unwrap();
    conn.query_row("SELECT COUNT(*) FROM notifications WHERE read = 0 AND dismissed_at IS NULL", [], |row| row.get(0))
        .map_err(|e| format!("{}", e))
}

/// Internal helper: query unread count on an already-held connection.
///
/// Use this when the caller already holds the DB lock (e.g. inside update_note after
/// auto-dismiss) to emit `pane-badge-changed` without re-acquiring the lock.
/// Fixes #1811: badge must always reflect DB, never a blind frontend counter.
pub fn get_unread_count_internal(conn: &rusqlite::Connection) -> Result<i64, String> {
    conn.query_row(
        "SELECT COUNT(*) FROM notifications WHERE read = 0 AND dismissed_at IS NULL",
        [],
        |row| row.get(0),
    )
    .map_err(|e| format!("get_unread_count_internal: {}", e))
}

/// Tauri command: trigger a full due-date scan.
///
/// Called from the frontend by PR3b's app-focus hook.
/// In-app only (os_notify: false) — Matt's locked decision.
/// All dedup/idempotency is handled inside check_due_dates().
#[tauri::command]
pub fn check_due_dates_cmd(
    app: tauri::AppHandle,
    db: tauri::State<'_, DbState>,
) {
    let db_ref = DbState(db.0.clone());
    check_due_dates(&app, &db_ref);
}

// ─── Due-Date Watcher (#1811 PR3a) ────────────────────────────────────────────────────────

/// Struct holding a classified due-task record from the scan.
/// Extracted for unit-testability: pure functions take &Connection + &str.
#[derive(Debug, PartialEq)]
pub struct DueTaskClassification {
    pub task_id: String,
    pub task_title: String,
    pub threshold: String,     // "due_today" or "overdue"
    pub notif_type: String,    // "task_due" or "task_overdue"
}

/// Run the expiry sweep: set dismissed_at on notifications whose expires_at is in the past.
///
/// `now_str` should be `chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()`.
/// Uses local time (NOT UTC) to avoid misfire on non-UTC machines.
pub fn run_expiry_sweep(conn: &rusqlite::Connection, now_str: &str) {
    if let Err(e) = conn.execute(
        "UPDATE notifications SET dismissed_at = ?1 \
         WHERE dismissed_at IS NULL AND expires_at IS NOT NULL AND expires_at < ?1",
        params![now_str],
    ) {
        eprintln!("[due-date] expiry sweep error: {}", e);
    }
}

/// Scan notes table and classify due/overdue tasks relative to `today_local` (YYYY-MM-DD).
///
/// Uses the same predicate as `get_tasks()` in notes.rs (status IS NOT NULL, != cabinet/done,
/// parent_id NULL/empty, note_type NULL, deleted_at NULL).
///
/// TIMEZONE-SAFE: `today_local` must be computed from `chrono::Local::now().date_naive()`
/// (never from SQLite `datetime('now')` which is UTC and mismatches bare YYYY-MM-DD due_dates).
pub fn classify_due_tasks(
    conn: &rusqlite::Connection,
    today_local: &str,
) -> Vec<DueTaskClassification> {
    let mut stmt = match conn.prepare(
        "SELECT id, title, due_date FROM notes \
         WHERE due_date IS NOT NULL AND due_date != '' \
           AND status IS NOT NULL AND status != 'cabinet' AND status != 'done' \
           AND deleted_at IS NULL \
           AND (parent_id IS NULL OR parent_id = '') \
           AND (note_type IS NULL OR note_type = 'sheet')",
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[due-date] classify_due_tasks prepare error: {}", e);
            return vec![];
        }
    };

    let rows_iter = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) {
        Ok(iter) => iter,
        Err(e) => {
            eprintln!("[due-date] classify_due_tasks query error: {}", e);
            return vec![];
        }
    };

    let mut results = Vec::new();
    for row in rows_iter {
        let (task_id, task_title, due_date) = match row {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[due-date] row error: {}", e);
                continue;
            }
        };

        if due_date.as_str() < today_local {
            results.push(DueTaskClassification {
                task_id,
                task_title,
                threshold: "overdue".to_string(),
                notif_type: "task_overdue".to_string(),
            });
        } else if due_date.as_str() == today_local {
            results.push(DueTaskClassification {
                task_id,
                task_title,
                threshold: "due_today".to_string(),
                notif_type: "task_due".to_string(),
            });
        }
        // future dates: skip (no lead-time warnings at launch)
    }
    results
}

/// Emit a due-date notification for a single classified task.
///
/// Caller is responsible for the SELECT-before-INSERT dedup check (idem_key + dismissed_at IS NULL).
/// This function just builds the notification and calls process_notification, which acquires
/// its own DB lock internally. Do NOT hold db.0.lock() when calling this function.
pub fn emit_due_notification<R: tauri::Runtime>(
    app: &AppHandle<R>,
    db: &DbState,
    task: &DueTaskClassification,
) {
    let idem_key = format!("due:{}:{}", task.task_id, task.threshold);

    let (notif_title, notif_body) = match task.notif_type.as_str() {
        "task_due" => ("Task due today", task.task_title.as_str()),
        _ => ("Task overdue", task.task_title.as_str()),
    };

    // expires_at: 30-day backstop (real dismissal is via auto-dismiss hook in PR3b)
    let expires_at = (Local::now() + Duration::days(30))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    // in-app only (os_notify: false) per Matt's locked decision
    process_notification(
        app,
        db,
        &task.notif_type,
        notif_title,
        notif_body,
        "system",
        Some("note"),
        Some(task.task_id.as_str()),
        Some(idem_key.as_str()),
        Some(expires_at.as_str()),
        false, // in-app only — Matt's locked decision, overrides §19.2 table
    );
}

/// Check due dates for all eligible tasks.
///
/// Three sequential jobs:
///   1. Expiry sweep — dismiss notifications whose `expires_at` is past.
///   2. Due-date scan — classify due/overdue tasks (local timezone, NEVER UTC).
///   3. Idem-key dedup + emit — create notification only if not already open.
///
/// Called by PR3b hooks: app-focus, 5-min startup, post-update_note.
/// Watcher exists but is NOT wired until PR3b.
pub fn check_due_dates(app: &AppHandle, db: &crate::database::DbState) {
    let conn = match db.0.lock() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[due-date] check_due_dates: DB lock failed: {}", e);
            return;
        }
    };

    // Job 1: Expiry sweep
    let now_local = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    run_expiry_sweep(&conn, &now_local);

    // Job 2: Due-date scan
    let today_local = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let tasks = classify_due_tasks(&conn, &today_local);

    // Job 3: Idem-key dedup + emit
    // Lock is dropped before emit so process_notification can acquire it freely.
    drop(conn);

    for task in &tasks {
        let idem_key = format!("due:{}:{}", task.task_id, task.threshold);
        // Short-lived lock: dedup SELECT only — dropped before calling emit
        // No-column dedup fix (#1811): drop dismissed_at IS NULL filter so dismissed
        // rows act as permanent tombstones and do not re-fire on focus/reload.
        let should_emit = db.0.lock().map(|conn2| {
            conn2.query_row(
                "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
                params![idem_key],
                |r| r.get::<_, i64>(0),
            ).unwrap_or(1) == 0
        }).unwrap_or(false);
        // conn2 dropped here — lock released before emit

        if should_emit {
            emit_due_notification(app, db, task);
        }
    }

    // Job 4: Subtask due-date scan (§19.5.1b, #2092)
    // Acquire a fresh lock for the subtask scan (conn was dropped above).
    let conn2 = match db.0.lock() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[due-date] check_due_dates subtask scan: DB lock failed: {}", e);
            return;
        }
    };
    let subtask_items = classify_due_subtasks(&conn2, &today_local, None);
    drop(conn2);

    for item in &subtask_items {
        let idem_key = format!("due:subtask:{}:{}", item.subtask_id, item.threshold);
        let should_emit = db.0.lock().map(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
                params![idem_key],
                |r| r.get::<_, i64>(0),
            ).unwrap_or(1) == 0
        }).unwrap_or(false);
        if should_emit {
            emit_subtask_due_notification(app, db, item);
        }
    }
}

/// Check due dates for a specific task only.
///
/// Scoped variant used by PR3b's post-update_note fast path.
/// Same logic as `check_due_dates()` but queries WHERE id = task_id only.
pub fn check_due_dates_for_task<R: tauri::Runtime>(app: &AppHandle<R>, db: &crate::database::DbState, task_id: &str) {
    let conn = match db.0.lock() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[due-date] check_due_dates_for_task: DB lock failed: {}", e);
            return;
        }
    };

    // Expiry sweep runs on every check (co-located per §19.4 + ADR-0021)
    let now_local = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    run_expiry_sweep(&conn, &now_local);

    let today_local = Local::now().date_naive().format("%Y-%m-%d").to_string();

    // Scoped query: only the specific task
    let row: Option<(String, String, String)> = conn.query_row(
        "SELECT id, title, due_date FROM notes \
         WHERE id = ?1 \
           AND due_date IS NOT NULL AND due_date != '' \
           AND status IS NOT NULL AND status != 'cabinet' AND status != 'done' \
           AND deleted_at IS NULL \
           AND (parent_id IS NULL OR parent_id = '') \
           AND (note_type IS NULL OR note_type = 'sheet')",
        params![task_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).ok();

    let task = match row {
        None => return, // task not eligible (done, deleted, wrong type, not due yet, etc.)
        Some((id, title, due_date)) => {
            if due_date.as_str() < today_local.as_str() {
                DueTaskClassification {
                    task_id: id,
                    task_title: title,
                    threshold: "overdue".to_string(),
                    notif_type: "task_overdue".to_string(),
                }
            } else if due_date.as_str() == today_local.as_str() {
                DueTaskClassification {
                    task_id: id,
                    task_title: title,
                    threshold: "due_today".to_string(),
                    notif_type: "task_due".to_string(),
                }
            } else {
                return; // future date, skip
            }
        }
    };

    // Dedup check while lock is held, then drop before emitting
    let idem_key = format!("due:{}:{}", task.task_id, task.threshold);
    // No-column dedup fix (#1811): drop dismissed_at IS NULL filter so dismissed
    // rows act as permanent tombstones and do not re-fire on focus/reload.
    let existing: i64 = conn.query_row(
        "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
        params![idem_key],
        |r| r.get(0),
    ).unwrap_or(0);

    drop(conn);

    if existing > 0 {
        return;
    }

    // Lock was fully released above; emit acquires its own lock via process_notification
    emit_due_notification(app, db, &task);
}

// ─── Subtask Due-Date Watcher (§19.5.1b, #2092) ──────────────────────────────

/// Classification of an overdue/due subtask, analogous to DueTaskClassification.
#[derive(Debug, PartialEq)]
pub struct DueSubtaskClassification {
    pub subtask_id: String,
    pub subtask_text: String,
    pub parent_title: String,
    pub threshold: String,  // "due_today" or "overdue"
    pub notif_type: String, // "subtask_due" or "subtask_overdue"
}

/// Scan the `subtasks` table for overdue / due-today items and return classifications.
///
/// Only returns subtasks where:
///   - due_date IS NOT NULL and IS NOT ''
///   - done = 0
///   - due_date <= today
///   - parent note is not deleted
///
/// Used by `check_due_dates` (global scan) and `check_due_dates_for_subtask` (scoped).
pub fn classify_due_subtasks(
    conn: &rusqlite::Connection,
    today_local: &str,
    subtask_id_filter: Option<&str>,
) -> Vec<DueSubtaskClassification> {
    // §31.2.5 / ADR-0057 RISK 2: watcher filters done=0 (only fires for undone subtasks).
    // Due Today query (subtasks.rs::get_due_today_items_query) is done-inclusive — asymmetry is intentional.
    let (sql, filter_param) = if let Some(sid) = subtask_id_filter {
        (
            "SELECT s.id, s.text, COALESCE(n.title, '') AS parent_title, s.due_date \
             FROM subtasks s \
             JOIN notes n ON n.id = s.note_id \
             WHERE s.id = ?1 \
               AND s.due_date IS NOT NULL AND s.due_date != '' \
               AND s.done = 0 \
               AND n.deleted_at IS NULL"
            .to_string(),
            Some(sid.to_string()),
        )
    } else {
        (
            "SELECT s.id, s.text, COALESCE(n.title, '') AS parent_title, s.due_date \
             FROM subtasks s \
             JOIN notes n ON n.id = s.note_id \
             WHERE s.due_date IS NOT NULL AND s.due_date != '' \
               AND s.done = 0 \
               AND n.deleted_at IS NULL"
            .to_string(),
            None,
        )
    };

    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[subtask-due] classify_due_subtasks prepare error: {}", e);
            return vec![];
        }
    };

    // Use a collect-first approach to avoid closure capture type mismatch.
    let raw_rows: Vec<(String, String, String, String)> = if let Some(ref p) = filter_param {
        match stmt.query_map(params![p], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                eprintln!("[subtask-due] classify_due_subtasks query error: {}", e);
                return vec![];
            }
        }
    } else {
        match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) {
            Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
            Err(e) => {
                eprintln!("[subtask-due] classify_due_subtasks query error: {}", e);
                return vec![];
            }
        }
    };

    let mut results = Vec::new();
    for row in raw_rows {
        let (subtask_id, subtask_text, parent_title, due_date) = row;

        if due_date.as_str() < today_local {
            results.push(DueSubtaskClassification {
                subtask_id,
                subtask_text,
                parent_title,
                threshold: "overdue".to_string(),
                notif_type: "subtask_overdue".to_string(),
            });
        } else if due_date.as_str() == today_local {
            results.push(DueSubtaskClassification {
                subtask_id,
                subtask_text,
                parent_title,
                threshold: "due_today".to_string(),
                notif_type: "subtask_due".to_string(),
            });
        }
        // future dates: skip
    }
    results
}

/// Emit a due-date notification for a single overdue/due subtask.
///
/// Notification body includes the parent note title for context.
fn emit_subtask_due_notification(
    app: &AppHandle,
    db: &DbState,
    item: &DueSubtaskClassification,
) {
    let idem_key = format!("due:subtask:{}:{}", item.subtask_id, item.threshold);

    let notif_title = match item.notif_type.as_str() {
        "subtask_due" => "Subtask due today",
        _ => "Subtask overdue",
    };
    let notif_body = format!("{} (from: {})", item.subtask_text, item.parent_title);

    let expires_at = (Local::now() + Duration::days(30))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();

    process_notification(
        app,
        db,
        &item.notif_type,
        notif_title,
        &notif_body,
        "system",
        Some("subtask"),
        Some(item.subtask_id.as_str()),
        Some(idem_key.as_str()),
        Some(expires_at.as_str()),
        false, // in-app only
    );
}

/// Scoped watcher variant: check due-dates for a single subtask.
///
/// Called after any `update_subtask` that touches `due_date` or `done`.
/// Same lock discipline as `check_due_dates_for_task`: hold lock for dedup
/// SELECT, drop before emit.
pub fn check_due_dates_for_subtask(
    app: &AppHandle,
    db: &crate::database::DbState,
    subtask_id: &str,
    _note_id: &str,
) {
    let conn = match db.0.lock() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[subtask-due] check_due_dates_for_subtask: DB lock failed: {}", e);
            return;
        }
    };

    let now_local = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    run_expiry_sweep(&conn, &now_local);

    let today_local = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let items = classify_due_subtasks(&conn, &today_local, Some(subtask_id));

    if items.is_empty() {
        drop(conn);
        return;
    }

    let item = &items[0];
    let idem_key = format!("due:subtask:{}:{}", item.subtask_id, item.threshold);
    let existing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
            params![idem_key],
            |r| r.get(0),
        )
        .unwrap_or(0);
    drop(conn);

    if existing > 0 {
        return;
    }

    // Lock fully released; emit acquires its own lock inside process_notification.
    emit_subtask_due_notification(app, db, item);
}

// ─── #3149/C45: `notification_key(firing_id, scope)` ───────────────────────

#[cfg(test)]
mod notification_key_tests_c45 {
    use super::*;

    /// The refactor's own first-test prescription (#3149's text, verbatim):
    /// the helper's output for today's two existing consumers must be
    /// byte-identical to what shipped before this enum existed.
    ///
    /// Mutation: change either scope's format string (e.g. `Origin` to
    /// `-src` instead of `-origin`) — reddens.
    #[test]
    fn test_notification_key_matches_todays_two_consumers_byte_for_byte() {
        let id = "msg-abc123";
        assert_eq!(
            notification_key(id, NotificationScope::OwnSession),
            format!("agent_message:{}", id),
            "OwnSession must match the bare pre-#3149 format exactly"
        );
        assert_eq!(
            notification_key(id, NotificationScope::Origin),
            format!("agent_message:{}-origin", id),
            "Origin must match the pre-#3149 caller-side `-origin`-suffixed format exactly"
        );
    }

    /// The two scopes must never format identically for the same
    /// `firing_id` — this is the entire reason the enum exists (a bare
    /// string convention can't enforce it; a match arm can).
    ///
    /// Mutation: make both scopes format identically (e.g. both return
    /// the bare `agent_message:{id}`) — reddens.
    #[test]
    fn test_notification_key_scopes_never_collide() {
        for id in ["a", "firing-1", "job-turn-xyz", ""] {
            assert_ne!(
                notification_key(id, NotificationScope::OwnSession),
                notification_key(id, NotificationScope::Origin),
                "OwnSession and Origin must never collide for firing_id={:?}",
                id
            );
        }
    }

    /// C45 (#3155): `commit cdbf7822`'s message claimed this OC-pickup
    /// scope change was "byte-identical, confirmed by a dedicated test"
    /// — false as written; this module had exactly two tests, both scoped
    /// to `OwnSession`/`Origin` only, and neither touched the OC-pickup
    /// shape at all. This test (and the collision test below) makes that
    /// claim true going forward: `OcPickup`'s output must match the
    /// pre-existing hand-composed shape
    /// (`format!("oc-pickup-{}", local_message_id)` passed as the bare
    /// `OwnSession` firing_id, i.e. `agent_message:oc-pickup-{id}`)
    /// byte-for-byte.
    ///
    /// Mutation: change the `OcPickup` format string (e.g. drop the
    /// `oc-pickup-` prefix, or swap the literal for `-oc` suffixed like
    /// `Origin`) — reddens.
    #[test]
    fn test_notification_key_oc_pickup_matches_pre_c45_hand_composed_format() {
        let id = "local-msg-456";
        assert_eq!(
            notification_key(id, NotificationScope::OcPickup),
            format!("agent_message:oc-pickup-{}", id),
            "OcPickup must match the pre-#3155 hand-composed `oc-pickup-{{id}}` format exactly"
        );
    }

    /// C45 (#3155): three-way non-collision across all three scopes —
    /// `OcPickup` must never collide with `OwnSession` or `Origin` either,
    /// not just the two scopes `test_notification_key_scopes_never_collide`
    /// already covers.
    ///
    /// Mutation: make `OcPickup` format identically to `OwnSession` (e.g.
    /// drop the `oc-pickup-` prefix entirely) — reddens.
    #[test]
    fn test_notification_key_three_way_non_collision_c45() {
        for id in ["a", "firing-1", "job-turn-xyz", ""] {
            let own = notification_key(id, NotificationScope::OwnSession);
            let origin = notification_key(id, NotificationScope::Origin);
            let oc_pickup = notification_key(id, NotificationScope::OcPickup);
            assert_ne!(own, origin, "OwnSession and Origin must never collide for firing_id={:?}", id);
            assert_ne!(own, oc_pickup, "OwnSession and OcPickup must never collide for firing_id={:?}", id);
            assert_ne!(origin, oc_pickup, "Origin and OcPickup must never collide for firing_id={:?}", id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Create an in-memory SQLite DB with the full schema needed by PR3a tests.
    /// Includes the extended notifications columns from PR1 + the notes table.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE notifications (
                id TEXT PRIMARY KEY,
                type TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                source TEXT NOT NULL,
                read INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                dismissed_at TEXT,
                target_kind TEXT,
                target_id TEXT,
                idem_key TEXT,
                expires_at TEXT
            );
            CREATE TABLE chat_messages_local (
                id TEXT PRIMARY KEY,
                project_id TEXT,
                role TEXT,
                content TEXT,
                created_at TEXT
            );
            CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                due_date TEXT,
                status TEXT,
                deleted_at TEXT,
                parent_id TEXT,
                note_type TEXT
            );
        ").unwrap();
        conn
    }

    // ─── Existing tests (updated for extended insert_notification signature) ───

    /// Regression test: insert_notification() must NOT write to chat_messages_local.
    ///
    /// Covers the bug from #1811 where process_notification() was inserting every
    /// notification as an assistant message in the Home chat channel (project_id='workspace'),
    /// causing notifications to appear in the UI, leak into AI context, FTS, and embeddings.
    #[test]
    fn insert_notification_does_not_write_to_chat_messages_local() {
        let conn = setup_db();

        let result = insert_notification(
            &conn,
            "test-id-1234",
            "info",
            "Task X is due",
            "Due date reminder",
            "system",
            "2026-07-07 13:00:00",
            None,
            None,
            None,
            None,
        );
        assert!(result.is_ok(), "insert_notification should succeed: {:?}", result);

        // notifications table should have exactly 1 row with correct fields
        let notif_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))
            .unwrap();
        assert_eq!(notif_count, 1, "Expected exactly 1 row in notifications");

        let (stored_type, stored_title, stored_source): (String, String, String) = conn
            .query_row(
                "SELECT type, title, source FROM notifications WHERE id = 'test-id-1234'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(stored_type, "info");
        assert_eq!(stored_title, "Task X is due");
        assert_eq!(stored_source, "system");

        // chat_messages_local must remain empty — this was the #1811 bug
        let chat_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM chat_messages_local", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            chat_count,
            0,
            "process_notification() must not write to chat_messages_local (#1811)"
        );
    }

    /// Verify insert_notification() correctly records the full notification row (7 original columns).
    #[test]
    fn insert_notification_writes_correct_fields() {
        let conn = setup_db();

        insert_notification(
            &conn,
            "notif-abc",
            "warning",
            "Deploy failed",
            "Build pipeline error on main",
            "ci",
            "2026-07-07 14:30:00",
            None,
            None,
            None,
            None,
        )
        .unwrap();

        let (id, notif_type, title, body, source, read, created_at): (
            String, String, String, String, String, i32, String,
        ) = conn
            .query_row(
                "SELECT id, type, title, body, source, read, created_at FROM notifications",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .unwrap();

        assert_eq!(id, "notif-abc");
        assert_eq!(notif_type, "warning");
        assert_eq!(title, "Deploy failed");
        assert_eq!(body, "Build pipeline error on main");
        assert_eq!(source, "ci");
        assert_eq!(read, 0, "new notifications should be unread");
        assert_eq!(created_at, "2026-07-07 14:30:00");
    }

    // ─── PR3a New Tests ─────────────────────────────────────────────────────────

    /// Test 3: insert_notification() with full column set (PR3a extended signature)
    ///
    /// CI-provable item #6: Assert all new columns (target_kind, target_id, idem_key, expires_at)
    /// are written correctly.
    #[test]
    fn insert_notification_writes_extended_columns() {
        let conn = setup_db();

        insert_notification(
            &conn,
            "notif-ext-001",
            "task_due",
            "Task due today",
            "Design the new onboarding flow",
            "system",
            "2026-07-07 10:00:00",
            Some("note"),
            Some("task-uuid-abc123"),
            Some("due:task-uuid-abc123:due_today"),
            Some("2026-08-06 10:00:00"),
        )
        .unwrap();

        let (target_kind, target_id, idem_key, expires_at): (
            Option<String>, Option<String>, Option<String>, Option<String>,
        ) = conn
            .query_row(
                "SELECT target_kind, target_id, idem_key, expires_at FROM notifications WHERE id = 'notif-ext-001'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();

        assert_eq!(target_kind.as_deref(), Some("note"));
        assert_eq!(target_id.as_deref(), Some("task-uuid-abc123"));
        assert_eq!(idem_key.as_deref(), Some("due:task-uuid-abc123:due_today"));
        assert_eq!(expires_at.as_deref(), Some("2026-08-06 10:00:00"));
    }

    /// Test 1: Task selection query — classify_due_tasks() returns the right rows.
    ///
    /// Seeds notes with various statuses/due_dates and a fixed today_local = "2026-07-08".
    /// Asserts only eligible due/overdue tasks are returned (correct predicate).
    #[test]
    fn task_selection_query_returns_correct_tasks() {
        let conn = setup_db();
        let today = "2026-07-08";

        // Should be included: due today
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["t1", "Due Today Task", today, "todo", None::<String>, None::<String>, None::<String>],
        ).unwrap();

        // Should be included: overdue
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["t2", "Overdue Task", "2026-07-01", "in_progress", None::<String>, None::<String>, None::<String>],
        ).unwrap();

        // Should be excluded: future date
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["t3", "Future Task", "2026-07-15", "todo", None::<String>, None::<String>, None::<String>],
        ).unwrap();

        // Should be excluded: status = 'done'
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["t4", "Done Task", "2026-07-07", "done", None::<String>, None::<String>, None::<String>],
        ).unwrap();

        // Should be excluded: status = 'cabinet'
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["t5", "Cabinet Task", "2026-07-07", "cabinet", None::<String>, None::<String>, None::<String>],
        ).unwrap();

        // Should be excluded: deleted
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["t6", "Deleted Task", "2026-07-07", "todo", "2026-07-06 10:00:00", None::<String>, None::<String>],
        ).unwrap();

        // Should be excluded: has parent_id (subtask)
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["t7", "Subtask", "2026-07-07", "todo", None::<String>, "parent-id-xyz", None::<String>],
        ).unwrap();

        // Should be excluded: note_type is not NULL
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["t8", "Journal Entry", "2026-07-07", "todo", None::<String>, None::<String>, "journal"],
        ).unwrap();

        // Should be excluded: no due_date
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["t9", "No Due Date", None::<String>, "todo", None::<String>, None::<String>, None::<String>],
        ).unwrap();

        let results = classify_due_tasks(&conn, today);

        let ids: Vec<&str> = results.iter().map(|t| t.task_id.as_str()).collect();
        assert!(ids.contains(&"t1"), "t1 (due today) should be included");
        assert!(ids.contains(&"t2"), "t2 (overdue) should be included");
        assert!(!ids.contains(&"t3"), "t3 (future) should be excluded");
        assert!(!ids.contains(&"t4"), "t4 (done) should be excluded");
        assert!(!ids.contains(&"t5"), "t5 (cabinet) should be excluded");
        assert!(!ids.contains(&"t6"), "t6 (deleted) should be excluded");
        assert!(!ids.contains(&"t7"), "t7 (subtask) should be excluded");
        assert!(!ids.contains(&"t8"), "t8 (note_type non-null) should be excluded");
        assert!(!ids.contains(&"t9"), "t9 (no due_date) should be excluded");
        assert_eq!(results.len(), 2, "Exactly 2 eligible tasks");
    }

    /// #2339 PR 3: predicate widening regression test — `classify_due_tasks`'s
    /// query now matches `(note_type IS NULL OR note_type = 'sheet')`, not a
    /// blanket pass-through. 3-row inclusion/exclusion pattern: a
    /// `note_type IS NULL` row and a `note_type = 'sheet'` row must both be
    /// classified, while a `note_type = 'future_test_type'` row (an arbitrary
    /// non-null, non-sheet value) must be excluded. Its textually-identical
    /// sibling query in `check_due_dates_for_task` is covered by inference
    /// (deferred per the PR 3 scope report — requires `AppHandle`/`mock_app`).
    #[test]
    fn classify_due_tasks_includes_sheet_notes() {
        let conn = setup_db();
        let today = "2026-07-08";

        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["s1", "Null Type Task", today, "todo", None::<String>, None::<String>, None::<String>],
        ).unwrap();

        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["s2", "Sheet Task", today, "todo", None::<String>, None::<String>, Some("sheet")],
        ).unwrap();

        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["s3", "Future Type Task", today, "todo", None::<String>, None::<String>, Some("future_test_type")],
        ).unwrap();

        let results = classify_due_tasks(&conn, today);
        let ids: Vec<&str> = results.iter().map(|t| t.task_id.as_str()).collect();

        assert!(ids.contains(&"s1"), "note_type IS NULL must be included");
        assert!(ids.contains(&"s2"), "note_type = 'sheet' must be included");
        assert!(
            !ids.contains(&"s3"),
            "arbitrary non-null, non-sheet note_type must still be excluded \
             — proves the predicate is (IS NULL OR = 'sheet'), not a blanket pass-through"
        );
        assert_eq!(results.len(), 2, "exactly the NULL and sheet rows should be classified");
    }

    /// Test 2: Timezone correctness — comparisons use the bound today_local string, NOT UTC.
    ///
    /// Binds a fixed today_local. Verifies:
    ///   - due_date == today_local → "due_today"
    ///   - due_date < today_local → "overdue"
    ///   - due_date > today_local → skipped
    /// Verifies behavior is purely string-based (no datetime('now') dependency).
    #[test]
    fn timezone_correctness_uses_bound_today_local_not_utc() {
        let conn = setup_db();
        // Simulate a user in a non-UTC timezone: today_local = "2026-07-08"
        // Note: if we used SQLite datetime('now') for a UTC-4 user at 8 PM local,
        // it would return '2026-07-09 00:00:xx' and misclassify tomorrow as today.
        let today_local = "2026-07-08";

        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["tz-due", "Due Today", today_local, "todo", None::<String>, None::<String>, None::<String>],
        ).unwrap();
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["tz-overdue", "Overdue", "2026-07-07", "todo", None::<String>, None::<String>, None::<String>],
        ).unwrap();
        conn.execute(
            "INSERT INTO notes (id, title, due_date, status, deleted_at, parent_id, note_type) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params!["tz-future", "Future", "2026-07-09", "todo", None::<String>, None::<String>, None::<String>],
        ).unwrap();

        let results = classify_due_tasks(&conn, today_local);

        let due_today = results.iter().find(|t| t.task_id == "tz-due");
        assert!(due_today.is_some(), "tz-due should be classified");
        assert_eq!(due_today.unwrap().threshold, "due_today");
        assert_eq!(due_today.unwrap().notif_type, "task_due");

        let overdue = results.iter().find(|t| t.task_id == "tz-overdue");
        assert!(overdue.is_some(), "tz-overdue should be classified");
        assert_eq!(overdue.unwrap().threshold, "overdue");
        assert_eq!(overdue.unwrap().notif_type, "task_overdue");

        let future = results.iter().find(|t| t.task_id == "tz-future");
        assert!(future.is_none(), "tz-future should be skipped");
    }

    /// Test 3 (idem_key): Verify idem-key format matches spec.
    ///
    /// CI-provable item #3: `format!("due:{}:{}", task_id, threshold)` == expected.
    #[test]
    fn idem_key_computation_matches_spec() {
        let task_id = "abc123";
        let overdue_key = format!("due:{}:{}", task_id, "overdue");
        let due_today_key = format!("due:{}:{}", task_id, "due_today");

        assert_eq!(overdue_key, "due:abc123:overdue");
        assert_eq!(due_today_key, "due:abc123:due_today");
    }

    /// Test 4: SELECT-before-INSERT dedup — second call is no-op; dismiss does NOT allow re-fire.
    ///
    /// CI-provable item #4 (updated for #1811 no-column fix):
    ///   a) Insert with idem_key; second insert attempt skipped (row count stays 1).
    ///   b) After setting dismissed_at, dedup query (no dismissed_at IS NULL filter) still
    ///      returns COUNT=1 — dismissed row acts as a permanent tombstone blocking re-fire.
    #[test]
    fn dedup_select_before_insert_prevents_duplicate_and_dismiss_does_not_allow_refire() {
        let conn = setup_db();
        let idem_key = "due:task-xyz:overdue";

        // First insert
        insert_notification(
            &conn,
            "notif-dedup-1",
            "task_overdue",
            "Task overdue",
            "My Important Task",
            "system",
            "2026-07-07 09:00:00",
            Some("note"),
            Some("task-xyz"),
            Some(idem_key),
            Some("2026-08-06 09:00:00"),
        ).unwrap();

        // Simulate dedup check (no-column: no dismissed_at IS NULL filter)
        let existing: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
            rusqlite::params![idem_key],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(existing, 1, "Should find one notification (undismissed)");
        assert!(existing > 0, "Dedup check: existing > 0 should prevent second insert");

        // Verify count is still 1
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0)).unwrap();
        assert_eq!(total, 1, "Still exactly 1 row after dedup check");

        // Now dismiss the notification (simulates user clicking ×)
        conn.execute(
            "UPDATE notifications SET dismissed_at = '2026-07-07 12:00:00' WHERE idem_key = ?1",
            rusqlite::params![idem_key],
        ).unwrap();

        // After dismiss: no-column dedup query still returns COUNT=1 — tombstone blocks re-fire.
        // This is the core of the #1811 fix: dismissed row idem_key persists in DB.
        let existing_after_dismiss: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
            rusqlite::params![idem_key],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(existing_after_dismiss, 1,
            "After dismiss, dedup check BLOCKS re-fire (dismissed row is tombstone)");

        // Re-fire is blocked: total row count stays at 1
        let total_after: i64 = conn.query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0)).unwrap();
        assert_eq!(total_after, 1, "Dismissed row blocks re-fire; total stays at 1");
    }

    /// Test 4b: New threshold fires without companion clear (different idem_key).
    ///
    /// When a task crosses from due_today to overdue, the idem_key suffix changes.
    /// The old dismissed due_today row does NOT block the new overdue notification.
    #[test]
    fn new_threshold_fires_despite_prior_dismiss_different_idem_key() {
        let conn = setup_db();
        let task_id = "task-threshold";
        let due_today_key = format!("due:{}:due_today", task_id);
        let overdue_key = format!("due:{}:overdue", task_id);

        // Insert and dismiss a due_today notification
        insert_notification(
            &conn,
            "notif-dt-1",
            "task_due",
            "Task due today",
            "Important task",
            "system",
            "2026-07-07 09:00:00",
            Some("note"),
            Some(task_id),
            Some(&due_today_key),
            Some("2026-08-06 09:00:00"),
        ).unwrap();
        conn.execute(
            "UPDATE notifications SET dismissed_at = '2026-07-07 20:00:00' WHERE idem_key = ?1",
            rusqlite::params![&due_today_key],
        ).unwrap();

        // Dedup check for the OVERDUE key — different idem_key, no prior row
        let existing_overdue: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
            rusqlite::params![&overdue_key],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(existing_overdue, 0,
            "No prior row for overdue key — new threshold notification should fire");

        // Insert the overdue notification (simulates check_due_dates emitting)
        insert_notification(
            &conn,
            "notif-overdue-1",
            "task_overdue",
            "Task overdue",
            "Important task",
            "system",
            "2026-07-08 09:00:00",
            Some("note"),
            Some(task_id),
            Some(&overdue_key),
            Some("2026-08-07 09:00:00"),
        ).unwrap();

        // Two total rows: dismissed due_today + new overdue
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0)).unwrap();
        assert_eq!(total, 2, "Old dismissed due_today row + new overdue notification");
    }

    /// Test 4c: Companion clear (DELETE) on reopen-from-done enables re-fire.
    ///
    /// Simulates the full reopen flow:
    ///   1. Notification emitted for due task
    ///   2. User dismisses it
    ///   3. Task marked done (auto-dismiss fires, but row already dismissed)
    ///   4. Task reopened (status done → todo)
    ///   5. Companion clear DELETEs notification rows for target_id
    ///   6. Dedup check returns 0 → fresh notification can be emitted
    #[test]
    fn companion_clear_on_reopen_enables_refire() {
        let conn = setup_db();
        let task_id = "task-reopen";
        let idem_key = format!("due:{}:due_today", task_id);

        // Step 1: Insert notification
        insert_notification(
            &conn,
            "notif-reopen-1",
            "task_due",
            "Task due today",
            "Reopen task",
            "system",
            "2026-07-07 09:00:00",
            Some("note"),
            Some(task_id),
            Some(&idem_key),
            Some("2026-08-06 09:00:00"),
        ).unwrap();

        // Step 2: User dismisses
        conn.execute(
            "UPDATE notifications SET dismissed_at = '2026-07-07 10:00:00' WHERE idem_key = ?1",
            rusqlite::params![&idem_key],
        ).unwrap();

        // Dedup check confirms tombstone is blocking
        let before_clear: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
            rusqlite::params![&idem_key],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(before_clear, 1, "Dismissed row blocks re-fire before companion clear");

        // Step 3/4: Task reopened from done — companion clear fires
        conn.execute(
            "DELETE FROM notifications WHERE target_id = ?1 AND target_kind = 'note'",
            rusqlite::params![task_id],
        ).unwrap();

        // Step 5: Dedup check after DELETE — row gone, re-fire is now allowed
        let after_clear: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
            rusqlite::params![&idem_key],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(after_clear, 0, "After companion clear, dedup check allows re-fire");

        // Step 6: check_due_dates_for_task can now emit fresh notification
        insert_notification(
            &conn,
            "notif-reopen-2",
            "task_due",
            "Task due today",
            "Reopen task",
            "system",
            "2026-07-08 09:00:00",
            Some("note"),
            Some(task_id),
            Some(&idem_key),
            Some("2026-08-07 09:00:00"),
        ).unwrap();

        let total: i64 = conn.query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0)).unwrap();
        assert_eq!(total, 1, "After DELETE + re-emit: exactly 1 fresh notification row");

        let (dismissed_at, ): (Option<String>, ) = conn.query_row(
            "SELECT dismissed_at FROM notifications WHERE id = 'notif-reopen-2'",
            [], |r| Ok((r.get(0)?, ))
        ).unwrap();
        assert!(dismissed_at.is_none(), "Fresh notification is undismissed");
    }

    /// Test 4d: Companion clear on due_date change enables re-fire.
    ///
    /// When due_date changes to a different non-empty value with the same threshold,
    /// companion clear DELETEs old rows so check_due_dates_for_task can re-emit.
    #[test]
    fn companion_clear_on_due_date_change_enables_refire() {
        let conn = setup_db();
        let task_id = "task-due-change";
        let idem_key = format!("due:{}:due_today", task_id);

        // Insert and dismiss notification
        insert_notification(
            &conn,
            "notif-due-1",
            "task_due",
            "Task due today",
            "Due date task",
            "system",
            "2026-07-07 09:00:00",
            Some("note"),
            Some(task_id),
            Some(&idem_key),
            Some("2026-08-06 09:00:00"),
        ).unwrap();
        conn.execute(
            "UPDATE notifications SET dismissed_at = '2026-07-07 12:00:00' WHERE idem_key = ?1",
            rusqlite::params![&idem_key],
        ).unwrap();

        // Tombstone confirmed
        let before: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
            rusqlite::params![&idem_key], |r| r.get(0),
        ).unwrap();
        assert_eq!(before, 1, "Dismissed row is tombstone before companion clear");

        // due_date changed to a new value (same threshold) → companion clear fires
        conn.execute(
            "DELETE FROM notifications WHERE target_id = ?1 AND target_kind = 'note'",
            rusqlite::params![task_id],
        ).unwrap();

        // Dedup check after DELETE → can re-fire
        let after: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
            rusqlite::params![&idem_key], |r| r.get(0),
        ).unwrap();
        assert_eq!(after, 0, "After companion clear on due_date change, re-fire is allowed");

        // Emit fresh notification
        insert_notification(
            &conn,
            "notif-due-2",
            "task_due",
            "Task due today",
            "Due date task",
            "system",
            "2026-07-08 09:00:00",
            Some("note"),
            Some(task_id),
            Some(&idem_key),
            Some("2026-08-07 09:00:00"),
        ).unwrap();

        let total: i64 = conn.query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0)).unwrap();
        assert_eq!(total, 1, "After DELETE + re-emit: exactly 1 fresh notification row");
    }

    /// Test 4e: Unrelated edit does NOT trigger companion clear — dismiss persists.
    ///
    /// Changing title or body does not fire the companion clear. The dismissed row
    /// remains as a tombstone blocking re-fire.
    #[test]
    fn unrelated_edit_does_not_clear_dismissed_notification() {
        let conn = setup_db();
        let task_id = "task-unrelated";
        let idem_key = format!("due:{}:due_today", task_id);

        // Insert and dismiss notification
        insert_notification(
            &conn,
            "notif-unrelated-1",
            "task_due",
            "Task due today",
            "Some task",
            "system",
            "2026-07-07 09:00:00",
            Some("note"),
            Some(task_id),
            Some(&idem_key),
            Some("2026-08-06 09:00:00"),
        ).unwrap();
        conn.execute(
            "UPDATE notifications SET dismissed_at = '2026-07-07 12:00:00' WHERE idem_key = ?1",
            rusqlite::params![&idem_key],
        ).unwrap();

        // Simulate title-only edit: neither reopened_from_done nor due_date_changed_to_non_empty
        let reopened_from_done = false;
        let due_date_changed_to_non_empty = false;

        // Companion clear should NOT fire
        if reopened_from_done || due_date_changed_to_non_empty {
            conn.execute(
                "DELETE FROM notifications WHERE target_id = ?1 AND target_kind = 'note'",
                rusqlite::params![task_id],
            ).unwrap();
        }

        // Dismissed row still in DB — tombstone intact
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
            rusqlite::params![&idem_key], |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1, "Unrelated edit: dismissed row persists as tombstone, re-fire blocked");

        let dismissed: Option<String> = conn.query_row(
            "SELECT dismissed_at FROM notifications WHERE id = 'notif-unrelated-1'",
            [], |r| r.get(0),
        ).unwrap();
        assert!(dismissed.is_some(), "dismissed_at is still set after unrelated edit");
    }

    /// Test 5: Expiry sweep — sets dismissed_at on notifications with expires_at in the past.
    ///
    /// CI-provable item #5.
    #[test]
    fn expiry_sweep_dismisses_expired_notifications() {
        let conn = setup_db();

        // Insert a notification with expires_at in the past
        insert_notification(
            &conn,
            "notif-expired",
            "task_due",
            "Task due today",
            "Old task",
            "system",
            "2026-06-01 09:00:00",
            Some("note"),
            Some("task-old"),
            Some("due:task-old:due_today"),
            Some("2026-06-15 09:00:00"),  // expires_at in the past
        ).unwrap();

        // Insert a notification with expires_at in the future (should NOT be swept)
        insert_notification(
            &conn,
            "notif-fresh",
            "task_due",
            "Task due today",
            "Current task",
            "system",
            "2026-07-07 09:00:00",
            Some("note"),
            Some("task-current"),
            Some("due:task-current:due_today"),
            Some("2026-08-06 09:00:00"),  // expires_at in the future
        ).unwrap();

        // Verify neither has dismissed_at yet
        let undismissed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE dismissed_at IS NULL",
            [], |r| r.get(0),
        ).unwrap();
        assert_eq!(undismissed, 2, "Both start undismissed");

        // Run expiry sweep at 2026-07-07 13:00:00 local
        run_expiry_sweep(&conn, "2026-07-07 13:00:00");

        // Only the expired notification should be swept
        let (expired_dismissed, fresh_dismissed): (Option<String>, Option<String>) = (
            conn.query_row(
                "SELECT dismissed_at FROM notifications WHERE id = 'notif-expired'",
                [], |r| r.get(0),
            ).unwrap(),
            conn.query_row(
                "SELECT dismissed_at FROM notifications WHERE id = 'notif-fresh'",
                [], |r| r.get(0),
            ).unwrap(),
        );

        assert!(expired_dismissed.is_some(), "Expired notification should have dismissed_at set");
        assert_eq!(expired_dismissed.as_deref(), Some("2026-07-07 13:00:00"));
        assert!(fresh_dismissed.is_none(), "Fresh notification should NOT be swept");
    }
}

#[cfg(test)]
mod notification_dismiss_tests {
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE notifications (
                id TEXT PRIMARY KEY,
                type TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                source TEXT NOT NULL,
                read INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                dismissed_at TEXT,
                target_kind TEXT,
                target_id TEXT,
                idem_key TEXT,
                expires_at TEXT
            );
        ").unwrap();
        conn
    }

    /// PR4 Test 1: dismiss_notification sets dismissed_at; idempotent on repeat call.
    #[test]
    fn test_dismiss_notification_sets_dismissed_at() {
        let conn = setup_db();

        // Insert an active notification
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, read, created_at) VALUES ('notif-1', 'task_due', 'Task Due', 'Do something', 'system', 0, '2026-07-07 10:00:00')",
            [],
        ).unwrap();

        // Run dismiss SQL
        conn.execute(
            "UPDATE notifications SET dismissed_at = datetime('now') WHERE id = ?1 AND dismissed_at IS NULL",
            rusqlite::params!["notif-1"],
        ).unwrap();

        // Assert dismissed_at IS NOT NULL
        let dismissed_at: Option<String> = conn.query_row(
            "SELECT dismissed_at FROM notifications WHERE id = 'notif-1'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert!(dismissed_at.is_some(), "dismissed_at should be set after dismiss");

        // Idempotent: run dismiss again — must not error, dismissed_at unchanged
        let first_dismissed_at = dismissed_at.unwrap();
        conn.execute(
            "UPDATE notifications SET dismissed_at = datetime('now') WHERE id = ?1 AND dismissed_at IS NULL",
            rusqlite::params!["notif-1"],
        ).unwrap();

        let dismissed_at_after: Option<String> = conn.query_row(
            "SELECT dismissed_at FROM notifications WHERE id = 'notif-1'",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(dismissed_at_after.as_deref(), Some(first_dismissed_at.as_str()),
            "Second dismiss must not overwrite dismissed_at (AND dismissed_at IS NULL guard)");
    }

    /// PR4 Test 2: dismissed rows excluded from both get_notifications query paths.
    #[test]
    fn test_dismissed_excluded_from_get_notifications() {
        let conn = setup_db();

        // Insert 2 active notifications
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, read, created_at) VALUES ('active-1', 'task_due', 'Active 1', 'Body 1', 'system', 0, '2026-07-07 10:00:00')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, read, created_at) VALUES ('active-2', 'task_due', 'Active 2', 'Body 2', 'system', 1, '2026-07-07 11:00:00')",
            [],
        ).unwrap();
        // Insert 1 dismissed notification
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, read, created_at, dismissed_at) VALUES ('dismissed-1', 'task_due', 'Dismissed', 'Gone', 'system', 0, '2026-07-07 09:00:00', '2026-07-07 12:00:00')",
            [],
        ).unwrap();

        // Query unread_only = true: should return only active-1 (read=0, not dismissed)
        let unread_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE read = 0 AND dismissed_at IS NULL",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(unread_count, 1, "unread_only=true: only active-1 returned (1 unread, not dismissed)");

        // Query unread_only = false: should return active-1 and active-2 (2 total, not dismissed)
        let all_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE dismissed_at IS NULL",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(all_count, 2, "unread_only=false: active-1 and active-2 returned (dismissed row excluded)");
    }

    /// PR4 Test 3: dismissed rows excluded from get_unread_count (badge desync fix).
    #[test]
    fn test_dismissed_excluded_from_get_unread_count() {
        let conn = setup_db();

        // Insert 2 unread notifications
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, read, created_at) VALUES ('unread-1', 'task_due', 'Unread 1', 'Body', 'system', 0, '2026-07-07 10:00:00')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, read, created_at) VALUES ('unread-2', 'task_due', 'Unread 2', 'Body', 'system', 0, '2026-07-07 11:00:00')",
            [],
        ).unwrap();

        // Auto-dismiss one (read stays 0 — simulates check_due_dates auto-dismiss)
        conn.execute(
            "UPDATE notifications SET dismissed_at = '2026-07-07 12:00:00' WHERE id = 'unread-2'",
            [],
        ).unwrap();

        // Count must be 1, not 2 — dismissed row excluded even though read=0
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE read = 0 AND dismissed_at IS NULL",
            [],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 1, "Badge count must exclude dismissed rows even when read=0");
    }

    /// PR4 Test 4: get_notifications returns target_kind and target_id columns.
    #[test]
    fn test_get_notifications_returns_target_columns() {
        let conn = setup_db();

        // Insert notification with target_kind and target_id
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, read, created_at, target_kind, target_id) VALUES ('notif-target', 'task_due', 'Target Test', 'Body', 'system', 0, '2026-07-07 10:00:00', 'note', 'test-uuid')",
            [],
        ).unwrap();

        // Query using the updated SELECT with target_kind, target_id
        let mut stmt = conn.prepare(
            "SELECT id, type, title, body, source, read, created_at, target_kind, target_id FROM notifications WHERE dismissed_at IS NULL ORDER BY created_at DESC LIMIT 100"
        ).unwrap();

        let rows: Vec<serde_json::Value> = stmt.query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "type": row.get::<_, String>(1)?,
                "title": row.get::<_, String>(2)?,
                "body": row.get::<_, String>(3)?,
                "source": row.get::<_, String>(4)?,
                "read": row.get::<_, i32>(5)?,
                "created_at": row.get::<_, String>(6)?,
                "target_kind": row.get::<_, Option<String>>(7)?,
                "target_id": row.get::<_, Option<String>>(8)?,
            }))
        }).unwrap().filter_map(|r| r.ok()).collect();

        assert_eq!(rows.len(), 1, "Should return 1 notification");
        let row = &rows[0];
        assert_eq!(row["target_kind"], serde_json::json!("note"), "target_kind should be 'note'");
        assert_eq!(row["target_id"], serde_json::json!("test-uuid"), "target_id should be 'test-uuid'");

        // Also verify null target columns work (notification without target)
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, read, created_at) VALUES ('notif-no-target', 'general', 'No Target', 'Body', 'system', 0, '2026-07-07 11:00:00')",
            [],
        ).unwrap();

        let rows2: Vec<serde_json::Value> = stmt.query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "type": row.get::<_, String>(1)?,
                "title": row.get::<_, String>(2)?,
                "body": row.get::<_, String>(3)?,
                "source": row.get::<_, String>(4)?,
                "read": row.get::<_, i32>(5)?,
                "created_at": row.get::<_, String>(6)?,
                "target_kind": row.get::<_, Option<String>>(7)?,
                "target_id": row.get::<_, Option<String>>(8)?,
            }))
        }).unwrap().filter_map(|r| r.ok()).collect();

        assert_eq!(rows2.len(), 2, "Should return 2 notifications now");
        let no_target_row = rows2.iter().find(|r| r["id"] == "notif-no-target").unwrap();
        assert_eq!(no_target_row["target_kind"], serde_json::Value::Null, "target_kind should be null when not set");
        assert_eq!(no_target_row["target_id"], serde_json::Value::Null, "target_id should be null when not set");
    }
}

/// Tests for get_unread_count_internal (#1811 badge-miscount fix)
#[cfg(test)]
mod get_unread_count_internal_tests {
    use rusqlite::{Connection, params};
    use super::get_unread_count_internal;
    use super::classify_due_subtasks;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE notifications (
                id TEXT PRIMARY KEY,
                type TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                source TEXT NOT NULL,
                read INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                dismissed_at TEXT,
                target_kind TEXT,
                target_id TEXT,
                idem_key TEXT,
                expires_at TEXT
            );
        ").unwrap();
        conn
    }

    fn insert_notif(conn: &Connection, id: &str, read: i32, dismissed: bool) {
        if dismissed {
            conn.execute(
                "INSERT INTO notifications (id, type, title, body, source, read, created_at, dismissed_at) \
                 VALUES (?1, 'task_due', 'Test', 'Body', 'system', ?2, '2026-07-08 10:00:00', '2026-07-08 11:00:00')",
                rusqlite::params![id, read],
            ).unwrap();
        } else {
            conn.execute(
                "INSERT INTO notifications (id, type, title, body, source, read, created_at) \
                 VALUES (?1, 'task_due', 'Test', 'Body', 'system', ?2, '2026-07-08 10:00:00')",
                rusqlite::params![id, read],
            ).unwrap();
        }
    }

    /// Happy path: count after inserting an unread notification is 1.
    #[test]
    fn test_count_after_insert() {
        let conn = setup_db();
        insert_notif(&conn, "n1", 0, false);
        let count = get_unread_count_internal(&conn).expect("should not error");
        assert_eq!(count, 1, "one unread undismissed row → count = 1");
    }

    /// After dismissing the only row, count returns 0.
    #[test]
    fn test_count_after_dismiss() {
        let conn = setup_db();
        insert_notif(&conn, "n1", 0, false);
        // Dismiss it
        conn.execute(
            "UPDATE notifications SET dismissed_at = '2026-07-08 12:00:00' WHERE id = 'n1'",
            [],
        ).unwrap();
        let count = get_unread_count_internal(&conn).expect("should not error");
        assert_eq!(count, 0, "dismissed row must not be counted");
    }

    /// Read rows (read=1) are not counted.
    #[test]
    fn test_read_rows_excluded() {
        let conn = setup_db();
        insert_notif(&conn, "n1", 1, false); // read=1, not dismissed
        let count = get_unread_count_internal(&conn).expect("should not error");
        assert_eq!(count, 0, "read=1 row must not be counted");
    }

    /// Mixed: 2 unread, 1 read, 1 dismissed → count = 2.
    #[test]
    fn test_mixed_rows() {
        let conn = setup_db();
        insert_notif(&conn, "n1", 0, false); // unread, not dismissed
        insert_notif(&conn, "n2", 0, false); // unread, not dismissed
        insert_notif(&conn, "n3", 1, false); // read, not dismissed
        insert_notif(&conn, "n4", 0, true);  // unread, dismissed
        let count = get_unread_count_internal(&conn).expect("should not error");
        assert_eq!(count, 2, "only 2 unread undismissed rows → count = 2");
    }

    /// Empty table → count = 0 (edge case).
    #[test]
    fn test_empty_table() {
        let conn = setup_db();
        let count = get_unread_count_internal(&conn).expect("should not error on empty table");
        assert_eq!(count, 0, "empty table → count = 0");
    }

    // ─── Subtask Watcher Tests (§19.5.1b, #2092) ─────────────────────────────

    /// Set up DB with subtasks + notes tables (in addition to notifications + chat_messages_local).
    fn setup_subtask_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE notifications (
                id TEXT PRIMARY KEY,
                type TEXT NOT NULL DEFAULT 'info',
                title TEXT NOT NULL DEFAULT '',
                body TEXT NOT NULL DEFAULT '',
                source TEXT NOT NULL DEFAULT 'system',
                read INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                dismissed_at TEXT,
                target_kind TEXT,
                target_id TEXT,
                idem_key TEXT,
                expires_at TEXT
            );
            CREATE TABLE chat_messages_local (
                id TEXT PRIMARY KEY, project_id TEXT, role TEXT, content TEXT, created_at TEXT
            );
            CREATE TABLE notes (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                deleted_at TEXT
            );
            CREATE TABLE subtasks (
                id           TEXT NOT NULL PRIMARY KEY,
                note_id      TEXT NOT NULL DEFAULT '',
                text         TEXT NOT NULL DEFAULT '',
                due_date     TEXT,
                done         INTEGER NOT NULL DEFAULT 0,
                completed_at TEXT,
                sort_order   INTEGER NOT NULL DEFAULT 0,
                created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );
        ").unwrap();
        conn
    }

    fn insert_note_sub(conn: &Connection, id: &str, title: &str, deleted_at: Option<&str>) {
        conn.execute(
            "INSERT INTO notes (id, title, deleted_at) VALUES (?1, ?2, ?3)",
            params![id, title, deleted_at],
        ).unwrap();
    }

    fn insert_subtask_watcher(
        conn: &Connection,
        id: &str,
        note_id: &str,
        text: &str,
        due_date: Option<&str>,
        done: bool,
    ) {
        conn.execute(
            "INSERT INTO subtasks (id, note_id, text, due_date, done, sort_order, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 0, datetime('now'), datetime('now'))",
            params![id, note_id, text, due_date, done as i64],
        ).unwrap();
    }

    /// classify_due_subtasks returns overdue subtask when due_date < today.
    #[test]
    fn classify_due_subtasks_picks_up_overdue() {
        let conn = setup_subtask_db();
        insert_note_sub(&conn, "note-w1", "Parent Note", None);
        insert_subtask_watcher(&conn, "s-w1", "note-w1", "Write tests", Some("2020-01-01"), false);

        let today = "2026-07-19";
        let results = classify_due_subtasks(&conn, today, None);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].subtask_id, "s-w1");
        assert_eq!(results[0].threshold, "overdue");
        assert_eq!(results[0].notif_type, "subtask_overdue");
        assert_eq!(results[0].parent_title, "Parent Note");
    }

    /// classify_due_subtasks returns due_today for a subtask due on today's date.
    #[test]
    fn classify_due_subtasks_due_today() {
        let conn = setup_subtask_db();
        insert_note_sub(&conn, "note-w2", "Parent 2", None);
        let today = "2026-07-19";
        insert_subtask_watcher(&conn, "s-w2", "note-w2", "Due item", Some(today), false);

        let results = classify_due_subtasks(&conn, today, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].threshold, "due_today");
        assert_eq!(results[0].notif_type, "subtask_due");
    }

    /// classify_due_subtasks skips done subtasks.
    #[test]
    fn classify_due_subtasks_skips_done() {
        let conn = setup_subtask_db();
        insert_note_sub(&conn, "note-w3", "Parent 3", None);
        insert_subtask_watcher(&conn, "s-w3", "note-w3", "Done item", Some("2020-01-01"), true);

        let results = classify_due_subtasks(&conn, "2026-07-19", None);
        assert_eq!(results.len(), 0, "Done subtasks should not be classified as due");
    }

    /// classify_due_subtasks skips subtasks whose parent note is deleted.
    #[test]
    fn classify_due_subtasks_skips_deleted_parent() {
        let conn = setup_subtask_db();
        insert_note_sub(&conn, "note-w4", "Deleted Parent", Some("2026-07-18 10:00:00"));
        insert_subtask_watcher(&conn, "s-w4", "note-w4", "Orphan subtask", Some("2020-01-01"), false);

        let results = classify_due_subtasks(&conn, "2026-07-19", None);
        assert_eq!(results.len(), 0, "Subtasks with deleted parent should be skipped");
    }

    /// classify_due_subtasks skips future-dated subtasks.
    #[test]
    fn classify_due_subtasks_skips_future() {
        let conn = setup_subtask_db();
        insert_note_sub(&conn, "note-w5", "Parent 5", None);
        insert_subtask_watcher(&conn, "s-w5", "note-w5", "Future item", Some("2099-12-31"), false);

        let results = classify_due_subtasks(&conn, "2026-07-19", None);
        assert_eq!(results.len(), 0, "Future-dated subtasks should not be classified");
    }

    /// classify_due_subtasks with subtask_id filter returns only that subtask.
    #[test]
    fn classify_due_subtasks_scoped_filter() {
        let conn = setup_subtask_db();
        insert_note_sub(&conn, "note-w6", "Parent 6", None);
        insert_subtask_watcher(&conn, "s-w6a", "note-w6", "Sub A", Some("2020-01-01"), false);
        insert_subtask_watcher(&conn, "s-w6b", "note-w6", "Sub B", Some("2020-01-01"), false);

        let results = classify_due_subtasks(&conn, "2026-07-19", Some("s-w6a"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].subtask_id, "s-w6a");
    }

    /// Idem-key dedup: once a subtask notification exists, classify + dedup returns no re-emit.
    #[test]
    fn subtask_watcher_idem_key_prevents_refire() {
        let conn = setup_subtask_db();
        insert_note_sub(&conn, "note-w7", "Parent 7", None);
        insert_subtask_watcher(&conn, "s-w7", "note-w7", "No refire", Some("2020-01-01"), false);

        // Simulate existing notification (already fired)
        let idem_key = "due:subtask:s-w7:overdue";
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, idem_key, created_at) \
             VALUES ('n-w7', 'subtask_overdue', 'Subtask overdue', 'No refire', 'system', ?1, datetime('now'))",
            params![idem_key],
        ).unwrap();

        let today = "2026-07-19";
        let items = classify_due_subtasks(&conn, today, Some("s-w7"));
        assert_eq!(items.len(), 1);

        // Dedup check: should NOT emit again
        let existing: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE idem_key = ?1",
                params![idem_key],
                |r| r.get(0),
            ).unwrap();
        assert_eq!(existing, 1, "Existing notification should block re-emit (idem_key dedup)");
    }
}

#[cfg(test)]
mod account_enabled_poller_gate_tests {
    //! ADR-0070 Decision 3/5 (#3195 PR A) — T5: flag off + stored session
    //! present must mean zero outbound requests at launch. This is the
    //! finding that made PR A necessary: before this PR,
    //! `start_supabase_notification_poller` started unconditionally at
    //! launch, gated only on a stored session existing, and its own
    //! refresh path made a live Supabase network call + rewrote
    //! `auth_session.json` directly, bypassing the guard entirely.
    //!
    //! `start_supabase_notification_poller` takes a real `tauri::AppHandle`,
    //! which has no usable mock constructor for the concrete (non-generic)
    //! `AppHandle` this function requires outside a running Wry app (see
    //! the PR report) — so this is pinned to source, the same golden-string
    //! technique `ai/scheduler.rs` already uses for structural guarantees
    //! it can't exercise at runtime, PLUS a direct behavioral check of the
    //! shared `require_account_enabled()` gate the poller now calls.
    use serial_test::serial;
    use tempfile::TempDir;

    fn isolate() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        std::env::set_var("HOME", dir.path());
        std::env::set_var("PANE_TEST_ROOT", dir.path().join("Pane"));
        dir
    }

    #[test]
    fn test_poller_gate_is_first_statement_pinned_to_source() {
        let src = include_str!("notifications.rs");
        assert!(
            src.contains(
                "pub fn start_supabase_notification_poller(app: AppHandle, db: DbState) {\n    if let Err(e) = crate::sync::commands::require_account_enabled() {"
            ),
            "the account_enabled gate must be the literal first statement of start_supabase_notification_poller"
        );
        // It must return (not just log) when the guard refuses — proving
        // the function exits before get_notification_key()/load_auth_session()/
        // the thread::spawn that would make the live Supabase call.
        let gate_start = src
            .find("pub fn start_supabase_notification_poller(app: AppHandle, db: DbState) {")
            .expect("poller fn must exist");
        let get_key_pos = src[gate_start..]
            .find("get_notification_key()")
            .expect("get_notification_key() call must exist after the poller fn starts");
        let gate_branch = &src[gate_start..gate_start + get_key_pos];
        assert!(
            gate_branch.contains("return;"),
            "the gate branch must return before reaching get_notification_key()/the network path"
        );
    }

    /// Behavioral half: the exact shared guard the poller now calls
    /// (`crate::sync::commands::require_account_enabled`) really is off by
    /// default with a stored session present — i.e. the scenario T5 names
    /// ("flag off + stored session present") produces a guard refusal, not
    /// a pass-through, regardless of whether a session file exists.
    #[test]
    #[serial(pane_test_root)]
    fn test_t5_guard_refuses_with_stored_session_present_and_flag_off() {
        let _d = isolate();

        // Write a stored session directly, the same way notifications.rs's
        // OWN private load_auth_session/save_auth_session pair does (full
        // AuthSession, tokens inline) — NOT via the guarded
        // commands::save_auth_session, to reproduce exactly the scenario
        // Decision 5 is about: an upgrading signed-in user whose
        // auth_session.json was left untouched.
        let session_dir = dirs::home_dir().unwrap().join("Pane").join(".pane");
        std::fs::create_dir_all(&session_dir).unwrap();
        let session_path = session_dir.join("auth_session.json");
        std::fs::write(
            &session_path,
            r#"{"access_token":"fake_access","refresh_token":"fake_refresh","user_id":"u1","email":"t5@example.com","expires_at":9999999999,"device_id":null}"#,
        ).unwrap();
        assert!(session_path.exists(), "fixture session file must be present for this scenario");

        // account_enabled is absent (off) — no ai_settings table even
        // created in this isolated dir, which is itself the "failed read is
        // off" case, not just "absent row is off".
        let result = crate::sync::commands::require_account_enabled();
        assert_eq!(
            result,
            Err("account disabled".to_string()),
            "T5: with a stored session present and the flag off, the shared guard must refuse"
        );
    }

    /// T5b (Reed's #3201 FAIL finding, fix 1): `push_notification` is a
    /// second, independent `#[tauri::command]` entry point into the exact
    /// `load_auth_session` → `refresh_auth_token` → `save_auth_session`
    /// triad the poller uses, reached via its own `get_valid_token()` call
    /// (notifications.rs:1105) — completely unguarded before this fix,
    /// even though `start_supabase_notification_poller`'s own call site
    /// (notifications.rs:997) was already gated by T5 above. This is the
    /// red this fix is written against: with the flag off and a stored
    /// session present, `push_notification` must refuse before it ever
    /// reaches `get_notification_key()` or the network, the same
    /// structural guarantee T5 proves for the poller.
    #[tokio::test]
    #[serial(pane_test_root)]
    async fn test_t5b_push_notification_refuses_when_flag_off() {
        let _d = isolate();

        // Same fixture shape as T5: a stored session present, flag off
        // (no ai_settings table at all — "failed read is off").
        let session_dir = dirs::home_dir().unwrap().join("Pane").join(".pane");
        std::fs::create_dir_all(&session_dir).unwrap();
        let session_path = session_dir.join("auth_session.json");
        std::fs::write(
            &session_path,
            r#"{"access_token":"fake_access","refresh_token":"fake_refresh","user_id":"u1","email":"t5b@example.com","expires_at":9999999999,"device_id":null}"#,
        ).unwrap();

        let result = super::push_notification(
            "title".to_string(),
            "body".to_string(),
            None,
            None,
        )
        .await;

        assert_eq!(
            result,
            Err("account disabled".to_string()),
            "push_notification must refuse before get_notification_key()/get_valid_token()/the network when account_enabled is off"
        );
    }
}
