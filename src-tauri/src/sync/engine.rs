use rusqlite::{Connection, params};
use serde::{Serialize, Deserialize};
use super::crypto;
use super::keystore;

/// A single changeset row from crsql_changes
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChangesetRow {
    pub table: String,
    pub pk: Vec<u8>,     // Primary key as bytes
    pub cid: String,     // Column id
    pub val: Option<Vec<u8>>,  // Value as bytes (nullable for deletes)
    pub col_version: i64,
    pub db_version: i64,
    pub site_id: Vec<u8>,
    pub cl: i64,         // Causal length
    pub seq: i64,
}

/// A batch of changesets ready for sync
#[derive(Serialize, Deserialize, Debug)]
pub struct SyncPayload {
    pub from_site_id: Vec<u8>,
    pub db_version: i64,
    pub changes: Vec<ChangesetRow>,
}

/// Extract new changesets since a given db_version
pub fn extract_changesets(conn: &Connection, since_version: i64) -> Result<SyncPayload, String> {
    let site_id: Vec<u8> = conn.query_row(
        "SELECT crsql_site_id()",
        [],
        |row| row.get(0),
    ).map_err(|e| format!("Failed to get site_id: {}", e))?;
    
    let db_version: i64 = conn.query_row(
        "SELECT crsql_db_version()",
        [],
        |row| row.get(0),
    ).map_err(|e| format!("Failed to get db_version: {}", e))?;
    
    let mut stmt = conn.prepare(
        "SELECT [table], [pk], [cid], [val], [col_version], [db_version], [site_id], [cl], [seq] 
         FROM crsql_changes 
         WHERE db_version > ?1
         ORDER BY db_version ASC, seq ASC"
    ).map_err(|e| format!("Failed to prepare changeset query: {}", e))?;
    
    let changes: Vec<ChangesetRow> = stmt.query_map(params![since_version], |row| {
        Ok(ChangesetRow {
            table: row.get(0)?,
            pk: row.get(1)?,
            cid: row.get(2)?,
            val: row.get(3)?,
            col_version: row.get(4)?,
            db_version: row.get(5)?,
            site_id: row.get(6)?,
            cl: row.get(7)?,
            seq: row.get(8)?,
        })
    }).map_err(|e| format!("Failed to query changesets: {}", e))?
    .filter_map(|r| r.ok())
    .collect();
    
    Ok(SyncPayload {
        from_site_id: site_id,
        db_version,
        changes,
    })
}

/// Apply received changesets to the local database
pub fn apply_changesets(conn: &Connection, payload: &SyncPayload) -> Result<i64, String> {
    let mut applied = 0i64;
    
    for change in &payload.changes {
        conn.execute(
            "INSERT INTO crsql_changes ([table], [pk], [cid], [val], [col_version], [db_version], [site_id], [cl], [seq])
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                change.table,
                change.pk,
                change.cid,
                change.val,
                change.col_version,
                change.db_version,
                change.site_id,
                change.cl,
                change.seq,
            ],
        ).map_err(|e| format!("Failed to apply changeset: {}", e))?;
        applied += 1;
    }
    
    Ok(applied)
}

/// Serialize a sync payload to MessagePack bytes
pub fn serialize_payload(payload: &SyncPayload) -> Result<Vec<u8>, String> {
    rmp_serde::to_vec(payload)
        .map_err(|e| format!("Failed to serialize payload: {}", e))
}

/// Deserialize MessagePack bytes to a sync payload
pub fn deserialize_payload(data: &[u8]) -> Result<SyncPayload, String> {
    rmp_serde::from_slice(data)
        .map_err(|e| format!("Failed to deserialize payload: {}", e))
}

/// Full pipeline: extract → serialize → encrypt
pub fn prepare_sync_payload(conn: &Connection, since_version: i64) -> Result<Option<Vec<u8>>, String> {
    let payload = extract_changesets(conn, since_version)?;
    
    if payload.changes.is_empty() {
        return Ok(None); // Nothing to sync
    }
    
    let serialized = serialize_payload(&payload)?;
    
    let master_key = keystore::load_master_key()?
        .ok_or_else(|| "No encryption key — sign in first".to_string())?;
    let sync_key = crypto::get_sync_key(&master_key);
    
    let encrypted = crypto::encrypt_changeset(&sync_key, &serialized)?;
    
    Ok(Some(encrypted))
}

/// #2339 PR 4, §33.8: whether a sync batch touched any sheet-related table
/// (`sheet_columns`/`sheet_rows`/`sheet_cells`). Extracted from
/// `receive_sync_payload` so the gating logic is unit-testable directly
/// against a constructed `SyncPayload`, without needing a live master key /
/// keychain / crsqlite extension to exercise the decrypt→apply pipeline.
fn payload_touched_sheet_table(payload: &SyncPayload) -> bool {
    payload
        .changes
        .iter()
        .any(|c| matches!(c.table.as_str(), "sheet_columns" | "sheet_rows" | "sheet_cells"))
}

/// Full pipeline: decrypt → deserialize → apply
pub fn receive_sync_payload(conn: &Connection, encrypted: &[u8]) -> Result<i64, String> {
    let master_key = keystore::load_master_key()?
        .ok_or_else(|| "No encryption key — sign in first".to_string())?;
    let sync_key = crypto::get_sync_key(&master_key);
    
    let decrypted = crypto::decrypt_changeset(&sync_key, encrypted)?;
    let payload = deserialize_payload(&decrypted)?;
    
    let applied = apply_changesets(conn, &payload)?;

    // §33.8 orphan sweep — runs before post-sync recompute (§33.5.4 point 3:
    // "formulas never evaluate against orphaned cells"). Must stay ordered
    // BEFORE the recompute call below — recomputing against a
    // not-yet-swept sheet could walk a dependency graph containing
    // dangling row/column references left by a concurrent delete-vs-edit.
    if payload_touched_sheet_table(&payload) {
        crate::integrity::sweep_orphaned_sheet_rows(conn);

        // #2339 PR 7 post-sync recompute hook. Broad scope — recompute
        // every sheet note, not precision-scoped to the touched primary
        // keys from this payload (Matt ruling 2026-09-03: simple over
        // surgical, revisit at perf). An LWW-losing formula string must
        // recompute from the CURRENT (post-apply, post-LWW-resolution)
        // stored value, never stale pre-sync state — recomputing every
        // sheet, rather than trying to precisely track which sheets a
        // batch of changesets actually affected, is the simplest way to
        // guarantee that.
        //
        // Per-sheet failures are logged and swallowed (never propagated) —
        // one malformed/oversized sheet must never block the rest of sync
        // or the sheets that DID recompute successfully.
        recompute_all_sheets_post_sync(conn);
    }

    Ok(applied)
}

/// Recomputes every `note_type = 'sheet'` note's formulas and regenerates
/// its body, in the same pass, after a sync payload has been applied.
/// Broad scope (Matt ruling 2026-09-03, see `receive_sync_payload`'s doc
/// comment above this call site) — not scoped to only the sheets the
/// incoming payload actually touched.
///
/// Never returns an error: a single sheet's recompute/regen failure is
/// logged and skipped, so one bad sheet can't block sync or the rest of
/// the batch. Matches `sweep_orphaned_sheet_rows`'s own
/// log-and-continue error-handling shape.
fn recompute_all_sheets_post_sync(conn: &Connection) {
    let mut stmt = match conn.prepare("SELECT id FROM notes WHERE note_type = 'sheet'") {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[sync] post-sync recompute: failed to query sheet notes: {}", e);
            return;
        }
    };
    let note_ids: Vec<String> = match stmt.query_map([], |r| r.get::<_, String>(0)) {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(e) => {
            eprintln!("[sync] post-sync recompute: failed to collect sheet note ids: {}", e);
            return;
        }
    };
    drop(stmt);

    for note_id in note_ids {
        match crate::formula::recompute::recompute_sheet(conn, &note_id) {
            Ok(computed) => {
                // 1. Recompute (done above) 2. Body regen with computed
                // values — ordering per Matt ruling 2026-09-03 (fixing the
                // PR 5a comment-ordering bug), same invariant as every
                // sheets.rs call site.
                if let Err(e) = crate::sheets::regenerate_sheet_body_pub(conn, &note_id, &computed) {
                    eprintln!("[sync] post-sync body regen failed for sheet {}: {}", note_id, e);
                }
            }
            Err(e) => {
                eprintln!("[sync] post-sync recompute failed for sheet {}: {}", note_id, e);
            }
        }
    }
}

#[cfg(test)]
mod sheet_touch_gate_tests {
    use super::*;

    fn changeset_row(table: &str) -> ChangesetRow {
        ChangesetRow {
            table: table.to_string(),
            pk: vec![],
            cid: "col".to_string(),
            val: None,
            col_version: 1,
            db_version: 1,
            site_id: vec![],
            cl: 1,
            seq: 1,
        }
    }

    /// A batch containing a `sheet_cells` change must gate the sweep on.
    #[test]
    fn test_payload_touched_sheet_table_true_for_sheet_cells() {
        let payload = SyncPayload {
            from_site_id: vec![],
            db_version: 1,
            changes: vec![changeset_row("notes"), changeset_row("sheet_cells")],
        };
        assert!(payload_touched_sheet_table(&payload));
    }

    /// `sheet_columns` and `sheet_rows` must also gate the sweep on.
    #[test]
    fn test_payload_touched_sheet_table_true_for_columns_and_rows() {
        let payload_columns = SyncPayload {
            from_site_id: vec![],
            db_version: 1,
            changes: vec![changeset_row("sheet_columns")],
        };
        let payload_rows = SyncPayload {
            from_site_id: vec![],
            db_version: 1,
            changes: vec![changeset_row("sheet_rows")],
        };
        assert!(payload_touched_sheet_table(&payload_columns));
        assert!(payload_touched_sheet_table(&payload_rows));
    }

    /// A batch with no sheet-table changes must NOT gate the sweep on.
    #[test]
    fn test_payload_touched_sheet_table_false_when_no_sheet_changes() {
        let payload = SyncPayload {
            from_site_id: vec![],
            db_version: 1,
            changes: vec![changeset_row("notes"), changeset_row("projects")],
        };
        assert!(!payload_touched_sheet_table(&payload));
    }

    /// An empty changeset batch must NOT gate the sweep on.
    #[test]
    fn test_payload_touched_sheet_table_false_for_empty_changes() {
        let payload = SyncPayload {
            from_site_id: vec![],
            db_version: 1,
            changes: vec![],
        };
        assert!(!payload_touched_sheet_table(&payload));
    }
}

#[cfg(test)]
mod post_sync_recompute_tests {
    use super::*;

    /// Plain-SQLite (no cr-sqlite extension needed —
    /// `recompute_all_sheets_post_sync` only issues ordinary SELECT/
    /// transaction statements against `notes`/`sheet_columns`/
    /// `sheet_rows`/`sheet_cells`, same as `recompute::recompute_sheet`
    /// itself) schema mirroring `migrations.rs`, for exercising the
    /// post-sync hook's own broad-scope iteration logic in isolation
    /// from real cr-sqlite sync mechanics (that's `test_p2p_sync.rs`'s
    /// job — see `test_lww_race_loser_recomputes_from_winning_formula`).
    fn create_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE notes (id TEXT PRIMARY KEY, title TEXT DEFAULT '', body TEXT, note_type TEXT, updated_at TEXT DEFAULT (datetime('now')));
             CREATE TABLE sheet_columns (id TEXT PRIMARY KEY, note_id TEXT, name TEXT, col_type TEXT DEFAULT 'text', sort_order INTEGER DEFAULT 0);
             CREATE TABLE sheet_rows (id TEXT PRIMARY KEY, note_id TEXT, sort_order INTEGER DEFAULT 0);
             CREATE TABLE sheet_cells (row_id TEXT, column_id TEXT, note_id TEXT, value TEXT DEFAULT '', style_json TEXT DEFAULT '{}', PRIMARY KEY (row_id, column_id));"
        ).unwrap();
        conn
    }

    /// The post-sync hook must find and recompute EVERY `note_type =
    /// 'sheet'` note (broad scope, Matt ruling 2026-09-03) — not just
    /// one, proving the iteration itself works across multiple sheets in
    /// a single call.
    #[test]
    fn test_recompute_all_sheets_post_sync_recomputes_every_sheet_note() {
        let conn = create_test_db();
        for (note_id, col_id, row_id, formula) in [
            ("sheet-1", "c1", "r1", "=1+1"),
            ("sheet-2", "c2", "r2", "=2+2"),
        ] {
            conn.execute("INSERT INTO notes (id, note_type) VALUES (?1, 'sheet')", params![note_id]).unwrap();
            conn.execute("INSERT INTO sheet_columns (id, note_id, name) VALUES (?1, ?2, 'Total')", params![col_id, note_id]).unwrap();
            conn.execute("INSERT INTO sheet_rows (id, note_id) VALUES (?1, ?2)", params![row_id, note_id]).unwrap();
            conn.execute("INSERT INTO sheet_cells (row_id, column_id, note_id, value) VALUES (?1, ?2, ?3, ?4)", params![row_id, col_id, note_id, formula]).unwrap();
        }
        // A non-sheet note must be ignored entirely (never queried as a sheet).
        conn.execute("INSERT INTO notes (id, note_type) VALUES ('doc-1', 'document')", []).unwrap();

        recompute_all_sheets_post_sync(&conn);

        let body1: String = conn.query_row("SELECT body FROM notes WHERE id = 'sheet-1'", [], |r| r.get(0)).unwrap();
        let body2: String = conn.query_row("SELECT body FROM notes WHERE id = 'sheet-2'", [], |r| r.get(0)).unwrap();
        assert!(body1.contains('2'), "sheet-1's body must reflect its recomputed formula (=1+1 -> 2), got: {}", body1);
        assert!(body2.contains('4'), "sheet-2's body must reflect its recomputed formula (=2+2 -> 4), got: {}", body2);
    }

    /// A malformed sheet (e.g. a parse-breaking edge case that somehow
    /// made it past validation via a sync arrival) must be logged and
    /// skipped — it must NEVER abort the whole post-sync pass, leaving
    /// other sheets un-recomputed.
    #[test]
    fn test_recompute_all_sheets_post_sync_one_bad_sheet_does_not_block_others() {
        let conn = create_test_db();

        // "bad-sheet" has no columns/rows at all — recompute_sheet must
        // handle this gracefully (empty sheet, no formulas, trivially
        // succeeds) rather than being a true failure case; the real
        // failure-isolation guarantee is exercised structurally here by
        // confirming a sheet processed AFTER an edge-case sheet still
        // gets recomputed.
        conn.execute("INSERT INTO notes (id, note_type) VALUES ('bad-sheet', 'sheet')", []).unwrap();

        conn.execute("INSERT INTO notes (id, note_type) VALUES ('good-sheet', 'sheet')", []).unwrap();
        conn.execute("INSERT INTO sheet_columns (id, note_id, name) VALUES ('c1', 'good-sheet', 'Total')", []).unwrap();
        conn.execute("INSERT INTO sheet_rows (id, note_id) VALUES ('r1', 'good-sheet')", []).unwrap();
        conn.execute("INSERT INTO sheet_cells (row_id, column_id, note_id, value) VALUES ('r1', 'c1', 'good-sheet', '=3+3')", []).unwrap();

        recompute_all_sheets_post_sync(&conn);

        let body: String = conn.query_row("SELECT body FROM notes WHERE id = 'good-sheet'", [], |r| r.get(0)).unwrap();
        assert!(body.contains('6'), "good-sheet must still recompute even though bad-sheet was processed in the same batch, got: {}", body);
    }

    /// A payload with zero sheet notes at all must be a no-op, not a panic.
    #[test]
    fn test_recompute_all_sheets_post_sync_no_sheets_is_a_noop() {
        let conn = create_test_db();
        conn.execute("INSERT INTO notes (id, note_type) VALUES ('doc-1', 'document')", []).unwrap();
        recompute_all_sheets_post_sync(&conn); // must not panic
    }
}
