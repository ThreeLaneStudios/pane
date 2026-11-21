// Pane — Template CRUD + Apply Commands (§32, §1.10, ADR-0058, #193)
// spec-covered-by: #193 (§32, §1.10, ADR-0058)
//
// Templates are named, reusable snapshots of note content: the body markdown
// (including component sentinels) plus checklist item payload. They live in the
// dedicated `templates` table (ADR-0058).
//
// Key design decisions (see ADR-0058 + §32):
//   - `apply_template` is a single DB transaction: body write + subtask inserts
//     land atomically or not at all. Partial apply = broken state.
//   - Backend owns date-stamping of due-today sentinels at apply time.
//     Amendment per Matt 2026-07-20: frontend passes template content through
//     untouched; this file stamps `data-date` with today's local date.
//   - No FOREIGN KEY constraints on any column (CRR table pattern, #493).
//   - Collision detection (§32.2.2) is app-level, not schema-level (CRR sync
//     can produce same-named rows from two devices; DB cannot enforce uniqueness).

use crate::database::DbState;
use chrono::Local;
use regex::Regex;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

// ─── Structs ─────────────────────────────────────────────────────────────────

/// A row from the `templates` table.
///
/// `checklist_json` is opaque TEXT — a JSON array of `{"text":"...","sort_order":N}`
/// entries (§32.1). Never queried by field; parsed at apply time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub content: String,
    pub checklist_json: String,
    pub created_at: String,
    pub updated_at: String,
}

/// A single checklist entry from `checklist_json` — deserialized during apply.
///
/// Per §32.1: only `text` and `sort_order` are stored. `done`, `completed_at`,
/// and `due_date` are NEVER stored (stale dates / done-state are instance state,
/// not template content — Matt 2026-07-20 lock).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChecklistItem {
    text: String,
    sort_order: i64,
}

// ─── Row mapper ──────────────────────────────────────────────────────────────

fn row_to_template(row: &rusqlite::Row<'_>) -> rusqlite::Result<Template> {
    Ok(Template {
        id: row.get(0)?,
        name: row.get(1)?,
        content: row.get(2)?,
        checklist_json: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

// ─── Internal helpers ────────────────────────────────────────────────────────

/// Stamp `data-date` on all `data-component="dueToday"` sentinels in `content`
/// with `date` (YYYY-MM-DD).
///
/// §32.1: templates store due-today sentinels without `data-date` (stripped at
/// save time). §32.3.2: apply stamps today's local date on every sentinel so a
/// dateless sentinel never reaches the rendered note (§31.2.3 anchored-not-live
/// null-date rule: error state, never silent-now()).
///
/// Defensive: if a sentinel already has `data-date` (shouldn't happen in a
/// well-formed template), the stale value is replaced with `date`.
///
/// Sentinel form in stored markdown (from DueToday.ts renderHTML):
///   - no date (standard template): `<div data-component="dueToday"></div>`
///   - stale date (defensive):      `<div data-component="dueToday" data-date="YYYY-MM-DD"></div>`
pub(crate) fn stamp_due_today_sentinels(content: &str, date: &str) -> String {
    // Single regex: matches the dueToday opening tag with or without data-date.
    // Group captures the entire tag content; closure emits a freshly-dated tag.
    let re = Regex::new(
        r#"<div\s+data-component="dueToday"(?:\s+data-date="[^"]*")?>"#,
    )
    .expect("[TEMPLATES] stamp_due_today_sentinels: regex compile error");

    re.replace_all(content, |_: &regex::Captures| {
        format!(r#"<div data-component="dueToday" data-date="{}">"#, date)
    })
    .to_string()
}

// ─── Tauri Commands ──────────────────────────────────────────────────────────

/// List all templates ordered by `updated_at` DESC.
///
/// Used by the template picker (Cmd+Shift+T surface) and Settings → Templates tab.
#[tauri::command]
pub fn list_templates(db: State<DbState>) -> Result<Vec<Template>, String> {
    eprintln!("[TEMPLATES] list_templates called");
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, name, content, checklist_json, created_at, updated_at
             FROM templates
             ORDER BY updated_at DESC",
        )
        .map_err(|e| format!("list_templates: prepare failed: {}", e))?;

    let rows = stmt
        .query_map([], row_to_template)
        .map_err(|e| format!("list_templates: query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("list_templates: collect failed: {}", e))?;

    eprintln!("[TEMPLATES] list_templates → {} row(s)", rows.len());
    Ok(rows)
}

/// Fetch a single template by id.
///
/// Used on the apply path — caller supplies `template_id`, command reads the row.
#[tauri::command]
pub fn get_template(id: String, db: State<DbState>) -> Result<Template, String> {
    eprintln!("[TEMPLATES] get_template called: id={}", id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    conn.query_row(
        "SELECT id, name, content, checklist_json, created_at, updated_at
         FROM templates WHERE id = ?1",
        params![id],
        row_to_template,
    )
    .map_err(|e| format!("get_template: not found (id={}): {}", id, e))
}

/// Create a new template row with a fresh UUID.
///
/// `content` must already have `data-date` stripped from due-today sentinels
/// (the editor serializer does this at save-as-template time, §32.1).
/// `checklist_json` is the JSON array of `{"text":"...","sort_order":N}` items
/// captured from the note's `subtasks` rows at save time.
#[tauri::command]
pub fn save_template(
    name: String,
    content: String,
    checklist_json: String,
    db: State<DbState>,
) -> Result<Template, String> {
    eprintln!("[TEMPLATES] save_template called: name={}", name);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    let id = Uuid::new_v4().to_string();

    conn.execute(
        "INSERT INTO templates (id, name, content, checklist_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))",
        params![id, name, content, checklist_json],
    )
    .map_err(|e| format!("save_template: insert failed: {}", e))?;

    conn.query_row(
        "SELECT id, name, content, checklist_json, created_at, updated_at
         FROM templates WHERE id = ?1",
        params![id],
        row_to_template,
    )
    .map_err(|e| format!("save_template: read-back failed: {}", e))
}

/// Update an existing template's content and checklist_json in place.
///
/// `updated_at` is caller-supplied so the LWW CRR merge policy applies correctly
/// across devices (same-id row with newer `updated_at` wins on sync).
/// `name` is NOT changed here — use `rename_template` for name changes.
#[tauri::command]
pub fn overwrite_template(
    id: String,
    content: String,
    checklist_json: String,
    updated_at: String,
    db: State<DbState>,
) -> Result<Template, String> {
    eprintln!("[TEMPLATES] overwrite_template called: id={}", id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    conn.execute(
        "UPDATE templates SET content = ?1, checklist_json = ?2, updated_at = ?3 WHERE id = ?4",
        params![content, checklist_json, updated_at, id],
    )
    .map_err(|e| format!("overwrite_template: update failed (id={}): {}", id, e))?;

    conn.query_row(
        "SELECT id, name, content, checklist_json, created_at, updated_at
         FROM templates WHERE id = ?1",
        params![id],
        row_to_template,
    )
    .map_err(|e| format!("overwrite_template: read-back failed (id={}): {}", id, e))
}

/// Rename a template in place. Applies the same collision flow as save-as-template
/// (§32.2.2) — collision detection is enforced by the caller before this command runs.
#[tauri::command]
pub fn rename_template(id: String, name: String, db: State<DbState>) -> Result<Template, String> {
    eprintln!("[TEMPLATES] rename_template called: id={}, name={}", id, name);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    conn.execute(
        "UPDATE templates SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![name, id],
    )
    .map_err(|e| format!("rename_template: update failed (id={}): {}", id, e))?;

    conn.query_row(
        "SELECT id, name, content, checklist_json, created_at, updated_at
         FROM templates WHERE id = ?1",
        params![id],
        row_to_template,
    )
    .map_err(|e| format!("rename_template: read-back failed (id={}): {}", id, e))
}

/// Hard-delete a template row.
///
/// No soft-delete — templates are not notes; nothing references them (§32.4).
/// Applied notes are copies; deleting a template never touches prior applied notes.
#[tauri::command]
pub fn delete_template(id: String, db: State<DbState>) -> Result<(), String> {
    eprintln!("[TEMPLATES] delete_template called: id={}", id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    conn.execute("DELETE FROM templates WHERE id = ?1", params![id])
        .map_err(|e| format!("delete_template: delete failed (id={}): {}", id, e))?;

    eprintln!("[TEMPLATES] delete_template complete: id={}", id);
    Ok(())
}

/// Case-insensitive name search — used for collision detection before save/rename (§32.2.2).
///
/// Zero matches → create new. One match → Overwrite/Rename prompt. Two+ matches →
/// list picker (sync can produce duplicate-named rows from independent device creates).
#[tauri::command]
pub fn list_templates_by_name(
    name: String,
    db: State<DbState>,
) -> Result<Vec<Template>, String> {
    eprintln!("[TEMPLATES] list_templates_by_name called: name={}", name);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, name, content, checklist_json, created_at, updated_at
             FROM templates
             WHERE lower(name) = lower(?1)
             ORDER BY updated_at DESC",
        )
        .map_err(|e| format!("list_templates_by_name: prepare failed: {}", e))?;

    let rows = stmt
        .query_map(params![name], row_to_template)
        .map_err(|e| format!("list_templates_by_name: query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("list_templates_by_name: collect failed: {}", e))?;

    eprintln!(
        "[TEMPLATES] list_templates_by_name '{}' → {} match(es)",
        name,
        rows.len()
    );
    Ok(rows)
}

/// Apply a template to a note — atomic body write + subtask inserts.
///
/// # Steps (§32.3.2)
///
/// 1. Read the template's `content` and `checklist_json`.
/// 2. **Stamp `data-date`** on every `data-component="dueToday"` sentinel in the
///    content with today's local calendar date (YYYY-MM-DD). Backend owns this
///    per the Matt 2026-07-20 amendment. Templates store sentinels dateless;
///    a dateless sentinel must never reach the rendered note (§31.2.3).
/// 3. Write the stamped content to `notes.body` for `note_id`.
/// 4. Parse `checklist_json` → insert fresh `subtasks` rows keyed to `note_id`,
///    appending after the note's current max `sort_order` (§32.3.2 append semantics).
///    Each new row gets: new UUID, `text` from template, `done = 0`, `due_date = NULL`,
///    `completed_at = NULL` (§32.1 — no stale done-state in templates).
///
/// Steps 3–4 execute inside a single SQLite transaction (`BEGIN ... COMMIT`).
/// On any failure the transaction is rolled back; `note_id` is left unchanged.
#[tauri::command]
pub fn apply_template(
    note_id: String,
    template_id: String,
    db: State<DbState>,
) -> Result<(), String> {
    eprintln!(
        "[TEMPLATES] apply_template called: note_id={}, template_id={}",
        note_id, template_id
    );

    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;

    // ── Step 1: Read template row ────────────────────────────────────────────
    let template = conn
        .query_row(
            "SELECT id, name, content, checklist_json, created_at, updated_at
             FROM templates WHERE id = ?1",
            params![template_id],
            row_to_template,
        )
        .map_err(|e| format!("apply_template: template not found (id={}): {}", template_id, e))?;

    // ── Step 2: Stamp due-today sentinels ────────────────────────────────────
    let today = Local::now().format("%Y-%m-%d").to_string();
    let stamped_content = stamp_due_today_sentinels(&template.content, &today);

    // ── Step 3 + 4: Atomic transaction ──────────────────────────────────────
    // Parse checklist before entering the transaction (fail fast on bad JSON).
    let checklist_items: Vec<ChecklistItem> =
        serde_json::from_str(&template.checklist_json).map_err(|e| {
            format!(
                "apply_template: invalid checklist_json (template_id={}): {} — raw: {}",
                template_id, e, template.checklist_json
            )
        })?;

    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|e| format!("apply_template: begin transaction failed: {}", e))?;

    let apply_result: Result<(), String> = (|| {
        // Write stamped content to note body
        conn.execute(
            "UPDATE notes SET body = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![stamped_content, note_id],
        )
        .map_err(|e| format!("apply_template: body write failed (note_id={}): {}", note_id, e))?;

        // Determine append offset: continue after the note's current max sort_order.
        // COALESCE to -1 so the first inserted item lands at sort_order 0.
        let max_sort_order: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM subtasks WHERE note_id = ?1",
                params![note_id],
                |row| row.get(0),
            )
            .map_err(|e| {
                format!(
                    "apply_template: max sort_order query failed (note_id={}): {}",
                    note_id, e
                )
            })?;

        // Insert fresh subtask rows — one per checklist item.
        for (i, item) in checklist_items.iter().enumerate() {
            let subtask_id = Uuid::new_v4().to_string();
            let sort_order = max_sort_order + 1 + i as i64;

            conn.execute(
                "INSERT INTO subtasks \
                 (id, note_id, text, due_date, done, completed_at, sort_order, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, NULL, 0, NULL, ?4, datetime('now'), datetime('now'))",
                params![subtask_id, note_id, item.text, sort_order],
            )
            .map_err(|e| {
                format!(
                    "apply_template: subtask insert failed (item {}, note_id={}): {}",
                    i, note_id, e
                )
            })?;
        }

        Ok(())
    })();

    match apply_result {
        Ok(()) => {
            conn.execute_batch("COMMIT")
                .map_err(|e| format!("apply_template: commit failed: {}", e))?;
            eprintln!(
                "[TEMPLATES] apply_template complete: note_id={}, {} subtask(s) inserted",
                note_id,
                checklist_items.len()
            );
            Ok(())
        }
        Err(e) => {
            conn.execute_batch("ROLLBACK").ok(); // best-effort rollback
            eprintln!(
                "[TEMPLATES] apply_template ROLLED BACK (note_id={}): {}",
                note_id, e
            );
            Err(e)
        }
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Minimal in-memory DB with `templates`, `notes`, and `subtasks` tables.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE templates (
                id             TEXT NOT NULL PRIMARY KEY,
                name           TEXT NOT NULL DEFAULT '',
                content        TEXT NOT NULL DEFAULT '',
                checklist_json TEXT NOT NULL DEFAULT '[]',
                created_at     TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
            );
            CREATE INDEX IF NOT EXISTS idx_templates_name ON templates(name);

            CREATE TABLE notes (
                id         TEXT NOT NULL PRIMARY KEY,
                title      TEXT NOT NULL DEFAULT '',
                body       TEXT,
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
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
            );",
        )
        .unwrap();
        conn
    }

    fn insert_template(
        conn: &Connection,
        id: &str,
        name: &str,
        content: &str,
        checklist_json: &str,
    ) {
        conn.execute(
            "INSERT INTO templates (id, name, content, checklist_json, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))",
            params![id, name, content, checklist_json],
        )
        .unwrap();
    }

    fn insert_note(conn: &Connection, id: &str, body: Option<&str>) {
        conn.execute(
            "INSERT INTO notes (id, title, body) VALUES (?1, 'Test Note', ?2)",
            params![id, body],
        )
        .unwrap();
    }

    fn insert_subtask(conn: &Connection, id: &str, note_id: &str, text: &str, sort_order: i64) {
        conn.execute(
            "INSERT INTO subtasks (id, note_id, text, sort_order) VALUES (?1, ?2, ?3, ?4)",
            params![id, note_id, text, sort_order],
        )
        .unwrap();
    }

    // ── stamp_due_today_sentinels ────────────────────────────────────────────

    #[test]
    fn stamp_no_sentinels_returns_content_unchanged() {
        let content = "<p>No due-today blocks here.</p>";
        let result = stamp_due_today_sentinels(content, "2026-07-20");
        assert_eq!(result, content);
    }

    #[test]
    fn stamp_adds_date_to_dateless_sentinel() {
        let content = r#"<p>Hello</p><div data-component="dueToday"></div><p>World</p>"#;
        let result = stamp_due_today_sentinels(content, "2026-07-20");
        assert!(
            result.contains(r#"<div data-component="dueToday" data-date="2026-07-20">"#),
            "Expected stamped sentinel, got: {}",
            result
        );
        // Other content intact
        assert!(result.contains("<p>Hello</p>"));
        assert!(result.contains("<p>World</p>"));
    }

    #[test]
    fn stamp_replaces_stale_date_on_sentinel() {
        // Defensive: template might have a stale date (shouldn't happen but handled)
        let content = r#"<div data-component="dueToday" data-date="2020-01-01"></div>"#;
        let result = stamp_due_today_sentinels(content, "2026-07-20");
        assert!(
            result.contains(r#"data-date="2026-07-20""#),
            "Expected new date, got: {}",
            result
        );
        assert!(
            !result.contains(r#"data-date="2020-01-01""#),
            "Old date should be gone, got: {}",
            result
        );
    }

    #[test]
    fn stamp_handles_multiple_sentinels() {
        let content = concat!(
            r#"<div data-component="dueToday"></div>"#,
            "<p>Mid text</p>",
            r#"<div data-component="dueToday"></div>"#,
        );
        let result = stamp_due_today_sentinels(content, "2026-07-21");
        let count = result
            .matches(r#"data-date="2026-07-21""#)
            .count();
        assert_eq!(count, 2, "Both sentinels should be stamped, got: {}", result);
    }

    #[test]
    fn stamp_does_not_modify_checklist_sentinel() {
        // Checklist divs must never be touched by the stamp function
        let content = r#"<div data-component="checklist"></div><div data-component="dueToday"></div>"#;
        let result = stamp_due_today_sentinels(content, "2026-07-20");
        assert!(
            result.contains(r#"<div data-component="checklist"></div>"#),
            "Checklist sentinel must be untouched, got: {}",
            result
        );
        assert!(
            result.contains(r#"data-component="dueToday" data-date="2026-07-20""#),
            "dueToday sentinel should be stamped, got: {}",
            result
        );
    }

    // ── list_templates ───────────────────────────────────────────────────────

    #[test]
    fn list_templates_empty_returns_vec() {
        let conn = setup_db();
        let mut stmt = conn
            .prepare(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates ORDER BY updated_at DESC",
            )
            .unwrap();
        let rows: Vec<Template> = stmt
            .query_map([], row_to_template)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn list_templates_returns_all_ordered_by_updated_at_desc() {
        let conn = setup_db();
        insert_template(&conn, "t1", "Alpha", "# Alpha", "[]");
        // Push t1's updated_at back so t2 is newer
        conn.execute(
            "UPDATE templates SET updated_at = '2026-01-01 00:00:00' WHERE id = 't1'",
            [],
        )
        .unwrap();
        insert_template(&conn, "t2", "Beta", "# Beta", "[]");

        let mut stmt = conn
            .prepare(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates ORDER BY updated_at DESC",
            )
            .unwrap();
        let rows: Vec<Template> = stmt
            .query_map([], row_to_template)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "t2", "Newer template should come first");
        assert_eq!(rows[1].id, "t1");
    }

    // ── get_template ─────────────────────────────────────────────────────────

    #[test]
    fn get_template_returns_correct_row() {
        let conn = setup_db();
        insert_template(&conn, "tmpl-get", "My Template", "# Body", r#"[{"text":"Task A","sort_order":0}]"#);

        let t = conn
            .query_row(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates WHERE id = ?1",
                params!["tmpl-get"],
                row_to_template,
            )
            .unwrap();

        assert_eq!(t.id, "tmpl-get");
        assert_eq!(t.name, "My Template");
        assert_eq!(t.content, "# Body");
        assert!(t.checklist_json.contains("Task A"));
    }

    #[test]
    fn get_template_missing_returns_error() {
        let conn = setup_db();
        let result = conn.query_row(
            "SELECT id, name, content, checklist_json, created_at, updated_at \
             FROM templates WHERE id = ?1",
            params!["does-not-exist"],
            row_to_template,
        );
        assert!(result.is_err(), "Missing template should return error");
    }

    // ── save_template ────────────────────────────────────────────────────────

    #[test]
    fn save_template_inserts_row_with_uuid() {
        let conn = setup_db();
        let id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO templates (id, name, content, checklist_json, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))",
            params![id, "New Template", "# Content", "[]"],
        )
        .unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM templates WHERE id = ?1", params![id], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "Template row should exist after insert");
    }

    // ── overwrite_template ───────────────────────────────────────────────────

    #[test]
    fn overwrite_template_updates_content_and_updated_at() {
        let conn = setup_db();
        insert_template(&conn, "t-ow", "Original", "# Old content", "[]");

        conn.execute(
            "UPDATE templates SET content = ?1, checklist_json = ?2, updated_at = ?3 WHERE id = ?4",
            params!["# New content", r#"[{"text":"New task","sort_order":0}]"#, "2026-07-20 10:00:00", "t-ow"],
        )
        .unwrap();

        let t = conn
            .query_row(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates WHERE id = ?1",
                params!["t-ow"],
                row_to_template,
            )
            .unwrap();

        assert_eq!(t.content, "# New content");
        assert!(t.checklist_json.contains("New task"));
        assert_eq!(t.updated_at, "2026-07-20 10:00:00");
        assert_eq!(t.name, "Original", "Name must be unchanged by overwrite");
    }

    // ── rename_template ──────────────────────────────────────────────────────

    #[test]
    fn rename_template_updates_name_only() {
        let conn = setup_db();
        insert_template(&conn, "t-ren", "Old Name", "# Body", "[]");

        conn.execute(
            "UPDATE templates SET name = ?1, updated_at = datetime('now') WHERE id = ?2",
            params!["New Name", "t-ren"],
        )
        .unwrap();

        let t = conn
            .query_row(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates WHERE id = ?1",
                params!["t-ren"],
                row_to_template,
            )
            .unwrap();

        assert_eq!(t.name, "New Name");
        assert_eq!(t.content, "# Body", "Content must be unchanged by rename");
    }

    // ── delete_template ──────────────────────────────────────────────────────

    #[test]
    fn delete_template_removes_row() {
        let conn = setup_db();
        insert_template(&conn, "t-del", "To Delete", "", "[]");

        conn.execute("DELETE FROM templates WHERE id = ?1", params!["t-del"]).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM templates WHERE id = ?1",
                params!["t-del"],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "Template row should be gone after delete");
    }

    // ── list_templates_by_name ───────────────────────────────────────────────

    #[test]
    fn list_templates_by_name_case_insensitive_match() {
        let conn = setup_db();
        insert_template(&conn, "n1", "Daily Note", "", "[]");
        insert_template(&conn, "n2", "DAILY NOTE", "", "[]");
        insert_template(&conn, "n3", "daily note", "", "[]");
        insert_template(&conn, "n4", "Weekly", "", "[]");

        let mut stmt = conn
            .prepare(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates WHERE lower(name) = lower(?1) ORDER BY updated_at DESC",
            )
            .unwrap();
        let rows: Vec<Template> = stmt
            .query_map(params!["Daily Note"], row_to_template)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(rows.len(), 3, "Case-insensitive match should return 3 rows");
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert!(!ids.contains(&"n4"), "Non-matching template should be excluded");
    }

    #[test]
    fn list_templates_by_name_no_match_returns_empty() {
        let conn = setup_db();
        insert_template(&conn, "n5", "Sprint Plan", "", "[]");

        let mut stmt = conn
            .prepare(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates WHERE lower(name) = lower(?1) ORDER BY updated_at DESC",
            )
            .unwrap();
        let rows: Vec<Template> = stmt
            .query_map(params!["Does Not Exist"], row_to_template)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(rows.is_empty(), "No match should return empty vec");
    }

    // ── apply_template (core behavioral tests) ───────────────────────────────

    /// §32.3.2: apply stamps data-date on due-today sentinels with today's local date.
    #[test]
    fn apply_template_stamps_due_today_date() {
        let conn = setup_db();
        let content = r#"<p>Hello</p><div data-component="dueToday"></div>"#;
        insert_template(&conn, "tmpl-dt", "DT Template", content, "[]");
        insert_note(&conn, "note-dt", None);

        // Run the apply logic directly (without Tauri State)
        let template = conn
            .query_row(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates WHERE id = ?1",
                params!["tmpl-dt"],
                row_to_template,
            )
            .unwrap();

        let today = Local::now().format("%Y-%m-%d").to_string();
        let stamped = stamp_due_today_sentinels(&template.content, &today);

        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.execute(
            "UPDATE notes SET body = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![stamped, "note-dt"],
        )
        .unwrap();
        conn.execute_batch("COMMIT").unwrap();

        let body: Option<String> = conn
            .query_row("SELECT body FROM notes WHERE id = 'note-dt'", [], |r| r.get(0))
            .unwrap();

        let body = body.expect("body should be set after apply");
        let expected_stamp = format!(r#"data-date="{}""#, today);
        assert!(
            body.contains(&expected_stamp),
            "Body should contain today's date stamp ({}), got: {}",
            today,
            body
        );
    }

    /// §32.3.2 step 3–4: body write + subtask inserts are atomic.
    /// Verifies: after a successful apply, BOTH the note body and subtask rows exist.
    #[test]
    fn apply_template_is_atomic_body_and_subtasks_both_land() {
        let conn = setup_db();
        let checklist_json = r#"[{"text":"Task A","sort_order":0},{"text":"Task B","sort_order":1}]"#;
        insert_template(&conn, "tmpl-at", "Atomic Template", "# Body", checklist_json);
        insert_note(&conn, "note-at", None);

        // Simulate the apply_template logic
        let template = conn
            .query_row(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates WHERE id = ?1",
                params!["tmpl-at"],
                row_to_template,
            )
            .unwrap();

        let today = Local::now().format("%Y-%m-%d").to_string();
        let stamped = stamp_due_today_sentinels(&template.content, &today);
        let items: Vec<ChecklistItem> = serde_json::from_str(&template.checklist_json).unwrap();

        conn.execute_batch("BEGIN IMMEDIATE").unwrap();

        conn.execute(
            "UPDATE notes SET body = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![stamped, "note-at"],
        )
        .unwrap();

        let max_so: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM subtasks WHERE note_id = 'note-at'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        for (i, item) in items.iter().enumerate() {
            let sub_id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO subtasks \
                 (id, note_id, text, due_date, done, completed_at, sort_order, created_at, updated_at) \
                 VALUES (?1, 'note-at', ?2, NULL, 0, NULL, ?3, datetime('now'), datetime('now'))",
                params![sub_id, item.text, max_so + 1 + i as i64],
            )
            .unwrap();
        }

        conn.execute_batch("COMMIT").unwrap();

        // Verify body was written
        let body: Option<String> = conn
            .query_row("SELECT body FROM notes WHERE id = 'note-at'", [], |r| r.get(0))
            .unwrap();
        assert!(body.is_some(), "Note body should be set after apply");
        assert_eq!(body.unwrap(), "# Body");

        // Verify both subtasks were inserted
        let sub_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subtasks WHERE note_id = 'note-at'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sub_count, 2, "Both subtask rows should exist after apply");

        // Verify subtask properties (§32.1: done=0, due_date=NULL, completed_at=NULL)
        let sub_texts: Vec<String> = conn
            .prepare(
                "SELECT text FROM subtasks WHERE note_id = 'note-at' ORDER BY sort_order ASC",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(sub_texts, vec!["Task A", "Task B"]);

        let done_sum: i64 = conn
            .query_row(
                "SELECT SUM(done) FROM subtasks WHERE note_id = 'note-at'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(done_sum, 0, "All applied subtasks must start as done=0");
    }

    /// §32.3.2: apply appends subtasks after the note's existing max sort_order.
    #[test]
    fn apply_template_appends_after_existing_subtasks() {
        let conn = setup_db();
        let checklist_json = r#"[{"text":"New Task","sort_order":0}]"#;
        insert_template(&conn, "tmpl-ap", "Append Template", "# Body", checklist_json);
        insert_note(&conn, "note-ap", Some("existing body"));

        // Pre-existing subtasks with sort_order 0, 5, 10
        insert_subtask(&conn, "pre-1", "note-ap", "Pre-existing A", 0);
        insert_subtask(&conn, "pre-2", "note-ap", "Pre-existing B", 5);
        insert_subtask(&conn, "pre-3", "note-ap", "Pre-existing C", 10);

        let template = conn
            .query_row(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates WHERE id = ?1",
                params!["tmpl-ap"],
                row_to_template,
            )
            .unwrap();

        let items: Vec<ChecklistItem> = serde_json::from_str(&template.checklist_json).unwrap();
        let today = Local::now().format("%Y-%m-%d").to_string();
        let stamped = stamp_due_today_sentinels(&template.content, &today);

        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.execute(
            "UPDATE notes SET body = ?1, updated_at = datetime('now') WHERE id = 'note-ap'",
            params![stamped],
        )
        .unwrap();

        let max_so: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(sort_order), -1) FROM subtasks WHERE note_id = 'note-ap'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(max_so, 10, "Max sort_order of pre-existing subtasks should be 10");

        let new_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO subtasks \
             (id, note_id, text, due_date, done, completed_at, sort_order, created_at, updated_at) \
             VALUES (?1, 'note-ap', ?2, NULL, 0, NULL, ?3, datetime('now'), datetime('now'))",
            params![new_id, items[0].text, max_so + 1],
        )
        .unwrap();

        conn.execute_batch("COMMIT").unwrap();

        // New subtask should have sort_order = 11
        let new_sort_order: i64 = conn
            .query_row(
                "SELECT sort_order FROM subtasks WHERE id = ?1",
                params![new_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_sort_order, 11, "New subtask should append after existing max (10)");

        // Pre-existing subtasks untouched
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subtasks WHERE note_id = 'note-ap'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(total, 4, "3 pre-existing + 1 new = 4 total subtasks");
    }

    /// Atomicity: simulated failure mid-apply rolls back both body and subtasks.
    #[test]
    fn apply_template_rollback_leaves_note_unchanged() {
        let conn = setup_db();
        insert_template(&conn, "tmpl-rb", "Rollback Template", "# New Body", r#"[{"text":"Task","sort_order":0}]"#);
        insert_note(&conn, "note-rb", Some("original body"));

        // Simulate a BEGIN that rolls back before COMMIT
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();

        conn.execute(
            "UPDATE notes SET body = '# New Body', updated_at = datetime('now') WHERE id = 'note-rb'",
            [],
        )
        .unwrap();

        let sub_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO subtasks \
             (id, note_id, text, done, sort_order, created_at, updated_at) \
             VALUES (?1, 'note-rb', 'Task', 0, 0, datetime('now'), datetime('now'))",
            params![sub_id],
        )
        .unwrap();

        // Rollback — simulates apply_template failure path
        conn.execute_batch("ROLLBACK").unwrap();

        // Note body must be unchanged
        let body: Option<String> = conn
            .query_row("SELECT body FROM notes WHERE id = 'note-rb'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            body.as_deref(),
            Some("original body"),
            "Body must be unchanged after rollback"
        );

        // No subtask rows
        let sub_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subtasks WHERE note_id = 'note-rb'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sub_count, 0, "No subtasks should exist after rollback");
    }

    /// apply_template with empty checklist_json inserts no subtasks (body-only apply).
    #[test]
    fn apply_template_empty_checklist_inserts_no_subtasks() {
        let conn = setup_db();
        insert_template(&conn, "tmpl-ec", "Empty Checklist", "# Body only", "[]");
        insert_note(&conn, "note-ec", None);

        let template = conn
            .query_row(
                "SELECT id, name, content, checklist_json, created_at, updated_at \
                 FROM templates WHERE id = ?1",
                params!["tmpl-ec"],
                row_to_template,
            )
            .unwrap();

        let items: Vec<ChecklistItem> = serde_json::from_str(&template.checklist_json).unwrap();
        assert!(items.is_empty());

        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.execute(
            "UPDATE notes SET body = ?1, updated_at = datetime('now') WHERE id = 'note-ec'",
            params![template.content],
        )
        .unwrap();
        // No subtask loop runs — items is empty
        conn.execute_batch("COMMIT").unwrap();

        let sub_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM subtasks WHERE note_id = 'note-ec'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(sub_count, 0, "Empty checklist should insert no subtasks");
    }
}
