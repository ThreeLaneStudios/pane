#[cfg(target_os = "macos")]
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use std::collections::HashMap;
use std::sync::Mutex;

/// Production Keychain service name — matches the macOS bundle identifier
/// (`tauri.conf.json`), used verbatim.
const SERVICE_NAME_PROD: &str = "com.threelane.pane";
/// Test-isolated Keychain service name. Root cause of a prior incident
/// (smoke-launch.sh wiping the real Keychain): filesystem isolation
/// (`PANE_TEST_ROOT`, see `database.rs::get_pane_root()`) and Keychain
/// isolation were never the same mechanism — the Keychain service name was a
/// bare constant with zero test branching. `service_name()` below mirrors
/// `get_pane_root()`'s own existing `PANE_TEST_ROOT`-branch pattern so every
/// test run (and `smoke-launch.sh`, see its own matching literal) reads from
/// and writes to a distinct Keychain service that can never collide with the
/// one real production installs use.
const SERVICE_NAME_TEST: &str = "com.threelane.pane.test";

/// Resolves which Keychain service name to use for this process: the
/// test-isolated name when `PANE_TEST_ROOT` is set (any value — presence is
/// the signal, matching `get_pane_root()`'s own check), the production name
/// otherwise. Every call site below (six total) already took `SERVICE_NAME`
/// as a value, so swapping in `service_name()` is a drop-in replacement, not
/// a signature change.
fn service_name() -> String {
    if std::env::var("PANE_TEST_ROOT").is_ok() {
        SERVICE_NAME_TEST.to_string()
    } else {
        SERVICE_NAME_PROD.to_string()
    }
}

#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Trait for secure credential storage providers.
/// Allows swapping the platform keychain with a mock implementation for testing.
pub trait KeychainProvider: Send + Sync {
    fn store_password(&self, account: &str, password: &str) -> Result<(), String>;
    fn get_password(&self, account: &str) -> Result<Option<String>, String>;
    fn delete_password(&self, account: &str) -> Result<(), String>;
}

/// Production credential storage using the operating system's secure store.
pub struct RealKeychain;

#[cfg(target_os = "macos")]
impl KeychainProvider for RealKeychain {
    fn store_password(&self, account: &str, password: &str) -> Result<(), String> {
        set_generic_password(&service_name(), account, password.as_bytes())
            .map_err(|e| format!("Keychain store failed: {}", e))
    }

    fn get_password(&self, account: &str) -> Result<Option<String>, String> {
        match get_generic_password(&service_name(), account) {
            Ok(bytes) => Ok(Some(
                String::from_utf8(bytes.to_vec())
                    .map_err(|e| format!("Keychain decode failed: {}", e))?,
            )),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(format!("Keychain read failed: {}", e)),
        }
    }

    fn delete_password(&self, account: &str) -> Result<(), String> {
        match delete_generic_password(&service_name(), account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(format!("Keychain delete failed: {}", e)),
        }
    }
}

#[cfg(target_os = "linux")]
impl KeychainProvider for RealKeychain {
    fn store_password(&self, account: &str, password: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(&service_name(), account)
            .map_err(|e| format!("Secret Service entry creation failed: {e}"))?;

        entry
            .set_password(password)
            .map_err(|e| format!("Secret Service store failed: {e}"))
    }

    fn get_password(&self, account: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(&service_name(), account)
            .map_err(|e| format!("Secret Service entry creation failed: {e}"))?;

        match entry.get_password() {
            Ok(password) => Ok(Some(password)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("Secret Service read failed: {e}")),
        }
    }

    fn delete_password(&self, account: &str) -> Result<(), String> {
        let entry = keyring::Entry::new(&service_name(), account)
            .map_err(|e| format!("Secret Service entry creation failed: {e}"))?;

        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Secret Service delete failed: {e}")),
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl KeychainProvider for RealKeychain {
    fn store_password(&self, _account: &str, _password: &str) -> Result<(), String> {
        Err("Secure credential storage is unsupported on this platform".to_string())
    }

    fn get_password(&self, _account: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    fn delete_password(&self, _account: &str) -> Result<(), String> {
        Ok(())
    }
}
/// Per-account, per-operation failure injection for `MockKeychain`.
///
/// C5 (#2584) test gap fix: the original `MockKeychain` was all-or-nothing —
/// `get_password`/`store_password` either always succeeded (backed by the
/// shared HashMap) or the whole provider was swapped for `FailingKeychain`
/// (always fails everything). That meant the exact failure shape that caused
/// #2584 — a write that reports `Ok(())` but whose corresponding read
/// returns `Ok(None)` or `Err` for that *specific* account — was structurally
/// unreachable in tests. This lets a test toggle store/get failures
/// independently, per account, so write-succeeds-but-read-fails scenarios
/// (and the reverse) are exercisable without hand-rolling a new struct.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub enum KeychainFailureMode {
    /// The operation returns `Ok(None)` (get) — a value visibility gap, not
    /// an OS-level error.
    SilentMiss,
    /// The operation returns `Err(message)`.
    Error(String),
    /// D26 (#2673 PR-A): the operation blocks the calling thread for the given
    /// duration before returning `Ok(None)` — simulates a stalled macOS keychain
    /// authorization prompt (the exact failure mode #2673 is about). Only
    /// meaningful for `get_password`; used by tests that assert the
    /// `spawn_blocking` + `tokio::time::timeout` wrapping around keychain reads
    /// actually turns a stalled OS-level call into a bounded error instead of an
    /// indefinite hang.
    Hang(std::time::Duration),
}

/// Mock keychain for testing (in-memory HashMap), with optional per-account,
/// per-operation failure injection (see `KeychainFailureMode`).
#[cfg(test)]
pub struct MockKeychain {
    store: Mutex<HashMap<String, String>>,
    fail_store: Mutex<HashMap<String, KeychainFailureMode>>,
    fail_get: Mutex<HashMap<String, KeychainFailureMode>>,
}

#[cfg(test)]
impl MockKeychain {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
            fail_store: Mutex::new(HashMap::new()),
            fail_get: Mutex::new(HashMap::new()),
        }
    }

    /// Inject a failure mode for the next `store_password(account, ...)` calls
    /// (persists until cleared — matches the always-on semantics of the
    /// underlying HashMap so tests don't need to reason about call counts).
    #[allow(dead_code)] // exercised by future write-path failure-injection tests; part of the C5 (#2584) permanent test-double surface
    pub fn fail_store_for(&self, account: &str, mode: KeychainFailureMode) {
        self.fail_store
            .lock()
            .unwrap()
            .insert(account.to_string(), mode);
    }

    /// Inject a failure mode for `get_password(account)` calls.
    pub fn fail_get_for(&self, account: &str, mode: KeychainFailureMode) {
        self.fail_get
            .lock()
            .unwrap()
            .insert(account.to_string(), mode);
    }
}

#[cfg(test)]
impl KeychainProvider for MockKeychain {
    fn store_password(&self, account: &str, password: &str) -> Result<(), String> {
        let mode = self.fail_store.lock().unwrap().get(account).cloned();
        if let Some(mode) = mode {
            match mode {
                KeychainFailureMode::SilentMiss => {
                    // "Silent miss" on store has no meaningful distinct behavior
                    // (store either happens or errors) — treat as a no-op write
                    // that reports success without actually persisting, which is
                    // the real-world shape of a write whose OSStatus is Ok but
                    // that never becomes durably queryable.
                    return Ok(());
                }
                KeychainFailureMode::Error(msg) => return Err(msg),
                KeychainFailureMode::Hang(duration) => {
                    // D26: not the primary use case for store (reads are what block
                    // on an OS auth prompt), but honor it symmetrically rather than
                    // silently ignoring an injected mode a future test might set.
                    // Lock is released before sleeping so concurrent keychain reads
                    // are not blocked by a Hang injected on a different account.
                    std::thread::sleep(duration);
                    return Ok(());
                }
            }
        }
        self.store
            .lock()
            .unwrap()
            .insert(account.to_string(), password.to_string());
        Ok(())
    }

    fn get_password(&self, account: &str) -> Result<Option<String>, String> {
        let mode = self.fail_get.lock().unwrap().get(account).cloned();
        if let Some(mode) = mode {
            match mode {
                KeychainFailureMode::SilentMiss => return Ok(None),
                KeychainFailureMode::Error(msg) => return Err(msg),
                KeychainFailureMode::Hang(duration) => {
                    // Lock is released before sleeping so concurrent keychain reads
                    // on other accounts are not blocked by a Hang injected on a
                    // different account (d26 test isolation: Venice read must not
                    // stall behind anthropic's injected Hang).
                    std::thread::sleep(duration);
                    return Ok(None);
                }
            }
        }
        Ok(self.store.lock().unwrap().get(account).cloned())
    }

    fn delete_password(&self, account: &str) -> Result<(), String> {
        self.store.lock().unwrap().remove(account);
        Ok(())
    }
}

/// Global keychain provider override for testing
/// Tests can inject a mock keychain; production uses RealKeychain fallback
///
/// D26 (#2673 PR-A): holds an `Arc`, not a bare `Box`, so `store_password`/`get_password`/
/// `delete_password` only need to hold this mutex for the instant it takes to clone the Arc
/// out — the actual (possibly slow/hanging, e.g. `MockKeychain`'s `KeychainFailureMode::Hang`
/// in tests, or a real stalled macOS auth prompt in production) provider call then runs with
/// the mutex already released. Previously the mutex guard was held for the provider call's
/// full duration, so one slow/hung keychain call would block every other thread's keychain
/// access — including `set_test_keychain`/`clear_test_keychain` in concurrent tests.
static KEYCHAIN_OVERRIDE: Mutex<Option<std::sync::Arc<dyn KeychainProvider>>> = Mutex::new(None);

/// Set a test keychain provider (used in tests to inject MockKeychain)
#[cfg(test)]
pub fn set_test_keychain(provider: Box<dyn KeychainProvider>) {
    *KEYCHAIN_OVERRIDE.lock().unwrap() = Some(std::sync::Arc::from(provider));
}

/// Clear test keychain provider (restore production behavior).
///
/// Installs an empty passthrough `MockKeychain` rather than `None` so that any
/// background `spawn_blocking` threads still in flight (e.g. a timed-out keychain
/// read whose blocking thread hasn't woken yet) don't fall through to the real
/// macOS `get_generic_password` syscall after test cleanup — that syscall can
/// prompt for authorization and hang indefinitely on a CI machine or dev box.
/// An empty MockKeychain returns `Ok(None)` / `Ok(())` for everything, which is
/// indistinguishable from "not configured" and drains quickly, letting the tokio
/// test runtime join all blocking threads and exit cleanly.
#[cfg(test)]
pub fn clear_test_keychain() {
    *KEYCHAIN_OVERRIDE.lock().unwrap() = Some(std::sync::Arc::new(MockKeychain::new()));
}

pub fn store_password(account: &str, password: &str) -> Result<(), String> {
    // Check for test override first — clone the Arc out, then drop the mutex guard before
    // calling into the (potentially slow) provider (see D26 note on KEYCHAIN_OVERRIDE above).
    let override_provider = KEYCHAIN_OVERRIDE.lock().unwrap().clone();
    if let Some(provider) = override_provider {
        return provider.store_password(account, password);
    }
    // Fallback to real keychain
    RealKeychain.store_password(account, password)
}

pub fn get_password(account: &str) -> Result<Option<String>, String> {
    // Check for test override first — clone the Arc out, then drop the mutex guard before
    // calling into the (potentially slow) provider (see D26 note on KEYCHAIN_OVERRIDE above).
    let override_provider = KEYCHAIN_OVERRIDE.lock().unwrap().clone();
    if let Some(provider) = override_provider {
        return provider.get_password(account);
    }
    // Fallback to real keychain
    RealKeychain.get_password(account)
}

pub fn delete_password(account: &str) -> Result<(), String> {
    // Check for test override first — clone the Arc out, then drop the mutex guard before
    // calling into the (potentially slow) provider (see D26 note on KEYCHAIN_OVERRIDE above).
    let override_provider = KEYCHAIN_OVERRIDE.lock().unwrap().clone();
    if let Some(provider) = override_provider {
        return provider.delete_password(account);
    }
    // Fallback to real keychain
    RealKeychain.delete_password(account)
}

/// Export all keychain items for backup
/// Returns a map of account -> password for all known keychain entries
/// Note: This is used during backup. Encryption of the export blob is a follow-up (issue #972)
pub fn export_all_keychain_items(
    known_accounts: &[String],
) -> Result<HashMap<String, String>, String> {
    let mut items = HashMap::new();
    let mut failed_accounts = Vec::new();

    eprintln!(
        "[KEYCHAIN] Exporting {} keychain items for backup",
        known_accounts.len()
    );

    for account in known_accounts {
        match get_password(account) {
            Ok(Some(password)) => {
                items.insert(account.clone(), password);
            }
            Ok(None) => {
                eprintln!("[KEYCHAIN] Account '{}' not found in keychain", account);
            }
            Err(e) => {
                eprintln!("[KEYCHAIN] Failed to export account '{}': {}", account, e);
                failed_accounts.push(account.clone());
            }
        }
    }

    if !failed_accounts.is_empty() {
        eprintln!(
            "[KEYCHAIN] Warning: Failed to export {} accounts: {:?}",
            failed_accounts.len(),
            failed_accounts
        );
    }

    eprintln!(
        "[KEYCHAIN] Successfully exported {} keychain items",
        items.len()
    );
    Ok(items)
}

/// Import keychain items during restore
/// Writes keychain items from a backup export
pub fn import_keychain_items(items: &HashMap<String, String>) -> Result<(), String> {
    let mut failed_accounts = Vec::new();
    let mut success_count = 0;

    eprintln!(
        "[KEYCHAIN] Importing {} keychain items from backup",
        items.len()
    );

    for (account, password) in items {
        match store_password(account, password) {
            Ok(()) => {
                success_count += 1;
            }
            Err(e) => {
                eprintln!("[KEYCHAIN] Failed to import account '{}': {}", account, e);
                failed_accounts.push(account.clone());
            }
        }
    }

    if !failed_accounts.is_empty() {
        eprintln!(
            "[KEYCHAIN] Warning: Failed to import {} accounts: {:?}",
            failed_accounts.len(),
            failed_accounts
        );
        return Err(format!(
            "Partial keychain import: {} succeeded, {} failed",
            success_count,
            failed_accounts.len()
        ));
    }

    eprintln!(
        "[KEYCHAIN] Successfully imported {} keychain items",
        success_count
    );
    Ok(())
}

/// Test-only support types for keychain isolation across the test suite.
///
/// `KeychainGuard` is the crate-wide RAII replacement for the previous
/// pattern of manually pairing `set_test_keychain(...)` with a trailing
/// `clear_test_keychain()` call at the end of a test body. Because Rust
/// always runs `Drop` during unwind (unless the process aborts), binding
/// `let _kc = KeychainGuard::set();` at the top of a test ensures the
/// global keychain override is cleared even if the test panics partway
/// through, preventing one test's mock/failure-injected provider from
/// leaking into whichever test runs next in the same process.
#[cfg(test)]
pub(crate) mod test_support {
    use super::{clear_test_keychain, set_test_keychain, KeychainProvider, MockKeychain};

    pub struct KeychainGuard;

    impl KeychainGuard {
        /// Install a fresh empty MockKeychain.
        pub fn set() -> Self {
            set_test_keychain(Box::new(MockKeychain::new()));
            KeychainGuard
        }

        /// Install a caller-supplied provider (custom mock, failure-injector, etc.).
        pub fn set_with(provider: Box<dyn KeychainProvider>) -> Self {
            set_test_keychain(provider);
            KeychainGuard
        }
    }

    impl Drop for KeychainGuard {
        fn drop(&mut self) {
            clear_test_keychain();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::KeychainGuard;
    use super::*;
    use serial_test::serial;

    /// Root cause of a prior incident (`smoke-launch.sh` wiping the real
    /// Keychain): `SERVICE_NAME` never branched on `PANE_TEST_ROOT` the way
    /// `get_pane_root()` already did for the filesystem. `service_name()`
    /// must return the `.test`-suffixed name when `PANE_TEST_ROOT` is set
    /// and the plain production name otherwise.
    ///
    /// Mutation: hardcode `service_name()` to always return
    /// `SERVICE_NAME_PROD` → the isolation test stops seeing the `.test`
    /// suffix while `PANE_TEST_ROOT` is set → red.
    #[test]
    #[serial(pane_test_root)]
    fn test_service_name_branches_on_pane_test_root() {
        let prev = std::env::var("PANE_TEST_ROOT").ok();

        std::env::remove_var("PANE_TEST_ROOT");
        assert_eq!(
            service_name(),
            SERVICE_NAME_PROD,
            "with PANE_TEST_ROOT unset, service_name() must return the production literal"
        );

        std::env::set_var("PANE_TEST_ROOT", "/tmp/some-test-root");
        assert_eq!(
            service_name(),
            SERVICE_NAME_TEST,
            "with PANE_TEST_ROOT set, service_name() must return the .test-suffixed literal, \
             never the production one"
        );

        match prev {
            Some(v) => std::env::set_var("PANE_TEST_ROOT", v),
            None => std::env::remove_var("PANE_TEST_ROOT"),
        }
    }

    #[test]
    #[serial(keychain_override)]
    fn test_keychain_export_empty() {
        let _kc = KeychainGuard::set();
        let accounts: Vec<String> = vec![];
        let result = export_all_keychain_items(&accounts).unwrap();
        assert_eq!(result.len(), 0);
    }

    #[test]
    #[serial(keychain_override)]
    fn test_keychain_import_empty() {
        let _kc = KeychainGuard::set();
        let items = HashMap::new();
        let result = import_keychain_items(&items);
        assert!(result.is_ok());
    }

    #[test]
    #[serial(keychain_override)]
    fn test_keychain_roundtrip() {
        let _kc = KeychainGuard::set();
        let test_account = format!("test-account-{}", uuid::Uuid::new_v4());
        let test_password = "test-password-123";

        // Store
        store_password(&test_account, test_password).unwrap();

        // Export
        let accounts = vec![test_account.clone()];
        let exported = export_all_keychain_items(&accounts).unwrap();
        assert_eq!(exported.len(), 1);
        assert_eq!(exported.get(&test_account).unwrap(), test_password);

        // Delete
        delete_password(&test_account).unwrap();

        // Verify deleted
        let retrieved = get_password(&test_account).unwrap();
        assert!(retrieved.is_none(), "Password should be deleted");

        // Import
        import_keychain_items(&exported).unwrap();

        // Verify restored
        let restored = get_password(&test_account).unwrap();
        assert_eq!(
            restored.as_ref().unwrap(),
            test_password,
            "Password should be restored"
        );

        // Cleanup
        delete_password(&test_account).unwrap();
    }
}
