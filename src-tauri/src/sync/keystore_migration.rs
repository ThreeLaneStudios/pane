// sync/keystore_migration.rs — ADR-0044 Addendum A: Sync master key migration (#1498)
// Built: 2026-06-17 (schema v64, issue #1498)
//
// Implements the 6-state idempotent migration machine (States A–F) that moves the
// sync master key from ~/Pane/.pane/sync_key.bin into the OS keychain.
//
// Call run_sync_key_migration(conn) at app launch AFTER DB-open, BEFORE sync/notifications.
// Idempotent — fast-paths when DB flag is set (State B).
//
// Migration flag: migration_flags table, flag = "sync_master_key_v1"
// Keychain account: keystore::KEYCHAIN_ACCOUNT ("sync_master_key")
// Service: com.threelane.pane (shared service identifier)

use rusqlite::Connection;
use std::fs;
use base64::{Engine as _, engine::general_purpose};

// ─── Constants ─────────────────────────────────────────────────────────────

/// Row key in migration_flags that records sync master key migration completion.
const MIGRATION_FLAG: &str = "sync_master_key_v1";

// ─── Migration Flag (DB) ───────────────────────────────────────────────────

/// Check whether the DB migration flag is set.
fn is_db_flag_set(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM migration_flags WHERE flag = ?1",
        rusqlite::params![MIGRATION_FLAG],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// Set the DB migration flag with the current UTC timestamp.
/// Also sets the in-memory cache so consumers see the change immediately.
fn set_db_flag(conn: &Connection) {
    match conn.execute(
        "INSERT OR REPLACE INTO migration_flags (flag, completed_at) VALUES (?1, datetime('now'))",
        rusqlite::params![MIGRATION_FLAG],
    ) {
        Ok(_) => {
            eprintln!("[KEYSTORE-MIGRATION] DB flag set ({})", MIGRATION_FLAG);
            super::keystore::mark_migration_complete();
        }
        Err(e) => {
            eprintln!(
                "[KEYSTORE-MIGRATION] WARNING: Failed to set DB flag: {} \
                 — setting in-memory flag anyway (will retry DB next launch)",
                e
            );
            // Still set in-memory flag; a transient DB write failure shouldn't leave
            // the process in a broken read state if the keychain write already succeeded.
            super::keystore::mark_migration_complete();
        }
    }
}

// ─── File Helpers ─────────────────────────────────────────────────────────

/// Read the base64-encoded key string from the legacy file.
/// Returns the raw base64 string (the value to store in keychain).
fn read_file_raw(path: &std::path::Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("[KEYSTORE-MIGRATION] Failed to read sync_key.bin: {}", e))
}

/// Delete the legacy sync_key.bin file, logging any failure (non-fatal — the
/// keychain copy already exists and is confirmed; stale file is just cleanup).
fn delete_file(path: &std::path::Path) {
    match fs::remove_file(path) {
        Ok(_) => eprintln!("[KEYSTORE-MIGRATION] sync_key.bin deleted"),
        Err(e) => eprintln!(
            "[KEYSTORE-MIGRATION] WARNING: Failed to delete sync_key.bin: {} \
             (non-fatal — keychain copy is intact)",
            e
        ),
    }
}

// ─── State F: flag set, file present ──────────────────────────────────────

/// Handle State F (file present after migration flag set).
/// The flag is authoritative: the file is a stale artifact, never re-adopted.
/// Delete it without reading. Keychain state determines what load_master_key() returns.
///
/// Decision 5 / Addendum A:
///   - Keychain present: delete file (stale artifact), keychain is live key
///   - Keychain absent:  delete file (plaintext hazard), surface key-unavailable
///     DO NOT re-adopt or re-migrate the file. The key is genuinely unavailable.
fn handle_state_f(file_path: &std::path::Path) {
    let keychain_present = crate::keychain::get_password(crate::sync::keystore::KEYCHAIN_ACCOUNT)
        .ok()
        .flatten()
        .is_some();

    if keychain_present {
        eprintln!(
            "[KEYSTORE-MIGRATION] State F (flag + file + keychain): \
             file is stale artifact — deleting without reading"
        );
    } else {
        eprintln!(
            "[KEYSTORE-MIGRATION] State F (flag + file + NO keychain): \
             key unavailable — deleting file, NOT re-adopting (Decision 5)"
        );
    }

    delete_file(file_path);
    // The in-memory flag is already set (caller set it before calling us).
    // load_master_key() will return keychain value or None — correct in both cases.
}

// ─── Main Migration Entry Point ────────────────────────────────────────────

/// Run the sync master key migration pass. Called at app launch (integrity-check phase,
/// same as ai::credential_migration::run_credential_migration). Must execute before
/// the sync periodic loop and notification poller reach steady state.
///
/// **Idempotent** — safe to call every launch. Fast-paths on State B (flag already set).
///
/// **State machine** (ADR-0044 Addendum A):
/// - A — file present, no keychain, no flag → copy → confirm → delete file → set flag
/// - B — keychain present, file absent, flag set → no-op
/// - C — keychain + file present, no flag → plaintext-file-wins, re-write keychain,
///         delete file, set flag
/// - D — keychain present, file absent, no flag → crash recovery, set flag
/// - E — nothing present, no flag → fresh install, set flag (key written by init_encryption)
/// - F — flag set, file present → delete file without reading (keychain absent = unavailable)
pub fn run_sync_key_migration(conn: &Connection) {
    eprintln!("[KEYSTORE-MIGRATION] Starting sync master key migration pass...");

    // ── Fast-path: DB flag set ─────────────────────────────────────────────
    if is_db_flag_set(conn) {
        eprintln!("[KEYSTORE-MIGRATION] DB flag set — activating in-memory flag (post-migration regime)");
        // Set in-memory flag so all keystore operations use keychain-only paths
        super::keystore::mark_migration_complete();

        // State F: flag is set but file still exists → stale artifact, delete it
        let file_path = super::keystore::key_file_path();
        if file_path.exists() {
            handle_state_f(&file_path);
        }

        eprintln!("[KEYSTORE-MIGRATION] Pass complete (already migrated)");
        return;
    }

    // ── Migration needed — evaluate current state ──────────────────────────
    let file_path = super::keystore::key_file_path();
    let file_exists = file_path.exists();
    let keychain_val = crate::keychain::get_password(crate::sync::keystore::KEYCHAIN_ACCOUNT)
        .unwrap_or_else(|e| {
            eprintln!("[KEYSTORE-MIGRATION] Keychain read error: {} — treating as absent", e);
            None
        });

    match (file_exists, keychain_val) {
        // ── State E: fresh install — nothing to migrate ─────────────────
        (false, None) => {
            eprintln!(
                "[KEYSTORE-MIGRATION] State E (fresh install — nothing to migrate) — setting flag"
            );
            set_db_flag(conn);
            eprintln!("[KEYSTORE-MIGRATION] Pass complete (State E — fresh install)");
        }

        // ── State D: keychain present, file absent, no flag ─────────────
        // Crash occurred between file-delete and flag-set on a prior launch.
        // Keychain already has the key; just set the flag.
        (false, Some(_)) => {
            eprintln!(
                "[KEYSTORE-MIGRATION] State D (keychain present, file absent, no flag) \
                 — crash recovery, setting flag"
            );
            set_db_flag(conn);
            eprintln!("[KEYSTORE-MIGRATION] Pass complete (State D — crash recovery)");
        }

        // ── States A / C: file present ───────────────────────────────────
        (true, keychain_opt) => {
            // Read the file (source-of-truth for pre-flag migrates)
            let file_raw = match read_file_raw(&file_path) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "[KEYSTORE-MIGRATION] Cannot read sync_key.bin: {} \
                         — skipping migration, will retry next launch",
                        e
                    );
                    return;
                }
            };

            // Validate the file value is a decodable 32-byte key before proceeding
            let valid = general_purpose::STANDARD
                .decode(file_raw.as_bytes())
                .ok()
                .filter(|v| v.len() == 32)
                .is_some();
            if !valid {
                eprintln!(
                    "[KEYSTORE-MIGRATION] sync_key.bin contains invalid key data \
                     (wrong length or encoding) — skipping migration"
                );
                return;
            }

            let effective_val = match keychain_opt {
                None => {
                    // State A: normal upgrade path — file is the source
                    eprintln!(
                        "[KEYSTORE-MIGRATION] State A (file present, no keychain) — migrating"
                    );
                    file_raw.clone()
                }
                Some(ref kc_val) if kc_val == &file_raw => {
                    // State C (values match): prior attempt wrote keychain but didn't finish
                    eprintln!(
                        "[KEYSTORE-MIGRATION] State C (values match) — \
                         completing interrupted migration"
                    );
                    file_raw.clone()
                }
                Some(_) => {
                    // State C (diverge): plaintext file wins per ADR-0044 Decision 2
                    // (the file is the value the app has been decrypting data with)
                    eprintln!(
                        "[KEYSTORE-MIGRATION] State C (DIVERGE) — \
                         plaintext file wins, re-writing keychain"
                    );
                    file_raw.clone()
                }
            };

            // Step 1: Write to keychain
            if let Err(e) =
                crate::keychain::store_password(crate::sync::keystore::KEYCHAIN_ACCOUNT, &effective_val)
            {
                eprintln!(
                    "[KEYSTORE-MIGRATION] Keychain write failed: {} \
                     — preserving sync_key.bin, will retry next launch",
                    e
                );
                return;
            }

            // Step 2: Read back to confirm (copy-then-clear; never delete before confirmed)
            match crate::keychain::get_password(crate::sync::keystore::KEYCHAIN_ACCOUNT) {
                Ok(Some(ref confirmed)) if confirmed == &effective_val => {
                    eprintln!("[KEYSTORE-MIGRATION] Keychain read-back confirmed");
                }
                Ok(Some(_)) => {
                    eprintln!(
                        "[KEYSTORE-MIGRATION] Keychain read-back VALUE MISMATCH \
                         — aborting, preserving sync_key.bin"
                    );
                    return;
                }
                Ok(None) => {
                    eprintln!(
                        "[KEYSTORE-MIGRATION] Keychain read-back returned None \
                         — aborting, preserving sync_key.bin"
                    );
                    return;
                }
                Err(e) => {
                    eprintln!(
                        "[KEYSTORE-MIGRATION] Keychain read-back error: {} \
                         — aborting, preserving sync_key.bin",
                        e
                    );
                    return;
                }
            }

            // Step 3: Delete sync_key.bin (ONLY after confirmed keychain copy)
            delete_file(&file_path);

            // Step 4: Set migration flag (DB + in-memory)
            set_db_flag(conn);

            eprintln!("[KEYSTORE-MIGRATION] Pass complete — post-migration regime active");
        }
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::test_support::KeychainGuard;
    use rusqlite::Connection;
    use serial_test::serial;
    use std::sync::Mutex;
    use base64::engine::general_purpose;

    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    // ── DB helpers ────────────────────────────────────────────────────────

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE migration_flags (
                flag         TEXT PRIMARY KEY NOT NULL,
                completed_at TEXT NOT NULL DEFAULT (datetime('now'))
            );"
        )
        .unwrap();
        conn
    }

    fn db_flag_is_set(conn: &Connection) -> bool {
        is_db_flag_set(conn)
    }

    // ── File helpers ──────────────────────────────────────────────────────

    fn make_test_key(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn encode_key(key: &[u8; 32]) -> String {
        general_purpose::STANDARD.encode(key)
    }

    fn write_key_file(path: &std::path::Path, key: &[u8; 32]) {
        let dir = path.parent().unwrap();
        fs::create_dir_all(dir).unwrap();
        fs::write(path, encode_key(key)).unwrap();
    }

    fn read_keychain(account: &str) -> Option<String> {
        crate::keychain::get_password(account).unwrap()
    }

    // ── Setup / teardown ──────────────────────────────────────────────────

    fn setup() -> (Connection, tempfile::TempDir, KeychainGuard) {
        let kc = KeychainGuard::set();
        super::super::keystore::reset_migration_flag_for_test();
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sync_key.bin");
        super::super::keystore::set_test_key_path(Some(path));
        (setup_db(), dir, kc)
    }

    fn teardown() {
        super::super::keystore::set_test_key_path(None);
        super::super::keystore::reset_migration_flag_for_test();
    }

    // ── Test 1: State A — fresh migrate ───────────────────────────────────
    //
    // File present, no keychain entry, no flag
    // → copies to keychain, deletes file, sets flag

    #[test]
    #[serial(keychain_override)]
    fn test_state_a_fresh_migrate() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, dir, _kc) = setup();
        let key_path = dir.path().join("sync_key.bin");
        let master_key = make_test_key(0x01);
        write_key_file(&key_path, &master_key);

        run_sync_key_migration(&conn);

        // File should be deleted
        assert!(!key_path.exists(), "State A: sync_key.bin should be deleted after migration");

        // Key should be in keychain
        let in_keychain = read_keychain(crate::sync::keystore::KEYCHAIN_ACCOUNT);
        assert_eq!(
            in_keychain.as_deref(),
            Some(encode_key(&master_key).as_str()),
            "State A: key should be in keychain after migration"
        );

        // DB flag should be set
        assert!(db_flag_is_set(&conn), "State A: DB flag should be set after migration");

        // In-memory flag should be set
        assert!(
            crate::sync::keystore::is_migration_complete(),
            "State A: in-memory flag should be set after migration"
        );

        teardown();
    }

    // ── Test 2: State B — already migrated (no-op) ────────────────────────
    //
    // Flag set, keychain present, file absent → no-op, fast-path

    #[test]
    #[serial(keychain_override)]
    fn test_state_b_already_migrated_noop() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _dir, _kc) = setup();

        // Pre-populate keychain and set DB flag
        let master_key = make_test_key(0x02);
        crate::keychain::store_password(
            crate::sync::keystore::KEYCHAIN_ACCOUNT,
            &encode_key(&master_key),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO migration_flags (flag, completed_at) VALUES (?1, datetime('now'))",
            rusqlite::params![MIGRATION_FLAG],
        )
        .unwrap();

        run_sync_key_migration(&conn);

        // Keychain value unchanged
        let in_keychain = read_keychain(crate::sync::keystore::KEYCHAIN_ACCOUNT);
        assert_eq!(
            in_keychain.as_deref(),
            Some(encode_key(&master_key).as_str()),
            "State B: keychain value should be unchanged"
        );

        // In-memory flag set by fast-path
        assert!(
            crate::sync::keystore::is_migration_complete(),
            "State B: in-memory flag should be set"
        );

        teardown();
    }

    // ── Test 3: State C — partial/interrupted (plaintext-file-wins on diverge) ─
    //
    // Keychain present + file present, no flag, values DIFFERENT → file wins

    #[test]
    #[serial(keychain_override)]
    fn test_state_c_partial_plaintext_file_wins() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, dir, _kc) = setup();
        let key_path = dir.path().join("sync_key.bin");

        let file_key = make_test_key(0x03); // The authoritative value (in-use by app)
        let stale_kc_key = make_test_key(0xFF); // Stale keychain value from prior attempt

        // Write stale value to keychain
        crate::keychain::store_password(
            crate::sync::keystore::KEYCHAIN_ACCOUNT,
            &encode_key(&stale_kc_key),
        )
        .unwrap();

        // Write the correct (live) value to file
        write_key_file(&key_path, &file_key);

        run_sync_key_migration(&conn);

        // Keychain should be overwritten with file value
        let in_keychain = read_keychain(crate::sync::keystore::KEYCHAIN_ACCOUNT);
        assert_eq!(
            in_keychain.as_deref(),
            Some(encode_key(&file_key).as_str()),
            "State C diverge: file wins — keychain should hold file value"
        );

        // File should be deleted
        assert!(!key_path.exists(), "State C: sync_key.bin should be deleted after reconciliation");

        // Flag should be set
        assert!(db_flag_is_set(&conn), "State C: DB flag should be set");
        assert!(
            crate::sync::keystore::is_migration_complete(),
            "State C: in-memory flag should be set"
        );

        teardown();
    }

    // ── Test 4: State D — crash recovery (keychain present, file absent, no flag) ─

    #[test]
    #[serial(keychain_override)]
    fn test_state_d_crash_recovery() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _dir, _kc) = setup();

        let master_key = make_test_key(0x04);
        crate::keychain::store_password(
            crate::sync::keystore::KEYCHAIN_ACCOUNT,
            &encode_key(&master_key),
        )
        .unwrap();
        // File absent, flag absent (crash between delete and flag-set)

        run_sync_key_migration(&conn);

        // Flag should now be set
        assert!(db_flag_is_set(&conn), "State D: DB flag should be set (crash recovery)");
        assert!(
            crate::sync::keystore::is_migration_complete(),
            "State D: in-memory flag should be set"
        );

        // Keychain value should be unchanged
        let in_keychain = read_keychain(crate::sync::keystore::KEYCHAIN_ACCOUNT);
        assert_eq!(
            in_keychain.as_deref(),
            Some(encode_key(&master_key).as_str()),
            "State D: keychain value should be preserved"
        );

        teardown();
    }

    // ── Test 5: State E — fresh install (nothing exists) ──────────────────

    #[test]
    #[serial(keychain_override)]
    fn test_state_e_fresh_install() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _dir, _kc) = setup();
        // Nothing: no file, no keychain, no flag

        run_sync_key_migration(&conn);

        // Flag should be set (nothing to migrate = migration complete)
        assert!(db_flag_is_set(&conn), "State E: DB flag should be set on fresh install");
        assert!(
            crate::sync::keystore::is_migration_complete(),
            "State E: in-memory flag should be set"
        );

        // No key in keychain (fresh install — key not yet derived by user)
        let in_keychain = read_keychain(crate::sync::keystore::KEYCHAIN_ACCOUNT);
        assert!(in_keychain.is_none(), "State E: keychain should be empty on fresh install");

        teardown();
    }

    // ── Test 6: State F — flag set, file present, keychain PRESENT ────────
    //
    // File is stale artifact. Delete it. Keychain holds the live key.

    #[test]
    #[serial(keychain_override)]
    fn test_state_f_flag_file_keychain_present() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, dir, _kc) = setup();
        let key_path = dir.path().join("sync_key.bin");

        // Set DB flag (migration already completed)
        conn.execute(
            "INSERT INTO migration_flags (flag, completed_at) VALUES (?1, datetime('now'))",
            rusqlite::params![MIGRATION_FLAG],
        )
        .unwrap();

        // Keychain has the live key
        let live_key = make_test_key(0x06);
        crate::keychain::store_password(
            crate::sync::keystore::KEYCHAIN_ACCOUNT,
            &encode_key(&live_key),
        )
        .unwrap();

        // Stale file also present (e.g., asymmetric restore)
        let stale_key = make_test_key(0xDD);
        write_key_file(&key_path, &stale_key);

        run_sync_key_migration(&conn);

        // File should be deleted (stale artifact)
        assert!(!key_path.exists(), "State F: stale sync_key.bin should be deleted");

        // Keychain should be UNCHANGED (not overwritten with stale file)
        let in_keychain = read_keychain(crate::sync::keystore::KEYCHAIN_ACCOUNT);
        assert_eq!(
            in_keychain.as_deref(),
            Some(encode_key(&live_key).as_str()),
            "State F: keychain should hold live key, not stale file content"
        );

        // In-memory flag set
        assert!(
            crate::sync::keystore::is_migration_complete(),
            "State F: in-memory flag should be set"
        );

        teardown();
    }

    // ── Test 7: State F — flag set, file present, keychain ABSENT ─────────
    //
    // CRITICAL (Decision 5): file is NOT re-adopted. Delete it. Key unavailable.

    #[test]
    #[serial(keychain_override)]
    fn test_state_f_flag_file_keychain_absent() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, dir, _kc) = setup();
        let key_path = dir.path().join("sync_key.bin");

        // Set DB flag (migration already completed)
        conn.execute(
            "INSERT INTO migration_flags (flag, completed_at) VALUES (?1, datetime('now'))",
            rusqlite::params![MIGRATION_FLAG],
        )
        .unwrap();

        // No keychain entry (e.g., keychain was wiped)
        // File present (e.g., asymmetric backup restore pre-#1498)
        let stale_key = make_test_key(0xEE);
        write_key_file(&key_path, &stale_key);

        run_sync_key_migration(&conn);

        // File should be deleted (plaintext hazard)
        assert!(!key_path.exists(), "State F (no keychain): stale sync_key.bin should be deleted");

        // Keychain should still be absent — file content NOT re-adopted
        let in_keychain = read_keychain(crate::sync::keystore::KEYCHAIN_ACCOUNT);
        assert!(
            in_keychain.is_none(),
            "State F (no keychain): file must NOT be re-adopted into keychain (Decision 5)"
        );

        // In-memory flag set — subsequent load_master_key() will return None
        assert!(
            crate::sync::keystore::is_migration_complete(),
            "State F (no keychain): in-memory flag should be set"
        );

        // Verify load_master_key returns None (key unavailable, not the stale file content)
        let key = crate::sync::keystore::load_master_key().unwrap();
        assert!(
            key.is_none(),
            "State F (no keychain): load_master_key must return None — not stale file content"
        );

        teardown();
    }

    // ── Test 8: Post-migration keychain miss returns None ──────────────────
    //
    // Flag set, keychain absent (different from State F — no file either).
    // load_master_key() must return None, not attempt file read.

    #[test]
    #[serial(keychain_override)]
    fn test_post_migration_keychain_miss_returns_none() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, _dir, _kc) = setup();

        // Set DB flag
        conn.execute(
            "INSERT INTO migration_flags (flag, completed_at) VALUES (?1, datetime('now'))",
            rusqlite::params![MIGRATION_FLAG],
        )
        .unwrap();

        run_sync_key_migration(&conn);

        // Keychain empty → key unavailable
        let key = crate::sync::keystore::load_master_key().unwrap();
        assert!(
            key.is_none(),
            "Post-migration keychain miss must return None (Decision 5)"
        );

        teardown();
    }

    // ── Test 9: delete_master_key post-migration: deletes keychain, not file, flag preserved ─

    #[test]
    #[serial(keychain_override)]
    fn test_delete_post_migration_contract() {
        let _lock = TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let (conn, dir, _kc) = setup();
        let key_path = dir.path().join("sync_key.bin");

        // Set DB flag
        conn.execute(
            "INSERT INTO migration_flags (flag, completed_at) VALUES (?1, datetime('now'))",
            rusqlite::params![MIGRATION_FLAG],
        )
        .unwrap();

        // Store key in keychain
        let master_key = make_test_key(0x09);
        crate::keychain::store_password(
            crate::sync::keystore::KEYCHAIN_ACCOUNT,
            &encode_key(&master_key),
        )
        .unwrap();

        run_sync_key_migration(&conn); // Sets in-memory flag

        // Call delete_master_key
        crate::sync::keystore::delete_master_key().unwrap();

        // Keychain entry should be gone
        let in_keychain = read_keychain(crate::sync::keystore::KEYCHAIN_ACCOUNT);
        assert!(
            in_keychain.is_none(),
            "delete_master_key post-migration: keychain entry should be deleted"
        );

        // File should NOT have been created or deleted (delete targets keychain, not file)
        assert!(!key_path.exists(), "delete_master_key post-migration: should not touch file");

        // In-memory migration flag must still be set (Decision — delete doesn't clear flag)
        assert!(
            crate::sync::keystore::is_migration_complete(),
            "delete_master_key must NOT clear the migration flag"
        );

        // DB flag must still be set
        assert!(
            db_flag_is_set(&conn),
            "delete_master_key must NOT clear the DB migration flag"
        );

        teardown();
    }
}
