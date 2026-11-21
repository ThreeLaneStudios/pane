use crate::watcher::IMPORT_ACTIVE;
use std::sync::atomic::Ordering;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::State;
use rusqlite::params;

use crate::database::DbState;
use crate::filesystem_policy::{PaneFs, PaneRoot};
use crate::notes::{CreateNoteRequest, create_note_internal};

const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10MB

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImportNode {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub children: Vec<ImportNode>,
    pub file_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ImportResult {
    pub imported: usize,
    pub skipped: usize,
    pub projects_created: usize,
    pub folders_created: usize,
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.starts_with('.'))
        .unwrap_or(false)
}

pub(crate) fn is_importable(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some("md") | Some("txt") => true,
        _ => false,
    }
}

#[derive(Debug)]
struct ExternalTreeEntry {
    path: PathBuf,
    is_dir: bool,
    file_size: u64,
    children: Vec<ExternalTreeEntry>,
}

fn validate_external_tree(selected_root: &Path) -> Result<ExternalTreeEntry, String> {
    let root_metadata = fs::symlink_metadata(selected_root).map_err(|error| {
        format!(
            "Failed to inspect selected import folder '{}': {error}",
            selected_root.display()
        )
    })?;

    if root_metadata.file_type().is_symlink() {
        return Err(format!(
            "Selected import folder '{}' is a symbolic link; select the real directory instead",
            selected_root.display()
        ));
    }
    if !root_metadata.is_dir() {
        return Err(format!(
            "Selected import path '{}' is not a directory",
            selected_root.display()
        ));
    }

    // Keep accepting native-picker paths outside Pane, but establish one canonical
    // boundary before walking anything beneath them.
    let canonical_root = fs::canonicalize(selected_root).map_err(|error| {
        format!(
            "Failed to resolve selected import folder '{}': {error}",
            selected_root.display()
        )
    })?;
    let mut visited_directories = HashSet::new();
    walk_external_directory(&canonical_root, &canonical_root, &mut visited_directories)
}

fn walk_external_directory(
    directory: &Path,
    canonical_root: &Path,
    visited_directories: &mut HashSet<PathBuf>,
) -> Result<ExternalTreeEntry, String> {
    if !directory.starts_with(canonical_root) {
        return Err(format!(
            "Import directory '{}' resolves outside selected folder '{}'",
            directory.display(),
            canonical_root.display()
        ));
    }
    if !visited_directories.insert(directory.to_path_buf()) {
        return Err(format!(
            "Import directory cycle detected at '{}'",
            directory.display()
        ));
    }

    let read_dir = fs::read_dir(directory).map_err(|error| {
        format!(
            "Failed to read import directory '{}': {error}",
            directory.display()
        )
    })?;
    let mut entries = read_dir.collect::<Result<Vec<_>, _>>().map_err(|error| {
        format!(
            "Failed to enumerate import directory '{}': {error}",
            directory.display()
        )
    })?;
    entries.sort_by_key(|entry| entry.file_name());

    let mut children = Vec::new();
    for entry in entries {
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path).map_err(|error| {
            format!(
                "Failed to inspect import entry '{}': {error}",
                entry_path.display()
            )
        })?;

        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Import entry '{}' is a symbolic link; symbolic links are not allowed",
                entry_path.display()
            ));
        }

        let is_dir = metadata.is_dir();
        let is_file = metadata.is_file();
        if !is_dir && !is_file {
            continue;
        }

        let canonical_path = fs::canonicalize(&entry_path).map_err(|error| {
            format!(
                "Failed to resolve import entry '{}': {error}",
                entry_path.display()
            )
        })?;
        if !canonical_path.starts_with(canonical_root) {
            return Err(format!(
                "Import entry '{}' resolves outside selected folder '{}'",
                entry_path.display(),
                canonical_root.display()
            ));
        }

        // Hidden entries remain excluded from imports, but hidden directories are
        // still walked so every nested symlink is rejected.
        if is_hidden(&entry_path) {
            if is_dir {
                walk_external_directory(&canonical_path, canonical_root, visited_directories)?;
            }
            continue;
        }

        if is_dir {
            children.push(walk_external_directory(
                &canonical_path,
                canonical_root,
                visited_directories,
            )?);
        } else {
            children.push(ExternalTreeEntry {
                path: canonical_path,
                is_dir: false,
                file_size: metadata.len(),
                children: Vec::new(),
            });
        }
    }

    Ok(ExternalTreeEntry {
        path: directory.to_path_buf(),
        is_dir: true,
        file_size: 0,
        children,
    })
}

fn build_import_tree(entry: &ExternalTreeEntry) -> Result<ImportNode, String> {
    if !entry.is_dir {
        return Err(format!(
            "Cannot build an import tree from file '{}'",
            entry.path.display()
        ));
    }

    let mut children = Vec::new();
    let mut file_count = 0usize;
    for child in &entry.children {
        if child.is_dir {
            let child_tree = build_import_tree(child)?;
            if child_tree.file_count > 0 {
                file_count = file_count
                    .checked_add(child_tree.file_count)
                    .ok_or_else(|| {
                        format!(
                            "Too many importable files beneath '{}'",
                            entry.path.display()
                        )
                    })?;
                children.push(child_tree);
            }
        } else if is_importable(&child.path) && child.file_size <= MAX_FILE_SIZE {
            file_count = file_count.checked_add(1).ok_or_else(|| {
                format!(
                    "Too many importable files beneath '{}'",
                    entry.path.display()
                )
            })?;
            children.push(ImportNode {
                name: child
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                path: child.path.to_string_lossy().to_string(),
                is_dir: false,
                children: Vec::new(),
                file_count: 0,
            });
        }
    }

    Ok(ImportNode {
        name: entry
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        path: entry.path.to_string_lossy().to_string(),
        is_dir: true,
        children,
        file_count,
    })
}

fn collect_importable_files<'a>(entry: &'a ExternalTreeEntry, files: &mut Vec<&'a Path>) {
    for child in &entry.children {
        if child.is_dir {
            collect_importable_files(child, files);
        } else if is_importable(&child.path) && child.file_size <= MAX_FILE_SIZE {
            files.push(&child.path);
        }
    }
}

fn validate_import_destination_names(tree: &ExternalTreeEntry) -> Result<(), String> {
    let has_root_files = tree.children.iter().any(|entry| {
        !entry.is_dir && is_importable(&entry.path) && entry.file_size <= MAX_FILE_SIZE
    });
    if has_root_files {
        if let Some(name) = tree.path.file_name().and_then(|name| name.to_str()) {
            PaneFs::validate_name(name)
                .map_err(|error| format!("Unsafe import project name {name:?}: {error}"))?;
        }
    }

    for project in tree.children.iter().filter(|entry| entry.is_dir) {
        let Some(project_name) = project.path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        PaneFs::validate_name(project_name)
            .map_err(|error| format!("Unsafe import project name {project_name:?}: {error}"))?;
        if project_name == "__system" {
            continue;
        }

        for folder in project.children.iter().filter(|entry| entry.is_dir) {
            let Some(folder_name) = folder.path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            PaneFs::validate_name(folder_name)
                .map_err(|error| format!("Unsafe import folder name {folder_name:?}: {error}"))?;
        }
    }

    Ok(())
}

fn revalidate_external_file(
    file_path: &Path,
    canonical_root: &Path,
) -> Result<Option<PathBuf>, String> {
    let metadata = fs::symlink_metadata(file_path).map_err(|error| {
        format!(
            "Failed to re-check import file {} before reading: {error}",
            file_path.display()
        )
    })?;
    // Deliberately reject every symlink rather than following safe-looking targets.
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "Import file {} became a symbolic link before reading; symbolic links are not allowed",
            file_path.display()
        ));
    }
    if !metadata.is_file() {
        return Err(format!(
            "Import source is no longer a regular file: {}",
            file_path.display()
        ));
    }
    if metadata.len() > MAX_FILE_SIZE {
        return Ok(None);
    }

    let canonical_file = fs::canonicalize(file_path).map_err(|error| {
        format!(
            "Failed to resolve import file {} before reading: {error}",
            file_path.display()
        )
    })?;
    if !canonical_file.starts_with(canonical_root) {
        return Err(format!(
            "Import file {} resolves outside selected folder {} before reading",
            file_path.display(),
            canonical_root.display()
        ));
    }

    Ok(Some(canonical_file))
}

#[tauri::command]
pub fn scan_import_folder(path: String) -> Result<ImportNode, String> {
    let tree = validate_external_tree(Path::new(&path))?;
    build_import_tree(&tree)
}

#[tauri::command]
pub fn import_folder(path: String, db: State<DbState>) -> Result<ImportResult, String> {
    // Validate the complete external tree before taking the database lock or
    // creating any Pane-owned projects, folders, or notes.
    let external_tree = validate_external_tree(Path::new(&path))?;
    validate_import_destination_names(&external_tree)?;
    let canonical_root = external_tree.path.clone();
    let dir = &external_tree.path;
    let pane_fs = PaneFs::for_runtime()
        .map_err(|error| format!("Failed to initialize filesystem policy: {error}"))?;
    let conn = db.0.lock().unwrap();

    IMPORT_ACTIVE.store(true, Ordering::Relaxed);
    let mut imported = 0usize;
    let mut skipped = 0usize;
    let mut projects_created = 0usize;
    let mut folders_created = 0usize;

    let find_or_create_project = |conn: &rusqlite::Connection, name: &str, projects_created: &mut usize| -> Result<(String, Option<String>), String> {
        let existing: Option<String> = conn.query_row(
            "SELECT id FROM projects WHERE name = ?1 AND parent_id IS NULL",
            params![name],
            |row| row.get(0),
        ).ok();

        let project_id = if let Some(id) = existing {
            id
        } else {
            let project_relative = PaneFs::relative_from_names([name])
                .map_err(|e| format!("Unsafe import project path: {}", e))?;
            pane_fs
                .create_dir_all(PaneRoot::Projects, &project_relative)
                .map_err(|e| format!("Failed to create project dir: {}", e))?;
            let assets_relative = PaneFs::relative_from_names([name, "Assets"])
                .map_err(|e| format!("Unsafe import Assets path: {}", e))?;
            pane_fs
                .create_dir_all(PaneRoot::Projects, &assets_relative)
                .map_err(|e| format!("Failed to create Assets dir: {}", e))?;
            
            // §24: Skip default/AI folder creation for system projects
            let is_system_project = name == "__system";
            if !is_system_project {
                let general_relative = PaneFs::relative_from_names([name, crate::projects::DEFAULT_FOLDER_NAME])
                    .map_err(|e| format!("Unsafe import {} path: {}", crate::projects::DEFAULT_FOLDER_NAME, e))?;
                pane_fs
                    .create_dir_all(PaneRoot::Projects, &general_relative)
                    .map_err(|e| format!("Failed to create {} dir: {}", crate::projects::DEFAULT_FOLDER_NAME, e))?;
            }

            let id = uuid::Uuid::new_v4().to_string();
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO projects (id, name, owner_id, is_system, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, name, "user_local", if is_system_project { 1 } else { 0 }, now, now],
            ).map_err(|e| format!("Failed to insert project: {}", e))?;

            // §24: Skip default folder for system projects
            if !is_system_project {
                let folder_id = uuid::Uuid::new_v4().to_string();
                conn.execute(
                    "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![folder_id, crate::projects::DEFAULT_FOLDER_NAME, id, "user_local", now, now],
                ).map_err(|e| format!("Failed to create {} folder: {}", crate::projects::DEFAULT_FOLDER_NAME, e))?;

                // §1.7.1: the first folder a project gets is always its default folder.
                conn.execute(
                    "UPDATE projects SET default_folder_id = ?1 WHERE id = ?2",
                    params![folder_id, id],
                ).map_err(|e| format!("Failed to set default_folder_id: {}", e))?;
            }

            *projects_created += 1;
            id
        };

        // §24: Make general_id optional for system projects. §1.7.1: resolved by
        // ID (default_folder_id), never by matching the name "General".
        let general_id: Option<String> = if name != "__system" {
            conn.query_row(
                "SELECT default_folder_id FROM projects WHERE id = ?1",
                params![project_id],
                |row| row.get::<_, Option<String>>(0),
            ).ok().flatten()
        } else {
            None
        };

        Ok((project_id, general_id))
    };

    let find_or_create_subfolder = |conn: &rusqlite::Connection, parent_id: &str, name: &str, folders_created: &mut usize| -> Result<String, String> {
        let existing: Option<String> = conn.query_row(
            "SELECT id FROM projects WHERE name = ?1 AND parent_id = ?2",
            params![name, parent_id],
            |row| row.get(0),
        ).ok();

        if let Some(id) = existing {
            return Ok(id);
        }

        // §1.7.2: no sibling may already have this name (case-insensitive) — the
        // exact-match SELECT above can miss a case-sensitivity collision.
        crate::projects::check_folder_name_unique(conn, parent_id, name, "")?;

        // Authorize and create the destination before inserting its DB row.
        let project_name: String = conn.query_row(
            "SELECT name FROM projects WHERE id = ?1", params![parent_id], |row| row.get(0),
        ).map_err(|e| format!("Failed to resolve import project name: {}", e))?;
        let folder_relative = PaneFs::relative_from_names([project_name.as_str(), name])
            .map_err(|e| format!("Unsafe import folder path: {}", e))?;
        pane_fs.create_dir_all(PaneRoot::Projects, &folder_relative)
            .map_err(|e| format!("Failed to create folder dir: {}", e))?;

        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO projects (id, name, parent_id, owner_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![id, name, parent_id, "user_local", now, now],
        ).map_err(|e| format!("Failed to create folder: {}", e))?;

        *folders_created += 1;
        Ok(id)
    };

    let import_file = |conn: &rusqlite::Connection,
                       file_path: &Path,
                       project_id: &str,
                       folder_id: &str,
                       imported: &mut usize,
                       skipped: &mut usize|
     -> Result<(), String> {
        // The shared external-tree walk supplies canonical, validated paths.
        let abs_path = file_path.to_string_lossy().to_string();
        
        // Check if already imported by file_path
        let already: bool = conn
            .query_row(
            "SELECT COUNT(*) > 0 FROM notes WHERE file_path = ?1 AND deleted_at IS NULL",
            params![abs_path],
            |row| row.get(0),
            )
            .map_err(|e| {
                format!(
                    "Failed to check existing import {}: {}",
                    file_path.display(),
                    e
                )
            })?;

        if already {
            *skipped += 1;
            return Ok(());
        }

        // Case-insensitive title dedup (handles README.md vs readme.md on macOS)
        let title_check = file_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let title_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM notes WHERE lower(title) = lower(?1) AND folder_id IS ?2 AND project_id = ?3 AND deleted_at IS NULL",
            params![title_check, folder_id, project_id],
            |row| row.get(0),
        ).map_err(|e| format!("Failed to check duplicate note {}: {}", file_path.display(), e))?;
        if title_exists {
            *skipped += 1;
            return Ok(());
        }

        let title = file_path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let canonical_file = match revalidate_external_file(file_path, &canonical_root)? {
            Some(path) => path,
            None => {
                *skipped += 1;
                return Ok(());
            }
        };
        let content = fs::read_to_string(&canonical_file).map_err(|e| {
            format!(
                "Failed to read import file {}: {}",
                canonical_file.display(),
                e
            )
        })?;

        let req = CreateNoteRequest {
            project_id: project_id.to_string(),
            folder_id: Some(folder_id.to_string()),
            title,
            content: Some(content),
            tags: None,
            status: Some("cabinet".to_string()),
            priority: None,
            due_date: None,
            parent_id: None,
        };

        match create_note_internal(conn, req) {
            Ok(meta) => {
                // Store MANAGED file path (where Pane wrote it), not original source
                // This prevents the file watcher from re-importing the same file
                let managed_file_path = if !meta.file_path.is_empty() {
                    meta.file_path.clone()
                } else {
                    abs_path.clone()
                };
                conn.execute(
                    "UPDATE notes SET file_path = ?1 WHERE id = ?2",
                    params![managed_file_path, meta.id],
                )
                .map_err(|e| format!("Failed to update imported note path: {}", e))?;
                
                // Detect daily logs: files in logs/ or memory/ folders with YYYY-MM-DD filename
                let file_name_str = file_path.file_stem().and_then(|n| n.to_str()).unwrap_or("");
                let parent_folder_name = file_path
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("");

                let is_daily = (parent_folder_name == "logs" || parent_folder_name == "memory")
                    && file_name_str.len() == 10
                    && file_name_str.contains('-')
                    && file_name_str
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == '-');

                if is_daily {
                    conn.execute(
                        "UPDATE notes SET note_type = 'daily', title = ?1 WHERE id = ?2",
                        params![file_name_str, meta.id],
                    )
                    .map_err(|e| format!("Failed to tag imported daily note: {}", e))?;
                    eprintln!("[import] Tagged daily log: {} ({})", file_name_str, meta.id);
                }
                
                *imported += 1;
                Ok(())
            }
            Err(e) if e.starts_with("DUPLICATE_NOTE") => {
                *skipped += 1;
                Ok(())
            }
            Err(e) => Err(format!(
                "Failed to import note from {}: {}",
                file_path.display(),
                e
            )),
        }
    };

    // Root-level importable files → a project named after the selected folder.
    let root_files: Vec<&Path> = external_tree
        .children
        .iter()
        .filter(|entry| {
            !entry.is_dir && is_importable(&entry.path) && entry.file_size <= MAX_FILE_SIZE
        })
        .map(|entry| entry.path.as_path())
        .collect();

    if !root_files.is_empty() {
        let import_project_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Imported")
            .to_string();
        let (proj_id, folder_id_opt) =
            find_or_create_project(&conn, &import_project_name, &mut projects_created)?;
        if let Some(folder_id) = folder_id_opt {
            for f in root_files {
                import_file(&conn, f, &proj_id, &folder_id, &mut imported, &mut skipped)?;
            }
        }
    }

    // Subdirectories → one project per top-level subdir
    for subdirectory in external_tree.children.iter().filter(|entry| entry.is_dir) {
        let folder_name = subdirectory
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if folder_name.is_empty() {
            continue;
        }

        let (proj_id, general_folder_id_opt) =
            find_or_create_project(&conn, &folder_name, &mut projects_created)?;
        let general_folder_id = match general_folder_id_opt {
            Some(id) => id,
            None => {
                // For projects without General (e.g., system projects), skip root-level file imports
                eprintln!(
                    "[import] Skipping root files for project {} (no General folder)",
                    folder_name
                );
                continue;
            }
        };

        // Import direct files into General; flatten each immediate child directory
        // into one Pane subfolder, preserving the existing import behavior.
        for child in &subdirectory.children {
            if child.is_dir {
                let sub_name = child
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                if sub_name.is_empty() {
                    continue;
                    }
                    
                let sub_folder_id =
                    find_or_create_subfolder(&conn, &proj_id, &sub_name, &mut folders_created)?;
                let mut files = Vec::new();
                collect_importable_files(child, &mut files);
                for file in files {
                    import_file(
                        &conn,
                        file,
                        &proj_id,
                        &sub_folder_id,
                        &mut imported,
                        &mut skipped,
                    )?;
                }
            } else if is_importable(&child.path) && child.file_size <= MAX_FILE_SIZE {
                import_file(
                    &conn,
                    &child.path,
                    &proj_id,
                    &general_folder_id,
                    &mut imported,
                    &mut skipped,
                )?;
            }
        }
    }

    Ok(ImportResult {
        imported,
        skipped,
        projects_created,
        folders_created,
    })
}
