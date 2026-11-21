// install_uuid.rs — Install UUID Management (ADR-0026)
// Built: 2026-05-07 (v0.18.0, issue #972)
//
// Generates and persists a unique identifier for this Pane installation.
// Used for backup/restore to distinguish different installations and for
// server-side device management (ADR-0026 §4).
//
// Storage: ~/Library/Application Support/Pane/install-uuid.json
// Format: {"install_uuid": "<uuid>", "created_at": "<rfc3339>"}
// Write pattern: atomic (tempfile + rename, per §2.3A)

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;
use chrono::Utc;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InstallUuid {
    pub install_uuid: String,
    pub created_at: String,
}

/// Get the path to the install-uuid.json file
/// Location: ~/Library/Application Support/Pane/install-uuid.json
pub fn get_install_uuid_path() -> PathBuf {
    // Allow test override
    if let Ok(test_root) = std::env::var("PANE_TEST_ROOT") {
        let path = PathBuf::from(&test_root);
        // Ensure test directory exists
        if let Err(e) = fs::create_dir_all(&path) {
            eprintln!("[INSTALL_UUID] Failed to create test directory: {}", e);
        }
        return path.join("install-uuid.json");
    }
    
    // Production path: ~/Library/Application Support/Pane/
    let app_support = dirs::config_dir()
        .expect("Failed to get config directory")
        .join("Pane");
    
    // Ensure directory exists
    if let Err(e) = fs::create_dir_all(&app_support) {
        eprintln!("[INSTALL_UUID] Failed to create app support directory: {}", e);
    }
    
    app_support.join("install-uuid.json")
}

/// Get or create the install UUID
/// Returns the UUID string, generating and storing it if it doesn't exist
pub fn get_or_create_install_uuid() -> Result<String, String> {
    let path = get_install_uuid_path();
    
    // Try to read existing UUID
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(contents) => {
                match serde_json::from_str::<InstallUuid>(&contents) {
                    Ok(data) => {
                        eprintln!("[INSTALL_UUID] Loaded existing UUID: {}", data.install_uuid);
                        return Ok(data.install_uuid);
                    }
                    Err(e) => {
                        eprintln!("[INSTALL_UUID] Failed to parse existing UUID file: {}", e);
                        // Fall through to regenerate
                    }
                }
            }
            Err(e) => {
                eprintln!("[INSTALL_UUID] Failed to read UUID file: {}", e);
                // Fall through to regenerate
            }
        }
    }
    
    // Generate new UUID
    let uuid = Uuid::new_v4().to_string();
    let created_at = Utc::now().to_rfc3339();
    
    let data = InstallUuid {
        install_uuid: uuid.clone(),
        created_at,
    };
    
    // Atomic write: tempfile + rename (§2.3A)
    let json = serde_json::to_string_pretty(&data)
        .map_err(|e| format!("Failed to serialize install UUID: {}", e))?;
    
    let temp_path = path.with_extension("tmp");
    
    fs::write(&temp_path, json)
        .map_err(|e| format!("Failed to write temp install UUID file: {}", e))?;
    
    fs::rename(&temp_path, &path)
        .map_err(|e| format!("Failed to rename install UUID file: {}", e))?;
    
    eprintln!("[INSTALL_UUID] Generated and stored new UUID: {}", uuid);
    
    Ok(uuid)
}

/// Read the install UUID without creating it (used during restore and restore tests).
#[cfg_attr(not(test), allow(dead_code))]
pub fn read_install_uuid() -> Result<Option<InstallUuid>, String> {
    let path = get_install_uuid_path();
    
    if !path.exists() {
        return Ok(None);
    }
    
    let contents = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read install UUID file: {}", e))?;
    
    let data = serde_json::from_str::<InstallUuid>(&contents)
        .map_err(|e| format!("Failed to parse install UUID file: {}", e))?;
    
    Ok(Some(data))
}

/// Write install UUID (used during restore)
pub fn write_install_uuid(data: &InstallUuid) -> Result<(), String> {
    let path = get_install_uuid_path();
    
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create install UUID directory: {}", e))?;
    }
    
    let json = serde_json::to_string_pretty(data)
        .map_err(|e| format!("Failed to serialize install UUID: {}", e))?;
    
    // Atomic write: tempfile + rename (§2.3A)
    let temp_path = path.with_extension("tmp");
    
    fs::write(&temp_path, json)
        .map_err(|e| format!("Failed to write temp install UUID file: {}", e))?;
    
    fs::rename(&temp_path, &path)
        .map_err(|e| format!("Failed to rename install UUID file: {}", e))?;
    
    eprintln!("[INSTALL_UUID] Restored UUID: {}", data.install_uuid);
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use serial_test::serial;
    
    fn setup_test_env() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        // Create the expected subdirectory structure
        fs::create_dir_all(temp_dir.path().join("Pane")).unwrap();
        std::env::set_var("PANE_TEST_ROOT", temp_dir.path().join("Pane"));
        temp_dir
    }
    
    #[test]
    #[serial(pane_test_root)]
    fn test_install_uuid_generation() {
        let _temp = setup_test_env();
        
        let uuid = get_or_create_install_uuid().unwrap();
        
        // Verify UUID format (should be valid v4 UUID)
        assert!(Uuid::parse_str(&uuid).is_ok());
        
        // Verify file exists
        let path = get_install_uuid_path();
        assert!(path.exists());
        
        // Verify file contents
        let contents = fs::read_to_string(&path).unwrap();
        let data: InstallUuid = serde_json::from_str(&contents).unwrap();
        assert_eq!(data.install_uuid, uuid);
        assert!(!data.created_at.is_empty());
    }
    
    #[test]
    #[serial(pane_test_root)]
    fn test_install_uuid_idempotent() {
        let _temp = setup_test_env();
        
        let uuid1 = get_or_create_install_uuid().unwrap();
        let uuid2 = get_or_create_install_uuid().unwrap();
        
        // Should return the same UUID on second call
        assert_eq!(uuid1, uuid2);
    }
    
    #[test]
    #[serial(pane_test_root)]
    fn test_install_uuid_atomic_write() {
        let _temp = setup_test_env();
        
        let uuid = get_or_create_install_uuid().unwrap();
        
        let path = get_install_uuid_path();
        assert!(path.exists());
        
        // Verify no temp file left behind
        let temp_path = path.with_extension("tmp");
        assert!(!temp_path.exists());
        
        // Verify readable
        let data = read_install_uuid().unwrap();
        assert!(data.is_some());
        assert_eq!(data.unwrap().install_uuid, uuid);
    }
    
    #[test]
    #[serial(pane_test_root)]
    fn test_read_install_uuid_nonexistent() {
        let _temp = setup_test_env();
        
        let result = read_install_uuid().unwrap();
        assert!(result.is_none());
    }
    
    #[test]
    #[serial(pane_test_root)]
    fn test_write_install_uuid() {
        let _temp = setup_test_env();
        
        let test_data = InstallUuid {
            install_uuid: "test-uuid-12345".to_string(),
            created_at: "2026-05-07T12:00:00Z".to_string(),
        };
        
        write_install_uuid(&test_data).unwrap();
        
        let read_data = read_install_uuid().unwrap().unwrap();
        assert_eq!(read_data.install_uuid, test_data.install_uuid);
        assert_eq!(read_data.created_at, test_data.created_at);
    }
}
