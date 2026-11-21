// Pane — Sheets CRUD Backend (#2339 PR 5a, §33.2–§33.8)
//
// CRUD commands for the Sheets feature's grid data: `sheet_columns`,
// `sheet_rows`, `sheet_cells` (schema BUILT in PR 2, §1.12). This module
// owns:
//   - `create_sheet`: the public creation entry point, wrapping PR 4's
//     `create_sheet_note_internal` + default-column seeding in one txn.
//   - Column/row/cell CRUD commands (§1.1–1.3 of the PR 5 scoping report).
//   - The body-regeneration chokepoint (§33.4) that every value-mutating
//     command calls, in the same transaction as the mutation.
//   - `cascade_delete_sheet_tables_for_note`, called from
//     `notes::purge_note_internal` (§1.12.2 point 3).
//
// Transaction idiom: bare-SQL `BEGIN TRANSACTION` / `COMMIT` / `ROLLBACK`,
// matching `reorder_notes_internal` (notes.rs) and `create_sheet`'s own
// scoping-report reference implementation — NOT `rusqlite::Connection::
// transaction()`, which is unused elsewhere in this codebase.
//
// Formula strings: `sheet_cells.value` stores either a plain value or a
// canonical `=`-prefixed formula string in the SAME column (§33.5.2) — there
// is no separate `formula`/`computed_value` column in the built schema
// (core-data-model.md §1.12 DDL, verified against migrations.rs). PR 5
// stored whatever string was passed verbatim, no parsing, no validation.
// PR 7 (`crate::formula::recompute`) adds: parse-on-write validation for
// local edits (rejects unparseable formulas before they're ever stored;
// sync/AI-originated formulas still bypass this and degrade to #PARSE!
// at recompute time — §33.5.3), and the live recompute engine itself.
// `computed_value` is STILL never persisted anywhere (ADR-0066 D4,
// structural — ground-truthed against migrations.rs: `sheet_cells` has no
// `computed_value` column) — it lives only in the in-memory
// `HashMap<CellId, CellValue>` `recompute::recompute_sheet` returns fresh
// on every call, consumed immediately by `regenerate_sheet_body` in the
// same transaction and never retained past that.

use crate::database::DbState;
use crate::formula::recompute::{self, CellId};
use crate::formula::CellValue;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tauri::State;
use uuid::Uuid;

/// Default column count seeded into every newly created sheet (§33.2.1:
/// "a new sheet is born with a default column set (design phase decides
/// count/types)"). Provisional pending design-phase sign-off — see the
/// module-level comment on `seed_default_sheet_columns` below.
///
// Default column count/types provisional — design phase may revise this constant
const DEFAULT_SHEET_COLUMNS: usize = 5;

/// Gap used for append-at-end `sort_order` placement on columns and rows,
/// mirroring `templates.rs`'s `apply_template` subtask-append pattern
/// shape (`COALESCE(MAX(sort_order), sentinel) + gap`) applied to the
/// Sheets grid's column/row ordering (§33.3.3, "match the Cabinet §1.11
/// mechanism"). Deviates from `templates.rs`'s literal `-1` sentinel:
/// that function uses a gap of 1 (`+ 1`), so its `-1` sentinel yields `0`
/// for the first append. This module's gap is 1000, so the sentinel is
/// `-1000` (not `-1`) to preserve the same "first append lands at 0"
/// invariant — `-1 + 1000 = 999` would otherwise put the very first row/
/// column one gap-unit short of the intended 0/1000/2000/... sequence.
const SORT_ORDER_GAP: i64 = 1000;

/// Valid `sheet_columns.col_type` values (§33.3.1).
pub(crate) const VALID_COLUMN_TYPES: [&str; 6] =
    ["text", "number", "currency", "date", "checkbox", "select"];

/// Positional-insert placement, shared by rows and columns (§33.3.4).
/// `Before` places the new row/column immediately above/left of the
/// `reference_id`. `After` exists in the contract (parses correctly) but
/// is not wired to any UI entry point yet — only `Before` is exposed by
/// the context menu ("Insert row above" / "Insert column left").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InsertPosition {
    Before,
    After,
}

impl InsertPosition {
    /// Parses the IPC-level `Option<String>` position field. `None` is
    /// NOT handled here (that's the caller's "both absent = append"
    /// branch) — this only parses a `Some(String)` that has already been
    /// determined to accompany a `reference_id`. Any string other than
    /// `"before"`/`"after"` (case-sensitive, matching the rest of this
    /// module's plain-string-enum conventions e.g. `col_type`) is a
    /// hard error, never a silent fallback to append.
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "before" => Ok(InsertPosition::Before),
            "after" => Ok(InsertPosition::After),
            other => Err(format!(
                "InsertPosition: invalid position '{}' — must be 'before' or 'after'",
                other
            )),
        }
    }
}

// === Data Types ===

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetColumn {
    pub id: String,
    pub note_id: String,
    pub name: String,
    pub col_type: String,
    pub options_json: String,
    pub width: i64,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetRow {
    pub id: String,
    pub note_id: String,
    pub sort_order: i64,
    pub height: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// Item 40 (#2874) — one column's creation spec for `create_sheet`'s
/// optional `columns` param. `col_type` is validated against
/// `VALID_COLUMN_TYPES` by `seed_columns_from_spec` before any INSERT
/// runs — an invalid entry rejects the WHOLE `create_sheet_internal` call
/// (transaction rollback, no partial sheet). `options` mirrors
/// `add_sheet_column`'s existing `options_json: Option<&str>` contract —
/// a `serde_json::Value` here, serialized via `Display`/`to_string()`
/// (valid JSON), defaulting to `"{}"` when absent, same as
/// `create_sheet_column_internal`'s own `options_json.unwrap_or("{}")`.
#[derive(Debug, Clone, Deserialize)]
pub struct ColumnSpec {
    pub name: String,
    pub col_type: String,
    pub options: Option<serde_json::Value>,
}

/// One row's creation spec for `add_sheet_rows` (#2970) and the
/// ghost-row-commit batch backend it extends. `values` mirrors
/// `add_sheet_row`'s existing `values: Option<HashMap<String, String>>`
/// contract — an optional per-row `{column_id: value}` map. `None` or an
/// empty map is a blank (sparse, cell-less) row — the exact shape
/// `create_sheet_rows_batch_internal` always produced before #2970, so
/// the ghost-row-commit Tauri wrapper (`create_sheet_rows`) passes one
/// `RowSpec { values: None }` per row and gets byte-identical behavior.
#[derive(Debug, Clone, Deserialize)]
pub struct RowSpec {
    pub values: Option<HashMap<String, String>>,
}

/// `raw_value` is `sheet_cells.value` verbatim for STORAGE, but every
/// `SheetCell` returned to a caller — from a read (`get_sheet_data`) OR
/// a write (`update_sheet_cell_internal`, #2874 item 47) — carries it in
/// current A1 DISPLAY form (`translate_cells_for_display`), never the
/// canonical `{column_id:row_id}` storage form. `computed_value` is
/// likewise populated uniformly: `get_sheet_data` (§33.5.4 trigger 3,
/// sheet open/load recompute) AND every write through
/// `update_sheet_cell_internal` — value-changing (#2874 item 25) OR
/// style-only (#2874 item 47, superseding item 25's original
/// value-changed-only scoping and item 27's frontend-side preserve
/// patch) — run a full recompute and return the cell's live computed
/// value inline, so the grid never has to guess or hold a stale value
/// across any kind of save. `computed_value` is `None` for non-formula
/// cells, and for formula cells whose sheet is past the guard-rail soft
/// limit (recompute skipped, §33.5.4 degradation) — the frontend/AI tool
/// (§4.1) should fall back to `raw_value` whenever `computed_value` is
/// `None`. Never persisted (ADR-0066 D4) — computed values live only in
/// memory, freshly rebuilt on every recompute pass (including, as of
/// item 47, a style-only write's pass — the recompute cost this implies
/// on every styled cell is a deliberate, accepted trade-off, not an
/// oversight).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetCell {
    pub row_id: String,
    pub column_id: String,
    pub note_id: String,
    pub raw_value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub computed_value: Option<String>,
    pub style_json: String,
    pub updated_at: String,
}

/// Full grid structure for a sheet note — columns/rows/cells, all ordered.
/// Returned by `create_sheet` and `get_sheet_data` so the frontend can
/// render a grid immediately without a second round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetData {
    pub note_meta: crate::database::NoteMeta,
    pub columns: Vec<SheetColumn>,
    pub rows: Vec<SheetRow>,
    pub cells: Vec<SheetCell>,
    /// Total `=`-prefixed formula cells currently in this sheet (#2339
    /// PR 14, Deliverable 2 — exposes `recompute::FORMULA_CELL_SOFT_LIMIT`'s
    /// guard-rail state to the frontend). Drives `SheetStatusBar`'s
    /// already-existing-but-previously-unused `formulas` prop and the
    /// `ShPaused` banner's bold count text. Counted from the SAME `cells`
    /// list returned below — always in sync, no second query.
    pub formula_count: usize,
    /// True iff `formula_count` exceeds `recompute::FORMULA_CELL_SOFT_LIMIT`
    /// — mirrors exactly the condition `recompute_sheet` itself uses to
    /// skip recompute (same threshold, same "strictly greater than"
    /// comparison), so this flag and "were any cells actually recomputed
    /// this pass" can never disagree.
    pub formulas_paused: bool,
}

// === Row-mapping helpers ===

fn row_to_column(row: &rusqlite::Row<'_>) -> rusqlite::Result<SheetColumn> {
    Ok(SheetColumn {
        id: row.get(0)?,
        note_id: row.get(1)?,
        name: row.get(2)?,
        col_type: row.get(3)?,
        options_json: row.get(4)?,
        width: row.get(5)?,
        sort_order: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn row_to_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SheetRow> {
    Ok(SheetRow {
        id: row.get(0)?,
        note_id: row.get(1)?,
        sort_order: row.get(2)?,
        height: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

fn row_to_cell(row: &rusqlite::Row<'_>) -> rusqlite::Result<SheetCell> {
    Ok(SheetCell {
        row_id: row.get(0)?,
        column_id: row.get(1)?,
        note_id: row.get(2)?,
        raw_value: row.get(3)?,
        computed_value: None,
        style_json: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// Attaches computed values (from a just-run recompute pass) onto a
/// vec of `SheetCell`s already loaded via `row_to_cell` — used by
/// `get_sheet_data` (§33.5.4 trigger 3) so the returned grid carries
/// both `raw_value` and `computed_value` per formula cell. Non-formula
/// cells, and formula cells with no entry in `computed_values`
/// (guard-rail skip, or a parse/eval error not represented in the map —
/// though `recompute_sheet` does populate `#PARSE!`/error `CellValue`s),
/// are left with `computed_value: None`.
fn attach_computed_values(cells: &mut [SheetCell], computed_values: &HashMap<CellId, CellValue>) {
    for cell in cells.iter_mut() {
        if cell.raw_value.starts_with('=') {
            let key = (cell.column_id.clone(), cell.row_id.clone());
            if let Some(v) = computed_values.get(&key) {
                cell.computed_value = Some(v.to_string());
            }
        }
    }
}

/// `sheet_columns.id`s for `note_id`, ordered by `sort_order ASC` —
/// position `i` is exactly what A1 column letter `i` currently denotes
/// (`A` = 0, `B` = 1, ...). Shared by `update_sheet_cell_internal`'s
/// write-path translation and `get_sheet_data_internal`'s read-path
/// translation (#2339 PR 14, ReferenceMapper) so both directions are
/// guaranteed to agree on "what column letter means what id right now" —
/// same query, same ordering, no risk of the two directions drifting.
fn load_ordered_column_ids(conn: &Connection, note_id: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC")
        .map_err(|e| format!("load_ordered_column_ids: prepare failed: {}", e))?;
    let ids = stmt
        .query_map(params![note_id], |r| r.get::<_, String>(0))
        .map_err(|e| format!("load_ordered_column_ids: query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("load_ordered_column_ids: row collect failed: {}", e))?;
    Ok(ids)
}

/// `sheet_rows.id`s for `note_id`, ordered by `sort_order ASC` — position
/// `i` is exactly what A1 row number `i + 1` currently denotes. See
/// `load_ordered_column_ids`'s doc comment — same rationale, row side.
fn load_ordered_row_ids(conn: &Connection, note_id: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT id FROM sheet_rows WHERE note_id = ?1 ORDER BY sort_order ASC")
        .map_err(|e| format!("load_ordered_row_ids: prepare failed: {}", e))?;
    let ids = stmt
        .query_map(params![note_id], |r| r.get::<_, String>(0))
        .map_err(|e| format!("load_ordered_row_ids: query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("load_ordered_row_ids: row collect failed: {}", e))?;
    Ok(ids)
}

/// READ-path translation (#2339 PR 14, §33.5.2, ReferenceMapper): rewrites
/// every formula cell's `raw_value` in place, re-rendering its stored
/// stable-ID references (`{column_id:row_id}`) as their CURRENT A1
/// position, for display in the formula bar / in-cell raw text. Only
/// touches `=`-prefixed values; a formula that fails to parse (sync-
/// arrived malformed text) is left completely unchanged by
/// `translate_formula_ids_to_a1`'s own fallback contract. Called by
/// `get_sheet_data_internal` (the full-sheet read path) AND, as of #2874
/// item 47, `update_sheet_cell_internal` (the single-cell write
/// chokepoint) — every `SheetCell` returned to a caller, read or write,
/// is translated through this SAME function; item 47 explicitly REUSES
/// it rather than duplicating the ID-to-A1 rewrite logic for the
/// single-cell write-reply case.
fn translate_cells_for_display(cells: &mut [SheetCell], ordered_column_ids: &[String], ordered_row_ids: &[String]) {
    for cell in cells.iter_mut() {
        if cell.raw_value.starts_with('=') {
            cell.raw_value = crate::formula::reference_mapper::translate_formula_ids_to_a1(
                &cell.raw_value,
                ordered_column_ids,
                ordered_row_ids,
            );
        }
    }
}

// === Body regeneration chokepoint (§33.4) ===

/// Regenerates `notes.body` for a sheet note from its current grid state
/// (§33.4). MUST be called inside the same transaction as the grid
/// mutation that triggered it — no async/debounced regen, no window where
/// grid truth and `body` disagree locally.
///
/// Format: title line, then a tab-separated column-name header line, then
/// one tab-separated line per non-empty row (columns in `sort_order`).
/// Empty rows (every cell sparse/missing) produce NO body line (Reed's
/// recommendation, Matt confirmed 2026-09-03) — keeps FTS clean.
///
/// `computed_values` (#2339 PR 7, Deliverable 2 — "closing the PR 5a
/// interim gap"): a formula cell's rendered value comes from this map
/// (produced by `recompute::recompute_sheet` for this same note,
/// immediately prior, in the SAME transaction — see `recompute_and_regen`
/// below) rather than its raw `=`-prefixed formula string. If a formula
/// cell has no entry in `computed_values` (shouldn't happen post-
/// recompute, but defensive against the guard-rail-skip / empty-map
/// paths), the raw formula string is rendered as a fallback — the
/// pre-PR-7 interim behavior, never a panic or a blank cell.
fn regenerate_sheet_body(
    conn: &Connection,
    note_id: &str,
    computed_values: &HashMap<CellId, CellValue>,
) -> Result<(), String> {
    let title: String = conn
        .query_row("SELECT title FROM notes WHERE id = ?1", params![note_id], |r| r.get(0))
        .map_err(|e| format!("regenerate_sheet_body: title lookup failed (note_id={}): {}", note_id, e))?;

    let mut col_stmt = conn
        .prepare("SELECT id, name FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC")
        .map_err(|e| format!("regenerate_sheet_body: column query prepare failed: {}", e))?;
    let columns: Vec<(String, String)> = col_stmt
        .query_map(params![note_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("regenerate_sheet_body: column query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("regenerate_sheet_body: column row collect failed: {}", e))?;

    let mut row_stmt = conn
        .prepare("SELECT id FROM sheet_rows WHERE note_id = ?1 ORDER BY sort_order ASC")
        .map_err(|e| format!("regenerate_sheet_body: row query prepare failed: {}", e))?;
    let row_ids: Vec<String> = row_stmt
        .query_map(params![note_id], |r| r.get::<_, String>(0))
        .map_err(|e| format!("regenerate_sheet_body: row query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("regenerate_sheet_body: row id collect failed: {}", e))?;

    let header_line = columns
        .iter()
        .map(|(_, name)| name.as_str())
        .collect::<Vec<_>>()
        .join("\t");

    let mut lines: Vec<String> = vec![title, header_line];

    for row_id in &row_ids {
        let mut cell_stmt = conn
            .prepare("SELECT column_id, value FROM sheet_cells WHERE row_id = ?1")
            .map_err(|e| format!("regenerate_sheet_body: cell query prepare failed: {}", e))?;
        let cells: std::collections::HashMap<String, String> = cell_stmt
            .query_map(params![row_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| format!("regenerate_sheet_body: cell query failed: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("regenerate_sheet_body: cell row collect failed: {}", e))?
            .into_iter()
            .collect();

        let all_empty = columns
            .iter()
            .all(|(col_id, _)| cells.get(col_id).map(|v| v.is_empty()).unwrap_or(true));

        if all_empty {
            // Empty-row handling: no body line for a row with no non-empty cells.
            continue;
        }

        let line = columns
            .iter()
            .map(|(col_id, _)| {
                let raw = cells.get(col_id).cloned().unwrap_or_default();
                if raw.starts_with('=') {
                    // Formula cell: render the computed value, not the raw
                    // formula string (#2339 PR 7, closing the PR 5a
                    // interim gap). Fall back to the raw formula string if
                    // no computed entry exists (defensive — guard-rail
                    // skip, or this call site passed an empty map).
                    match computed_values.get(&(col_id.clone(), row_id.clone())) {
                        Some(v) => v.to_string(),
                        None => raw,
                    }
                } else {
                    raw
                }
            })
            .collect::<Vec<_>>()
            .join("\t");
        lines.push(line);
    }

    let body = lines.join("\n");

    conn.execute(
        "UPDATE notes SET body = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![body, note_id],
    )
    .map_err(|e| format!("regenerate_sheet_body: body write failed (note_id={}): {}", note_id, e))?;

    Ok(())
}

/// The shared recompute-then-regen chokepoint (#2339 PR 7, Deliverable 5).
/// Every value-mutating command in this module, and `sync/engine.rs`'s
/// post-sync hook, call this instead of `regenerate_sheet_body` directly.
///
/// **Ordering is load-bearing (Matt ruling 2026-09-03, fixing the PR 5a
/// comment-ordering bug):** recompute MUST run before body regen. The
/// PR 5a insertion-point comments this replaces said "regen, then
/// recompute after" — that order is backwards: if body regen ran first,
/// it would render stale (or raw-formula-string) values, and the freshly
/// computed values would never make it into `notes.body` until the NEXT
/// mutation. Recomputing first means regen always sees this pass's fresh
/// computed values.
///
/// Full-graph rebuild per mutation: Matt ruling 2026-09-03. Deviates from
/// §33.5.4's dirty-subgraph design for v1 simplicity. Revisit if perf
/// requires targeted invalidation. (The actual rebuild happens inside
/// `recompute::recompute_sheet` — see that function's doc comment.)
pub(crate) fn recompute_and_regen(conn: &Connection, note_id: &str) -> Result<(), String> {
    let computed = recompute::recompute_sheet(conn, note_id)?;
    regenerate_sheet_body(conn, note_id, &computed)
}

/// `pub(crate)` seam for `sync/engine.rs`'s post-sync recompute hook,
/// which already has its own freshly computed `HashMap<CellId, CellValue>`
/// (it calls `recompute::recompute_sheet` itself, once per touched sheet,
/// so it can log-and-continue per-sheet rather than aborting the whole
/// post-sync pass on one bad sheet — seeing `recompute_and_regen`'s
/// single combined `Result` wouldn't let it distinguish "recompute
/// failed" from "regen failed" for logging purposes). Thin pass-through
/// to the private `regenerate_sheet_body` — no separate implementation.
pub(crate) fn regenerate_sheet_body_pub(
    conn: &Connection,
    note_id: &str,
    computed_values: &HashMap<CellId, CellValue>,
) -> Result<(), String> {
    regenerate_sheet_body(conn, note_id, computed_values)
}

// === Cascade (§1.12.2 point 3) ===

/// Hard-delete all side-table rows for a sheet note. Called from
/// `notes::purge_note_internal` immediately after the subtasks cascade and
/// before the `note_versions` cleanup, mirroring
/// `subtasks::cascade_delete_subtasks_for_note`'s shape exactly. Order is
/// child-first (cells, then rows/columns) — defensive, not load-bearing
/// since there are no FKs on these CRR tables (#493).
pub(crate) fn cascade_delete_sheet_tables_for_note(
    conn: &Connection,
    note_id: &str,
) -> Result<(), String> {
    conn.execute("DELETE FROM sheet_cells WHERE note_id = ?1", params![note_id])
        .map_err(|e| format!("cascade_delete_sheet_tables_for_note: cells delete failed: {}", e))?;
    conn.execute("DELETE FROM sheet_rows WHERE note_id = ?1", params![note_id])
        .map_err(|e| format!("cascade_delete_sheet_tables_for_note: rows delete failed: {}", e))?;
    conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![note_id])
        .map_err(|e| format!("cascade_delete_sheet_tables_for_note: columns delete failed: {}", e))?;
    Ok(())
}

// === create_sheet (Deliverable 1) ===

/// Seeds `DEFAULT_SHEET_COLUMNS` fresh text columns for a newly created
/// sheet, append-ordered with the standard `SORT_ORDER_GAP`. Caller owns
/// the transaction.
fn seed_default_sheet_columns(conn: &Connection, note_id: &str) -> Result<Vec<SheetColumn>, String> {
    let mut created = Vec::with_capacity(DEFAULT_SHEET_COLUMNS);
    let now = chrono::Utc::now().to_rfc3339();
    for i in 0..DEFAULT_SHEET_COLUMNS {
        let id = Uuid::new_v4().to_string();
        let name = format!("Column {}", i + 1);
        let sort_order = (i as i64) * SORT_ORDER_GAP;
        conn.execute(
            "INSERT INTO sheet_columns (id, note_id, name, col_type, options_json, width, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'text', '{}', 0, ?4, ?5, ?5)",
            params![id, note_id, name, sort_order, now],
        )
        .map_err(|e| format!("seed_default_sheet_columns: insert failed (index {}): {}", i, e))?;
        created.push(SheetColumn {
            id,
            note_id: note_id.to_string(),
            name,
            col_type: "text".to_string(),
            options_json: "{}".to_string(),
            width: 0,
            sort_order,
            created_at: now.clone(),
            updated_at: now.clone(),
        });
    }
    Ok(created)
}

/// Item 40 (#2874) — seeds columns for a newly created sheet from an
/// explicit `ColumnSpec` list instead of `seed_default_sheet_columns`'s
/// hardcoded 5-Text set. Same shape as that function (loop,
/// `SORT_ORDER_GAP`-spaced `sort_order`, same INSERT statement, caller
/// owns the transaction — no nested `BEGIN`/`COMMIT` here, unlike
/// `create_sheet_column_internal` which is NOT reused for this reason:
/// calling it from inside `create_sheet_internal`'s already-open
/// transaction would attempt a nested `BEGIN TRANSACTION`, which SQLite
/// rejects). All `col_type`s are validated against `VALID_COLUMN_TYPES`
/// up front, before any INSERT runs — one bad entry rejects the whole
/// batch (caller rolls back the whole `create_sheet_internal` call, no
/// partial sheet with some columns and a dangling note).
fn seed_columns_from_spec(conn: &Connection, note_id: &str, columns: &[ColumnSpec]) -> Result<Vec<SheetColumn>, String> {
    for spec in columns {
        if !VALID_COLUMN_TYPES.contains(&spec.col_type.as_str()) {
            return Err(format!(
                "seed_columns_from_spec: invalid col_type '{}' for column '{}' — must be one of {:?}",
                spec.col_type, spec.name, VALID_COLUMN_TYPES
            ));
        }
    }

    let mut created = Vec::with_capacity(columns.len());
    let now = chrono::Utc::now().to_rfc3339();
    for (i, spec) in columns.iter().enumerate() {
        let id = Uuid::new_v4().to_string();
        let sort_order = (i as i64) * SORT_ORDER_GAP;
        // serde_json::Value::to_string() via Display IS valid JSON serialization
        // — do NOT swap this for format!("{:?}", ...) (Rust Debug syntax, invalid
        // JSON). Same contract as execute_add_sheet_column's options handling.
        let options_json = spec.options.as_ref().map(|v| v.to_string()).unwrap_or_else(|| "{}".to_string());
        conn.execute(
            "INSERT INTO sheet_columns (id, note_id, name, col_type, options_json, width, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?7)",
            params![id, note_id, spec.name, spec.col_type, options_json, sort_order, now],
        )
        .map_err(|e| format!("seed_columns_from_spec: insert failed (index {}): {}", i, e))?;
        created.push(SheetColumn {
            id,
            note_id: note_id.to_string(),
            name: spec.name.clone(),
            col_type: spec.col_type.clone(),
            options_json,
            width: 0,
            sort_order,
            created_at: now.clone(),
            updated_at: now.clone(),
        });
    }
    Ok(created)
}

/// The public sheet-creation entry point (§33.2, Deliverable 1). Calls
/// PR 4's `create_sheet_note_internal` then seeds the default column set
/// in the SAME transaction, then runs an initial body regen so the
/// column-name header line is reflected immediately rather than waiting
/// for the first cell edit. Returns the full `SheetData` so the frontend
/// can render the grid without a second round-trip.
#[tauri::command]
pub fn create_sheet(
    mut request: crate::notes::CreateSheetNoteRequest,
    db: State<DbState>,
) -> Result<SheetData, String> {
    eprintln!("[SHEETS] create_sheet called with: title={}, project_id={}", request.title, request.project_id);

    // Required item 4: `notes.status` is `TEXT NOT NULL DEFAULT 'cabinet'`,
    // but that DEFAULT only applies when the column is omitted from the
    // INSERT's column list — `create_sheet_note_internal` always includes
    // `status` in its INSERT, so a `None` here binds an explicit SQL NULL
    // and violates NOT NULL. Coalesce here, at the new command boundary,
    // matching `create_note_internal`'s existing frontend-side contract
    // (`src/lib/api.ts`'s `createNoteInProject` passes `status || 'cabinet'`)
    // without changing `create_sheet_note_internal`'s own tested passthrough
    // contract (PR 4).
    if request.status.is_none() {
        request.status = Some("cabinet".to_string());
    }

    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    // Tauri UI command: always default columns (item 40, #2874 — behavioral
    // parity requirement, this call site is UNCHANGED from pre-item-40
    // behavior).
    create_sheet_internal(&conn, request, None)
}

/// `columns`: item 40 (#2874) — `None` seeds `DEFAULT_SHEET_COLUMNS` (5)
/// Text columns via `seed_default_sheet_columns`, exactly as before this
/// item shipped (every pre-existing caller passes `None` and is
/// behaviorally identical to pre-item-40). `Some(specs)` seeds columns
/// from the caller-supplied spec via `seed_columns_from_spec` instead —
/// an empty `Some(vec![])` ALSO falls back to the 5-default path (the
/// agent tool's "omitted OR empty array" contract, both map to `None`
/// at the `execute_create_sheet` boundary, but treating an empty
/// `Some(vec![])` the same way here is a defensive belt-and-suspenders
/// match, not just a caller-side contract).
pub(crate) fn create_sheet_internal(
    conn: &Connection,
    request: crate::notes::CreateSheetNoteRequest,
    columns: Option<Vec<ColumnSpec>>,
) -> Result<SheetData, String> {
    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("create_sheet: failed to begin transaction: {}", e))?;

    let meta = match crate::notes::create_sheet_note_internal(conn, request) {
        Ok(m) => m,
        Err(e) => {
            conn.execute("ROLLBACK", []).ok();
            return Err(e);
        }
    };

    let seed_result = match columns {
        Some(specs) if !specs.is_empty() => seed_columns_from_spec(conn, &meta.id, &specs),
        _ => seed_default_sheet_columns(conn, &meta.id),
    };
    if let Err(e) = seed_result {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    // 1. Recompute formulas (full graph rebuild — Matt ruling 2026-09-03)
    // 2. Body regen with computed values (a brand-new sheet has no cells
    //    yet, so `computed` is always empty here, but every mutating
    //    command uses the same recompute-then-regen chokepoint for
    //    consistency — see `recompute_and_regen`'s doc comment).
    if let Err(e) = recompute_and_regen(conn, &meta.id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("create_sheet: failed to commit: {}", e));
    }

    get_sheet_data_internal(conn, &meta.id)
}

// === get_sheet_data (read) ===

pub(crate) fn get_sheet_data_internal(conn: &Connection, note_id: &str) -> Result<SheetData, String> {
    let note_meta = crate::notes::get_note_meta(conn, note_id)?;

    let mut col_stmt = conn
        .prepare(
            "SELECT id, note_id, name, col_type, options_json, width, sort_order, created_at, updated_at
             FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC",
        )
        .map_err(|e| format!("get_sheet_data: column query prepare failed: {}", e))?;
    let columns = col_stmt
        .query_map(params![note_id], row_to_column)
        .map_err(|e| format!("get_sheet_data: column query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("get_sheet_data: column row collect failed: {}", e))?;

    let mut row_stmt = conn
        .prepare(
            "SELECT id, note_id, sort_order, height, created_at, updated_at
             FROM sheet_rows WHERE note_id = ?1 ORDER BY sort_order ASC",
        )
        .map_err(|e| format!("get_sheet_data: row query prepare failed: {}", e))?;
    let rows = row_stmt
        .query_map(params![note_id], row_to_row)
        .map_err(|e| format!("get_sheet_data: row query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("get_sheet_data: row row collect failed: {}", e))?;

    let mut cell_stmt = conn
        .prepare(
            "SELECT row_id, column_id, note_id, value, style_json, updated_at
             FROM sheet_cells WHERE note_id = ?1",
        )
        .map_err(|e| format!("get_sheet_data: cell query prepare failed: {}", e))?;
    let mut cells = cell_stmt
        .query_map(params![note_id], row_to_cell)
        .map_err(|e| format!("get_sheet_data: cell query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("get_sheet_data: cell row collect failed: {}", e))?;

    // §33.5.4 trigger 3: sheet open/load recompute. Ensures computed values
    // are fresh on first read with no prior mutation or sync required.
    // Matt ruling 2026-09-03.
    //
    // Read-only: unlike the mutation chokepoint (`recompute_and_regen`),
    // this does NOT call `regenerate_sheet_body` — a plain read must never
    // write to `notes.body`/`updated_at` as a side effect. If the sheet is
    // past the guard-rail soft limit, `recompute_sheet` returns an empty
    // map (existing degradation behavior) and the sheet still loads fine
    // with raw values only — see `attach_computed_values`.
    let computed = recompute::recompute_sheet(conn, note_id)?;
    attach_computed_values(&mut cells, &computed);

    // READ-path translation (#2339 PR 14, §33.5.2, ReferenceMapper):
    // re-render every formula cell's stable-ID references as their
    // current A1 position for display — `columns`/`rows` above are
    // already ordered by `sort_order ASC`, so their `id`s are exactly the
    // position lists the translator needs (position `i` == column letter
    // / row number `i`).
    let ordered_column_ids: Vec<String> = columns.iter().map(|c| c.id.clone()).collect();
    let ordered_row_ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
    translate_cells_for_display(&mut cells, &ordered_column_ids, &ordered_row_ids);

    let formula_count = cells.iter().filter(|c| c.raw_value.starts_with('=')).count();
    let formulas_paused = formula_count > recompute::FORMULA_CELL_SOFT_LIMIT;

    Ok(SheetData { note_meta, columns, rows, cells, formula_count, formulas_paused })
}

#[tauri::command]
pub fn get_sheet_data(note_id: String, db: State<DbState>) -> Result<SheetData, String> {
    eprintln!("[SHEETS] get_sheet_data called with: note_id={}", note_id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    get_sheet_data_internal(&conn, &note_id)
}

// === CSV Export (#2339 PR 5c, §33.12) ===

/// Return shape for `export_sheet_csv` — RFC 4180 CSV content plus a
/// filesystem-safe suggested filename. `csv_content` is a `String` (not
/// bytes): the grid is all-text/UTF-8 by construction (§33.12), and a
/// plain string is directly consumable by a frontend save-dialog flow
/// (`invoke` → `writeTextFile`) with no base64 round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsvExport {
    pub csv_content: String,
    pub filename: String,
}

/// RFC 4180 §2, rules 5–7: a field is wrapped in `"..."` only if it
/// contains a comma, a double-quote, or a line break (`\n`/`\r`);
/// embedded double-quotes are escaped by doubling (`"` → `""`). Fields
/// with no special characters are emitted unquoted — the RFC permits
/// either, and unquoted-when-possible keeps output more human-readable
/// (matches common CSV writer behavior).
fn quote_csv_field(field: &str) -> String {
    let needs_quoting = field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r');
    if !needs_quoting {
        return field.to_string();
    }
    let escaped = field.replace('"', "\"\"");
    format!("\"{}\"", escaped)
}

/// Formats one column's cell value for CSV export (§33.12 value-type
/// formatting table). `checkbox` is the ONE column type requiring active
/// reformatting — `'1'` → `TRUE`, `'0'`/empty → `FALSE`. Every other
/// type (`text`/`number`/`currency`/`date`/`select`) is already stored/
/// computed in its final display form and passes through verbatim,
/// including error strings (`#DIV/0!` etc., already produced by
/// `CellValue::Error`'s `Display` impl upstream in `computed_value`).
/// Empty/sparse cells are handled by the caller (no `SheetCell` entry at
/// all for that coordinate) — this function only ever sees a present
/// cell's resolved value string.
fn format_csv_cell_value(value: &str, col_type: &str) -> String {
    if col_type == "checkbox" {
        if value == "1" {
            "TRUE".to_string()
        } else {
            "FALSE".to_string()
        }
    } else {
        value.to_string()
    }
}

/// Replaces filesystem-hostile characters (the Windows-illegal set —
/// safe superset also covering macOS/Linux) with `_`, trims leading/
/// trailing whitespace and dots (macOS/Windows both dislike trailing
/// dots), and falls back to `"sheet"` if sanitization empties the
/// string (e.g. a title of just `"???"`). Does NOT append `.csv` — the
/// caller does that after sanitizing.
fn sanitize_filename(title: &str) -> String {
    let replaced: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect();
    let trimmed = replaced.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "sheet".to_string()
    } else {
        trimmed
    }
}

/// Formats a full `SheetData` grid as an RFC 4180 CSV string (§33.12 §3).
/// Header row: column `name`s in `sort_order`. Data rows: one per
/// `SheetRow` in `sort_order` — unlike `regenerate_sheet_body`'s flat-
/// text format, empty rows are NOT skipped here, since a CSV export
/// should reproduce the visible row structure a spreadsheet-literate
/// user expects on open in Excel/Numbers/Sheets. Line terminator is the
/// RFC-literal `\r\n`.
fn format_sheet_as_csv(data: &SheetData) -> String {
    // Sparse cell lookup: (row_id, column_id) -> resolved display value
    // (computed_value if present — formula cells and errors — else
    // raw_value, matching get_sheet_data's own fallback contract).
    let mut cell_lookup: HashMap<(&str, &str), &str> = HashMap::new();
    for cell in &data.cells {
        let resolved = cell.computed_value.as_deref().unwrap_or(cell.raw_value.as_str());
        cell_lookup.insert((cell.row_id.as_str(), cell.column_id.as_str()), resolved);
    }

    let header = data
        .columns
        .iter()
        .map(|c| quote_csv_field(&c.name))
        .collect::<Vec<_>>()
        .join(",");

    let mut lines: Vec<String> = vec![header];

    for row in &data.rows {
        let line = data
            .columns
            .iter()
            .map(|col| {
                let raw = cell_lookup.get(&(row.id.as_str(), col.id.as_str())).copied().unwrap_or("");
                let formatted = format_csv_cell_value(raw, &col.col_type);
                quote_csv_field(&formatted)
            })
            .collect::<Vec<_>>()
            .join(",");
        lines.push(line);
    }

    lines.join("\r\n")
}

/// Exports a sheet's grid as RFC 4180 CSV (§33.12). Pure read — calls
/// `get_sheet_data_internal` (the SAME read path `get_sheet_data`'s
/// Tauri command uses), which already runs `recompute_sheet` and
/// `attach_computed_values`; export is a pure formatting layer on top of
/// the exact same `SheetData` the frontend renders. Zero new grid-
/// loading code, zero writes to any table.
pub(crate) fn export_sheet_csv_internal(conn: &Connection, note_id: &str) -> Result<CsvExport, String> {
    let data = get_sheet_data_internal(conn, note_id)?;
    let csv_content = format_sheet_as_csv(&data);
    let filename = format!("{}.csv", sanitize_filename(&data.note_meta.title));
    Ok(CsvExport { csv_content, filename })
}

/// Ships callable-but-unwired (§33.12 — design-gated): registered in
/// `lib.rs`, fully invokable via `invoke()` or direct test call, but no
/// "Export CSV" UI button exists in the sheet view yet.
#[tauri::command]
pub fn export_sheet_csv(note_id: String, db: State<DbState>) -> Result<CsvExport, String> {
    eprintln!("[SHEETS] export_sheet_csv called with: note_id={}", note_id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    export_sheet_csv_internal(&conn, &note_id)
}

// === Card summary (#2339 PR 15a, board card face) ===
//
// Read-only counts consumed by the Kanban/AllTasks/folder card faces'
// micro-grid + `cols × rows` footer metric. Deliberately does NOT reuse
// `get_sheet_data_internal`: that function fetches every column, row and
// cell (plus a full formula recompute pass) to build an editable grid,
// which is far heavier than a card face needs — a board column full of
// sheet cards would otherwise pay for N full-grid loads just to paint a
// summary line. This queries `COUNT(*)`/column-type-list only, no cell
// values, no recompute.

/// Card-face summary for one sheet note — column count, row count,
/// formula-cell count and the ordered list of column types (drives the
/// micro-grid's `TypeGlyph` header row from REAL column config, not a
/// mock). `formula_count` is computed the same way `get_sheet_data_
/// internal` computes it (a `value` starting with `=`), just via a
/// `COUNT(*) ... LIKE '=%'` query instead of fetching every cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetCardSummary {
    pub note_id: String,
    pub col_count: usize,
    pub row_count: usize,
    pub formula_count: usize,
    pub col_types: Vec<String>,
}

pub(crate) fn get_sheet_card_summary_internal(
    conn: &Connection,
    note_id: &str,
) -> Result<SheetCardSummary, String> {
    let note_type: Option<String> = conn
        .query_row(
            "SELECT note_type FROM notes WHERE id = ?1 AND deleted_at IS NULL",
            params![note_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|e| format!("get_sheet_card_summary: note lookup failed: {}", e))?
        .flatten();

    if note_type.as_deref() != Some("sheet") {
        return Err(format!(
            "get_sheet_card_summary: note {} is not a sheet",
            note_id
        ));
    }

    let col_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM sheet_columns WHERE note_id = ?1",
            params![note_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("get_sheet_card_summary: column count failed: {}", e))?;

    let row_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM sheet_rows WHERE note_id = ?1",
            params![note_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("get_sheet_card_summary: row count failed: {}", e))?;

    let formula_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM sheet_cells WHERE note_id = ?1 AND value LIKE '=%'",
            params![note_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("get_sheet_card_summary: formula count failed: {}", e))?;

    let mut col_stmt = conn
        .prepare("SELECT col_type FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC")
        .map_err(|e| format!("get_sheet_card_summary: col_type query prepare failed: {}", e))?;
    let col_types = col_stmt
        .query_map(params![note_id], |r| r.get::<_, String>(0))
        .map_err(|e| format!("get_sheet_card_summary: col_type query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("get_sheet_card_summary: col_type row collect failed: {}", e))?;

    Ok(SheetCardSummary {
        note_id: note_id.to_string(),
        col_count,
        row_count,
        formula_count,
        col_types,
    })
}

/// Single-sheet card summary. See module doc above for why this doesn't
/// reuse `get_sheet_data_internal`.
#[tauri::command]
pub fn get_sheet_card_summary(
    note_id: String,
    db: State<DbState>,
) -> Result<SheetCardSummary, String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    get_sheet_card_summary_internal(&conn, &note_id)
}

/// Batched card summaries for a visible set of sheet notes (a Kanban
/// column, an All Tasks page, a folder view) — one DB lock/round-trip
/// instead of N. Matt's ruling 2026-09-05: batch across visible sheet
/// notes since the per-note query is trivial. Note IDs that don't
/// resolve to a sheet (deleted, non-sheet, unknown) are silently
/// omitted from the result rather than failing the whole batch — callers
/// pass a mixed list of note IDs from a card list and only want the
/// sheet subset back.
#[tauri::command]
pub fn get_sheet_card_summaries(
    note_ids: Vec<String>,
    db: State<DbState>,
) -> Result<Vec<SheetCardSummary>, String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    Ok(note_ids
        .iter()
        .filter_map(|id| get_sheet_card_summary_internal(&conn, id).ok())
        .collect())
}

// === Positional insert (§33.3.4) ===
//
// Shared by `create_sheet_row_internal` and `create_sheet_column_internal`.
// `table` is always a hardcoded literal from this module's own call sites
// ("sheet_rows" or "sheet_columns") — never user input — so interpolating
// it into the SQL text below is safe.

/// Midpoint `sort_order` for a positional insert relative to
/// `reference_id`, which MUST already sit in `note_id` (the query itself
/// is the cross-note scope check — a `reference_id` from a different
/// note simply matches zero rows and errors, mirroring every other
/// scope-validated lookup in this module). Re-fetches the reference's
/// current `sort_order` fresh on every call rather than trusting a
/// caller-supplied value, so it's safe to call twice (before and after a
/// renumber) without the caller threading stale state through.
///
/// `Before`: midpoint between the predecessor (`MAX(sort_order) <
/// reference`, or `0` if `reference` is first) and `reference`.
/// `After`: midpoint between `reference` and its successor
/// (`MIN(sort_order) > reference`); if `reference` is last, there's no
/// upper neighbour to split against, so this just appends past it
/// (`reference + SORT_ORDER_GAP`) — always has room, never gap-exhausted.
fn compute_positional_sort_order(
    conn: &Connection,
    table: &str,
    note_id: &str,
    reference_id: &str,
    position: InsertPosition,
) -> Result<i64, String> {
    let reference_sort_order: i64 = conn
        .query_row(
            &format!("SELECT sort_order FROM {} WHERE id = ?1 AND note_id = ?2", table),
            params![reference_id, note_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("reference {} not found in note {}: {}", reference_id, note_id, e))?;

    match position {
        InsertPosition::Before => {
            let predecessor: Option<i64> = conn
                .query_row(
                    &format!("SELECT MAX(sort_order) FROM {} WHERE note_id = ?1 AND sort_order < ?2", table),
                    params![note_id, reference_sort_order],
                    |r| r.get(0),
                )
                .map_err(|e| format!("predecessor query failed: {}", e))?;
            let low = predecessor.unwrap_or(0);
            Ok((low + reference_sort_order) / 2)
        }
        InsertPosition::After => {
            let successor: Option<i64> = conn
                .query_row(
                    &format!("SELECT MIN(sort_order) FROM {} WHERE note_id = ?1 AND sort_order > ?2", table),
                    params![note_id, reference_sort_order],
                    |r| r.get(0),
                )
                .map_err(|e| format!("successor query failed: {}", e))?;
            match successor {
                Some(high) => Ok((reference_sort_order + high) / 2),
                None => Ok(reference_sort_order + SORT_ORDER_GAP),
            }
        }
    }
}

/// True when the two neighbours a positional insert would sit between
/// differ by less than 2 (no integer strictly between them) — the
/// renumber trigger condition (§33.3.4). Same neighbour selection as
/// `compute_positional_sort_order`; `After` past the last row/column
/// never triggers (no upper neighbour to run out of room against).
fn positional_insert_gap_exhausted(
    conn: &Connection,
    table: &str,
    note_id: &str,
    reference_id: &str,
    position: InsertPosition,
) -> Result<bool, String> {
    let reference_sort_order: i64 = conn
        .query_row(
            &format!("SELECT sort_order FROM {} WHERE id = ?1 AND note_id = ?2", table),
            params![reference_id, note_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("reference {} not found in note {}: {}", reference_id, note_id, e))?;

    match position {
        InsertPosition::Before => {
            let predecessor: Option<i64> = conn
                .query_row(
                    &format!("SELECT MAX(sort_order) FROM {} WHERE note_id = ?1 AND sort_order < ?2", table),
                    params![note_id, reference_sort_order],
                    |r| r.get(0),
                )
                .map_err(|e| format!("predecessor query failed: {}", e))?;
            let low = predecessor.unwrap_or(0);
            Ok(reference_sort_order - low < 2)
        }
        InsertPosition::After => {
            let successor: Option<i64> = conn
                .query_row(
                    &format!("SELECT MIN(sort_order) FROM {} WHERE note_id = ?1 AND sort_order > ?2", table),
                    params![note_id, reference_sort_order],
                    |r| r.get(0),
                )
                .map_err(|e| format!("successor query failed: {}", e))?;
            match successor {
                Some(high) => Ok(high - reference_sort_order < 2),
                None => Ok(false),
            }
        }
    }
}

/// Renumbers every row in `note_id` to fresh gap-1000 `sort_order`s, read
/// in current `sort_order ASC` order and rewritten as `(i + 1) *
/// SORT_ORDER_GAP` (1000, 2000, 3000, ...) — deliberately 1-indexed,
/// NOT `reorder_sheet_rows_internal`'s 0-indexed `i * SORT_ORDER_GAP`.
/// This is load-bearing for the §33.3.4 "insert above the first row"
/// edge case: that path's virtual predecessor is a fixed `0` ("midpoint
/// between 0 and the reference"), so if the renumber left the first row
/// AT `sort_order = 0`, `midpoint(0, 0)` would collide with the
/// reference forever, no matter how many times it renumbers. Starting
/// at 1000 instead of 0 reserves headroom below the first row so that
/// edge case always resolves after exactly one renumber pass. Otherwise
/// identical to the reorder loop's body, just fed by a DB-order `SELECT`
/// (`load_ordered_row_ids`) instead of a caller-supplied ID list. Caller
/// owns the transaction (called from inside `create_sheet_row_internal`'s
/// positional-insert path, same txn/snapshot/recompute discipline as
/// every other structural mutation in this module).
pub(crate) fn renumber_sheet_rows(conn: &Connection, note_id: &str) -> Result<(), String> {
    let row_ids = load_ordered_row_ids(conn, note_id)?;
    for (i, row_id) in row_ids.iter().enumerate() {
        let sort_order = ((i as i64) + 1) * SORT_ORDER_GAP;
        conn.execute(
            "UPDATE sheet_rows SET sort_order = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![sort_order, row_id],
        )
        .map_err(|e| format!("renumber_sheet_rows: update failed for {}: {}", row_id, e))?;
    }
    Ok(())
}

/// Column-side mirror of `renumber_sheet_rows`. See its doc comment —
/// same 1-indexed `(i + 1) * SORT_ORDER_GAP` rationale.
pub(crate) fn renumber_sheet_columns(conn: &Connection, note_id: &str) -> Result<(), String> {
    let column_ids = load_ordered_column_ids(conn, note_id)?;
    for (i, column_id) in column_ids.iter().enumerate() {
        let sort_order = ((i as i64) + 1) * SORT_ORDER_GAP;
        conn.execute(
            "UPDATE sheet_columns SET sort_order = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![sort_order, column_id],
        )
        .map_err(|e| format!("renumber_sheet_columns: update failed for {}: {}", column_id, e))?;
    }
    Ok(())
}

/// Resolves the final `sort_order` for a positional insert: checks the
/// renumber trigger first (gap exhausted — no integer slot between the
/// two neighbours), renumbers if needed (fresh gap-1000 spacing,
/// guaranteeing 999 integer slots), then computes the midpoint against
/// (possibly just-renumbered) current state. `renumber_fn` is
/// `renumber_sheet_rows`/`renumber_sheet_columns` — passed in rather than
/// hardcoded so this one function serves both axes.
fn resolve_positional_sort_order(
    conn: &Connection,
    table: &str,
    note_id: &str,
    reference_id: &str,
    position: InsertPosition,
    renumber_fn: fn(&Connection, &str) -> Result<(), String>,
) -> Result<i64, String> {
    if positional_insert_gap_exhausted(conn, table, note_id, reference_id, position)? {
        renumber_fn(conn, note_id)?;
    }
    compute_positional_sort_order(conn, table, note_id, reference_id, position)
}

// === Column commands ===

fn get_sheet_column_by_id(conn: &Connection, column_id: &str) -> Result<SheetColumn, String> {
    conn.query_row(
        "SELECT id, note_id, name, col_type, options_json, width, sort_order, created_at, updated_at
         FROM sheet_columns WHERE id = ?1",
        params![column_id],
        row_to_column,
    )
    .map_err(|e| format!("Sheet column not found (id={}): {}", column_id, e))
}

/// `reference_id`/`position` implement §33.3.4's positional insert.
/// Both `None` → existing append-at-end path (`MAX(sort_order) + GAP`),
/// byte-identical to pre-§33.3.4 behavior. Both `Some` → midpoint insert
/// against `reference_id` (renumbering first if the neighbouring gap is
/// exhausted). One `Some`/one `None` is a caller contract violation —
/// error, never a silent fallback to either mode.
pub(crate) fn create_sheet_column_internal(
    conn: &Connection,
    note_id: &str,
    name: &str,
    col_type: &str,
    options_json: Option<&str>,
    reference_id: Option<&str>,
    position: Option<InsertPosition>,
) -> Result<SheetColumn, String> {
    if !VALID_COLUMN_TYPES.contains(&col_type) {
        return Err(format!(
            "create_sheet_column: invalid col_type '{}' — must be one of {:?}",
            col_type, VALID_COLUMN_TYPES
        ));
    }

    if reference_id.is_some() != position.is_some() {
        return Err(
            "create_sheet_column: reference_id and position must both be present or both absent".to_string(),
        );
    }

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("create_sheet_column: failed to begin transaction: {}", e))?;

    // Structural mutation (#2339 PR 8, §33.9.2): snapshot capture bypasses
    // the throttle — capture failure rolls back the whole mutation.
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    let sort_order: i64 = match (reference_id, position) {
        (Some(ref_id), Some(pos)) => {
            match resolve_positional_sort_order(conn, "sheet_columns", note_id, ref_id, pos, renumber_sheet_columns) {
                Ok(v) => v,
                Err(e) => {
                    conn.execute("ROLLBACK", []).ok();
                    return Err(format!("create_sheet_column: positional insert failed: {}", e));
                }
            }
        }
        _ => {
            let max_sort_order: i64 = match conn.query_row(
                "SELECT COALESCE(MAX(sort_order), -1000) FROM sheet_columns WHERE note_id = ?1",
                params![note_id],
                |r| r.get(0),
            ) {
                Ok(v) => v,
                Err(e) => {
                    conn.execute("ROLLBACK", []).ok();
                    return Err(format!("create_sheet_column: max sort_order query failed: {}", e));
                }
            };
            max_sort_order + SORT_ORDER_GAP
        }
    };

    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let options = options_json.unwrap_or("{}");

    if let Err(e) = conn.execute(
        "INSERT INTO sheet_columns (id, note_id, name, col_type, options_json, width, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?7)",
        params![id, note_id, name, col_type, options, sort_order, now],
    ) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("create_sheet_column: insert failed: {}", e));
    }

    // 1. Recompute formulas (full graph rebuild — Matt ruling 2026-09-03)
    // 2. Body regen with computed values
    if let Err(e) = recompute_and_regen(conn, note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("create_sheet_column: failed to commit: {}", e));
    }

    get_sheet_column_by_id(conn, &id)
}

/// Append a new column to a sheet's grid, gap-of-1000 `sort_order` (mirrors
/// `templates.rs`'s subtask-append pattern), OR — when `reference_id` +
/// `position` are both supplied — a positional insert per §33.3.4 (only
/// `"before"` is exposed by the UI today; `"after"` parses but nothing
/// wires it). Triggers body regen (new column adds a header cell).
#[tauri::command]
pub fn create_sheet_column(
    note_id: String,
    name: String,
    col_type: String,
    options_json: Option<String>,
    reference_id: Option<String>,
    position: Option<String>,
    db: State<DbState>,
) -> Result<SheetColumn, String> {
    eprintln!(
        "[SHEETS] create_sheet_column called with: note_id={}, name={}, col_type={}, reference_id={:?}, position={:?}",
        note_id, name, col_type, reference_id, position
    );
    let parsed_position = match position.as_deref() {
        Some(raw) => Some(InsertPosition::parse(raw)?),
        None => None,
    };
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    create_sheet_column_internal(
        &conn,
        &note_id,
        &name,
        &col_type,
        options_json.as_deref(),
        reference_id.as_deref(),
        parsed_position,
    )
}

/// Batch append `count` new columns to a sheet's grid in ONE transaction
/// (item 23 / #2874, Matt's ghost-column ruling). Each new column gets
/// `name = ""`, `col_type = "text"` — same defaults `beginGhostCommit`'s
/// existing single-insert Case 2 already used. Mirrors
/// `create_sheet_column_internal`'s append-at-end shape (`MAX(sort_order)
/// + gap`) but reads `MAX(sort_order)` ONCE for the whole batch and
/// computes each new column's `sort_order` as `max + (i+1) * SORT_ORDER_GAP`
/// — same 1-indexed idiom `renumber_sheet_columns` uses (line ~1052) —
/// instead of re-querying MAX per row. One snapshot capture, one
/// `recompute_and_regen`, one COMMIT for the entire batch (Matt's
/// discipline: batch path must NOT do these per-row). Follows
/// `templates.rs::apply_template`'s proven "BEGIN once, loop INSERT,
/// COMMIT once" shape for multi-row inserts.
///
/// `count == 0` is a no-op that still validates note existence isn't
/// required (nothing to insert) — returns `Ok(vec![])` without opening a
/// transaction, since ghost-commit's offset math (§33.x) never produces
/// an offset less than 1 for an actual ghost click, but defensive
/// callers (tests, future callers) shouldn't need to special-case 0
/// upstream.
/// Extended for #2970 (`add_sheet_columns`) to accept a `name`/`col_type`/
/// `options` triple per column instead of the previous hardcoded blank-
/// text default — the exact same function, not a parallel path. The
/// ghost-column-commit Tauri wrapper (`create_sheet_columns`, below)
/// passes `count` blank `ColumnSpec`s and gets byte-identical behavior to
/// before this extension.
///
/// Every entry is validated BEFORE the transaction opens (#2970 spec:
/// "checked before the transaction opens — an over-cap/invalid call
/// creates nothing") — mirrors `seed_columns_from_spec`'s up-front
/// validation loop exactly. One recalculation runs unconditionally at the
/// end, regardless of column count — columns carry no cell values
/// (unlike `create_sheet_rows_batch_internal`'s row-side gating), so
/// there is nothing to condition it on.
pub(crate) fn create_sheet_columns_batch_internal(
    conn: &Connection,
    note_id: &str,
    columns: &[ColumnSpec],
) -> Result<Vec<SheetColumn>, String> {
    if columns.is_empty() {
        return Ok(vec![]);
    }

    for (i, spec) in columns.iter().enumerate() {
        if !VALID_COLUMN_TYPES.contains(&spec.col_type.as_str()) {
            return Err(format!(
                "add_sheet_columns: column {} has invalid col_type '{}' — must be one of {:?}",
                i, spec.col_type, VALID_COLUMN_TYPES
            ));
        }
    }

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("create_sheet_columns_batch: failed to begin transaction: {}", e))?;

    // ONE snapshot for the whole batch (Matt's discipline — not per row).
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    let max_sort_order: i64 = match conn.query_row(
        "SELECT COALESCE(MAX(sort_order), -1000) FROM sheet_columns WHERE note_id = ?1",
        params![note_id],
        |r| r.get(0),
    ) {
        Ok(v) => v,
        Err(e) => {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!("create_sheet_columns_batch: max sort_order query failed: {}", e));
        }
    };

    let now = chrono::Utc::now().to_rfc3339();
    let mut ids: Vec<String> = Vec::with_capacity(columns.len());

    for (i, spec) in columns.iter().enumerate() {
        let id = Uuid::new_v4().to_string();
        let sort_order = max_sort_order + (i as i64 + 1) * SORT_ORDER_GAP;
        // serde_json::Value::to_string() via Display IS valid JSON
        // serialization — do NOT swap this for format!("{:?}", ...) (Rust
        // Debug syntax, invalid JSON). Same contract as
        // `seed_columns_from_spec`/`execute_add_sheet_column`'s options handling.
        let options_json = spec.options.as_ref().map(|v| v.to_string()).unwrap_or_else(|| "{}".to_string());

        if let Err(e) = conn.execute(
            "INSERT INTO sheet_columns (id, note_id, name, col_type, options_json, width, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, ?7, ?7)",
            params![id, note_id, spec.name, spec.col_type, options_json, sort_order, now],
        ) {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!("create_sheet_columns_batch: insert failed (index {}): {}", i, e));
        }
        ids.push(id);
    }

    // ONE recompute+regen for the whole batch (Matt's discipline — not per row).
    if let Err(e) = recompute_and_regen(conn, note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("create_sheet_columns_batch: failed to commit: {}", e));
    }

    ids.iter().map(|id| get_sheet_column_by_id(conn, id)).collect()
}

/// Tauri wrapper for `create_sheet_columns_batch_internal` (item 23 /
/// #2874). Ghost-column commit's batch endpoint — creates `count` blank
/// columns in one transaction so a value typed into ghost column N lands
/// in the Nth newly-created column, not the first. Existing single-insert
/// `create_sheet_column` is UNCHANGED and still used by the toolbar/
/// context-menu's single-column insert flows.
#[tauri::command]
pub fn create_sheet_columns(
    note_id: String,
    count: i64,
    db: State<DbState>,
) -> Result<Vec<SheetColumn>, String> {
    eprintln!(
        "[SHEETS] create_sheet_columns called with: note_id={}, count={}",
        note_id, count
    );
    if count < 0 {
        return Err(format!("create_sheet_columns: count must be >= 0, got {}", count));
    }
    let blank_specs: Vec<ColumnSpec> = (0..count as usize)
        .map(|_| ColumnSpec { name: String::new(), col_type: "text".to_string(), options: None })
        .collect();
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    create_sheet_columns_batch_internal(&conn, &note_id, &blank_specs)
}

pub(crate) fn update_sheet_column_internal(
    conn: &Connection,
    column_id: &str,
    name: Option<&str>,
    col_type: Option<&str>,
    options_json: Option<&str>,
    width: Option<i64>,
) -> Result<SheetColumn, String> {
    if let Some(ct) = col_type {
        if !VALID_COLUMN_TYPES.contains(&ct) {
            return Err(format!(
                "update_sheet_column: invalid col_type '{}' — must be one of {:?}",
                ct, VALID_COLUMN_TYPES
            ));
        }
    }

    let current = get_sheet_column_by_id(conn, column_id)?;

    let new_name = name.unwrap_or(&current.name).to_string();
    let new_col_type = col_type.unwrap_or(&current.col_type).to_string();
    let new_options = options_json.unwrap_or(&current.options_json).to_string();
    let new_width = width.unwrap_or(current.width);

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("update_sheet_column: failed to begin transaction: {}", e))?;

    // Structural mutation (#2339 PR 8, §33.9.2): snapshot capture bypasses
    // the throttle — capture failure rolls back the whole mutation.
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, &current.note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    // Type-change note (§33.3.1): "type changes reinterpret in place;
    // unparseable values render as text with a type-mismatch indicator
    // (design phase), never destructively rewritten." PR 5 has no formula
    // engine / typed-value parser yet (PR 6/7) and the spec explicitly
    // forbids destructive rewrites — so cell VALUES are deliberately left
    // untouched on a type change; only column metadata changes here.
    // Reinterpretation-at-display is a frontend/design-phase concern.
    if let Err(e) = conn.execute(
        "UPDATE sheet_columns SET name = ?1, col_type = ?2, options_json = ?3, width = ?4, updated_at = datetime('now') WHERE id = ?5",
        params![new_name, new_col_type, new_options, new_width, column_id],
    ) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("update_sheet_column: update failed: {}", e));
    }

    // Name/type changes affect the body's header line and cell display —
    // width-only changes do not (layout, not content). Regen unconditionally
    // for simplicity and correctness (a no-op regen is cheap and safe).
    // 1. Recompute formulas (full graph rebuild — Matt ruling 2026-09-03)
    // 2. Body regen with computed values
    if let Err(e) = recompute_and_regen(conn, &current.note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("update_sheet_column: failed to commit: {}", e));
    }

    get_sheet_column_by_id(conn, column_id)
}

/// Update a column's `name`/`col_type`/`options_json`/`width`. All fields
/// optional — only provided fields are written. Triggers body regen.
#[tauri::command]
pub fn update_sheet_column(
    column_id: String,
    name: Option<String>,
    col_type: Option<String>,
    options_json: Option<String>,
    width: Option<i64>,
    db: State<DbState>,
) -> Result<SheetColumn, String> {
    eprintln!("[SHEETS] update_sheet_column called with: column_id={}", column_id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    update_sheet_column_internal(
        &conn,
        &column_id,
        name.as_deref(),
        col_type.as_deref(),
        options_json.as_deref(),
        width,
    )
}

pub(crate) fn delete_sheet_column_internal(conn: &Connection, column_id: &str) -> Result<(), String> {
    let note_id: String = conn
        .query_row(
            "SELECT note_id FROM sheet_columns WHERE id = ?1",
            params![column_id],
            |r| r.get(0),
        )
        .map_err(|e| format!("delete_sheet_column: column not found (id={}): {}", column_id, e))?;

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("delete_sheet_column: failed to begin transaction: {}", e))?;

    // Structural mutation (#2339 PR 8, §33.9.2): snapshot capture bypasses
    // the throttle — capture failure rolls back the whole mutation.
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, &note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    // §1.12.2 point 4: cascade-delete sheet_cells with this column_id, same txn.
    if let Err(e) = conn.execute("DELETE FROM sheet_cells WHERE column_id = ?1", params![column_id]) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("delete_sheet_column: cell cascade delete failed: {}", e));
    }

    if let Err(e) = conn.execute("DELETE FROM sheet_columns WHERE id = ?1", params![column_id]) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("delete_sheet_column: column delete failed: {}", e));
    }

    // 1. Recompute formulas (full graph rebuild — Matt ruling 2026-09-03)
    // 2. Body regen with computed values
    if let Err(e) = recompute_and_regen(conn, &note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("delete_sheet_column: failed to commit: {}", e));
    }

    Ok(())
}

/// Delete a column. Cascade-deletes all `sheet_cells` with that
/// `column_id` (§1.12.2 point 4) in the same transaction. Triggers body
/// regen.
#[tauri::command]
pub fn delete_sheet_column(column_id: String, db: State<DbState>) -> Result<(), String> {
    eprintln!("[SHEETS] delete_sheet_column called with: column_id={}", column_id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    delete_sheet_column_internal(&conn, &column_id)
}

pub(crate) fn reorder_sheet_columns_internal(
    conn: &Connection,
    note_id: &str,
    column_ids: &[String],
) -> Result<(), String> {
    if column_ids.is_empty() {
        return Ok(());
    }

    // Scope validation: every column_id must belong to note_id — mirrors
    // reorder_notes_internal's scope-validation-before-any-write pattern.
    for column_id in column_ids {
        let owner: Option<String> = conn
            .query_row(
                "SELECT note_id FROM sheet_columns WHERE id = ?1",
                params![column_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("reorder_sheet_columns: lookup failed for {}: {}", column_id, e))?;

        match owner {
            None => return Err(format!("reorder_sheet_columns: column {} not found", column_id)),
            Some(owner_note_id) if owner_note_id != note_id => {
                return Err(format!(
                    "reorder_sheet_columns: column {} does not belong to note {}",
                    column_id, note_id
                ));
            }
            _ => {}
        }
    }

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("reorder_sheet_columns: failed to begin transaction: {}", e))?;

    // Structural mutation (Matt ruling 2026-09-04): reorders bypass the
    // throttle. Range membership is computed by row order between
    // endpoints, so a reorder can silently change SUM/AVERAGE results —
    // consequence-heavy enough to warrant a guaranteed pre-capture, not
    // just an opportunistic one. Capture failure rolls back the mutation.
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    for (i, column_id) in column_ids.iter().enumerate() {
        let sort_order = (i as i64) * SORT_ORDER_GAP;
        if let Err(e) = conn.execute(
            "UPDATE sheet_columns SET sort_order = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![sort_order, column_id],
        ) {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!("reorder_sheet_columns: update failed for {}: {}", column_id, e));
        }
    }

    // Header order changes — regen.
    // 1. Recompute formulas (full graph rebuild — Matt ruling 2026-09-03)
    // 2. Body regen with computed values
    if let Err(e) = recompute_and_regen(conn, note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("reorder_sheet_columns: failed to commit: {}", e));
    }

    Ok(())
}

/// Reorder columns: accepts an ordered list of column IDs (the desired
/// final order), rewrites `sort_order` via gap-1000 resequence. Scope-
/// validates every id belongs to `note_id`, transactional with rollback
/// on any failure (mirrors `reorder_notes_internal`'s hardening).
#[tauri::command]
pub fn reorder_sheet_columns(
    note_id: String,
    column_ids: Vec<String>,
    db: State<DbState>,
) -> Result<(), String> {
    eprintln!("[SHEETS] reorder_sheet_columns called with: note_id={}, count={}", note_id, column_ids.len());
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    reorder_sheet_columns_internal(&conn, &note_id, &column_ids)
}

// === Row commands ===

fn get_sheet_row_by_id(conn: &Connection, row_id: &str) -> Result<SheetRow, String> {
    conn.query_row(
        "SELECT id, note_id, sort_order, height, created_at, updated_at FROM sheet_rows WHERE id = ?1",
        params![row_id],
        row_to_row,
    )
    .map_err(|e| format!("Sheet row not found (id={}): {}", row_id, e))
}

/// `reference_id`/`position` implement §33.3.4's positional insert. See
/// `create_sheet_column_internal`'s doc comment — same contract, row side.
pub(crate) fn create_sheet_row_internal(
    conn: &Connection,
    note_id: &str,
    reference_id: Option<&str>,
    position: Option<InsertPosition>,
) -> Result<SheetRow, String> {
    if reference_id.is_some() != position.is_some() {
        return Err(
            "create_sheet_row: reference_id and position must both be present or both absent".to_string(),
        );
    }

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("create_sheet_row: failed to begin transaction: {}", e))?;

    // Structural mutation (#2339 PR 8, §33.9.2): snapshot capture bypasses
    // the throttle — capture failure rolls back the whole mutation.
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    let sort_order: i64 = match (reference_id, position) {
        (Some(ref_id), Some(pos)) => {
            match resolve_positional_sort_order(conn, "sheet_rows", note_id, ref_id, pos, renumber_sheet_rows) {
                Ok(v) => v,
                Err(e) => {
                    conn.execute("ROLLBACK", []).ok();
                    return Err(format!("create_sheet_row: positional insert failed: {}", e));
                }
            }
        }
        _ => {
            let max_sort_order: i64 = match conn.query_row(
                "SELECT COALESCE(MAX(sort_order), -1000) FROM sheet_rows WHERE note_id = ?1",
                params![note_id],
                |r| r.get(0),
            ) {
                Ok(v) => v,
                Err(e) => {
                    conn.execute("ROLLBACK", []).ok();
                    return Err(format!("create_sheet_row: max sort_order query failed: {}", e));
                }
            };
            max_sort_order + SORT_ORDER_GAP
        }
    };

    let id = Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();

    if let Err(e) = conn.execute(
        "INSERT INTO sheet_rows (id, note_id, sort_order, height, created_at, updated_at)
         VALUES (?1, ?2, ?3, 0, ?4, ?4)",
        params![id, note_id, sort_order, now],
    ) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("create_sheet_row: insert failed: {}", e));
    }

    // A freshly inserted, sparse (no cells) row produces no body line
    // (empty-row handling) — but regen anyway for consistency/correctness
    // with every other mutating command, and because a future row-with-
    // cells edit expects body already reflects row count consistently.
    // 1. Recompute formulas (full graph rebuild — Matt ruling 2026-09-03)
    // 2. Body regen with computed values
    if let Err(e) = recompute_and_regen(conn, note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("create_sheet_row: failed to commit: {}", e));
    }

    get_sheet_row_by_id(conn, &id)
}

/// Append a new (sparse, cell-less) row to a sheet's grid, gap-of-1000
/// `sort_order`, OR — when `reference_id` + `position` are both supplied
/// — a positional insert per §33.3.4 (only `"before"` is exposed by the
/// UI today; `"after"` parses but nothing wires it). Triggers body regen
/// (a no-op line-wise per the empty-row rule, but keeps the chokepoint's
/// invariant that every mutation regens).
#[tauri::command]
pub fn create_sheet_row(
    note_id: String,
    reference_id: Option<String>,
    position: Option<String>,
    db: State<DbState>,
) -> Result<SheetRow, String> {
    eprintln!(
        "[SHEETS] create_sheet_row called with: note_id={}, reference_id={:?}, position={:?}",
        note_id, reference_id, position
    );
    let parsed_position = match position.as_deref() {
        Some(raw) => Some(InsertPosition::parse(raw)?),
        None => None,
    };
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    create_sheet_row_internal(&conn, &note_id, reference_id.as_deref(), parsed_position)
}

/// Extended for #2970 (`add_sheet_rows`) to accept a per-row `values`
/// map and an `at_start` placement flag, in addition to the pure
/// structural `count`-rows shape it had before — the exact same
/// function, not a parallel path. The ghost-row-commit Tauri wrapper
/// (`create_sheet_rows`, below) passes `count` blank `RowSpec`s with
/// `at_start: false` and gets byte-identical behavior to before this
/// extension.
///
/// **Value-filling (#2970):** each row's optional cell values are
/// written with `upsert_sheet_cell_no_recompute` (#2874 item 51's raw
/// per-cell write primitive) — NOT `update_sheet_cell_internal`, which
/// recomputes the whole sheet unconditionally on every call (ruling
/// 2026-09-24, item 47) and would reintroduce a per-cell recompute cost
/// at batch scale, the exact cost #2970 exists to eliminate. A1
/// translation and formula validation still run per cell (inside
/// `upsert_sheet_cell_no_recompute` itself) — only the recompute/regen is
/// deferred to one call at the end of the whole batch.
///
/// **Recompute gating (ai-integration.md §4.1 `add_sheet_rows`, spec
/// text verbatim): "One recalculation and one body regeneration run at
/// the end, only if any value was written."** This deliberately deviates
/// from `create_sheet_columns_batch_internal`'s unconditional recompute
/// (columns carry no cell values, so there's nothing to gate on) and
/// from item 51's style-batch pattern (which calls `recompute_sheet`
/// ALONE, never `recompute_and_regen`, because style edits never change
/// the note body — values DO, so this path needs the regen half too). A
/// values-less batch (the pre-#2970 ghost-row-commit shape) skips
/// `recompute_and_regen` entirely — safe because a freshly inserted,
/// sparse row produces no body line and touches no formula dependency
/// (`create_sheet_row_internal`'s own doc comment), so there is nothing
/// for a recompute to pick up.
///
/// **Rollback (#2970):** a bad value at any row rolls back the ENTIRE
/// batch — both the already-inserted rows AND any already-written cells
/// from earlier rows in the same call — one transaction, no partial
/// batch. The error names the failing row's index and column id.
pub(crate) fn create_sheet_rows_batch_internal(
    conn: &Connection,
    note_id: &str,
    rows: &[RowSpec],
    at_start: bool,
) -> Result<(Vec<SheetRow>, u32), String> {
    if rows.is_empty() {
        return Ok((vec![], 0));
    }

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("create_sheet_rows_batch: failed to begin transaction: {}", e))?;

    // ONE snapshot for the whole batch (Matt's discipline — not per row).
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    // #2970: `at_start` lands the WHOLE batch as one contiguous block
    // below the sheet's current minimum sort_order (preserving the
    // batch's own internal order), instead of the default append-at-end
    // (`max_sort_order + (i+1)*GAP`). Computed up front, inside this same
    // transaction — no separate post-insert UPDATE/re-sort step, so
    // `position: 'start'` stays part of the same all-or-nothing batch.
    let sort_orders: Vec<i64> = if at_start {
        let min_sort_order: i64 = match conn.query_row(
            "SELECT COALESCE(MIN(sort_order), 1000) FROM sheet_rows WHERE note_id = ?1",
            params![note_id],
            |r| r.get(0),
        ) {
            Ok(v) => v,
            Err(e) => {
                conn.execute("ROLLBACK", []).ok();
                return Err(format!("create_sheet_rows_batch: min sort_order query failed: {}", e));
            }
        };
        (0..rows.len())
            .map(|i| min_sort_order - ((rows.len() - i) as i64) * SORT_ORDER_GAP)
            .collect()
    } else {
        let max_sort_order: i64 = match conn.query_row(
            "SELECT COALESCE(MAX(sort_order), -1000) FROM sheet_rows WHERE note_id = ?1",
            params![note_id],
            |r| r.get(0),
        ) {
            Ok(v) => v,
            Err(e) => {
                conn.execute("ROLLBACK", []).ok();
                return Err(format!("create_sheet_rows_batch: max sort_order query failed: {}", e));
            }
        };
        (0..rows.len())
            .map(|i| max_sort_order + ((i + 1) as i64) * SORT_ORDER_GAP)
            .collect()
    };

    let now = chrono::Utc::now().to_rfc3339();
    let mut ids: Vec<String> = Vec::with_capacity(rows.len());

    for (i, sort_order) in sort_orders.into_iter().enumerate() {
        let id = Uuid::new_v4().to_string();

        if let Err(e) = conn.execute(
            "INSERT INTO sheet_rows (id, note_id, sort_order, height, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?4)",
            params![id, note_id, sort_order, now],
        ) {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!("create_sheet_rows_batch: insert failed (index {}): {}", i, e));
        }
        ids.push(id);
    }

    let mut written_cells: u32 = 0;
    let mut any_value_written = false;
    for (i, spec) in rows.iter().enumerate() {
        let values = match &spec.values {
            Some(v) if !v.is_empty() => v,
            _ => continue,
        };
        let row_id = &ids[i];
        for (column_id, value) in values {
            match upsert_sheet_cell_no_recompute(conn, row_id, column_id, Some(value.as_str()), None) {
                Ok(_) => {
                    any_value_written = true;
                    written_cells += 1;
                }
                Err(e) => {
                    conn.execute("ROLLBACK", []).ok();
                    return Err(format!(
                        "add_sheet_rows: invalid value at row {} column {}: {}",
                        i, column_id, e
                    ));
                }
            }
        }
    }

    // ONE recompute+regen for the whole batch, only if any value was
    // written — see doc comment above for why this is gated (unlike the
    // column batch's unconditional call).
    if any_value_written {
        if let Err(e) = recompute_and_regen(conn, note_id) {
            conn.execute("ROLLBACK", []).ok();
            return Err(e);
        }
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("create_sheet_rows_batch: failed to commit: {}", e));
    }

    let created_rows: Vec<SheetRow> = match ids.iter().map(|id| get_sheet_row_by_id(conn, id)).collect() {
        Ok(v) => v,
        Err(e) => return Err(e),
    };
    Ok((created_rows, written_cells))
}

/// Tauri wrapper for `create_sheet_rows_batch_internal` (item 23 /
/// #2874). Ghost-row commit's batch endpoint — creates `count` blank
/// rows in one transaction so a value typed into ghost row N lands in
/// the Nth newly-created row, not the first. Existing single-insert
/// `create_sheet_row` is UNCHANGED and still used by the toolbar/
/// context-menu's single-row insert flows.
#[tauri::command]
pub fn create_sheet_rows(
    note_id: String,
    count: i64,
    db: State<DbState>,
) -> Result<Vec<SheetRow>, String> {
    eprintln!(
        "[SHEETS] create_sheet_rows called with: note_id={}, count={}",
        note_id, count
    );
    if count < 0 {
        return Err(format!("create_sheet_rows: count must be >= 0, got {}", count));
    }
    let blank_specs: Vec<RowSpec> = (0..count as usize).map(|_| RowSpec { values: None }).collect();
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    let (rows, _written_cells) = create_sheet_rows_batch_internal(&conn, &note_id, &blank_specs, false)?;
    Ok(rows)
}

/// Update a row's `height` (#2339 PR 13, §33.3.3). Mirrors
/// `update_sheet_column_internal`'s exact shape (partial-update signature,
/// structural-mutation snapshot bypass, transaction wrapping) with one
/// deliberate difference: **no `recompute_and_regen` call.** Row height
/// changes zero formula references and zero body-serialized text — unlike
/// column name/type changes (which affect the body's header line and cell
/// display), a row's pixel height has no computed-value or body-
/// serialization impact at all. Matt's ruling 2026-09-04, per Reed's PR 13
/// scope-report recommendation (§5.2): skip the recompute/regen chokepoint
/// entirely for this command rather than over-regen "for simplicity" the
/// way `update_sheet_column` does for width-only changes.
pub(crate) fn update_sheet_row_internal(
    conn: &Connection,
    row_id: &str,
    height: Option<i64>,
) -> Result<SheetRow, String> {
    let current = get_sheet_row_by_id(conn, row_id)?;

    let new_height = height.unwrap_or(current.height);

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("update_sheet_row: failed to begin transaction: {}", e))?;

    // Structural mutation (#2339 PR 13, mirrors PR 8's §33.9.2 column-
    // update precedent): snapshot capture bypasses the throttle — capture
    // failure rolls back the whole mutation.
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, &current.note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute(
        "UPDATE sheet_rows SET height = ?1, updated_at = datetime('now') WHERE id = ?2",
        params![new_height, row_id],
    ) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("update_sheet_row: update failed: {}", e));
    }

    // Deliberately NO recompute_and_regen call here — see doc comment above.

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("update_sheet_row: failed to commit: {}", e));
    }

    get_sheet_row_by_id(conn, row_id)
}

/// Update a row's `height`. Optional field — a `None` preserves the
/// current stored height (mirrors `update_sheet_column`'s partial-update
/// contract). Does NOT trigger body regen (see `update_sheet_row_internal`
/// doc comment).
#[tauri::command]
pub fn update_sheet_row(
    row_id: String,
    height: Option<i64>,
    db: State<DbState>,
) -> Result<SheetRow, String> {
    eprintln!("[SHEETS] update_sheet_row called with: row_id={}", row_id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    update_sheet_row_internal(&conn, &row_id, height)
}

pub(crate) fn delete_sheet_row_internal(conn: &Connection, row_id: &str) -> Result<(), String> {
    let note_id: String = conn
        .query_row("SELECT note_id FROM sheet_rows WHERE id = ?1", params![row_id], |r| r.get(0))
        .map_err(|e| format!("delete_sheet_row: row not found (id={}): {}", row_id, e))?;

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("delete_sheet_row: failed to begin transaction: {}", e))?;

    // Structural mutation (#2339 PR 8, §33.9.2): snapshot capture bypasses
    // the throttle — capture failure rolls back the whole mutation.
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, &note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    // §1.12.2 point 4: cascade-delete sheet_cells with this row_id, same txn.
    if let Err(e) = conn.execute("DELETE FROM sheet_cells WHERE row_id = ?1", params![row_id]) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("delete_sheet_row: cell cascade delete failed: {}", e));
    }

    if let Err(e) = conn.execute("DELETE FROM sheet_rows WHERE id = ?1", params![row_id]) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("delete_sheet_row: row delete failed: {}", e));
    }

    // 1. Recompute formulas (full graph rebuild — Matt ruling 2026-09-03)
    // 2. Body regen with computed values
    if let Err(e) = recompute_and_regen(conn, &note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("delete_sheet_row: failed to commit: {}", e));
    }

    Ok(())
}

/// Delete a row. Cascade-deletes all `sheet_cells` with that `row_id`
/// (§1.12.2 point 4) in the same transaction. Triggers body regen.
#[tauri::command]
pub fn delete_sheet_row(row_id: String, db: State<DbState>) -> Result<(), String> {
    eprintln!("[SHEETS] delete_sheet_row called with: row_id={}", row_id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    delete_sheet_row_internal(&conn, &row_id)
}

pub(crate) fn reorder_sheet_rows_internal(
    conn: &Connection,
    note_id: &str,
    row_ids: &[String],
) -> Result<(), String> {
    if row_ids.is_empty() {
        return Ok(());
    }

    for row_id in row_ids {
        let owner: Option<String> = conn
            .query_row("SELECT note_id FROM sheet_rows WHERE id = ?1", params![row_id], |r| r.get(0))
            .optional()
            .map_err(|e| format!("reorder_sheet_rows: lookup failed for {}: {}", row_id, e))?;

        match owner {
            None => return Err(format!("reorder_sheet_rows: row {} not found", row_id)),
            Some(owner_note_id) if owner_note_id != note_id => {
                return Err(format!(
                    "reorder_sheet_rows: row {} does not belong to note {}",
                    row_id, note_id
                ));
            }
            _ => {}
        }
    }

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("reorder_sheet_rows: failed to begin transaction: {}", e))?;

    // Structural mutation (Matt ruling 2026-09-04): reorders bypass the
    // throttle — same rationale as `reorder_sheet_columns_internal`
    // (range membership depends on row order, so reorders can silently
    // change formula results). Capture failure rolls back the mutation.
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, note_id, true) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    for (i, row_id) in row_ids.iter().enumerate() {
        let sort_order = (i as i64) * SORT_ORDER_GAP;
        if let Err(e) = conn.execute(
            "UPDATE sheet_rows SET sort_order = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![sort_order, row_id],
        ) {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!("reorder_sheet_rows: update failed for {}: {}", row_id, e));
        }
    }

    // Row order changes body's row-line order — regen.
    // 1. Recompute formulas (full graph rebuild — Matt ruling 2026-09-03)
    // 2. Body regen with computed values
    if let Err(e) = recompute_and_regen(conn, note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("reorder_sheet_rows: failed to commit: {}", e));
    }

    Ok(())
}

/// Reorder rows: accepts an ordered list of row IDs, rewrites `sort_order`
/// via gap-1000 resequence. Scope-validated + transactional, mirrors
/// `reorder_sheet_columns`.
#[tauri::command]
pub fn reorder_sheet_rows(
    note_id: String,
    row_ids: Vec<String>,
    db: State<DbState>,
) -> Result<(), String> {
    eprintln!("[SHEETS] reorder_sheet_rows called with: note_id={}, count={}", note_id, row_ids.len());
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    reorder_sheet_rows_internal(&conn, &note_id, &row_ids)
}

// === Cell commands ===

/// Upsert a cell's `value` and/or `style_json` on the composite PK
/// `(row_id, column_id)` (§1.12.1 — "the natural key IS the PK"). Both
/// fields optional — a field left `None` keeps its current stored value
/// (or the sparse default if the cell doesn't exist yet).
///
/// Sparse-storage rule (§33.6/§1.12): if the resulting `value` is empty
/// AND `style_json` is the default `{}`, the row is DELETED instead of
/// upserted with an empty string — "a `sheet_cells` row exists iff the
/// cell has a value or a non-default style."
///
/// Formula values (`=`-prefixed) are stored verbatim in `value` (their
/// syntax is validated — see §33.5.3 parse-on-write note below — but the
/// text itself is not rewritten/canonicalized); `computed_value` is never
/// persisted anywhere (ADR-0066 D4) — see module-level comment.
///
/// **§33.5.3 parse-on-write validation (#2339 PR 7, Deliverable 4):**
/// Local formula edits are validated at write time — parse failures
/// rejected inline, before any DB write, returning an error to the
/// frontend. Sync/AI-originated formulas bypass validation (already
/// committed by cr-sqlite, arriving via `apply_changesets` — this
/// function is never in that call chain) and display #PARSE! on
/// recompute failure instead (see `recompute::recompute_sheet`'s
/// per-cell parse-failure handling).
///
/// Body regen fires ONLY when the effective `value` changes — pure style
/// changes do NOT trigger regen (Matt ruling 2026-09-03, style isn't part
/// of the flat-text serialization).
/// Raw per-cell write result (#2874 item 51) — everything a cell write
/// needs to know about what it just did, short of recompute/regen/commit.
/// Returned by `upsert_sheet_cell_no_recompute` below.
pub(crate) struct CellRawWrite {
    pub note_id: String,
    pub new_value: String,
    pub new_style: String,
    pub value_changed: bool,
}

/// The per-cell write chokepoint's RAW half (#2874 item 51) — reference
/// translation, §33.5.3 parse-on-write validation, existing-row lookup,
/// and the sparse-delete-or-upsert write itself. Extracted verbatim from
/// what `update_sheet_cell_internal` used to do inline (that function now
/// calls this, then wraps it with its OWN transaction/snapshot/recompute/
/// regen/commit — single-cell behavior is UNCHANGED, see that function's
/// own doc comment).
///
/// Does NOT begin/commit a transaction, capture a snapshot, recompute, or
/// regenerate the body — the caller owns all of that, for exactly one
/// cell or batched across N. MUST be called inside an already-open
/// transaction (the snapshot capture that must precede any write has
/// already happened by the time a caller reaches this).
///
/// **Named explicitly for #2970 reuse (Matt's ruling, PR body):** this is
/// the reusable "cell upsert that doesn't recalculate" piece. #2970's
/// value-filling batch tool should call this per cell with
/// `value: Some(...)`, collecting each entry's `value_changed`, then call
/// `recompute_and_regen` (`sheets.rs:442`) ONCE after the loop iff ANY
/// entry's `value_changed` is true — the same two-piece split
/// `update_sheet_cells_style_batch_internal` below uses (loop this, then
/// ONE recompute call at the end), just gated on whether body regen is
/// needed instead of always skipping it. No new "recompute without regen"
/// primitive needs to be built for that — `recompute::recompute_sheet`
/// (called directly, below) and `regenerate_sheet_body` (wrapped by
/// `recompute_and_regen`) already exist as the two separable halves.
pub(crate) fn upsert_sheet_cell_no_recompute(
    conn: &Connection,
    row_id: &str,
    column_id: &str,
    value: Option<&str>,
    style_json: Option<&str>,
) -> Result<CellRawWrite, String> {
    let note_id: String = conn
        .query_row("SELECT note_id FROM sheet_rows WHERE id = ?1", params![row_id], |r| r.get(0))
        .map_err(|e| format!("upsert_sheet_cell: row not found (row_id={}): {}", row_id, e))?;

    // Validate column belongs to the same note (defensive — no FK to catch this).
    let column_note_id: String = conn
        .query_row("SELECT note_id FROM sheet_columns WHERE id = ?1", params![column_id], |r| r.get(0))
        .map_err(|e| format!("upsert_sheet_cell: column not found (column_id={}): {}", column_id, e))?;
    if column_note_id != note_id {
        return Err(format!(
            "upsert_sheet_cell: row {} and column {} belong to different sheets",
            row_id, column_id
        ));
    }

    // #2339 PR 14, §33.5.2, Matt ruling 2026-09-05 #8: the ReferenceMapper
    // sits HERE, at this write chokepoint — BEFORE
    // `validate_formula_on_write` — so both the human bar/cell path AND
    // the AI-tool `set_sheet_cells` path (which calls this same function)
    // translate A1-style references (`A1`, `B2:B9`) to canonical
    // `{column_id:row_id}` stable-ID form before storage. An A1 reference
    // pointing past the sheet's current bounds is rejected right here,
    // inline, with the same error surface a parse rejection uses (ruling
    // #3: "a ref stores as a permanent cell ID; a nonexistent cell has no
    // ID to store") — nothing partial is ever written. Only
    // `apply_changesets` (sync replay) never reaches this function at all
    // and stays untranslated, per ruling #8.
    let translated_value: Option<String> = match value {
        Some(v) if v.starts_with('=') => {
            let ordered_column_ids = load_ordered_column_ids(conn, &note_id)?;
            let ordered_row_ids = load_ordered_row_ids(conn, &note_id)?;
            Some(crate::formula::reference_mapper::translate_formula_a1_to_ids(
                v,
                &ordered_column_ids,
                &ordered_row_ids,
            )?)
        }
        Some(v) => Some(v.to_string()),
        None => None,
    };
    let value: Option<&str> = translated_value.as_deref();

    // §33.5.3 parse-on-write: reject an unparseable formula inline,
    // BEFORE any DB write — the unparseable string is never stored. Only
    // applies when `value` is explicitly being set (a style-only edit,
    // `value: None`, has nothing to validate). Runs AFTER reference
    // translation above — `translate_formula_a1_to_ids` already parses
    // the formula internally (and rejects unparseable input itself), so
    // this is a defensive re-check against the (already-translated)
    // stored form, matching the existing contract for non-`=`-prefixed /
    // non-formula values.
    if let Some(v) = value {
        recompute::validate_formula_on_write(v)?;
    }

    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT value, style_json FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![row_id, column_id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|e| format!("upsert_sheet_cell: existing-cell lookup failed: {}", e))?;

    let (existing_value, existing_style) = existing
        .clone()
        .unwrap_or_else(|| (String::new(), "{}".to_string()));

    let new_value = value.unwrap_or(&existing_value).to_string();
    let new_style = style_json.unwrap_or(&existing_style).to_string();
    let value_changed = new_value != existing_value;

    let is_sparse = new_value.is_empty() && new_style == "{}";

    if is_sparse {
        conn.execute(
            "DELETE FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![row_id, column_id],
        )
        .map_err(|e| format!("upsert_sheet_cell: sparse delete failed: {}", e))?;
    } else {
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id, value, style_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
             ON CONFLICT(row_id, column_id) DO UPDATE SET
                value = excluded.value,
                style_json = excluded.style_json,
                updated_at = excluded.updated_at",
            params![row_id, column_id, note_id, new_value, new_style],
        )
        .map_err(|e| format!("upsert_sheet_cell: upsert failed: {}", e))?;
    }

    Ok(CellRawWrite { note_id, new_value, new_style, value_changed })
}

pub(crate) fn update_sheet_cell_internal(
    conn: &Connection,
    row_id: &str,
    column_id: &str,
    value: Option<&str>,
    style_json: Option<&str>,
) -> Result<SheetCell, String> {
    // Cheap up-front lookup so the snapshot capture below (which MUST
    // precede the write, same ordering as before #2874 item 51's
    // extraction) has a `note_id` to key off — `upsert_sheet_cell_no_
    // recompute` below does its OWN (slightly redundant, one extra
    // indexed SELECT) copy of this same lookup, kept self-contained so a
    // batch caller that doesn't have a trusted note_id for every row up
    // front can still use it safely (see that function's doc comment).
    let note_id: String = conn
        .query_row("SELECT note_id FROM sheet_rows WHERE id = ?1", params![row_id], |r| r.get(0))
        .map_err(|e| format!("update_sheet_cell: row not found (row_id={}): {}", row_id, e))?;

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("update_sheet_cell: failed to begin transaction: {}", e))?;

    // Grid mutation (#2339 PR 8, §33.9.2): throttled, not bypassed — a
    // burst of cell edits produces one snapshot, not fifty. Capture
    // failure rolls back the whole mutation (unlike
    // `maybe_save_manual_version`'s fire-and-log-only pattern for
    // regular notes — capture here runs inside the mutation's own txn).
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, &note_id, false) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    // #2874 item 51: the raw write is the extracted, reusable
    // `upsert_sheet_cell_no_recompute` piece (also used by
    // `update_sheet_cells_style_batch_internal` below, and named for
    // #2970 reuse).
    let write = match upsert_sheet_cell_no_recompute(conn, row_id, column_id, value, style_json) {
        Ok(w) => w,
        Err(e) => {
            conn.execute("ROLLBACK", []).ok();
            return Err(e);
        }
    };

    // #2874 item 47 (Matt ruling, 2026-09-24, superseding item 25's PR
    // amendment below): recompute now runs UNCONDITIONALLY, on every
    // write — including a style-only one (`value: None`,
    // `value_changed == false`) — so this function's reply always
    // carries a formula cell's current computed value, exactly like a
    // normal read. The recompute cost this reintroduces on every styled
    // cell (the exact cost item 25's amendment comment below was written
    // to avoid) is explicitly accepted by Matt's ruling: "I accept the
    // recompute cost" — computed values are never persisted (ADR-0066
    // D4), so there is no cheaper cache to consult instead. Body regen
    // (the actual DB write to `notes.body`) stays gated on `value_changed`
    // only — Matt ruling 2026-09-03 is UNCHANGED: a pure style change
    // still never touches `notes.body`/`updated_at`.
    //
    // (Historical context, #2874 item 25's original scoped-down PR
    // amendment comment: recompute used to run only when THIS write
    // actually changed `raw_value`, returning `computed_value: None` on
    // every style-only write and leaving item 27 — preserving an existing
    // computed value, e.g. a live `#REF!` error, across a style-only
    // write — to a frontend patch, `SheetGrid.svelte`'s
    // `preserveComputedValueOnStyleOnlyWrite`. Item 47 supersedes both:
    // that frontend helper is deleted, since its very premise — "the
    // backend doesn't tell you" — is no longer true.)
    let computed = match recompute::recompute_sheet(conn, &note_id) {
        Ok(c) => c,
        Err(e) => {
            conn.execute("ROLLBACK", []).ok();
            return Err(e);
        }
    };

    if write.value_changed {
        // Body regen with this pass's freshly computed values — only for
        // an actual value-changing write (Matt ruling 2026-09-03,
        // unaffected by item 47).
        if let Err(e) = regenerate_sheet_body(conn, &note_id, &computed) {
            conn.execute("ROLLBACK", []).ok();
            return Err(e);
        }
    }

    // Only a formula cell (`new_value` starts with `=`) ever gets a
    // `computed_value` — matches `attach_computed_values`'/`cellKind`'s
    // existing contract (§33.5.4): non-formula cells are always `None`.
    let computed_value = if write.new_value.starts_with('=') {
        computed.get(&(column_id.to_string(), row_id.to_string())).map(|v| v.to_string())
    } else {
        None
    };

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("update_sheet_cell: failed to commit: {}", e));
    }

    // READ-path-equivalent translation (#2874 item 47) — REUSES
    // `translate_cells_for_display` (the exact function
    // `get_sheet_data_internal` calls for every cell on a read) rather
    // than duplicating its ID-to-A1 rewrite logic. Every write's reply
    // now carries `raw_value` in current A1 display form, never the
    // canonical `{column_id:row_id}` storage form — true for a
    // style-only write's pre-existing formula just as much as for a
    // freshly-written one, since a style-only write's `raw_value` is
    // read back out of storage unchanged.
    let ordered_column_ids = load_ordered_column_ids(conn, &note_id)?;
    let ordered_row_ids = load_ordered_row_ids(conn, &note_id)?;
    let mut result_cells = vec![SheetCell {
        row_id: row_id.to_string(),
        column_id: column_id.to_string(),
        note_id: note_id.clone(),
        raw_value: write.new_value,
        computed_value,
        style_json: write.new_style,
        updated_at: chrono::Utc::now().to_rfc3339(),
    }];
    translate_cells_for_display(&mut result_cells, &ordered_column_ids, &ordered_row_ids);

    Ok(result_cells.remove(0))
}

/// One entry in a style batch write (#2874 item 51) — `row_id`/
/// `column_id` identify the cell, `style_json` is the already fully
/// resolved style the frontend computed (the per-cell border/fill/ink
/// merge math in `SheetGrid.svelte`'s `applyStyleToSelection` is
/// UNCHANGED by this item — only how the resulting per-cell payloads are
/// DELIVERED to the backend changes, from N `update_sheet_cell` calls to
/// one `update_sheet_cells_style_batch` call carrying N of these).
#[derive(Debug, Deserialize)]
pub struct StyleBatchEntry {
    pub row_id: String,
    pub column_id: String,
    pub style_json: String,
}

/// Batched style-only cell write (#2874 item 51). Matt's ruling: "a batch
/// style write in one transaction, with one snapshot check and one
/// recalculation for the whole batch, and no body regeneration
/// (style-only, as today)". Follows `create_sheet_columns_batch_internal`
/// (`sheets.rs:1327`)'s shape: ONE transaction, ONE snapshot capture, a
/// loop of raw per-cell writes, ONE recompute, ONE commit.
///
/// **Not `recompute_and_regen`:** every entry here writes `style_json`
/// only (`value: None`), so `upsert_sheet_cell_no_recompute`'s
/// `value_changed` is `false` for every entry, by construction — exactly
/// like a single style-only `update_sheet_cell` write today, which never
/// triggers body regen (Matt ruling 2026-09-03, unaffected). Calling
/// `recompute_and_regen` (`sheets.rs:442`) here would be WRONG: it
/// unconditionally calls `regenerate_sheet_body` regardless of whether
/// any value changed, which would reintroduce a body regen style writes
/// don't do today. `recompute::recompute_sheet` is called directly
/// instead — the "recompute half without the regen half" the brief asks
/// for already exists as a separate function; `recompute_and_regen` was
/// only ever a thin wrapper combining it with `regenerate_sheet_body`, so
/// no new primitive was needed here.
///
/// Every returned cell carries its current computed value and
/// A1-translated raw formula (item 47) via `translate_cells_for_display`
/// — the SAME function the single-cell path and the read path use, not
/// duplicated here.
///
/// Mid-batch failure (an entry's row/column doesn't exist, or belongs to
/// a different sheet than `note_id`) rolls back the ENTIRE batch — no
/// entry before the failing one is left written, matching the column/row
/// batch functions' atomicity.
pub(crate) fn update_sheet_cells_style_batch_internal(
    conn: &Connection,
    note_id: &str,
    entries: &[(String, String, String)], // (row_id, column_id, style_json)
) -> Result<Vec<SheetCell>, String> {
    if entries.is_empty() {
        return Ok(vec![]);
    }

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("update_sheet_cells_style_batch: failed to begin transaction: {}", e))?;

    // ONE snapshot for the whole batch (Matt's discipline — not per
    // cell). `bypass_throttle: false` — matches the single-cell style
    // write's own throttle posture (a style batch is the same "cell
    // mutation" category as a single style edit, not the "deliberate,
    // infrequent structural create" category `create_sheet_columns_
    // batch_internal`'s forced capture is for).
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, note_id, false) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    let mut writes: Vec<(String, String, CellRawWrite)> = Vec::with_capacity(entries.len());
    for (i, (row_id, column_id, style_json)) in entries.iter().enumerate() {
        match upsert_sheet_cell_no_recompute(conn, row_id, column_id, None, Some(style_json.as_str())) {
            Ok(w) => {
                if w.note_id != note_id {
                    conn.execute("ROLLBACK", []).ok();
                    return Err(format!(
                        "update_sheet_cells_style_batch: entry {} (row {}, column {}) belongs to a different sheet than note_id={}",
                        i, row_id, column_id, note_id
                    ));
                }
                writes.push((row_id.clone(), column_id.clone(), w));
            }
            Err(e) => {
                conn.execute("ROLLBACK", []).ok();
                return Err(format!("update_sheet_cells_style_batch: write failed (entry {}): {}", i, e));
            }
        }
    }

    // ONE recompute for the whole batch — see doc comment above for why
    // this is `recompute::recompute_sheet` directly, not
    // `recompute_and_regen`.
    let computed = match recompute::recompute_sheet(conn, note_id) {
        Ok(c) => c,
        Err(e) => {
            conn.execute("ROLLBACK", []).ok();
            return Err(e);
        }
    };

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("update_sheet_cells_style_batch: failed to commit: {}", e));
    }

    let ordered_column_ids = load_ordered_column_ids(conn, note_id)?;
    let ordered_row_ids = load_ordered_row_ids(conn, note_id)?;

    let mut result_cells: Vec<SheetCell> = writes
        .into_iter()
        .map(|(row_id, column_id, w)| {
            let computed_value = if w.new_value.starts_with('=') {
                computed.get(&(column_id.clone(), row_id.clone())).map(|v| v.to_string())
            } else {
                None
            };
            SheetCell {
                row_id,
                column_id,
                note_id: w.note_id,
                raw_value: w.new_value,
                computed_value,
                style_json: w.new_style,
                updated_at: chrono::Utc::now().to_rfc3339(),
            }
        })
        .collect();

    translate_cells_for_display(&mut result_cells, &ordered_column_ids, &ordered_row_ids);

    Ok(result_cells)
}

/// Tauri wrapper for `update_sheet_cells_style_batch_internal` (#2874
/// item 51) — batches a selection's style writes into ONE transaction
/// with ONE recalculation, instead of `SheetGrid.svelte`'s old per-cell
/// `update_sheet_cell` loop (one transaction + one full-sheet recompute
/// PER cell, item 47's accepted-at-single-cell-scale cost showing at
/// selection scale). Used by `applyStyleToSelection`/
/// `clearSelectedStyling`.
#[tauri::command]
pub fn update_sheet_cells_style_batch(
    note_id: String,
    entries: Vec<StyleBatchEntry>,
    db: State<DbState>,
) -> Result<Vec<SheetCell>, String> {
    eprintln!(
        "[SHEETS] update_sheet_cells_style_batch called with: note_id={}, entries.len()={}",
        note_id,
        entries.len()
    );
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    let tuples: Vec<(String, String, String)> = entries
        .into_iter()
        .map(|e| (e.row_id, e.column_id, e.style_json))
        .collect();
    update_sheet_cells_style_batch_internal(&conn, &note_id, &tuples)
}

#[tauri::command]
pub fn update_sheet_cell(
    row_id: String,
    column_id: String,
    value: Option<String>,
    style_json: Option<String>,
    db: State<DbState>,
) -> Result<SheetCell, String> {
    eprintln!("[SHEETS] update_sheet_cell called with: row_id={}, column_id={}", row_id, column_id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    update_sheet_cell_internal(&conn, &row_id, &column_id, value.as_deref(), style_json.as_deref())
}

pub(crate) fn delete_sheet_cell_internal(conn: &Connection, row_id: &str, column_id: &str) -> Result<(), String> {
    let note_id: Option<String> = conn
        .query_row("SELECT note_id FROM sheet_rows WHERE id = ?1", params![row_id], |r| r.get(0))
        .optional()
        .map_err(|e| format!("delete_sheet_cell: row lookup failed: {}", e))?;
    let Some(note_id) = note_id else {
        return Err(format!("delete_sheet_cell: row not found (row_id={})", row_id));
    };

    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("delete_sheet_cell: failed to begin transaction: {}", e))?;

    // Grid mutation (#2339 PR 8, §33.9.2): throttled, not bypassed.
    if let Err(e) = crate::sheets_snapshots::maybe_capture_sheet_snapshot(conn, &note_id, false) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute(
        "DELETE FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
        params![row_id, column_id],
    ) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("delete_sheet_cell: delete failed: {}", e));
    }

    // Explicit cell deletion always changes effective value (to empty) —
    // always regen.
    // 1. Recompute formulas (full graph rebuild — Matt ruling 2026-09-03)
    // 2. Body regen with computed values
    if let Err(e) = recompute_and_regen(conn, &note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("delete_sheet_cell: failed to commit: {}", e));
    }

    Ok(())
}

/// Explicit cell delete — removes the sparse cell entry. Triggers body
/// regen. For callers that don't route through `update_sheet_cell`'s
/// clear-to-empty path (e.g. a "clear cell" UI action).
#[tauri::command]
pub fn delete_sheet_cell(row_id: String, column_id: String, db: State<DbState>) -> Result<(), String> {
    eprintln!("[SHEETS] delete_sheet_cell called with: row_id={}, column_id={}", row_id, column_id);
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    delete_sheet_cell_internal(&conn, &row_id, &column_id)
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Real sheet_columns/sheet_rows/sheet_cells DDL (from migrations.rs)
    /// plus a minimal `notes`/`projects` schema — enough for every command
    /// in this module to run unmodified, catching NOT-NULL-without-DEFAULT
    /// mistakes that a simplified stand-in schema would hide.
    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 parent_id TEXT,
                 is_system INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE notes (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL DEFAULT '',
                 body TEXT,
                 content_preview TEXT NOT NULL DEFAULT '',
                 file_path TEXT NOT NULL DEFAULT '',
                 project_id TEXT,
                 folder_id TEXT,
                 status TEXT NOT NULL DEFAULT 'cabinet',
                 priority TEXT,
                 due_date TEXT,
                 assignee TEXT,
                 completed_at TEXT,
                 sort_order INTEGER NOT NULL DEFAULT 0,
                 word_count INTEGER NOT NULL DEFAULT 0,
                 pinned INTEGER NOT NULL DEFAULT 0,
                 owner_id TEXT,
                 tags_json TEXT NOT NULL DEFAULT '[]',
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                 deleted_at TEXT,
                 parent_id TEXT,
                 note_type TEXT
             );
             CREATE TABLE tags (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT UNIQUE NOT NULL
             );
             CREATE TABLE note_tags (
                 note_id TEXT NOT NULL,
                 tag_id INTEGER NOT NULL
             );
             CREATE TABLE sheet_columns (
                 id           TEXT NOT NULL PRIMARY KEY,
                 note_id      TEXT NOT NULL DEFAULT '',
                 name         TEXT NOT NULL DEFAULT '',
                 col_type     TEXT NOT NULL DEFAULT 'text',
                 options_json TEXT NOT NULL DEFAULT '{}',
                 width        INTEGER NOT NULL DEFAULT 0,
                 sort_order   INTEGER NOT NULL DEFAULT 0,
                 created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE sheet_rows (
                 id           TEXT NOT NULL PRIMARY KEY,
                 note_id      TEXT NOT NULL DEFAULT '',
                 sort_order   INTEGER NOT NULL DEFAULT 0,
                 height       INTEGER NOT NULL DEFAULT 0,
                 created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE sheet_cells (
                 row_id     TEXT NOT NULL,
                 column_id  TEXT NOT NULL,
                 note_id    TEXT NOT NULL DEFAULT '',
                 value      TEXT NOT NULL DEFAULT '',
                 style_json TEXT NOT NULL DEFAULT '{}',
                 updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (row_id, column_id)
             );
             CREATE TABLE note_versions (
                 id         TEXT NOT NULL PRIMARY KEY,
                 note_id    TEXT NOT NULL DEFAULT '',
                 content    TEXT NOT NULL DEFAULT '',
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 source     TEXT NOT NULL DEFAULT 'manual'
             );
             CREATE INDEX IF NOT EXISTS idx_note_versions_note_id
                 ON note_versions(note_id);",
        )
        .unwrap();
        conn
    }

    fn insert_project(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO projects (id, name, is_system) VALUES (?1, ?1, 0)",
            params![id],
        )
        .unwrap();
    }

    fn make_sheet(conn: &Connection, project_id: &str) -> String {
        let req = crate::notes::CreateSheetNoteRequest {
            project_id: project_id.to_string(),
            folder_id: None,
            title: "Test Sheet".to_string(),
            tags: None,
            status: Some("cabinet".to_string()),
            priority: None,
            due_date: None,
        };
        let data = create_sheet_internal(conn, req, None).expect("create_sheet_internal should succeed");
        data.note_meta.id
    }

    /// A sheet note with ZERO columns/rows — bypasses `create_sheet_internal`'s
    /// default-column seeding (`create_sheet_note_internal` directly) so
    /// positional-insert tests can seed rows/columns at exact, controlled
    /// `sort_order`s without the 5 default columns' 0/1000/2000/3000/4000
    /// getting in the way.
    fn make_sheet_without_default_columns(conn: &Connection, project_id: &str) -> String {
        let req = crate::notes::CreateSheetNoteRequest {
            project_id: project_id.to_string(),
            folder_id: None,
            title: "Test Sheet (no defaults)".to_string(),
            tags: None,
            status: Some("cabinet".to_string()),
            priority: None,
            due_date: None,
        };
        let meta = crate::notes::create_sheet_note_internal(conn, req).expect("create_sheet_note_internal should succeed");
        meta.id
    }

    /// Inserts a column directly at an exact `sort_order`, bypassing
    /// `create_sheet_column_internal`'s append/positional-insert math —
    /// used to seed precise pre-conditions (e.g. two columns at 0 and 1)
    /// for the renumber-trigger tests.
    fn seed_column_at(conn: &Connection, note_id: &str, name: &str, sort_order: i64) -> String {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO sheet_columns (id, note_id, name, col_type, options_json, width, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'text', '{}', 0, ?4, ?5, ?5)",
            params![id, note_id, name, sort_order, now],
        )
        .unwrap();
        id
    }

    /// Row-side mirror of `seed_column_at`.
    fn seed_row_at(conn: &Connection, note_id: &str, sort_order: i64) -> String {
        let id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO sheet_rows (id, note_id, sort_order, height, created_at, updated_at)
             VALUES (?1, ?2, ?3, 0, ?4, ?4)",
            params![id, note_id, sort_order, now],
        )
        .unwrap();
        id
    }

    // ── create_sheet ─────────────────────────────────────────────────────

    #[test]
    fn test_create_sheet_seeds_default_columns_and_status() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");

        let req = crate::notes::CreateSheetNoteRequest {
            project_id: "proj-1".to_string(),
            folder_id: None,
            title: "Budget".to_string(),
            tags: None,
            status: None, // exercise the None-coalesce path
            priority: None,
            due_date: None,
        };

        // Simulate the create_sheet command's coalesce (not calling the
        // #[tauri::command] wrapper directly since it needs a State<DbState>).
        let mut req = req;
        if req.status.is_none() {
            req.status = Some("cabinet".to_string());
        }

        let data = create_sheet_internal(&conn, req, None).expect("create_sheet_internal should succeed");

        assert_eq!(data.note_meta.note_type.as_deref(), Some("sheet"));
        assert_eq!(data.note_meta.status.as_deref(), Some("cabinet"));
        assert_eq!(data.columns.len(), DEFAULT_SHEET_COLUMNS, "must seed DEFAULT_SHEET_COLUMNS columns");
        for col in &data.columns {
            assert_eq!(col.col_type, "text", "all default columns must be text type");
        }
        assert_eq!(data.rows.len(), 0, "fresh sheet has zero rows");
        assert_eq!(data.cells.len(), 0, "fresh sheet has zero cells");

        // Body must reflect the header line from the seeded columns.
        let body: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&data.note_meta.id], |r| r.get(0)).unwrap();
        assert!(body.contains("Column 1"), "body header must include seeded column names");
    }

    #[test]
    fn test_create_sheet_rejects_none_status_via_coalesce_not_null_violation() {
        // Proves the coalesce is load-bearing: calling create_sheet_note_internal
        // directly (bypassing the coalesce) with status=None must fail with a
        // NOT NULL violation, confirming why create_sheet's wrapper-level
        // coalesce (Required Item 4) is necessary.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let req = crate::notes::CreateSheetNoteRequest {
            project_id: "proj-1".to_string(),
            folder_id: None,
            title: "No Status".to_string(),
            tags: None,
            status: None,
            priority: None,
            due_date: None,
        };
        let result = crate::notes::create_sheet_note_internal(&conn, req);
        assert!(result.is_err(), "status=None must violate NOT NULL without the create_sheet coalesce");
    }

    // ── Column CRUD ──────────────────────────────────────────────────────

    #[test]
    fn test_create_sheet_column_appends_with_gap_sort_order() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let col = create_sheet_column_internal(&conn, &note_id, "Amount", "number", None, None, None)
            .expect("create_sheet_column should succeed");

        assert_eq!(col.name, "Amount");
        assert_eq!(col.col_type, "number");
        // 5 default columns at sort_order 0,1000,2000,3000,4000 — new one appends at 5000.
        assert_eq!(col.sort_order, (DEFAULT_SHEET_COLUMNS as i64) * SORT_ORDER_GAP);
    }

    #[test]
    fn test_create_sheet_column_rejects_invalid_col_type() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let result = create_sheet_column_internal(&conn, &note_id, "Bad", "not_a_type", None, None, None);
        assert!(result.is_err(), "invalid col_type must be rejected");

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert_eq!(count, DEFAULT_SHEET_COLUMNS as i64, "no column should be inserted on rejection");
    }

    #[test]
    fn test_create_sheet_column_rejects_mismatched_reference_and_position() {
        // One Some, one None must error — never silently fall back to
        // either append or positional-insert mode (§33.3.4 contract).
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();

        let result = create_sheet_column_internal(&conn, &note_id, "B", "text", None, Some(&col.id), None);
        assert!(result.is_err(), "reference_id without position must be rejected");

        let result2 = create_sheet_column_internal(&conn, &note_id, "B", "text", None, None, Some(InsertPosition::Before));
        assert!(result2.is_err(), "position without reference_id must be rejected");
    }

    #[test]
    fn test_insert_column_left_midpoint() {
        // Two default columns already exist at sort_order 0 and 1000 (the
        // seeded set has 5, but the first two are exactly what the spec
        // example describes) — insert before the second: midpoint(0, 1000) = 500.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let second_col_id: String = conn
            .query_row(
                "SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1 OFFSET 1",
                params![&note_id],
                |r| r.get(0),
            )
            .unwrap();

        let new_col = create_sheet_column_internal(
            &conn, &note_id, "Inserted", "text", None, Some(&second_col_id), Some(InsertPosition::Before),
        )
        .expect("positional insert should succeed");

        assert_eq!(new_col.sort_order, 500, "midpoint of 0 and 1000 must be 500");
    }

    #[test]
    fn test_insert_column_left_of_first_column() {
        // Seed a fresh sheet with a single column at sort_order 1000
        // (simulating a sheet whose first column isn't at 0) — insert
        // before it: midpoint(0, 1000) = 500.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet_without_default_columns(&conn, "proj-1");
        seed_column_at(&conn, &note_id, "Only", 1000);
        let only_col_id: String = conn
            .query_row("SELECT id FROM sheet_columns WHERE note_id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();

        let new_col = create_sheet_column_internal(
            &conn, &note_id, "Inserted", "text", None, Some(&only_col_id), Some(InsertPosition::Before),
        )
        .expect("positional insert should succeed");
        assert_eq!(new_col.sort_order, 500, "midpoint of 0 and 1000 must be 500");

        // Separately: a column at sort_order 0 (the true first-ever
        // column) — inserting before it must hit the renumber path since
        // midpoint(0, 0) collides with the reference itself.
        let note_id_2 = make_sheet_without_default_columns(&conn, "proj-1");
        seed_column_at(&conn, &note_id_2, "AtZero", 0);
        let zero_col_id: String = conn
            .query_row("SELECT id FROM sheet_columns WHERE note_id = ?1", params![&note_id_2], |r| r.get(0))
            .unwrap();

        let new_col_2 = create_sheet_column_internal(
            &conn, &note_id_2, "Inserted2", "text", None, Some(&zero_col_id), Some(InsertPosition::Before),
        )
        .expect("positional insert against a sort_order=0 reference must renumber and succeed");
        assert_ne!(new_col_2.sort_order, 0, "post-renumber midpoint must not collide with the reference");

        let renumbered_zero_col_sort_order: i64 = conn
            .query_row("SELECT sort_order FROM sheet_columns WHERE id = ?1", params![&zero_col_id], |r| r.get(0))
            .unwrap();
        assert_ne!(new_col_2.sort_order, renumbered_zero_col_sort_order, "new column must not collide with the renumbered reference either");
    }

    #[test]
    fn test_insert_column_triggers_renumber_when_gap_exhausted() {
        // Two adjacent columns at sort_order 0 and 1 (no integer slot
        // between them) — insert before the second must renumber ALL
        // columns in the sheet to fresh i*1000 gaps first, then compute
        // the midpoint against the post-renumber neighbours.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet_without_default_columns(&conn, "proj-1");
        seed_column_at(&conn, &note_id, "First", 0);
        seed_column_at(&conn, &note_id, "Second", 1);

        let mut stmt = conn.prepare("SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC").unwrap();
        let ids: Vec<String> = stmt.query_map(params![&note_id], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        let (first_id, second_id) = (ids[0].clone(), ids[1].clone());

        let new_col = create_sheet_column_internal(
            &conn, &note_id, "Inserted", "text", None, Some(&second_id), Some(InsertPosition::Before),
        )
        .expect("gap-exhausted positional insert must renumber and succeed");

        // Post-renumber (1-indexed — see renumber_sheet_columns doc comment):
        // First -> 1000, Second -> 2000, new column's midpoint against
        // those is (1000 + 2000) / 2 = 1500.
        let first_sort_order: i64 = conn
            .query_row("SELECT sort_order FROM sheet_columns WHERE id = ?1", params![&first_id], |r| r.get(0))
            .unwrap();
        let second_sort_order: i64 = conn
            .query_row("SELECT sort_order FROM sheet_columns WHERE id = ?1", params![&second_id], |r| r.get(0))
            .unwrap();
        assert_eq!(first_sort_order, SORT_ORDER_GAP, "renumber must resequence the first column to fresh gap-1000");
        assert_eq!(second_sort_order, 2 * SORT_ORDER_GAP, "renumber must resequence the second column to fresh gap-2000");
        assert_eq!(new_col.sort_order, 1500, "new column's sort_order must be the post-renumber midpoint");
    }

    #[test]
    fn test_insert_column_rejects_reference_from_different_note() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_a = make_sheet(&conn, "proj-1");
        let note_b = make_sheet(&conn, "proj-1");

        let col_from_b: String = conn
            .query_row("SELECT id FROM sheet_columns WHERE note_id = ?1 LIMIT 1", params![&note_b], |r| r.get(0))
            .unwrap();

        let result = create_sheet_column_internal(
            &conn, &note_a, "Inserted", "text", None, Some(&col_from_b), Some(InsertPosition::Before),
        );
        assert!(result.is_err(), "reference_id from a different note must be rejected");
    }

    #[test]
    fn test_update_sheet_column_changes_name_type_width() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "Original", "text", None, None, None).unwrap();

        let updated = update_sheet_column_internal(
            &conn,
            &col.id,
            Some("Renamed"),
            Some("currency"),
            Some(r#"{"currency":"USD"}"#),
            Some(150),
        )
        .expect("update should succeed");

        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.col_type, "currency");
        assert_eq!(updated.options_json, r#"{"currency":"USD"}"#);
        assert_eq!(updated.width, 150);
    }

    #[test]
    fn test_update_sheet_column_partial_update_preserves_unspecified_fields() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "Original", "text", None, None, None).unwrap();

        let updated = update_sheet_column_internal(&conn, &col.id, Some("NewName"), None, None, None)
            .expect("update should succeed");

        assert_eq!(updated.name, "NewName");
        assert_eq!(updated.col_type, "text", "unspecified col_type must be preserved");
        assert_eq!(updated.width, 0, "unspecified width must be preserved");
    }

    #[test]
    fn test_delete_sheet_column_cascades_cells_and_preserves_others() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col_a = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let col_b = create_sheet_column_internal(&conn, &note_id, "B", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row.id, &col_a.id, Some("val-a"), None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col_b.id, Some("val-b"), None).unwrap();

        delete_sheet_column_internal(&conn, &col_a.id).expect("delete should succeed");

        let col_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns WHERE id = ?1", params![&col_a.id], |r| r.get(0)).unwrap();
        assert_eq!(col_count, 0, "deleted column must be gone");

        let cell_a_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sheet_cells WHERE column_id = ?1",
            params![&col_a.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(cell_a_count, 0, "cells for deleted column must be cascaded");

        let cell_b: String = conn.query_row(
            "SELECT value FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![&row.id, &col_b.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(cell_b, "val-b", "other column's cell must survive");
    }

    #[test]
    fn test_reorder_sheet_columns_rewrites_sort_order() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        // Grab the 5 default columns, reverse their order.
        let mut stmt = conn.prepare("SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC").unwrap();
        let ids: Vec<String> = stmt.query_map(params![&note_id], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        let mut reversed = ids.clone();
        reversed.reverse();

        reorder_sheet_columns_internal(&conn, &note_id, &reversed).expect("reorder should succeed");

        let mut stmt2 = conn.prepare("SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC").unwrap();
        let after: Vec<String> = stmt2.query_map(params![&note_id], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(after, reversed, "sort_order must reflect the new order");
    }

    #[test]
    fn test_reorder_sheet_columns_rejects_column_from_different_note() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_a = make_sheet(&conn, "proj-1");
        let note_b = make_sheet(&conn, "proj-1");

        let col_from_b: String = conn.query_row(
            "SELECT id FROM sheet_columns WHERE note_id = ?1 LIMIT 1",
            params![&note_b],
            |r| r.get(0),
        ).unwrap();

        let result = reorder_sheet_columns_internal(&conn, &note_a, &[col_from_b]);
        assert!(result.is_err(), "cross-sheet column id must be rejected");
    }

    // ── Column batch create (item 23 / #2874) ───────────────────────────

    /// #2970 — `count` blank `ColumnSpec`s, mirroring `create_sheet_columns`'s
    /// ghost-column-commit default shape (empty name, text type, no options).
    fn blank_column_specs(count: usize) -> Vec<ColumnSpec> {
        (0..count).map(|_| ColumnSpec { name: String::new(), col_type: "text".to_string(), options: None }).collect()
    }

    #[test]
    fn test_create_sheet_columns_batch_lands_at_max_plus_i_times_gap() {
        // Matt's assertion 1: N columns land at max_sort_order + (i+1)*GAP.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        // 5 default columns already exist at sort_order 0,1000,2000,3000,4000.
        let max_before = (DEFAULT_SHEET_COLUMNS as i64 - 1) * SORT_ORDER_GAP;

        let batch = create_sheet_columns_batch_internal(&conn, &note_id, &blank_column_specs(3))
            .expect("batch create should succeed");

        assert_eq!(batch.len(), 3, "must create exactly N columns");
        for (i, col) in batch.iter().enumerate() {
            let expected = max_before + (i as i64 + 1) * SORT_ORDER_GAP;
            assert_eq!(col.sort_order, expected, "column {} must land at max + (i+1)*GAP", i);
            assert_eq!(col.name, "", "batch-created columns must start with empty name (ghost-commit default)");
            assert_eq!(col.col_type, "text", "batch-created columns must default to text type");
        }
    }

    #[test]
    fn test_create_sheet_columns_batch_mid_batch_failure_rolls_back_all() {
        // Matt's assertion 2: a mid-batch failure rolls back ALL N columns
        // (transaction atomicity). Simulated by dropping the sheet_columns
        // table's NOT NULL guarantee via a duplicate-id collision injected
        // partway through: pre-insert a column with an id we force the
        // batch to reuse is impractical (ids are fresh UUIDs each call), so
        // instead we corrupt the table mid-batch by making note_id NOT NULL
        // violation on row 2 impossible to hit safely. Simplest reliable
        // injection: drop the table after a controlled number of successful
        // inserts by wrapping in a savepoint-free connection is unavailable
        // here, so instead we exercise atomicity via a full-table-lock
        // conflict: run the batch, then verify a *closed*/dropped table mid-
        // way via a second connection is not applicable to an in-memory DB.
        //
        // Practical injection: make the table schema reject the 3rd insert
        // by adding a CHECK constraint that only the 3rd row (by a
        // deterministic property) can violate. We drop the `note_id`
        // column's usability for row 3 by deleting the parent note between
        // row 2 and row 3 is also not enforced (no FK). So we instead
        // directly verify atomicity by forcing `recompute_and_regen` to fail
        // after the loop's inserts: delete the note row so the final regen
        // step (which reads/writes `notes.body`) errors, then confirm ALL
        // inserted columns were rolled back despite the loop itself fully
        // succeeding.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let before_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sheet_columns WHERE note_id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();

        // Delete the parent note so recompute_and_regen's UPDATE notes SET
        // body=... affects 0 rows but more importantly so any note-lookup
        // inside regen fails — forcing a post-insert, pre-commit failure.
        conn.execute("DELETE FROM notes WHERE id = ?1", params![&note_id]).unwrap();

        let result = create_sheet_columns_batch_internal(&conn, &note_id, &blank_column_specs(3));
        assert!(result.is_err(), "batch must fail when recompute_and_regen can't find the note");

        let after_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sheet_columns WHERE note_id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();
        assert_eq!(after_count, before_count, "mid-batch failure must roll back ALL inserted columns, not just the failing one");
    }

    #[test]
    fn test_create_sheet_columns_batch_one_snapshot_for_whole_batch() {
        // Matt's snapshot discipline: ONE snapshot before the inserts, not
        // one per column.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let count_snapshots = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM note_versions WHERE note_id = ?1 AND source = 'sheet_snapshot'",
                params![&note_id],
                |r| r.get(0),
            )
            .unwrap()
        };

        let before = count_snapshots(&conn);
        create_sheet_columns_batch_internal(&conn, &note_id, &blank_column_specs(4)).expect("batch create should succeed");
        let after = count_snapshots(&conn);

        assert_eq!(after, before + 1, "a 4-column batch must capture exactly ONE snapshot, not four");
    }

    #[test]
    fn test_create_sheet_column_single_insert_unchanged_by_batch_addition() {
        // Matt's assertion 3: single-row endpoint create_sheet_column is
        // UNCHANGED — no regressions from adding the batch path.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let col = create_sheet_column_internal(&conn, &note_id, "Amount", "number", None, None, None)
            .expect("create_sheet_column should succeed");

        assert_eq!(col.name, "Amount");
        assert_eq!(col.col_type, "number");
        assert_eq!(col.sort_order, (DEFAULT_SHEET_COLUMNS as i64) * SORT_ORDER_GAP, "single-insert append math must be unchanged");

        let total: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert_eq!(total, DEFAULT_SHEET_COLUMNS as i64 + 1, "single insert must create exactly one column");
    }

    #[test]
    fn test_create_sheet_columns_batch_zero_count_is_noop() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let batch = create_sheet_columns_batch_internal(&conn, &note_id, &blank_column_specs(0)).expect("count=0 must not error");
        assert_eq!(batch.len(), 0, "count=0 must return an empty vec");

        let total: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert_eq!(total, DEFAULT_SHEET_COLUMNS as i64, "count=0 must not insert anything");
    }

    #[test]
    fn test_create_sheet_columns_batch_rejects_invalid_col_type_before_any_insert() {
        // #2970 (`add_sheet_columns`): validation runs BEFORE the
        // transaction opens — an invalid entry creates NOTHING, not a
        // partial batch up to the bad entry. Error names the column index
        // and the reason.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let before: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();

        let specs = vec![
            ColumnSpec { name: "Good".to_string(), col_type: "text".to_string(), options: None },
            ColumnSpec { name: "Bad".to_string(), col_type: "not-a-type".to_string(), options: None },
        ];
        let result = create_sheet_columns_batch_internal(&conn, &note_id, &specs);
        let err = result.expect_err("invalid col_type must reject the whole batch");
        assert!(err.contains('1'), "error must name the invalid entry's index (1): {}", err);
        assert!(err.contains("not-a-type"), "error must name the reason: {}", err);

        let after: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert_eq!(after, before, "a validation failure must create ZERO columns, not the valid ones before the bad entry");
    }

    // ── Row CRUD ─────────────────────────────────────────────────────────

    #[test]
    fn test_create_sheet_row_appends_with_gap_sort_order() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        assert_eq!(row1.sort_order, 0);
        assert_eq!(row2.sort_order, SORT_ORDER_GAP);
    }

    #[test]
    fn test_create_sheet_row_rejects_mismatched_reference_and_position() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let result = create_sheet_row_internal(&conn, &note_id, Some(&row.id), None);
        assert!(result.is_err(), "reference_id without position must be rejected");

        let result2 = create_sheet_row_internal(&conn, &note_id, None, Some(InsertPosition::Before));
        assert!(result2.is_err(), "position without reference_id must be rejected");
    }

    #[test]
    fn test_insert_row_above_midpoint() {
        // Two rows at sort_order 0 and 1000; insert before the second —
        // midpoint(0, 1000) = 500.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        assert_eq!(row1.sort_order, 0);
        assert_eq!(row2.sort_order, SORT_ORDER_GAP);

        let new_row = create_sheet_row_internal(&conn, &note_id, Some(&row2.id), Some(InsertPosition::Before))
            .expect("positional insert should succeed");

        assert_eq!(new_row.sort_order, 500, "midpoint of 0 and 1000 must be 500");
    }

    #[test]
    fn test_insert_row_above_first_row() {
        // A single row at sort_order 1000 (simulating a sheet whose first
        // row isn't at 0) — insert before it: midpoint(0, 1000) = 500.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let only_row_id = seed_row_at(&conn, &note_id, 1000);

        let new_row = create_sheet_row_internal(&conn, &note_id, Some(&only_row_id), Some(InsertPosition::Before))
            .expect("positional insert should succeed");
        assert_eq!(new_row.sort_order, 500, "midpoint of 0 and 1000 must be 500");

        // Separately: a row at sort_order 0 (the true first-ever row) —
        // inserting before it must hit the renumber path since
        // midpoint(0, 0) collides with the reference itself.
        let note_id_2 = make_sheet(&conn, "proj-1");
        let zero_row_id = seed_row_at(&conn, &note_id_2, 0);

        let new_row_2 = create_sheet_row_internal(&conn, &note_id_2, Some(&zero_row_id), Some(InsertPosition::Before))
            .expect("positional insert against a sort_order=0 reference must renumber and succeed");
        assert_ne!(new_row_2.sort_order, 0, "post-renumber midpoint must not collide with the reference");

        let renumbered_zero_row_sort_order: i64 = conn
            .query_row("SELECT sort_order FROM sheet_rows WHERE id = ?1", params![&zero_row_id], |r| r.get(0))
            .unwrap();
        assert_ne!(new_row_2.sort_order, renumbered_zero_row_sort_order, "new row must not collide with the renumbered reference either");
    }

    #[test]
    fn test_insert_row_triggers_renumber_when_gap_exhausted() {
        // Two adjacent rows at sort_order 0 and 1 (no integer slot between
        // them) — insert before the second must renumber ALL rows in the
        // sheet to fresh i*1000 gaps first, then compute the midpoint
        // against the post-renumber neighbours.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let first_id = seed_row_at(&conn, &note_id, 0);
        let second_id = seed_row_at(&conn, &note_id, 1);

        let new_row = create_sheet_row_internal(&conn, &note_id, Some(&second_id), Some(InsertPosition::Before))
            .expect("gap-exhausted positional insert must renumber and succeed");

        // Post-renumber (1-indexed — see renumber_sheet_rows doc comment):
        // First -> 1000, Second -> 2000, new row's midpoint against those
        // is (1000 + 2000) / 2 = 1500.
        let first_sort_order: i64 = conn
            .query_row("SELECT sort_order FROM sheet_rows WHERE id = ?1", params![&first_id], |r| r.get(0))
            .unwrap();
        let second_sort_order: i64 = conn
            .query_row("SELECT sort_order FROM sheet_rows WHERE id = ?1", params![&second_id], |r| r.get(0))
            .unwrap();
        assert_eq!(first_sort_order, SORT_ORDER_GAP, "renumber must resequence the first row to fresh gap-1000");
        assert_eq!(second_sort_order, 2 * SORT_ORDER_GAP, "renumber must resequence the second row to fresh gap-2000");
        assert_eq!(new_row.sort_order, 1500, "new row's sort_order must be the post-renumber midpoint");
    }

    #[test]
    fn test_insert_row_rejects_reference_from_different_note() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_a = make_sheet(&conn, "proj-1");
        let note_b = make_sheet(&conn, "proj-1");
        let row_from_b = create_sheet_row_internal(&conn, &note_b, None, None).unwrap();

        let result = create_sheet_row_internal(&conn, &note_a, Some(&row_from_b.id), Some(InsertPosition::Before));
        assert!(result.is_err(), "reference_id from a different note must be rejected");
    }

    #[test]
    fn test_insert_row_positional_insert_recomputes_and_regens_body() {
        // Confirms the positional-insert path still runs through the same
        // recompute_and_regen chokepoint as every other structural
        // mutation — a row-with-data inserted above another row-with-data
        // must appear in notes.body at the new position.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row1.id, &col.id, Some("bottom"), None).unwrap();

        let new_row = create_sheet_row_internal(&conn, &note_id, Some(&row1.id), Some(InsertPosition::Before)).unwrap();
        update_sheet_cell_internal(&conn, &new_row.id, &col.id, Some("top"), None).unwrap();

        let body: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        let top_pos = body.find("top").expect("body must contain the new row's value");
        let bottom_pos = body.find("bottom").expect("body must contain the existing row's value");
        assert!(top_pos < bottom_pos, "the positionally-inserted row must render BEFORE the reference row in body");
    }

    #[test]
    fn test_update_sheet_row_height_only_writes_correctly() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        assert_eq!(row.height, 0, "row is seeded with height 0");

        let updated = update_sheet_row_internal(&conn, &row.id, Some(80))
            .expect("update_sheet_row_internal should succeed");

        assert_eq!(updated.height, 80);
        assert_eq!(updated.id, row.id, "other fields untouched: id");
        assert_eq!(updated.note_id, row.note_id, "other fields untouched: note_id");
        assert_eq!(updated.sort_order, row.sort_order, "other fields untouched: sort_order");
        assert_ne!(updated.updated_at, row.created_at, "updated_at must be bumped");

        let stored_height: i64 = conn
            .query_row("SELECT height FROM sheet_rows WHERE id = ?1", params![&row.id], |r| r.get(0))
            .unwrap();
        assert_eq!(stored_height, 80, "height must be persisted in the DB");
    }

    #[test]
    fn test_update_sheet_row_partial_update_preserves_height_when_none() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_row_internal(&conn, &row.id, Some(50)).unwrap();

        let updated = update_sheet_row_internal(&conn, &row.id, None)
            .expect("update with height=None should succeed");

        assert_eq!(updated.height, 50, "height=None must preserve the current stored height");
    }

    #[test]
    fn test_update_sheet_row_structural_snapshot_bypasses_throttle() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let count_snapshots = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM note_versions WHERE note_id = ?1 AND source = 'sheet_snapshot'",
                params![&note_id],
                |r| r.get(0),
            )
            .unwrap()
        };

        let before = count_snapshots(&conn);
        // Two height updates back-to-back, within any throttle window —
        // both must capture since row-height updates are a structural
        // mutation (bypass_throttle = true), mirroring update_sheet_column.
        update_sheet_row_internal(&conn, &row.id, Some(60)).unwrap();
        update_sheet_row_internal(&conn, &row.id, Some(70)).unwrap();
        let after = count_snapshots(&conn);

        assert_eq!(after, before + 2, "two row-height updates within the throttle window must BOTH capture (structural bypass)");
    }

    #[test]
    fn test_update_sheet_row_height_only_does_not_touch_notes_body() {
        // CRITICAL: confirms the deliberate `recompute_and_regen` skip in
        // `update_sheet_row_internal` is correct — a height-only row update
        // must NOT regenerate `notes.body` (unlike update_sheet_column,
        // which always regens even for width-only changes).
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("hello"), None).unwrap();

        let body_before: String = conn
            .query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();

        update_sheet_row_internal(&conn, &row.id, Some(90)).expect("update should succeed");

        let body_after: String = conn
            .query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();

        assert_eq!(body_after, body_before, "notes.body must be UNCHANGED by a height-only row update (confirms recompute_and_regen skip)");
    }

    #[test]
    fn test_delete_sheet_row_cascades_cells_and_preserves_others() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row_a = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row_b = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row_a.id, &col.id, Some("row-a-val"), None).unwrap();
        update_sheet_cell_internal(&conn, &row_b.id, &col.id, Some("row-b-val"), None).unwrap();

        delete_sheet_row_internal(&conn, &row_a.id).expect("delete should succeed");

        let row_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_rows WHERE id = ?1", params![&row_a.id], |r| r.get(0)).unwrap();
        assert_eq!(row_count, 0);

        let cell_a_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells WHERE row_id = ?1", params![&row_a.id], |r| r.get(0)).unwrap();
        assert_eq!(cell_a_count, 0, "cells for deleted row must be cascaded");

        let cell_b: String = conn.query_row(
            "SELECT value FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![&row_b.id, &col.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(cell_b, "row-b-val", "other row's cell must survive");
    }

    #[test]
    fn test_reorder_sheet_rows_rewrites_sort_order() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row3 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let new_order = vec![row3.id.clone(), row1.id.clone(), row2.id.clone()];
        reorder_sheet_rows_internal(&conn, &note_id, &new_order).expect("reorder should succeed");

        let mut stmt = conn.prepare("SELECT id FROM sheet_rows WHERE note_id = ?1 ORDER BY sort_order ASC").unwrap();
        let after: Vec<String> = stmt.query_map(params![&note_id], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(after, new_order);
    }

    // ── Row batch create (item 23 / #2874) ─────────────────────────────

    /// #2970 — `count` blank `RowSpec`s, mirroring `create_sheet_rows`'s
    /// ghost-row-commit default shape (no values, append at end).
    fn blank_row_specs(count: usize) -> Vec<RowSpec> {
        (0..count).map(|_| RowSpec { values: None }).collect()
    }

    #[test]
    fn test_create_sheet_rows_batch_lands_at_max_plus_i_times_gap() {
        // Matt's assertion 1: N rows land at max_sort_order + (i+1)*GAP.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        // Fresh sheet has zero rows — MAX(sort_order) coalesces to -1000.
        let max_before: i64 = -1000;

        let (batch, written_cells) = create_sheet_rows_batch_internal(&conn, &note_id, &blank_row_specs(5), false)
            .expect("batch create should succeed");

        assert_eq!(batch.len(), 5, "must create exactly N rows");
        assert_eq!(written_cells, 0, "a values-less batch must report zero written cells");
        for (i, row) in batch.iter().enumerate() {
            let expected = max_before + (i as i64 + 1) * SORT_ORDER_GAP;
            assert_eq!(row.sort_order, expected, "row {} must land at max + (i+1)*GAP", i);
            assert_eq!(row.height, 0, "batch-created rows must default to height 0 (sparse row default)");
        }

        // Also verify against a non-empty starting point (append after
        // existing rows), since ghost-row commits always happen past
        // real rows.
        let existing = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let max_after_existing = existing.sort_order;
        let (batch2, _) = create_sheet_rows_batch_internal(&conn, &note_id, &blank_row_specs(3), false)
            .expect("second batch create should succeed");
        for (i, row) in batch2.iter().enumerate() {
            let expected = max_after_existing + (i as i64 + 1) * SORT_ORDER_GAP;
            assert_eq!(row.sort_order, expected, "batch-2 row {} must land at (existing) max + (i+1)*GAP", i);
        }
    }

    #[test]
    fn test_create_sheet_rows_batch_mid_batch_failure_rolls_back_all() {
        // Matt's assertion 2, extended for #2970's value-fill phase: a
        // failure on row 2's VALUE (not the structural insert) must roll
        // back ALL N rows AND any cells already written for earlier rows
        // in the same call — verified via a direct DB check (per #2970
        // scope §9: "not merely that an error returned"), not just the
        // `Result::is_err()` shape. Injection: row 1 writes a real value
        // into an existing column; row 2 references a nonexistent
        // column_id, which `upsert_sheet_cell_no_recompute`'s own column
        // lookup rejects.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();

        let before_row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sheet_rows WHERE note_id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();
        let before_cell_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells", [], |r| r.get(0)).unwrap();

        let mut row1_values = HashMap::new();
        row1_values.insert(col.id.clone(), "good-value".to_string());
        let mut row2_values = HashMap::new();
        row2_values.insert("nonexistent-column-id".to_string(), "bad-value".to_string());
        let specs = vec![
            RowSpec { values: Some(row1_values) },
            RowSpec { values: Some(row2_values) },
        ];

        let result = create_sheet_rows_batch_internal(&conn, &note_id, &specs, false);
        let err = result.expect_err("a bad value mid-fill must fail the whole batch");
        assert!(err.contains("row 1"), "error must name the failing row's index: {}", err);
        assert!(err.contains("nonexistent-column-id"), "error must name the failing column: {}", err);

        let after_row_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sheet_rows WHERE note_id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();
        let after_cell_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells", [], |r| r.get(0)).unwrap();
        assert_eq!(after_row_count, before_row_count, "mid-batch failure must roll back ALL inserted rows, including row 0");
        assert_eq!(after_cell_count, before_cell_count, "mid-batch failure must roll back row 0's already-written cell too — no partial value-fill survives");
    }

    #[test]
    fn test_create_sheet_rows_batch_one_snapshot_for_whole_batch() {
        // Matt's snapshot discipline: ONE snapshot before the inserts, not
        // one per row.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let count_snapshots = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM note_versions WHERE note_id = ?1 AND source = 'sheet_snapshot'",
                params![&note_id],
                |r| r.get(0),
            )
            .unwrap()
        };

        let before = count_snapshots(&conn);
        create_sheet_rows_batch_internal(&conn, &note_id, &blank_row_specs(6), false).expect("batch create should succeed");
        let after = count_snapshots(&conn);

        assert_eq!(after, before + 1, "a 6-row batch must capture exactly ONE snapshot, not six");
    }

    #[test]
    fn test_create_sheet_row_single_insert_unchanged_by_batch_addition() {
        // Matt's assertion 3: single-row endpoint create_sheet_row is
        // UNCHANGED — no regressions from adding the batch path.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        assert_eq!(row1.sort_order, 0, "single-insert append math must be unchanged");
        assert_eq!(row2.sort_order, SORT_ORDER_GAP, "single-insert append math must be unchanged");

        let total: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_rows WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert_eq!(total, 2, "single inserts must each create exactly one row");
    }

    #[test]
    fn test_create_sheet_rows_batch_zero_count_is_noop() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let (batch, written_cells) = create_sheet_rows_batch_internal(&conn, &note_id, &blank_row_specs(0), false).expect("count=0 must not error");
        assert_eq!(batch.len(), 0, "count=0 must return an empty vec");
        assert_eq!(written_cells, 0);

        let total: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_rows WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert_eq!(total, 0, "count=0 must not insert anything");
    }

    #[test]
    fn test_create_sheet_rows_batch_intermediate_rows_are_empty() {
        // Intermediate-rows-are-empty invariant: every row except the last
        // one in a batch must be a sparse (cell-less) row — the ghost-
        // commit's typed value only ever targets the LAST created row's
        // id, and the batch-insert function itself never writes any cell
        // values, so all N rows must have zero associated sheet_cells rows.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let (batch, _) = create_sheet_rows_batch_internal(&conn, &note_id, &blank_row_specs(4), false).expect("batch create should succeed");

        for row in &batch {
            let cell_count: i64 = conn
                .query_row("SELECT COUNT(*) FROM sheet_cells WHERE row_id = ?1", params![&row.id], |r| r.get(0))
                .unwrap();
            assert_eq!(cell_count, 0, "batch-created rows must have no cells until the caller writes the typed value into the last one");
        }
    }

    #[test]
    fn test_create_sheet_rows_batch_with_values_writes_cells_and_counts_written() {
        // #2970 happy path: N rows, each with values, land correctly and
        // `written_cells` reflects the total cell count across the batch.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col_a = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let col_b = create_sheet_column_internal(&conn, &note_id, "B", "text", None, None, None).unwrap();

        let mut row0 = HashMap::new();
        row0.insert(col_a.id.clone(), "r0a".to_string());
        row0.insert(col_b.id.clone(), "r0b".to_string());
        let mut row1 = HashMap::new();
        row1.insert(col_a.id.clone(), "r1a".to_string());
        let specs = vec![RowSpec { values: Some(row0) }, RowSpec { values: Some(row1) }];

        let (rows, written_cells) =
            create_sheet_rows_batch_internal(&conn, &note_id, &specs, false).expect("batch should succeed");
        assert_eq!(rows.len(), 2);
        assert_eq!(written_cells, 3, "3 total cell values across both rows");

        let stored_r0a: String = conn.query_row(
            "SELECT value FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![&rows[0].id, &col_a.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(stored_r0a, "r0a");
        let r1b_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![&rows[1].id, &col_b.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(r1b_count, 0, "row 1 never specified column B — no cell should exist for it");
    }

    #[test]
    fn test_create_sheet_rows_batch_recompute_gated_on_any_value_written() {
        // #2970, ai-integration.md §4.1 `add_sheet_rows`: "One
        // recalculation... run at the end, only if any value was
        // written." Uses the production call counter item 51 added to
        // `formula/recompute.rs` (`RECOMPUTE_CALL_COUNT`) — the same
        // counter `update_sheet_cells_style_batch_internal`'s own tests
        // use — to prove the call COUNT, not just that the batch succeeded.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();

        // Case 1: a values-less batch (today's ghost-row-commit shape)
        // must skip recompute entirely — BEFORE #2970: unconditional,
        // always 1. AFTER (#2970): gated, 0 for a values-less batch.
        recompute::reset_recompute_call_count();
        create_sheet_rows_batch_internal(&conn, &note_id, &blank_row_specs(10), false)
            .expect("values-less batch should succeed");
        assert_eq!(
            recompute::get_recompute_call_count(), 0,
            "a values-less 10-row batch must trigger ZERO recomputes (before #2970 this was 1; after, gated to 0)"
        );

        // Case 2: a batch where EVERY row writes a value must trigger
        // EXACTLY ONE recompute, regardless of row/cell count — not one
        // per row, not one per cell.
        recompute::reset_recompute_call_count();
        let specs: Vec<RowSpec> = (0..10)
            .map(|i| {
                let mut values = HashMap::new();
                values.insert(col.id.clone(), format!("v{}", i));
                RowSpec { values: Some(values) }
            })
            .collect();
        create_sheet_rows_batch_internal(&conn, &note_id, &specs, false).expect("values batch should succeed");
        assert_eq!(
            recompute::get_recompute_call_count(), 1,
            "a 10-row batch where every row writes a value must trigger EXACTLY ONE recompute, not ten"
        );
    }

    #[test]
    fn test_create_sheet_rows_batch_at_start_lands_contiguous_block_before_existing() {
        // #2970 `position: 'start'`: the WHOLE batch lands as one
        // contiguous block below all pre-existing rows, in the same
        // relative order the batch was given — not interleaved.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let existing1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let existing2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let (new_rows, _) = create_sheet_rows_batch_internal(&conn, &note_id, &blank_row_specs(3), true)
            .expect("at_start batch should succeed");

        let mut stmt = conn.prepare("SELECT id FROM sheet_rows WHERE note_id = ?1 ORDER BY sort_order ASC").unwrap();
        let ordered_ids: Vec<String> = stmt.query_map(params![&note_id], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();

        let expected: Vec<String> = new_rows.iter().map(|r| r.id.clone())
            .chain([existing1.id.clone(), existing2.id.clone()])
            .collect();
        assert_eq!(ordered_ids, expected, "the new batch must land as one contiguous block, in order, before the pre-existing rows");
    }

    // ── Cell upsert/delete ───────────────────────────────────────────────

    #[test]
    fn test_update_sheet_cell_creates_via_upsert() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let cell = update_sheet_cell_internal(&conn, &row.id, &col.id, Some("hello"), None)
            .expect("cell create should succeed");
        assert_eq!(cell.raw_value, "hello");
        assert_eq!(cell.note_id, note_id);

        let stored: String = conn.query_row(
            "SELECT value FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![&row.id, &col.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(stored, "hello");
    }

    #[test]
    fn test_update_sheet_cell_updates_existing_value() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("first"), None).unwrap();
        let updated = update_sheet_cell_internal(&conn, &row.id, &col.id, Some("second"), None).unwrap();
        assert_eq!(updated.raw_value, "second");

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1, "upsert must not create a duplicate row for the same composite PK");
    }

    /// #2339 PR 14 (ReferenceMapper, Matt ruling 2026-09-05 #8): an
    /// A1-style formula is no longer stored verbatim — it's translated
    /// to canonical `{column_id:row_id}` stable-ID form at the write
    /// chokepoint BEFORE storage, so a later row/column reorder can't
    /// silently repoint what the formula means. `cargo run`-visible raw
    /// SQL confirms the STORED bytes are ID-form. As of #2874 item 47,
    /// the RETURNED `SheetCell.raw_value` is translated back to current
    /// A1 display form (same as a `get_sheet_data` read would show) —
    /// previously it returned the untranslated ID form verbatim; this
    /// test now asserts the display-form round-trip instead.
    #[test]
    fn test_update_sheet_cell_translates_a1_formula_to_stable_id_form_on_write() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let rows: Vec<SheetRow> = (0..5).map(|_| create_sheet_row_internal(&conn, &note_id, None, None).unwrap()).collect();

        let cell = update_sheet_cell_internal(&conn, &rows[0].id, &col.id, Some("=SUM(A1:A5)"), None).unwrap();
        // Item 37 (#2874): ALL ID-form ranges — including plain column
        // ranges — now serialize as two complete `{col_id:row_id}` cell
        // refs joined by a colon, not the legacy `{col:row1:row2}` shape,
        // in STORAGE. The RETURNED raw_value (item 47) is the A1 form.
        let expected_stored = format!("=SUM({{{}:{}}}:{{{}:{}}})", col.id, rows[0].id, col.id, rows[4].id);
        assert_eq!(cell.raw_value, "=SUM(A1:A5)", "the returned SheetCell's raw_value must be A1 display form, same as a normal read, got: {}", cell.raw_value);

        let stored: String = conn
            .query_row(
                "SELECT value FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![&rows[0].id, &col.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, expected_stored, "the DB row itself must hold translated stable-ID form, not the raw A1 text the user typed");
    }

    /// #2339 PR 14 (ReferenceMapper): an A1-form formula already using
    /// canonical `{column_id:row_id}` ID syntax must pass through
    /// unchanged in STORAGE (idempotent, no double-translation on write)
    /// — this is the shape an AI tool's `set_sheet_cells` call may
    /// already produce. As of #2874 item 47, the RETURNED `SheetCell`
    /// still translates it to A1 display form for the caller (same as a
    /// read) — storage idempotency and display translation are two
    /// separate, non-conflicting guarantees.
    #[test]
    fn test_update_sheet_cell_id_form_formula_passes_through_unchanged() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let source_col = create_sheet_column_internal(&conn, &note_id, "Source", "number", None, None, None).unwrap();
        let total_col = create_sheet_column_internal(&conn, &note_id, "Total", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let formula = format!("={{{}:{}}}*2", source_col.id, row.id);
        let cell = update_sheet_cell_internal(&conn, &row.id, &total_col.id, Some(&formula), None).unwrap();
        // `make_sheet` seeds 5 default columns first, so `source_col` (the
        // 6th column created) sits at position F, not A — this is a
        // display-form ROUND TRIP check, not a claim about which letter.
        assert_eq!(cell.raw_value, "=F1*2", "the returned SheetCell's raw_value must be A1 display form (source_col is the 6th column, position F; row is row 1), got: {}", cell.raw_value);

        let stored: String = conn
            .query_row(
                "SELECT value FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![&row.id, &total_col.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, formula, "already-ID-form formula text must be STORED verbatim (no double-translation on write)");
    }

    /// #2339 PR 14, Matt ruling 2026-09-05 #3: an A1 reference pointing
    /// past the sheet's current bounds must be REJECTED at write time
    /// with a clear inline message, same UX surface as a parse
    /// rejection — nothing partial is ever stored.
    #[test]
    fn test_update_sheet_cell_rejects_out_of_bounds_a1_reference() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        // Only 1 row exists (A1) — A5 is out of bounds.
        let result = update_sheet_cell_internal(&conn, &row.id, &col.id, Some("=A5"), None);
        assert!(result.is_err(), "an out-of-bounds A1 reference must be rejected, not stored");
        let err = result.unwrap_err();
        assert!(err.contains("out of bounds"), "error must clearly describe the out-of-bounds condition, got: {}", err);

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![&row.id, &col.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "a rejected out-of-bounds formula must not be stored at all");
    }

    // ── §33.5.3 parse-on-write validation (#2339 PR 7, Deliverable 4) ──────

    #[test]
    fn test_update_sheet_cell_accepts_valid_formula() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let result = update_sheet_cell_internal(&conn, &row.id, &col.id, Some("=1+1"), None);
        assert!(result.is_ok(), "a syntactically valid local formula edit must be accepted");
    }

    #[test]
    fn test_update_sheet_cell_rejects_invalid_formula_and_does_not_store_it() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let result = update_sheet_cell_internal(&conn, &row.id, &col.id, Some("=1+"), None);
        assert!(result.is_err(), "a local formula edit with a parse error must be rejected inline");

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![&row.id, &col.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0, "the unparseable formula must NOT be stored — the write is rejected before any DB write");
    }

    #[test]
    fn test_update_sheet_cell_rejects_deeply_nested_formula_without_crashing() {
        // Exercises the parser recursion-depth guard through the actual
        // write path, not just parser.rs's unit tests — a pathological
        // formula pasted into a cell must be rejected gracefully, never
        // crash the write path.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let pathological = format!("={}{}{}", "(".repeat(200), "1", ")".repeat(200));
        let result = update_sheet_cell_internal(&conn, &row.id, &col.id, Some(&pathological), None);
        assert!(result.is_err(), "deeply nested formula must be rejected, not crash");
    }

    #[test]
    fn test_update_sheet_cell_style_only_edit_skips_formula_validation() {
        // value: None (style-only edit) must never invoke formula
        // validation — there's no `value` to validate.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("plain text"), None).unwrap();

        let result = update_sheet_cell_internal(&conn, &row.id, &col.id, None, Some(r#"{"bg":"red"}"#));
        assert!(result.is_ok(), "style-only edit must succeed without attempting formula validation");
    }

    #[test]
    fn test_update_sheet_cell_empty_value_and_style_deletes_sparse_row() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("something"), None).unwrap();
        // Clear both value and style — must delete the row (sparse rule).
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some(""), Some("{}")).unwrap();

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![&row.id, &col.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0, "clearing value+style must delete the sparse row, not upsert empty");
    }

    #[test]
    fn test_delete_sheet_cell_removes_row() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("val"), None).unwrap();

        delete_sheet_cell_internal(&conn, &row.id, &col.id).expect("delete should succeed");

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            params![&row.id, &col.id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(count, 0);
    }

    // ── get_sheet_data ───────────────────────────────────────────────────

    #[test]
    fn test_get_sheet_data_returns_complete_ordered_structure() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let extra_col = create_sheet_column_internal(&conn, &note_id, "Extra", "text", None, None, None).unwrap();
        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row1.id, &extra_col.id, Some("cell-val"), None).unwrap();

        let data = get_sheet_data_internal(&conn, &note_id).expect("get_sheet_data should succeed");

        assert_eq!(data.columns.len(), DEFAULT_SHEET_COLUMNS + 1);
        assert_eq!(data.rows.len(), 2);
        assert_eq!(data.cells.len(), 1);
        // Ordered: last column appended (Extra) must sort last.
        assert_eq!(data.columns.last().unwrap().id, extra_col.id);
        assert_eq!(data.rows[0].id, row1.id);
        assert_eq!(data.rows[1].id, row2.id);
    }

    /// §33.5.4 trigger 3 regression: a freshly LOADED sheet must return
    /// computed formula values with NO PRIOR MUTATION OR SYNC on that
    /// sheet — `get_sheet_data` itself is the trigger. Cells are inserted
    /// via raw SQL (bypassing `update_sheet_cell_internal`, which would
    /// itself run `recompute_and_regen` as mutation trigger 1 and
    /// confound the test) so the ONLY recompute that can possibly have
    /// populated `computed_value` is the one `get_sheet_data_internal`
    /// runs on this call. Matt ruling 2026-09-03.
    ///
    /// #2339 PR 14 (ReferenceMapper read path, §33.5.2): `raw_value` is
    /// no longer the untouched stored string — `get_sheet_data_internal`
    /// now re-renders stable-ID references as their CURRENT A1 position
    /// for display, so `{col_a:row}*2` (2 columns, 1 row: A1) reads back
    /// as `=A1*2`.
    #[test]
    fn test_get_sheet_data_recomputes_formulas_on_load_with_no_prior_mutation() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        // Delete the seeded default columns so col_a/col_b land at
        // sort_order positions 0/1 — required for the A1 display
        // translation below (A=position 0, B=position 1) to line up.
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col_a = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let col_b = create_sheet_column_internal(&conn, &note_id, "B", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        // Raw SQL inserts — deliberately NOT `update_sheet_cell_internal`,
        // so no recompute/regen has ever run against this sheet's cells
        // before the `get_sheet_data_internal` call below.
        let formula = format!("={{{}:{}}}*2", col_a.id, row.id);
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id, value, style_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, '{}', datetime('now'))",
            params![&row.id, &col_a.id, &note_id, "5"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id, value, style_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, '{}', datetime('now'))",
            params![&row.id, &col_b.id, &note_id, &formula],
        )
        .unwrap();

        let data = get_sheet_data_internal(&conn, &note_id).expect("get_sheet_data should succeed");

        let b1 = data
            .cells
            .iter()
            .find(|c| c.column_id == col_b.id && c.row_id == row.id)
            .expect("B1 cell must be present in the returned grid");
        assert_eq!(b1.raw_value, "=A1*2", "raw_value must be re-rendered as its current A1 display form (ReferenceMapper read path), not the raw stored ID-form string");
        assert_eq!(
            b1.computed_value.as_deref(),
            Some("10"),
            "B1's computed value must be 10 (A1*2 = 5*2), not the raw formula string, with no prior mutation or sync on this sheet"
        );

        let a1 = data
            .cells
            .iter()
            .find(|c| c.column_id == col_a.id && c.row_id == row.id)
            .expect("A1 cell must be present in the returned grid");
        assert_eq!(a1.raw_value, "5");
        assert_eq!(a1.computed_value, None, "a non-formula cell must not get a computed_value");
    }

    // ── Cascade: purge_note ─────────────────────────────────────────────

    #[test]
    fn test_cascade_delete_sheet_tables_for_note_removes_all_side_table_rows() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("val"), None).unwrap();

        cascade_delete_sheet_tables_for_note(&conn, &note_id).expect("cascade should succeed");

        let col_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        let row_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_rows WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        let cell_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert_eq!(col_count, 0);
        assert_eq!(row_count, 0);
        assert_eq!(cell_count, 0);
    }

    // ── Body regeneration ────────────────────────────────────────────────

    #[test]
    fn test_body_regen_reflects_cell_value_change() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "Name", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("Widget"), None).unwrap();

        let body: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert!(body.contains("Widget"), "body must reflect the new cell value, got: {}", body);
    }

    #[test]
    fn test_body_regen_skipped_on_pure_style_change() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "Name", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("Widget"), None).unwrap();

        let body_before: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        let updated_at_before: String = conn.query_row("SELECT updated_at FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();

        // Pure style change — value unchanged (None means keep the current 'Widget').
        std::thread::sleep(std::time::Duration::from_millis(5));
        update_sheet_cell_internal(&conn, &row.id, &col.id, None, Some(r#"{"bg":"blue"}"#)).unwrap();

        let body_after: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        let updated_at_after: String = conn.query_row("SELECT updated_at FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();

        assert_eq!(body_before, body_after, "pure style change must not alter body content");
        assert_eq!(updated_at_before, updated_at_after, "pure style change must not touch notes.updated_at (no regen write)");
    }

    #[test]
    fn test_body_regen_empty_row_produces_no_body_line() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "Name", "text", None, None, None).unwrap();
        let row_with_data = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let _empty_row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row_with_data.id, &col.id, Some("Filled"), None).unwrap();

        let body: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        // title + header + exactly one data line (the empty row contributes none).
        let line_count = body.lines().count();
        assert_eq!(line_count, 3, "empty row must not add a body line, got body: {:?}", body);
    }

    /// #2339 PR 7 closes the PR 5a interim gap: a formula cell's body
    /// line must show its COMPUTED value, not the raw formula string.
    /// Uses ID-form reference syntax (`{column_id:row_id}`) since that's
    /// the form that resolves against real `sheet_columns`/`sheet_rows`
    /// UUIDs — see `formula/recompute.rs`'s module-level scope note on
    /// A1-vs-ID reference forms.
    #[test]
    fn test_body_regen_formula_cell_shows_computed_value_not_raw_string() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let source_col = create_sheet_column_internal(&conn, &note_id, "Source", "number", None, None, None).unwrap();
        let total_col = create_sheet_column_internal(&conn, &note_id, "Total", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row.id, &source_col.id, Some("4"), None).unwrap();
        let formula = format!("={{{}:{}}}*2", source_col.id, row.id);
        update_sheet_cell_internal(&conn, &row.id, &total_col.id, Some(&formula), None).unwrap();

        let body: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert!(body.contains("8"), "formula cell must render its computed value (8), got body: {}", body);
        assert!(!body.contains(&formula), "formula cell must NOT render the raw formula string once computed, got body: {}", body);
    }

    /// #2874 item 25 regression: `update_sheet_cell_internal`'s OWN
    /// returned `SheetCell` (not a follow-up `get_sheet_data` read) must
    /// carry the freshly computed value for the formula cell that was
    /// JUST committed — before this fix, the mutation return hardcoded
    /// `computed_value: None`, so the frontend's optimistic local patch
    /// (`applyLocalCellPatch`) briefly rendered the raw `=...` formula
    /// text instead of the computed result.
    #[test]
    fn test_update_sheet_cell_returns_computed_value_for_formula_write() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let source_col = create_sheet_column_internal(&conn, &note_id, "Source", "number", None, None, None).unwrap();
        let total_col = create_sheet_column_internal(&conn, &note_id, "Total", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row.id, &source_col.id, Some("4"), None).unwrap();
        let formula = format!("={{{}:{}}}*2", source_col.id, row.id);
        let returned = update_sheet_cell_internal(&conn, &row.id, &total_col.id, Some(&formula), None)
            .expect("formula write should succeed");

        assert_eq!(
            returned.computed_value.as_deref(),
            Some("8"),
            "the SheetCell returned directly from the formula write must carry its computed value (8), got: {:?}",
            returned.computed_value
        );
    }

    /// #2874 item 47 (Matt ruling 2026-09-24, supersedes item 27's
    /// original scoped-down PR amendment): a style-only write
    /// (`value: None`, `value_changed == false`) now runs a full
    /// recompute just like any other write, so the returned `SheetCell`
    /// carries the cell's CURRENT computed value — never `None` for a
    /// healthy formula cell, and never dropped to `None` for an error
    /// cell either. This applies uniformly to a healthy formula cell AND
    /// one currently resolving to an error (`#REF!` etc.) — the backend
    /// recomputes and reports the true live value for both, exactly as a
    /// normal `get_sheet_data` read would. (Renamed from
    /// `test_update_sheet_cell_style_only_write_returns_none_computed_value`,
    /// which asserted the now-reversed old contract.)
    #[test]
    fn test_update_sheet_cell_style_only_write_returns_current_computed_value() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let source_col = create_sheet_column_internal(&conn, &note_id, "Source", "number", None, None, None).unwrap();
        let total_col = create_sheet_column_internal(&conn, &note_id, "Total", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        // A healthy formula cell first — style-only write must return the
        // LIVE computed value (8), not None and not the raw formula text.
        update_sheet_cell_internal(&conn, &row.id, &source_col.id, Some("4"), None).unwrap();
        let formula = format!("={{{}:{}}}*2", source_col.id, row.id);
        update_sheet_cell_internal(&conn, &row.id, &total_col.id, Some(&formula), None).unwrap();

        let styled = update_sheet_cell_internal(&conn, &row.id, &total_col.id, None, Some(r#"{"bg":"blue"}"#))
            .expect("style-only write should succeed");
        assert_eq!(
            styled.computed_value.as_deref(),
            Some("8"),
            "style-only write must return the cell's current computed value (8), same as a normal read, got: {:?}",
            styled.computed_value
        );
        assert_eq!(styled.style_json, r#"{"bg":"blue"}"#);
        // The style-only write must ALSO never touch notes.body (Matt
        // ruling 2026-09-03, unaffected by item 47's recompute change).
        let body: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        let body_before_style_edit = body.clone();
        let _ = update_sheet_cell_internal(&conn, &row.id, &total_col.id, None, Some(r#"{"bg":"green"}"#))
            .expect("second style-only write should succeed");
        let body_after: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert_eq!(body_after, body_before_style_edit, "a style-only write must never touch notes.body");

        // Same contract on an ERROR cell: reference a column that
        // doesn't exist -> #REF!. Style-only write on THIS cell must
        // return the live #REF! string, not None — a style edit must
        // never make a visible error silently vanish.
        let err_row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let bad_formula = "={ZZZZ-nonexistent-col:1}".to_string();
        update_sheet_cell_internal(&conn, &err_row.id, &total_col.id, Some(&bad_formula), None).unwrap();

        let restyled = update_sheet_cell_internal(&conn, &err_row.id, &total_col.id, None, Some(r#"{"bg":"red"}"#))
            .expect("style-only write on an error cell should succeed");
        assert_eq!(
            restyled.computed_value.as_deref(),
            Some("#REF!"),
            "style-only write on an error formula cell must return the live #REF! error string, not None, got: {:?}",
            restyled.computed_value
        );
    }

    /// #2874 item 47: a style-only write's returned `raw_value` for a
    /// formula cell must be in A1 DISPLAY form, never the internal
    /// `{column_id:row_id}` storage form — the bug this item fixes was a
    /// client-side comparison between this (previously untranslated) raw
    /// form and a cached A1 form always failing for any formula cell.
    #[test]
    fn test_update_sheet_cell_style_only_write_returns_a1_form_raw_value() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let source_col = create_sheet_column_internal(&conn, &note_id, "Source", "number", None, None, None).unwrap();
        let total_col = create_sheet_column_internal(&conn, &note_id, "Total", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row.id, &source_col.id, Some("4"), None).unwrap();
        // Write the formula using its A1 form (mirrors a real human typing
        // "=A1*2" into the formula bar) — `update_sheet_cell_internal`
        // translates A1 -> ID form for storage on the write path.
        update_sheet_cell_internal(&conn, &row.id, &total_col.id, Some("=A1*2"), None).unwrap();

        let styled = update_sheet_cell_internal(&conn, &row.id, &total_col.id, None, Some(r#"{"bg":"blue"}"#))
            .expect("style-only write should succeed");
        assert_eq!(
            styled.raw_value, "=A1*2",
            "a style-only write's raw_value must be translated back to A1 display form, not the internal {{column_id:row_id}} storage form, got: {}",
            styled.raw_value
        );
        assert!(
            !styled.raw_value.contains(&total_col.id) && !styled.raw_value.contains(&source_col.id),
            "a style-only write's raw_value must never contain an internal column/row ID, got: {}",
            styled.raw_value
        );
    }

    /// #2874 item 47: a style-only write on a sheet PAST
    /// `FORMULA_CELL_SOFT_LIMIT` must degrade the same way a read does —
    /// `computed_value: None` (guard-rail skip), never an error and never
    /// a panic. Confirms the new unconditional recompute call doesn't
    /// introduce a new failure mode at the guard-rail boundary.
    #[test]
    fn test_update_sheet_cell_style_only_write_past_formula_soft_limit_degrades_like_a_read() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let source_col = create_sheet_column_internal(&conn, &note_id, "Source", "number", None, None, None).unwrap();
        let total_col = create_sheet_column_internal(&conn, &note_id, "Total", "number", None, None, None).unwrap();

        // Seed more formula cells than the soft limit so recompute_sheet
        // degrades (skips) for this note, matching the read-path guard
        // rail's own degradation contract.
        let mut last_row = None;
        for i in 0..(recompute::FORMULA_CELL_SOFT_LIMIT + 1) {
            let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
            update_sheet_cell_internal(&conn, &row.id, &source_col.id, Some(&i.to_string()), None).unwrap();
            let formula = format!("={{{}:{}}}*2", source_col.id, row.id);
            update_sheet_cell_internal(&conn, &row.id, &total_col.id, Some(&formula), None).unwrap();
            last_row = Some(row);
        }
        let last_row = last_row.unwrap();

        let styled = update_sheet_cell_internal(&conn, &last_row.id, &total_col.id, None, Some(r#"{"bg":"blue"}"#))
            .expect("style-only write past the guard rail must still succeed, not error");
        assert_eq!(
            styled.computed_value, None,
            "a style-only write past FORMULA_CELL_SOFT_LIMIT must degrade to computed_value: None (guard-rail skip), same as a read, got: {:?}",
            styled.computed_value
        );
    }

    /// #2339 PR 14 (ReferenceMapper) supersedes the old pre-PR-14 gap:
    /// an A1-style formula against a REAL grid now resolves correctly
    /// (translated to stable-ID form at write time), not to an empty
    /// range. This replaces the prior
    /// `test_body_regen_a1_style_formula_against_uuid_columns_computes_as_empty_range`
    /// test, which documented the interim gap this PR closes.
    #[test]
    fn test_body_regen_a1_style_formula_against_real_grid_resolves_via_reference_mapper() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let source_col = create_sheet_column_internal(&conn, &note_id, "Source", "number", None, None, None).unwrap();
        let total_col = create_sheet_column_internal(&conn, &note_id, "Total", "number", None, None, None).unwrap();
        let rows: Vec<SheetRow> = (0..5).map(|_| create_sheet_row_internal(&conn, &note_id, None, None).unwrap()).collect();
        for (i, r) in rows.iter().enumerate() {
            update_sheet_cell_internal(&conn, &r.id, &source_col.id, Some(&(i + 1).to_string()), None).unwrap();
        }

        // A SUM formula in the SECOND column referencing the first
        // column's whole range by A1 letter/number — this now resolves
        // against the real grid via the ReferenceMapper instead of
        // computing as an empty range.
        update_sheet_cell_internal(&conn, &rows[0].id, &total_col.id, Some("=SUM(A1:A5)"), None).unwrap();

        let body: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert!(!body.contains("=SUM(A1:A5)"), "raw formula string must not leak into body once recompute runs, got: {}", body);
        assert!(
            body.contains("15"),
            "1+2+3+4+5 = 15: an in-bounds, resolvable A1-style SUM must resolve against the real grid via the ReferenceMapper, not silently compute as empty-range 0 (the pre-PR-14 gap), got body: {}",
            body
        );
    }

    // ── FTS ──────────────────────────────────────────────────────────────
    // NOTE: FTS trigger wiring is verified in the `fts_tests` module below
    // (needs the real notes_fts virtual table + triggers, not this
    // module's minimal test schema).

    // ── Carried Test Item 1 (#2339 PR 5c, from the PR 5a audit) ─────────
    // value-only update preserves style_json

    #[test]
    fn test_update_sheet_cell_value_only_preserves_style_json() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        // Set a value AND a distinctive style.
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("val"), Some(r#"{"bg":"red"}"#)).unwrap();

        // Value-only update: style_json param is None.
        let updated = update_sheet_cell_internal(&conn, &row.id, &col.id, Some("new_val"), None)
            .expect("value-only update should succeed");
        assert_eq!(updated.raw_value, "new_val");
        assert_eq!(
            updated.style_json, r#"{"bg":"red"}"#,
            "a value-only update (style_json: None) must preserve the existing style_json, not reset it"
        );

        let stored_style: String = conn
            .query_row(
                "SELECT style_json FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![&row.id, &col.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored_style, r#"{"bg":"red"}"#, "stored style_json must survive a value-only edit");
    }

    // ── CSV Export (#2339 PR 5c, §33.12) ────────────────────────────────

    #[test]
    fn test_export_basic_csv_output() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        // make_sheet seeds DEFAULT_SHEET_COLUMNS text columns — build a
        // fresh, minimal 2-col grid on a NEW sheet-less connection instead
        // isn't practical here, so we construct via create_sheet_column
        // directly and ignore the seeded default columns by only reading
        // back a controlled subset via get_sheet_data — instead, simplest:
        // delete the seeded defaults so the grid is exactly 2 columns.
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col_a = create_sheet_column_internal(&conn, &note_id, "Name", "text", None, None, None).unwrap();
        let col_b = create_sheet_column_internal(&conn, &note_id, "Age", "number", None, None, None).unwrap();
        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row1.id, &col_a.id, Some("Alice"), None).unwrap();
        update_sheet_cell_internal(&conn, &row1.id, &col_b.id, Some("30"), None).unwrap();
        update_sheet_cell_internal(&conn, &row2.id, &col_a.id, Some("Bob"), None).unwrap();
        update_sheet_cell_internal(&conn, &row2.id, &col_b.id, Some("25"), None).unwrap();

        let export = export_sheet_csv_internal(&conn, &note_id).expect("export should succeed");
        let expected = "Name,Age\r\nAlice,30\r\nBob,25";
        assert_eq!(export.csv_content, expected, "got: {:?}", export.csv_content);
    }

    #[test]
    fn test_export_quotes_fields_with_commas() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Name", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("Smith, John"), None).unwrap();

        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        assert!(
            export.csv_content.contains("\"Smith, John\""),
            "a field containing a comma must be wrapped in quotes, got: {:?}",
            export.csv_content
        );
    }

    #[test]
    fn test_export_quotes_and_escapes_embedded_quotes() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Quote", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some(r#"She said "hi""#), None).unwrap();

        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        assert!(
            export.csv_content.contains(r#""She said ""hi"""#),
            "embedded double-quotes must be escaped by doubling and the whole field quoted, got: {:?}",
            export.csv_content
        );
    }

    #[test]
    fn test_export_quotes_fields_with_newlines() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Notes", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("line one\nline two"), None).unwrap();

        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        assert!(
            export.csv_content.contains("\"line one\nline two\""),
            "a field containing a newline must be quoted, got: {:?}",
            export.csv_content
        );
    }

    #[test]
    fn test_export_error_cell_exports_error_string() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Result", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("=5/0"), None).unwrap();

        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        assert!(
            export.csv_content.contains("#DIV/0!"),
            "a formula cell computing #DIV/0! must export the literal error string, not the raw formula, got: {:?}",
            export.csv_content
        );
        assert!(
            !export.csv_content.contains("=5/0"),
            "raw formula string must not leak into the export, got: {:?}",
            export.csv_content
        );
    }

    #[test]
    fn test_export_checkbox_true_false() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Done", "checkbox", None, None, None).unwrap();
        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row1.id, &col.id, Some("1"), None).unwrap();
        update_sheet_cell_internal(&conn, &row2.id, &col.id, Some("0"), None).unwrap();

        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        let expected = "Done\r\nTRUE\r\nFALSE";
        assert_eq!(export.csv_content, expected, "got: {:?}", export.csv_content);
    }

    #[test]
    fn test_export_currency_no_symbol() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Price", "currency", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("19.99"), None).unwrap();

        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        let expected = "Price\r\n19.99";
        assert_eq!(export.csv_content, expected, "currency must export the bare decimal, no $/code, got: {:?}", export.csv_content);
    }

    #[test]
    fn test_export_date_iso_format() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Due", "date", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("2026-09-04"), None).unwrap();

        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        let expected = "Due\r\n2026-09-04";
        assert_eq!(export.csv_content, expected, "ISO 8601 date must pass through verbatim, got: {:?}", export.csv_content);
    }

    #[test]
    fn test_export_empty_cell_is_empty_string() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col_a = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let _col_b = create_sheet_column_internal(&conn, &note_id, "B", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        // Only col_a gets a value — col_b's coordinate is sparse (no
        // sheet_cells row at all for this row/column pair, but the
        // column itself must still appear as a header/empty field).
        update_sheet_cell_internal(&conn, &row.id, &col_a.id, Some("filled"), None).unwrap();

        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        let expected = "A,B\r\nfilled,";
        assert_eq!(export.csv_content, expected, "a sparse coordinate must export as an empty field, got: {:?}", export.csv_content);
    }

    #[test]
    fn test_export_multi_row_multi_column() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col_name = create_sheet_column_internal(&conn, &note_id, "Item", "text", None, None, None).unwrap();
        let col_qty = create_sheet_column_internal(&conn, &note_id, "Qty", "number", None, None, None).unwrap();
        let col_done = create_sheet_column_internal(&conn, &note_id, "Done", "checkbox", None, None, None).unwrap();
        let rows: Vec<SheetRow> = (0..3).map(|_| create_sheet_row_internal(&conn, &note_id, None, None).unwrap()).collect();

        update_sheet_cell_internal(&conn, &rows[0].id, &col_name.id, Some("Widget"), None).unwrap();
        update_sheet_cell_internal(&conn, &rows[0].id, &col_qty.id, Some("3"), None).unwrap();
        update_sheet_cell_internal(&conn, &rows[0].id, &col_done.id, Some("1"), None).unwrap();

        update_sheet_cell_internal(&conn, &rows[1].id, &col_name.id, Some("Gadget"), None).unwrap();
        update_sheet_cell_internal(&conn, &rows[1].id, &col_qty.id, Some("7"), None).unwrap();
        update_sheet_cell_internal(&conn, &rows[1].id, &col_done.id, Some("0"), None).unwrap();

        update_sheet_cell_internal(&conn, &rows[2].id, &col_name.id, Some("Gizmo"), None).unwrap();
        // rows[2] Qty and Done left sparse.

        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        let expected = "Item,Qty,Done\r\nWidget,3,TRUE\r\nGadget,7,FALSE\r\nGizmo,,FALSE";
        assert_eq!(export.csv_content, expected, "got: {:?}", export.csv_content);
    }

    #[test]
    fn test_export_filename_sanitizes_title() {
        assert_eq!(sanitize_filename("My/Sheet:Name?"), "My_Sheet_Name_", "hostile chars must be replaced with underscores");
        assert_eq!(sanitize_filename("Plain Title"), "Plain Title", "a plain title must pass through unchanged");
        assert_eq!(sanitize_filename("..."), "sheet", "a title that trims to nothing (dots-only) must fall back to 'sheet'");
        assert_eq!(sanitize_filename("   "), "sheet", "a whitespace-only title must fall back to 'sheet'");
        assert_eq!(sanitize_filename("  spaced.  "), "spaced", "leading/trailing whitespace and dots must be trimmed");

        // End-to-end via export_sheet_csv_internal.
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let req = crate::notes::CreateSheetNoteRequest {
            project_id: "proj-1".to_string(),
            folder_id: None,
            title: "My/Sheet:Name?".to_string(),
            tags: None,
            status: Some("cabinet".to_string()),
            priority: None,
            due_date: None,
        };
        let note_id = create_sheet_internal(&conn, req, None).unwrap().note_meta.id;
        let export = export_sheet_csv_internal(&conn, &note_id).unwrap();
        assert_eq!(export.filename, "My_Sheet_Name_.csv");
    }

    #[test]
    fn test_export_reads_via_get_sheet_data_no_side_effects() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("val"), None).unwrap();

        let updated_at_before: String = conn
            .query_row("SELECT updated_at FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();
        let body_before: String = conn
            .query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));
        let _export = export_sheet_csv_internal(&conn, &note_id).expect("export should succeed");

        let updated_at_after: String = conn
            .query_row("SELECT updated_at FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();
        let body_after: String = conn
            .query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();

        assert_eq!(updated_at_before, updated_at_after, "export must trigger zero writes to notes.updated_at");
        assert_eq!(body_before, body_after, "export must trigger zero writes to notes.body");

        let cell_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells WHERE note_id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert_eq!(cell_count, 1, "export must not mutate sheet_cells");
    }

    // ── get_sheet_card_summary(_internal) / get_sheet_card_summaries (#2339 PR 15a) ──

    #[test]
    fn test_card_summary_counts_columns_rows_and_formulas() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        // make_sheet seeds DEFAULT_SHEET_COLUMNS (5) text columns; replace
        // with a known set so column-type assertions are exact.
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let col_a = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let col_b = create_sheet_column_internal(&conn, &note_id, "B", "number", None, None, None).unwrap();
        let col_c = create_sheet_column_internal(&conn, &note_id, "C", "currency", None, None, None).unwrap();

        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row1.id, &col_a.id, Some("hello"), None).unwrap();
        update_sheet_cell_internal(&conn, &row1.id, &col_b.id, Some("3"), None).unwrap();
        update_sheet_cell_internal(&conn, &row2.id, &col_c.id, Some("=B1*2"), None).unwrap();

        let summary = get_sheet_card_summary_internal(&conn, &note_id).unwrap();
        assert_eq!(summary.note_id, note_id);
        assert_eq!(summary.col_count, 3);
        assert_eq!(summary.row_count, 2);
        assert_eq!(summary.formula_count, 1, "only the one '='-prefixed cell counts as a formula");
        assert_eq!(summary.col_types, vec!["text".to_string(), "number".to_string(), "currency".to_string()], "col_types must be ordered by sort_order, matching column display order");
    }

    #[test]
    fn test_card_summary_empty_sheet_returns_zero_rows_and_seeded_columns() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        // Untouched fresh sheet: DEFAULT_SHEET_COLUMNS seeded columns, zero rows.
        let note_id = make_sheet(&conn, "proj-1");

        let summary = get_sheet_card_summary_internal(&conn, &note_id).unwrap();
        assert_eq!(summary.col_count, DEFAULT_SHEET_COLUMNS);
        assert_eq!(summary.row_count, 0, "a brand-new sheet has no rows yet");
        assert_eq!(summary.formula_count, 0);
        assert_eq!(summary.col_types.len(), DEFAULT_SHEET_COLUMNS, "empty sheet must still report its seeded column types for the glyph header row");
    }

    #[test]
    fn test_card_summary_non_sheet_note_returns_error() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        conn.execute(
            "INSERT INTO notes (id, title, note_type) VALUES ('plain-note-1', 'A Note', NULL)",
            [],
        )
        .unwrap();

        let result = get_sheet_card_summary_internal(&conn, "plain-note-1");
        assert!(result.is_err(), "a non-sheet note must not produce a card summary");
    }

    #[test]
    fn test_card_summary_unknown_note_id_returns_error() {
        let conn = create_test_db();
        let result = get_sheet_card_summary_internal(&conn, "does-not-exist");
        assert!(result.is_err(), "an unknown note id must error, not panic or return a default summary");
    }

    #[test]
    fn test_card_summary_is_read_only_no_side_effects() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        let updated_at_before: String = conn
            .query_row("SELECT updated_at FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();
        let body_before: String = conn
            .query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(5));
        let _summary = get_sheet_card_summary_internal(&conn, &note_id).expect("summary should succeed");

        let updated_at_after: String = conn
            .query_row("SELECT updated_at FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();
        let body_after: String = conn
            .query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0))
            .unwrap();

        assert_eq!(updated_at_before, updated_at_after, "card summary must trigger zero writes to notes.updated_at");
        assert_eq!(body_before, body_after, "card summary must trigger zero writes to notes.body");
    }

    #[test]
    fn test_card_summaries_batches_multiple_sheets_and_skips_non_sheets() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let sheet_a = make_sheet(&conn, "proj-1");
        let sheet_b = make_sheet(&conn, "proj-1");
        conn.execute(
            "INSERT INTO notes (id, title, note_type) VALUES ('plain-note-2', 'A Note', NULL)",
            [],
        )
        .unwrap();

        let row = create_sheet_row_internal(&conn, &sheet_b, None, None).unwrap();
        let col = conn
            .query_row("SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1", params![&sheet_b], |r| r.get::<_, String>(0))
            .unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col, Some("x"), None).unwrap();

        // Mixed list: two real sheets, one non-sheet note, one unknown id —
        // batched call must return only the two sheet summaries, in whatever
        // order they resolve, silently dropping the rest rather than erroring.
        let ids = vec![sheet_a.clone(), "plain-note-2".to_string(), sheet_b.clone(), "unknown-id".to_string()];
        let summaries = get_sheet_card_summaries_internal_for_test(&conn, &ids);

        assert_eq!(summaries.len(), 2, "non-sheet and unknown ids must be silently omitted, not errored");
        let ids_returned: Vec<&str> = summaries.iter().map(|s| s.note_id.as_str()).collect();
        assert!(ids_returned.contains(&sheet_a.as_str()));
        assert!(ids_returned.contains(&sheet_b.as_str()));

        let summary_b = summaries.iter().find(|s| s.note_id == sheet_b).unwrap();
        assert_eq!(summary_b.row_count, 1);
    }

    /// Test-only helper mirroring `get_sheet_card_summaries`'s filter-map
    /// body without the `State<DbState>` Tauri plumbing, so the batching
    /// behavior is testable against the in-memory `Connection` directly.
    fn get_sheet_card_summaries_internal_for_test(conn: &Connection, note_ids: &[String]) -> Vec<SheetCardSummary> {
        note_ids.iter().filter_map(|id| get_sheet_card_summary_internal(conn, id).ok()).collect()
    }

    // ── #2874 item 51: batch style write — one transaction, one recompute ──

    /// Matt's whole-point assertion: a many-cell style batch recomputes
    /// ONCE, not once per cell. Before this item, `applyStyleToSelection`
    /// drove N separate `update_sheet_cell` calls, each running its own
    /// full-sheet `recompute_sheet` — an N-cell style apply recomputed N
    /// times. This is the direct backend proof the new batch path does
    /// not reintroduce that cost: it must recompute EXACTLY once
    /// regardless of how many cells are in the batch.
    #[test]
    fn test_update_sheet_cells_style_batch_recomputes_exactly_once_for_a_many_cell_batch() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = conn
            .query_row(
                "SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1",
                params![&note_id],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        let rows: Vec<SheetRow> = (0..12).map(|_| create_sheet_row_internal(&conn, &note_id, None, None).unwrap()).collect();

        let entries: Vec<(String, String, String)> = rows
            .iter()
            .map(|r| (r.id.clone(), col.clone(), r##"{"bg":"#232327"}"##.to_string()))
            .collect();

        recompute::reset_recompute_call_count();
        let result = update_sheet_cells_style_batch_internal(&conn, &note_id, &entries)
            .expect("12-cell style batch should succeed");

        assert_eq!(result.len(), 12, "must return all 12 styled cells");
        assert_eq!(
            recompute::get_recompute_call_count(),
            1,
            "a 12-cell style batch must recompute EXACTLY ONCE, not once per cell (BEFORE this item: 12 recomputes, one per `update_sheet_cell` call; AFTER: 1)"
        );
    }

    /// A single-cell `update_sheet_cell_internal` call, for contrast with
    /// the batch test above — proves the "before" shape this item fixes:
    /// N single-cell writes recompute N times (unchanged, non-batch
    /// behavior; `applyStyleToSelection`'s OLD per-cell loop is what this
    /// drove before item 51).
    #[test]
    fn test_update_sheet_cell_single_cell_writes_recompute_once_per_call_unbatched() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = conn
            .query_row(
                "SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1",
                params![&note_id],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        let rows: Vec<SheetRow> = (0..3).map(|_| create_sheet_row_internal(&conn, &note_id, None, None).unwrap()).collect();

        recompute::reset_recompute_call_count();
        for r in &rows {
            update_sheet_cell_internal(&conn, &r.id, &col, None, Some(r##"{"bg":"#232327"}"##)).unwrap();
        }

        assert_eq!(
            recompute::get_recompute_call_count(),
            3,
            "3 separate update_sheet_cell_internal calls must still recompute 3 times — the OLD per-cell-loop cost this item's batch path replaces, documented here for before/after contrast"
        );
    }

    /// Matt's ruling: "a batch style write in one transaction ... mid-
    /// batch failure rolls back everything." Entry 2 of 3 (a nonexistent
    /// `row_id`) fails `upsert_sheet_cell_no_recompute`'s row lookup —
    /// entry 1, which would have succeeded in isolation, must NOT be left
    /// written once the whole call fails.
    #[test]
    fn test_update_sheet_cells_style_batch_mid_batch_failure_rolls_back_all() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = conn
            .query_row(
                "SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1",
                params![&note_id],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row3 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let entries: Vec<(String, String, String)> = vec![
            (row1.id.clone(), col.clone(), r##"{"bg":"#232327"}"##.to_string()),
            ("nonexistent-row-id".to_string(), col.clone(), r##"{"bg":"#232327"}"##.to_string()),
            (row3.id.clone(), col.clone(), r##"{"bg":"#232327"}"##.to_string()),
        ];

        let result = update_sheet_cells_style_batch_internal(&conn, &note_id, &entries);
        assert!(result.is_err(), "a batch containing a nonexistent row_id must fail");

        let row1_style: Option<String> = conn
            .query_row(
                "SELECT style_json FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![&row1.id, &col],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(
            row1_style, None,
            "entry 1 (which would have succeeded alone) must NOT be written — mid-batch failure rolls back the ENTIRE batch, not just the failing entry"
        );

        let row3_style: Option<String> = conn
            .query_row(
                "SELECT style_json FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![&row3.id, &col],
                |r| r.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(row3_style, None, "entry 3 (after the failing entry) must also not be written");
    }

    /// Matt's snapshot discipline, mirrored from
    /// `test_create_sheet_columns_batch_one_snapshot_for_whole_batch`: ONE
    /// snapshot for the whole batch, not one per cell.
    #[test]
    fn test_update_sheet_cells_style_batch_one_snapshot_for_whole_batch() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = conn
            .query_row(
                "SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1",
                params![&note_id],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        // Raw INSERT, not `create_sheet_row_internal` — that helper forces
        // its OWN snapshot per call (`bypass_throttle: true`, matching
        // ghost-row-commit's deliberate-structural-create posture), which
        // would pollute this test's "before" count with 5 unrelated
        // snapshots before the style batch even runs. A raw row insert is
        // fixture setup only, with no snapshot side effect, isolating the
        // count to the batch call under test.
        let rows: Vec<String> = (0..5)
            .map(|i| {
                let id = Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO sheet_rows (id, note_id, sort_order) VALUES (?1, ?2, ?3)",
                    params![id, note_id, (i as i64) * 1000],
                )
                .unwrap();
                id
            })
            .collect();
        let entries: Vec<(String, String, String)> = rows
            .iter()
            .map(|row_id| (row_id.clone(), col.clone(), r##"{"bg":"#232327"}"##.to_string()))
            .collect();

        let count_snapshots = |conn: &Connection| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM note_versions WHERE note_id = ?1 AND source = 'sheet_snapshot'",
                params![&note_id],
                |r| r.get(0),
            )
            .unwrap()
        };

        let before = count_snapshots(&conn);
        update_sheet_cells_style_batch_internal(&conn, &note_id, &entries).expect("batch style write should succeed");
        let after = count_snapshots(&conn);

        assert_eq!(after, before + 1, "a 5-cell style batch must capture exactly ONE snapshot, not five");
    }

    /// Matt's ruling: "no body regeneration (style-only, as today)".
    /// `notes.body`/`notes.updated_at` must be completely untouched by a
    /// style-only batch, even though the batch DOES run a recompute
    /// (item 47's guarantee is about the in-memory computed_value reply,
    /// never about persisting anything to `notes.body`).
    #[test]
    fn test_update_sheet_cells_style_batch_does_not_regenerate_body() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = conn
            .query_row(
                "SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1",
                params![&note_id],
                |r| r.get::<_, String>(0),
            )
            .unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        // Give the row a real value first so the body isn't trivially empty.
        update_sheet_cell_internal(&conn, &row.id, &col, Some("hello"), None).unwrap();

        let (body_before, updated_at_before): (String, String) = conn
            .query_row("SELECT body, updated_at FROM notes WHERE id = ?1", params![&note_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();

        let entries = vec![(row.id.clone(), col.clone(), r##"{"bg":"#232327"}"##.to_string())];
        update_sheet_cells_style_batch_internal(&conn, &note_id, &entries).expect("style batch should succeed");

        let (body_after, updated_at_after): (String, String) = conn
            .query_row("SELECT body, updated_at FROM notes WHERE id = ?1", params![&note_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();

        assert_eq!(body_before, body_after, "a style-only batch must never touch notes.body");
        assert_eq!(updated_at_before, updated_at_after, "a style-only batch must never touch notes.updated_at");
    }

    /// Item 47 is non-negotiable per this item's brief: every cell the
    /// batch returns carries its readable (A1-translated) formula and its
    /// live computed value — exactly like a single-cell style write
    /// already does, and exactly like a normal read.
    #[test]
    fn test_update_sheet_cells_style_batch_returns_a1_formula_and_computed_value_for_every_cell() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        conn.execute("DELETE FROM sheet_columns WHERE note_id = ?1", params![&note_id]).unwrap();
        let source_col = create_sheet_column_internal(&conn, &note_id, "Source", "number", None, None, None).unwrap();
        let total_col = create_sheet_column_internal(&conn, &note_id, "Total", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row.id, &source_col.id, Some("4"), None).unwrap();
        // Written in A1 form, as a human/AI caller would type it — proves
        // the batch path translates A1-to-stable-ID on write (matching
        // the single-cell chokepoint) and A1-FROM-stable-ID on the way
        // back out (item 47's `translate_cells_for_display` reuse).
        update_sheet_cell_internal(&conn, &row.id, &total_col.id, Some("=A1*2"), None).unwrap();

        let entries = vec![
            (row.id.clone(), source_col.id.clone(), r##"{"bg":"#111111"}"##.to_string()),
            (row.id.clone(), total_col.id.clone(), r##"{"bg":"#222222"}"##.to_string()),
        ];
        let result = update_sheet_cells_style_batch_internal(&conn, &note_id, &entries).expect("batch style write should succeed");

        let total_cell = result.iter().find(|c| c.column_id == total_col.id).expect("total cell must be in the reply");
        assert_eq!(
            total_cell.raw_value, "=A1*2",
            "the formula cell's raw_value must come back in current A1 display form, not the internal {{column_id:row_id}} storage form"
        );
        assert_eq!(
            total_cell.computed_value.as_deref(),
            Some("8"),
            "the formula cell's computed_value must be live and correct (4*2=8), not None/stale"
        );
        assert_eq!(total_cell.style_json, r##"{"bg":"#222222"}"##, "the style write itself must still land");

        let source_cell = result.iter().find(|c| c.column_id == source_col.id).unwrap();
        assert_eq!(source_cell.computed_value, None, "a non-formula cell's computed_value must stay None");
    }

    /// Zero-entry batch is a no-op, mirroring
    /// `test_create_sheet_columns_batch_zero_count_is_noop`.
    #[test]
    fn test_update_sheet_cells_style_batch_zero_entries_is_noop() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");

        recompute::reset_recompute_call_count();
        let result = update_sheet_cells_style_batch_internal(&conn, &note_id, &[]).expect("empty batch should succeed as a no-op");
        assert_eq!(result.len(), 0);
        assert_eq!(recompute::get_recompute_call_count(), 0, "an empty batch must not recompute at all");
    }

    /// A cross-sheet entry (row/column that don't actually belong to
    /// `note_id`) is rejected, and rejects the WHOLE batch — defense in
    /// depth alongside the per-entry row/column note_id cross-check
    /// `upsert_sheet_cell_no_recompute` already does.
    #[test]
    fn test_update_sheet_cells_style_batch_rejects_entry_from_a_different_sheet() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_a = make_sheet(&conn, "proj-1");
        let note_b = make_sheet(&conn, "proj-1");
        let row_a = create_sheet_row_internal(&conn, &note_a, None, None).unwrap();
        let col_b = conn
            .query_row("SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1", params![&note_b], |r| r.get::<_, String>(0))
            .unwrap();

        // row_a belongs to note_a, but col_b belongs to note_b — this
        // entry is internally cross-sheet before even reaching the
        // batch's own note_id check.
        let entries = vec![(row_a.id.clone(), col_b.clone(), r##"{"bg":"#232327"}"##.to_string())];
        let result = update_sheet_cells_style_batch_internal(&conn, &note_a, &entries);
        assert!(result.is_err(), "an entry whose row/column belong to different sheets must be rejected");
    }

    /// Manual wall-clock comparison (#2874 item 51) — not part of the
    /// required CI gate (`#[ignore]`d: timing is not a correctness
    /// assertion and would be flaky in CI), but documents the
    /// user-visible win this item claims. Run with:
    /// `cargo test test_style_batch_wall_clock_comparison_50_cells -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn test_style_batch_wall_clock_comparison_50_cells() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = conn
            .query_row("SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1", params![&note_id], |r| r.get::<_, String>(0))
            .unwrap();
        let rows: Vec<SheetRow> = (0..50).map(|_| create_sheet_row_internal(&conn, &note_id, None, None).unwrap()).collect();

        let before_start = std::time::Instant::now();
        for r in &rows {
            update_sheet_cell_internal(&conn, &r.id, &col, None, Some(r##"{"bg":"#232327"}"##)).unwrap();
        }
        let before_elapsed = before_start.elapsed();

        // Reset styling so the batch pass below has the same starting
        // condition (sparse-free-but-unstyled would delete the row's cell
        // entirely — use a different, still-non-default color instead of
        // clearing, so both passes do a real upsert, not a delete).
        for r in &rows {
            update_sheet_cell_internal(&conn, &r.id, &col, None, Some(r##"{"bg":"#111111"}"##)).unwrap();
        }

        let entries: Vec<(String, String, String)> = rows
            .iter()
            .map(|r| (r.id.clone(), col.clone(), r##"{"bg":"#232327"}"##.to_string()))
            .collect();
        let after_start = std::time::Instant::now();
        update_sheet_cells_style_batch_internal(&conn, &note_id, &entries).expect("batch should succeed");
        let after_elapsed = after_start.elapsed();

        eprintln!(
            "[item 51 bench] 50-cell style apply — BEFORE (50 x update_sheet_cell_internal): {:?} | AFTER (1 x update_sheet_cells_style_batch_internal): {:?}",
            before_elapsed, after_elapsed
        );
        assert!(after_elapsed < before_elapsed, "batch path must be faster than the 50-call loop it replaces");
    }

    /// Single-cell `update_sheet_cell_internal` behavior must be
    /// byte-identical after the item 51 refactor extracted its raw-write
    /// half into `upsert_sheet_cell_no_recompute` — this is a direct
    /// regression check on the refactor itself, independent of the many
    /// pre-existing `test_update_sheet_cell_*` tests above (which already
    /// cover this implicitly by continuing to pass).
    #[test]
    fn test_update_sheet_cell_internal_unchanged_after_item_51_extraction() {
        let conn = create_test_db();
        insert_project(&conn, "proj-1");
        let note_id = make_sheet(&conn, "proj-1");
        let col = conn
            .query_row("SELECT id FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC LIMIT 1", params![&note_id], |r| r.get::<_, String>(0))
            .unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let result = update_sheet_cell_internal(&conn, &row.id, &col, Some("hello"), Some(r##"{"bg":"#232327"}"##))
            .expect("single-cell write should still succeed");
        assert_eq!(result.raw_value, "hello");
        assert_eq!(result.style_json, r##"{"bg":"#232327"}"##);
        assert_eq!(result.note_id, note_id);

        let body: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert!(body.contains("hello"), "value-changing single-cell write must still regenerate the body, got: {}", body);
    }
}

#[cfg(test)]
mod fts_tests {
    //! Deliverable 5: proves a sheet's regenerated body is searchable via
    //! the existing `search_notes` path — zero new FTS code needed, the
    //! existing `notes_fts` triggers fire automatically on any `body`
    //! UPDATE (migrations.rs's `create_fts_triggers`). This test wires up
    //! the REAL notes_fts virtual table + triggers (not a stub), matching
    //! `notes.rs`'s `search_notes_system_scope_tests::create_search_test_db`
    //! pattern, plus the real sheet tables, and drives a cell mutation
    //! through the real `update_sheet_cell_internal` -> body regen ->
    //! UPDATE notes SET body -> trigger -> notes_fts path end-to-end.
    use super::*;
    use rusqlite::Connection;

    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE projects (
                 id TEXT PRIMARY KEY,
                 name TEXT NOT NULL,
                 parent_id TEXT,
                 is_system INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE notes (
                 id TEXT PRIMARY KEY,
                 title TEXT NOT NULL DEFAULT '',
                 body TEXT NOT NULL DEFAULT '',
                 content_preview TEXT NOT NULL DEFAULT '',
                 file_path TEXT NOT NULL DEFAULT '',
                 project_id TEXT,
                 folder_id TEXT,
                 status TEXT NOT NULL DEFAULT 'cabinet',
                 priority TEXT,
                 due_date TEXT,
                 assignee TEXT,
                 completed_at TEXT,
                 sort_order INTEGER NOT NULL DEFAULT 0,
                 word_count INTEGER NOT NULL DEFAULT 0,
                 pinned INTEGER NOT NULL DEFAULT 0,
                 owner_id TEXT,
                 tags_json TEXT NOT NULL DEFAULT '[]',
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                 deleted_at TEXT,
                 parent_id TEXT,
                 note_type TEXT
             );
             CREATE TABLE tags (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT UNIQUE NOT NULL
             );
             CREATE TABLE note_tags (
                 note_id TEXT NOT NULL,
                 tag_id INTEGER NOT NULL
             );
             CREATE TABLE sheet_columns (
                 id           TEXT NOT NULL PRIMARY KEY,
                 note_id      TEXT NOT NULL DEFAULT '',
                 name         TEXT NOT NULL DEFAULT '',
                 col_type     TEXT NOT NULL DEFAULT 'text',
                 options_json TEXT NOT NULL DEFAULT '{}',
                 width        INTEGER NOT NULL DEFAULT 0,
                 sort_order   INTEGER NOT NULL DEFAULT 0,
                 created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE sheet_rows (
                 id           TEXT NOT NULL PRIMARY KEY,
                 note_id      TEXT NOT NULL DEFAULT '',
                 sort_order   INTEGER NOT NULL DEFAULT 0,
                 height       INTEGER NOT NULL DEFAULT 0,
                 created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                 updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE TABLE sheet_cells (
                 row_id     TEXT NOT NULL,
                 column_id  TEXT NOT NULL,
                 note_id    TEXT NOT NULL DEFAULT '',
                 value      TEXT NOT NULL DEFAULT '',
                 style_json TEXT NOT NULL DEFAULT '{}',
                 updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (row_id, column_id)
             );

             CREATE TABLE note_versions (
                 id         TEXT NOT NULL PRIMARY KEY,
                 note_id    TEXT NOT NULL DEFAULT '',
                 content    TEXT NOT NULL DEFAULT '',
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 source     TEXT NOT NULL DEFAULT 'manual'
             );
             CREATE INDEX IF NOT EXISTS idx_note_versions_note_id
                 ON note_versions(note_id);

             CREATE VIRTUAL TABLE notes_fts USING fts5(
                 title,
                 body,
                 content='notes',
                 content_rowid='rowid',
                 tokenize='porter unicode61'
             );

             CREATE TRIGGER notes_fts_insert AFTER INSERT ON notes BEGIN
                 INSERT INTO notes_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
             END;
             CREATE TRIGGER notes_fts_update AFTER UPDATE ON notes BEGIN
                 INSERT INTO notes_fts(notes_fts, rowid, title, body) VALUES('delete', old.rowid, old.title, old.body);
                 INSERT INTO notes_fts(rowid, title, body) VALUES (new.rowid, new.title, new.body);
             END;",
        )
        .unwrap();
        conn
    }

    #[test]
    fn test_sheet_body_regen_is_searchable_via_search_notes() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO projects (id, name, is_system) VALUES ('proj-1', 'Proj', 0)",
            [],
        )
        .unwrap();

        let req = crate::notes::CreateSheetNoteRequest {
            project_id: "proj-1".to_string(),
            folder_id: None,
            title: "Q3 Budget".to_string(),
            tags: None,
            status: Some("cabinet".to_string()),
            priority: None,
            due_date: None,
        };
        let data = create_sheet_internal(&conn, req, None).expect("create_sheet_internal should succeed");
        let note_id = data.note_meta.id.clone();
        let col_id = data.columns[0].id.clone();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        // Set a distinctive, searchable cell value — triggers body regen,
        // which UPDATEs notes.body, which fires the real notes_fts_update
        // trigger (no new FTS code — this is the whole point of Deliverable 5).
        update_sheet_cell_internal(&conn, &row.id, &col_id, Some("marmalade-sandwich-unique-token"), None)
            .expect("update_sheet_cell_internal should succeed");

        let request = crate::notes::SearchNotesRequest {
            query: "marmalade-sandwich-unique-token".to_string(),
            project_id: None,
            status: None,
            tags: None,
            has_due_date: None,
            limit: Some(20),
            title_prefix: None,
        };
        let results = crate::notes::search_notes_internal(&conn, &request)
            .expect("search_notes_internal should not error");

        assert_eq!(results.len(), 1, "sheet note with the searched cell value must appear in search results");
        assert_eq!(results[0].note_id, note_id);
    }
}
