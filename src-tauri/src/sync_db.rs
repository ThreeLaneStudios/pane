use rusqlite::{Connection, OptionalExtension};
use super::sync::SyncChange;

// --- Sync operations ---

/// Get sync changes since a given version
pub fn get_sync_changes(conn: &Connection, since_version: i64) -> Result<Vec<SyncChange>, String> {
    let mut stmt = conn.prepare(
        "SELECT \"table\", pk, cid, val, col_version, db_version, site_id, cl, seq 
         FROM crsql_changes WHERE db_version > ?"
    ).map_err(|e| format!("Failed to prepare sync changes query: {}", e))?;

    let changes = stmt.query_map([since_version], |row| {
        // pk is always a blob in cr-sqlite — read as raw bytes
        let pk: Vec<u8> = row.get(1)?;
        
        // val can be various types — read as raw Value and convert
        let val_value: rusqlite::types::Value = row.get(3)?;
        let val = match val_value {
            rusqlite::types::Value::Text(s) => Some(s),
            rusqlite::types::Value::Integer(i) => Some(i.to_string()),
            rusqlite::types::Value::Real(f) => Some(f.to_string()),
            rusqlite::types::Value::Blob(b) => {
                Some(String::from_utf8(b.clone()).unwrap_or_else(|_| {
                    b.iter().map(|byte| format!("{:02x}", byte)).collect::<String>()
                }))
            },
            rusqlite::types::Value::Null => None,
        };

        Ok(SyncChange {
            table_name: row.get(0)?,
            pk,
            cid: row.get(2)?,
            val,
            col_version: row.get(4)?,
            db_version: row.get(5)?,
            site_id: row.get(6)?,
            cl: row.get(7)?,
            seq: row.get(8)?,
        })
    }).map_err(|e| format!("Failed to execute sync changes query: {}", e))?;

    let mut result = Vec::new();
    for change in changes {
        result.push(change.map_err(|e| format!("Failed to read sync change: {}", e))?);
    }

    Ok(result)
}

/// Apply sync changes from a remote peer
pub fn apply_sync_changes(conn: &Connection, changes: Vec<SyncChange>) -> Result<(u64,), String> {
    let mut conflict_count: u64 = 0;
    let my_site_id: Vec<u8> = conn.query_row("SELECT crsql_site_id()", [], |row| row.get(0))
        .map_err(|e| format!("Failed to get site_id: {}", e))?;
    
    for change in &changes {
        // Only a real conflict if we have a LOCAL edit to the same field with a DIFFERENT col_version
        // (meaning both sides independently modified the same field)
        let is_conflict: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM crsql_changes WHERE \"table\" = ? AND pk = ? AND cid = ? AND site_id = ? AND col_version != ?",
            [&change.table_name as &dyn rusqlite::ToSql, &change.pk, &change.cid, &my_site_id, &change.col_version],
            |row| row.get(0),
        ).unwrap_or(false);
        
        if is_conflict {
            conflict_count += 1;
            println!("[SYNC] Conflict detected: {}.{} (last-write-wins)", change.table_name, change.cid);
        }
        
        conn.execute(
            "INSERT INTO crsql_changes (\"table\", pk, cid, val, col_version, db_version, site_id, cl, seq) 
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            [
                &change.table_name as &dyn rusqlite::ToSql,
                &change.pk,
                &change.cid,
                &change.val,
                &change.col_version,
                &change.db_version,
                &change.site_id,
                &change.cl,
                &change.seq,
            ],
        ).map_err(|e| format!("Failed to apply sync change: {}", e))?;
    }

    Ok((conflict_count,))
}

/// Get current database version for sync
pub fn get_sync_version(conn: &Connection) -> Result<i64, String> {
    conn.query_row("SELECT crsql_db_version()", [], |row| row.get(0))
        .map_err(|e| format!("Failed to get sync version: {}", e))
}

/// Get current site ID for sync
pub fn get_site_id(conn: &Connection) -> Result<Vec<u8>, String> {
    conn.query_row("SELECT crsql_site_id()", [], |row| row.get(0))
        .map_err(|e| format!("Failed to get site ID: {}", e))
}

/// Check for sync conflicts by comparing versions
/// This is a simplified conflict detection - in reality cr-sqlite handles conflicts automatically
/// but we want to track when they occur for UI feedback
pub fn detect_conflicts(conn: &Connection, changes: &[SyncChange]) -> Result<u64, String> {
    let mut conflict_count = 0;
    
    for change in changes {
        // Check if there's a local change to the same row/column that's newer
        // This is a heuristic since cr-sqlite handles the actual conflict resolution
        let local_version: Option<i64> = conn.query_row(
            "SELECT col_version FROM crsql_changes WHERE \"table\" = ? AND pk = ? AND cid = ? ORDER BY col_version DESC LIMIT 1",
            [&change.table_name as &dyn rusqlite::ToSql, &change.pk, &change.cid],
            |row| row.get(0)
        ).optional()
        .map_err(|e| format!("Failed to check for conflicts: {}", e))?;
        
        if let Some(local_ver) = local_version {
            // If local version exists and is different, it might be a conflict
            if local_ver != change.col_version {
                conflict_count += 1;
            }
        }
    }
    
    Ok(conflict_count)
}