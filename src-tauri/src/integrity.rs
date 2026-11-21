// Pane — Sheets Integrity Sweep (#2339, §33.8)
//
// Orphan sweep for the Sheets feature's side tables (`sheet_columns`,
// `sheet_rows`, `sheet_cells`). Because these are CRR tables with no FKs
// (app-enforced relationships per #493), a concurrent delete-vs-edit
// (one device deletes a sheet/row/column while another edits a cell in it)
// can leave dangling side-table rows behind after sync converges. This
// module is the single implementation of the cleanup sweep; both the
// launch-time trigger (lib.rs) and the post-sync trigger
// (sync/engine.rs::receive_sync_payload) call the same function here —
// "one implementation, two triggers, one owner" per §33.8.

use rusqlite::Connection;

/// Sweep orphaned `sheet_columns`/`sheet_rows`/`sheet_cells` rows.
///
/// Two orphan classes, four DELETE statements, one transaction:
///
/// - Class 1: side-table rows whose `note_id` has no matching `notes` row
///   with `note_type = 'sheet'`. This deliberately does NOT filter on
///   `deleted_at` — soft-deleted sheets (§1.12.2 point 1: "side tables
///   untouched" on soft-delete) still have a `notes` row with
///   `note_type = 'sheet'`, so they are correctly NOT swept here.
/// - Class 2: `sheet_cells` rows whose `row_id`/`column_id` no longer
///   exists in `sheet_rows`/`sheet_columns` — the concurrent
///   delete-vs-edit class the spec names explicitly. Runs after Class 1
///   so it doesn't need to special-case already-orphaned rows/columns.
///
/// Transactional (BEGIN/COMMIT, ROLLBACK on failure), eprintln-logged,
/// non-fatal to the caller — failures are logged and swallowed so a sweep
/// failure never blocks app startup or sync. Mirrors the workshop-sweep
/// error-handling shape at lib.rs's startup orphan sweep block.
pub fn sweep_orphaned_sheet_rows(conn: &Connection) {
    if let Err(e) = conn.execute("BEGIN", []) {
        eprintln!("[integrity] Failed to begin sheet orphan sweep transaction: {}", e);
        return;
    }

    let mut failed = false;
    for sql in [
        "DELETE FROM sheet_columns WHERE note_id NOT IN (SELECT id FROM notes WHERE note_type = 'sheet')",
        "DELETE FROM sheet_rows WHERE note_id NOT IN (SELECT id FROM notes WHERE note_type = 'sheet')",
        "DELETE FROM sheet_cells WHERE note_id NOT IN (SELECT id FROM notes WHERE note_type = 'sheet')",
        "DELETE FROM sheet_cells WHERE row_id NOT IN (SELECT id FROM sheet_rows) OR column_id NOT IN (SELECT id FROM sheet_columns)",
    ] {
        if let Err(e) = conn.execute(sql, []) {
            eprintln!("[integrity] sheet orphan sweep failed: {}", e);
            failed = true;
            break;
        }
    }

    if failed {
        conn.execute("ROLLBACK", []).ok();
        eprintln!("[integrity] Sheet orphan sweep rolled back");
    } else if let Err(e) = conn.execute("COMMIT", []) {
        eprintln!("[integrity] Failed to commit sheet orphan sweep transaction: {}", e);
        conn.execute("ROLLBACK", []).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal schema covering `notes` + the three sheet tables — enough
    /// for `sweep_orphaned_sheet_rows` to run unmodified.
    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE notes (
                 id         TEXT PRIMARY KEY,
                 note_type  TEXT,
                 deleted_at TEXT
             );
             CREATE TABLE sheet_columns (
                 id      TEXT NOT NULL PRIMARY KEY,
                 note_id TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE sheet_rows (
                 id      TEXT NOT NULL PRIMARY KEY,
                 note_id TEXT NOT NULL DEFAULT ''
             );
             CREATE TABLE sheet_cells (
                 row_id    TEXT NOT NULL,
                 column_id TEXT NOT NULL,
                 note_id   TEXT NOT NULL DEFAULT '',
                 PRIMARY KEY (row_id, column_id)
             );",
        )
        .unwrap();
        conn
    }

    /// Class 1: side-table rows with no matching `notes` row at all → swept.
    #[test]
    fn test_sweep_deletes_rows_with_no_matching_notes_row() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO sheet_columns (id, note_id) VALUES ('col-orphan', 'no-such-note')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_rows (id, note_id) VALUES ('row-orphan', 'no-such-note')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id) VALUES ('row-orphan', 'col-orphan', 'no-such-note')",
            [],
        )
        .unwrap();

        sweep_orphaned_sheet_rows(&conn);

        let col_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns", [], |r| r.get(0)).unwrap();
        let row_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_rows", [], |r| r.get(0)).unwrap();
        let cell_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells", [], |r| r.get(0)).unwrap();
        assert_eq!(col_count, 0, "orphaned sheet_columns row must be swept");
        assert_eq!(row_count, 0, "orphaned sheet_rows row must be swept");
        assert_eq!(cell_count, 0, "orphaned sheet_cells row must be swept");
    }

    /// Class 1: matching `notes` row exists but `note_type != 'sheet'` → still swept.
    #[test]
    fn test_sweep_deletes_rows_whose_note_is_not_a_sheet() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO notes (id, note_type) VALUES ('note-1', 'task')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_columns (id, note_id) VALUES ('col-1', 'note-1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_rows (id, note_id) VALUES ('row-1', 'note-1')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id) VALUES ('row-1', 'col-1', 'note-1')",
            [],
        )
        .unwrap();

        sweep_orphaned_sheet_rows(&conn);

        let col_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns", [], |r| r.get(0)).unwrap();
        let row_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_rows", [], |r| r.get(0)).unwrap();
        let cell_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells", [], |r| r.get(0)).unwrap();
        assert_eq!(col_count, 0, "sheet_columns row for a non-sheet note must be swept");
        assert_eq!(row_count, 0, "sheet_rows row for a non-sheet note must be swept");
        assert_eq!(cell_count, 0, "sheet_cells row for a non-sheet note must be swept");
    }

    /// §1.12.2 point 1: a soft-deleted sheet (deleted_at set, note_type
    /// still 'sheet') must NOT be swept — side tables stay untouched on
    /// soft-delete.
    #[test]
    fn test_sweep_does_not_delete_rows_for_soft_deleted_sheet() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO notes (id, note_type, deleted_at) VALUES ('note-sheet', 'sheet', '2026-09-03T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_columns (id, note_id) VALUES ('col-1', 'note-sheet')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_rows (id, note_id) VALUES ('row-1', 'note-sheet')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id) VALUES ('row-1', 'col-1', 'note-sheet')",
            [],
        )
        .unwrap();

        sweep_orphaned_sheet_rows(&conn);

        let col_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns", [], |r| r.get(0)).unwrap();
        let row_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_rows", [], |r| r.get(0)).unwrap();
        let cell_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells", [], |r| r.get(0)).unwrap();
        assert_eq!(col_count, 1, "soft-deleted sheet's sheet_columns row must survive");
        assert_eq!(row_count, 1, "soft-deleted sheet's sheet_rows row must survive");
        assert_eq!(cell_count, 1, "soft-deleted sheet's sheet_cells row must survive");
    }

    /// Class 2: a cell with a valid note_id but a row_id/column_id that no
    /// longer exists must be swept.
    #[test]
    fn test_sweep_deletes_cells_with_dangling_row_or_column_id() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO notes (id, note_type) VALUES ('note-sheet', 'sheet')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_columns (id, note_id) VALUES ('col-real', 'note-sheet')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_rows (id, note_id) VALUES ('row-real', 'note-sheet')",
            [],
        )
        .unwrap();
        // Valid note_id, but row_id points at nothing.
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id) VALUES ('row-missing', 'col-real', 'note-sheet')",
            [],
        )
        .unwrap();
        // Valid note_id, but column_id points at nothing.
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id) VALUES ('row-real', 'col-missing', 'note-sheet')",
            [],
        )
        .unwrap();
        // Fully valid cell — must survive.
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id) VALUES ('row-real', 'col-real', 'note-sheet')",
            [],
        )
        .unwrap();

        sweep_orphaned_sheet_rows(&conn);

        let cell_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells", [], |r| r.get(0)).unwrap();
        assert_eq!(cell_count, 1, "only the fully valid cell should survive Class 2 sweep");

        let remaining_row_id: String = conn
            .query_row("SELECT row_id FROM sheet_cells", [], |r| r.get(0))
            .unwrap();
        assert_eq!(remaining_row_id, "row-real");
    }

    /// Fully valid grid — nothing swept.
    #[test]
    fn test_sweep_leaves_fully_valid_grid_untouched() {
        let conn = create_test_db();
        conn.execute(
            "INSERT INTO notes (id, note_type) VALUES ('note-sheet', 'sheet')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_columns (id, note_id) VALUES ('col-1', 'note-sheet')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_rows (id, note_id) VALUES ('row-1', 'note-sheet')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id) VALUES ('row-1', 'col-1', 'note-sheet')",
            [],
        )
        .unwrap();

        sweep_orphaned_sheet_rows(&conn);

        let col_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_columns", [], |r| r.get(0)).unwrap();
        let row_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_rows", [], |r| r.get(0)).unwrap();
        let cell_count: i64 = conn.query_row("SELECT COUNT(*) FROM sheet_cells", [], |r| r.get(0)).unwrap();
        assert_eq!(col_count, 1);
        assert_eq!(row_count, 1);
        assert_eq!(cell_count, 1);
    }
}
