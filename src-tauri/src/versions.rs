// Pane v11.10.13 — Version History System
// Provides version tracking for notes to enable undo/history and protection

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NoteVersion {
    pub id: String,
    pub note_id: String,
    pub content: String,
    pub created_at: String,
    pub source: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NoteVersionMeta {
    pub id: String,
    pub note_id: String,
    pub created_at: String,
    pub source: String,
}

/// Save a version of a note's content. Prunes to max 50 versions per note.
pub fn save_version(conn: &Connection, note_id: &str, content: &str, source: &str) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    
    conn.execute(
        "INSERT INTO note_versions (id, note_id, content, created_at, source) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, note_id, content, now, source],
    ).map_err(|e| format!("Failed to save version: {}", e))?;
    
    // Prune to 50 versions per note (delete oldest)
    conn.execute(
        "DELETE FROM note_versions WHERE note_id = ?1 AND id NOT IN (
            SELECT id FROM note_versions WHERE note_id = ?1 ORDER BY created_at DESC LIMIT 50
        )",
        params![note_id],
    ).ok();
    
    Ok(id)
}

/// Get version metadata (without content) for a note, newest first.
pub fn get_versions(conn: &Connection, note_id: &str) -> Result<Vec<NoteVersionMeta>, String> {
    let mut stmt = conn.prepare(
        "SELECT id, note_id, created_at, source FROM note_versions WHERE note_id = ?1 ORDER BY created_at DESC"
    ).map_err(|e| format!("Failed to prepare: {}", e))?;
    
    let versions = stmt.query_map(params![note_id], |row| {
        Ok(NoteVersionMeta {
            id: row.get(0)?,
            note_id: row.get(1)?,
            created_at: row.get(2)?,
            source: row.get(3)?,
        })
    }).map_err(|e| format!("Failed to query: {}", e))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("Failed to collect: {}", e))?;
    
    Ok(versions)
}

/// Get a single version with full content.
pub fn get_version(conn: &Connection, version_id: &str) -> Result<NoteVersion, String> {
    conn.query_row(
        "SELECT id, note_id, content, created_at, source FROM note_versions WHERE id = ?1",
        params![version_id],
        |row| Ok(NoteVersion {
            id: row.get(0)?,
            note_id: row.get(1)?,
            content: row.get(2)?,
            created_at: row.get(3)?,
            source: row.get(4)?,
        }),
    ).map_err(|e| format!("Version not found: {}", e))
}

/// Restore a version: saves current content as a version, then replaces note content.
pub fn restore_version(conn: &Connection, note_id: &str, version_id: &str) -> Result<(), String> {
    // Get note metadata
    let (file_path, body, note_type): (String, Option<String>, Option<String>) = conn.query_row(
        "SELECT file_path, body, note_type FROM notes WHERE id = ?1",
        params![note_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).map_err(|e| format!("Note not found: {}", e))?;
    
    // Check if system note
    let is_system = note_type.as_deref().map_or(false, |nt| 
        matches!(nt, "soul" | "memory" | "rules" | "identity" | "user" | "tools" | "agents" | "heartbeat" | "daily")
    );
    
    // Read current content: DB body for regular notes, file for system notes
    let current_content = if is_system {
        crate::database::read_note_content(&file_path)?
    } else {
        body.unwrap_or_else(|| crate::database::read_note_content(&file_path).unwrap_or_default())
    };
    
    // Save current as a version before restoring
    save_version(conn, note_id, &current_content, "pre_restore")?;
    
    // Get the version to restore
    let version = get_version(conn, version_id)?;
    
    // Write restored content to file
    crate::database::write_note_content(&file_path, &version.content, Some(conn))?;
    
    // Update note metadata (and body for regular notes)
    let word_count = version.content.split_whitespace().count() as i64;
    let content_preview = version.content.chars().take(200).collect::<String>();
    let now = chrono::Utc::now().to_rfc3339();
    
    if is_system {
        // System notes: file-only (body stays NULL)
        conn.execute(
            "UPDATE notes SET content_preview = ?1, word_count = ?2, updated_at = ?3 WHERE id = ?4",
            params![content_preview, word_count, now, note_id],
        ).map_err(|e| format!("Failed to update note: {}", e))?;
    } else {
        // Regular notes: update body + metadata
        conn.execute(
            "UPDATE notes SET body = ?1, content_preview = ?2, word_count = ?3, updated_at = ?4 WHERE id = ?5",
            params![&version.content, content_preview, word_count, now, note_id],
        ).map_err(|e| format!("Failed to update note: {}", e))?;
    }
    
    Ok(())
}

// === Tauri Commands ===

use crate::database::DbState;
use tauri::State;

#[tauri::command]
pub fn get_note_versions(note_id: String, db: State<DbState>) -> Result<Vec<NoteVersionMeta>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    get_versions(&conn, &note_id)
}

#[tauri::command]
pub fn get_note_version(version_id: String, db: State<DbState>) -> Result<NoteVersion, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    get_version(&conn, &version_id)
}

#[tauri::command]
pub fn restore_note_version(note_id: String, version_id: String, db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    restore_version(&conn, &note_id, &version_id)
}
