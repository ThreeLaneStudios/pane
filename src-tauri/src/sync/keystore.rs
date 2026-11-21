// sync/keystore.rs — ADR-0044 Addendum A: Sync master key keychain storage (#1498)
// Built: 2026-06-17 (schema v64, issue #1498)
//
// The sync master key moves from ~/Pane/.pane/sync_key.bin to OS keychain.
// Keychain account: "sync_master_key", service "com.threelane.pane".
//
// Migration-aware read order (Decision 5, Addendum A):
//   Flag NOT set → keychain first, fall back to sync_key.bin
//   Flag IS set  → keychain only; miss = key not available
//
// The migration flag is a DB row (migration_flags table, flag="sync_master_key_v1").
// An in-memory AtomicBool caches the flag value so load/store/delete need no DB
// parameter — migration is guaranteed to run before any consumer (lib.rs startup order).
//
// delete_master_key() post-migration: removes keychain entry, NOT file. Flag preserved.
// store_master_key() post-migration: writes directly to keychain.

use std::path::PathBuf;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use base64::{Engine as _, engine::general_purpose};

// ─── Keychain Account Constant ─────────────────────────────────────────────

/// Keychain account label for the sync master key.
/// Service is the shared "com.threelane.pane" identifier.
/// Not email-scoped — one master key per device.
pub const KEYCHAIN_ACCOUNT: &str = "sync_master_key";

// ─── In-Memory Migration Flag ──────────────────────────────────────────────

/// Cached migration-complete state. Set by run_sync_key_migration() at startup
/// (before any consumer is reached). Consumers call is_migration_complete() to
/// branch between keychain-only (post) and file-fallback (pre) read paths.
static MIGRATION_COMPLETE: AtomicBool = AtomicBool::new(false);

/// Mark migration complete. Called by keystore_migration::run_sync_key_migration()
/// after the DB flag is set. All subsequent keystore operations use keychain-only paths.
pub fn mark_migration_complete() {
    MIGRATION_COMPLETE.store(true, Ordering::SeqCst);
    eprintln!("[KEYSTORE] Migration flag set — post-migration regime active");
}

/// Check whether migration has completed (in-memory cache).
pub fn is_migration_complete() -> bool {
    MIGRATION_COMPLETE.load(Ordering::Acquire)
}

/// Reset the in-memory migration flag (test-only).
#[cfg(test)]
pub fn reset_migration_flag_for_test() {
    MIGRATION_COMPLETE.store(false, Ordering::SeqCst);
}

// ─── File Path (legacy) ────────────────────────────────────────────────────

/// Path to the legacy sync_key.bin plaintext file.
/// Only read/written pre-migration. Post-migration this file does not exist
/// (deleted by migration), and is never re-adopted if found (Decision 5).
/// pub(super) so keystore_migration.rs can use it without duplicating the path logic.
pub(super) fn key_file_path() -> PathBuf {
    // Test override: lets tests point at a temp file instead of the real path
    #[cfg(test)]
    {
        if let Ok(guard) = TEST_KEY_PATH_OVERRIDE.lock() {
            if let Some(ref p) = *guard {
                return p.clone();
            }
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("Pane")
        .join(".pane")
        .join("sync_key.bin")
}

#[cfg(test)]
static TEST_KEY_PATH_OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Override the key file path for tests. Pass None to clear.
#[cfg(test)]
pub fn set_test_key_path(path: Option<PathBuf>) {
    *TEST_KEY_PATH_OVERRIDE.lock().unwrap() = path;
}

// ─── Internal Helpers ──────────────────────────────────────────────────────

/// Read and decode the master key from the legacy file.
/// Returns None if the file does not exist.
fn load_from_file() -> Result<Option<[u8; 32]>, String> {
    let path = key_file_path();
    if !path.exists() {
        return Ok(None);
    }

    let encoded = fs::read_to_string(&path)
        .map_err(|e| format!("[KEYSTORE] Failed to read sync_key.bin: {}", e))?;

    decode_base64_key(&encoded)
}

/// Read and decode the master key from the OS keychain.
/// Returns None if the account does not exist in the keychain.
fn load_from_keychain() -> Result<Option<[u8; 32]>, String> {
    match crate::keychain::get_password(KEYCHAIN_ACCOUNT) {
        Ok(Some(ref encoded)) => decode_base64_key(encoded),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("[KEYSTORE] Keychain read error: {}", e)),
    }
}

/// Decode a base64 string into a 32-byte key array.
fn decode_base64_key(encoded: &str) -> Result<Option<[u8; 32]>, String> {
    let decoded = general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|e| format!("[KEYSTORE] Failed to decode key: {}", e))?;

    if decoded.len() != 32 {
        return Err(format!(
            "[KEYSTORE] Invalid key length: expected 32, got {}",
            decoded.len()
        ));
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(&decoded);
    Ok(Some(key))
}

// ─── Public API ────────────────────────────────────────────────────────────

/// Store the sync master key.
///
/// Post-migration (flag set): writes directly to keychain.
/// Pre-migration (flag not set): writes to sync_key.bin (legacy path).
///
/// Called from init_encryption Tauri command when user sets up sync encryption.
/// Post-migration this always writes to keychain — the file path is never used.
pub fn store_master_key(master_key: &[u8; 32]) -> Result<(), String> {
    let encoded = general_purpose::STANDARD.encode(master_key);

    if is_migration_complete() {
        // Post-migration: write directly to keychain
        crate::keychain::store_password(KEYCHAIN_ACCOUNT, &encoded)
            .map_err(|e| format!("[KEYSTORE] Failed to store master key in keychain: {}", e))?;
        eprintln!("[KEYSTORE] Master key stored in keychain (post-migration)");
    } else {
        // Pre-migration: write to sync_key.bin (legacy path)
        let path = key_file_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("[KEYSTORE] Failed to create key directory: {}", e))?;
        }

        fs::write(&path, &encoded)
            .map_err(|e| format!("[KEYSTORE] Failed to write sync_key.bin: {}", e))?;

        // Restrict permissions (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|e| format!("[KEYSTORE] Failed to set key permissions: {}", e))?;
        }

        eprintln!("[KEYSTORE] Master key stored in sync_key.bin (pre-migration)");
    }

    Ok(())
}

/// Load the sync master key.
///
/// Migration-aware read order (ADR-0044 Addendum A, Decision 5):
///   Post-migration (flag set)  → keychain only; miss = Ok(None) (key not available)
///   Pre-migration (flag unset) → keychain first; fall back to sync_key.bin
///
/// The file fallback (pre-migration) exists so the app can decrypt data from before
/// the migration ran. Once the flag is set, sync_key.bin is gone and the fallback
/// is never reached.
pub fn load_master_key() -> Result<Option<[u8; 32]>, String> {
    if is_migration_complete() {
        // Post-migration: keychain only. A miss means the key is unavailable.
        // Decision 5: MUST NOT fall back to sync_key.bin.
        let key = load_from_keychain()?;
        if key.is_none() {
            eprintln!(
                "[KEYSTORE] load_master_key: keychain miss post-migration \
                 — key not available (Decision 5)"
            );
        }
        Ok(key)
    } else {
        // Pre-migration: keychain first (handles State C partial migration),
        // fall back to sync_key.bin.
        let from_keychain = load_from_keychain()?;
        if from_keychain.is_some() {
            eprintln!("[KEYSTORE] load_master_key: loaded from keychain (pre-migration)");
            return Ok(from_keychain);
        }
        eprintln!("[KEYSTORE] load_master_key: keychain miss — falling back to sync_key.bin");
        load_from_file()
    }
}

/// Delete the sync master key (on sign out).
///
/// Post-migration (flag set): deletes the keychain entry.
/// Pre-migration (flag not set): deletes sync_key.bin.
///
/// Does NOT clear the migration flag (Addendum A: a deleted key on a migrated
/// install is "migrated, key now absent" — not a trigger for file re-adoption).
pub fn delete_master_key() -> Result<(), String> {
    if is_migration_complete() {
        // Post-migration: delete from keychain
        match crate::keychain::delete_password(KEYCHAIN_ACCOUNT) {
            Ok(_) => eprintln!("[KEYSTORE] Master key deleted from keychain (post-migration)"),
            Err(e) => eprintln!("[KEYSTORE] Failed to delete master key from keychain: {}", e),
        }
    } else {
        // Pre-migration: delete sync_key.bin
        let path = key_file_path();
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| format!("[KEYSTORE] Failed to delete sync_key.bin: {}", e))?;
            eprintln!("[KEYSTORE] sync_key.bin deleted (pre-migration)");
        }
    }
    Ok(())
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Serialize keystore tests — they share static state (AtomicBool + keychain mock)
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn with_temp_key_file(f: impl FnOnce(&TempDir)) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sync_key.bin");
        set_test_key_path(Some(path));
        f(&dir);
        set_test_key_path(None);
    }

    fn write_test_key_file(path: &std::path::Path, key: &[u8; 32]) {
        let encoded = general_purpose::STANDARD.encode(key);
        fs::write(path, encoded).unwrap();
    }

    // ─── store_master_key pre-migration ───────────────────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_store_pre_migration_writes_file() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        reset_migration_flag_for_test();

        with_temp_key_file(|dir| {
            let path = dir.path().join("sync_key.bin");
            let key = [0x42u8; 32];
            store_master_key(&key).unwrap();
            assert!(path.exists(), "File should be written pre-migration");
        });

    }

    // ─── store_master_key post-migration ──────────────────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_store_post_migration_writes_keychain() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        reset_migration_flag_for_test();
        mark_migration_complete();

        with_temp_key_file(|dir| {
            let path = dir.path().join("sync_key.bin");
            let key = [0xABu8; 32];
            store_master_key(&key).unwrap();

            // File should NOT be written post-migration
            assert!(!path.exists(), "File must not be written post-migration");

            // Key should be in keychain
            let stored = crate::keychain::get_password(KEYCHAIN_ACCOUNT).unwrap();
            assert!(stored.is_some(), "Key should be in keychain post-migration");
        });

        reset_migration_flag_for_test();
    }

    // ─── load_master_key pre-migration: keychain first ────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_load_pre_migration_keychain_first() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        reset_migration_flag_for_test();

        let kc_key = [0x11u8; 32];
        let kc_encoded = general_purpose::STANDARD.encode(&kc_key);
        crate::keychain::store_password(KEYCHAIN_ACCOUNT, &kc_encoded).unwrap();

        with_temp_key_file(|dir| {
            let path = dir.path().join("sync_key.bin");
            let file_key = [0x22u8; 32];
            write_test_key_file(&path, &file_key);

            // Pre-migration: should return keychain value
            let loaded = load_master_key().unwrap().unwrap();
            assert_eq!(loaded, kc_key, "Keychain should win pre-migration when present");
        });

    }

    // ─── load_master_key pre-migration: file fallback ─────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_load_pre_migration_file_fallback() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        reset_migration_flag_for_test();

        // Keychain empty
        with_temp_key_file(|dir| {
            let path = dir.path().join("sync_key.bin");
            let file_key = [0x33u8; 32];
            write_test_key_file(&path, &file_key);

            let loaded = load_master_key().unwrap().unwrap();
            assert_eq!(loaded, file_key, "Should fall back to file pre-migration");
        });

    }

    // ─── load_master_key post-migration: keychain only ────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_load_post_migration_keychain_only() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        reset_migration_flag_for_test();
        mark_migration_complete();

        let kc_key = [0x44u8; 32];
        let kc_encoded = general_purpose::STANDARD.encode(&kc_key);
        crate::keychain::store_password(KEYCHAIN_ACCOUNT, &kc_encoded).unwrap();

        with_temp_key_file(|dir| {
            // Even if file somehow exists, should NOT read it
            let path = dir.path().join("sync_key.bin");
            let stale_key = [0xFFu8; 32];
            write_test_key_file(&path, &stale_key);

            let loaded = load_master_key().unwrap().unwrap();
            assert_eq!(loaded, kc_key, "Post-migration: keychain value, never stale file");
        });

        reset_migration_flag_for_test();
    }

    // ─── load_master_key post-migration miss: returns None ────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_load_post_migration_keychain_miss_returns_none() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        reset_migration_flag_for_test();
        mark_migration_complete();

        // Keychain empty, file also present (State F scenario)
        with_temp_key_file(|dir| {
            let path = dir.path().join("sync_key.bin");
            let stale_key = [0xDEu8; 32];
            write_test_key_file(&path, &stale_key);

            // MUST return None, not the file content (Decision 5 / State F)
            let result = load_master_key().unwrap();
            assert!(
                result.is_none(),
                "Post-migration keychain miss must return None — not the file content (Decision 5)"
            );
        });

        reset_migration_flag_for_test();
    }

    // ─── delete_master_key pre-migration: deletes file ────────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_delete_pre_migration_removes_file() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        reset_migration_flag_for_test();

        with_temp_key_file(|dir| {
            let path = dir.path().join("sync_key.bin");
            let key = [0x55u8; 32];
            write_test_key_file(&path, &key);
            assert!(path.exists());

            delete_master_key().unwrap();
            assert!(!path.exists(), "File should be deleted pre-migration");
        });

    }

    // ─── delete_master_key post-migration: deletes keychain entry ─────────

    #[test]
    #[serial(keychain_override)]
    fn test_delete_post_migration_removes_keychain_not_flag() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let _kc = KeychainGuard::set();
        reset_migration_flag_for_test();
        mark_migration_complete();

        let key = [0x66u8; 32];
        let encoded = general_purpose::STANDARD.encode(&key);
        crate::keychain::store_password(KEYCHAIN_ACCOUNT, &encoded).unwrap();

        // Verify key is there
        assert!(crate::keychain::get_password(KEYCHAIN_ACCOUNT).unwrap().is_some());

        with_temp_key_file(|_dir| {
            delete_master_key().unwrap();

            // Keychain entry should be gone
            let result = crate::keychain::get_password(KEYCHAIN_ACCOUNT).unwrap();
            assert!(result.is_none(), "Keychain entry should be deleted post-migration");

            // In-memory migration flag should still be set
            assert!(
                is_migration_complete(),
                "Migration flag must NOT be cleared by delete_master_key"
            );
        });

        reset_migration_flag_for_test();
    }
}
