use rusqlite::Connection;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

/// Validates and joins a filesystem segment to a base path, rejecting traversal attacks.
///
/// # Security
/// All path construction from **external (gateway-supplied or migration-supplied) filenames**
/// MUST go through this function. It is the single enforcement point for the invariant:
/// "Sync ingest MUST NOT write outside `$HOME/Pane/.pane/system/{uuid}/`."
///
/// # Rejection criteria (returns `Err`)
/// - `segment` is empty
/// - `segment` is an absolute path (e.g. `/etc/passwd`)
/// - `segment` contains any `..` (parent-dir) component (e.g. `../foo`, `a/b/../../etc`)
/// - The resolved `base.join(segment)` path does not start with `base` (belt-and-suspenders)
///
/// # Note on `fs::canonicalize`
/// This function deliberately does NOT call `fs::canonicalize`. Canonicalization resolves
/// symlinks and requires the file to exist on disk. We use `starts_with` on the unresolved
/// `PathBuf` instead — sufficient to catch all traversal, no disk access needed.
/// (Per Reed's recommendation in scoping audit `docs/audits/2026-06-03-1300-path-traversal-scoping.md`.)
///
/// # Windows-separator note
/// On Unix, `..\.\foo` is a single path component (the literal string `..\foo`), not a
/// `ParentDir` component. The function's `ParentDir` check is therefore OS-native and will
/// not catch Windows-style traversal on a Unix build. The `starts_with(base)` post-join
/// check provides a belt-and-suspenders backstop, but true cross-platform defense requires
/// the allowlist filter (ADR-0041 §22C.8.5) to also reject Windows-separator filenames,
/// which it does today because no static allowlist entry contains backslashes.
///
/// # Unit tests
/// See `src-tauri/src/tests/test_pull_pending_changes_blocking.rs` for comprehensive tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn safe_path_join(base: &Path, segment: &str) -> Result<PathBuf, String> {
    // 1. Reject empty segment
    if segment.is_empty() {
        return Err("empty segment".to_string());
    }

    let seg_path = Path::new(segment);

    // 2. Reject absolute paths
    if seg_path.is_absolute() {
        return Err(format!("absolute path rejected: {}", segment));
    }

    // 3. Reject any parent-dir (..) components
    if seg_path
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(format!("parent-dir traversal rejected: {}", segment));
    }

    // 4. Build joined path and belt-and-suspenders check
    let joined = base.join(segment);
    if !joined.starts_with(base) {
        return Err(format!(
            "path escape rejected: {} resolves outside base {}",
            segment,
            base.display()
        ));
    }

    Ok(joined)
}

/// Declares the origin of an atomic write.
///
/// Every call to `atomic_write` must explicitly declare its origin. There is no default.
///
/// - `Local` — Pane-originated write (user action, AI tool, consolidation). `trigger_sync` runs
///   so the file is pushed to the gateway after the write.
/// - `Remote` — Ingest write from the gateway (`pull_pending_changes_blocking`, `initial_sync`, or any
///   future recovery flow). `trigger_sync` is suppressed to prevent the ping-pong loop where
///   an ingested file is immediately re-queued for push back to the gateway. Canonical write-back
///   after ingest is handled by the explicit `flush_sync_queue` call at the end of the ingest loop.
///
/// See `docs/spec/file-system.md` §2.3A and `docs/INVARIANTS.md` (Pane Gateway §22C section).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum WriteOrigin {
    /// Pane-originated write — trigger_sync runs.
    Local,
    /// Ingest write from gateway — trigger_sync suppressed.
    Remote,
}

/// Unified atomic write function for all file I/O in Pane.
///
/// This function:
/// 1. Writes content to a temporary file
/// 2. Verifies the write byte-for-byte
/// 3. Suppresses the file watcher to prevent re-processing our own writes
/// 4. Atomically renames the temp file to the target path
/// 5. If `db_conn` is Some AND path is a system file AND `origin == WriteOrigin::Local`: triggers sync
/// 6. Logs the write
///
/// # Arguments
/// * `path`    - Target file path
/// * `content` - Content to write
/// * `db_conn` - Optional database connection for sync triggering
/// * `origin`  - `WriteOrigin::Local` for user/AI writes; `WriteOrigin::Remote` for ingest writes
///
/// # Returns
/// `Ok(())` on success, or an error string describing what went wrong
pub fn atomic_write(
    path: &Path,
    content: &str,
    db_conn: Option<&Connection>,
    origin: WriteOrigin,
) -> Result<(), String> {
    // 1. Create a unique temporary file in the already-authorized target parent.
    // Keeping it in the same directory preserves atomic rename semantics without
    // following a predictable `.tmp` symlink.
    let parent = path
        .parent()
        .ok_or_else(|| format!("target has no parent directory: {}", path.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("temp file creation failed: {error}"))?;
    temp.as_file_mut()
        .write_all(content.as_bytes())
        .map_err(|error| format!("temp write failed: {error}"))?;
    temp.as_file_mut()
        .flush()
        .map_err(|error| format!("temp flush failed: {error}"))?;

    // 2. Verify write succeeded
    let written = std::fs::read_to_string(temp.path())
        .map_err(|error| format!("temp read failed: {error}"))?;
    if written != content {
        return Err("write verification failed".to_string());
    }

    // 3. Suppress file watcher
    crate::watcher::suppress_path(path);

    // 4. Atomically persist the temporary file at the target path.
    temp.persist(path)
        .map_err(|error| format!("rename failed: {}", error.error))?;

    // 5. Trigger sync only for local writes on system files with a DB connection.
    //    Remote (ingest) writes suppress trigger_sync to avoid the ping-pong loop
    //    (ingest → re-push → ingest). Canonical write-back after ingest is the
    //    explicit flush_sync_queue call in pull_pending_changes_blocking / initial_sync.
    if origin == WriteOrigin::Local {
        if let Some(conn) = db_conn {
            if crate::sync_client::is_system_path(path) {
                crate::sync_client::trigger_sync(conn, path, content);
            }
        }
    }

    eprintln!(
        "[WRITE-GUARD] Atomic write: {} ({} bytes, origin={:?})",
        path.display(),
        content.len(),
        origin
    );

    Ok(())
}
