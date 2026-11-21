use rusqlite::Connection;
use super::{engine, relay};

/// Tracks sync state
pub struct SyncManager {
    pub last_pushed_version: i64,
    pub device_id: Option<String>,
    pub user_id: Option<String>,
    pub access_token: Option<String>,
}

impl SyncManager {
    pub fn new() -> Self {
        SyncManager {
            last_pushed_version: 0,
            device_id: None,
            user_id: None,
            access_token: None,
        }
    }
    
    pub fn set_auth(&mut self, user_id: String, device_id: String, access_token: String) {
        self.user_id = Some(user_id);
        self.device_id = Some(device_id);
        self.access_token = Some(access_token);
    }
    
    // Retained for callers that reset dormant legacy sync state.
    #[allow(dead_code)]
    pub fn clear_auth(&mut self) {
        self.user_id = None;
        self.device_id = None;
        self.access_token = None;
    }
    
    pub fn is_ready(&self) -> bool {
        self.access_token.is_some() && self.device_id.is_some()
    }
}

/// Push local changes to the cloud
// Retained for the dormant legacy sync path.
#[allow(dead_code)]
pub async fn push_changes(
    conn: &Connection,
    manager: &SyncManager,
) -> Result<u64, String> {
    if !manager.is_ready() {
        return Err("Sync not configured — sign in first".to_string());
    }
    
    let encrypted = engine::prepare_sync_payload(conn, manager.last_pushed_version)?;
    
    match encrypted {
        None => Ok(0), // Nothing to sync
        Some(data) => {
            let db_version: i64 = conn.query_row(
                "SELECT crsql_db_version()",
                [],
                |row| row.get(0),
            ).map_err(|e| e.to_string())?;
            
            relay::push_to_queue(
                manager.access_token.as_ref().unwrap(),
                manager.user_id.as_ref().unwrap(),
                manager.device_id.as_ref().unwrap(),
                &data,
                db_version,
            ).await?;
            
            Ok(data.len() as u64)
        }
    }
}

/// Pull and apply remote changes
// Retained for the dormant legacy sync path.
#[allow(dead_code)]
pub async fn pull_changes(
    conn: &Connection,
    manager: &SyncManager,
) -> Result<i64, String> {
    if !manager.is_ready() {
        return Err("Sync not configured — sign in first".to_string());
    }
    
    let queued = relay::pull_from_queue(
        manager.access_token.as_ref().unwrap(),
        manager.device_id.as_ref().unwrap(),
    ).await?;
    
    let mut total_applied = 0i64;
    
    for item in &queued {
        // Skip our own changesets
        if item.from_device_id == *manager.device_id.as_ref().unwrap() {
            // Clean up our own queued items
            relay::delete_from_queue(
                manager.access_token.as_ref().unwrap(),
                &item.id,
            ).await.ok();
            continue;
        }
        
        // Decode the hex payload from Postgres bytea
        let payload_bytes = hex::decode(
            item.encrypted_payload.trim_start_matches("\\x")
        ).map_err(|e| format!("Failed to decode payload: {}", e))?;
        
        // Decrypt and apply
        let applied = engine::receive_sync_payload(conn, &payload_bytes)?;
        total_applied += applied;
        
        // Delete processed changeset
        relay::delete_from_queue(
            manager.access_token.as_ref().unwrap(),
            &item.id,
        ).await.ok();
    }
    
    Ok(total_applied)
}
