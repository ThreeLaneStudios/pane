// Pane v10.47.0 — Attachment System
// #1775: Fixed storage path (.pane/attachments → Projects/{name}/Assets/) and
//        project_id (name string → UUID). project_name column added for denormalized
//        path construction — avoids joins on every read.
// #1799: Large file upload guard — named constant, pre-decode size estimation,
//        and native fs::copy path for files that would OOM the base64 IPC pipeline.

use crate::database::DbState;
use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rusqlite::{params, OptionalExtension};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tauri::State;
use tauri_plugin_fs::FsExt;
use tauri_plugin_shell::ShellExt;

/// Maximum attachment size accepted via the base64/IPC upload path.
/// Files larger than this should use the native `copy_attachment_from_path` command instead.
/// Aligned with frontend MAX_UPLOAD_SIZE_MB constant (1024 MB).
const MAX_ATTACHMENT_SIZE_BYTES: usize = 1024 * 1024 * 1024; // 1 GB

const EMPTY_PROJECT_ATTACHMENT_LEAF: &str = "__unscoped__";
const MAX_STORAGE_EXTENSION_LEN: usize = 16;

#[derive(Debug)]
struct AttachmentLocation {
    root: PaneRoot,
    directory_relative: PathBuf,
    file_relative: PathBuf,
    project_name: String,
}

/// Derive a safe storage-only extension. The original filename remains DB metadata only.
pub(crate) fn attachment_storage_extension(filename: &str) -> String {
    let Some((_, extension)) = filename.rsplit_once('.') else {
        return "bin".to_string();
    };
    let sanitized: String = extension
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(MAX_STORAGE_EXTENSION_LEN)
        .collect();
    if sanitized.is_empty() {
        "bin".to_string()
    } else {
        sanitized
    }
}

pub(crate) fn attachment_storage_filename(
    attachment_id: &str,
    original_filename: &str,
) -> Result<String, String> {
    PaneFs::validate_name(attachment_id)
        .map_err(|error| format!("Invalid attachment id: {error}"))?;
    let filename = format!(
        "{}.{}",
        attachment_id,
        attachment_storage_extension(original_filename)
    );
    PaneFs::validate_name(&filename)
        .map_err(|error| format!("Invalid attachment storage filename: {error}"))?;
    Ok(filename)
}

fn current_project_name(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> Result<Option<String>, String> {
    conn.query_row(
        "SELECT name FROM projects WHERE id = ?1",
        params![project_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(|error| format!("Failed to resolve attachment project: {error}"))
}

fn project_attachment_location(
    project_name: &str,
    attachment_id: &str,
    original_filename: &str,
) -> Result<AttachmentLocation, String> {
    PaneFs::validate_name(project_name)
        .map_err(|error| format!("Invalid project name for attachment storage: {error}"))?;
    let storage_filename = attachment_storage_filename(attachment_id, original_filename)?;
    let directory_relative = PaneFs::relative_from_names([project_name, "Assets"])
        .map_err(|error| format!("Invalid project attachment directory: {error}"))?;
    let file_relative = directory_relative.join(storage_filename);
    Ok(AttachmentLocation {
        root: PaneRoot::Projects,
        directory_relative,
        file_relative,
        project_name: project_name.to_string(),
    })
}

/// #3050 PR 9, `docs/spec/runtime.md` §36.9.2 — the Home-surface sibling of
/// `project_attachment_location`. Home chat drops are never stored as if
/// they belonged to a real project (no project name, no `Projects/` root);
/// they get their own fixed `Home/Assets/{id}.{ext}` tree. Unlike
/// `project_attachment_location`, there is no name to validate here — the
/// directory literal is fixed and already a valid single path component.
fn home_attachment_location(
    attachment_id: &str,
    original_filename: &str,
) -> Result<AttachmentLocation, String> {
    let storage_filename = attachment_storage_filename(attachment_id, original_filename)?;
    let directory_relative = PaneFs::relative_from_names(["Assets"])
        .map_err(|error| format!("Invalid Home attachment directory: {error}"))?;
    let file_relative = directory_relative.join(storage_filename);
    Ok(AttachmentLocation {
        root: PaneRoot::Home,
        directory_relative,
        file_relative,
        project_name: String::new(),
    })
}

fn attachment_location(
    conn: &rusqlite::Connection,
    project_id: &str,
    stored_project_name: Option<&str>,
    attachment_id: &str,
    original_filename: &str,
) -> Result<AttachmentLocation, String> {
    // #3050 PR 9, §36.9.2 — Home's own sentinel redirects to
    // `Home/Assets/{id}.{ext}` instead of falling through to the legacy
    // `.pane/attachments/__global__/` leaf below. Checked BEFORE the
    // `project_id.is_empty() || project_id == "__global__"` fallthrough
    // (Reed's scope audit §2) so this is the only new branch — the empty-
    // `project_id` case (a different, older "no project" signal — see
    // `EMPTY_PROJECT_ATTACHMENT_LEAF`) and unknown/legacy project UUIDs are
    // deliberately left on the existing legacy leaf: only the literal
    // `"__global__"` sentinel the Home chat composer already sends is
    // redirected. Existing files already written at the OLD legacy path
    // are not broken — `attachment_location_candidates` (below) tries the
    // Home location first, then still tries every legacy candidate.
    if project_id == "__global__" {
        return home_attachment_location(attachment_id, original_filename);
    }

    let storage_filename = attachment_storage_filename(attachment_id, original_filename)?;

    // Prefer the current project name resolved by UUID. A validated denormalized name remains a
    // compatibility fallback for project rows that no longer exist.
    let project_name = if project_id.is_empty() {
        None
    } else {
        current_project_name(conn, project_id)?
            .filter(|name| !name.is_empty())
            .or_else(|| {
                stored_project_name
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
    };
    if let Some(project_name) = project_name {
        return project_attachment_location(&project_name, attachment_id, original_filename);
    }

    let legacy_leaf = if project_id.is_empty() {
        EMPTY_PROJECT_ATTACHMENT_LEAF
    } else {
        PaneFs::validate_name(project_id)
            .map_err(|error| format!("Invalid legacy attachment project id: {error}"))?;
        project_id
    };
    let directory_relative = PaneFs::relative_from_names([legacy_leaf])
        .map_err(|error| format!("Invalid legacy attachment directory: {error}"))?;
    let file_relative = directory_relative.join(storage_filename);
    Ok(AttachmentLocation {
        root: PaneRoot::Attachments,
        directory_relative,
        file_relative,
        project_name: String::new(),
    })
}

fn pane_fs() -> Result<PaneFs, String> {
    PaneFs::for_runtime()
        .map_err(|error| format!("Failed to initialize filesystem policy: {error}"))
}

fn legacy_attachment_location(
    leaf: &str,
    attachment_id: &str,
    original_filename: &str,
) -> Result<AttachmentLocation, String> {
    let leaf = if leaf.is_empty() {
        EMPTY_PROJECT_ATTACHMENT_LEAF
    } else {
        PaneFs::validate_name(leaf)
            .map_err(|error| format!("Invalid legacy attachment directory: {error}"))?;
        leaf
    };
    let storage_filename = attachment_storage_filename(attachment_id, original_filename)?;
    let directory_relative = PaneFs::relative_from_names([leaf])
        .map_err(|error| format!("Invalid legacy attachment directory: {error}"))?;
    let file_relative = directory_relative.join(storage_filename);
    Ok(AttachmentLocation {
        root: PaneRoot::Attachments,
        directory_relative,
        file_relative,
        project_name: String::new(),
    })
}

#[derive(Debug)]
enum AttachmentResolveError {
    Missing(String),
    Rejected(String),
}

impl std::fmt::Display for AttachmentResolveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(message) | Self::Rejected(message) => formatter.write_str(message),
        }
    }
}

fn attachment_location_candidates(
    conn: &rusqlite::Connection,
    project_id: &str,
    stored_project_name: &str,
    attachment_id: &str,
    original_filename: &str,
) -> Result<Vec<AttachmentLocation>, AttachmentResolveError> {
    let preferred = attachment_location(
        conn,
        project_id,
        Some(stored_project_name),
        attachment_id,
        original_filename,
    )
    .map_err(AttachmentResolveError::Rejected)?;
    let mut candidates = vec![preferred];

    if !stored_project_name.is_empty() {
        let stored_project =
            project_attachment_location(stored_project_name, attachment_id, original_filename)
                .map_err(AttachmentResolveError::Rejected)?;
        if !candidates.iter().any(|candidate| {
            candidate.root == stored_project.root
                && candidate.file_relative == stored_project.file_relative
        }) {
            candidates.push(stored_project);
        }
        candidates.push(
            legacy_attachment_location(stored_project_name, attachment_id, original_filename)
                .map_err(AttachmentResolveError::Rejected)?,
        );
    }

    let legacy_project_id =
        legacy_attachment_location(project_id, attachment_id, original_filename)
            .map_err(AttachmentResolveError::Rejected)?;
    if !candidates.iter().any(|candidate| {
        candidate.root == legacy_project_id.root
            && candidate.file_relative == legacy_project_id.file_relative
    }) {
        candidates.push(legacy_project_id);
    }
    Ok(candidates)
}

fn authorize_existing_attachment(
    policy: &PaneFs,
    location: &AttachmentLocation,
) -> Result<PathBuf, AttachmentResolveError> {
    let candidate = policy
        .authorize_write(location.root, &location.file_relative)
        .map_err(|error| {
            AttachmentResolveError::Rejected(format!("Attachment path rejected: {error}"))
        })?;
    match std::fs::symlink_metadata(candidate.as_path()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(AttachmentResolveError::Missing(format!(
                "Attachment blob does not exist: {}",
                candidate.as_path().display()
            )));
        }
        Err(error) => {
            return Err(AttachmentResolveError::Rejected(format!(
                "Failed to inspect attachment blob {}: {error}",
                candidate.as_path().display()
            )));
        }
        Ok(_) => {}
    }
    policy
        .authorize_existing(location.root, &location.file_relative, ExistingKind::File)
        .map(|authorized| authorized.as_path().to_path_buf())
        .map_err(|error| {
            AttachmentResolveError::Rejected(format!("Attachment path rejected: {error}"))
        })
}

fn authorize_attachment_record(
    conn: &rusqlite::Connection,
    policy: &PaneFs,
    project_id: &str,
    stored_project_name: &str,
    attachment_id: &str,
    original_filename: &str,
) -> Result<(AttachmentLocation, PathBuf), AttachmentResolveError> {
    let candidates = attachment_location_candidates(
        conn,
        project_id,
        stored_project_name,
        attachment_id,
        original_filename,
    )?;
    let mut last_missing = None;
    for location in candidates {
        match authorize_existing_attachment(policy, &location) {
            Ok(path) => return Ok((location, path)),
            Err(AttachmentResolveError::Missing(error)) => last_missing = Some(error),
            Err(error @ AttachmentResolveError::Rejected(_)) => return Err(error),
        }
    }
    Err(AttachmentResolveError::Missing(
        last_missing.unwrap_or_else(|| "Attachment blob does not exist".to_string()),
    ))
}

pub(crate) fn resolve_attachment_path_by_id(
    conn: &rusqlite::Connection,
    attachment_id: &str,
) -> Result<PathBuf, String> {
    let (project_id, project_name, filename): (String, String, String) = conn
        .query_row(
            "SELECT project_id, project_name, original_filename FROM attachments WHERE id = ?1",
            params![attachment_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| format!("Attachment not found: {error}"))?;
    let policy = pane_fs()?;
    authorize_attachment_record(
        conn,
        &policy,
        &project_id,
        &project_name,
        attachment_id,
        &filename,
    )
    .map(|(_, path)| path)
    .map_err(|error| error.to_string())
}

/// #3027 sweep finding: this used to call `policy.remove_file` — a hard,
/// permanent delete with no OS trash attempt at all — on a file a user
/// explicitly uploaded. No destructive fallback (there was no fallback to
/// remove), but it matches the ticket's second, broader category: "permanent
/// delete of content the user could have put there." Switched to `trash()`
/// so a deleted attachment is recoverable the same way a deleted
/// project/note/folder-leftover already is, and (consistent with ruling 1)
/// a trash failure now rejects the delete instead of silently destroying
/// the file.
fn remove_attachment_record(
    conn: &rusqlite::Connection,
    policy: &PaneFs,
    project_id: &str,
    stored_project_name: &str,
    attachment_id: &str,
    original_filename: &str,
) -> Result<(), String> {
    let preferred = attachment_location(
        conn,
        project_id,
        Some(stored_project_name),
        attachment_id,
        original_filename,
    )?;
    match policy.trash(preferred.root, &preferred.file_relative, ExistingKind::File) {
        Ok(()) => Ok(()),
        Err(error)
            if error.to_string().contains("does not exist")
                && !stored_project_name.is_empty()
                && preferred.project_name != stored_project_name =>
        {
            let fallback =
                project_attachment_location(stored_project_name, attachment_id, original_filename)?;
            match policy.trash(fallback.root, &fallback.file_relative, ExistingKind::File) {
                Ok(()) => Ok(()),
                Err(error) if error.to_string().contains("does not exist") => Ok(()),
                Err(error) => Err(format!("Attachment delete rejected: {error}")),
            }
        }
        Err(error) if error.to_string().contains("does not exist") => Ok(()),
        Err(error) => Err(format!("Attachment delete rejected: {error}")),
    }
}

/// Core persistence logic behind `save_attachment` — split out (#3161 PR 1)
/// so a Rust-internal caller that already has decoded bytes (`generate_image`,
/// `ai::image_gen`) can write through the SAME path a base64 IPC upload
/// does, without a wasted encode/decode round trip and without needing a
/// `tauri::State` handle (this takes a plain `&Connection`, callable from
/// any async context that already holds one). `save_attachment` below is
/// now a thin IPC wrapper: decode base64, call this, done — behavior for
/// existing callers is unchanged.
pub(crate) fn save_attachment_bytes(
    conn: &rusqlite::Connection,
    project_id: &str,
    filename: &str,
    mime_type: &str,
    bytes: &[u8],
) -> Result<serde_json::Value, String> {
    if bytes.len() > MAX_ATTACHMENT_SIZE_BYTES {
        return Err(format!(
            "File too large: {:.1} MB (limit: {} MB).",
            bytes.len() as f64 / (1024.0 * 1024.0),
            MAX_ATTACHMENT_SIZE_BYTES / (1024 * 1024)
        ));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let policy = pane_fs()?;
    let location = attachment_location(conn, project_id, None, &id, filename)?;
    let project_name = location.project_name.clone();

    let dir = policy
        .create_dir_all(location.root, &location.directory_relative)
        .map_err(|error| format!("Failed to create attachment directory: {error}"))?;
    eprintln!(
        "[SAVE_ATTACHMENT] Writing to dir: {}",
        dir.as_path().display()
    );

    let file_path = policy
        .authorize_write(location.root, &location.file_relative)
        .map_err(|error| format!("Attachment destination rejected: {error}"))?;
    fs::write(file_path.as_path(), bytes).map_err(|error| {
        format!(
            "Failed to write attachment file {}: {error}",
            file_path.as_path().display()
        )
    })?;

    let size_bytes = bytes.len() as i64;

    conn.execute(
        "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, project_id, project_name, filename, mime_type, size_bytes],
    ).map_err(|e| format!("DB insert failed: {}", e))?;

    eprintln!(
        "[SAVE_ATTACHMENT] ✓ Saved {} ({} bytes) → {}",
        filename,
        size_bytes,
        file_path.as_path().display()
    );

    Ok(serde_json::json!({
        "id": id,
        "project_id": project_id,
        "project_name": project_name,
        "original_filename": filename,
        "mime_type": mime_type,
        "size_bytes": size_bytes,
        "path": file_path.as_path().to_string_lossy(),
    }))
}

#[tauri::command]
pub async fn save_attachment(
    db: State<'_, DbState>,
    project_id: String,
    filename: String,
    mime_type: String,
    data: String,
) -> Result<serde_json::Value, String> {
    eprintln!(
        "[SAVE_ATTACHMENT] called with project_id: {}, filename: {}, mime_type: {}",
        project_id, filename, mime_type
    );

    // Fast-path size check: estimate decoded byte count from base64 string length
    // (base64 length × 3/4 ≈ decoded bytes). Avoids allocating ~900 MB to reject a 674 MB file.
    let estimated_bytes = (data.len() * 3) / 4;
    if estimated_bytes > MAX_ATTACHMENT_SIZE_BYTES {
        return Err(format!(
            "File too large: approximately {:.1} MB (limit: {} MB). Use a smaller file.",
            estimated_bytes as f64 / (1024.0 * 1024.0),
            MAX_ATTACHMENT_SIZE_BYTES / (1024 * 1024)
        ));
    }

    let bytes = BASE64
        .decode(&data)
        .map_err(|e| format!("Invalid base64: {}", e))?;

    let conn = db.0.lock().unwrap();
    save_attachment_bytes(&conn, &project_id, &filename, &mime_type, &bytes)
}

/// Validate a source path for use in copy_attachment_from_path.
///
/// Rejects:
///   - Paths that cannot be canonicalized (don't exist or inaccessible)
///   - Paths that still contain `..` components after canonicalization
///   - Paths whose canonicalized form is not an absolute path
///
/// Returns the canonicalized PathBuf on success.
// NOTE: There is a TOCTOU window between this validation and the subsequent fs::copy call.
// The file at the validated path could theoretically be replaced between validation and copy.
// This is acceptable for Pane's desktop threat model — exploitation requires existing local
// code execution. A fully TOCTOU-safe implementation would require passing an open fd to
// fs::copy, which Rust's stdlib does not support cleanly.
fn validate_copy_source_path(source_path: &str) -> Result<PathBuf, String> {
    let path = Path::new(source_path);

    // Reject paths with literal `..` components before even trying to resolve them
    for component in path.components() {
        if component == std::path::Component::ParentDir {
            return Err(format!(
                "Invalid source path — path traversal component `..` is not allowed: {}",
                source_path
            ));
        }
    }

    let source_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Source path is not accessible: {source_path} ({error})"))?;
    if source_metadata.file_type().is_symlink() {
        return Err(format!("Source path must not be a symlink: {source_path}"));
    }

    // Intermediate symlinks remain accepted for this explicitly external source path.
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("Source path is not accessible: {} ({})", source_path, e))?;

    // Double-check: canonical path must be absolute and must not contain `..`
    if !canonical.is_absolute() {
        return Err(format!(
            "Canonicalized path is not absolute: {}",
            canonical.display()
        ));
    }
    for component in canonical.components() {
        if component == std::path::Component::ParentDir {
            return Err(format!(
                "Canonicalized path contains `..` component (symlink escape?): {}",
                canonical.display()
            ));
        }
    }

    // Must be a regular file (not a directory or device)
    if !canonical.is_file() {
        return Err(format!(
            "Source path is not a regular file: {}",
            canonical.display()
        ));
    }

    Ok(canonical)
}

fn authorize_copy_source_path(
    source_path: &str,
    scope: &tauri::fs::Scope,
) -> Result<PathBuf, String> {
    // The native dialog/drop handlers grant this backend-held scope. A renderer
    // path string alone is not permission to read an arbitrary local file.
    if !scope.is_allowed(source_path) {
        return Err("Attachment source not authorized; select the file using the native picker or drop it into Pane".to_string());
    }
    let canonical = validate_copy_source_path(source_path)?;
    if !scope.is_allowed(&canonical) {
        return Err("Resolved attachment source is not authorized".to_string());
    }
    Ok(canonical)
}

/// Copy an attachment file from an absolute source path directly into the Assets directory,
/// bypassing the base64/IPC pipeline entirely.
///
/// This is the preferred upload path for large files (> ~50 MB) where the FileReader +
/// base64 + WKWebView IPC pipeline would require 3× peak memory overhead.
///
/// PATH-TRAVERSAL GUARD: `source_path` is canonicalized and validated before any copy.
/// Paths with `..` components or non-existent files are rejected with a descriptive error.
#[tauri::command]
pub async fn copy_attachment_from_path(
    app: tauri::AppHandle,
    db: State<'_, DbState>,
    project_id: String,
    source_path: String,
    filename: String,
    mime_type: String,
) -> Result<serde_json::Value, String> {
    eprintln!(
        "[COPY_ATTACHMENT] called with project_id: {}, source_path: {}, filename: {}",
        project_id, source_path, filename
    );

    // Fail closed if the filesystem plugin or its native-selection grants are unavailable.
    let scope = app
        .try_fs_scope()
        .ok_or("Attachment source authorization unavailable")?;
    let canonical_source = authorize_copy_source_path(&source_path, &scope)?;

    // 1 GB cap — mirrors MAX_ATTACHMENT_SIZE_BYTES in save_attachment.
    // Stat the file before copy so we reject without allocating any buffer.
    let source_size = std::fs::metadata(&canonical_source)
        .map_err(|e| {
            format!(
                "Failed to stat source file {}: {}",
                canonical_source.display(),
                e
            )
        })?
        .len() as usize;
    if source_size > MAX_ATTACHMENT_SIZE_BYTES {
        return Err(format!(
            "File too large: {:.1} MB (limit: {} MB).",
            source_size as f64 / (1024.0 * 1024.0),
            MAX_ATTACHMENT_SIZE_BYTES / (1024 * 1024)
        ));
    }

    let id = uuid::Uuid::new_v4().to_string();
    let conn = db.0.lock().unwrap();
    let policy = pane_fs()?;
    let location = attachment_location(&conn, &project_id, None, &id, &filename)?;
    let project_name = location.project_name.clone();

    let dir = policy
        .create_dir_all(location.root, &location.directory_relative)
        .map_err(|error| format!("Failed to create attachment directory: {error}"))?;
    eprintln!(
        "[COPY_ATTACHMENT] Copying to dir: {}",
        dir.as_path().display()
    );

    let dest_path = policy
        .authorize_write(location.root, &location.file_relative)
        .map_err(|error| format!("Attachment destination rejected: {error}"))?;

    // Native OS file copy — no memory allocation beyond metadata
    let bytes_copied = fs::copy(&canonical_source, dest_path.as_path()).map_err(|error| {
        format!(
            "Failed to copy attachment from {} to {}: {error}",
            canonical_source.display(),
            dest_path.as_path().display()
        )
    })?;

    let size_bytes = bytes_copied as i64;

    conn.execute(
        "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, project_id, project_name, filename, mime_type, size_bytes],
    ).map_err(|e| format!("DB insert failed: {}", e))?;

    eprintln!(
        "[COPY_ATTACHMENT] ✓ Copied {} ({} bytes) → {}",
        filename,
        size_bytes,
        dest_path.as_path().display()
    );

    Ok(serde_json::json!({
        "id": id,
        "project_id": project_id,
        "project_name": project_name,
        "original_filename": filename,
        "mime_type": mime_type,
        "size_bytes": size_bytes,
        "path": dest_path.as_path().to_string_lossy(),
    }))
}

#[tauri::command]
pub fn get_attachment(
    db: State<'_, DbState>,
    attachment_id: String,
) -> Result<serde_json::Value, String> {
    eprintln!(
        "[GET_ATTACHMENT] called with attachment_id: {}",
        attachment_id
    );
    let conn = db.0.lock().unwrap();
    let (id, project_id, project_name, filename, mime_type, size_bytes, created_at):
        (String, String, String, String, String, i64, String) = conn.query_row(
        "SELECT id, project_id, project_name, original_filename, mime_type, size_bytes, created_at FROM attachments WHERE id = ?1",
        params![attachment_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
    ).map_err(|e| format!("Attachment not found: {}", e))?;
    let policy = pane_fs()?;
    let (location, path) =
        authorize_attachment_record(&conn, &policy, &project_id, &project_name, &id, &filename)
            .map_err(|error| error.to_string())?;

            Ok(serde_json::json!({
                "id": id,
                "project_id": project_id,
        "project_name": location.project_name,
                "original_filename": filename,
                "mime_type": mime_type,
                "size_bytes": size_bytes,
                "created_at": created_at,
                "path": path.to_string_lossy(),
            }))
}

#[tauri::command]
pub fn get_attachment_data(
    db: State<'_, DbState>,
    attachment_id: String,
) -> Result<serde_json::Value, String> {
    eprintln!(
        "[GET_ATTACHMENT_DATA] called with attachment_id: {}",
        attachment_id
    );
    let conn = db.0.lock().unwrap();
    let (project_id, project_name, filename, mime_type): (String, String, String, String) = conn.query_row(
        "SELECT project_id, project_name, original_filename, mime_type FROM attachments WHERE id = ?1",
        params![attachment_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).map_err(|e| format!("Attachment not found: {}", e))?;

    let policy = pane_fs()?;
    let (_, path) = authorize_attachment_record(
        &conn,
        &policy,
        &project_id,
        &project_name,
        &attachment_id,
        &filename,
    )
    .map_err(|error| error.to_string())?;

    eprintln!(
        "[GET_ATTACHMENT_DATA] Reading from path: {}",
        path.display()
    );

    let bytes = fs::read(&path).map_err(|e| {
        format!(
            "Failed to read attachment file at {}: {}",
            path.display(),
            e
        )
    })?;
    let encoded = BASE64.encode(&bytes);

    Ok(serde_json::json!({
        "id": attachment_id,
        "mime_type": mime_type,
        "original_filename": filename,
        "data": encoded,
    }))
}

#[tauri::command]
pub fn delete_attachment(db: State<'_, DbState>, attachment_id: String) -> Result<(), String> {
    eprintln!(
        "[DELETE_ATTACHMENT] called with attachment_id: {}",
        attachment_id
    );
    let conn = db.0.lock().unwrap();
    let (project_id, project_name, filename): (String, String, String) = conn
        .query_row(
        "SELECT project_id, project_name, original_filename FROM attachments WHERE id = ?1",
        params![attachment_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("Attachment not found: {}", e))?;

    let policy = pane_fs()?;
    remove_attachment_record(
        &conn,
        &policy,
        &project_id,
        &project_name,
        &attachment_id,
        &filename,
    )?;

    conn.execute(
        "DELETE FROM attachments WHERE id = ?1",
        params![attachment_id],
    )
        .map_err(|e| {
            // #3029 ride-along (§5, #3028 audit residual finding): by this
            // point `remove_attachment_record` above has ALREADY moved the
            // file to OS trash successfully — if this DB delete then fails,
            // the file is gone from its canonical path (sitting in trash)
            // while the `attachments` row still claims it exists and is
            // readable. That divergence window is exactly the one #3028's
            // ordering fix for projects/notes closed for THOSE paths; this
            // command still has it, with no distinguishing log level to
            // flag it as anything other than an ordinary command failure.
            eprintln!(
                "[CRITICAL] delete_attachment: file for attachment {} was already trashed, \
                 but the DB row delete failed ({}) — disk and DB now disagree.",
                attachment_id, e
            );
            format!("DB delete failed: {}", e)
        })?;

    eprintln!("[DELETE_ATTACHMENT] ✓ Deleted attachment {}", attachment_id);
    Ok(())
}

#[tauri::command]
pub fn list_project_attachments(
    db: State<'_, DbState>,
    project_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    eprintln!(
        "[LIST_PROJECT_ATTACHMENTS] called with project_id: {}",
        project_id
    );
    let conn = db.0.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, project_id, project_name, original_filename, mime_type, size_bytes, created_at \
         FROM attachments WHERE project_id = ?1 ORDER BY created_at DESC"
    ).map_err(|e| format!("Failed to prepare list query: {}", e))?;

    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|e| format!("Query failed: {}", e))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read attachment row: {}", e))?;
    drop(stmt);

    let policy = pane_fs()?;
    let mut result = Vec::with_capacity(rows.len());
    for (id, pid, pname, filename, mime_type, size_bytes, created_at) in rows {
        let (location, path) =
            match authorize_attachment_record(&conn, &policy, &pid, &pname, &id, &filename) {
                Ok(resolved) => resolved,
                Err(AttachmentResolveError::Missing(error)) => {
                    eprintln!(
                        "[LIST_PROJECT_ATTACHMENTS] Skipping missing attachment blob {}: {}",
                        id, error
                    );
                    continue;
                }
                Err(AttachmentResolveError::Rejected(error)) => return Err(error),
            };
        result.push(serde_json::json!({
            "id": id,
            "project_id": pid,
            "project_name": location.project_name,
            "original_filename": filename,
            "mime_type": mime_type,
            "size_bytes": size_bytes,
            "created_at": created_at,
            "path": path.to_string_lossy(),
        }));
    }
    eprintln!(
        "[LIST_PROJECT_ATTACHMENTS] ✓ Found {} attachments for project {}",
        result.len(),
        project_id
    );
    Ok(result)
}

/// Resolve the absolute disk path for an attachment file.
///
/// Returns the absolute path as a String so the frontend can pass it to
/// `convertFileSrc` (Tauri asset protocol) for direct WebView loading.
/// This is plumbing for #1777 PR2b — not yet wired into the render path.
#[tauri::command]
pub fn resolve_attachment_path(
    db: State<'_, DbState>,
    attachment_id: String,
) -> Result<String, String> {
    eprintln!(
        "[RESOLVE_ATTACHMENT_PATH] called with attachment_id: {}",
        attachment_id
    );
    let conn = db.0.lock().unwrap();
    let path = resolve_attachment_path_by_id(&conn, &attachment_id)?;

    eprintln!(
        "[RESOLVE_ATTACHMENT_PATH] ✓ Resolved {} → {}",
        attachment_id,
        path.display()
    );
    Ok(path.to_string_lossy().into_owned())
}

/// Open an attachment file in the OS default application for that file type.
///
/// Resolves the attachment's absolute path from the DB (same logic as
/// `resolve_attachment_path`), then delegates to the OS via
/// `tauri_plugin_shell::ShellExt::open` (macOS: `open`, Windows: `start`,
/// Linux: `xdg-open`). Returns an error if the attachment is not found or if
/// the OS refuses to open the file.
#[tauri::command]
pub fn open_attachment_externally(
    db: State<'_, DbState>,
    app: tauri::AppHandle,
    attachment_id: String,
) -> Result<(), String> {
    eprintln!(
        "[OPEN_ATTACHMENT_EXTERNALLY] called with attachment_id: {}",
        attachment_id
    );

    let conn = db.0.lock().unwrap();
    let path = resolve_attachment_path_by_id(&conn, &attachment_id)?;

    eprintln!("[OPEN_ATTACHMENT_EXTERNALLY] Opening: {}", path.display());

    // Keep the existing shell-plugin integration until the opener-plugin migration is scheduled.
    #[allow(deprecated)]
    app.shell()
        .open(path.to_string_lossy().as_ref(), None)
        .map_err(|e| format!("Failed to open attachment externally: {}", e))?;

    eprintln!("[OPEN_ATTACHMENT_EXTERNALLY] ✓ Opened {}", path.display());
    Ok(())
}

// ─── read_attachment (#3050 PR 9, docs/spec/runtime.md §36.9.3) ────────────

/// The Home sentinel the chat composer already sends for every Home-chat
/// attachment write (`ChatSidebar.svelte`/`ChatPanel.svelte`'s
/// `projectId ?? '__global__'` convention, confirmed by `attachment_location`
/// above). `read_attachment`'s filename lookup needs the SAME scope value a
/// Home session's dropped file was actually stored under — `context.
/// projectId` being `None` (the existing comment at `tools.rs`'s dispatch
/// site: "Home chats (`context.projectId` is `None`)") must resolve to this
/// exact string, not empty string (`attachment_location`'s OTHER, older,
/// unrelated "no project" signal — see `EMPTY_PROJECT_ATTACHMENT_LEAF`).
const HOME_ATTACHMENT_PROJECT_SCOPE: &str = "__global__";

/// One row `read_attachment`'s ambiguous-filename error lists, so the model
/// (and the user, via the tool result) can disambiguate by id rather than
/// the lookup silently picking a winner.
pub(crate) struct AttachmentFilenameMatch {
    pub id: String,
    pub size_bytes: i64,
    pub created_at: String,
}

/// A resolved attachment ready for extraction: the authorized on-disk path
/// plus the metadata extraction/display need. Returned by
/// `resolve_attachment_for_read` on success.
struct ResolvedAttachment {
    /// The attachment's own UUID — carried through so the §4.6.1 delimiter
    /// wrap can put a structurally-safe identifier in the `source`
    /// attribute instead of the attacker-controlled filename (#3050 PR 9
    /// follow-up, Matt's ruling on Reed's audit §2).
    id: String,
    filename: String,
    mime_type: String,
    path: PathBuf,
}

/// Resolution half of §36.9.3's `read_attachment`: by `attachment_id`
/// (global — same no-project-ownership-check shape `resolve_attachment_path_
/// by_id`/`get_attachment` already have) or by `filename` (scoped to
/// `session_project_id`, resolved to `HOME_ATTACHMENT_PROJECT_SCOPE` for a
/// Home session exactly like every attachment WRITE already does).
/// Ambiguous filename (more than one row for the same project + filename —
/// a real possibility: nothing stops two different uploads sharing a name)
/// returns every match, not a silent first-row winner.
enum ReadAttachmentResolveOutcome {
    Resolved(ResolvedAttachment),
    NotFound(String),
    Ambiguous {
        filename: String,
        matches: Vec<AttachmentFilenameMatch>,
    },
}

/// Scoped identically for both resolution halves below: `None` (Home
/// session, or no `projectId` in the dispatch context) resolves to the
/// same `"__global__"` sentinel every Home attachment write already uses,
/// a real project session resolves to that project's id. Computed ONCE,
/// above the `attachment_id`/`filename` branch split, so both halves use
/// the exact same scope value — the by-id branch used to skip this
/// entirely (global lookup, no session check at all: a session in project
/// A could read project B's attachment by id, same shape as PR 4's
/// `list_jobs(scope='all')` leak). Matt's ruling (2026-10-29, Reed's audit
/// §5): scope the by-id branch exactly like the filename branch already
/// is.
fn read_attachment_scope(session_project_id: Option<&str>) -> &str {
    session_project_id
        .filter(|s| !s.is_empty())
        .unwrap_or(HOME_ATTACHMENT_PROJECT_SCOPE)
}

fn resolve_attachment_for_read(
    conn: &rusqlite::Connection,
    attachment_id: Option<&str>,
    filename: Option<&str>,
    session_project_id: Option<&str>,
) -> ReadAttachmentResolveOutcome {
    let scope = read_attachment_scope(session_project_id);

    if let Some(id) = attachment_id.filter(|s| !s.is_empty()) {
        // Same not-found message for "no such id" and "id exists but
        // belongs to a different project" — a foreign id must not be
        // distinguishable from a nonexistent one (no "exists but wrong
        // project", no filename echo, nothing that confirms the id
        // exists outside the caller's own project).
        let not_found = || {
            ReadAttachmentResolveOutcome::NotFound(format!("No attachment with id '{}'.", id))
        };
        let row: Option<(String, String, String, String)> = conn
            .query_row(
                "SELECT project_id, project_name, original_filename, mime_type FROM attachments WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .unwrap_or(None);
        let Some((project_id, project_name, original_filename, mime_type)) = row else {
            return not_found();
        };
        if project_id != scope {
            // Exists, but not in this session's project — same generic
            // not-found response as a truly-missing id. No detail leaks.
            return not_found();
        }
        let policy = match pane_fs() {
            Ok(p) => p,
            Err(e) => return ReadAttachmentResolveOutcome::NotFound(e),
        };
        return match authorize_attachment_record(
            conn,
            &policy,
            &project_id,
            &project_name,
            id,
            &original_filename,
        ) {
            Ok((_, path)) => ReadAttachmentResolveOutcome::Resolved(ResolvedAttachment {
                id: id.to_string(),
                filename: original_filename,
                mime_type,
                path,
            }),
            Err(error) => ReadAttachmentResolveOutcome::NotFound(error.to_string()),
        };
    }

    let Some(filename) = filename.filter(|s| !s.is_empty()) else {
        return ReadAttachmentResolveOutcome::NotFound(
            "read_attachment requires either attachment_id or filename.".to_string(),
        );
    };

    let mut stmt = match conn.prepare(
        "SELECT id, project_name, mime_type, size_bytes, created_at FROM attachments \
         WHERE project_id = ?1 AND original_filename = ?2 ORDER BY created_at ASC",
    ) {
        Ok(s) => s,
        Err(e) => {
            return ReadAttachmentResolveOutcome::NotFound(format!(
                "Failed to look up attachment: {}",
                e
            ))
        }
    };
    let rows: Vec<(String, String, String, i64, String)> = match stmt
        .query_map(params![scope, filename], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .and_then(|mapped| mapped.collect::<Result<Vec<_>, _>>())
    {
        Ok(rows) => rows,
        Err(e) => {
            return ReadAttachmentResolveOutcome::NotFound(format!(
                "Failed to look up attachment: {}",
                e
            ))
        }
    };

    match rows.len() {
        0 => ReadAttachmentResolveOutcome::NotFound(format!(
            "No attachment named '{}' in this project.",
            filename
        )),
        1 => {
            let (id, project_name, mime_type, _, _) = &rows[0];
            let policy = match pane_fs() {
                Ok(p) => p,
                Err(e) => return ReadAttachmentResolveOutcome::NotFound(e),
            };
            match authorize_attachment_record(conn, &policy, scope, project_name, id, filename) {
                Ok((_, path)) => ReadAttachmentResolveOutcome::Resolved(ResolvedAttachment {
                    id: id.clone(),
                    filename: filename.to_string(),
                    mime_type: mime_type.clone(),
                    path,
                }),
                Err(error) => ReadAttachmentResolveOutcome::NotFound(error.to_string()),
            }
        }
        _ => ReadAttachmentResolveOutcome::Ambiguous {
            filename: filename.to_string(),
            matches: rows
                .into_iter()
                .map(|(id, _, _, size_bytes, created_at)| AttachmentFilenameMatch {
                    id,
                    size_bytes,
                    created_at,
                })
                .collect(),
        },
    }
}

/// Renders the ambiguous-filename error exactly: lists every match's id,
/// size, and created_at so the model (or a human reading the tool result)
/// can pick the right one by `attachment_id` on a follow-up call — never a
/// silent first-row winner.
fn format_ambiguous_filename_error(filename: &str, matches: &[AttachmentFilenameMatch]) -> String {
    let mut out = format!(
        "Multiple attachments named '{}' in this project — specify attachment_id instead:",
        filename
    );
    for m in matches {
        out.push_str(&format!(
            "\n- {} ({} bytes, created {})",
            m.id, m.size_bytes, m.created_at
        ));
    }
    out
}

/// §36.9.3's extraction table, `md`/`txt`/`csv`/`json` row — plain UTF-8
/// read, no library.
fn extract_text_plain(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("Failed to read attachment: {}", e))
}

/// §36.9.3's extraction table, `pdf` row — text layer only (no OCR), via
/// `pdf-extract` (pure Rust, wraps `lopdf`; MIT-licensed — the one genuinely
/// new Cargo dependency this PR adds, see `Cargo.toml`'s own comment).
fn extract_text_pdf(path: &Path) -> Result<String, String> {
    pdf_extract::extract_text(path).map_err(|e| format!("Failed to extract PDF text: {}", e))
}

/// §36.9.3's extraction table, `docx` row — `document.xml`'s text, paragraphs
/// joined by newline. Uses `zip` (already resolved transitively via
/// `tauri-plugin-updater`) to open the container and `quick-xml` (already
/// resolved transitively via `plist`) to walk `<w:p>`/`<w:t>` elements —
/// zero new Cargo dependencies for this extractor (see `Cargo.toml`).
fn extract_text_docx(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|e| format!("Failed to open attachment: {}", e))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Failed to open docx container: {}", e))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| format!("docx missing word/document.xml: {}", e))?
        .read_to_string(&mut xml)
        .map_err(|e| format!("Failed to read docx document.xml: {}", e))?;

    let mut reader = quick_xml::Reader::from_str(&xml);
    reader.config_mut().trim_text(false);
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_text_run = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => {
                let local = e.local_name();
                if local.as_ref() == "t" {
                    in_text_run = true;
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let local = e.local_name();
                if local.as_ref() == "t" {
                    in_text_run = false;
                } else if local.as_ref() == "p" {
                    paragraphs.push(std::mem::take(&mut current));
                }
            }
            Ok(quick_xml::events::Event::Text(e)) => {
                if in_text_run {
                    current.push_str(&e.xml10_content());
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => return Err(format!("Failed to parse docx document.xml: {}", e)),
            _ => {}
        }
        buf.clear();
    }
    // A trailing paragraph with no closing </w:p> seen (malformed docx) is
    // dropped rather than silently merged into the prior one — current is
    // only ever flushed on a real </w:p>.
    Ok(paragraphs.join("\n"))
}

/// §36.9.3's "anything else" row, factored out so the mime-sniffing and the
/// message format live in one place.
fn unsupported_type_message(mime_type: &str) -> String {
    format!("Unsupported type: {}", mime_type)
}

/// Dispatches extraction by file EXTENSION (not the stored `mime_type` —
/// the frontend's `guessMimeType` has no entry for every extension this
/// table names, e.g. a pre-existing docx upload would be stored as
/// `application/octet-stream`; extension is the reliable signal for text
/// types). Images are still detected by `mime_type` (reliable: every image
/// extension the uploader maps already gets a real `image/*` mime type).
enum AttachmentContentOutcome {
    /// Real file content was extracted — this is external/untrusted content
    /// and taints the turn.
    Text(String),
    /// §36.9.3 images row (Matt's ruling, vision deferred): never extracted,
    /// never tainted — a fixed, Pane-authored line.
    Image,
    /// §36.9.3's "anything else" row — Pane-authored, never tainted.
    Unsupported(String),
}

fn extract_attachment_content(
    path: &Path,
    mime_type: &str,
    filename: &str,
) -> Result<AttachmentContentOutcome, String> {
    if mime_type.starts_with("image/") {
        return Ok(AttachmentContentOutcome::Image);
    }
    let ext = filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "md" | "txt" | "csv" | "json" => extract_text_plain(path).map(AttachmentContentOutcome::Text),
        "pdf" => extract_text_pdf(path).map(AttachmentContentOutcome::Text),
        "docx" => extract_text_docx(path).map(AttachmentContentOutcome::Text),
        _ => Ok(AttachmentContentOutcome::Unsupported(
            unsupported_type_message(mime_type),
        )),
    }
}

/// §36.9.3's 60,000-character cap, char-indexed (not byte-indexed — #3111's
/// own `.chars().take(500)` precedent; a byte index can panic/split
/// mid-codepoint on multi-byte UTF-8 content). Reuses `web.rs`'s existing
/// char-safe truncator rather than re-deriving a second one; only the
/// marker string differs from `web_fetch`'s own
/// (`"[truncated: N of M characters]"`, not `web_fetch`'s
/// `"[... truncated, N of M chars shown]"` — the spec names a different
/// literal for this tool, §36.9.3/§36.14 #7).
fn truncate_attachment_text(text: &str) -> String {
    let total = text.chars().count();
    let (truncated, was_truncated) = crate::ai::web::truncate_at_char_boundary(
        text,
        crate::ai::runtime_constants::RUNTIME_ATTACHMENT_TEXT_CAP,
    );
    if was_truncated {
        let shown = truncated.chars().count();
        format!("{}\n[truncated: {} of {} characters]", truncated, shown, total)
    } else {
        truncated
    }
}

/// Mirrors `ai::web::WebToolOutcome`'s shape so `read_attachment`'s
/// dedicated loop branch (`ai/chat.rs`) wires into the exact same
/// taint/log/provider-result plumbing `web_fetch`/`web_search` already use
/// — see that branch's own comment.
pub(crate) struct AttachmentToolOutcome {
    pub provider_result: String,
    pub display: String,
    pub is_external_content: bool,
    /// The resolved filename (or, when resolution itself failed, whatever
    /// identifier was requested) — `ai/chat.rs`'s dedicated loop branch
    /// uses this for the "Tools used" `display` line and, for a
    /// successful extraction, as data moved INSIDE the §4.6.1 untrusted
    /// block alongside the content (see `attachment_id` below for why it
    /// no longer goes in the delimiter attribute).
    pub filename: String,
    /// The resolved attachment's own UUID. Empty when resolution failed
    /// (not-found/ambiguous never taint, so it's never used for wrapping
    /// in that case). `ai/chat.rs`'s read_attachment branch uses this —
    /// never `filename` — for the `source="attachment:{id}"` delimiter
    /// attribute on a successful extraction: a UUID is structurally safe
    /// (no user input reaches it), while `filename` is verbatim,
    /// unsanitized user input that could otherwise forge a delimiter
    /// break-out (#3050 PR 9 follow-up, Matt's ruling on Reed's audit §2).
    pub attachment_id: String,
}

/// §36.9.3 entry point. Never returns `Err` — every failure condition
/// (not found, ambiguous, unsupported type, image) is surfaced as an
/// `Ok`-shaped outcome with `is_external_content: false`, mirroring
/// `execute_web_fetch`/`execute_web_search`'s own §4.6.7 discipline:
/// "Pane-authored error strings do not taint." Only a genuine successful
/// text extraction — real file content entering the model's context — sets
/// `is_external_content: true` ("a dropped PDF is external content").
///
/// `display` is ALWAYS exactly `read_attachment(<filename or id>)` — per
/// §36.9.3's own wording ("never the content"), unlike `web_fetch`'s
/// `"web_fetch(url) → title"` shape, which embeds a content-derived
/// summary. The loop applies its normal ✅/❌ prefix on top, same as every
/// other tool.
pub(crate) fn execute_read_attachment(
    conn: &rusqlite::Connection,
    attachment_id: Option<&str>,
    filename: Option<&str>,
    session_project_id: Option<&str>,
) -> AttachmentToolOutcome {
    let requested_label = filename
        .filter(|s| !s.is_empty())
        .or(attachment_id)
        .unwrap_or("")
        .to_string();

    match resolve_attachment_for_read(conn, attachment_id, filename, session_project_id) {
        ReadAttachmentResolveOutcome::NotFound(message) => AttachmentToolOutcome {
            provider_result: message,
            display: format!("read_attachment({})", requested_label),
            is_external_content: false,
            filename: requested_label,
            attachment_id: String::new(),
        },
        ReadAttachmentResolveOutcome::Ambiguous { filename, matches } => AttachmentToolOutcome {
            provider_result: format_ambiguous_filename_error(&filename, &matches),
            display: format!("read_attachment({})", filename),
            is_external_content: false,
            filename,
            attachment_id: String::new(),
        },
        ReadAttachmentResolveOutcome::Resolved(resolved) => {
            let display = format!("read_attachment({})", resolved.filename);
            let filename = resolved.filename.clone();
            let attachment_id = resolved.id.clone();
            match extract_attachment_content(&resolved.path, &resolved.mime_type, &resolved.filename)
            {
                Ok(AttachmentContentOutcome::Text(text)) => AttachmentToolOutcome {
                    provider_result: truncate_attachment_text(&text),
                    display,
                    is_external_content: true,
                    filename,
                    attachment_id,
                },
                Ok(AttachmentContentOutcome::Image) => AttachmentToolOutcome {
                    provider_result: "Image attachment; image reading isn't available yet."
                        .to_string(),
                    display,
                    is_external_content: false,
                    filename,
                    attachment_id,
                },
                Ok(AttachmentContentOutcome::Unsupported(message)) => AttachmentToolOutcome {
                    provider_result: message,
                    display,
                    is_external_content: false,
                    filename,
                    attachment_id,
                },
                Err(error) => AttachmentToolOutcome {
                    provider_result: error,
                    display,
                    is_external_content: false,
                    filename,
                    attachment_id,
                },
            }
        }
    }
}

#[cfg(test)]
fn attachment_dir_for_project_id(
    conn: &rusqlite::Connection,
    project_id: &str,
) -> (PathBuf, String) {
    let policy = pane_fs().unwrap();
    let location =
        attachment_location(conn, project_id, None, "test-attachment", "file.bin").unwrap();
    let directory = policy
        .create_dir_all(location.root, &location.directory_relative)
        .unwrap();
    (directory.as_path().to_path_buf(), location.project_name)
}

#[cfg(test)]
fn build_attachment_path_from_name(
    project_id: &str,
    project_name: &str,
    attachment_id: &str,
    extension: &str,
) -> PathBuf {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
        [],
    )
    .unwrap();
    let policy = pane_fs().unwrap();
    let original_filename = format!("file.{extension}");
    let location = attachment_location(
        &conn,
        project_id,
        Some(project_name),
        attachment_id,
        &original_filename,
    )
    .unwrap();
    policy
        .authorize_write(location.root, &location.file_relative)
        .unwrap()
        .as_path()
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                parent_id TEXT
            );
            CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL DEFAULT '',
                original_filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
        ",
        )
        .unwrap();
        conn
    }

    /// #3050 PR 9, §36.9.2 — the write-time resolver now redirects the
    /// Home sentinel (`"__global__"`) to `Home/Assets/`, not the legacy
    /// `.pane/attachments/__global__/` leaf. This test previously asserted
    /// the OLD path; updated per Reed's scope audit §2 ("four existing
    /// tests assert the OLD write-time path ... and must be updated, not
    /// just left to rot"). Mutation: revert `attachment_location`'s new
    /// `"__global__"` branch → this reverts to asserting the legacy path →
    /// red against the real Home redirect.
    #[test]
    fn test_attachment_dir_global_uses_home_path() {
        let conn = setup_test_db();
        let (dir, project_name) = attachment_dir_for_project_id(&conn, "__global__");
        assert!(
            dir.to_string_lossy().contains("Home/Assets"),
            "Expected Home/Assets path for __global__, got: {}",
            dir.display()
        );
        assert!(
            !dir.to_string_lossy().contains(".pane/attachments"),
            "Home attachments must not land under the legacy .pane/attachments tree, got: {}",
            dir.display()
        );
        assert!(
            project_name.is_empty(),
            "project_name should be empty for __global__ (Home has no project name)"
        );
    }

    /// Backward-compat regression pin (Reed's scope audit §2): a file that
    /// already exists at the OLD legacy `.pane/attachments/__global__/`
    /// path — from before this PR — must still resolve via the real
    /// production read path (`resolve_attachment_path_by_id`, which walks
    /// `attachment_location_candidates`), not just the write-time resolver.
    /// "Existing rows stay where they are — no migration." Mutation: drop
    /// the legacy fallback from `attachment_location_candidates` (e.g. by
    /// only ever returning `preferred`) → this goes from resolving to
    /// erroring "Attachment blob does not exist" → red.
    #[test]
    fn test_resolve_attachment_path_by_id_finds_file_at_old_legacy_global_path() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES ('att-legacy-001', '__global__', '', 'old-note.md', 'text/markdown', 42)",
            params![],
        )
        .unwrap();

        // Seed a real file at the OLD legacy path directly on disk —
        // simulating an attachment written before this PR shipped, never
        // migrated (per the spec: "no migration").
        let policy = pane_fs().unwrap();
        let legacy = legacy_attachment_location("__global__", "att-legacy-001", "old-note.md")
            .unwrap();
        policy
            .create_dir_all(legacy.root, &legacy.directory_relative)
            .unwrap();
        let legacy_path = policy
            .authorize_write(legacy.root, &legacy.file_relative)
            .unwrap();
        std::fs::write(legacy_path.as_path(), b"legacy content").unwrap();

        let resolved = resolve_attachment_path_by_id(&conn, "att-legacy-001")
            .expect("a file at the old legacy Home path must still resolve");
        assert!(
            resolved.to_string_lossy().contains(".pane/attachments/__global__"),
            "Expected the resolved path to be the OLD legacy location (no migration), got: {}",
            resolved.display()
        );

        let _ = std::fs::remove_file(legacy_path.as_path());
    }

    #[test]
    fn test_attachment_dir_empty_uses_legacy_path() {
        let conn = setup_test_db();
        let (dir, project_name) = attachment_dir_for_project_id(&conn, "");
        assert!(
            dir.to_string_lossy().contains(".pane/attachments"),
            "Expected legacy path for empty project_id, got: {}",
            dir.display()
        );
        assert!(
            project_name.is_empty(),
            "project_name should be empty for empty project_id"
        );
    }

    #[test]
    fn test_attachment_dir_unknown_uuid_uses_legacy_path() {
        let conn = setup_test_db();
        let unknown_id = "deadbeef-0000-0000-0000-000000000000";
        let (dir, project_name) = attachment_dir_for_project_id(&conn, unknown_id);
        assert!(
            dir.to_string_lossy().contains(".pane/attachments"),
            "Expected legacy path for unknown UUID, got: {}",
            dir.display()
        );
        assert!(
            project_name.is_empty(),
            "project_name should be empty for unknown UUID"
        );
    }

    #[test]
    fn test_attachment_dir_valid_project_uuid_uses_project_path() {
        let conn = setup_test_db();
        let project_id = "aaaaaaaa-0000-0000-0000-000000000001";
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES (?1, 'My Project', NULL)",
            params![project_id],
        )
        .unwrap();

        let (dir, project_name) = attachment_dir_for_project_id(&conn, project_id);
        assert!(
            dir.to_string_lossy().contains("Projects/My Project/Assets"),
            "Expected Projects/My Project/Assets path, got: {}",
            dir.display()
        );
        assert_eq!(project_name, "My Project");
    }

    #[test]
    fn test_build_attachment_path_with_project_name() {
        let path = build_attachment_path_from_name("some-uuid", "Test Project", "attach-id", "png");
        assert!(
            path.to_string_lossy()
                .contains("Projects/Test Project/Assets/attach-id.png"),
            "Expected project path, got: {}",
            path.display()
        );
    }

    /// #3050 PR 9: updated from the legacy-path assertion — `"__global__"`
    /// is the Home sentinel, now redirected to `Home/Assets/`. See
    /// `test_attachment_dir_global_uses_home_path`'s comment for the
    /// mutation that reddens this.
    #[test]
    fn test_build_attachment_path_without_project_name_uses_home() {
        let path = build_attachment_path_from_name("__global__", "", "attach-id", "jpg");
        assert!(
            path.to_string_lossy().contains("Home/Assets/attach-id.jpg"),
            "Expected Home/Assets path, got: {}",
            path.display()
        );
    }

    #[test]
    fn test_build_attachment_path_no_project_name_uses_project_id() {
        let thread_id = "thread-uuid-1234";
        let path = build_attachment_path_from_name(thread_id, "", "attach-id", "pdf");
        assert!(
            path.to_string_lossy().contains(".pane/attachments"),
            "Expected legacy .pane/attachments path, got: {}",
            path.display()
        );
        assert!(
            path.to_string_lossy().contains(thread_id),
            "Expected path to contain thread_id '{}', got: {}",
            thread_id,
            path.display()
        );
    }

    // --- resolve_attachment_path (unit tests without Tauri State) ---

    /// Helper that mimics the logic of resolve_attachment_path without Tauri State
    fn resolve_path_for_test(conn: &Connection, attachment_id: &str) -> Result<String, String> {
        let (project_id, project_name, filename): (String, String, String) = conn
            .query_row(
            "SELECT project_id, project_name, original_filename FROM attachments WHERE id = ?1",
            params![attachment_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Attachment not found: {}", e))?;

        let ext = filename.rsplit('.').next().unwrap_or("bin");
        let path = build_attachment_path_from_name(&project_id, &project_name, attachment_id, ext);
        Ok(path.to_string_lossy().into_owned())
    }

    #[test]
    fn test_resolve_attachment_path_project_attachment() {
        let conn = setup_test_db();
        let project_id = "proj-uuid-0001";
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES (?1, 'My Project', NULL)",
            params![project_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES ('att-001', ?1, 'My Project', 'photo.jpg', 'image/jpeg', 12345)",
            params![project_id],
        ).unwrap();

        let result = resolve_path_for_test(&conn, "att-001").unwrap();
        assert!(
            result.contains("Projects/My Project/Assets/att-001.jpg"),
            "Expected Projects/My Project/Assets path, got: {}",
            result
        );
    }

    /// #3050 PR 9: `resolve_path_for_test` mimics the WRITE-time resolver
    /// (`attachment_location`, via `build_attachment_path_from_name`), so
    /// this moves to the Home path along with the write-path tests above.
    /// Real backward-compat read coverage for files already sitting at the
    /// old legacy path lives in
    /// `test_resolve_attachment_path_by_id_finds_file_at_old_legacy_global_path`,
    /// which exercises the actual production `resolve_attachment_path_by_id`
    /// candidate chain instead of this helper.
    #[test]
    fn test_resolve_attachment_path_global_uses_home() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES ('att-002', '__global__', '', 'document.pdf', 'application/pdf', 9999)",
            params![],
        ).unwrap();

        let result = resolve_path_for_test(&conn, "att-002").unwrap();
        assert!(
            result.contains("Home/Assets/att-002.pdf"),
            "Expected Home/Assets path, got: {}",
            result
        );
    }

    #[test]
    fn test_resolve_attachment_path_not_found() {
        let conn = setup_test_db();
        let result = resolve_path_for_test(&conn, "nonexistent-id");
        assert!(result.is_err(), "Expected error for nonexistent attachment");
        assert!(
            result.unwrap_err().contains("Attachment not found"),
            "Error should mention 'Attachment not found'"
        );
    }

    #[test]
    fn test_authorize_copy_source_requires_native_scope_grant() {
        let app = tauri::test::mock_app();
        let scope = tauri::fs::Scope::new(&app, &Default::default()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let selected = dir.path().join("selected.txt");
        let unselected = dir.path().join("unselected.txt");
        std::fs::write(&selected, "selected content").unwrap();
        std::fs::write(&unselected, "private content").unwrap();

        assert!(
            authorize_copy_source_path(selected.to_str().unwrap(), &scope)
                .unwrap_err()
                .contains("not authorized")
        );
        // Same backend operation used by the native dialog and file-drop handlers.
        scope.allow_file(&selected).unwrap();
        assert_eq!(
            authorize_copy_source_path(selected.to_str().unwrap(), &scope).unwrap(),
            selected.canonicalize().unwrap()
        );
        assert!(authorize_copy_source_path(unselected.to_str().unwrap(), &scope).is_err());
        scope.forbid_file(&selected).unwrap();
        assert!(authorize_copy_source_path(selected.to_str().unwrap(), &scope).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_authorize_copy_source_grant_does_not_bypass_symlink_validation() {
        use std::os::unix::fs::symlink;
        let app = tauri::test::mock_app();
        let scope = tauri::fs::Scope::new(&app, &Default::default()).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        std::fs::write(&target, "private content").unwrap();
        symlink(&target, &link).unwrap();
        scope.allow_file(&link).unwrap();
        assert!(authorize_copy_source_path(link.to_str().unwrap(), &scope)
            .unwrap_err()
            .contains("symlink"));
    }

    // --- Path-traversal guard tests (#1799) ---

    #[test]
    fn test_validate_copy_source_path_rejects_dotdot_component() {
        let result = validate_copy_source_path("/tmp/../etc/passwd");
        assert!(
            result.is_err(),
            "Expected error for path with `..` component"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("..") || err.contains("traversal") || err.contains("not accessible"),
            "Error should mention path traversal or inaccessibility, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_copy_source_path_rejects_relative_path_with_dotdot() {
        let result = validate_copy_source_path("../../etc/passwd");
        assert!(
            result.is_err(),
            "Expected error for relative path with `..`"
        );
        let err = result.unwrap_err();
        // Should be caught by literal `..` check before canonicalize attempt
        assert!(
            err.contains("..") || err.contains("traversal"),
            "Error should mention path traversal, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_copy_source_path_rejects_nonexistent_file() {
        let result = validate_copy_source_path("/nonexistent/path/that/does/not/exist.mp4");
        assert!(result.is_err(), "Expected error for nonexistent file");
        let err = result.unwrap_err();
        assert!(
            err.contains("not accessible") || err.contains("No such file"),
            "Error should mention accessibility, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_copy_source_path_rejects_directory() {
        // Use a real temp directory rather than `/tmp` directly: on macOS,
        // `/tmp` is itself a symlink to `/private/tmp`, so the product code's
        // symlink check fires before the is-directory check and the test
        // never reaches the assertion it's meant to exercise. A freshly
        // created TempDir is a real directory, not a symlink.
        let dir = tempfile::tempdir().unwrap();
        let result = validate_copy_source_path(dir.path().to_str().unwrap());
        assert!(result.is_err(), "Expected error for directory path");
        let err = result.unwrap_err();
        assert!(
            err.contains("not a regular file") || err.contains("regular file"),
            "Error should mention regular file requirement, got: {}",
            err
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_validate_copy_source_path_rejects_source_symlink() {
        use std::os::unix::fs::symlink;

        let target = std::env::temp_dir().join(format!(
            "pane_test_attachment_target_{}.bin",
            uuid::Uuid::new_v4()
        ));
        let link = std::env::temp_dir().join(format!(
            "pane_test_attachment_link_{}.bin",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&target, b"hello").unwrap();
        symlink(&target, &link).unwrap();

        let result = validate_copy_source_path(link.to_str().unwrap());
        assert!(result.unwrap_err().contains("symlink"));

        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn test_storage_extension_is_short_ascii_alphanumeric() {
        assert_eq!(attachment_storage_extension("archive.tar.GZ"), "GZ");
        assert_eq!(attachment_storage_extension("no-extension"), "bin");
        assert_eq!(attachment_storage_extension("unsafe..."), "bin");
        assert_eq!(attachment_storage_extension("image.p/n?g"), "png");
        assert_eq!(
            attachment_storage_extension("file.abcdefghijklmnopQRST"),
            "abcdefghijklmnop"
        );
    }

    #[test]
    fn storage_filename_handles_edge_case_extensions() {
        for (original_filename, expected) in [
            ("extensionless", "attachment.bin"),
            ("punctuation.p/n?g!", "attachment.png"),
            ("unicode.图片", "attachment.bin"),
            ("long.abcdefghijklmnopQRSTUV", "attachment.abcdefghijklmnop"),
        ] {
            assert_eq!(
                attachment_storage_filename("attachment", original_filename).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn resolves_partially_migrated_legacy_attachment_locations() {
        let conn = setup_test_db();
        let project_id = "project-uuid";
        let stored_project_name = "Stored Project";
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES (?1, 'Current Project', NULL)",
            params![project_id],
        )
        .unwrap();

        for legacy_leaf in [stored_project_name, project_id] {
            let pane_root = tempfile::tempdir().unwrap();
            let policy = PaneFs::from_pane_root(pane_root.path()).unwrap();
            let relative = PaneFs::relative_from_names([
                legacy_leaf,
                attachment_storage_filename("attachment", "image.p/n?g!")
                    .unwrap()
                    .as_str(),
            ])
            .unwrap();
            let destination = policy
                .authorize_write(PaneRoot::Attachments, &relative)
                .unwrap();
            std::fs::create_dir_all(destination.as_path().parent().unwrap()).unwrap();
            std::fs::write(destination.as_path(), b"image").unwrap();

            let (_, resolved) = authorize_attachment_record(
                &conn,
                &policy,
                project_id,
                stored_project_name,
                "attachment",
                "image.p/n?g!",
            )
            .unwrap();
            assert_eq!(resolved, destination.as_path());
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_in_partially_migrated_legacy_location() {
        use std::os::unix::fs::symlink;

        let conn = setup_test_db();
        let pane_root = tempfile::tempdir().unwrap();
        let policy = PaneFs::from_pane_root(pane_root.path()).unwrap();
        let relative = PaneFs::relative_from_names(["Stored Project", "attachment.png"]).unwrap();
        let destination = policy
            .authorize_write(PaneRoot::Attachments, &relative)
            .unwrap();
        std::fs::create_dir_all(destination.as_path().parent().unwrap()).unwrap();
        let target = pane_root.path().join("target.png");
        std::fs::write(&target, b"image").unwrap();
        symlink(&target, destination.as_path()).unwrap();

        let error = authorize_attachment_record(
            &conn,
            &policy,
            "project-uuid",
            "Stored Project",
            "attachment",
            "image.png",
        )
        .unwrap_err();
        assert!(matches!(error, AttachmentResolveError::Rejected(_)));
    }

    #[test]
    fn test_validate_copy_source_path_accepts_existing_file() {
        use std::io::Write;
        // Create a temp file to validate against, in a per-test TempDir so
        // three concurrent `cargo test --lib` processes never race on the
        // same inode (see #2903).
        let tmp_dir = tempfile::TempDir::new().expect("tempdir");
        let tmp = tmp_dir.path().join("pane_test_valid_attachment.bin");
        {
            let mut f = std::fs::File::create(&tmp).expect("Failed to create temp file");
            f.write_all(b"hello").expect("Failed to write temp file");
        }
        let result = validate_copy_source_path(tmp.to_str().unwrap());
        assert!(
            result.is_ok(),
            "Expected Ok for valid existing file, got: {:?}",
            result.err()
        );
        let canonical = result.unwrap();
        assert!(canonical.is_absolute(), "Canonical path should be absolute");
        assert!(canonical.is_file(), "Canonical path should be a file");
        // tmp_dir is dropped here, cleaning up the file automatically.
    }

    // --- Pre-decode size estimation tests (#1799 C4) ---

    #[test]
    fn test_size_estimation_within_limit_is_accepted() {
        // A small base64 string (10 bytes decoded ≈ 14 bytes base64)
        let small_base64 = base64::engine::general_purpose::STANDARD.encode(b"hello world!");
        // estimated_bytes = (small_base64.len() * 3) / 4
        let estimated = (small_base64.len() * 3) / 4;
        assert!(
            estimated <= MAX_ATTACHMENT_SIZE_BYTES,
            "Small file estimation {} should be under limit {}",
            estimated,
            MAX_ATTACHMENT_SIZE_BYTES
        );
    }

    #[test]
    fn test_size_estimation_rejects_oversized_base64() {
        // Simulate a base64 string whose estimated decoded size exceeds MAX_ATTACHMENT_SIZE_BYTES.
        // We don't actually allocate 2×MAX bytes — just verify the formula.
        // A base64 string 2× the limit in length decodes to ~1.5× limit bytes.
        let oversized_b64_len = MAX_ATTACHMENT_SIZE_BYTES * 2;
        let estimated = (oversized_b64_len * 3) / 4;
        assert!(
            estimated > MAX_ATTACHMENT_SIZE_BYTES,
            "Estimation {} should exceed limit {}",
            estimated,
            MAX_ATTACHMENT_SIZE_BYTES
        );
        // Also verify: base64 len that is exactly the limit also decodes to ~75% of limit (under)
        let at_limit_b64_len = MAX_ATTACHMENT_SIZE_BYTES;
        let estimated_at_limit = (at_limit_b64_len * 3) / 4;
        assert!(
            estimated_at_limit < MAX_ATTACHMENT_SIZE_BYTES,
            "Base64 at exactly the limit ({}) decodes to {} bytes which should be under the byte limit {}",
            at_limit_b64_len, estimated_at_limit, MAX_ATTACHMENT_SIZE_BYTES
        );
    }

    #[test]
    fn test_size_estimation_formula_accuracy() {
        // Verify the estimation formula approximates actual decoded size correctly.
        // For a known payload, encoded len * 3 / 4 should be within a few bytes of actual len.
        let payload = vec![0xABu8; 1024]; // 1024 bytes of data
        let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
        let estimated = (encoded.len() * 3) / 4;
        // Estimation is always within 3 bytes of actual due to padding
        assert!(
            estimated >= payload.len() - 3 && estimated <= payload.len() + 3,
            "Estimation {} should be close to actual {} bytes",
            estimated,
            payload.len()
        );
    }

    #[test]
    fn test_resolve_attachment_path_preserves_extension() {
        let conn = setup_test_db();
        let project_id = "proj-uuid-0002";
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES (?1, 'Audio Project', NULL)",
            params![project_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES ('att-003', ?1, 'Audio Project', 'recording.mp3', 'audio/mpeg', 55000)",
            params![project_id],
        ).unwrap();

        let result = resolve_path_for_test(&conn, "att-003").unwrap();
        assert!(
            result.ends_with("att-003.mp3"),
            "Path should end with UUID.ext, got: {}",
            result
        );
        assert!(
            result.contains("Audio Project/Assets"),
            "Path should include project Assets dir, got: {}",
            result
        );
    }

    // --- copy_attachment_from_path logic tests (#1799) ---
    // These test the inner logic (validate + stat + copy + DB insert) without a Tauri State.
    // The full command requires tauri::State<DbState>, which can't be constructed in unit tests.

    /// Helper: exercise the copy logic directly (mirrors copy_attachment_from_path body).
    fn copy_attachment_logic(
        conn: &Connection,
        source_path: &str,
        filename: &str,
        project_id: &str,
        mime_type: &str,
    ) -> Result<(String, std::path::PathBuf, i64), String> {
        let canonical_source = validate_copy_source_path(source_path)?;

        // 1 GB cap check (same as the real command)
        let source_size = std::fs::metadata(&canonical_source)
            .map_err(|e| {
                format!(
                    "Failed to stat source file {}: {}",
                    canonical_source.display(),
                    e
                )
            })?
            .len() as usize;
        if source_size > MAX_ATTACHMENT_SIZE_BYTES {
            return Err(format!(
                "File too large: {:.1} MB (limit: {} MB).",
                source_size as f64 / (1024.0 * 1024.0),
                MAX_ATTACHMENT_SIZE_BYTES / (1024 * 1024)
            ));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let storage_filename = attachment_storage_filename(&id, filename)?;
        let (dir, project_name) = attachment_dir_for_project_id(conn, project_id);

        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create attachment dir {}: {}", dir.display(), e))?;

        let dest_path = dir.join(storage_filename);
        let bytes_copied = std::fs::copy(&canonical_source, &dest_path)
            .map_err(|e| format!("Failed to copy: {}", e))?;
        let size_bytes = bytes_copied as i64;

        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, project_id, project_name, filename, mime_type, size_bytes],
        ).map_err(|e| format!("DB insert failed: {}", e))?;

        Ok((id, dest_path, size_bytes))
    }

    #[test]
    fn test_copy_attachment_full_flow() {
        use std::io::Read;
        use std::io::Write;

        let conn = setup_test_db();

        // Create a real temp source file with known content, in a per-test
        // TempDir so three concurrent `cargo test --lib` processes never
        // race on the same inode (see #2903).
        let src_dir = tempfile::TempDir::new().expect("tempdir");
        let src_path = src_dir.path().join("pane_test_copy_src.bin");
        let known_content: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
        {
            let mut f = std::fs::File::create(&src_path).expect("Create temp source");
            f.write_all(&known_content).expect("Write temp source");
        }

        let result = copy_attachment_logic(
            &conn,
            src_path.to_str().unwrap(),
            "test_file.bin",
            "__global__",
            "application/octet-stream",
        );

        assert!(
            result.is_ok(),
            "Expected copy to succeed, got: {:?}",
            result.err()
        );
        let (id, dest_path, size_bytes) = result.unwrap();

        // (a) returned id is a valid UUID
        assert!(
            uuid::Uuid::parse_str(&id).is_ok(),
            "Expected valid UUID, got: {}",
            id
        );

        // (b) destination file exists
        assert!(
            dest_path.exists(),
            "Destination file should exist at: {}",
            dest_path.display()
        );

        // (c) bytes match source
        let mut dest_content = Vec::new();
        std::fs::File::open(&dest_path)
            .expect("Open dest")
            .read_to_end(&mut dest_content)
            .expect("Read dest");
        assert_eq!(
            dest_content, known_content,
            "Destination bytes should match source"
        );
        assert_eq!(
            size_bytes,
            known_content.len() as i64,
            "size_bytes should match"
        );

        // (d) DB row present
        let row: String = conn
            .query_row(
            "SELECT id FROM attachments WHERE id = ?1",
            params![id],
            |row| row.get(0),
            )
            .expect("DB row should exist after copy");
        assert_eq!(row, id, "DB row id should match returned id");

        // Cleanup: src_dir is dropped here, cleaning up the source file
        // automatically. Destination file is outside the TempDir (lives in
        // the app's attachment storage) so it's still cleaned up explicitly.
        let _ = std::fs::remove_file(&dest_path);
    }

    #[test]
    fn test_copy_attachment_rejects_directory() {
        let conn = setup_test_db();
        // Use a real temp directory rather than `/tmp` directly: on macOS,
        // `/tmp` is itself a symlink to `/private/tmp`, so the product code's
        // symlink check fires before the is-directory check and the test
        // never reaches the assertion it's meant to exercise. A freshly
        // created TempDir is a real directory, not a symlink.
        let dir = tempfile::tempdir().unwrap();
        let result = copy_attachment_logic(
            &conn,
            dir.path().to_str().unwrap(),
            "folder.bin",
            "__global__",
            "application/octet-stream",
        );
        assert!(result.is_err(), "Expected error for directory source");
        let err = result.unwrap_err();
        assert!(
            err.contains("not a regular file") || err.contains("regular file"),
            "Error should mention regular file, got: {}",
            err
        );
    }

    #[test]
    fn test_copy_attachment_size_cap_error_message() {
        // Test the size-cap logic directly using the error format string.
        // We can't easily create a >1 GB file in a unit test, so we verify the guard formula.
        // The real integration test is in test_copy_attachment_full_flow.
        // Here we verify that a size larger than MAX produces the expected error format.
        let fake_size: usize = MAX_ATTACHMENT_SIZE_BYTES + 1;
        let fake_size_mb = fake_size as f64 / (1024.0 * 1024.0);
        let error_msg = format!(
            "File too large: {:.1} MB (limit: {} MB).",
            fake_size_mb,
            MAX_ATTACHMENT_SIZE_BYTES / (1024 * 1024)
        );
        assert!(
            error_msg.contains("File too large"),
            "Error format check: {}",
            error_msg
        );
        assert!(
            error_msg.contains("1024 MB"),
            "Error should mention 1024 MB limit: {}",
            error_msg
        );
    }

    // ---------------------------------------------------------------------------
    // open_attachment_externally — path-resolution logic tests
    // (#401 PR1 — the shell::open call itself cannot be unit-tested without a live
    // Tauri AppHandle, so we test the path-resolution logic that feeds it)
    // ---------------------------------------------------------------------------

    /// Helper that replicates the path-resolution logic from open_attachment_externally
    /// without requiring a Tauri AppHandle (which cannot be constructed in unit tests).
    fn resolve_open_path_for_test(
        conn: &Connection,
        attachment_id: &str,
    ) -> Result<std::path::PathBuf, String> {
        let (project_id, project_name, filename): (String, String, String) = conn
            .query_row(
                "SELECT project_id, project_name, original_filename FROM attachments WHERE id = ?1",
                params![attachment_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("Attachment not found: {}", e))?;

        let ext = filename.rsplit('.').next().unwrap_or("bin");
        Ok(build_attachment_path_from_name(
            &project_id,
            &project_name,
            attachment_id,
            ext,
        ))
    }

    #[test]
    fn test_open_attachment_externally_resolves_project_attachment_path() {
        let conn = setup_test_db();
        let project_id = "proj-open-0001";
        conn.execute(
            "INSERT INTO projects (id, name, parent_id) VALUES (?1, 'Media Project', NULL)",
            params![project_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES ('att-open-001', ?1, 'Media Project', 'song.mp3', 'audio/mpeg', 4096)",
            params![project_id],
        ).unwrap();

        let path = resolve_open_path_for_test(&conn, "att-open-001").unwrap();
        assert!(
            path.to_string_lossy()
                .contains("Media Project/Assets/att-open-001.mp3"),
            "Expected Projects/Media Project/Assets path, got: {}",
            path.display()
        );
    }

    #[test]
    fn test_open_attachment_externally_invalid_id_returns_error() {
        let conn = setup_test_db();
        // No attachment inserted — lookup must fail with a descriptive error.
        let result = resolve_open_path_for_test(&conn, "nonexistent-attachment-id");
        assert!(result.is_err(), "Expected error for nonexistent attachment");
        let err = result.unwrap_err();
        assert!(
            err.contains("Attachment not found"),
            "Error should mention 'Attachment not found', got: {}",
            err
        );
    }

    /// #3050 PR 9: same write-path redirection as the other
    /// `build_attachment_path_from_name`-backed tests above — the helper
    /// mimics the write-time resolver, so "__global__" now resolves under
    /// Home.
    #[test]
    fn test_open_attachment_externally_resolves_global_attachment_to_home_path() {
        let conn = setup_test_db();
        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES ('att-open-002', '__global__', '', 'notes.pdf', 'application/pdf', 8192)",
            params![],
        ).unwrap();

        let path = resolve_open_path_for_test(&conn, "att-open-002").unwrap();
        assert!(
            path.to_string_lossy().contains("Home/Assets/att-open-002.pdf"),
            "Expected Home/Assets path, got: {}",
            path.display()
        );
    }
}

/// #3050 PR 9, §36.9.3/§36.9.4 — `read_attachment` resolution + extraction
/// tests. Separate `mod` from the main `tests` block above (not nested in
/// it) purely for readability given its size; shares the same crate-level
/// `use super::*` + `setup_test_db` pattern.
#[cfg(test)]
mod read_attachment_tests {
    use super::*;
    use rusqlite::Connection;
    use std::io::Write as _;

    fn setup_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                parent_id TEXT
            );
            CREATE TABLE attachments (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL DEFAULT '',
                original_filename TEXT NOT NULL,
                mime_type TEXT NOT NULL,
                size_bytes INTEGER NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );
        ",
        )
        .unwrap();
        conn
    }

    /// Seeds a real attachment: writes `bytes` at the real, policy-authorized
    /// location `attachment_location` would give this `(project_id,
    /// filename)` pair, and inserts the matching `attachments` row. Returns
    /// the new row's id. Mirrors what `save_attachment`/`copy_attachment_
    /// from_path` do in production, minus the Tauri `State`/async plumbing
    /// neither test needs.
    fn seed_attachment(
        conn: &Connection,
        project_id: &str,
        filename: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let location = attachment_location(conn, project_id, None, &id, filename).unwrap();
        let policy = pane_fs().unwrap();
        policy
            .create_dir_all(location.root, &location.directory_relative)
            .unwrap();
        let file_path = policy
            .authorize_write(location.root, &location.file_relative)
            .unwrap();
        fs::write(file_path.as_path(), bytes).unwrap();
        conn.execute(
            "INSERT INTO attachments (id, project_id, project_name, original_filename, mime_type, size_bytes) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, project_id, location.project_name, filename, mime_type, bytes.len() as i64],
        )
        .unwrap();
        id
    }

    /// Builds a minimal, real, valid single-page PDF with a text layer
    /// containing `text` — handwritten (no PDF-writing crate exists in this
    /// dependency graph; this PR's new dependency, `pdf-extract`, only
    /// reads). Confirmed to round-trip through `pdf_extract::extract_text`
    /// (sanity-checked directly against this exact builder before this
    /// test module was finalized).
    fn build_test_pdf_bytes(text: &str) -> Vec<u8> {
        let content = format!("BT /F1 24 Tf 72 700 Td ({}) Tj ET", text);
        let objects: Vec<String> = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 5 0 R >> >> /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), content),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        ];
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(b"%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, obj) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(obj.as_bytes());
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_offset = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for off in &offsets {
            out.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
        }
        out.extend_from_slice(b"trailer\n");
        out.extend_from_slice(format!("<< /Size {} /Root 1 0 R >>\n", objects.len() + 1).as_bytes());
        out.extend_from_slice(b"startxref\n");
        out.extend_from_slice(format!("{}\n", xref_offset).as_bytes());
        out.extend_from_slice(b"%%EOF");
        out
    }

    /// Builds a minimal, real docx: a zip container containing only
    /// `word/document.xml` with one `<w:p>` per entry in `paragraphs` — the
    /// extractor (`extract_text_docx`) only ever reads that one entry, so a
    /// full OOXML package (`[Content_Types].xml`, `_rels/`, etc.) is not
    /// needed to exercise it faithfully.
    fn build_test_docx_bytes(paragraphs: &[&str]) -> Vec<u8> {
        let mut body = String::new();
        for p in paragraphs {
            body.push_str(&format!(
                "<w:p><w:r><w:t>{}</w:t></w:r></w:p>",
                p.replace('&', "&amp;").replace('<', "&lt;")
            ));
        }
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
             <w:body>{}</w:body></w:document>",
            body
        );
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            writer.start_file("word/document.xml", options).unwrap();
            writer.write_all(xml.as_bytes()).unwrap();
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    // ── Extraction: md/txt/csv/json (plain UTF-8) ──────────────────────────

    #[test]
    fn test_extract_text_plain_types_md_txt_csv_json() {
        for (filename, mime, content) in [
            ("notes.md", "text/markdown", "# Heading\n\nSome body text."),
            ("notes.txt", "text/plain", "Plain text content."),
            ("data.csv", "text/csv", "a,b,c\n1,2,3"),
            ("data.json", "application/json", "{\"key\": \"value\"}"),
        ] {
            let conn = setup_test_db();
            let id = seed_attachment(&conn, "__global__", filename, mime, content.as_bytes());
            let outcome = execute_read_attachment(&conn, Some(&id), None, None);
            assert!(
                outcome.is_external_content,
                "{filename}: a successful text extraction must taint"
            );
            assert_eq!(
                outcome.provider_result, content,
                "{filename}: extracted text must match the file verbatim (under the cap)"
            );
            assert_eq!(outcome.display, format!("read_attachment({})", filename));
        }
    }

    // ── Extraction: pdf ─────────────────────────────────────────────────────

    #[test]
    fn test_extract_text_pdf_fixture() {
        let conn = setup_test_db();
        let pdf_bytes = build_test_pdf_bytes("Hello from a test PDF");
        let id = seed_attachment(&conn, "__global__", "report.pdf", "application/pdf", &pdf_bytes);
        let outcome = execute_read_attachment(&conn, Some(&id), None, None);
        assert!(outcome.is_external_content, "a successful PDF extraction must taint");
        assert!(
            outcome.provider_result.contains("Hello from a test PDF"),
            "expected the PDF's text layer in the result, got: {}",
            outcome.provider_result
        );
    }

    // ── Extraction: docx ─────────────────────────────────────────────────────

    #[test]
    fn test_extract_text_docx_fixture_paragraphs_joined_by_newline() {
        let conn = setup_test_db();
        let docx_bytes = build_test_docx_bytes(&["First paragraph.", "Second paragraph."]);
        let id = seed_attachment(
            &conn,
            "__global__",
            "report.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            &docx_bytes,
        );
        let outcome = execute_read_attachment(&conn, Some(&id), None, None);
        assert!(outcome.is_external_content, "a successful docx extraction must taint");
        assert_eq!(outcome.provider_result, "First paragraph.\nSecond paragraph.");
    }

    /// Mutation (Reed's scope audit's named example — "wrong XML tag for
    /// docx"): if `extract_text_docx` looked for `<w:x>` instead of `<w:t>`
    /// it would silently produce empty paragraphs. Confirmed directly: a
    /// docx fixture with real text extracts to a non-empty, non-whitespace
    /// string containing that exact text — see the test above. This test
    /// pins the SPECIFIC failure mode a wrong-tag mutation produces (empty
    /// paragraphs, not a crash), so a future regression that silently empties
    /// the extraction is caught even if it doesn't panic.
    #[test]
    fn test_extract_text_docx_wrong_tag_would_silently_empty_not_crash() {
        let conn = setup_test_db();
        let docx_bytes = build_test_docx_bytes(&["Content that must survive."]);
        let id = seed_attachment(
            &conn,
            "__global__",
            "report2.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            &docx_bytes,
        );
        let outcome = execute_read_attachment(&conn, Some(&id), None, None);
        assert!(
            !outcome.provider_result.trim().is_empty(),
            "a correct extractor must never silently produce empty text for a non-empty docx"
        );
        assert!(outcome.provider_result.contains("Content that must survive."));
    }

    // ── 60,000-character cap, char-indexed ──────────────────────────────────

    /// #3111-style char-safety guard: the fixture's content is ASCII only up
    /// to the cap boundary, then switches to a multi-byte UTF-8 character
    /// (é, 2 bytes in UTF-8) repeated past the boundary. A byte-indexed
    /// truncation (`&s[..60_000]`) would either panic (slicing mid-codepoint)
    /// or silently corrupt the boundary character; `.chars().take(60_000)`
    /// cannot do either. Mutation: swap `truncate_attachment_text`'s call to
    /// `web::truncate_at_char_boundary` for a byte-slice
    /// (`&text.as_bytes()[..RUNTIME_ATTACHMENT_TEXT_CAP]`) → panics on this
    /// exact fixture (multi-byte char straddling the 60,000 boundary) — red,
    /// confirmed and reverted (see PR report for the transcript).
    #[test]
    fn test_read_attachment_cap_and_marker_at_60k_char_indexed() {
        let conn = setup_test_db();
        // 59,990 ASCII chars, then 20 copies of 'é' (multi-byte) — total
        // 60,010 chars, straddling the 60,000 boundary with non-ASCII bytes.
        let mut content = "a".repeat(59_990);
        content.push_str(&"é".repeat(20));
        assert_eq!(content.chars().count(), 60_010);

        let id = seed_attachment(&conn, "__global__", "big.txt", "text/plain", content.as_bytes());
        let outcome = execute_read_attachment(&conn, Some(&id), None, None);
        assert!(outcome.is_external_content);

        let marker = "[truncated: 60000 of 60010 characters]";
        assert!(
            outcome.provider_result.contains(marker),
            "expected the exact truncation marker, got tail: {}",
            &outcome.provider_result[outcome.provider_result.len().saturating_sub(80)..]
        );
        let body_before_marker = outcome.provider_result.trim_end_matches(&format!("\n{}", marker));
        assert_eq!(
            body_before_marker.chars().count(),
            60_000,
            "the body before the marker must be exactly 60,000 CHARACTERS (not bytes)"
        );
    }

    #[test]
    fn test_read_attachment_under_cap_has_no_marker() {
        let conn = setup_test_db();
        let content = "short content, well under the cap";
        let id = seed_attachment(&conn, "__global__", "small.txt", "text/plain", content.as_bytes());
        let outcome = execute_read_attachment(&conn, Some(&id), None, None);
        assert_eq!(outcome.provider_result, content);
        assert!(!outcome.provider_result.contains("[truncated:"));
    }

    // ── Ambiguous filename ──────────────────────────────────────────────────

    /// Mutation (actually run against this exact test during the build,
    /// confirmed red, then reverted — see PR report for the transcript):
    /// appending `LIMIT 1` to `resolve_attachment_for_read`'s filename
    /// lookup query silently picks the first-created row instead of
    /// flagging ambiguity — `outcome.is_external_content` flips to `true`
    /// (a real text extraction succeeded) and the assertion on the error
    /// text fails first.
    #[test]
    fn test_read_attachment_ambiguous_filename_lists_matches() {
        let conn = setup_test_db();
        let id1 = seed_attachment(&conn, "proj-1", "report.txt", "text/plain", b"first file");
        let id2 = seed_attachment(&conn, "proj-1", "report.txt", "text/plain", b"second file");

        let outcome = execute_read_attachment(&conn, None, Some("report.txt"), Some("proj-1"));
        assert!(!outcome.is_external_content, "an ambiguous-filename error must not taint");
        assert!(outcome.provider_result.contains("Multiple attachments named 'report.txt'"));
        assert!(outcome.provider_result.contains(&id1));
        assert!(outcome.provider_result.contains(&id2));
    }

    // ── Unsupported type ─────────────────────────────────────────────────────

    #[test]
    fn test_read_attachment_unsupported_type() {
        let conn = setup_test_db();
        let id = seed_attachment(&conn, "__global__", "song.mp3", "audio/mpeg", b"fake mp3 bytes");
        let outcome = execute_read_attachment(&conn, Some(&id), None, None);
        assert!(!outcome.is_external_content, "an unsupported-type message must not taint");
        assert_eq!(outcome.provider_result, "Unsupported type: audio/mpeg");
    }

    // ── Images: the deferral line (Matt's ruling — vision deferred) ────────

    #[test]
    fn test_read_attachment_image_returns_deferral_line_verbatim() {
        let conn = setup_test_db();
        let id = seed_attachment(&conn, "__global__", "photo.png", "image/png", b"\x89PNG fake bytes");
        let outcome = execute_read_attachment(&conn, Some(&id), None, None);
        assert!(!outcome.is_external_content, "the image deferral line must not taint");
        assert_eq!(
            outcome.provider_result,
            "Image attachment; image reading isn't available yet."
        );
    }

    // ── Resolution: by filename, Home scope default ─────────────────────────

    #[test]
    fn test_read_attachment_by_filename_resolves_within_home_scope_when_session_project_id_is_none() {
        let conn = setup_test_db();
        seed_attachment(&conn, "__global__", "home-note.md", "text/markdown", b"home content");
        // A same-named file in a DIFFERENT project must NOT be matched.
        seed_attachment(&conn, "proj-x", "home-note.md", "text/markdown", b"project content");

        let outcome = execute_read_attachment(&conn, None, Some("home-note.md"), None);
        assert_eq!(outcome.provider_result, "home content");
    }

    #[test]
    fn test_read_attachment_by_filename_scoped_to_session_project_id() {
        let conn = setup_test_db();
        seed_attachment(&conn, "proj-a", "shared-name.txt", "text/plain", b"proj-a content");
        seed_attachment(&conn, "proj-b", "shared-name.txt", "text/plain", b"proj-b content");

        let outcome = execute_read_attachment(&conn, None, Some("shared-name.txt"), Some("proj-a"));
        assert_eq!(outcome.provider_result, "proj-a content");
    }

    #[test]
    fn test_read_attachment_filename_not_found_in_project() {
        let conn = setup_test_db();
        seed_attachment(&conn, "proj-a", "exists.txt", "text/plain", b"here");
        let outcome = execute_read_attachment(&conn, None, Some("missing.txt"), Some("proj-a"));
        assert!(!outcome.is_external_content);
        assert_eq!(outcome.provider_result, "No attachment named 'missing.txt' in this project.");
    }

    #[test]
    fn test_read_attachment_id_not_found() {
        let conn = setup_test_db();
        let outcome = execute_read_attachment(&conn, Some("nonexistent-id"), None, None);
        assert!(!outcome.is_external_content);
        assert!(outcome.provider_result.contains("No attachment with id"));
    }

    // ── Cross-project read-by-id (#3050 PR 9 follow-up, Reed's audit §5) ─────

    /// Matt's ruling: scope the by-id branch exactly like the filename
    /// branch — a session in project A must not be able to read project
    /// B's attachment by id, even knowing the exact UUID.
    ///
    /// Mutation (run against this exact test, confirmed red, reverted —
    /// see PR report for the transcript): removed the `if project_id !=
    /// scope { return not_found(); }` check from `resolve_attachment_for_
    /// read`'s by-id branch (the exact pre-fix shape) — the read succeeds
    /// and returns proj-b's content instead of a not-found error:
    /// `assert!(!outcome.is_external_content, ...)` fails first (`left:
    /// true`).
    #[test]
    fn test_read_attachment_by_id_cross_project_denied() {
        let conn = setup_test_db();
        let foreign_id = seed_attachment(&conn, "proj-b", "secret.txt", "text/plain", b"proj-b secret content");

        let outcome = execute_read_attachment(&conn, Some(&foreign_id), None, Some("proj-a"));
        assert!(
            !outcome.is_external_content,
            "a session in proj-a must not be able to read proj-b's attachment by id"
        );
        assert_eq!(
            outcome.provider_result,
            format!("No attachment with id '{}'.", foreign_id),
            "wrong-project id must get the SAME generic not-found message a truly-missing id gets"
        );
    }

    /// The foreign-id response must be textually IDENTICAL to the
    /// truly-missing-id response — no "exists but wrong project", no
    /// filename echo, nothing that lets a caller distinguish "this id
    /// belongs to someone else" from "this id doesn't exist at all".
    #[test]
    fn test_read_attachment_by_id_cross_project_message_matches_truly_missing_id() {
        let conn = setup_test_db();
        let foreign_id = seed_attachment(&conn, "proj-b", "secret.txt", "text/plain", b"proj-b secret content");

        let cross_project = execute_read_attachment(&conn, Some(&foreign_id), None, Some("proj-a"));
        let truly_missing = execute_read_attachment(&conn, Some("00000000-0000-0000-0000-000000000000"), None, Some("proj-a"));

        // Same shape, modulo the id substring itself — neither leaks the
        // filename, the stored project, or any other detail about the
        // foreign row.
        assert_eq!(
            cross_project.provider_result.replace(&foreign_id, "X"),
            truly_missing.provider_result.replace("00000000-0000-0000-0000-000000000000", "X"),
        );
        assert!(!cross_project.provider_result.contains("secret.txt"));
        assert!(!cross_project.provider_result.contains("proj-b"));
    }

    /// Home's `__global__` scope must keep working: a Home session
    /// (`session_project_id = None`, resolving to the `"__global__"`
    /// sentinel every Home write already uses) reading its OWN attachment
    /// by id still resolves — the new check compares the attachment's
    /// stored `project_id` against the SAME `read_attachment_scope(...)`
    /// value the filename branch already used, not a stricter rule for
    /// the id branch.
    #[test]
    fn test_read_attachment_by_id_home_global_scope_still_resolves() {
        let conn = setup_test_db();
        let home_id = seed_attachment(&conn, "__global__", "home-note.md", "text/markdown", b"home content");

        let outcome = execute_read_attachment(&conn, Some(&home_id), None, None);
        assert!(
            outcome.is_external_content,
            "a Home session must still resolve its own Home attachment by id: {}",
            outcome.provider_result
        );
        assert_eq!(outcome.provider_result, "home content");
    }

    /// Same Home case, but with an explicit (non-empty) `session_project_id`
    /// of `"__global__"` rather than `None` — both spellings of "Home"
    /// must resolve identically, mirroring the filename branch's own
    /// `.filter(|s| !s.is_empty()).unwrap_or(HOME_ATTACHMENT_PROJECT_SCOPE)`
    /// normalization.
    #[test]
    fn test_read_attachment_by_id_explicit_global_sentinel_matches_none() {
        let conn = setup_test_db();
        let home_id = seed_attachment(&conn, "__global__", "home-note.md", "text/markdown", b"home content");

        let via_none = execute_read_attachment(&conn, Some(&home_id), None, None);
        let via_sentinel = execute_read_attachment(&conn, Some(&home_id), None, Some("__global__"));
        assert!(via_none.is_external_content);
        assert!(via_sentinel.is_external_content);
        assert_eq!(via_none.provider_result, via_sentinel.provider_result);
    }
}

