#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use std::path::PathBuf;

    fn get_extension_path() -> String {
        let paths = vec![
            PathBuf::from("resources/crsqlite.dylib"),
            PathBuf::from("src-tauri/resources/crsqlite.dylib"),
            PathBuf::from("../resources/crsqlite.dylib"),
        ];
        for p in paths {
            if p.exists() {
                return p.to_string_lossy().to_string();
            }
        }
        panic!("Could not find crsqlite.dylib");
    }

    fn setup_db(path: &str) -> Connection {
        let conn = Connection::open(path).expect("Failed to open database");
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();

        unsafe {
            conn.load_extension_enable().unwrap();
            conn.load_extension(get_extension_path(), Some("sqlite3_crsqlite_init")).unwrap();
        }
        conn.load_extension_disable().unwrap();

        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS test_sync (
                id TEXT PRIMARY KEY NOT NULL,
                title TEXT DEFAULT '',
                content TEXT DEFAULT '',
                updated_at TEXT DEFAULT ''
            );
        ").unwrap();

        conn.query_row("SELECT crsql_as_crr('test_sync')", [], |_| Ok(())).unwrap();
        conn
    }

    fn get_changes(conn: &Connection, since: i64) -> Vec<(String, String, String, Option<String>, i64, i64, Vec<u8>, i64, i64)> {
        let mut stmt = conn.prepare(
            "SELECT \"table\", pk, cid, val, col_version, db_version, site_id, cl, seq 
             FROM crsql_changes WHERE db_version > ?"
        ).unwrap();

        stmt.query_map([since], |row| {
            let pk_val: rusqlite::types::Value = row.get(1)?;
            let pk = match pk_val {
                rusqlite::types::Value::Text(s) => s,
                rusqlite::types::Value::Blob(b) => String::from_utf8(b).unwrap_or_default(),
                other => format!("{:?}", other),
            };
            let val_raw: rusqlite::types::Value = row.get(3)?;
            let val = match val_raw {
                rusqlite::types::Value::Null => None,
                rusqlite::types::Value::Text(s) => Some(s),
                rusqlite::types::Value::Integer(i) => Some(i.to_string()),
                rusqlite::types::Value::Real(f) => Some(f.to_string()),
                rusqlite::types::Value::Blob(b) => Some(String::from_utf8(b).unwrap_or_default()),
            };
            Ok((
                row.get::<_, String>(0)?,
                pk,
                row.get::<_, String>(2)?,
                val,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Vec<u8>>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
            ))
        }).unwrap().map(|r| r.unwrap()).collect()
    }

    fn apply_changes(conn: &Connection, changes: &[(String, String, String, Option<String>, i64, i64, Vec<u8>, i64, i64)]) {
        for c in changes {
            conn.execute(
                "INSERT INTO crsql_changes (\"table\", pk, cid, val, col_version, db_version, site_id, cl, seq) 
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![c.0, c.1, c.2, c.3, c.4, c.5, c.6, c.7, c.8],
            ).unwrap();
        }
    }

    #[test]
    fn test_bidirectional_sync() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let db1_path = dir.path().join("db1.sqlite");
        let db2_path = dir.path().join("db2.sqlite");

        let db1 = setup_db(db1_path.to_str().unwrap());
        let db2 = setup_db(db2_path.to_str().unwrap());

        // Insert different data in each
        db1.execute(
            "INSERT INTO test_sync (id, title, content, updated_at) VALUES ('a1', 'Note A', 'Content from DB1', '2024-01-01')",
            [],
        ).unwrap();

        db2.execute(
            "INSERT INTO test_sync (id, title, content, updated_at) VALUES ('b1', 'Note B', 'Content from DB2', '2024-01-01')",
            [],
        ).unwrap();

        // Get changes from both
        let changes_from_1 = get_changes(&db1, 0);
        let changes_from_2 = get_changes(&db2, 0);

        assert!(!changes_from_1.is_empty(), "DB1 should have changes");
        assert!(!changes_from_2.is_empty(), "DB2 should have changes");

        // Exchange changesets
        apply_changes(&db2, &changes_from_1);
        apply_changes(&db1, &changes_from_2);

        // Verify both have merged data
        let count1: i64 = db1.query_row("SELECT COUNT(*) FROM test_sync", [], |r| r.get(0)).unwrap();
        let count2: i64 = db2.query_row("SELECT COUNT(*) FROM test_sync", [], |r| r.get(0)).unwrap();

        assert_eq!(count1, 2, "DB1 should have 2 rows after sync");
        assert_eq!(count2, 2, "DB2 should have 2 rows after sync");

        // Verify specific data
        let title_a_in_2: String = db2.query_row(
            "SELECT title FROM test_sync WHERE id = 'a1'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(title_a_in_2, "Note A");

        let title_b_in_1: String = db1.query_row(
            "SELECT title FROM test_sync WHERE id = 'b1'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(title_b_in_1, "Note B");

        // Clean up cr-sqlite before dropping
        let _ = db1.execute("SELECT crsql_finalize()", []);
        let _ = db2.execute("SELECT crsql_finalize()", []);
    }

    /// #2339 PR 5a, Deliverable 6 (required item 1): `sheet_cells` is the
    /// first CRR table in the codebase with a COMPOSITE primary key
    /// (row_id, column_id) *and* a non-PK payload column (note_id, value,
    /// style_json, updated_at). Every other CRR table so far (test_sync
    /// above, `notes`, `subtasks`, etc.) has a single-column PK — cr-sqlite's
    /// LWW conflict resolution is documented to work per-column regardless
    /// of PK shape, but that's never been *proven* against a composite-PK
    /// table in this codebase until now.
    ///
    /// Shape: two DBs sync an initial identical `sheet_cells` row (same
    /// composite PK). Both then diverge on the SAME non-PK column
    /// (`note_id`) with different values, DB2's write timestamped later.
    /// After bidirectional sync, both DBs must converge to DB2's `note_id`
    /// value (later write wins) while the PK identity (row_id, column_id)
    /// stays a SINGLE row, not duplicated/split — proving LWW resolves
    /// per-column on this table independent of the composite PK, not via
    /// some PK-triggered full-row semantics.
    #[test]
    fn test_sheet_cells_composite_pk_lww_resolves_non_pk_column() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let db1_path = dir.path().join("sheet_lww_db1.sqlite");
        let db2_path = dir.path().join("sheet_lww_db2.sqlite");

        let db1 = setup_sheet_cells_db(db1_path.to_str().unwrap());
        let db2 = setup_sheet_cells_db(db2_path.to_str().unwrap());

        // Seed an identical sheet_cells row (same composite PK) on db1, sync to db2
        // so both start from the exact same baseline row.
        db1.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id, value, style_json, updated_at)
             VALUES ('row-1', 'col-1', 'note-original', 'hello', '{}', '2024-01-01T00:00:00Z')",
            [],
        ).unwrap();

        let seed_changes = get_changes(&db1, 0);
        assert!(!seed_changes.is_empty(), "seed insert must produce changefeed entries");
        apply_changes(&db2, &seed_changes);

        // db1 writes note_id first and its change is synced to db2 BEFORE db2
        // makes its own edit. This gives db2's subsequent edit a strictly
        // higher cr-sqlite col_version for the `note_id` column (its local
        // version already reflects db1's write), so the later write wins
        // unambiguously on col_version comparison alone — no site_id tie-break
        // needed, keeping the assertion deterministic rather than relying on
        // cr-sqlite's (unspecified-here) tie-break ordering.
        db1.execute(
            "UPDATE sheet_cells SET note_id = 'note-from-db1' WHERE row_id = 'row-1' AND column_id = 'col-1'",
            [],
        ).unwrap();
        let db1_update_changes = get_changes(&db1, 0);
        apply_changes(&db2, &db1_update_changes);

        let db2_version_before_own_edit: i64 = db2.query_row(
            "SELECT COALESCE(MAX(db_version), 0) FROM crsql_changes", [], |r| r.get(0)
        ).unwrap();

        // Now db2 makes the LATER edit, on the SAME non-PK column, for the
        // SAME composite-PK row.
        db2.execute(
            "UPDATE sheet_cells SET note_id = 'note-from-db2-later' WHERE row_id = 'row-1' AND column_id = 'col-1'",
            [],
        ).unwrap();
        let db2_update_changes = get_changes(&db2, db2_version_before_own_edit);
        apply_changes(&db1, &db2_update_changes);

        // Convergence assertion 1: exactly ONE row for this composite PK on
        // each side — no duplication/fan-out from the composite-PK conflict.
        let row_count_db1: i64 = db1.query_row(
            "SELECT COUNT(*) FROM sheet_cells WHERE row_id = 'row-1' AND column_id = 'col-1'", [], |r| r.get(0)
        ).unwrap();
        let row_count_db2: i64 = db2.query_row(
            "SELECT COUNT(*) FROM sheet_cells WHERE row_id = 'row-1' AND column_id = 'col-1'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(row_count_db1, 1, "composite-PK row must not duplicate on db1 after LWW resolution");
        assert_eq!(row_count_db2, 1, "composite-PK row must not duplicate on db2 after LWW resolution");

        // Convergence assertion 2: both sides agree on the SAME note_id
        // value post-sync (LWW picked one winner consistently on both sides).
        let note_id_db1: String = db1.query_row(
            "SELECT note_id FROM sheet_cells WHERE row_id = 'row-1' AND column_id = 'col-1'", [], |r| r.get(0)
        ).unwrap();
        let note_id_db2: String = db2.query_row(
            "SELECT note_id FROM sheet_cells WHERE row_id = 'row-1' AND column_id = 'col-1'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(note_id_db1, note_id_db2, "both sides must converge to the same note_id value post-sync");

        // Convergence assertion 3: the WINNING value is the later write
        // (db2's), proving LWW — not "first write wins" or "PK-order wins" —
        // resolved the conflict on this non-PK column.
        assert_eq!(note_id_db1, "note-from-db2-later", "LWW must resolve to the later write's non-PK column value");

        // Convergence assertion 4: the OTHER non-PK columns on the same row
        // (value, style_json — untouched by either side's UPDATE) survived
        // the conflict resolution unchanged, proving resolution is truly
        // per-column, not "last writer's full row wins".
        let value_db1: String = db1.query_row(
            "SELECT value FROM sheet_cells WHERE row_id = 'row-1' AND column_id = 'col-1'", [], |r| r.get(0)
        ).unwrap();
        assert_eq!(value_db1, "hello", "untouched non-PK column (value) must survive LWW resolution on a different column");

        let _ = db1.execute("SELECT crsql_finalize()", []);
        let _ = db2.execute("SELECT crsql_finalize()", []);
    }

    /// Real `sheet_cells` DDL (migrations.rs `create_sheet_tables`) —
    /// composite PK (row_id, column_id), denormalized non-PK `note_id`,
    /// registered as a CRR table via `crsql_as_crr`, mirroring
    /// `setup_db`'s pattern above exactly.
    fn setup_sheet_cells_db(path: &str) -> Connection {
        let conn = Connection::open(path).expect("Failed to open database");
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();

        unsafe {
            conn.load_extension_enable().unwrap();
            conn.load_extension(get_extension_path(), Some("sqlite3_crsqlite_init")).unwrap();
        }
        conn.load_extension_disable().unwrap();

        conn.execute_batch("
            CREATE TABLE IF NOT EXISTS sheet_cells (
                row_id     TEXT NOT NULL,
                column_id  TEXT NOT NULL,
                note_id    TEXT NOT NULL DEFAULT '',
                value      TEXT NOT NULL DEFAULT '',
                style_json TEXT NOT NULL DEFAULT '{}',
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (row_id, column_id)
            );
        ").unwrap();

        conn.query_row("SELECT crsql_as_crr('sheet_cells')", [], |_| Ok(())).unwrap();
        conn
    }

    /// #2339 PR 7, Deliverable 6: a full `notes` + `sheet_columns` +
    /// `sheet_rows` + `sheet_cells` schema, all four registered as CRR
    /// tables, so a real sync payload can carry a sheet note plus its
    /// grid data end-to-end — building on `setup_sheet_cells_db`'s
    /// pattern but adding the parent tables `recompute::recompute_sheet`
    /// actually needs to load a sheet (it queries `notes`, `sheet_columns`,
    /// `sheet_rows`, `sheet_cells` directly, matching production schema
    /// shape from `migrations.rs`'s `create_notes_table`/`migrate_v90`).
    fn setup_sheet_recompute_db(path: &str) -> Connection {
        let conn = Connection::open(path).expect("Failed to open database");
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();

        unsafe {
            conn.load_extension_enable().unwrap();
            conn.load_extension(get_extension_path(), Some("sqlite3_crsqlite_init")).unwrap();
        }
        conn.load_extension_disable().unwrap();

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS notes (
                id              TEXT NOT NULL PRIMARY KEY,
                title           TEXT NOT NULL DEFAULT '',
                body            TEXT,
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

            CREATE TABLE IF NOT EXISTS sheet_columns (
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

            CREATE TABLE IF NOT EXISTS sheet_rows (
                id           TEXT NOT NULL PRIMARY KEY,
                note_id      TEXT NOT NULL DEFAULT '',
                sort_order   INTEGER NOT NULL DEFAULT 0,
                height       INTEGER NOT NULL DEFAULT 0,
                created_at   TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS sheet_cells (
                row_id     TEXT NOT NULL,
                column_id  TEXT NOT NULL,
                note_id    TEXT NOT NULL DEFAULT '',
                value      TEXT NOT NULL DEFAULT '',
                style_json TEXT NOT NULL DEFAULT '{}',
                updated_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (row_id, column_id)
            );
            
"
        )
        .unwrap();

        let crr_available = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM pragma_function_list WHERE name='crsql_as_crr'",
                [],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false);
        if crr_available {
            for table in ["notes", "sheet_columns", "sheet_rows", "sheet_cells"] {
                conn.query_row(&format!("SELECT crsql_as_crr('{}');", table), [], |_| Ok(()))
                    .unwrap_or_else(|e| panic!("failed to enable CRR on {}: {}", table, e));
            }
        }

        conn
    }

    /// #2339 PR 7, Deliverable 6: the scope report's LWW-race scenario.
    /// A formula cell that LOSES a sync race must recompute from the
    /// WINNING formula string once the loser observes the sync result —
    /// never from its own now-stale pre-sync formula. This is the
    /// concrete regression the recompute-before-body-regen ordering fix
    /// and the post-sync recompute hook (`sync/engine.rs`) both exist to
    /// guarantee.
    ///
    /// Shape: DB A writes `=1+1` to a cell and syncs it to DB B first.
    /// DB B then writes `=2+2` to the SAME cell (a later, conflicting
    /// write — the LWW winner). DB B's change is synced back to DB A.
    /// After DB A applies the incoming changeset, `recompute::
    /// recompute_sheet` is invoked directly on DB A's connection
    /// (standing in for the post-sync recompute hook's real call, since
    /// driving the full `sync::engine::receive_sync_payload` path here
    /// would require unrelated encryption/keychain setup this test
    /// harness doesn't have) — DB A's computed value MUST be 4 (from
    /// `=2+2`), never 2 (stale, from its own pre-sync `=1+1`).
    #[test]
    fn test_lww_race_loser_recomputes_from_winning_formula() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let db_a_path = dir.path().join("lww_race_a.sqlite");
        let db_b_path = dir.path().join("lww_race_b.sqlite");

        let db_a = setup_sheet_recompute_db(db_a_path.to_str().unwrap());
        let db_b = setup_sheet_recompute_db(db_b_path.to_str().unwrap());

        let note_id = "sheet-1";
        let col_id = "col-1";
        let row_id = "row-1";

        // Seed an identical sheet skeleton (note + column + row) on A,
        // sync it to B, so both sides start from the same baseline
        // structure before either writes a formula.
        db_a.execute(
            "INSERT INTO notes (id, title, note_type) VALUES (?1, 'LWW Race Sheet', 'sheet')",
            rusqlite::params![note_id],
        ).unwrap();
        db_a.execute(
            "INSERT INTO sheet_columns (id, note_id, name, col_type, sort_order) VALUES (?1, ?2, 'Total', 'number', 0)",
            rusqlite::params![col_id, note_id],
        ).unwrap();
        db_a.execute(
            "INSERT INTO sheet_rows (id, note_id, sort_order) VALUES (?1, ?2, 0)",
            rusqlite::params![row_id, note_id],
        ).unwrap();

        let seed_changes = get_changes(&db_a, 0);
        assert!(!seed_changes.is_empty(), "seed skeleton insert must produce changefeed entries");
        apply_changes(&db_b, &seed_changes);

        // DB A writes the LOSING formula first, and it's synced to B
        // before B makes its own edit — mirroring the LWW composite-PK
        // test's approach above, this gives B's subsequent edit a
        // strictly higher col_version, making the later-write-wins
        // outcome deterministic without relying on a site_id tie-break.
        db_a.execute(
            "INSERT INTO sheet_cells (row_id, column_id, note_id, value) VALUES (?1, ?2, ?3, '=1+1')",
            rusqlite::params![row_id, col_id, note_id],
        ).unwrap();
        let db_a_first_write = get_changes(&db_a, 0);
        apply_changes(&db_b, &db_a_first_write);

        let db_b_version_before_own_edit: i64 = db_b.query_row(
            "SELECT COALESCE(MAX(db_version), 0) FROM crsql_changes", [], |r| r.get(0)
        ).unwrap();

        // DB B makes the LATER, WINNING edit on the same cell.
        db_b.execute(
            "UPDATE sheet_cells SET value = '=2+2' WHERE row_id = ?1 AND column_id = ?2",
            rusqlite::params![row_id, col_id],
        ).unwrap();
        let db_b_winning_write = get_changes(&db_b, db_b_version_before_own_edit);
        apply_changes(&db_a, &db_b_winning_write);

        // Sanity check: DB A's raw stored value must now reflect B's
        // winning write (proves the sync/LWW mechanics themselves worked,
        // independent of recompute).
        let stored_value: String = db_a.query_row(
            "SELECT value FROM sheet_cells WHERE row_id = ?1 AND column_id = ?2",
            rusqlite::params![row_id, col_id],
            |r| r.get(0),
        ).unwrap();
        assert_eq!(stored_value, "=2+2", "DB A's stored formula must be the LWW winner after sync, not its own stale write");

        // The regression assertion: recomputing on DB A (standing in for
        // the post-sync recompute hook) must produce 4.0 — the WINNING
        // formula's value — never 2.0 (DB A's own pre-sync formula's
        // value), proving recompute reads the post-sync stored value,
        // not some pre-sync cached/stale state.
        let computed = crate::formula::recompute::recompute_sheet(&db_a, note_id)
            .expect("recompute must succeed on a valid post-sync sheet");
        let cell_value = computed.get(&(col_id.to_string(), row_id.to_string()))
            .expect("the synced formula cell must have a computed entry");
        match cell_value {
            crate::formula::CellValue::Number(n) => {
                assert_eq!(*n, 4.0, "post-sync recompute must use the LWW-winning formula (=2+2 -> 4), not the loser's stale formula (=1+1 -> 2)");
            }
            other => panic!("expected a numeric computed value of 4.0, got: {:?}", other),
        }

        let _ = db_a.execute("SELECT crsql_finalize()", []);
        let _ = db_b.execute("SELECT crsql_finalize()", []);
    }
}
