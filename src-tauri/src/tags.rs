// Pane v9.0.0 — Tag Management Commands
// Implements tag operations (§17.3)

use crate::database::{DbState, TagInfo};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

// === Request Types ===

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTagRequest {
    pub name: String,
    pub color: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdateTagRequest {
    pub tag_id: String,
    pub name: Option<String>,
    pub color: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AddNoteTagsRequest {
    pub note_id: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RemoveNoteTagsRequest {
    pub note_id: String,
    pub tags: Vec<String>,
}

// === Tauri Commands ===

#[tauri::command]
pub fn get_tags(db: State<DbState>) -> Result<Vec<TagInfo>, String> {
    let conn = db.0.lock().unwrap();
    
    let mut stmt = conn.prepare(
        "SELECT id, name, color FROM tags ORDER BY name"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    let tags = stmt.query_map([], |row| {
        Ok(TagInfo {
            id: row.get(0)?,
            name: row.get(1)?,
            color: row.get(2)?,
        })
    })
    .map_err(|e| format!("Failed to query tags: {}", e))?
    .collect::<Result<Vec<_>, _>>()
    .map_err(|e| format!("Failed to collect tags: {}", e))?;
    
    Ok(tags)
}

#[tauri::command]
pub fn create_tag(
    request: CreateTagRequest,
    db: State<DbState>,
) -> Result<TagInfo, String> {
    let conn = db.0.lock().unwrap();
    
    let color = request.color.unwrap_or_else(|| "#6B7280".to_string());
    let id = uuid::Uuid::new_v4().to_string();
    
    // Use INSERT OR IGNORE to handle duplicate tag names gracefully
    let rows = conn.execute(
        "INSERT OR IGNORE INTO tags (id, name, color) VALUES (?1, ?2, ?3)",
        params![id, request.name, color],
    ).map_err(|e| format!("Failed to create tag: {}", e))?;
    
    // If insert was ignored (tag exists), fetch the existing one
    if rows == 0 {
        return conn.query_row(
            "SELECT id, name, color FROM tags WHERE name = ?1",
            params![request.name],
            |row| Ok(TagInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
            }),
        ).map_err(|e| format!("Failed to fetch existing tag: {}", e));
    }
    
    Ok(TagInfo {
        id,
        name: request.name,
        color,
    })
}

#[tauri::command]
pub fn update_tag(
    request: UpdateTagRequest,
    db: State<DbState>,
) -> Result<TagInfo, String> {
    let conn = db.0.lock().unwrap();
    
    let mut updates = Vec::new();
    let mut update_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    
    if let Some(name) = &request.name {
        updates.push("name = ?");
        update_params.push(Box::new(name.clone()));
    }
    
    if let Some(color) = &request.color {
        updates.push("color = ?");
        update_params.push(Box::new(color.clone()));
    }
    
    if !updates.is_empty() {
        let query = format!("UPDATE tags SET {} WHERE id = ?", updates.join(", "));
        update_params.push(Box::new(request.tag_id.clone()));
        
        let params_refs: Vec<&dyn rusqlite::ToSql> = update_params.iter().map(|b| b.as_ref() as &dyn rusqlite::ToSql).collect();
        
        conn.execute(&query, &params_refs[..])
            .map_err(|e| format!("Failed to update tag: {}", e))?;
    }
    
    get_tag(&conn, &request.tag_id)
}

#[tauri::command]
pub fn delete_tag(tag_id: String, db: State<DbState>) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    
    // Delete will cascade to note_tags
    conn.execute("DELETE FROM tags WHERE id = ?1", params![tag_id])
        .map_err(|e| format!("Failed to delete tag: {}", e))?;
    
    Ok(())
}

#[tauri::command]
pub fn add_note_tags(
    request: AddNoteTagsRequest,
    db: State<DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    
    for tag_name in &request.tags {
        // Find or create the tag by name
        let tag_id: String = match conn.query_row(
            "SELECT id FROM tags WHERE name = ?1",
            params![tag_name],
            |row| row.get(0),
        ).optional().map_err(|e| format!("Failed to find tag: {}", e))? {
            Some(id) => id,
            None => {
                let new_id = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO tags (id, name, color) VALUES (?1, ?2, '#6B7280')",
                    params![new_id, tag_name],
                ).map_err(|e| format!("Failed to create tag: {}", e))?;
                new_id
            }
        };
        
        conn.execute(
            "INSERT OR IGNORE INTO note_tags (note_id, tag_id) VALUES (?1, ?2)",
            params![request.note_id, tag_id],
        ).map_err(|e| format!("Failed to add tag: {}", e))?;
    }
    
    // Update denormalized tags_json in notes table
    update_note_tags_json(&conn, &request.note_id)?;
    
    Ok(())
}

#[tauri::command]
pub fn remove_note_tags(
    request: RemoveNoteTagsRequest,
    db: State<DbState>,
) -> Result<(), String> {
    let conn = db.0.lock().unwrap();
    
    for tag_name in &request.tags {
        // Find tag by name
        if let Some(tag_id) = conn.query_row(
            "SELECT id FROM tags WHERE name = ?1",
            params![tag_name],
            |row| row.get::<_, String>(0),
        ).optional().map_err(|e| format!("Failed to find tag: {}", e))? {
            conn.execute(
                "DELETE FROM note_tags WHERE note_id = ?1 AND tag_id = ?2",
                params![request.note_id, tag_id],
            ).map_err(|e| format!("Failed to remove tag: {}", e))?;
        }
    }
    
    // Update denormalized tags_json in notes table
    update_note_tags_json(&conn, &request.note_id)?;
    
    Ok(())
}

#[tauri::command]
pub fn get_notes_by_tag(tag_name: String, db: State<DbState>) -> Result<Vec<String>, String> {
    let conn = db.0.lock().unwrap();
    
    let mut stmt = conn.prepare(
        "SELECT nt.note_id FROM note_tags nt JOIN tags t ON nt.tag_id = t.id WHERE t.name = ?1"
    ).map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    let note_ids = stmt.query_map([&tag_name], |row| row.get(0))
        .map_err(|e| format!("Failed to query notes: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect note IDs: {}", e))?;
    
    Ok(note_ids)
}

// === Helper Functions ===

fn get_tag(conn: &rusqlite::Connection, tag_id: &str) -> Result<TagInfo, String> {
    conn.query_row(
        "SELECT id, name, color FROM tags WHERE id = ?1",
        params![tag_id],
        |row| {
            Ok(TagInfo {
                id: row.get(0)?,
                name: row.get(1)?,
                color: row.get(2)?,
            })
        },
    ).optional()
    .map_err(|e| format!("Failed to query tag: {}", e))?
    .ok_or_else(|| "Tag not found".to_string())
}

fn update_note_tags_json(conn: &rusqlite::Connection, note_id: &str) -> Result<(), String> {
    let mut stmt = conn.prepare(
        "SELECT t.name FROM tags t JOIN note_tags nt ON t.id = nt.tag_id WHERE nt.note_id = ?1 ORDER BY t.name"
    ).map_err(|e| format!("Failed to prepare tags query: {}", e))?;
    
    let tag_names: Vec<String> = stmt.query_map([note_id], |row| row.get(0))
        .map_err(|e| format!("Failed to query tag names: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect tag names: {}", e))?;
    
    let tags_json = serde_json::to_string(&tag_names)
        .map_err(|e| format!("Failed to serialize tags: {}", e))?;
    
    conn.execute(
        "UPDATE notes SET tags_json = ?1 WHERE id = ?2",
        params![tags_json, note_id],
    ).map_err(|e| format!("Failed to update tags_json: {}", e))?;
    
    Ok(())
}
