use super::auth::{self, AuthSession};
use super::device;
use super::crypto;
use super::keystore;
use super::manager::SyncManager;
use super::engine;
use tauri::State;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::io::Write;
use tempfile::NamedTempFile;
use crate::database::DbState;
use crate::ai::ingest::dedup_pane_system;

// ─── ADR-0070 Decision 3 (#3195 PR A): account_enabled guard ───────────────
//
// `account_enabled` is a runtime row in `ai_settings`, NOT a schema column —
// no migration, SCHEMA_VERSION unchanged. `'true'` is on; absent, anything
// else, or a failed read (DB can't even be opened, table doesn't exist yet,
// query errors) is off. Call this as the FIRST statement of every command
// that touches session storage, the master key, or Supabase. 20 of the 27
// commands in this file are guarded (Reed's original 19 + `test_encryption`,
// which the ADR's own "touches the master key" criterion covers even though
// it wasn't in the ADR's named list):
// auth_sign_in, auth_sign_out, auth_refresh, auth_request_password_reset,
// auth_change_password, register_device_cmd, save_auth_session,
// clear_auth_session, init_encryption, clear_encryption, test_encryption,
// sync_push, sync_pull, sync_configure, sync_files_push, sync_files_pull,
// start_auto_sync, check_cleanup_pending, write_cleanup_flag,
// clear_cleanup_flag. `get_auth_session` and `is_encryption_ready` are
// read-only and deliberately do NOT call this — tests need to assert state
// regardless of flag. The remaining 5 (`get_hostname`, `get_install_uuid`,
// `get_sync_info`, `stop_auto_sync`, `notify_sync_change`) are local-only —
// no session, key, or Supabase touch.
//
// `auth_email_check`, `auth_check_entitlement`, `auth_take_over_machine` —
// gated here as an interim measure by PR A (#3201 FAIL fix 2) since they
// shipped reachable before this PR's deletion landed — are REMOVED outright
// by this PR (#3195 PR B, ADR-0070 Decision 1/4). The guard count above
// (20, not 23) reflects their removal, not an oversight.
//
// Outside this file: `notifications::push_notification` (notifications.rs)
// is a 21st command-surface path to the exact `load_auth_session` ->
// `refresh_auth_token` -> `save_auth_session` triad these commands guard —
// gated separately in notifications.rs (#3201 FAIL fix 1), not counted in
// the 27 above because it isn't declared in this file.
//
// `pub(crate)` (not `pub`, not private): `notifications.rs`'s
// `start_supabase_notification_poller` reuses this SAME function for its
// own gate (Decision 3/5) rather than re-deriving the on/off check —
// one function per operation, not two copies of "what does 'off' mean"
// that could drift apart.
pub(crate) fn require_account_enabled() -> Result<(), String> {
    let enabled = crate::database::open_connection(None)
        .ok()
        .and_then(|conn| crate::database::get_ai_setting(&conn, "account_enabled"))
        .map(|v| v == "true")
        .unwrap_or(false);

    if enabled {
        Ok(())
    } else {
        eprintln!("[ACCOUNT_GUARD] account_enabled is off — refusing (ADR-0070 Decision 3)");
        Err("account disabled".to_string())
    }
}

/// ADR-0070 Decision 4 (#3195 PR C): sign-in now targets Supabase GoTrue
/// directly (`auth::sign_in`), which already returns a plain `AuthSession`
/// parsed by the same shared parser `refresh_session` uses — no entitlement
/// snapshot exists anymore, so there is nothing left for a `SignInResult`
/// wrapper to carry. `install_uuid` and `machine_name` are no longer
/// gathered or sent.
#[tauri::command]
pub async fn auth_sign_in(email: String, password: String) -> Result<AuthSession, String> {
    require_account_enabled()?;
    eprintln!("[AUTH_SIGN_IN] called with email: {}", email);

    let session = auth::sign_in(&email, &password)
        .await
        .map_err(|e| e.to_string())?;

    // Save session
    save_auth_session(session.clone())?;

    // Store password in keychain for future use
    crate::keychain::store_password(&email, &password).ok();

    eprintln!("[AUTH_SIGN_IN] Session saved for user: {}", session.user_id);

    Ok(session)
}

#[tauri::command]
pub async fn auth_sign_out(access_token: String) -> Result<(), String> {
    require_account_enabled()?;
    auth::sign_out(&access_token).await
}

#[tauri::command]
pub async fn auth_refresh(refresh_token: String) -> Result<AuthSession, String> {
    require_account_enabled()?;
    auth::refresh_session(&refresh_token).await
}

#[tauri::command]
pub async fn auth_request_password_reset(email: String) -> Result<(), String> {
    require_account_enabled()?;
    auth::request_password_reset(&email).await
}

#[tauri::command]
pub async fn auth_change_password(new_password: String) -> Result<AuthSession, String> {
    require_account_enabled()?;
    eprintln!("[AUTH_CHANGE_PASSWORD] Starting password change flow");
    
    // Read current session to get both identity fields and access_token for Bearer header
    let current_session = get_auth_session()?
        .ok_or("No active session".to_string())?;
    
    eprintln!("[AUTH_CHANGE_PASSWORD] Current session user_id: {}, email: {}", 
        current_session.user_id, current_session.email);
    
    // Call Supabase to change password and get rotated tokens
    let (new_access_token, new_refresh_token, response_expires_at) = 
        auth::change_password(&current_session.access_token, &new_password).await?;
    
    // Construct new session: rotated tokens + preserved identity fields
    // If Supabase returned expires_at (non-zero), use it; otherwise preserve old value
    let new_expires_at = if response_expires_at != 0 {
        eprintln!("[AUTH_CHANGE_PASSWORD] Using expires_at from Supabase response: {}", response_expires_at);
        response_expires_at
    } else {
        eprintln!("[AUTH_CHANGE_PASSWORD] Preserving expires_at from prior session: {}", current_session.expires_at);
        current_session.expires_at
    };
    
    let new_session = AuthSession {
        access_token: new_access_token,
        refresh_token: new_refresh_token,
        user_id: current_session.user_id,
        email: current_session.email,
        expires_at: new_expires_at,
        device_id: current_session.device_id,
    };
    
    // Persist to both keychain and metadata file
    save_auth_session(new_session.clone())?;
    
    eprintln!("[AUTH_CHANGE_PASSWORD] Password changed and rotated tokens persisted successfully");
    
    Ok(new_session)
}

#[tauri::command]
pub async fn register_device_cmd(
    access_token: String,
    user_id: String,
    device_name: String,
    db: State<'_, DbState>,
) -> Result<String, String> {
    require_account_enabled()?;
    // Get site_id from cr-sqlite (in a separate scope to ensure lock is dropped)
    let site_id: Vec<u8> = {
        let conn = db.0.lock().unwrap();
        conn.query_row("SELECT crsql_site_id()", [], |row| row.get(0))
            .map_err(|e| format!("Failed to get site_id: {}", e))?
    };

    device::register_device(&access_token, &user_id, &device_name, "macos", &site_id).await
}

/// Metadata-only session data stored in JSON (tokens live in keychain)
#[derive(serde::Serialize, serde::Deserialize)]
struct AuthSessionMetadata {
    user_id: String,
    email: String,
    expires_at: i64,
    device_id: Option<String>,
}

#[tauri::command]
pub fn get_auth_session() -> Result<Option<AuthSession>, String> {
    eprintln!("[GET_AUTH_SESSION] Reading session from keychain + metadata file");
    
    let session_path = get_pane_data_dir().join("auth_session.json");
    
    if !session_path.exists() {
        eprintln!("[GET_AUTH_SESSION] No session metadata file found");
        return Ok(None);
    }
    
    let data = std::fs::read_to_string(&session_path)
        .map_err(|e| format!("Failed to read session metadata: {}", e))?;
    
    // Parse as generic JSON to check for legacy format (has access_token field)
    let json_value: serde_json::Value = serde_json::from_str(&data)
        .map_err(|e| format!("Failed to parse session JSON: {}", e))?;
    
    // MIGRATION: Check if this is legacy format (contains access_token)
    if json_value.get("access_token").is_some() {
        eprintln!("[AUTH] Migrating session tokens from file to keychain");
        
        // Parse as full AuthSession (legacy format)
        let legacy_session: AuthSession = serde_json::from_value(json_value.clone())
            .map_err(|e| format!("Failed to parse legacy session: {}", e))?;
        
        // Store tokens in keychain
        crate::keychain::store_password("supabase_access_token", &legacy_session.access_token)
            .map_err(|e| format!("Failed to migrate access_token to keychain: {}", e))?;
        crate::keychain::store_password("supabase_refresh_token", &legacy_session.refresh_token)
            .map_err(|e| format!("Failed to migrate refresh_token to keychain: {}", e))?;
        
        // Write metadata-only JSON (no tokens)
        let metadata = AuthSessionMetadata {
            user_id: legacy_session.user_id.clone(),
            email: legacy_session.email.clone(),
            expires_at: legacy_session.expires_at,
            device_id: legacy_session.device_id.clone(),
        };
        
        let metadata_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| format!("Failed to serialize metadata: {}", e))?;
        std::fs::write(&session_path, metadata_json)
            .map_err(|e| format!("Failed to write metadata: {}", e))?;
        
        eprintln!("[AUTH] Migration complete: tokens in keychain, metadata in file");
        
        return Ok(Some(legacy_session));
    }
    
    // Normal path: read metadata from file, tokens from keychain
    let metadata: AuthSessionMetadata = serde_json::from_value(json_value)
        .map_err(|e| format!("Failed to parse session metadata: {}", e))?;
    
    // Read tokens from keychain (handle both Ok(None) and Err cases as "not found")
    let access_token = match crate::keychain::get_password("supabase_access_token") {
        Ok(Some(token)) => token,
        Ok(None) | Err(_) => {
            eprintln!("[GET_AUTH_SESSION] Metadata file exists but access_token not in keychain - session invalid");
            return Ok(None);
        }
    };
    
    let refresh_token = match crate::keychain::get_password("supabase_refresh_token") {
        Ok(Some(token)) => token,
        Ok(None) | Err(_) => {
            eprintln!("[GET_AUTH_SESSION] Metadata file exists but refresh_token not in keychain - session invalid");
            return Ok(None);
        }
    };
    
    // Reconstruct full AuthSession
    Ok(Some(AuthSession {
        access_token,
        refresh_token,
        user_id: metadata.user_id,
        email: metadata.email,
        expires_at: metadata.expires_at,
        device_id: metadata.device_id,
    }))
}

#[tauri::command]
pub fn save_auth_session(session: AuthSession) -> Result<(), String> {
    require_account_enabled()?;
    eprintln!("[SAVE_AUTH_SESSION] Storing tokens in keychain, metadata in file");
    
    // Store tokens in keychain
    crate::keychain::store_password("supabase_access_token", &session.access_token)
        .map_err(|e| format!("Failed to store access_token in keychain: {}", e))?;
    crate::keychain::store_password("supabase_refresh_token", &session.refresh_token)
        .map_err(|e| format!("Failed to store refresh_token in keychain: {}", e))?;
    
    // Write metadata-only JSON (no tokens)
    let metadata = AuthSessionMetadata {
        user_id: session.user_id,
        email: session.email,
        expires_at: session.expires_at,
        device_id: session.device_id,
    };
    
    let session_path = get_pane_data_dir().join("auth_session.json");
    let data = serde_json::to_string_pretty(&metadata)
        .map_err(|e| format!("Failed to serialize metadata: {}", e))?;
    
    // Atomic write: tempfile + persist (§2.3A atomicity requirement)
    let session_dir = session_path.parent().unwrap();
    let mut temp = NamedTempFile::new_in(session_dir)
        .map_err(|e| format!("Failed to create temp file: {}", e))?;
    temp.write_all(data.as_bytes())
        .map_err(|e| format!("Failed to write temp file: {}", e))?;
    temp.persist(&session_path)
        .map_err(|e| format!("Failed to persist session file: {}", e))?;
    
    eprintln!("[SAVE_AUTH_SESSION] Session stored successfully");
    Ok(())
}

#[tauri::command]
pub fn clear_auth_session() -> Result<(), String> {
    require_account_enabled()?;
    eprintln!("[CLEAR_AUTH_SESSION] Removing tokens from keychain and metadata file");
    
    // Delete keychain entries
    crate::keychain::delete_password("supabase_access_token")?;
    crate::keychain::delete_password("supabase_refresh_token")?;
    
    // Delete metadata file
    let session_path = get_pane_data_dir().join("auth_session.json");
    if session_path.exists() {
        std::fs::remove_file(&session_path)
            .map_err(|e| format!("Failed to remove session metadata: {}", e))?;
    }
    
    eprintln!("[CLEAR_AUTH_SESSION] Session cleared successfully");
    Ok(())
}

fn get_pane_data_dir() -> PathBuf {
    dirs::home_dir().unwrap().join("Pane").join(".pane")
}

#[tauri::command]
pub fn get_hostname() -> Result<String, String> {
    // Use gethostname from std (available on macOS/Linux)
    let name = std::process::Command::new("hostname")
        .output()
        .map_err(|e| format!("Failed to get hostname: {}", e))?;
    let hostname = String::from_utf8_lossy(&name.stdout).trim().to_string();
    hostname
        .split('.')
        .next()
        .map(|s| s.to_string())
        .ok_or_else(|| "Hostname is empty".to_string())
}

#[tauri::command]
pub fn get_install_uuid() -> Result<String, String> {
    eprintln!("[GET_INSTALL_UUID] called");
    crate::install_uuid::get_or_create_install_uuid()
}

// Encryption commands

/// Initialize encryption after sign-in
/// Derives master key from password + email and stores it
#[tauri::command]
pub async fn init_encryption(email: String, password: String) -> Result<(), String> {
    require_account_enabled()?;
    let master_key = crypto::derive_master_key(&password, &email)?;
    keystore::store_master_key(&master_key)?;
    Ok(())
}

/// Check if encryption is initialized (key exists)
#[tauri::command]
pub fn is_encryption_ready() -> Result<bool, String> {
    let key = keystore::load_master_key()?;
    Ok(key.is_some())
}

/// Clear encryption keys (on sign out)
#[tauri::command]
pub fn clear_encryption() -> Result<(), String> {
    require_account_enabled()?;
    keystore::delete_master_key()
}

/// Test encryption round-trip (for debugging)
#[tauri::command]
pub fn test_encryption() -> Result<String, String> {
    require_account_enabled()?;
    let key = keystore::load_master_key()?
        .ok_or_else(|| "No encryption key stored".to_string())?;
    
    let sync_key = crypto::get_sync_key(&key);
    let test_data = b"Hello from Pane sync encryption test!";
    
    let encrypted = crypto::encrypt_changeset(&sync_key, test_data)?;
    let decrypted = crypto::decrypt_changeset(&sync_key, &encrypted)?;
    
    if decrypted != test_data {
        return Err("Encryption round-trip failed!".to_string());
    }
    
    Ok(format!("Encryption OK. {} bytes → {} bytes encrypted → {} bytes decrypted", 
        test_data.len(), encrypted.len(), decrypted.len()))
}

// Sync Engine State and Commands

pub struct SyncManagerState(pub Arc<Mutex<SyncManager>>);

#[tauri::command]
pub async fn sync_push(
    db: tauri::State<'_, DbState>,
    sync_state: tauri::State<'_, SyncManagerState>,
) -> Result<String, String> {
    require_account_enabled()?;
    // Extract data from manager and connection BEFORE any async calls
    let (access_token, user_id, device_id, last_pushed_version) = {
        let manager = sync_state.0.lock().map_err(|e| e.to_string())?;
        if !manager.is_ready() {
            return Err("Not signed in — sync requires authentication".to_string());
        }
        (
            manager.access_token.clone().unwrap(),
            manager.user_id.clone().unwrap(),
            manager.device_id.clone().unwrap(),
            manager.last_pushed_version,
        )
    }; // manager lock dropped here
    
    let (payload, db_version) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        
        let payload = engine::prepare_sync_payload(&conn, last_pushed_version)?;
        
        let db_version: i64 = conn.query_row(
            "SELECT crsql_db_version()",
            [],
            |row| row.get(0),
        ).map_err(|e| e.to_string())?;
        
        (payload, db_version)
    }; // conn lock dropped here
    
    match payload {
        None => Ok("No changes to sync".to_string()),
        Some(data) => {
            let size = data.len();
            
            super::relay::push_to_queue(
                &access_token,
                &user_id,
                &device_id,
                &data,
                db_version,
            ).await?;
            
            Ok(format!("Pushed {} bytes of encrypted changes", size))
        }
    }
}

#[tauri::command]
pub async fn sync_pull(
    db: tauri::State<'_, DbState>,
    sync_state: tauri::State<'_, SyncManagerState>,
) -> Result<String, String> {
    require_account_enabled()?;
    // Extract auth info before async call
    let (access_token, device_id) = {
        let manager = sync_state.0.lock().map_err(|e| e.to_string())?;
        if !manager.is_ready() {
            return Err("Not signed in — sync requires authentication".to_string());
        }
        (
            manager.access_token.clone().unwrap(),
            manager.device_id.clone().unwrap(),
        )
    }; // manager lock dropped here
    
    // Clean up own-device changesets (prevents egress bloat)
    super::relay::cleanup_own_changesets(&access_token, &device_id).await.ok();
    
    let queued = super::relay::pull_from_queue(&access_token, &device_id).await?;
    
    if queued.is_empty() {
        return Ok("No pending changes".to_string());
    }
    
    let mut total_applied = 0i64;
    let mut skipped_own = 0i64;
    let device_count = queued.len();
    
    for item in &queued {
        // Skip our own changesets (other devices still need them)
        // Note: API filter should prevent these, but this is a safety measure
        if item.from_device_id == device_id {
            skipped_own += 1;
            // Delete own changesets from queue — we don't need them
            // Other devices will have already pulled them
            super::relay::delete_from_queue(&access_token, &item.id).await.ok();
            continue;
        }
        
        use base64::{Engine as _, engine::general_purpose};
        let payload_bytes = general_purpose::STANDARD.decode(&item.encrypted_payload)
            .map_err(|e| format!("Failed to decode payload: {}", e))?;
        
        // Apply changeset (lock and release quickly)
        let applied = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            engine::receive_sync_payload(&conn, &payload_bytes)?
        }; // conn lock dropped here
        
        total_applied += applied;
        
        super::relay::delete_from_queue(&access_token, &item.id).await.ok();
    }
    
    // After applying sync changes, clean up
    if total_applied > 0 {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        
        // §24: Run agent integrity check instead of Pane System dedup
        let has_agents_table: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='ai_agents'",
            [],
            |row| row.get(0),
        ).unwrap_or(false);
        
        if has_agents_table {
            crate::ai::ingest::verify_agents_integrity(&conn);
        } else {
            dedup_pane_system(&conn);
        }
        
        // #125: After sync pull, re-check system notes against flat files — largest content wins
        restore_system_notes_from_flat_files(&conn);
    }

    Ok(format!("Applied {} changes ({} in queue, {} skipped own)", total_applied, device_count, skipped_own))
}

#[tauri::command]
pub async fn sync_configure(
    user_id: String,
    device_id: String,
    access_token: String,
    sync_state: tauri::State<'_, SyncManagerState>,
) -> Result<(), String> {
    require_account_enabled()?;
    let mut manager = sync_state.0.lock().map_err(|e| e.to_string())?;
    manager.set_auth(user_id, device_id, access_token);
    Ok(())
}

#[tauri::command]
pub fn get_sync_info(
    db: tauri::State<'_, DbState>,
) -> Result<SyncInfo, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    
    let site_id: Vec<u8> = conn.query_row("SELECT crsql_site_id()", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    let db_version: i64 = conn.query_row("SELECT crsql_db_version()", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    
    // Count pending changes
    let change_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM crsql_changes",
        [],
        |row| row.get(0),
    ).map_err(|e| e.to_string())?;
    
    Ok(SyncInfo {
        site_id: hex::encode(&site_id),
        db_version,
        total_changes: change_count,
    })
}

#[derive(serde::Serialize)]
pub struct SyncInfo {
    site_id: String,
    db_version: i64,
    total_changes: i64,
}

// Auto-sync state wrapper
pub struct AutoSyncState(pub Arc<super::auto_sync::AutoSync>);

// Auto-sync commands
#[tauri::command]
pub fn start_auto_sync(auto_sync: State<'_, AutoSyncState>) -> Result<String, String> {
    require_account_enabled()?;
    auto_sync.0.enable();
    Ok("Auto-sync enabled".to_string())
}

#[tauri::command]
pub fn stop_auto_sync(auto_sync: State<'_, AutoSyncState>) -> Result<String, String> {
    auto_sync.0.disable();
    Ok("Auto-sync disabled".to_string())
}

#[tauri::command]
pub fn notify_sync_change(auto_sync: State<'_, AutoSyncState>) -> Result<(), String> {
    auto_sync.0.notify_change();
    Ok(())
}


// File sync commands

/// Sync all note files for the current user
#[tauri::command]
pub async fn sync_files_push(
    db: tauri::State<'_, DbState>,
    sync_state: tauri::State<'_, SyncManagerState>,
) -> Result<String, String> {
    require_account_enabled()?;
    let (access_token, user_id) = {
        let manager = sync_state.0.lock().map_err(|e| e.to_string())?;
        if !manager.is_ready() {
            return Ok("Not signed in".to_string());
        }
        (manager.access_token.clone().unwrap(), manager.user_id.clone().unwrap())
    };
    
    // Get all notes with file paths
    let notes: Vec<(String, String)> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, file_path FROM notes WHERE file_path IS NOT NULL AND file_path != '' AND parent_id IS NULL"
        ).map_err(|e| e.to_string())?;
        
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).map_err(|e| e.to_string())?;
        
        rows.filter_map(|r| r.ok()).collect()
    };
    
    let mut uploaded = 0;
    let mut errors = 0;
    
    for (note_id, file_path) in &notes {
        match super::file_sync::sync_note_file(&access_token, &user_id, note_id, file_path).await {
            Ok(_) => uploaded += 1,
            Err(e) => {
                println!("[file-sync] Error uploading {}: {}", note_id, e);
                errors += 1;
            }
        }
    }
    
    Ok(format!("Uploaded {} files ({} errors)", uploaded, errors))
}

/// Pull all note files from storage
#[tauri::command]
pub async fn sync_files_pull(
    db: tauri::State<'_, DbState>,
    sync_state: tauri::State<'_, SyncManagerState>,
) -> Result<String, String> {
    require_account_enabled()?;
    let (access_token, user_id) = {
        let manager = sync_state.0.lock().map_err(|e| e.to_string())?;
        if !manager.is_ready() {
            return Ok("Not signed in".to_string());
        }
        (manager.access_token.clone().unwrap(), manager.user_id.clone().unwrap())
    };
    
    // Get all notes with file paths
    let notes: Vec<(String, String)> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare(
            "SELECT id, file_path FROM notes WHERE file_path IS NOT NULL AND file_path != '' AND parent_id IS NULL"
        ).map_err(|e| e.to_string())?;
        
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }).map_err(|e| e.to_string())?;
        
        rows.filter_map(|r| r.ok()).collect()
    };
    
    let mut downloaded = 0;
    let mut errors = 0;
    
    for (note_id, file_path) in &notes {
        match super::file_sync::pull_note_file(&access_token, &user_id, &note_id, file_path).await {
            Ok(_) => downloaded += 1,
            Err(e) => {
                // 404 means file hasn't been uploaded yet — not an error
                if !e.contains("404") {
                    println!("[file-sync] Error downloading {}: {}", note_id, e);
                    errors += 1;
                }
            }
        }
    }
    
    Ok(format!("Downloaded {} files ({} errors)", downloaded, errors))
}

/// #125: After sync pull, compare DB system notes with flat files — keep the larger version.
/// This prevents stale synced data from overwriting richer local flat files.
fn restore_system_notes_from_flat_files(conn: &rusqlite::Connection) {
    use crate::database::{get_pane_root, read_note_content, write_note_content};
    
    let pane_root = get_pane_root();
    let system_dir = pane_root.join(".pane").join("system");
    
    let note_types = &[
        ("soul", "SOUL.md"),
        ("memory", "MEMORY.md"),
        ("rules", "AGENTS.md"),
        ("identity", "IDENTITY.md"),
        ("user", "USER.md"),
        ("tools", "TOOLS.md"),
    ];
    
    // Get all system notes from DB
    let mut stmt = match conn.prepare(
        "SELECT id, file_path, note_type FROM notes WHERE note_type IN ('soul', 'memory', 'rules', 'identity', 'user', 'tools') AND deleted_at IS NULL"
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[sync-restore] Failed to query system notes: {}", e);
            return;
        }
    };
    
    let notes: Vec<(String, String, String)> = stmt.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
    }).ok().map(|rows| rows.filter_map(|r| r.ok()).collect()).unwrap_or_default();
    
    for (note_id, file_path, note_type) in &notes {
        // Find the matching flat file
        let flat_filename = note_types.iter()
            .find(|(nt, _)| nt == note_type)
            .map(|(_, fname)| *fname);
        
        let flat_filename = match flat_filename {
            Some(f) => f,
            None => continue,
        };
        
        // Check main system dir and agent subdirs
        let flat_path = system_dir.join(flat_filename);
        let flat_content = match std::fs::read_to_string(&flat_path) {
            Ok(c) => c,
            Err(_) => continue, // No flat file, nothing to restore from
        };
        
        let db_content = read_note_content(file_path).unwrap_or_default();
        
        // #121: Compare timestamps — flat file mtime vs DB updated_at
        let should_restore = match std::fs::metadata(&flat_path) {
            Ok(meta) => {
                match meta.modified() {
                    Ok(file_mtime) => {
                        let file_time: chrono::DateTime<chrono::Utc> = file_mtime.into();
                        // Get DB updated_at for this note
                        let db_updated: Option<String> = conn.query_row(
                            "SELECT updated_at FROM notes WHERE id = ?1",
                            rusqlite::params![note_id],
                            |row| row.get(0),
                        ).ok();
                        match db_updated.and_then(|s| s.parse::<chrono::DateTime<chrono::Utc>>().ok()) {
                            Some(db_time) => {
                                let should_restore = file_time > db_time;
                                if should_restore {
                                    eprintln!("[sync-restore] Restoring {} from flat file (file: {}, db: {})", 
                                        note_type, file_time.to_rfc3339(), db_time.to_rfc3339());
                                }
                                should_restore
                            }
                            None => {
                                // Fallback to size if no timestamp
                                let size_restore = flat_content.len() > db_content.len() && flat_content.len() > 50;
                                if size_restore {
                                    eprintln!("[sync-restore] Restoring {} from flat file (no DB timestamp, {} > {} bytes)", 
                                        note_type, flat_content.len(), db_content.len());
                                }
                                size_restore
                            }
                        }
                    }
                    Err(_) => {
                        // Fallback to size if can't read mtime
                        let size_restore = flat_content.len() > db_content.len() && flat_content.len() > 50;
                        if size_restore {
                            eprintln!("[sync-restore] Restoring {} from flat file (no file mtime, {} > {} bytes)", 
                                note_type, flat_content.len(), db_content.len());
                        }
                        size_restore
                    }
                }
            }
            Err(_) => false
        };
        
        if should_restore {
            if let Err(e) = write_note_content(file_path, &flat_content, Some(conn)) {
                eprintln!("[sync-restore] Failed to restore {}: {}", note_type, e);
            } else {
                // Update metadata
                let word_count = flat_content.split_whitespace().count() as i64;
                let preview: String = flat_content.chars().take(200).collect();
                conn.execute(
                    "UPDATE notes SET content_preview = ?1, word_count = ?2, updated_at = ?3 WHERE id = ?4",
                    rusqlite::params![preview, word_count, chrono::Utc::now().to_rfc3339(), note_id],
                ).ok();
            }
        }
    }
}

// ============================================================
// Cleanup Pending Flag (v0.65.32 — #1079)
// ============================================================

#[derive(serde::Serialize, serde::Deserialize)]
pub struct CleanupPendingFlag {
    pub attempts: u32,
    pub created_at: String,
    pub cleanup_encryption: bool,
    pub cleanup_auth_session: bool,
}

fn cleanup_flag_path() -> Result<std::path::PathBuf, String> {
    let base = dirs::data_dir()
        .ok_or_else(|| "Could not determine data directory".to_string())?;
    Ok(base.join("Pane").join("cleanup_pending.json"))
}

#[tauri::command]
pub fn check_cleanup_pending() -> Result<Option<CleanupPendingFlag>, String> {
    require_account_enabled()?;
    eprintln!("[CHECK_CLEANUP_PENDING] called");
    let path = cleanup_flag_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("check_cleanup_pending: failed to read flag: {}", e))?;
    let flag: CleanupPendingFlag = serde_json::from_str(&content)
        .map_err(|e| format!("check_cleanup_pending: failed to parse flag: {}", e))?;
    eprintln!("[CHECK_CLEANUP_PENDING] found flag with attempts={}", flag.attempts);
    Ok(Some(flag))
}

#[tauri::command]
pub fn write_cleanup_flag(cleanup_encryption: bool, cleanup_auth_session: bool, attempts: Option<u32>) -> Result<(), String> {
    require_account_enabled()?;
    eprintln!("[WRITE_CLEANUP_FLAG] called cleanup_encryption={} cleanup_auth_session={}", cleanup_encryption, cleanup_auth_session);
    let path = cleanup_flag_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("write_cleanup_flag: failed to create dir: {}", e))?;
    }
    let flag = CleanupPendingFlag {
        attempts: attempts.unwrap_or(1),
        created_at: chrono::Utc::now().to_rfc3339(),
        cleanup_encryption,
        cleanup_auth_session,
    };
    let json = serde_json::to_string(&flag)
        .map_err(|e| format!("write_cleanup_flag: failed to serialize: {}", e))?;
    std::fs::write(&path, json)
        .map_err(|e| format!("write_cleanup_flag: failed to write: {}", e))?;
    eprintln!("[WRITE_CLEANUP_FLAG] flag written to {:?}", path);
    Ok(())
}

#[tauri::command]
pub fn clear_cleanup_flag() -> Result<(), String> {
    require_account_enabled()?;
    eprintln!("[CLEAR_CLEANUP_FLAG] called");
    let path = cleanup_flag_path()?;
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("clear_cleanup_flag: failed to delete: {}", e))?;
        eprintln!("[CLEAR_CLEANUP_FLAG] flag deleted");
    } else {
        eprintln!("[CLEAR_CLEANUP_FLAG] no flag file, nothing to delete");
    }
    Ok(())
}

#[cfg(test)]
mod cleanup_flag_tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn with_temp_flag_dir<F: FnOnce(std::path::PathBuf)>(f: F) {
        let dir = TempDir::new().unwrap();
        let flag_path = dir.path().join("cleanup_pending.json");
        f(flag_path);
    }

    #[test]
    fn test_write_and_read_cleanup_flag_roundtrip() {
        with_temp_flag_dir(|path| {
            // Write manually using the struct
            let flag = CleanupPendingFlag {
                attempts: 1,
                created_at: "2026-01-01T00:00:00Z".to_string(),
                cleanup_encryption: true,
                cleanup_auth_session: false,
            };
            let json = serde_json::to_string(&flag).unwrap();
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, json).unwrap();

            // Read back
            let content = fs::read_to_string(&path).unwrap();
            let parsed: CleanupPendingFlag = serde_json::from_str(&content).unwrap();
            assert_eq!(parsed.attempts, 1);
            assert_eq!(parsed.cleanup_encryption, true);
            assert_eq!(parsed.cleanup_auth_session, false);
        });
    }

    #[test]
    fn test_clear_cleanup_flag_removes_file() {
        with_temp_flag_dir(|path| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "{}").unwrap();
            assert!(path.exists());
            fs::remove_file(&path).unwrap();
            assert!(!path.exists());
        });
    }

    #[test]
    fn test_check_cleanup_pending_returns_none_when_no_file() {
        with_temp_flag_dir(|path| {
            assert!(!path.exists());
            // Simulate: no file → deserialize would fail, but we check exists first
            let result: Option<CleanupPendingFlag> = if path.exists() {
                let content = fs::read_to_string(&path).unwrap();
                Some(serde_json::from_str(&content).unwrap())
            } else {
                None
            };
            assert!(result.is_none());
        });
    }
}

#[cfg(test)]
mod account_enabled_guard_tests {
    //! ADR-0070 Decision 3 (#3195 PR A) — the `account_enabled` guard.
    //!
    //! Isolation note: this test binary runs on a real dev machine, not just
    //! CI. `get_db_path()` / `get_pane_data_dir()` / `cleanup_flag_path()`
    //! all key off `PANE_TEST_ROOT` and/or `HOME` — every test here sets
    //! BOTH to a throwaway `TempDir` before touching anything, and never
    //! reads `~/Pane/.pane/` for real. `#[serial(...)]` is required because
    //! `std::env::set_var` and the keychain override are process-global.
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tempfile::TempDir;

    /// Point HOME and PANE_TEST_ROOT at a fresh temp dir. Returns the
    /// TempDir so it stays alive (and is cleaned up) for the test's duration.
    fn isolate() -> TempDir {
        let dir = TempDir::new().expect("tempdir");
        std::env::set_var("HOME", dir.path());
        std::env::set_var("PANE_TEST_ROOT", dir.path().join("Pane"));
        dir
    }

    /// Seed (or leave absent) the `account_enabled` row in the isolated test
    /// DB, at the exact path `require_account_enabled()`'s own
    /// `database::open_connection(None)` call will resolve to.
    fn seed_account_enabled(value: Option<&str>) {
        let db_path = crate::database::get_db_path();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).expect("create db dir");
        }
        let conn = rusqlite::Connection::open(&db_path).expect("open test db");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ai_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            );",
        )
        .expect("create ai_settings table");
        match value {
            Some(v) => {
                crate::database::set_ai_setting(&conn, "account_enabled", v)
                    .expect("seed account_enabled");
            }
            None => {
                // Explicitly delete any prior row — `None` must mean truly
                // absent, not "whatever was last written".
                conn.execute("DELETE FROM ai_settings WHERE key = 'account_enabled'", [])
                    .expect("clear account_enabled row");
            }
        }
    }

    // ── require_account_enabled() itself — 'true' is on; absent, anything
    // else, or a failed read is off (ADR-0070 Decision 3, verbatim). ──────

    #[test]
    #[serial(pane_test_root)]
    fn test_guard_absent_row_is_off() {
        let _d = isolate();
        seed_account_enabled(None); // table exists, no row
        assert_eq!(require_account_enabled(), Err("account disabled".to_string()));
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_guard_no_table_at_all_is_off() {
        // A failed read (ai_settings doesn't even exist yet — fresh install
        // before any migration ever created it) must be off, never a panic
        // and never an `Ok`.
        let _d = isolate();
        assert_eq!(require_account_enabled(), Err("account disabled".to_string()));
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_guard_false_string_is_off() {
        let _d = isolate();
        seed_account_enabled(Some("false"));
        assert_eq!(require_account_enabled(), Err("account disabled".to_string()));
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_guard_garbage_value_is_off() {
        // Case-sensitive — the spec says literally 'true', not "True"/"TRUE".
        let _d = isolate();
        seed_account_enabled(Some("TRUE"));
        assert_eq!(require_account_enabled(), Err("account disabled".to_string()));
    }

    #[test]
    #[serial(pane_test_root)]
    fn test_guard_true_is_on() {
        let _d = isolate();
        seed_account_enabled(Some("true"));
        assert_eq!(require_account_enabled(), Ok(()));
    }

    // ── T2b (named red-first test in the PR body): the guard refuses
    // clear_encryption / clear_auth_session when off. ────────────────────

    #[test]
    #[serial(pane_test_root, keychain_override)]
    fn test_t2b_clear_encryption_refuses_when_flag_off() {
        let _d = isolate();
        let _kc = KeychainGuard::set();

        // Seed a real master key (flag briefly on to pass init, then off)
        // so we can prove it's untouched, not just that the call errored
        // for some unrelated reason.
        seed_account_enabled(Some("true"));
        crate::sync::keystore::store_master_key(&[7u8; 32]).expect("seed master key");
        assert!(crate::sync::keystore::load_master_key().unwrap().is_some());

        seed_account_enabled(None); // off
        let result = clear_encryption();
        assert_eq!(result, Err("account disabled".to_string()));

        // The master key must still be there — the guard fired before
        // keystore::delete_master_key() was ever reached.
        assert!(
            crate::sync::keystore::load_master_key().unwrap().is_some(),
            "clear_encryption must not delete the master key when account_enabled is off"
        );
    }

    #[test]
    #[serial(pane_test_root, keychain_override)]
    fn test_t2b_clear_auth_session_refuses_when_flag_off() {
        let _d = isolate();
        let _kc = KeychainGuard::set();

        let session = AuthSession {
            access_token: "tok_abc".to_string(),
            refresh_token: "ref_abc".to_string(),
            user_id: "u1".to_string(),
            email: "t@example.com".to_string(),
            expires_at: 9999999999,
            device_id: None,
        };
        // Seed with the flag briefly ON so save_auth_session (also guarded)
        // succeeds, then flip off before calling clear.
        seed_account_enabled(Some("true"));
        save_auth_session(session).expect("seed session");

        seed_account_enabled(None); // off
        let result = clear_auth_session();
        assert_eq!(result, Err("account disabled".to_string()));

        // The session must still be fully intact — the guard fired before
        // any keychain delete / file removal was reached. get_auth_session
        // is deliberately ungated (read-only) so this assertion itself
        // doesn't trip the guard.
        let still_there = get_auth_session().expect("get_auth_session is ungated by design");
        assert!(
            still_there.is_some(),
            "clear_auth_session must not delete the session when account_enabled is off"
        );
    }

    // ── T2c (green-first by design, per the PR body): a naive happy-path
    // test for auth_sign_in is green today for the WRONG reason — there's
    // no mock, so it fails on a real network attempt regardless of the
    // guard. T2c originally proved this dynamically by pointing the
    // ecom-wrapper's configurable base-URL env var at a mockito server
    // that was set up to succeed, then asserting it was never dispatched
    // (`.expect(0)`) while the guard was off.
    //
    // ADR-0070 Decision 4 (#3195 PR C) removes that seam on purpose:
    // `auth::sign_in` now posts to the hardcoded `SUPABASE_URL` directly —
    // there is no env var or parameter left to redirect it to a local
    // mock from inside `auth_sign_in`. Re-creating the old dynamic test
    // would require either (a) reintroducing an injectable URL into
    // `auth_sign_in` for no reason other than testing, which the ADR
    // explicitly retires, or (b) actually dispatching to the real
    // Supabase host to prove non-dispatch, which violates the mockito-only
    // rule outright. Neither is acceptable, so T2c moves to the same
    // source-pinned technique already used below for the 7 State-taking
    // commands — a different limitation (no test constructor for
    // `tauri::State`), same resolution (pin the guard as the literal first
    // statement and assert on the source text, not a runtime call). ─────

    #[test]
    fn test_t2c_auth_sign_in_guard_is_first_statement_pinned_to_source() {
        let src = include_str!("commands.rs");
        assert!(
            src.contains("pub async fn auth_sign_in(email: String, password: String) -> Result<AuthSession, String> {\n    require_account_enabled()?;"),
            "auth_sign_in's require_account_enabled() guard must be the literal first statement in the real source"
        );
    }

    // ── Structural pin: the 7 guarded commands that take a tauri::State
    // parameter can't be called directly here (State has no public test
    // constructor outside a running App — see PR report). Pin the guard as
    // the literal first statement in the real source instead, the same
    // golden-string technique scheduler.rs already uses for structural
    // guarantees it can't exercise at runtime. ───────────────────────────

    #[test]
    fn test_state_taking_commands_guard_is_first_statement_pinned_to_source() {
        let src = include_str!("commands.rs");

        let cases: &[(&str, &str)] = &[
            (
                "register_device_cmd",
                "    db: State<'_, DbState>,\n) -> Result<String, String> {\n    require_account_enabled()?;\n    // Get site_id from cr-sqlite",
            ),
            (
                "sync_push",
                "pub async fn sync_push(\n    db: tauri::State<'_, DbState>,\n    sync_state: tauri::State<'_, SyncManagerState>,\n) -> Result<String, String> {\n    require_account_enabled()?;",
            ),
            (
                "sync_pull",
                "pub async fn sync_pull(\n    db: tauri::State<'_, DbState>,\n    sync_state: tauri::State<'_, SyncManagerState>,\n) -> Result<String, String> {\n    require_account_enabled()?;",
            ),
            (
                "sync_configure",
                "    sync_state: tauri::State<'_, SyncManagerState>,\n) -> Result<(), String> {\n    require_account_enabled()?;\n    let mut manager = sync_state.0.lock()",
            ),
            (
                "sync_files_push",
                "pub async fn sync_files_push(\n    db: tauri::State<'_, DbState>,\n    sync_state: tauri::State<'_, SyncManagerState>,\n) -> Result<String, String> {\n    require_account_enabled()?;",
            ),
            (
                "sync_files_pull",
                "pub async fn sync_files_pull(\n    db: tauri::State<'_, DbState>,\n    sync_state: tauri::State<'_, SyncManagerState>,\n) -> Result<String, String> {\n    require_account_enabled()?;",
            ),
            (
                "start_auto_sync",
                "pub fn start_auto_sync(auto_sync: State<'_, AutoSyncState>) -> Result<String, String> {\n    require_account_enabled()?;",
            ),
        ];

        for (name, needle) in cases {
            assert!(
                src.contains(needle),
                "{}'s guard must be the literal first statement in the real source",
                name
            );
        }
    }

    // ── #3218 item 9: the e2e IPC recorder's command set, source-derived.
    //
    // `tests/e2e/tauri-mock.ts`'s `ACCOUNT_SYNC_CMD_PATTERN` is a hand-kept
    // list, on the other side of a Rust/TypeScript boundary, that is
    // supposed to record every command `require_account_enabled()` guards.
    // Nothing enforced that invariant — this test closes that gap using the
    // same source-text-scan idiom `no_credential_logging.rs` already uses
    // for "no site may do X" structural guards: read the real source as
    // text, derive the actual set, assert it against a duplicated constant.
    //
    // The derived set only scans the PRODUCTION half of this file (before
    // the first `#[cfg(test)]`), so golden-string assertions and doc
    // comments in the test modules below are never mistaken for real guard
    // call sites.
    fn derive_gated_account_sync_commands(src: &str) -> std::collections::BTreeSet<String> {
        let production_src = src.split("#[cfg(test)]").next().unwrap_or(src);
        let mut gated = std::collections::BTreeSet::new();
        for (idx, _) in production_src.match_indices("require_account_enabled()?;") {
            let preceding = &production_src[..idx];
            if let Some(fn_pos) = preceding.rfind("fn ") {
                let after_fn = &preceding[fn_pos + 3..];
                let name: String = after_fn
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    gated.insert(name);
                }
            }
        }
        gated
    }

    /// Mirrors `ACCOUNT_SYNC_CMD_PATTERN` in `tests/e2e/tauri-mock.ts`. This
    /// is a MANUALLY KEPT duplicate across the Rust/TypeScript boundary
    /// (ADR-0070, #3202, #3218 item 9) — a change to either list without
    /// the other goes red here. Keep sorted.
    const E2E_ACCOUNT_SYNC_RECORDER_COMMANDS: &[&str] = &[
        "auth_change_password",
        "auth_refresh",
        "auth_request_password_reset",
        "auth_sign_in",
        "auth_sign_out",
        "check_cleanup_pending",
        "clear_auth_session",
        "clear_cleanup_flag",
        "clear_encryption",
        "init_encryption",
        "register_device_cmd",
        "save_auth_session",
        "start_auto_sync",
        "sync_configure",
        "sync_files_pull",
        "sync_files_push",
        "sync_pull",
        "sync_push",
        "test_encryption",
        "write_cleanup_flag",
    ];

    #[test]
    fn test_e2e_account_sync_recorder_matches_require_account_enabled_guards() {
        let src = include_str!("commands.rs");
        let derived = derive_gated_account_sync_commands(src);
        let mirrored: std::collections::BTreeSet<String> = E2E_ACCOUNT_SYNC_RECORDER_COMMANDS
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            derived, mirrored,
            "tests/e2e/tauri-mock.ts's ACCOUNT_SYNC_CMD_PATTERN must list exactly the \
             commands require_account_enabled() guards in sync/commands.rs. If this \
             assertion fails, update BOTH this constant and the TS regex so they agree."
        );
    }

}
