// Pane v11.29.0 — Context Pins (§471)
// Pinned notes backend — replaces reference_files.rs

use crate::database::{DbState, read_note_content};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;

// === Types ===

#[derive(Debug, Serialize, Deserialize)]
pub struct ContextPin {
    pub id: String,
    pub title: String,
    pub content_preview: String,
    pub pinned_at: String,
    pub estimated_tokens: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ContextBudgetUsage {
    pub system_files_tokens: usize,
    pub project_brief_tokens: usize,
    pub pinned_notes_tokens: usize,
    pub pinned_notes_count: usize,
    pub total_used: usize,
    pub total_budget: usize,
    pub warning: bool,
}

// === Internal Functions ===

/// Pin a note to context for a project
pub fn pin_note_to_context_internal(
    note_id: &str,
    db: &DbState,
) -> Result<(), String> {
    eprintln!("[pin_note_to_context] Pinning note: {}", note_id);
    
    let conn = db.0.lock().unwrap();
    
    // Verify note exists and is not deleted
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM notes WHERE id = ?1 AND deleted_at IS NULL",
            params![note_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Note not found: {}", e))?;
    
    if !exists {
        return Err(format!("Note {} not found or deleted", note_id));
    }
    
    // Insert into note_context_pins (ON CONFLICT = already pinned, no-op)
    let now = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO note_context_pins (note_id, pinned_at, pinned_by)
         VALUES (?1, ?2, 'user')",
        params![note_id, now],
    )
    .map_err(|e| format!("Failed to pin note: {}", e))?;
    
    eprintln!("[pin_note_to_context] Note pinned successfully");
    Ok(())
}

/// Unpin a note from context
pub fn unpin_note_from_context_internal(
    note_id: &str,
    db: &DbState,
) -> Result<(), String> {
    eprintln!("[unpin_note_from_context] Unpinning note: {}", note_id);
    
    let conn = db.0.lock().unwrap();
    
    let deleted = conn.execute(
        "DELETE FROM note_context_pins WHERE note_id = ?1",
        params![note_id],
    )
    .map_err(|e| format!("Failed to unpin note: {}", e))?;
    
    if deleted == 0 {
        eprintln!("[unpin_note_from_context] Note was not pinned");
    } else {
        eprintln!("[unpin_note_from_context] Note unpinned successfully");
    }
    
    Ok(())
}

/// Check if a note is pinned
pub fn is_note_pinned_internal(
    note_id: &str,
    db: &DbState,
) -> Result<bool, String> {
    let conn = db.0.lock().unwrap();
    
    let pinned: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM note_context_pins WHERE note_id = ?1",
            params![note_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to check pin status: {}", e))?;
    
    Ok(pinned)
}

/// List all pinned notes for a project
pub fn list_context_pins_internal(
    project_id: &str,
    db: &DbState,
) -> Result<Vec<ContextPin>, String> {
    eprintln!("[list_context_pins] Listing for project: {}", project_id);
    
    let conn = db.0.lock().unwrap();
    
    let mut stmt = conn
        .prepare(
            "SELECT p.note_id, n.title, n.file_path, p.pinned_at, n.body, n.note_type
             FROM note_context_pins p
             JOIN notes n ON p.note_id = n.id
             WHERE n.project_id = ?1 AND n.deleted_at IS NULL
             ORDER BY p.pinned_at DESC",
        )
        .map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    let pin_data: Vec<(String, String, String, String, Option<String>, Option<String>)> = stmt
        .query_map(params![project_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })
        .map_err(|e| format!("Failed to query: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect: {}", e))?;
    
    let mut pins = Vec::new();
    for (note_id, title, file_path, pinned_at, body, note_type) in pin_data {
        // Check if system note
        let is_system = note_type.as_deref().map_or(false, |nt| 
            matches!(nt, "soul" | "memory" | "rules" | "identity" | "user" | "tools" | "agents" | "heartbeat" | "daily")
        );
        
        // Read content: DB body for regular notes, file for system notes
        let content = if is_system {
            read_note_content(&file_path).unwrap_or_default()
        } else {
            body.unwrap_or_else(|| read_note_content(&file_path).unwrap_or_default())
        };
        
        // Create preview and estimate tokens
        let preview = content.chars().take(100).collect::<String>();
        let preview = if content.chars().count() > 100 {
            format!("{}...", preview)
        } else {
            preview
        };
        // Rough token estimate: ~4 chars per token
        let tokens = (content.chars().count() + 3) / 4;
        let (content_preview, estimated_tokens) = (preview, tokens);
        
        pins.push(ContextPin {
            id: note_id,
            title,
            content_preview,
            pinned_at,
            estimated_tokens,
        });
    }
    
    eprintln!("[list_context_pins] Found {} pinned note(s)", pins.len());
    Ok(pins)
}

/// Get context budget usage for a project
pub fn get_context_budget_usage_internal(
    project_id: &str,
    db: &DbState,
) -> Result<ContextBudgetUsage, String> {
    eprintln!("[get_context_budget_usage] Calculating for project: {}", project_id);
    
    let conn = db.0.lock().unwrap();
    
    // Get all pinned notes for this project
    let mut stmt = conn
        .prepare(
            "SELECT n.file_path, n.body, n.note_type FROM note_context_pins p
             JOIN notes n ON p.note_id = n.id
             WHERE n.project_id = ?1 AND n.deleted_at IS NULL",
        )
        .map_err(|e| format!("Failed to prepare query: {}", e))?;
    
    let note_data: Vec<(String, Option<String>, Option<String>)> = stmt
        .query_map(params![project_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|e| format!("Failed to query: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to collect: {}", e))?;
    
    let mut pinned_chars = 0;
    let pinned_notes_count = note_data.len();
    
    // Read content from DB body for regular notes, file for system notes
    for (file_path, body, note_type) in &note_data {
        let is_system = note_type.as_deref().map_or(false, |nt| 
            matches!(nt, "soul" | "memory" | "rules" | "identity" | "user" | "tools" | "agents" | "heartbeat" | "daily")
        );
        
        let content = if is_system {
            read_note_content(file_path).unwrap_or_default()
        } else {
            body.clone().unwrap_or_else(|| read_note_content(file_path).unwrap_or_default())
        };
        
        pinned_chars += content.chars().count();
    }
    
    // Convert chars to tokens (~4 chars per token)
    let pinned_notes_tokens = (pinned_chars + 3) / 4;
    
    // Placeholder values for system files and project brief
    // These will be calculated from actual files in a future update
    let system_files_tokens = 500; // Estimated
    let project_brief_tokens = 200; // Estimated
    
    let total_used = system_files_tokens + project_brief_tokens + pinned_notes_tokens;
    let total_budget = 32000; // 32K tokens as per spec
    
    // Warning if pinned notes use more than 50% of budget
    let warning = pinned_notes_tokens as f64 > (total_budget as f64 * 0.5);
    
    eprintln!(
        "[get_context_budget_usage] {} notes, {} tokens (system: {}, brief: {}, pins: {}), total: {}/{}, warning: {}",
        pinned_notes_count, total_used, system_files_tokens, project_brief_tokens, 
        pinned_notes_tokens, total_used, total_budget, warning
    );
    
    Ok(ContextBudgetUsage {
        system_files_tokens,
        project_brief_tokens,
        pinned_notes_tokens,
        pinned_notes_count,
        total_used,
        total_budget,
        warning,
    })
}

// === Tauri Commands ===

#[tauri::command]
pub fn pin_note_to_context(note_id: String, db: State<DbState>) -> Result<(), String> {
    eprintln!("[pin_note_to_context] called with: note_id={}", note_id);
    pin_note_to_context_internal(&note_id, &db)
}

#[tauri::command]
pub fn unpin_note_from_context(note_id: String, db: State<DbState>) -> Result<(), String> {
    eprintln!("[unpin_note_from_context] called with: note_id={}", note_id);
    unpin_note_from_context_internal(&note_id, &db)
}

#[tauri::command]
pub fn is_note_pinned(note_id: String, db: State<DbState>) -> Result<bool, String> {
    eprintln!("[is_note_pinned] called with: note_id={}", note_id);
    is_note_pinned_internal(&note_id, &db)
}

#[tauri::command]
pub fn list_context_pins(project_id: String, db: State<DbState>) -> Result<Vec<ContextPin>, String> {
    eprintln!("[list_context_pins] called with: project_id={}", project_id);
    list_context_pins_internal(&project_id, &db)
}

#[tauri::command]
pub fn get_context_budget_usage(
    project_id: String,
    db: State<DbState>,
) -> Result<ContextBudgetUsage, String> {
    eprintln!("[get_context_budget_usage] called with: project_id={}", project_id);
    get_context_budget_usage_internal(&project_id, &db)
}
