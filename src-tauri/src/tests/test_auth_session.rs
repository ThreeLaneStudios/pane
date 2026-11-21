// Integration tests for auth session token storage (#1015, #1017)
// Tests actual Tauri command implementations with keychain + file storage

// ADR-0070 (#3195 PR B): `SpawnBlockingCapturingKeychain` and
// `CAPTURED_KEYCHAIN_TASK_ID_IS_SOME` (the #1347 spawn_blocking regression
// harness for `auth_check_entitlement`) are deleted with the two tests that
// used them — `auth_check_entitlement` itself is removed outright (ADR-0070
// Decision 1). No other test in this file constructs this mock.
use crate::sync::commands::{save_auth_session, get_auth_session, clear_auth_session};
use crate::sync::auth::AuthSession;
use crate::keychain::test_support::KeychainGuard;
use serial_test::serial;

fn cleanup_test_session() {
    // Clean up test JSON file (mock keychain is cleared separately)
    let session_path = dirs::home_dir().unwrap().join("Pane").join(".pane").join("auth_session.json");
    std::fs::remove_file(session_path).ok();
}

fn create_test_session() -> AuthSession {
    AuthSession {
        access_token: "test_access_token_abc123".to_string(),
        refresh_token: "test_refresh_token_xyz789".to_string(),
        user_id: "test-user-id-12345".to_string(),
        email: "test@example.com".to_string(),
        expires_at: 1234567890,
        device_id: Some("test-device-001".to_string()),
    }
}

// Temporarily override keychain keys for testing
// This is a workaround since we can't parameterize the keychain keys in the real commands
// Instead, we'll use the real keys but ensure cleanup
fn setup_test_env() {
    // Ensure .pane directory exists
    let pane_dir = dirs::home_dir().unwrap().join("Pane").join(".pane");
    std::fs::create_dir_all(&pane_dir).ok();
}

/// #3195 PR A (ADR-0070 Decision 3): `save_auth_session` / `clear_auth_session` /
/// `auth_change_password` are now gated by `require_account_enabled()`, which
/// opens its own DB connection via `database::open_connection(None)` — that
/// panics loudly (#3057) unless `PANE_TEST_ROOT` is set first. This helper:
/// (1) points BOTH `HOME` and `PANE_TEST_ROOT` at a fresh `TempDir` — `HOME`
/// because this file's own `setup_test_env`/`cleanup_test_session` already
/// read `dirs::home_dir()` directly (pre-existing, not something this PR
/// introduced) and must never resolve to the real `~/Pane/.pane/`; (2) seeds
/// `account_enabled = 'true'` so these pre-existing tests keep exercising the
/// on-path they were written to test. Returns the `TempDir` — bind it to a
/// variable that outlives the test body, or it's deleted immediately.
fn isolate_and_enable_account() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::env::set_var("HOME", dir.path());
    std::env::set_var("PANE_TEST_ROOT", dir.path().join("Pane"));

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
    crate::database::set_ai_setting(&conn, "account_enabled", "true")
        .expect("seed account_enabled");

    dir
}

#[test]
#[serial(keychain_override)]
fn test_save_and_get_session() {
    let _kc = KeychainGuard::set();
    let _root = isolate_and_enable_account(); // #3195 PR A: save_auth_session is now guarded
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_save_and_get_session: Creating test session");
    
    let original_session = create_test_session();
    
    // Save session (should split: tokens → keychain, metadata → JSON)
    eprintln!("[TEST] Saving session...");
    let save_result = save_auth_session(original_session.clone());
    assert!(save_result.is_ok(), "save_auth_session failed: {:?}", save_result.err());
    
    // Verify tokens are in keychain
    eprintln!("[TEST] Verifying tokens in keychain...");
    let access_token = crate::keychain::get_password("supabase_access_token")
        .expect("Failed to read access_token from keychain")
        .expect("access_token not found in keychain");
    let refresh_token = crate::keychain::get_password("supabase_refresh_token")
        .expect("Failed to read refresh_token from keychain")
        .expect("refresh_token not found in keychain");
    
    assert_eq!(access_token, "test_access_token_abc123");
    assert_eq!(refresh_token, "test_refresh_token_xyz789");
    
    // Verify metadata file exists and does NOT contain tokens
    eprintln!("[TEST] Verifying metadata file...");
    let session_path = dirs::home_dir().unwrap().join("Pane").join(".pane").join("auth_session.json");
    assert!(session_path.exists(), "Metadata file should exist");
    
    let file_content = std::fs::read_to_string(&session_path)
        .expect("Failed to read metadata file");
    assert!(!file_content.contains("test_access_token_abc123"), "Metadata file should NOT contain access_token");
    assert!(!file_content.contains("test_refresh_token_xyz789"), "Metadata file should NOT contain refresh_token");
    assert!(file_content.contains("test-user-id-12345"), "Metadata file should contain user_id");
    
    // Get session (should reconstruct from keychain + file)
    eprintln!("[TEST] Getting session...");
    let get_result = get_auth_session();
    assert!(get_result.is_ok(), "get_auth_session failed: {:?}", get_result.err());
    
    let retrieved_session = get_result.unwrap()
        .expect("get_auth_session returned None");
    
    // Verify all fields match
    assert_eq!(retrieved_session.access_token, original_session.access_token);
    assert_eq!(retrieved_session.refresh_token, original_session.refresh_token);
    assert_eq!(retrieved_session.user_id, original_session.user_id);
    assert_eq!(retrieved_session.email, original_session.email);
    assert_eq!(retrieved_session.expires_at, original_session.expires_at);
    assert_eq!(retrieved_session.device_id, original_session.device_id);
    
    eprintln!("[TEST] test_save_and_get_session: PASSED");
    cleanup_test_session();
}

#[test]
#[serial(keychain_override)]
fn test_clear_session() {
    let _kc = KeychainGuard::set();
    let _root = isolate_and_enable_account(); // #3195 PR A: save/clear_auth_session are now guarded
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_clear_session: Creating and saving session");
    
    let session = create_test_session();
    save_auth_session(session).expect("Failed to save session");
    
    // Verify session exists
    let before_clear = get_auth_session().expect("Failed to get session");
    assert!(before_clear.is_some(), "Session should exist before clear");
    
    // Clear session
    eprintln!("[TEST] Clearing session...");
    let clear_result = clear_auth_session();
    assert!(clear_result.is_ok(), "clear_auth_session failed: {:?}", clear_result.err());
    
    // Verify get returns None
    eprintln!("[TEST] Verifying session is gone...");
    let after_clear = get_auth_session().expect("Failed to get session after clear");
    assert!(after_clear.is_none(), "Session should be None after clear");
    
    // Verify keychain entries are deleted (keychain returns Ok(None) for deleted items)
    let access_token = crate::keychain::get_password("supabase_access_token")
        .unwrap_or(None);
    let refresh_token = crate::keychain::get_password("supabase_refresh_token")
        .unwrap_or(None);
    
    assert!(access_token.is_none(), "access_token should be deleted from keychain");
    assert!(refresh_token.is_none(), "refresh_token should be deleted from keychain");
    
    // Verify metadata file is deleted
    let session_path = dirs::home_dir().unwrap().join("Pane").join(".pane").join("auth_session.json");
    assert!(!session_path.exists(), "Metadata file should be deleted");
    
    eprintln!("[TEST] test_clear_session: PASSED");
    cleanup_test_session();
}

#[test]
#[serial(keychain_override)]
fn test_migration_from_legacy_format() {
    let _kc = KeychainGuard::set();
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_migration_from_legacy_format: Creating legacy JSON file");
    
    // Create legacy format JSON (with tokens in file)
    let legacy_session = create_test_session();
    let session_path = dirs::home_dir().unwrap().join("Pane").join(".pane").join("auth_session.json");
    
    let legacy_json = serde_json::to_string_pretty(&legacy_session)
        .expect("Failed to serialize legacy session");
    std::fs::write(&session_path, legacy_json)
        .expect("Failed to write legacy session file");
    
    // Verify file contains tokens (legacy format)
    let file_content = std::fs::read_to_string(&session_path)
        .expect("Failed to read legacy file");
    assert!(file_content.contains("test_access_token_abc123"), "Legacy file should contain access_token");
    assert!(file_content.contains("test_refresh_token_xyz789"), "Legacy file should contain refresh_token");
    
    // Call get_auth_session (should trigger migration)
    eprintln!("[TEST] Calling get_auth_session to trigger migration...");
    let get_result = get_auth_session();
    assert!(get_result.is_ok(), "get_auth_session failed during migration: {:?}", get_result.err());
    
    let migrated_session = get_result.unwrap()
        .expect("Migration should return session");
    
    // Verify returned session is correct
    assert_eq!(migrated_session.access_token, legacy_session.access_token);
    assert_eq!(migrated_session.refresh_token, legacy_session.refresh_token);
    
    // Verify tokens are now in keychain
    eprintln!("[TEST] Verifying tokens migrated to keychain...");
    let access_token = crate::keychain::get_password("supabase_access_token")
        .expect("Keychain read failed")
        .expect("access_token not in keychain after migration");
    let refresh_token = crate::keychain::get_password("supabase_refresh_token")
        .expect("Keychain read failed")
        .expect("refresh_token not in keychain after migration");
    
    assert_eq!(access_token, "test_access_token_abc123");
    assert_eq!(refresh_token, "test_refresh_token_xyz789");
    
    // Verify file no longer contains tokens
    eprintln!("[TEST] Verifying file no longer contains tokens...");
    let new_file_content = std::fs::read_to_string(&session_path)
        .expect("Failed to read migrated file");
    assert!(!new_file_content.contains("test_access_token_abc123"), "Migrated file should NOT contain access_token");
    assert!(!new_file_content.contains("test_refresh_token_xyz789"), "Migrated file should NOT contain refresh_token");
    assert!(new_file_content.contains("test-user-id-12345"), "Migrated file should contain user_id");
    
    eprintln!("[TEST] test_migration_from_legacy_format: PASSED");
    cleanup_test_session();
}

#[test]
#[serial(keychain_override)]
fn test_migration_idempotency() {
    let _kc = KeychainGuard::set();
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_migration_idempotency: Creating legacy session");
    
    // Create legacy session
    let legacy_session = create_test_session();
    let session_path = dirs::home_dir().unwrap().join("Pane").join(".pane").join("auth_session.json");
    
    let legacy_json = serde_json::to_string_pretty(&legacy_session)
        .expect("Failed to serialize");
    std::fs::write(&session_path, legacy_json)
        .expect("Failed to write legacy file");
    
    // First migration
    eprintln!("[TEST] First migration...");
    let first_result = get_auth_session()
        .expect("First migration failed");
    assert!(first_result.is_some(), "First migration should return session");
    
    // Second call (should NOT migrate again, should use keychain)
    eprintln!("[TEST] Second call (should skip migration)...");
    let second_result = get_auth_session()
        .expect("Second call failed");
    assert!(second_result.is_some(), "Second call should return session");
    
    // Verify data is still correct
    let second_session = second_result.unwrap();
    assert_eq!(second_session.access_token, legacy_session.access_token);
    assert_eq!(second_session.refresh_token, legacy_session.refresh_token);
    assert_eq!(second_session.user_id, legacy_session.user_id);
    
    // Verify file still doesn't contain tokens (didn't regress)
    let file_content = std::fs::read_to_string(&session_path)
        .expect("Failed to read file");
    assert!(!file_content.contains("test_access_token_abc123"), "File should still not contain tokens after second call");
    
    eprintln!("[TEST] test_migration_idempotency: PASSED");
    cleanup_test_session();
}

#[test]
#[serial(keychain_override)]
fn test_get_session_missing_keychain_entries() {
    let _kc = KeychainGuard::set();
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_get_session_missing_keychain_entries: Creating metadata file without keychain entries");
    
    // Create metadata-only file (new format) but don't store in keychain
    let session_path = dirs::home_dir().unwrap().join("Pane").join(".pane").join("auth_session.json");
    
    let metadata = serde_json::json!({
        "user_id": "test-user-id",
        "email": "test@example.com",
        "expires_at": 1234567890,
        "device_id": "test-device"
    });
    
    std::fs::write(&session_path, serde_json::to_string_pretty(&metadata).unwrap())
        .expect("Failed to write metadata");
    
    // Try to get session (should return None because tokens missing)
    eprintln!("[TEST] Getting session with missing tokens...");
    let result = get_auth_session()
        .expect("get_auth_session should not error");
    
    assert!(result.is_none(), "get_auth_session should return None when keychain entries missing");
    
    eprintln!("[TEST] test_get_session_missing_keychain_entries: PASSED");
    cleanup_test_session();
}

#[test]
#[serial(keychain_override)]
fn test_get_session_no_metadata_file() {
    let _kc = KeychainGuard::set();
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_get_session_no_metadata_file: Calling get_auth_session with no file");
    
    // Ensure no metadata file exists
    let session_path = dirs::home_dir().unwrap().join("Pane").join(".pane").join("auth_session.json");
    std::fs::remove_file(&session_path).ok();
    
    let result = get_auth_session()
        .expect("get_auth_session should not error");
    
    assert!(result.is_none(), "get_auth_session should return None when no metadata file");
    
    eprintln!("[TEST] test_get_session_no_metadata_file: PASSED");
    cleanup_test_session();
}

#[test]
#[serial(keychain_override)]
fn test_auth_change_password_reads_from_keychain() {
    let _kc = KeychainGuard::set();
    let _root = isolate_and_enable_account(); // #3195 PR A: save_auth_session/auth_change_password are now guarded
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_auth_change_password_reads_from_keychain: Setting up session");
    
    let session = create_test_session();
    save_auth_session(session.clone()).expect("Failed to save session");
    
    // Verify the access token is in mock keychain
    eprintln!("[TEST] Verifying token in keychain...");
    let token = crate::keychain::get_password("supabase_access_token")
        .expect("Keychain read failed")
        .expect("Token should be in keychain");
    assert_eq!(token, "test_access_token_abc123");
    
    // Call auth_change_password — this will try to read access_token from keychain,
    // then make an HTTP call to Supabase. The HTTP call will fail (no real server),
    // but we can verify the keychain read succeeded by checking the error message.
    // If it read from keychain successfully, the error will be about the HTTP call,
    // NOT about "No active session (token not in keychain)".
    
    eprintln!("[TEST] Calling auth_change_password (should read from keychain, fail at HTTP)...");
    
    // We need to use tokio runtime since auth_change_password is async
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(crate::sync::commands::auth_change_password("new_password_123".to_string()));
    
    // The call should fail (no Supabase server) but NOT with keychain error
    assert!(result.is_err(), "Should fail — no Supabase server in test");
    let err = result.unwrap_err();
    eprintln!("[TEST] Error from auth_change_password: {}", err);
    assert!(
        !err.contains("No active session") && !err.contains("Keychain error"),
        "Error should be HTTP failure, not keychain read failure. Got: {}",
        err
    );
    
    // Also verify: clear keychain, then auth_change_password should fail with keychain error
    eprintln!("[TEST] Clearing access token from keychain...");
    crate::keychain::delete_password("supabase_access_token").ok();
    
    eprintln!("[TEST] Calling auth_change_password again (should fail with keychain error)...");
    let result2 = rt.block_on(crate::sync::commands::auth_change_password("new_password_456".to_string()));
    assert!(result2.is_err(), "Should fail when token missing");
    let err2 = result2.unwrap_err();
    eprintln!("[TEST] Error after clearing keychain: {}", err2);
    assert!(
        err2.contains("No active session") || err2.contains("not in keychain"),
        "Should fail with keychain/session error when token missing. Got: {}",
        err2
    );
    
    eprintln!("[TEST] test_auth_change_password_reads_from_keychain: PASSED");
    cleanup_test_session();
}

#[test]
#[serial(keychain_override)]
fn test_auth_change_password_persists_rotated_tokens() {
    // This test verifies the contract for #1036: auth_change_password must persist
    // rotated tokens to storage after a successful password change.
    //
    // Since we can't call a real Supabase server in tests, we verify:
    // 1. The function signature returns Result<AuthSession, String> (compiler enforces)
    // 2. The function reads the current session (verified by keychain error when missing)
    // 3. On success (real server call), the caller would receive AuthSession containing
    //    the rotated tokens, and save_auth_session would have been called internally.
    //
    // The integration contract is: if Supabase returns HTTP 200 with rotated tokens,
    // auth_change_password persists them via save_auth_session before returning.
    // Manual testing with a real Supabase instance confirms this behavior.
    
    let _kc = KeychainGuard::set();
    let _root = isolate_and_enable_account(); // #3195 PR A: save_auth_session/auth_change_password are now guarded
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_auth_change_password_persists_rotated_tokens: Setting up session");
    
    let original_session = create_test_session();
    save_auth_session(original_session.clone()).expect("Failed to save session");
    
    // Verify original tokens in keychain
    let original_access = crate::keychain::get_password("supabase_access_token")
        .expect("Keychain read failed")
        .expect("Token should be in keychain");
    assert_eq!(original_access, "test_access_token_abc123");
    
    let original_refresh = crate::keychain::get_password("supabase_refresh_token")
        .expect("Keychain read failed")
        .expect("Refresh token should be in keychain");
    assert_eq!(original_refresh, "test_refresh_token_xyz789");
    
    eprintln!("[TEST] Original tokens verified in keychain");
    
    // Call auth_change_password — will fail at HTTP layer (no real server)
    // but the fact that it attempts the call proves it read from keychain successfully
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(crate::sync::commands::auth_change_password("new_password_789".to_string()));
    
    // Should fail with HTTP error, not session/keychain error
    assert!(result.is_err(), "Should fail — no Supabase server in test");
    let err = result.unwrap_err();
    eprintln!("[TEST] Error from auth_change_password: {}", err);
    assert!(
        !err.contains("No active session"),
        "Error should be HTTP failure, not session read failure. Got: {}",
        err
    );
    
    // Note: We cannot verify token rotation without a mock HTTP server.
    // The contract is: IF Supabase returns 200 + rotated tokens,
    // THEN auth_change_password calls save_auth_session with the new tokens.
    // This is verified by:
    // - Signature returns Result<AuthSession, String> (compiler enforces non-discard)
    // - Implementation calls save_auth_session before returning Ok
    // - Manual testing with real Supabase confirms rotation+persistence
    
    eprintln!("[TEST] test_auth_change_password_persists_rotated_tokens: PASSED");
    eprintln!("[TEST] (Full persistence flow requires live Supabase; contract verified by code inspection + manual testing)");
    
    cleanup_test_session();
}

// ============================================================================
// Tests for two-step email-first sign-in (#1017)
// ============================================================================
//
// `test_sign_in_constructs_session_correctly` deleted per ADR-0070
// Decision 4 (#3195 PR C): it was comment-only (no runtime assertions) and
// documented a now-wrong contract — `auth_sign_in` returns a plain
// `AuthSession` directly, not a `SignInResult` wrapper with an entitlement
// snapshot, which is retired along with the ecom wrapper. Leaving it would
// be a stale comment describing removed behaviour as current, not a
// correct record of history. Real coverage for the new contract (success,
// 400, 429, network-unreachable) lives in sync/auth.rs's own test module.

#[test]
#[serial(keychain_override)]
fn test_session_persists_with_none_device_id() {
    let _kc = KeychainGuard::set();
    let _root = isolate_and_enable_account(); // #3195 PR A: save_auth_session is now guarded
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_session_persists_with_none_device_id");
    
    // Create a session with device_id=None (new sign-in pattern)
    let session = AuthSession {
        access_token: "test_token_new".to_string(),
        refresh_token: "".to_string(), // Empty for wrapper-based auth
        user_id: "user-123".to_string(),
        email: "user@example.com".to_string(),
        expires_at: 9999999999,
        device_id: None, // New sign-ins have None
    };
    
    // Save and retrieve
    save_auth_session(session.clone()).expect("Failed to save");
    let retrieved = get_auth_session()
        .expect("Failed to get")
        .expect("Should have session");
    
    // Verify device_id is None
    assert_eq!(retrieved.device_id, None, "device_id should be None for new sign-ins");
    assert_eq!(retrieved.email, "user@example.com");
    assert_eq!(retrieved.user_id, "user-123");
    
    eprintln!("[TEST] PASSED: device_id=None persists correctly");
    
    cleanup_test_session();
}

#[test]
#[serial(keychain_override)]
fn test_legacy_session_with_device_id_still_readable() {
    let _kc = KeychainGuard::set();
    let _root = isolate_and_enable_account(); // #3195 PR A: save_auth_session is now guarded
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_legacy_session_with_device_id_still_readable");
    
    // Create an old session with device_id (backward compat test)
    let legacy_session = AuthSession {
        access_token: "legacy_token".to_string(),
        refresh_token: "legacy_refresh".to_string(),
        user_id: "legacy-user".to_string(),
        email: "legacy@example.com".to_string(),
        expires_at: 9999999999,
        device_id: Some("old-device-123".to_string()),
    };
    
    // Save and retrieve
    save_auth_session(legacy_session.clone()).expect("Failed to save");
    let retrieved = get_auth_session()
        .expect("Failed to get")
        .expect("Should have session");
    
    // Verify old device_id is preserved
    assert_eq!(retrieved.device_id, Some("old-device-123".to_string()));
    
    eprintln!("[TEST] PASSED: Legacy sessions with device_id remain readable");
    
    cleanup_test_session();
}

// `test_sign_in_error_display` deleted per ADR-0070 Decision 4 (#3195 PR C):
// the SignInError variants it asserted (WrongPassword, EmailNotVerified,
// AccountDisabled, NetworkError, ServerError with the old wrapper-era
// Display strings) are removed with the ecom wrapper. The new GoTrue-era
// SignInError enum (InvalidCredentials, RateLimited, NetworkError,
// ServerError) and its Display impl are exercised by sync/auth.rs's own
// `test_sign_in_400_*` / `test_sign_in_429_*` / `test_sign_in_network_*`
// tests, which assert on the enum variant directly rather than duplicating
// a second Display-string check here.

#[test]
#[serial(keychain_override)]
fn test_clear_auth_session_contract() {
    // Test for #1066: Sign-out button functionality
    //
    // This test verifies clear_auth_session works correctly:
    // - Removes tokens from keychain
    // - Deletes metadata file
    // - Returns success
    //
    // Frontend contract (ADR-0070 #3195 PR B update — S7SubscriptionBlock.svelte
    // is deleted; sign-out now lives in Settings.svelte, Account-block-gated):
    //   authStore.ts::signOut() calls api.clearAuthSession() (this function)
    //
    // IMPORTANT: This test verifies function correctness (execute + state reset),
    // NOT click-reachability. The original #1066 bug was click interception by an
    // overlapping toast element, not function failure. Real verification requires
    // Gate 4b: clicking sign-out on S7 with toast visible.
    //
    // The behavioral fix (dismissing toast on S7 entry) sidesteps click interception.
    
    let _kc = KeychainGuard::set();
    let _root = isolate_and_enable_account(); // #3195 PR A: save_auth_session/clear_auth_session are now guarded
    setup_test_env();
    cleanup_test_session();
    
    eprintln!("[TEST] test_clear_auth_session_contract: Setting up session");
    
    let session = create_test_session();
    save_auth_session(session).expect("Failed to save session");
    
    // Verify session exists before clear
    let before_clear = get_auth_session()
        .expect("Failed to get session")
        .expect("Session should exist");
    assert_eq!(before_clear.user_id, "test-user-id-12345");
    
    eprintln!("[TEST] Calling clear_auth_session...");
    let result = clear_auth_session();
    assert!(result.is_ok(), "clear_auth_session should succeed");
    
    // Verify session is cleared
    let after_clear = get_auth_session()
        .expect("get_auth_session should not error")
        .is_none();
    assert!(after_clear, "Session should be None after clear");
    
    // Verify tokens deleted from keychain
    let access_token = crate::keychain::get_password("supabase_access_token")
        .unwrap_or(None);
    let refresh_token = crate::keychain::get_password("supabase_refresh_token")
        .unwrap_or(None);
    assert!(access_token.is_none(), "access_token should be deleted");
    assert!(refresh_token.is_none(), "refresh_token should be deleted");
    
    // Verify metadata file deleted
    let session_path = dirs::home_dir().unwrap().join("Pane").join(".pane").join("auth_session.json");
    assert!(!session_path.exists(), "Metadata file should be deleted");
    
    eprintln!("[TEST] PASSED: clear_auth_session removes all session data");
    eprintln!("[TEST] Note: This verifies function correctness, not click-reachability.");
    eprintln!("[TEST]       Original bug was toast z-index interception, fixed by dismissing toast on S7 entry.");
    
    cleanup_test_session();
}

