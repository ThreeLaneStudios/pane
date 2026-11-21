// Pane — Sheets Snapshot Capture/Restore Backend (#2339 PR 8, §33.9)
//
// This module owns the version-history story for Sheets: pre-mutation
// snapshot capture into the existing `note_versions` table (source =
// "sheet_snapshot"), the versioned JSON grid envelope, and the
// ID-preserving diff-apply restore path.
//
// Design references: `docs/spec/sheets.md` §33.9, §33.9.1, §33.9.2,
// §33.9.3; the scope report `docs/audits/2026-09-03-sheets-pr8-pr5c-
// scope.md`; three rulings from Matt (2026-09-04):
//   1. `reorder_sheet_columns`/`reorder_sheet_rows` are STRUCTURAL —
//      snapshot-before + throttle bypass. Range membership (SUM/AVERAGE)
//      is computed by row order between endpoints, so a reorder can
//      silently change formula results — consequence-heavy enough to
//      warrant a guaranteed pre-capture, just like add/delete.
//   2. The pre-restore capture (§33.9.3 step 1) reuses the EXISTING
//      `pre_restore` source value — no sheet-specific `sheet_pre_restore`
//      variant. `source` is a diagnostic/filter tag; `note_id` already
//      tells a reader whether to parse `content` as sheet JSON or
//      markdown.
//   3. The manual-edit throttle constant is extracted once
//      (`notes::MANUAL_EDIT_THROTTLE_SECS`) and shared by both the
//      regular-note manual-edit gate and this module's grid-mutation
//      throttle. No broader refactor.
//
// `note_versions` requires zero migration — `source` is an unconstrained
// TEXT column (§1). `save_version`/`get_version` (`versions.rs`) are
// reused verbatim; `restore_version` is NOT touched or reused — it is
// markdown-specific (writes `notes.body` from plain text). This module's
// `restore_sheet_snapshot_internal` is the sheet-native replacement.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Snapshot source tag (§33.9.1) — a new `note_versions.source` value,
/// distinct from `manual_edit`/`ai_edit`/`pre_restore`/etc. No migration
/// needed (unconstrained TEXT column).
const SHEET_SNAPSHOT_SOURCE: &str = "sheet_snapshot";

/// Reused verbatim (Matt ruling 2026-09-04, item 2) — the pre-restore
/// capture inside `restore_sheet_snapshot_internal` uses this EXISTING
/// tag, not a sheet-specific variant.
const PRE_RESTORE_SOURCE: &str = "pre_restore";

/// Current (and only, for PR 8) envelope shape version. A restore that
/// encounters any other `v` value fails cleanly (`Err`) rather than
/// silently misparsing (§33.9.1).
const ENVELOPE_VERSION: u32 = 1;

// === Versioned JSON envelope (§33.9.1) ===

/// One `sheet_columns` row, as captured. IDs are stored verbatim — they
/// are load-bearing for the ID-preserving restore (§33.9.3). No
/// `created_at`/`updated_at` — those are DB-lifecycle metadata, not
/// restorable grid state.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvelopeColumn {
    id: String,
    name: String,
    col_type: String,
    options_json: String,
    width: i64,
    sort_order: i64,
}

/// One `sheet_rows` row, as captured.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvelopeRow {
    id: String,
    sort_order: i64,
    height: i64,
}

/// One `sheet_cells` row, as captured. `value` is `sheet_cells.value`
/// verbatim — a raw value OR an `=`-prefixed formula STRING, never a
/// computed value (ADR-0066 D4, restated for snapshots at §33.9.1).
/// Sparse: only cells that exist as real `sheet_cells` rows at capture
/// time are included — never a dense row×column matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EnvelopeCell {
    row_id: String,
    column_id: String,
    value: String,
    style_json: String,
}

/// The full versioned grid envelope stored as `note_versions.content`
/// for `source = "sheet_snapshot"` and `source = "pre_restore"` rows on a
/// sheet note. `note_id` is deliberately NOT included — it's redundant
/// with `note_versions.note_id` and would be a footgun if a snapshot were
/// ever copied cross-note.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SheetSnapshotEnvelope {
    v: u32,
    columns: Vec<EnvelopeColumn>,
    rows: Vec<EnvelopeRow>,
    cells: Vec<EnvelopeCell>,
}

/// Serializes the CURRENT grid state (columns/rows/cells for `note_id`)
/// into the `v: 1` JSON envelope. Called both by the pre-mutation capture
/// path (`maybe_capture_sheet_snapshot`) and by the pre-restore capture
/// step inside `restore_sheet_snapshot_internal` — same serialization,
/// same shape, regardless of which `source` tag the caller uses.
fn build_envelope_json(conn: &Connection, note_id: &str) -> Result<String, String> {
    let mut col_stmt = conn
        .prepare(
            "SELECT id, name, col_type, options_json, width, sort_order
             FROM sheet_columns WHERE note_id = ?1 ORDER BY sort_order ASC",
        )
        .map_err(|e| format!("build_envelope_json: column prepare failed: {}", e))?;
    let columns: Vec<EnvelopeColumn> = col_stmt
        .query_map(params![note_id], |r| {
            Ok(EnvelopeColumn {
                id: r.get(0)?,
                name: r.get(1)?,
                col_type: r.get(2)?,
                options_json: r.get(3)?,
                width: r.get(4)?,
                sort_order: r.get(5)?,
            })
        })
        .map_err(|e| format!("build_envelope_json: column query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("build_envelope_json: column row collect failed: {}", e))?;

    let mut row_stmt = conn
        .prepare(
            "SELECT id, sort_order, height FROM sheet_rows WHERE note_id = ?1 ORDER BY sort_order ASC",
        )
        .map_err(|e| format!("build_envelope_json: row prepare failed: {}", e))?;
    let rows: Vec<EnvelopeRow> = row_stmt
        .query_map(params![note_id], |r| {
            Ok(EnvelopeRow { id: r.get(0)?, sort_order: r.get(1)?, height: r.get(2)? })
        })
        .map_err(|e| format!("build_envelope_json: row query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("build_envelope_json: row row collect failed: {}", e))?;

    let mut cell_stmt = conn
        .prepare(
            "SELECT row_id, column_id, value, style_json FROM sheet_cells WHERE note_id = ?1",
        )
        .map_err(|e| format!("build_envelope_json: cell prepare failed: {}", e))?;
    let cells: Vec<EnvelopeCell> = cell_stmt
        .query_map(params![note_id], |r| {
            Ok(EnvelopeCell {
                row_id: r.get(0)?,
                column_id: r.get(1)?,
                value: r.get(2)?,
                style_json: r.get(3)?,
            })
        })
        .map_err(|e| format!("build_envelope_json: cell query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("build_envelope_json: cell row collect failed: {}", e))?;

    let envelope = SheetSnapshotEnvelope { v: ENVELOPE_VERSION, columns, rows, cells };
    serde_json::to_string(&envelope)
        .map_err(|e| format!("build_envelope_json: serialization failed: {}", e))
}

// === Capture (§33.9.2) ===

/// Captures the pre-mutation grid state as a `note_versions` row
/// (`source = "sheet_snapshot"`). Throttled for grid mutations
/// (`bypass_throttle = false`); bypassed for structural mutations
/// (`bypass_throttle = true` — add/delete/reorder column or row, column
/// type change).
///
/// Caller's responsibility: call this INSIDE the mutation's own
/// transaction, BEFORE the mutation's own DB writes (immediately after
/// `BEGIN TRANSACTION`) — a capture failure must roll back the whole
/// mutation, not just skip silently. Because the capture read runs
/// before this transaction's own INSERT/UPDATE/DELETE executes, ordinary
/// transaction visibility guarantees this captures pre-mutation state
/// with no special snapshot-isolation trickery needed.
///
/// Throttle check is scoped to `source = 'sheet_snapshot'` specifically
/// (not "any source") — a `pre_restore` row for this sheet must not
/// suppress the next grid-edit snapshot, and vice versa. No dedup-by-
/// content-equality check (unlike `maybe_save_manual_version`) — a
/// snapshot captures PRE-mutation state, and by construction the
/// mutation is about to change something, so content-identity dedup
/// would rarely fire; the throttle alone satisfies "a burst of cell
/// edits produces one snapshot, not fifty."
pub(crate) fn maybe_capture_sheet_snapshot(
    conn: &Connection,
    note_id: &str,
    bypass_throttle: bool,
) -> Result<(), String> {
    if !bypass_throttle {
        let last_created_at: Option<String> = conn
            .query_row(
                "SELECT created_at FROM note_versions
                 WHERE note_id = ?1 AND source = ?2
                 ORDER BY created_at DESC LIMIT 1",
                params![note_id, SHEET_SNAPSHOT_SOURCE],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| format!("maybe_capture_sheet_snapshot: throttle lookup failed: {}", e))?;

        if let Some(ts) = last_created_at {
            let last: chrono::DateTime<chrono::Utc> =
                ts.parse().unwrap_or(chrono::DateTime::<chrono::Utc>::MIN_UTC);
            let elapsed_ok = chrono::Utc::now().signed_duration_since(last).num_seconds()
                >= crate::notes::MANUAL_EDIT_THROTTLE_SECS;
            if !elapsed_ok {
                return Ok(()); // throttled — skip capture
            }
        }
    }

    let envelope_json = build_envelope_json(conn, note_id)?;
    crate::versions::save_version(conn, note_id, &envelope_json, SHEET_SNAPSHOT_SOURCE)
        .map(|_| ())
}

// === Restore (§33.9.3) ===

/// Diff-applies snapshot columns against live `sheet_columns` rows for
/// `note_id`, by stored `id` (upsert-by-id for present columns,
/// cascade-delete for live columns absent from the snapshot). Must run
/// BEFORE `diff_apply_rows`/`diff_apply_cells` — column deletes cascade
/// their cells, so cell diff-apply must see the post-column-diff state.
fn diff_apply_columns(
    conn: &Connection,
    note_id: &str,
    columns: &[EnvelopeColumn],
) -> Result<(), String> {
    let snapshot_ids: HashSet<&str> = columns.iter().map(|c| c.id.as_str()).collect();

    let mut stmt = conn
        .prepare("SELECT id FROM sheet_columns WHERE note_id = ?1")
        .map_err(|e| format!("diff_apply_columns: live-id prepare failed: {}", e))?;
    let live_ids: Vec<String> = stmt
        .query_map(params![note_id], |r| r.get::<_, String>(0))
        .map_err(|e| format!("diff_apply_columns: live-id query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("diff_apply_columns: live-id collect failed: {}", e))?;

    for live_id in &live_ids {
        if !snapshot_ids.contains(live_id.as_str()) {
            // §1.12.2 point 4: cascade-delete cells before the column itself.
            conn.execute("DELETE FROM sheet_cells WHERE column_id = ?1", params![live_id])
                .map_err(|e| format!("diff_apply_columns: cell cascade delete failed for {}: {}", live_id, e))?;
            conn.execute("DELETE FROM sheet_columns WHERE id = ?1", params![live_id])
                .map_err(|e| format!("diff_apply_columns: column delete failed for {}: {}", live_id, e))?;
        }
    }

    for col in columns {
        conn.execute(
            "INSERT INTO sheet_columns (id, note_id, name, col_type, options_json, width, sort_order, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, datetime('now'), datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                col_type = excluded.col_type,
                options_json = excluded.options_json,
                width = excluded.width,
                sort_order = excluded.sort_order,
                updated_at = datetime('now')",
            params![col.id, note_id, col.name, col.col_type, col.options_json, col.width, col.sort_order],
        )
        .map_err(|e| format!("diff_apply_columns: upsert failed for {}: {}", col.id, e))?;
    }

    Ok(())
}

/// Diff-applies snapshot rows against live `sheet_rows` rows for
/// `note_id`. Same shape as `diff_apply_columns`. Must run AFTER
/// `diff_apply_columns` and BEFORE `diff_apply_cells`, per the same
/// cascade-ordering rationale.
fn diff_apply_rows(conn: &Connection, note_id: &str, rows: &[EnvelopeRow]) -> Result<(), String> {
    let snapshot_ids: HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();

    let mut stmt = conn
        .prepare("SELECT id FROM sheet_rows WHERE note_id = ?1")
        .map_err(|e| format!("diff_apply_rows: live-id prepare failed: {}", e))?;
    let live_ids: Vec<String> = stmt
        .query_map(params![note_id], |r| r.get::<_, String>(0))
        .map_err(|e| format!("diff_apply_rows: live-id query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("diff_apply_rows: live-id collect failed: {}", e))?;

    for live_id in &live_ids {
        if !snapshot_ids.contains(live_id.as_str()) {
            conn.execute("DELETE FROM sheet_cells WHERE row_id = ?1", params![live_id])
                .map_err(|e| format!("diff_apply_rows: cell cascade delete failed for {}: {}", live_id, e))?;
            conn.execute("DELETE FROM sheet_rows WHERE id = ?1", params![live_id])
                .map_err(|e| format!("diff_apply_rows: row delete failed for {}: {}", live_id, e))?;
        }
    }

    for row in rows {
        conn.execute(
            "INSERT INTO sheet_rows (id, note_id, sort_order, height, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'), datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
                sort_order = excluded.sort_order,
                height = excluded.height,
                updated_at = datetime('now')",
            params![row.id, note_id, row.sort_order, row.height],
        )
        .map_err(|e| format!("diff_apply_rows: upsert failed for {}: {}", row.id, e))?;
    }

    Ok(())
}

/// Diff-applies snapshot cells against live `sheet_cells` rows for
/// `note_id`, by the composite `(row_id, column_id)` key. Must run LAST
/// — by this point `diff_apply_columns`/`diff_apply_rows` have already
/// cascade-deleted cells belonging to since-deleted columns/rows, so
/// every remaining live cell's coordinate is guaranteed still valid
/// (avoids double-deleting or leaving stray cells).
///
/// No formula validation here (§33.9.3) — restore's job is exact
/// reproduction of a past state, not gatekeeping; an unparseable
/// restored formula degrades to `#PARSE!` at recompute time exactly like
/// a sync-arrived one does (§33.5.3's documented asymmetry), never
/// blocks the whole restore.
///
/// Skips a cell whose live `value`/`style_json` already match the
/// snapshot — avoids a needless `updated_at` bump / CRR changefeed entry
/// for cells the restore didn't actually change (§33.9.3, "unchanged
/// cells produce no changefeed entries").
fn diff_apply_cells(conn: &Connection, note_id: &str, cells: &[EnvelopeCell]) -> Result<(), String> {
    let snapshot_keys: HashSet<(&str, &str)> =
        cells.iter().map(|c| (c.row_id.as_str(), c.column_id.as_str())).collect();

    let mut stmt = conn
        .prepare("SELECT row_id, column_id FROM sheet_cells WHERE note_id = ?1")
        .map_err(|e| format!("diff_apply_cells: live-key prepare failed: {}", e))?;
    let live_keys: Vec<(String, String)> = stmt
        .query_map(params![note_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| format!("diff_apply_cells: live-key query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("diff_apply_cells: live-key collect failed: {}", e))?;

    for (row_id, column_id) in &live_keys {
        if !snapshot_keys.contains(&(row_id.as_str(), column_id.as_str())) {
            conn.execute(
                "DELETE FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![row_id, column_id],
            )
            .map_err(|e| format!("diff_apply_cells: clear delete failed for ({}, {}): {}", row_id, column_id, e))?;
        }
    }

    for cell in cells {
        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT value, style_json FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![cell.row_id, cell.column_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| format!("diff_apply_cells: existing lookup failed for ({}, {}): {}", cell.row_id, cell.column_id, e))?;

        if let Some((ev, es)) = &existing {
            if ev == &cell.value && es == &cell.style_json {
                continue; // no-op — skip to avoid a needless CRR write
            }
        }

        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id, value, style_json, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
             ON CONFLICT(row_id, column_id) DO UPDATE SET
                value = excluded.value,
                style_json = excluded.style_json,
                updated_at = excluded.updated_at",
            params![cell.row_id, cell.column_id, note_id, cell.value, cell.style_json],
        )
        .map_err(|e| format!("diff_apply_cells: upsert failed for ({}, {}): {}", cell.row_id, cell.column_id, e))?;
    }

    Ok(())
}

/// The sheet-native restore path (§33.9.3) — replaces the general-note
/// `versions::restore_version` for sheet notes, which is markdown-
/// specific and NOT reused here.
///
/// Transaction structure (bare-SQL `BEGIN`/`COMMIT`/`ROLLBACK`, matching
/// `sheets.rs`'s existing idiom):
///   1. `BEGIN TRANSACTION`
///   2. Capture pre-restore snapshot, unconditional/throttle-bypassed —
///      restore is itself undoable. Reuses the EXISTING `pre_restore`
///      source tag (Matt ruling 2026-09-04, item 2), NOT a sheet-specific
///      variant.
///   3. `get_version(conn, version_id)` → parse `content` as the JSON
///      envelope; reject any `v != 1` cleanly.
///   4. Diff-apply: columns, then rows, then cells (order load-bearing —
///      see each function's doc comment).
///   5. `recompute_and_regen` (PR 7's existing chokepoint, reused
///      verbatim — no new recompute logic).
///   6. `COMMIT`
///
/// Any failure at any step rolls back the ENTIRE transaction — including
/// the pre-restore capture itself, so a failed restore leaves the grid
/// (and version history) completely unchanged, not partially mutated.
pub(crate) fn restore_sheet_snapshot_internal(
    conn: &Connection,
    note_id: &str,
    version_id: &str,
) -> Result<crate::sheets::SheetData, String> {
    conn.execute("BEGIN TRANSACTION", [])
        .map_err(|e| format!("restore_sheet_snapshot: failed to begin transaction: {}", e))?;

    // Step 2: pre-restore snapshot, unconditional (throttle-bypassed —
    // restore is itself an undo point). Existing `pre_restore` tag.
    let current_json = match build_envelope_json(conn, note_id) {
        Ok(j) => j,
        Err(e) => {
            conn.execute("ROLLBACK", []).ok();
            return Err(e);
        }
    };
    if let Err(e) = crate::versions::save_version(conn, note_id, &current_json, PRE_RESTORE_SOURCE) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    // Step 3: fetch + parse the target version.
    let version = match crate::versions::get_version(conn, version_id) {
        Ok(v) => v,
        Err(e) => {
            conn.execute("ROLLBACK", []).ok();
            return Err(e);
        }
    };
    if version.note_id != note_id {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!(
            "restore_sheet_snapshot: version {} belongs to note {}, not {}",
            version_id, version.note_id, note_id
        ));
    }

    let envelope: SheetSnapshotEnvelope = match serde_json::from_str(&version.content) {
        Ok(e) => e,
        Err(e) => {
            conn.execute("ROLLBACK", []).ok();
            return Err(format!("restore_sheet_snapshot: envelope parse failed: {}", e));
        }
    };
    if envelope.v != ENVELOPE_VERSION {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!(
            "restore_sheet_snapshot: unsupported envelope version {} (expected {})",
            envelope.v, ENVELOPE_VERSION
        ));
    }

    // Step 4: diff-apply, in load-bearing order (columns, rows, cells).
    if let Err(e) = diff_apply_columns(conn, note_id, &envelope.columns) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }
    if let Err(e) = diff_apply_rows(conn, note_id, &envelope.rows) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }
    if let Err(e) = diff_apply_cells(conn, note_id, &envelope.cells) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    // Step 5: reuse PR 7's existing recompute-then-regen chokepoint verbatim.
    if let Err(e) = crate::sheets::recompute_and_regen(conn, note_id) {
        conn.execute("ROLLBACK", []).ok();
        return Err(e);
    }

    if let Err(e) = conn.execute("COMMIT", []) {
        conn.execute("ROLLBACK", []).ok();
        return Err(format!("restore_sheet_snapshot: failed to commit: {}", e));
    }

    crate::sheets::get_sheet_data_internal(conn, note_id)
}

/// Restore a sheet note to a previously captured snapshot (§33.9.3).
/// `version_id` is a `note_versions.id` returned by `get_note_versions`/
/// `get_note_version` (reused as-is for listing/fetching sheet
/// snapshots — a sheet snapshot is still an ordinary `note_versions`
/// row, just with `content` holding the JSON envelope instead of
/// markdown).
///
/// Ships callable-but-unwired (#2339 PR 8): registered here and in
/// `lib.rs`, no frontend UI calls it yet — the version-history panel that
/// lists/triggers sheet restores is design-gated.
#[tauri::command]
pub fn restore_sheet_snapshot(
    note_id: String,
    version_id: String,
    db: tauri::State<crate::database::DbState>,
) -> Result<crate::sheets::SheetData, String> {
    eprintln!(
        "[SHEETS_SNAPSHOTS] restore_sheet_snapshot called with: note_id={}, version_id={}",
        note_id, version_id
    );
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    restore_sheet_snapshot_internal(&conn, &note_id, &version_id)
}

// === Version-history preview: computed values (item 32, #2874) ===

/// Recomputes a sheet snapshot's formula cells for DISPLAY ONLY. Never
/// writes anything back to `note_versions.content` (ADR-0066 D4 remains
/// fully intact — this is a pure read, computed fresh on every call,
/// same as the live grid's own on-load recompute). Returns an EMPTY map
/// (not an `Err`) if `version_id` doesn't parse as a `v:1` sheet envelope
/// (e.g. a markdown note's version) — mirrors
/// `parse_sheet_snapshot_envelope`'s existing frontend fallback contract
/// (never blocks the preview, just shows nothing computed).
///
/// Every formula cell's raw ID-form value is ALSO translated to A1
/// notation (via `translate_formula_ids_to_a1`, using this snapshot's
/// OWN column/row ordering at capture time — not the live sheet's
/// current ordering) so the caller never has to show, and can never
/// accidentally show, a raw internal column/row ID. Matt's ruling
/// (2026-09-15): a formula cell with no computed entry (in-flight or
/// failed) must render as an empty wedge, never the raw stored string —
/// this function's contract supports that by simply omitting such cells
/// from the returned map (the frontend's fallback is then "no entry" =>
/// empty, never falling back to the raw string).
fn get_sheet_snapshot_computed_values_internal(
    conn: &Connection,
    version_id: &str,
) -> Result<std::collections::HashMap<String, String>, String> {
    let version = crate::versions::get_version(conn, version_id)?;
    let envelope: SheetSnapshotEnvelope = match serde_json::from_str(&version.content) {
        Ok(e) => e,
        Err(_) => return Ok(std::collections::HashMap::new()), // not a sheet envelope -- nothing to compute
    };

    let ordered_column_ids: Vec<String> = envelope.columns.iter().map(|c| c.id.clone()).collect();
    let ordered_row_ids: Vec<String> = envelope.rows.iter().map(|r| r.id.clone()).collect();
    let raw_values: std::collections::HashMap<(String, String), String> = envelope
        .cells
        .iter()
        .map(|c| ((c.column_id.clone(), c.row_id.clone()), c.value.clone()))
        .collect();

    let computed = crate::formula::recompute::compute_envelope_values(
        ordered_column_ids.clone(),
        ordered_row_ids.clone(),
        raw_values.clone(),
    );

    // Key as "{column_id}:{row_id}" strings for a JSON-serializable,
    // frontend-friendly map (Rust tuple keys don't serde to JSON object
    // keys directly).
    //
    // Matt's ruling (2026-09-15): "Users must never see internal IDs
    // under any circumstance." `CellValue::Error(FormulaError::Parse(msg))`
    // is the one case where `msg` can embed a raw `{column_id:row_id}`
    // internal reference verbatim — the parser's `unexpected token
    // {other:?}` error path Debug-formats `Token::IdRef { column_id,
    // row_id }`, and a version-history envelope's formulas are stored in
    // ID-form, so a malformed historical formula's parse error can
    // literally contain a real column/row UUID. Sanitize: drop the
    // detail message for `#PARSE!` and show only the bare error code, so
    // no code path in this map can ever surface an internal ID, no
    // matter what the parser's error text happens to contain. All other
    // `CellValue`/`FormulaError` variants' `Display` output never embeds
    // a cell coordinate, so no sanitization is needed for them.
    let mut out = std::collections::HashMap::new();
    for ((col_id, row_id), value) in computed.into_iter() {
        let key = format!("{col_id}:{row_id}");
        let display = match value {
            crate::formula::CellValue::Error(crate::formula::FormulaError::Parse(_)) => {
                "#PARSE!".to_string()
            }
            other => other.to_string(),
        };
        out.insert(key, display);
    }
    let _ = (ordered_column_ids, ordered_row_ids, raw_values); // retained for potential future A1-formula-text response field
    Ok(out)
}

/// Tauri command wrapping `get_sheet_snapshot_computed_values_internal`
/// (item 32, #2874). See that function's doc comment for the full
/// contract.
#[tauri::command]
pub fn get_sheet_snapshot_computed_values(
    version_id: String,
    db: tauri::State<crate::database::DbState>,
) -> Result<std::collections::HashMap<String, String>, String> {
    let conn = db.0.lock().map_err(|e| format!("DB lock error: {}", e))?;
    get_sheet_snapshot_computed_values_internal(&conn, &version_id)
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheets::{create_sheet_column_internal, create_sheet_row_internal, delete_sheet_row_internal, reorder_sheet_columns_internal, reorder_sheet_rows_internal, update_sheet_cell_internal, update_sheet_column_internal};
    use rusqlite::Connection;

    /// Real `sheet_columns`/`sheet_rows`/`sheet_cells`/`note_versions` DDL
    /// plus a minimal `notes`/`projects` schema — mirrors `sheets.rs`'s
    /// own `create_test_db` so every capture/restore hook exercises the
    /// real command internals unmodified.
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
        let data = crate::sheets::create_sheet_internal(conn, req, None)
            .expect("create_sheet_internal should succeed");
        data.note_meta.id
    }

    fn count_sheet_snapshot_versions(conn: &Connection, note_id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM note_versions WHERE note_id = ?1 AND source = 'sheet_snapshot'",
            params![note_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn count_pre_restore_versions(conn: &Connection, note_id: &str) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM note_versions WHERE note_id = ?1 AND source = 'pre_restore'",
            params![note_id],
            |r| r.get(0),
        )
        .unwrap()
    }

    /// Backdates EVERY existing `sheet_snapshot` row for `note_id` to
    /// `seconds_ago`. Backdating only the single latest row would leave an
    /// earlier (but still real-time-recent) row as the new "most recent"
    /// once the former latest is pushed further into the past — the
    /// throttle gate's `ORDER BY created_at DESC LIMIT 1` would then pick
    /// THAT row instead, defeating the backdate. Blanket-backdating avoids
    /// this entirely for test setup purposes.
    fn backdate_all_sheet_snapshots(conn: &Connection, note_id: &str, seconds_ago: i64) {
        let ts = (chrono::Utc::now() - chrono::Duration::seconds(seconds_ago)).to_rfc3339();
        conn.execute(
            "UPDATE note_versions SET created_at = ?1 WHERE note_id = ?2 AND source = 'sheet_snapshot'",
            params![ts, note_id],
        )
        .unwrap();
    }

    // ── Capture: grid mutations (throttled) ─────────────────────────────

    #[test]
    fn test_capture_on_cell_value_change() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Amount", "number", None, None, None).unwrap();
        // Setup's own structural captures (row/column create) leave a
        // very recent sheet_snapshot row — backdate past the throttle
        // window so the mutation under test isn't itself throttled by
        // setup noise.
        backdate_all_sheet_snapshots(&conn, &note_id, 61);

        let before = count_sheet_snapshot_versions(&conn, &note_id);
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("42"), None).unwrap();
        let after = count_sheet_snapshot_versions(&conn, &note_id);

        assert_eq!(after, before + 1, "cell value change must capture exactly one new sheet_snapshot row");

        // Content must be pre-mutation JSON — the cell should NOT be
        // present with value "42" in the captured envelope (it didn't
        // exist yet at capture time).
        let latest: String = conn
            .query_row(
                "SELECT content FROM note_versions WHERE note_id = ?1 AND source = 'sheet_snapshot' ORDER BY created_at DESC LIMIT 1",
                params![&note_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(!latest.contains("\"value\":\"42\""), "captured snapshot must reflect PRE-mutation state");
    }

    #[test]
    fn test_capture_fires_on_style_only_change() {
        // Recommend: yes, capture fires on style-only edits too (Reed's
        // recommendation in the scope report) — distinct from the
        // separate body-regen gate, which explicitly skips style-only.
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Amount", "number", None, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("42"), None).unwrap();
        backdate_all_sheet_snapshots(&conn, &note_id, 61);

        let before = count_sheet_snapshot_versions(&conn, &note_id);
        // Style-only edit — no value change.
        update_sheet_cell_internal(&conn, &row.id, &col.id, None, Some(r#"{"bg":"red"}"#)).unwrap();
        let after = count_sheet_snapshot_versions(&conn, &note_id);

        assert_eq!(after, before + 1, "style-only edit must still capture (throttle window elapsed check aside)");
    }

    #[test]
    fn test_capture_throttled_within_window() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Amount", "number", None, None, None).unwrap();

        // First grid mutation captures (no prior sheet_snapshot row).
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("1"), None).unwrap();
        let after_first = count_sheet_snapshot_versions(&conn, &note_id);

        // Second grid mutation immediately after — within throttle window.
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("2"), None).unwrap();
        let after_second = count_sheet_snapshot_versions(&conn, &note_id);

        assert_eq!(after_second, after_first, "two grid mutations within the throttle window must produce exactly one sheet_snapshot row total for the second batch");
    }

    #[test]
    fn test_capture_fires_again_after_throttle_window() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Amount", "number", None, None, None).unwrap();

        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("1"), None).unwrap();
        backdate_all_sheet_snapshots(&conn, &note_id, 61);
        let before = count_sheet_snapshot_versions(&conn, &note_id);

        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("2"), None).unwrap();
        let after = count_sheet_snapshot_versions(&conn, &note_id);

        assert_eq!(after, before + 1, "a mutation past the throttle window must produce a new sheet_snapshot row");
    }

    // ── Capture: structural mutations (bypass throttle) ─────────────────

    #[test]
    fn test_structural_bypasses_throttle_column_add() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");

        let before = count_sheet_snapshot_versions(&conn, &note_id);
        create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        create_sheet_column_internal(&conn, &note_id, "B", "text", None, None, None).unwrap();
        let after = count_sheet_snapshot_versions(&conn, &note_id);

        assert_eq!(after, before + 2, "two structural column adds within the throttle window must BOTH capture");
    }

    #[test]
    fn test_structural_bypasses_throttle_row_delete() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let row2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let before = count_sheet_snapshot_versions(&conn, &note_id);
        delete_sheet_row_internal(&conn, &row1.id).unwrap();
        delete_sheet_row_internal(&conn, &row2.id).unwrap();
        let after = count_sheet_snapshot_versions(&conn, &note_id);

        assert_eq!(after, before + 2, "two structural row deletes within the throttle window must BOTH capture");
    }

    #[test]
    fn test_structural_bypasses_throttle_type_change() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();

        let before = count_sheet_snapshot_versions(&conn, &note_id);
        update_sheet_column_internal(&conn, &col.id, None, Some("number"), None, None).unwrap();
        update_sheet_column_internal(&conn, &col.id, None, Some("currency"), None, None).unwrap();
        let after = count_sheet_snapshot_versions(&conn, &note_id);

        assert_eq!(after, before + 2, "two structural type changes within the throttle window must BOTH capture");
    }

    #[test]
    fn test_structural_bypasses_throttle_column_reorder() {
        // Matt ruling 2026-09-04: reorders are structural.
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let c1 = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let c2 = create_sheet_column_internal(&conn, &note_id, "B", "text", None, None, None).unwrap();

        let before = count_sheet_snapshot_versions(&conn, &note_id);
        reorder_sheet_columns_internal(&conn, &note_id, &[c2.id.clone(), c1.id.clone()]).unwrap();
        reorder_sheet_columns_internal(&conn, &note_id, &[c1.id.clone(), c2.id.clone()]).unwrap();
        let after = count_sheet_snapshot_versions(&conn, &note_id);

        assert_eq!(after, before + 2, "two column reorders within the throttle window must BOTH capture (structural bypass)");
    }

    #[test]
    fn test_structural_bypasses_throttle_row_reorder() {
        // Matt ruling 2026-09-04: reorders are structural.
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let r1 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let r2 = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let before = count_sheet_snapshot_versions(&conn, &note_id);
        reorder_sheet_rows_internal(&conn, &note_id, &[r2.id.clone(), r1.id.clone()]).unwrap();
        reorder_sheet_rows_internal(&conn, &note_id, &[r1.id.clone(), r2.id.clone()]).unwrap();
        let after = count_sheet_snapshot_versions(&conn, &note_id);

        assert_eq!(after, before + 2, "two row reorders within the throttle window must BOTH capture (structural bypass)");
    }

    // ── Envelope shape ───────────────────────────────────────────────────

    #[test]
    fn test_capture_envelope_shape() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "Formula", "number", None, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("=1+1"), None).unwrap();

        // Build the envelope directly off CURRENT state (this is exactly
        // what `maybe_capture_sheet_snapshot` does internally at its own
        // call time) — avoids the pre-mutation-timing confusion of reading
        // back a capture hook's necessarily-PRE-mutation snapshot.
        let json = build_envelope_json(&conn, &note_id).unwrap();
        let parsed: SheetSnapshotEnvelope = serde_json::from_str(&json).expect("must parse as v1 envelope");

        assert_eq!(parsed.v, 1);
        assert!(parsed.columns.iter().any(|c| c.id == col.id));
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.rows[0].id, row.id);
        let formula_cell = parsed.cells.iter().find(|c| c.column_id == col.id && c.row_id == row.id);
        assert_eq!(
            formula_cell.map(|c| c.value.as_str()),
            Some("=1+1"),
            "captured cell value must be the raw formula string, not a computed value"
        );
    }

    // ── Restore ──────────────────────────────────────────────────────────

    #[test]
    fn test_restore_preserves_ids() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("hello"), None).unwrap();

        // Capture a snapshot to restore to.
        let version_id = crate::versions::save_version(&conn, &note_id, &build_envelope_json(&conn, &note_id).unwrap(), SHEET_SNAPSHOT_SOURCE).unwrap();

        // Mutate further.
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("changed"), None).unwrap();

        let restored = restore_sheet_snapshot_internal(&conn, &note_id, &version_id).unwrap();

        assert_eq!(restored.rows.len(), 1);
        assert_eq!(restored.rows[0].id, row.id, "row id must be preserved across restore");
        assert_eq!(restored.columns.iter().find(|c| c.id == col.id).map(|c| &c.id), Some(&col.id));
        let cell = restored.cells.iter().find(|c| c.row_id == row.id && c.column_id == col.id).unwrap();
        assert_eq!(cell.raw_value, "hello", "restore must revert the cell value");
    }

    #[test]
    fn test_restore_deletes_column_absent_from_snapshot() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let col_a = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();

        let version_id = crate::versions::save_version(&conn, &note_id, &build_envelope_json(&conn, &note_id).unwrap(), SHEET_SNAPSHOT_SOURCE).unwrap();

        // Add another column + a cell in it, after the snapshot.
        let col_b = create_sheet_column_internal(&conn, &note_id, "B", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col_b.id, Some("val"), None).unwrap();

        let restored = restore_sheet_snapshot_internal(&conn, &note_id, &version_id).unwrap();

        assert!(!restored.columns.iter().any(|c| c.id == col_b.id), "column added after snapshot must be gone");
        assert!(restored.columns.iter().any(|c| c.id == col_a.id), "pre-existing column must remain");
        assert!(!restored.cells.iter().any(|c| c.column_id == col_b.id), "cells in the deleted column must be gone");

        // Also verify at the DB level (cascade actually ran).
        let cell_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sheet_cells WHERE column_id = ?1", params![&col_b.id], |r| r.get(0))
            .unwrap();
        assert_eq!(cell_count, 0);
    }

    #[test]
    fn test_restore_deletes_row_absent_from_snapshot() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row_a = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        let version_id = crate::versions::save_version(&conn, &note_id, &build_envelope_json(&conn, &note_id).unwrap(), SHEET_SNAPSHOT_SOURCE).unwrap();

        let row_b = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row_b.id, &col.id, Some("val"), None).unwrap();

        let restored = restore_sheet_snapshot_internal(&conn, &note_id, &version_id).unwrap();

        assert!(!restored.rows.iter().any(|r| r.id == row_b.id), "row added after snapshot must be gone");
        assert!(restored.rows.iter().any(|r| r.id == row_a.id), "pre-existing row must remain");
        assert!(!restored.cells.iter().any(|c| c.row_id == row_b.id), "cells in the deleted row must be gone");
    }

    #[test]
    fn test_restore_creates_pre_restore_snapshot() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("v1"), None).unwrap();

        let version_id = crate::versions::save_version(&conn, &note_id, &build_envelope_json(&conn, &note_id).unwrap(), SHEET_SNAPSHOT_SOURCE).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("v2"), None).unwrap();

        let before = count_pre_restore_versions(&conn, &note_id);
        restore_sheet_snapshot_internal(&conn, &note_id, &version_id).unwrap();
        let after = count_pre_restore_versions(&conn, &note_id);

        assert_eq!(after, before + 1, "restore must create exactly one new pre_restore row before applying the diff");
    }

    #[test]
    fn test_restore_recomputes_and_regens_body() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("100"), None).unwrap();

        let version_id = crate::versions::save_version(&conn, &note_id, &build_envelope_json(&conn, &note_id).unwrap(), SHEET_SNAPSHOT_SOURCE).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("999"), None).unwrap();

        let body_before_restore: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert!(body_before_restore.contains("999"));

        restore_sheet_snapshot_internal(&conn, &note_id, &version_id).unwrap();

        let body_after_restore: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert!(body_after_restore.contains("100"), "restored body must reflect the restored grid's values");
        assert!(!body_after_restore.contains("999"), "restored body must NOT reflect the stale pre-restore value");
    }

    #[test]
    fn test_restore_is_one_transaction_rollback_on_failure() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("original"), None).unwrap();

        // Insert a malformed envelope directly as a fake version.
        let bad_version_id = crate::versions::save_version(&conn, &note_id, "{not valid json", "sheet_snapshot").unwrap();

        let pre_restore_before = count_pre_restore_versions(&conn, &note_id);
        let cell_before: String = conn
            .query_row(
                "SELECT value FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![&row.id, &col.id],
                |r| r.get(0),
            )
            .unwrap();

        let result = restore_sheet_snapshot_internal(&conn, &note_id, &bad_version_id);
        assert!(result.is_err(), "malformed envelope must fail the restore");

        // The whole transaction (including the pre-restore capture) must
        // have rolled back — no partial diff applied.
        let pre_restore_after = count_pre_restore_versions(&conn, &note_id);
        assert_eq!(pre_restore_after, pre_restore_before, "pre-restore capture must also roll back on failure");

        let cell_after: String = conn
            .query_row(
                "SELECT value FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
                params![&row.id, &col.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(cell_after, cell_before, "grid must be completely unchanged after a failed restore");
    }

    #[test]
    fn test_unparseable_restored_formula_degrades_to_parse_error() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();

        // Hand-craft a snapshot envelope with a since-invalid-syntax
        // formula — restore must NOT reject this at write time (no
        // formula validation on restore), it should degrade to #PARSE!
        // at recompute time.
        let envelope = SheetSnapshotEnvelope {
            v: 1,
            columns: vec![EnvelopeColumn {
                id: col.id.clone(),
                name: col.name.clone(),
                col_type: col.col_type.clone(),
                options_json: col.options_json.clone(),
                width: col.width,
                sort_order: col.sort_order,
            }],
            rows: vec![EnvelopeRow { id: row.id.clone(), sort_order: row.sort_order, height: row.height }],
            cells: vec![EnvelopeCell {
                row_id: row.id.clone(),
                column_id: col.id.clone(),
                value: "=((unbalanced".to_string(),
                style_json: "{}".to_string(),
            }],
        };
        let bad_formula_json = serde_json::to_string(&envelope).unwrap();
        let version_id = crate::versions::save_version(&conn, &note_id, &bad_formula_json, "sheet_snapshot").unwrap();

        let result = restore_sheet_snapshot_internal(&conn, &note_id, &version_id);
        assert!(result.is_ok(), "restore itself must succeed even with an unparseable formula");

        let restored = result.unwrap();
        let cell = restored.cells.iter().find(|c| c.row_id == row.id && c.column_id == col.id).unwrap();
        assert_eq!(cell.raw_value, "=((unbalanced", "raw formula string must be preserved verbatim, no validation rejection");

        let body: String = conn.query_row("SELECT body FROM notes WHERE id = ?1", params![&note_id], |r| r.get(0)).unwrap();
        assert!(body.contains("#PARSE!"), "recompute after restore must degrade the bad formula to #PARSE!, not fail the whole restore");
    }

    // ── Prune inheritance ────────────────────────────────────────────────

    #[test]
    fn test_prune_after_50_sheet_snapshots() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");

        // 51 direct sheet_snapshot captures via save_version, bypassing
        // the throttle entirely (this test proves prune inheritance, not
        // the throttle) — mirrors notes.rs's existing 51st-version test
        // shape.
        for i in 0..51 {
            let json = build_envelope_json(&conn, &note_id).unwrap();
            crate::versions::save_version(&conn, &note_id, &format!("{}#{}", json, i), "sheet_snapshot").unwrap();
        }

        let total = count_sheet_snapshot_versions(&conn, &note_id);
        assert_eq!(total, 50, "save_version's all-sources prune must cap the pool at 50 even for sheet_snapshot rows");
    }

    // ── get_sheet_snapshot_computed_values_internal (item 32, #2874) ──────

    #[test]
    fn test_get_sheet_snapshot_computed_values_internal_returns_a1_formula_never_id() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col_a = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let col_b = create_sheet_column_internal(&conn, &note_id, "B", "number", None, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col_a.id, Some("5"), None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col_b.id, Some(&format!("={{{}:{}}}*2", col_a.id, row.id)), None).unwrap();

        let json = build_envelope_json(&conn, &note_id).unwrap();
        let version_id = crate::versions::save_version(&conn, &note_id, &json, SHEET_SNAPSHOT_SOURCE).unwrap();

        let computed = get_sheet_snapshot_computed_values_internal(&conn, &version_id).unwrap();
        let key = format!("{}:{}", col_b.id, row.id);
        assert_eq!(computed.get(&key), Some(&"10".to_string()), "computed value must be the evaluated result, not the raw formula");

        // Nothing in the returned map's values may contain either raw
        // internal UUID (column or row) — proves the map only ever
        // carries computed display strings, never IDs.
        for value in computed.values() {
            assert!(!value.contains(&col_a.id), "computed display string must never contain a raw column ID");
            assert!(!value.contains(&row.id), "computed display string must never contain a raw row ID");
        }
    }

    #[test]
    fn test_get_sheet_snapshot_computed_values_internal_non_sheet_version_returns_empty() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let version_id = crate::versions::save_version(&conn, &note_id, "# Just a markdown note\n\nSome text.", "manual_edit").unwrap();

        let computed = get_sheet_snapshot_computed_values_internal(&conn, &version_id).unwrap();
        assert!(computed.is_empty(), "a non-envelope version must return an empty map, not an Err");
    }

    #[test]
    fn test_get_sheet_snapshot_computed_values_internal_preserves_note_versions_content() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("=1+1"), None).unwrap();

        let json = build_envelope_json(&conn, &note_id).unwrap();
        let version_id = crate::versions::save_version(&conn, &note_id, &json, SHEET_SNAPSHOT_SOURCE).unwrap();

        let content_before: String = conn
            .query_row("SELECT content FROM note_versions WHERE id = ?1", params![&version_id], |r| r.get(0))
            .unwrap();

        // Call it multiple times — must be a pure read every time.
        get_sheet_snapshot_computed_values_internal(&conn, &version_id).unwrap();
        get_sheet_snapshot_computed_values_internal(&conn, &version_id).unwrap();

        let content_after: String = conn
            .query_row("SELECT content FROM note_versions WHERE id = ?1", params![&version_id], |r| r.get(0))
            .unwrap();

        assert_eq!(content_before, content_after, "ADR-0066 D4: computing display values must never write back to note_versions.content");
    }

    #[test]
    fn test_get_sheet_snapshot_computed_values_internal_div_by_zero() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col_a = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let col_b = create_sheet_column_internal(&conn, &note_id, "B", "number", None, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col_a.id, Some("0"), None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col_b.id, Some(&format!("=14/{{{}:{}}}", col_a.id, row.id)), None).unwrap();

        let json = build_envelope_json(&conn, &note_id).unwrap();
        let version_id = crate::versions::save_version(&conn, &note_id, &json, SHEET_SNAPSHOT_SOURCE).unwrap();

        let computed = get_sheet_snapshot_computed_values_internal(&conn, &version_id).unwrap();
        let key = format!("{}:{}", col_b.id, row.id);
        assert_eq!(computed.get(&key), Some(&"#DIV/0!".to_string()));
    }

    #[test]
    fn test_get_sheet_snapshot_computed_values_internal_cycle() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col_a = create_sheet_column_internal(&conn, &note_id, "A", "number", None, None, None).unwrap();
        let col_b = create_sheet_column_internal(&conn, &note_id, "B", "number", None, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col_a.id, Some(&format!("={{{}:{}}}", col_b.id, row.id)), None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col_b.id, Some(&format!("={{{}:{}}}", col_a.id, row.id)), None).unwrap();

        let json = build_envelope_json(&conn, &note_id).unwrap();
        let version_id = crate::versions::save_version(&conn, &note_id, &json, SHEET_SNAPSHOT_SOURCE).unwrap();

        let computed = get_sheet_snapshot_computed_values_internal(&conn, &version_id).unwrap();
        let key_a = format!("{}:{}", col_a.id, row.id);
        let key_b = format!("{}:{}", col_b.id, row.id);
        assert_eq!(computed.get(&key_a), Some(&"#CYCLE!".to_string()));
        assert_eq!(computed.get(&key_b), Some(&"#CYCLE!".to_string()));
    }

    #[test]
    fn test_get_sheet_snapshot_computed_values_internal_excludes_non_formula_cells() {
        let conn = create_test_db();
        insert_project(&conn, "p1");
        let note_id = make_sheet(&conn, "p1");
        let row = create_sheet_row_internal(&conn, &note_id, None, None).unwrap();
        let col = create_sheet_column_internal(&conn, &note_id, "A", "text", None, None, None).unwrap();
        update_sheet_cell_internal(&conn, &row.id, &col.id, Some("plain literal"), None).unwrap();

        let json = build_envelope_json(&conn, &note_id).unwrap();
        let version_id = crate::versions::save_version(&conn, &note_id, &json, SHEET_SNAPSHOT_SOURCE).unwrap();

        let computed = get_sheet_snapshot_computed_values_internal(&conn, &version_id).unwrap();
        let key = format!("{}:{}", col.id, row.id);
        assert!(!computed.contains_key(&key), "non-formula cells must not appear in the computed-values map at all");
    }
}
