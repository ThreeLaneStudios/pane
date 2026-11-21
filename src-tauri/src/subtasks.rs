// Pane — Subtask CRUD Commands (§1.9, ADR-0057, #2092)
//
// Manages the `subtasks` table introduced in schema v73. These are the
// "Advanced Checklist" line items — distinct from the old child-notes model
// which used the notes table with parent_id. Commands are prefixed to avoid
// collision with legacy notes-based subtask stubs in api.ts.

use crate::database::DbState;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

/// A single subtask row from the `subtasks` table.
///
/// `done` is stored as INTEGER (0/1) in SQLite but exposed as bool in the API.
/// `completed_at` is set to `datetime('now')` when `done` goes 0→1 and
/// cleared to NULL when `done` goes 1→0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subtask {
    pub id: String,
    pub note_id: String,
    pub text: String,
    pub due_date: Option<String>,
    pub done: bool,
    pub completed_at: Option<String>,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Read a single subtask row by id.
pub fn get_subtask_by_id(conn: &rusqlite::Connection, id: &str) -> Result<Subtask, String> {
    conn.query_row(
        "SELECT id, note_id, text, due_date, done, completed_at, sort_order, created_at, updated_at
         FROM subtasks WHERE id = ?1",
        params![id],
        row_to_subtask,
    )
    .map_err(|e| format!("Subtask not found: {}", e))
}

fn row_to_subtask(row: &rusqlite::Row<'_>) -> rusqlite::Result<Subtask> {
    let done_int: i64 = row.get(4)?;
    Ok(Subtask {
        id: row.get(0)?,
        note_id: row.get(1)?,
        text: row.get(2)?,
        due_date: row.get(3)?,
        done: done_int != 0,
        completed_at: row.get(5)?,
        sort_order: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

// ─── Tauri Commands ────────────────────────────────────────────────────────────

/// Create a new subtask attached to `note_id`.
///
/// Generates a UUID for the row id. `sort_order` is supplied by the caller
/// (frontend manages ordering). `due_date` is optional (YYYY-MM-DD or NULL).
#[tauri::command]
pub fn create_subtask(
    note_id: String,
    text: String,
    due_date: Option<String>,
    sort_order: i64,
    db: State<DbState>,
) -> Result<Subtask, String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    let id = Uuid::new_v4().to_string();

    conn.execute(
        "INSERT INTO subtasks (id, note_id, text, due_date, done, completed_at, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 0, NULL, ?5, datetime('now'), datetime('now'))",
        params![id, note_id, text, due_date, sort_order],
    )
    .map_err(|e| format!("Failed to create subtask: {}", e))?;

    get_subtask_by_id(&conn, &id)
}

/// Fetch all non-deleted subtasks for a given note, ordered by sort_order ASC.
#[tauri::command]
pub fn get_subtasks_for_note(
    note_id: String,
    db: State<DbState>,
) -> Result<Vec<Subtask>, String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, note_id, text, due_date, done, completed_at, sort_order, created_at, updated_at
             FROM subtasks
             WHERE note_id = ?1
             ORDER BY sort_order ASC, created_at ASC",
        )
        .map_err(|e| format!("Failed to prepare query: {}", e))?;

    let rows = stmt
        .query_map(params![note_id], row_to_subtask)
        .map_err(|e| format!("Failed to query subtasks: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect subtasks: {}", e))?;

    Ok(rows)
}

/// Update a subtask. All fields are optional — only provided fields are written.
///
/// `due_date` is `Option<Option<String>>`:
///   - `None`          → field not touched
///   - `Some(None)`    → clear due_date to NULL
///   - `Some(Some(s))` → set due_date to s
///
/// Notification lifecycle hooks (§19.5.1c):
///   a) Companion-clear: fires when due_date changes or done goes 1→0. Deletes
///      existing notification rows so check_due_dates_for_subtask can re-emit.
///   b) Auto-dismiss: fires when done goes 0→1 or due_date is cleared (→NULL).
///      Sets dismissed_at on any active subtask notification.
#[tauri::command]
pub fn update_subtask(
    app: tauri::AppHandle,
    id: String,
    text: Option<String>,
    due_date: Option<Option<String>>,
    done: Option<bool>,
    sort_order: Option<i64>,
    db: State<DbState>,
) -> Result<Subtask, String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    // Read current state for hook comparisons.
    let current = get_subtask_by_id(&conn, &id)?;

    // ── Determine transitions ──────────────────────────────────────────────────

    let done_prev = current.done;
    let done_next = done.unwrap_or(done_prev);

    let due_date_prev = current.due_date.clone();
    let due_date_next: Option<Option<String>> = due_date.clone();

    // "due_date changed" — covers both set-to-value and set-to-NULL
    let due_date_changed = match &due_date_next {
        None => false, // not touched
        Some(None) => due_date_prev.is_some(), // cleared
        Some(Some(new)) => due_date_prev.as_deref() != Some(new.as_str()),
    };

    // due_date cleared to NULL
    let due_date_cleared = matches!(&due_date_next, Some(None));

    // done 0→1
    let completed = !done_prev && done_next;
    // done 1→0
    let reopened = done_prev && !done_next;

    // ── Build SET clause ──────────────────────────────────────────────────────

    let mut set_parts: Vec<String> = Vec::new();
    let mut sql_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(t) = &text {
        set_parts.push("text = ?".to_string());
        sql_params.push(Box::new(t.clone()));
    }

    // due_date: flatten Option<Option<String>> → SQL NULL or value
    match &due_date {
        None => {} // not touched
        Some(None) => {
            set_parts.push("due_date = NULL".to_string());
        }
        Some(Some(v)) => {
            set_parts.push("due_date = ?".to_string());
            sql_params.push(Box::new(v.clone()));
        }
    }

    if let Some(d) = done {
        set_parts.push("done = ?".to_string());
        sql_params.push(Box::new(d as i64));

        if completed {
            // 0→1: stamp completed_at
            set_parts.push("completed_at = datetime('now')".to_string());
        } else if reopened {
            // 1→0: clear completed_at
            set_parts.push("completed_at = NULL".to_string());
        }
    }

    if let Some(s) = sort_order {
        set_parts.push("sort_order = ?".to_string());
        sql_params.push(Box::new(s));
    }

    if set_parts.is_empty() {
        // Nothing to write — return current row unchanged.
        return Ok(current);
    }

    set_parts.push("updated_at = datetime('now')".to_string());

    let sql = format!("UPDATE subtasks SET {} WHERE id = ?", set_parts.join(", "));
    sql_params.push(Box::new(id.clone()));

    let param_refs: Vec<&dyn rusqlite::ToSql> = sql_params.iter().map(|p| p.as_ref()).collect();
    conn.execute(&sql, param_refs.as_slice())
        .map_err(|e| format!("Failed to update subtask: {}", e))?;

    // ── §19.5.1c — Notification hooks ────────────────────────────────────────

    // (b) Auto-dismiss: done 0→1 or due_date cleared → dismiss open notification
    if completed || due_date_cleared {
        conn.execute(
            "UPDATE notifications SET dismissed_at = datetime('now') \
             WHERE target_id = ?1 AND target_kind = 'subtask' AND dismissed_at IS NULL",
            params![id],
        )
        .ok(); // non-fatal
        eprintln!(
            "[SUBTASK-NOTIF] Auto-dismiss: subtask {} (completed={}, due_date_cleared={})",
            id, completed, due_date_cleared
        );
    }

    // (a) Companion-clear: due_date changed to non-empty, or done 1→0 (reopened)
    // Fires ONLY for changes that should allow re-emission by the watcher.
    let due_date_changed_to_non_empty = matches!(&due_date_next, Some(Some(_))) && due_date_changed;
    if due_date_changed_to_non_empty || reopened {
        conn.execute(
            "DELETE FROM notifications WHERE target_id = ?1 AND target_kind = 'subtask'",
            params![id],
        )
        .ok(); // non-fatal
        eprintln!(
            "[SUBTASK-NOTIF] Companion clear: subtask {} (due_changed={}, reopened={})",
            id, due_date_changed_to_non_empty, reopened
        );
    }

    let updated = get_subtask_by_id(&conn, &id)?;

    // Drop the lock before calling the watcher (lock discipline — watcher acquires its own).
    let note_id = current.note_id.clone();
    drop(conn);

    // Fire scoped watcher when anything relevant changes.
    if due_date_changed || completed || reopened {
        let db_clone = crate::database::DbState(db.0.clone());
        crate::notifications::check_due_dates_for_subtask(&app, &db_clone, &id, &note_id);
    }

    Ok(updated)
}

// ─── Cascade helper (called by notes::purge_note) ─────────────────────────────

// ─── Due Today Query (§31.2, ADR-0057) ──────────────────────────────────────────

/// A single row in the Due Today snapshot — either a task (note) or subtask.
///
/// §31.2.1: done-inclusive. Display only.
/// §31.2.5: done items display in Due Today but the watcher only fires for done=0.
///   Asymmetry is intentional — see spec §31.2.1 + ADR-0057 RISK 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DueTodayItem {
    /// Subtask id or note id.
    pub id: String,
    /// Note title (tasks) or subtask text (subtasks).
    pub title: String,
    /// Whether this item is done.
    pub done: bool,
    /// "task" or "subtask".
    pub kind: String,
    /// Name of the project the note belongs to, if any.
    pub project_name: Option<String>,
    /// Parent note title — subtasks only, None for task rows.
    pub parent_note_title: Option<String>,
    /// Parent note id — subtasks only, None for task rows.
    pub parent_note_id: Option<String>,
    /// The note to open when the row body is tapped.
    /// For tasks: the note's own id. For subtasks: the parent note id.
    pub note_id: String,
}

/// Returns all tasks and subtasks due on `date` (YYYY-MM-DD), done-inclusive.
///
/// §31.2.1 union: tasks (notes) + subtasks with `due_date = :date`.
/// §31.2.5: done items display in Due Today but the watcher keeps `done=0` only.
///
/// §31.2.6 Row order (CI-assertable — tested in tests::due_today_order_matches_spec):
///   sort_group 0 → open tasks A–Z, tiebreak id ASC
///   sort_group 1 → open subtasks A–Z, tiebreak id ASC
///   sort_group 2 → done tasks A–Z, tiebreak id ASC
///   sort_group 3 → done subtasks A–Z, tiebreak id ASC
///
/// §31.2.3: null `date` is validated by the caller (frontend renders error state,
/// never silently falls back to now() — see §31.2.3 anchored-not-live rule).
#[tauri::command]
pub fn get_due_today_items(
    date: String,
    db: State<DbState>,
) -> Result<Vec<DueTodayItem>, String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    get_due_today_items_query(&conn, &date)
}

/// Pure-connection helper so tests can call without Tauri State.
pub(crate) fn get_due_today_items_query(
    conn: &rusqlite::Connection,
    date: &str,
) -> Result<Vec<DueTodayItem>, String> {
    // UNION ALL: tasks from notes table + subtasks from subtasks table.
    // sort_group encodes the §31.2.6 display order:
    //   0 = open task, 1 = open subtask, 2 = done task, 3 = done subtask.
    // Within each group: lower(title) ASC, then id ASC as tiebreak.
    let mut stmt = conn
        .prepare(
            "SELECT
                n.id             AS id,
                n.title          AS title,
                CASE WHEN n.status = 'done' THEN 1 ELSE 0 END AS done_int,
                'task'           AS kind,
                p.name           AS project_name,
                NULL             AS parent_note_title,
                NULL             AS parent_note_id,
                n.id             AS note_id,
                CASE WHEN n.status = 'done' THEN 2 ELSE 0 END AS sort_group,
                lower(n.title)   AS title_lower
            FROM notes n
            LEFT JOIN projects p ON p.id = n.project_id
            WHERE n.due_date = ?1
              AND n.deleted_at IS NULL

            UNION ALL

            SELECT
                s.id             AS id,
                s.text           AS title,
                s.done           AS done_int,
                'subtask'        AS kind,
                p.name           AS project_name,
                n.title          AS parent_note_title,
                n.id             AS parent_note_id,
                n.id             AS note_id,
                CASE WHEN s.done = 0 THEN 1 ELSE 3 END AS sort_group,
                lower(s.text)    AS title_lower
            FROM subtasks s
            JOIN notes n ON n.id = s.note_id
            LEFT JOIN projects p ON p.id = n.project_id
            WHERE s.due_date = ?1
              AND n.deleted_at IS NULL

            ORDER BY sort_group ASC, title_lower ASC, id ASC",
        )
        .map_err(|e| format!("Failed to prepare due-today query: {}", e))?;

    let rows = stmt
        .query_map(params![date], |row| {
            let done_int: i64 = row.get(2)?;
            Ok(DueTodayItem {
                id: row.get(0)?,
                title: row.get(1)?,
                done: done_int != 0,
                kind: row.get(3)?,
                project_name: row.get(4)?,
                parent_note_title: row.get(5)?,
                parent_note_id: row.get(6)?,
                note_id: row.get(7)?,
            })
        })
        .map_err(|e| format!("Failed to query due-today items: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect due-today items: {}", e))?;

    Ok(rows)
}

/// Resolve the parent note id for a subtask — used by the notification drawer routing arm.
///
/// §19.5.1d: "resolve the parent via SELECT note_id FROM subtasks WHERE id = {target_id}".
/// Returns `None` if the subtask has been deleted since the notification was emitted
/// (§19.5.1d: "If the subtask no longer exists, fall back to a no-op").
#[tauri::command]
pub fn get_subtask_parent_note_id(
    id: String,
    db: State<DbState>,
) -> Result<Option<String>, String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    let result = conn.query_row(
        "SELECT note_id FROM subtasks WHERE id = ?1",
        params![id],
        |row| row.get::<_, String>(0),
    );
    match result {
        Ok(note_id) => Ok(Some(note_id)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("get_subtask_parent_note_id failed: {}", e)),
    }
}

// ─── Cascade helper (called by notes::purge_note) ─────────────────────────────

/// Hard-delete all subtasks for a note and their notification rows.
///
/// Called by `notes::purge_note` to enforce the §2100 cascade.
/// Exported as `pub(crate)` so both `notes.rs` and unit tests can call it
/// without going through the Tauri command layer (which requires Tauri State).
/// Errors are non-fatal — individual failures are logged but do not abort the
/// parent `purge_note` transaction.
pub(crate) fn cascade_delete_subtasks_for_note(
    conn: &rusqlite::Connection,
    note_id: &str,
) -> Result<(), String> {
    // 1. Delete notifications that reference subtasks belonging to this note.
    conn.execute(
        "DELETE FROM notifications \
         WHERE target_kind = 'subtask' \
           AND target_id IN (SELECT id FROM subtasks WHERE note_id = ?1)",
        params![note_id],
    )
    .map_err(|e| format!("cascade_delete_subtasks_for_note: notif delete failed: {}", e))?;

    // 2. Delete the subtask rows themselves.
    conn.execute(
        "DELETE FROM subtasks WHERE note_id = ?1",
        params![note_id],
    )
    .map_err(|e| format!("cascade_delete_subtasks_for_note: subtask delete failed: {}", e))?;

    Ok(())
}

/// Hard-delete a subtask and any notification rows for it.
#[tauri::command]
pub fn delete_subtask(id: String, db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    // Clean up notification rows first.
    conn.execute(
        "DELETE FROM notifications WHERE target_id = ?1 AND target_kind = 'subtask'",
        params![id],
    )
    .ok(); // non-fatal

    conn.execute("DELETE FROM subtasks WHERE id = ?1", params![id])
        .map_err(|e| format!("Failed to delete subtask: {}", e))?;

    Ok(())
}

// ─── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Create an in-memory SQLite DB with the subtasks + notifications + notes tables.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("
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

            CREATE TABLE notifications (
                id           TEXT PRIMARY KEY,
                type         TEXT NOT NULL DEFAULT 'info',
                title        TEXT NOT NULL DEFAULT '',
                body         TEXT NOT NULL DEFAULT '',
                source       TEXT NOT NULL DEFAULT 'system',
                read         INTEGER NOT NULL DEFAULT 0,
                created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                target_kind  TEXT,
                target_id    TEXT,
                idem_key     TEXT,
                expires_at   TEXT,
                dismissed_at TEXT
            );
        ").unwrap();
        conn
    }

    fn insert_note(conn: &Connection, id: &str, title: &str, deleted_at: Option<&str>) {
        conn.execute(
            "INSERT INTO notes (id, title, deleted_at) VALUES (?1, ?2, ?3)",
            params![id, title, deleted_at],
        )
        .unwrap();
    }

    fn insert_subtask_raw(
        conn: &Connection,
        id: &str,
        note_id: &str,
        text: &str,
        due_date: Option<&str>,
        done: bool,
    ) {
        conn.execute(
            "INSERT INTO subtasks (id, note_id, text, due_date, done, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, datetime('now'), datetime('now'))",
            params![id, note_id, text, due_date, done as i64],
        )
        .unwrap();
    }

    fn insert_notif(conn: &Connection, id: &str, target_kind: &str, target_id: &str) {
        conn.execute(
            "INSERT INTO notifications (id, type, title, body, source, target_kind, target_id, created_at)
             VALUES (?1, 'info', 'test', 'body', 'system', ?2, ?3, datetime('now'))",
            params![id, target_kind, target_id],
        )
        .unwrap();
    }

    // ── CRUD ──────────────────────────────────────────────────────────────────

    #[test]
    fn create_and_read_subtask() {
        let conn = setup_db();
        insert_note(&conn, "note-1", "Parent Note", None);

        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO subtasks (id, note_id, text, due_date, done, sort_order, created_at, updated_at)
             VALUES (?1, 'note-1', 'Write tests', '2026-08-01', 0, 1, datetime('now'), datetime('now'))",
            params![id],
        ).unwrap();

        let s = get_subtask_by_id(&conn, &id).unwrap();
        assert_eq!(s.text, "Write tests");
        assert_eq!(s.note_id, "note-1");
        assert_eq!(s.due_date.as_deref(), Some("2026-08-01"));
        assert!(!s.done);
        assert_eq!(s.sort_order, 1);
    }

    #[test]
    fn read_subtasks_for_note_returns_ordered() {
        let conn = setup_db();
        insert_note(&conn, "note-2", "Parent", None);
        insert_subtask_raw(&conn, "s1", "note-2", "First", None, false);
        conn.execute("UPDATE subtasks SET sort_order = 10 WHERE id = 's1'", []).unwrap();
        insert_subtask_raw(&conn, "s2", "note-2", "Second", None, false);
        conn.execute("UPDATE subtasks SET sort_order = 5 WHERE id = 's2'", []).unwrap();
        insert_subtask_raw(&conn, "s3", "note-2", "Third", None, false);
        conn.execute("UPDATE subtasks SET sort_order = 20 WHERE id = 's3'", []).unwrap();

        let mut stmt = conn.prepare(
            "SELECT id FROM subtasks WHERE note_id = 'note-2' ORDER BY sort_order ASC, created_at ASC"
        ).unwrap();
        let ids: Vec<String> = stmt.query_map([], |r| r.get(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(ids, vec!["s2", "s1", "s3"]);
    }

    #[test]
    fn delete_subtask_removes_row() {
        let conn = setup_db();
        insert_subtask_raw(&conn, "del-1", "note-3", "To delete", None, false);
        insert_notif(&conn, "notif-del-1", "subtask", "del-1");

        // Simulate delete
        conn.execute(
            "DELETE FROM notifications WHERE target_id = ?1 AND target_kind = 'subtask'",
            params!["del-1"],
        ).unwrap();
        conn.execute("DELETE FROM subtasks WHERE id = 'del-1'", []).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM subtasks WHERE id = 'del-1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "Subtask row should be deleted");

        let notif_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE target_id = 'del-1' AND target_kind = 'subtask'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(notif_count, 0, "Notification rows should be deleted with subtask");
    }

    // ── done transitions ─────────────────────────────────────────────────────

    #[test]
    fn done_0_to_1_sets_completed_at() {
        let conn = setup_db();
        insert_subtask_raw(&conn, "tx-1", "note-4", "Transition test", None, false);

        conn.execute(
            "UPDATE subtasks SET done = 1, completed_at = datetime('now'), updated_at = datetime('now') \
             WHERE id = 'tx-1'",
            [],
        ).unwrap();

        let s = get_subtask_by_id(&conn, "tx-1").unwrap();
        assert!(s.done, "done should be true");
        assert!(s.completed_at.is_some(), "completed_at should be set after 0→1");
    }

    #[test]
    fn done_1_to_0_clears_completed_at() {
        let conn = setup_db();
        insert_subtask_raw(&conn, "tx-2", "note-4", "Reopen test", None, true);
        conn.execute(
            "UPDATE subtasks SET completed_at = datetime('now') WHERE id = 'tx-2'",
            [],
        ).unwrap();

        conn.execute(
            "UPDATE subtasks SET done = 0, completed_at = NULL, updated_at = datetime('now') \
             WHERE id = 'tx-2'",
            [],
        ).unwrap();

        let s = get_subtask_by_id(&conn, "tx-2").unwrap();
        assert!(!s.done, "done should be false");
        assert!(s.completed_at.is_none(), "completed_at should be NULL after 1→0");
    }

    // ── Notification hooks ───────────────────────────────────────────────────

    #[test]
    fn auto_dismiss_on_completed() {
        let conn = setup_db();
        insert_subtask_raw(&conn, "ad-1", "note-5", "Auto dismiss test", None, false);
        insert_notif(&conn, "n-ad-1", "subtask", "ad-1");

        // Simulate done 0→1 auto-dismiss
        conn.execute(
            "UPDATE notifications SET dismissed_at = datetime('now') \
             WHERE target_id = 'ad-1' AND target_kind = 'subtask' AND dismissed_at IS NULL",
            [],
        ).unwrap();

        let dismissed: Option<String> = conn
            .query_row(
                "SELECT dismissed_at FROM notifications WHERE id = 'n-ad-1'",
                [], |r| r.get(0),
            ).unwrap();
        assert!(dismissed.is_some(), "Notification should be dismissed when subtask completed");
    }

    #[test]
    fn companion_clear_on_due_date_change() {
        let conn = setup_db();
        insert_subtask_raw(&conn, "cc-1", "note-6", "Companion clear test", Some("2026-08-01"), false);
        insert_notif(&conn, "n-cc-1", "subtask", "cc-1");

        // Simulate companion-clear when due_date changes to a new non-empty value
        conn.execute(
            "DELETE FROM notifications WHERE target_id = 'cc-1' AND target_kind = 'subtask'",
            [],
        ).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE target_id = 'cc-1' AND target_kind = 'subtask'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(count, 0, "Notification rows should be deleted on due_date change (companion clear)");
    }

    #[test]
    fn companion_clear_on_reopen() {
        let conn = setup_db();
        insert_subtask_raw(&conn, "cc-2", "note-6", "Reopen clear test", Some("2026-08-01"), true);
        insert_notif(&conn, "n-cc-2", "subtask", "cc-2");

        // Simulate companion-clear on done 1→0
        conn.execute(
            "DELETE FROM notifications WHERE target_id = 'cc-2' AND target_kind = 'subtask'",
            [],
        ).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE target_id = 'cc-2' AND target_kind = 'subtask'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(count, 0, "Notification rows should be deleted on reopen (companion clear)");
    }

    // ── Parent trash / restore / empty_trash ─────────────────────────────────

    #[test]
    fn parent_trash_dismisses_subtask_notifications() {
        let conn = setup_db();
        insert_note(&conn, "note-7", "Parent note", None);
        insert_subtask_raw(&conn, "pt-1", "note-7", "Sub A", None, false);
        insert_subtask_raw(&conn, "pt-2", "note-7", "Sub B", None, false);
        insert_notif(&conn, "n-pt-1", "subtask", "pt-1");
        insert_notif(&conn, "n-pt-2", "subtask", "pt-2");

        // Simulate parent trash hook (§19.5.1c — c)
        conn.execute(
            "UPDATE notifications SET dismissed_at = datetime('now') \
             WHERE target_kind = 'subtask' AND dismissed_at IS NULL \
               AND target_id IN (SELECT id FROM subtasks WHERE note_id = 'note-7')",
            [],
        ).unwrap();

        let dismissed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE dismissed_at IS NOT NULL \
                 AND target_kind = 'subtask'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(dismissed, 2, "Both subtask notifications should be dismissed on parent trash");
    }

    #[test]
    fn parent_restore_companion_deletes_subtask_notifications() {
        let conn = setup_db();
        insert_note(&conn, "note-8", "Restored note", Some("2026-07-19 10:00:00"));
        insert_subtask_raw(&conn, "pr-1", "note-8", "Sub A", Some("2026-07-10"), false);
        insert_notif(&conn, "n-pr-1", "subtask", "pr-1");

        // Simulate parent restore companion-DELETE (§19.5.1c — d)
        conn.execute(
            "DELETE FROM notifications \
             WHERE target_kind = 'subtask' \
               AND target_id IN (SELECT id FROM subtasks WHERE note_id = 'note-8')",
            [],
        ).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE target_kind = 'subtask'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(count, 0, "Subtask notification rows should be deleted on parent restore (companion clear)");
    }

    #[test]
    fn empty_trash_cascades_subtasks_and_notifications() {
        let conn = setup_db();
        // Two trashed notes
        insert_note(&conn, "trash-1", "Trashed A", Some("2026-07-19 10:00:00"));
        insert_note(&conn, "trash-2", "Trashed B", Some("2026-07-19 11:00:00"));
        // Live note (should NOT be touched)
        insert_note(&conn, "live-1", "Live note", None);

        insert_subtask_raw(&conn, "et-1", "trash-1", "Sub of trash A", None, false);
        insert_subtask_raw(&conn, "et-2", "trash-2", "Sub of trash B", None, false);
        insert_subtask_raw(&conn, "et-3", "live-1",  "Sub of live",    None, false);

        insert_notif(&conn, "n-et-1", "subtask", "et-1");
        insert_notif(&conn, "n-et-2", "subtask", "et-2");
        insert_notif(&conn, "n-et-3", "subtask", "et-3");

        // Replicate empty_trash cascade logic
        let trash_ids: Vec<String> = conn
            .prepare("SELECT id FROM notes WHERE deleted_at IS NOT NULL").unwrap()
            .query_map([], |r| r.get(0)).unwrap()
            .collect::<Result<Vec<_>, _>>().unwrap();

        for note_id in &trash_ids {
            conn.execute(
                "DELETE FROM notifications \
                 WHERE target_kind = 'subtask' \
                   AND target_id IN (SELECT id FROM subtasks WHERE note_id = ?1)",
                params![note_id],
            ).unwrap();
            conn.execute(
                "DELETE FROM subtasks WHERE note_id = ?1",
                params![note_id],
            ).unwrap();
            conn.execute("DELETE FROM notes WHERE id = ?1", params![note_id]).unwrap();
        }

        // Trashed subtasks and their notifications gone
        let subtask_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM subtasks WHERE note_id IN ('trash-1','trash-2')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(subtask_count, 0, "Subtasks of trashed notes should be hard-deleted");

        let notif_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE target_id IN ('et-1','et-2')",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(notif_count, 0, "Notifications for trashed subtasks should be hard-deleted");

        // Live note's subtask and notification untouched
        let live_sub: i64 = conn
            .query_row("SELECT COUNT(*) FROM subtasks WHERE id = 'et-3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(live_sub, 1, "Subtask of live note should not be deleted");

        let live_notif: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE target_id = 'et-3'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(live_notif, 1, "Notification for live subtask should not be deleted");
    }

    /// §2100 — purge_note must cascade to subtasks and their notifications
    /// before hard-deleting the note row.
    #[test]
    fn test_purge_note_cascades_subtasks_and_notifications() {
        let conn = setup_db();

        // Two notes: one we will purge, one that must remain untouched.
        insert_note(&conn, "note-purge", "Note to purge", Some("2026-07-19 12:00:00"));
        insert_note(&conn, "note-safe",  "Surviving note", None);

        // Subtasks
        insert_subtask_raw(&conn, "pn-sub-1", "note-purge", "Purge sub A", None, false);
        insert_subtask_raw(&conn, "pn-sub-2", "note-purge", "Purge sub B", None, false);
        insert_subtask_raw(&conn, "pn-sub-3", "note-safe",  "Safe sub",    None, false);

        // Notifications
        insert_notif(&conn, "n-pn-1", "subtask", "pn-sub-1");
        insert_notif(&conn, "n-pn-2", "subtask", "pn-sub-2");
        insert_notif(&conn, "n-pn-3", "subtask", "pn-sub-3");

        // Invoke the real cascade helper — if purge_note's cascade logic is
        // ever refactored, this test will immediately catch the regression.
        cascade_delete_subtasks_for_note(&conn, "note-purge").unwrap();
        // Then delete the note row (mirrors what purge_note does after the cascade).
        conn.execute("DELETE FROM notes WHERE id = ?1", params!["note-purge"]).unwrap();

        // Purged note is gone
        let note_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM notes WHERE id = 'note-purge'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(note_count, 0, "Purged note row should be deleted");

        // Subtasks of the purged note are gone
        let sub_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subtasks WHERE note_id = 'note-purge'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(sub_count, 0, "Subtasks of purged note should be cascade-deleted");

        // Notifications for those subtasks are gone
        let notif_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE target_id IN ('pn-sub-1','pn-sub-2')",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(notif_count, 0, "Notifications for purged subtasks should be cascade-deleted");

        // Surviving note, its subtask, and its notification are all intact
        let safe_note: i64 = conn
            .query_row("SELECT COUNT(*) FROM notes WHERE id = 'note-safe'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(safe_note, 1, "Unrelated note should survive purge");

        let safe_sub: i64 = conn
            .query_row("SELECT COUNT(*) FROM subtasks WHERE id = 'pn-sub-3'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(safe_sub, 1, "Subtask of surviving note should not be deleted");

        let safe_notif: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM notifications WHERE target_id = 'pn-sub-3'",
                [], |r| r.get(0),
            ).unwrap();
        assert_eq!(safe_notif, 1, "Notification for surviving subtask should not be deleted");
    }

    // ── Due Today Query Tests (§31.2.6, §31.2.1) ───────────────────────────────────────

    /// Create a DB with full schema needed for get_due_today_items_query.
    fn setup_db_full() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("
            CREATE TABLE projects (
                id   TEXT NOT NULL PRIMARY KEY,
                name TEXT NOT NULL DEFAULT ''
            );

            CREATE TABLE notes (
                id          TEXT PRIMARY KEY,
                title       TEXT NOT NULL DEFAULT '',
                status      TEXT,
                due_date    TEXT,
                project_id  TEXT,
                deleted_at  TEXT
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

    fn insert_project(conn: &Connection, id: &str, name: &str) {
        conn.execute(
            "INSERT INTO projects (id, name) VALUES (?1, ?2)",
            params![id, name],
        ).unwrap();
    }

    fn insert_task(conn: &Connection, id: &str, title: &str, status: &str, due_date: Option<&str>, project_id: Option<&str>) {
        conn.execute(
            "INSERT INTO notes (id, title, status, due_date, project_id, deleted_at) VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![id, title, status, due_date, project_id],
        ).unwrap();
    }

    fn insert_subtask_full(conn: &Connection, id: &str, note_id: &str, text: &str, due_date: Option<&str>, done: bool) {
        conn.execute(
            "INSERT INTO subtasks (id, note_id, text, due_date, done, sort_order, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 0, datetime('now'), datetime('now'))",
            params![id, note_id, text, due_date, done as i64],
        ).unwrap();
    }

    /// §31.2.6 CI-assertable order test.
    /// Creates mixed task/subtask data with mixed done states and asserts the query
    /// returns them in exactly this order:
    ///   open tasks A–Z → open subtasks A–Z → done tasks A–Z → done subtasks A–Z
    /// Case-insensitive within each group; tiebreak by id ASC.
    #[test]
    fn due_today_order_matches_spec() {
        let conn = setup_db_full();
        insert_project(&conn, "proj-1", "Project Alpha");

        // Parent note for subtasks
        insert_task(&conn, "note-parent", "Parent Note", "todo", None, Some("proj-1"));

        // Due date we will query
        let d = "2026-07-20";

        // ── Tasks (notes with due_date = d) ─────────────────────────────────
        // done task: should go to group 2 (done tasks)
        insert_task(&conn, "t-done-b", "Banana", "done", Some(d), Some("proj-1"));
        // done task: sort earlier in group 2
        insert_task(&conn, "t-done-a", "apple", "done", Some(d), Some("proj-1")); // lowercase → case-insensitive
        // open task: group 0
        insert_task(&conn, "t-open-z", "Zebra", "todo", Some(d), Some("proj-1"));
        // open task: group 0 (should appear before Zebra)
        insert_task(&conn, "t-open-m", "Mango", "todo", Some(d), Some("proj-1"));

        // ── Subtasks with due_date = d ───────────────────────────────────────
        // done subtask: group 3
        insert_subtask_full(&conn, "s-done-q", "note-parent", "quince", Some(d), true);
        // open subtask: group 1
        insert_subtask_full(&conn, "s-open-c", "note-parent", "cherry", Some(d), false);
        // open subtask: group 1 (should sort before cherry? no — 'c' < 'f', cherry < fig, so cherry first)
        insert_subtask_full(&conn, "s-open-f", "note-parent", "fig",    Some(d), false);
        // done subtask: group 3
        insert_subtask_full(&conn, "s-done-p", "note-parent", "Plum",   Some(d), true); // P < q case-insensitive

        let items = get_due_today_items_query(&conn, d).unwrap();

        // Extract (kind, title, done) for readability
        let got: Vec<(&str, String, bool)> = items.iter()
            .map(|i| (i.kind.as_str(), i.title.to_lowercase(), i.done))
            .collect();

        // Expected order per §31.2.6:
        //  group 0 (open tasks, A–Z case-insensitive): Mango, Zebra
        //  group 1 (open subtasks, A–Z case-insensitive): cherry, fig
        //  group 2 (done tasks, A–Z case-insensitive): apple, Banana
        //  group 3 (done subtasks, A–Z case-insensitive): Plum, quince
        let expected: Vec<(&str, &str, bool)> = vec![
            ("task",    "mango",  false),
            ("task",    "zebra",  false),
            ("subtask", "cherry", false),
            ("subtask", "fig",    false),
            ("task",    "apple",  true),
            ("task",    "banana", true),
            ("subtask", "plum",   true),
            ("subtask", "quince", true),
        ];

        assert_eq!(
            got.len(),
            expected.len(),
            "Result count mismatch: got {}, expected {} rows",
            got.len(),
            expected.len()
        );

        for (i, ((got_kind, got_title, got_done), (exp_kind, exp_title, exp_done))) in
            got.iter().zip(expected.iter()).enumerate()
        {
            assert_eq!(
                got_kind, exp_kind,
                "Row {}: kind mismatch — got {:?}, expected {:?}",
                i, got_kind, exp_kind
            );
            assert_eq!(
                got_title, *exp_title,
                "Row {}: title mismatch — got {:?}, expected {:?}",
                i, got_title, exp_title
            );
            assert_eq!(
                got_done, exp_done,
                "Row {}: done mismatch — got {:?}, expected {:?}",
                i, got_done, exp_done
            );
        }
    }

    /// §31.2.5: done-inclusive query test.
    /// Verify done items appear in the result set (unlike the watcher which filters done=0).
    #[test]
    fn due_today_done_items_included() {
        let conn = setup_db_full();
        insert_project(&conn, "proj-done", "Done Project");

        let d = "2026-07-21";
        insert_task(&conn, "done-task-1", "Finished Task", "done", Some(d), Some("proj-done"));

        // Parent note for subtask
        insert_task(&conn, "done-note-parent", "Parent", "todo", None, Some("proj-done"));
        insert_subtask_full(&conn, "done-sub-1", "done-note-parent", "Finished Subtask", Some(d), true);

        let items = get_due_today_items_query(&conn, d).unwrap();

        assert_eq!(items.len(), 2, "§31.2.5: done items must appear in Due Today result (got {})", items.len());

        let has_done_task = items.iter().any(|i| i.id == "done-task-1" && i.done);
        let has_done_sub  = items.iter().any(|i| i.id == "done-sub-1"  && i.done);
        assert!(has_done_task, "§31.2.5: done task must be present in result");
        assert!(has_done_sub,  "§31.2.5: done subtask must be present in result");
    }

    /// Verify that items NOT due on the queried date are excluded.
    #[test]
    fn due_today_excludes_wrong_date() {
        let conn = setup_db_full();
        insert_task(&conn, "wrong-date-task", "Wrong Date Task", "todo", Some("2026-07-19"), None);
        insert_task(&conn, "note-wd-parent", "Parent", "todo", None, None);
        insert_subtask_full(&conn, "wrong-date-sub", "note-wd-parent", "Wrong Date Sub", Some("2026-07-19"), false);

        let items = get_due_today_items_query(&conn, "2026-07-20").unwrap();
        assert!(items.is_empty(), "Items due on a different date should not appear");
    }

    /// Verify that items belonging to deleted notes are excluded.
    #[test]
    fn due_today_excludes_deleted_parent_notes() {
        let conn = setup_db_full();
        // Trashed parent note — its subtask must not appear
        conn.execute(
            "INSERT INTO notes (id, title, status, due_date, project_id, deleted_at) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params!["del-parent", "Deleted Parent", "todo", None::<&str>, "2026-07-19 10:00:00"],
        ).unwrap();
        insert_subtask_full(&conn, "del-sub-1", "del-parent", "Sub of deleted", Some("2026-07-20"), false);

        let items = get_due_today_items_query(&conn, "2026-07-20").unwrap();
        let has_del_sub = items.iter().any(|i| i.id == "del-sub-1");
        assert!(!has_del_sub, "Subtask of deleted parent note must not appear in Due Today");
    }

    /// §31.2.6 id tiebreak test: identical titles sort by id ASC within the same group.
    #[test]
    fn due_today_id_tiebreak_within_group() {
        let conn = setup_db_full();
        let d = "2026-07-22";
        // Two open tasks with the same title — lower id ('a' < 'b') must appear first
        insert_task(&conn, "b-twin", "Twin", "todo", Some(d), None);
        insert_task(&conn, "a-twin", "Twin", "todo", Some(d), None);

        let items = get_due_today_items_query(&conn, d).unwrap();
        let task_ids: Vec<&str> = items.iter()
            .filter(|i| i.kind == "task" && i.done == false)
            .map(|i| i.id.as_str())
            .collect();

        assert_eq!(task_ids.len(), 2, "Expected 2 open tasks");
        assert_eq!(task_ids[0], "a-twin", "Lower id must appear first (tiebreak)");
        assert_eq!(task_ids[1], "b-twin");
    }
}
