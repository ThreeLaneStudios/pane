fn expand_openclaw_path(
    raw: &str,
    home: &std::path::Path,
    relative_root: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    if raw.contains(['\\', '\0'])
        || std::path::Path::new(raw)
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir | std::path::Component::CurDir))
    {
        return Err("OpenClaw path traversal rejected".to_string());
    }
    let path = if raw == "~" {
        home.to_path_buf()
    } else if let Some(relative) = raw.strip_prefix("~/") {
        let relative = std::path::Path::new(relative);
        crate::filesystem_policy::PaneFs::validate_relative(relative)
            .map_err(|e| format!("Invalid home-relative OpenClaw path: {}", e))?;
        home.join(relative)
    } else if raw.starts_with('~') {
        return Err("Unsupported OpenClaw home path".to_string());
    } else {
        let path = std::path::PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            relative_root.join(path)
        }
    };
    Ok(path)
}

fn canonical_existing_directory(
    path: &std::path::Path,
    label: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to inspect {} {}: {}", label, path.display(), error)),
    };
    if !metadata.is_dir() && !metadata.file_type().is_symlink() {
        return Err(format!("{} is not a directory: {}", label, path.display()));
    }
    let canonical = std::fs::canonicalize(path)
        .map_err(|e| format!("Failed to canonicalize {} {}: {}", label, path.display(), e))?;
    if !std::fs::metadata(&canonical).map_err(|e| e.to_string())?.is_dir() {
        return Err(format!("{} is not a directory: {}", label, canonical.display()));
    }
    Ok(Some(canonical))
}

fn canonical_openclaw_root(home: &std::path::Path) -> Result<std::path::PathBuf, String> {
    canonical_existing_directory(&home.join(".openclaw"), "OpenClaw root")?
        .ok_or_else(|| "OpenClaw root not found".to_string())
}

fn authorize_openclaw_config(
    requested: Option<&str>,
    home: &std::path::Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf), String> {
    let openclaw_root = canonical_openclaw_root(home)?;
    let candidate = match requested {
        Some(path) => expand_openclaw_path(path, home, &openclaw_root)?,
        None => openclaw_root.join("openclaw.json"),
    };
    let canonical = std::fs::canonicalize(&candidate)
        .map_err(|e| format!("OpenClaw config not found: {}", e))?;
    if canonical == openclaw_root || !canonical.starts_with(&openclaw_root) {
        return Err("OpenClaw config path is outside ~/.openclaw".to_string());
    }
    if !std::fs::metadata(&canonical).map_err(|e| e.to_string())?.is_file() {
        return Err("OpenClaw config path is not a file".to_string());
    }
    Ok((canonical, openclaw_root))
}

fn openclaw_workspace_declarations(config: &serde_json::Value) -> Vec<&str> {
    let Some(agents) = config.get("agents").and_then(|value| value.as_object()) else {
        return Vec::new();
    };
    let mut workspaces: Vec<&str> = agents
        .get("list")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|agent| agent.get("workspace").and_then(|value| value.as_str()))
        .collect();
    workspaces.extend(
        agents
            .iter()
            .filter(|(key, _)| key.as_str() != "list")
            .filter_map(|(_, agent)| agent.get("workspace").and_then(|value| value.as_str())),
    );
    workspaces
}

fn canonical_declared_workspaces(
    config: &serde_json::Value,
    home: &std::path::Path,
    config_root: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, String> {
    let mut workspaces = Vec::new();
    for raw in openclaw_workspace_declarations(config) {
        let path = expand_openclaw_path(raw, home, config_root)?;
        if let Some(canonical) = canonical_existing_directory(&path, "declared OpenClaw workspace")? {
            workspaces.push(canonical);
        }
    }
    Ok(workspaces)
}

fn workspace_is_authorized(
    workspace: &std::path::Path,
    config_root: &std::path::Path,
    declared_workspaces: &[std::path::PathBuf],
) -> bool {
    (workspace != config_root && workspace.starts_with(config_root))
        || declared_workspaces.iter().any(|declared| declared == workspace)
}

fn authorize_workspace_candidate(
    candidate: &std::path::Path,
    config_root: &std::path::Path,
    declared_workspaces: &[std::path::PathBuf],
) -> Result<Option<std::path::PathBuf>, String> {
    let Some(canonical) = canonical_existing_directory(candidate, "OpenClaw workspace")? else {
        return Ok(None);
    };
    if !workspace_is_authorized(&canonical, config_root, declared_workspaces) {
        return Err(format!("OpenClaw workspace is outside the authorized boundary: {}", canonical.display()));
    }
    Ok(Some(canonical))
}

fn authorize_renderer_workspace(
    requested: &str,
    home: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let openclaw_root = canonical_openclaw_root(home)?;
    let candidate = expand_openclaw_path(requested, home, &openclaw_root)?;
    let canonical = canonical_existing_directory(&candidate, "OpenClaw workspace")?
        .ok_or_else(|| "OpenClaw workspace not found".to_string())?;
    if canonical != openclaw_root && canonical.starts_with(&openclaw_root) {
        return Ok(canonical);
    }

    let (config_path, _) = authorize_openclaw_config(None, home)?;
    let config = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read openclaw.json: {}", e))?;
    let config: serde_json::Value = serde_json::from_str(&config)
        .map_err(|e| format!("Invalid openclaw.json: {}", e))?;
    let declared = canonical_declared_workspaces(&config, home, &openclaw_root)?;
    if declared.iter().any(|workspace| workspace == &canonical) {
        Ok(canonical)
    } else {
        Err("Requested workspace is not authorized by OpenClaw configuration".to_string())
    }
}

fn authorized_fixed_workspace_file(
    workspace: &std::path::Path,
    filename: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    crate::filesystem_policy::PaneFs::validate_name(filename)
        .map_err(|e| format!("Invalid OpenClaw filename: {}", e))?;
    let candidate = workspace.join(filename);
    match std::fs::symlink_metadata(&candidate) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Failed to inspect OpenClaw file: {}", error)),
    }
    let canonical = std::fs::canonicalize(&candidate)
        .map_err(|e| format!("Failed to canonicalize OpenClaw file: {}", e))?;
    if canonical == workspace || !canonical.starts_with(workspace) {
        return Err(format!("OpenClaw file escapes its workspace: {}", candidate.display()));
    }
    if !std::fs::metadata(&canonical).map_err(|e| e.to_string())?.is_file() {
        return Ok(None);
    }
    Ok(Some(canonical))
}

/// Normalize system filename to canonical ALL CAPS form
/// Prevents case collisions on macOS case-insensitive filesystems
fn canonical_system_filename(name: &str) -> String {
    match name.to_lowercase().as_str() {
        "memory.md" => "MEMORY.md".to_string(),
        "soul.md" => "SOUL.md".to_string(),
        "identity.md" => "IDENTITY.md".to_string(),
        "user.md" => "USER.md".to_string(),
        "tools.md" => "TOOLS.md".to_string(),
        "agents.md" => "AGENTS.md".to_string(),
        "heartbeat.md" => "HEARTBEAT.md".to_string(),
        "rules.md" => "RULES.md".to_string(),
        _ => name.to_string(),
    }
}

fn read_fixed_workspace_files(
    workspace: &std::path::Path,
) -> Result<Vec<(String, String)>, String> {
    let file_names = ["SOUL.md", "MEMORY.md", "AGENTS.md", "IDENTITY.md", "USER.md", "TOOLS.md", "HEARTBEAT.md", "RULES.md"];
    let mut results = Vec::new();
    for name in &file_names {
        if let Some(path) = authorized_fixed_workspace_file(workspace, name)? {
            match std::fs::read_to_string(&path) {
                Ok(content) => results.push((name.to_string(), content)),
                Err(e) => eprintln!("Failed to read {}: {}", name, e),
            }
        }
    }
    Ok(results)
}

/// Read OpenClaw workspace files directly from the filesystem
/// This works for same-machine setups (most common case)
#[tauri::command]
pub fn read_openclaw_workspace() -> Result<Vec<(String, String)>, String> {
    let home = std::env::var("HOME").map_err(|_| "No HOME env")?;
    let home = std::path::Path::new(&home);
    let default_workspace = home.join(".openclaw").join("workspace");
    if std::fs::symlink_metadata(&default_workspace)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(Vec::new());
    }
    let workspace = authorize_renderer_workspace("~/.openclaw/workspace", home)?;
    read_fixed_workspace_files(&workspace)
}

/// Read OpenClaw workspace files from a user-specified path
/// Used when OpenClaw runs on a different machine and the workspace is shared
#[tauri::command]
pub fn read_workspace_from_path(path: String) -> Result<Vec<(String, String)>, String> {
    let home = std::env::var("HOME").map_err(|_| "No HOME env")?;
    let workspace = authorize_renderer_workspace(&path, std::path::Path::new(&home))?;
    read_fixed_workspace_files(&workspace)
}



/// Read OpenClaw config to discover agents
/// Optionally accepts a config_path for custom workspace locations
#[tauri::command]
pub fn read_openclaw_agents(config_path: Option<String>) -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|_| "No HOME env")?;
    let home = std::path::PathBuf::from(home);
    let (resolved_path, _openclaw_root) = authorize_openclaw_config(config_path.as_deref(), &home)?;
    
    let config = std::fs::read_to_string(&resolved_path)
        .map_err(|e| format!("Failed to read openclaw.json: {}", e))?;
    
    // Parse to extract agents section and read their workspace files
    let json: serde_json::Value = serde_json::from_str(&config)
        .map_err(|e| format!("Invalid JSON: {}", e))?;
    let config_root = resolved_path
        .parent()
        .ok_or_else(|| "OpenClaw config has no parent directory".to_string())?;
    let declared_workspaces = canonical_declared_workspaces(&json, &home, config_root)?;
    
    let mut result = serde_json::Map::new();
    
    // Support both agents.list (array) and agents as flat object
    let agent_entries: Vec<(String, &serde_json::Value)> = if let Some(agents_obj) = json.get("agents") {
        if let Some(list) = agents_obj.get("list").and_then(|l| l.as_array()) {
            // agents.list is an array of objects with "id" field
            list.iter().filter_map(|a| {
                a.get("id").and_then(|id| id.as_str()).map(|id| (id.to_string(), a))
            }).collect()
        } else if let Some(obj) = agents_obj.as_object() {
            // agents is a flat object with named keys
            obj.iter().filter(|(k, _)| k.as_str() != "defaults").map(|(k, v)| (k.clone(), v)).collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    for (id, agent_config) in &agent_entries {
        let mut agent_info = serde_json::Map::new();
        
        // Copy agent config fields
        if let Some(obj) = agent_config.as_object() {
            for (k, v) in obj {
                agent_info.insert(k.clone(), v.clone());
            }
        }

        // Extract model string from nested model.primary or flat model field
        if let Some(model_obj) = agent_config.get("model") {
            if let Some(primary) = model_obj.get("primary").and_then(|p| p.as_str()) {
                agent_info.insert("model_display".to_string(), serde_json::Value::String(primary.to_string()));
            }
        }

        // Extract name from identity.name or top-level name
        let mut display_name: Option<String> = None;
        if let Some(identity) = agent_config.get("identity").and_then(|i| i.as_object()) {
            if let Some(name) = identity.get("name").and_then(|n| n.as_str()) {
                display_name = Some(name.to_string());
            }
        }
        // If display_name matches the agent id (generic), try extracting from SOUL.md first line
        if display_name.as_deref() == Some(id.as_str()) || display_name.is_none() {
            // Will be populated after reading workspace files below
        }
        if let Some(ref dn) = display_name {
            agent_info.insert("display_name".to_string(), serde_json::Value::String(dn.clone()));
        }
        
        // Try to read agent workspace files (Soul.md, Memory.md)
        let ws_paths: Vec<std::path::PathBuf> = {
            let mut paths = Vec::new();
            if let Some(workspace) = agent_config.get("workspace").and_then(|w| w.as_str()) {
                let configured_path = expand_openclaw_path(workspace, &home, config_root)?;
                if let Some(authorized) = authorize_workspace_candidate(
                    &configured_path,
                    config_root,
                    &declared_workspaces,
                )? {
                    paths.push(authorized);
                } else if let Some(relative) = workspace.strip_prefix("~/") {
                    if let Some(ws_name) = std::path::Path::new(relative).file_name().and_then(|name| name.to_str()) {
                        crate::filesystem_policy::PaneFs::validate_name(ws_name)
                            .map_err(|e| format!("Invalid OpenClaw workspace name: {}", e))?;
                        let shared_path = config_root.join(ws_name);
                        if let Some(authorized) = authorize_workspace_candidate(
                            &shared_path,
                            config_root,
                            &declared_workspaces,
                        )? {
                            paths.push(authorized);
                        }
                    }
                }
            }

            // Also check the config-root agents directory.
            crate::filesystem_policy::PaneFs::validate_name(id)
                .map_err(|e| format!("Invalid OpenClaw agent id: {}", e))?;
            let shared_agent_dir = config_root.join("agents").join(id);
            if let Some(authorized) = authorize_workspace_candidate(
                &shared_agent_dir,
                config_root,
                &declared_workspaces,
            )? {
                if !paths.contains(&authorized) {
                    paths.push(authorized);
                            }
                        }
            paths
        };

        for ws_path in &ws_paths {
            let mut found_any = false;
            // Check both canonical and legacy variants during migration
            for fname_raw in &["SOUL.md", "Soul.md", "MEMORY.md", "Memory.md"] {
                let fname = canonical_system_filename(fname_raw);
                eprintln!("[CASE-FIX] Normalized {} → {}", fname_raw, fname);
                
                // Try canonical first, then legacy variant
                for candidate in &[fname.as_str(), *fname_raw] {
                    if let Some(fpath) = authorized_fixed_workspace_file(ws_path, candidate)? {
                        if let Ok(content) = std::fs::read_to_string(&fpath) {
                            let key = fname.replace(".md", "").to_lowercase();
                            agent_info.insert(key, serde_json::Value::String(content));
                            found_any = true;
                            break; // Found this file, move to next
                        }
                    }
                }
            }
            if found_any { break; }
        }

        // Best-effort: if display_name matches id (generic), try SOUL.md first line for a better name
        let needs_name = display_name.as_deref() == Some(id.as_str()) || display_name.is_none();
        if needs_name {
            if let Some(soul_val) = agent_info.get("soul").and_then(|v| v.as_str()) {
                // Look for "# Name" heading or "**Name:**" pattern in first few lines
                for line in soul_val.lines().take(5) {
                    let trimmed = line.trim();
                    if let Some(heading) = trimmed.strip_prefix("# ") {
                        let name = heading.trim().to_string();
                        if !name.is_empty() && name.len() < 40 {
                            agent_info.insert("display_name".to_string(), serde_json::Value::String(name));
                            break;
                        }
                    } else if trimmed.starts_with("**") && trimmed.contains(":**") {
                        // e.g. "**Name:** Stia" or "**Name**: Stia"
                        if let Some(after) = trimmed.split(":**").nth(1).or_else(|| trimmed.split(":*").nth(1)) {
                            let name = after.trim().trim_matches('*').trim().to_string();
                            if !name.is_empty() && name.len() < 40 {
                                agent_info.insert("display_name".to_string(), serde_json::Value::String(name));
                                break;
                            }
                        }
                    }
                }
            }
        }
        
        result.insert(id.clone(), serde_json::Value::Object(agent_info));
    }
    
    serde_json::to_string(&result).map_err(|e| format!("Serialize failed: {}", e))
}

fn pane_directory_exists_without_symlinks(
    path: &std::path::Path,
    label: &str,
) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!("Symlink {} rejected: {}", label, path.display()));
            }
            if !metadata.is_dir() {
                return Err(format!("{} is not a directory: {}", label, path.display()));
            }
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("Failed to inspect {} {}: {}", label, path.display(), error)),
    }
}

fn authorize_optional_system_file(
    pane_fs: &crate::filesystem_policy::PaneFs,
    agent_dir: &crate::filesystem_policy::AuthorizedPath,
    agent_relative: &std::path::Path,
    filename: &str,
) -> Result<Option<crate::filesystem_policy::AuthorizedPath>, String> {
    crate::filesystem_policy::PaneFs::validate_name(filename)
        .map_err(|e| format!("Invalid system filename: {}", e))?;
    let candidate = agent_dir.as_path().join(filename);
    match std::fs::symlink_metadata(&candidate) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(format!("Symlink system file rejected: {}", candidate.display()));
        }
    }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("Failed to inspect system file {}: {}", candidate.display(), error));
        }
    }

    let relative = agent_relative.join(filename);
    let authorized = pane_fs
        .authorize_existing(
            crate::filesystem_policy::PaneRoot::System,
            &relative,
            crate::filesystem_policy::ExistingKind::File,
        )
        .map_err(|e| format!("Unsafe system file {}: {}", candidate.display(), e))?;
    Ok(Some(authorized))
}

/// Discover agents from the Pane .pane/system folders
#[tauri::command]
pub fn discover_pane_agents() -> Result<Vec<serde_json::Value>, String> {
    let home = std::env::var("HOME").map_err(|_| "No HOME env")?;
    let pane_root = crate::database::get_pane_root();
    let pane_fs = crate::filesystem_policy::PaneFs::from_pane_root(&pane_root)
        .map_err(|e| format!("Filesystem policy error: {}", e))?;
    let internal_dir = pane_root.join(".pane");
    let system_dir = internal_dir.join("system");
    
    let mut agents = Vec::new();
    let mut discovered_names = std::collections::HashSet::new();
    
    // BUG 4 FIX: First, scan the Pane .pane/system directory for filesystem agents
    let internal_exists = pane_directory_exists_without_symlinks(&internal_dir, "Pane internal directory")?;
    let system_exists = internal_exists
        && pane_directory_exists_without_symlinks(&system_dir, "Pane system directory")?;
    if system_exists {
        for entry in std::fs::read_dir(&system_dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let entry_type = entry.file_type().map_err(|e| e.to_string())?;
            if entry_type.is_symlink() {
                return Err(format!("Symlink agent directory rejected: {}", entry.path().display()));
            }
            if !entry_type.is_dir() {
                continue;
            }
            
            let folder_name = entry.file_name()
                .to_str()
                .ok_or_else(|| "Agent directory name is not valid UTF-8".to_string())?
                .to_string();
            crate::filesystem_policy::PaneFs::validate_name(&folder_name)
                .map_err(|e| format!("Invalid agent directory name: {}", e))?;
            let agent_relative = crate::filesystem_policy::PaneFs::relative_from_names([
                folder_name.as_str(),
            ]).map_err(|e| format!("Invalid agent directory path: {}", e))?;
            let agent_dir = pane_fs
                .authorize_existing(
                    crate::filesystem_policy::PaneRoot::System,
                    &agent_relative,
                    crate::filesystem_policy::ExistingKind::Directory,
                )
                .map_err(|e| format!("Unsafe agent directory: {}", e))?;
            
            // Check what files exist in this agent folder (support both variants during migration)
            let soul_canonical = authorize_optional_system_file(
                &pane_fs,
                &agent_dir,
                &agent_relative,
                "SOUL.md",
            )?;
            let soul_legacy = authorize_optional_system_file(
                &pane_fs,
                &agent_dir,
                &agent_relative,
                "Soul.md",
            )?;
            let memory_canonical = authorize_optional_system_file(
                &pane_fs,
                &agent_dir,
                &agent_relative,
                "MEMORY.md",
            )?;
            let memory_legacy = authorize_optional_system_file(
                &pane_fs,
                &agent_dir,
                &agent_relative,
                "Memory.md",
            )?;
            let identity_file = authorize_optional_system_file(
                &pane_fs,
                &agent_dir,
                &agent_relative,
                "IDENTITY.md",
            )?;

            let soul_exists = soul_canonical.is_some() || soul_legacy.is_some();
            let memory_exists = memory_canonical.is_some() || memory_legacy.is_some();
            let identity_exists = identity_file.is_some();
            
            // Read first 200 chars of SOUL.md as preview (check both variants)
            let soul_preview = soul_canonical
                .as_ref()
                .or(soul_legacy.as_ref())
                .and_then(|p| std::fs::read_to_string(p.as_path()).ok())
                .map(|content| {
                    // Skip the first line (title) and get next ~200 chars
                    let lines: Vec<&str> = content.lines().skip(1).collect();
                    let text = lines.join(" ").trim().to_string();
                    if text.len() > 200 { text[..200].to_string() + "..." } else { text }
                })
                .unwrap_or_default();
            
            // Read first 200 chars of MEMORY.md as preview (check both variants)
            let memory_preview = memory_canonical
                .as_ref()
                .or(memory_legacy.as_ref())
                .and_then(|p| std::fs::read_to_string(p.as_path()).ok())
                .map(|content| {
                    let lines: Vec<&str> = content.lines().skip(1).collect();
                    let text = lines.join(" ").trim().to_string();
                    if text.len() > 200 { text[..200].to_string() + "..." } else { text }
                })
                .unwrap_or_default();
            
            let agent = serde_json::json!({
                "id": folder_name.to_lowercase().replace(' ', "-"),
                "name": folder_name,
                "has_soul": soul_exists,
                "has_memory": memory_exists,
                "has_identity": identity_exists,
                "soul_preview": soul_preview,
                "memory_preview": memory_preview,
                "folder_path": agent_dir.as_path().to_string_lossy().to_string(),
            });
            
            discovered_names.insert(folder_name.clone());
            agents.push(agent);
        }
    }
    
    // BUG 4 FIX: Also check the authorized default OpenClaw config for undiscovered agents.
    let default_config = std::path::PathBuf::from(&home).join(".openclaw").join("openclaw.json");
    if std::fs::symlink_metadata(&default_config).is_ok() {
        let (config_path, _openclaw_root) = authorize_openclaw_config(
            None,
            std::path::Path::new(&home),
        )?;
        let config_str = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read openclaw.json: {}", e))?;
        let config = serde_json::from_str::<serde_json::Value>(&config_str)
            .map_err(|e| format!("Invalid openclaw.json: {}", e))?;
        let config_root = config_path
            .parent()
            .ok_or_else(|| "OpenClaw config has no parent directory".to_string())?;
        let declared_workspaces = canonical_declared_workspaces(
            &config,
            std::path::Path::new(&home),
            config_root,
        )?;

                if let Some(agent_list) = config.get("agents").and_then(|a| a.get("list")).and_then(|l| l.as_array()) {
                    for agent_config in agent_list {
                        let agent_name = agent_config.get("name").and_then(|n| n.as_str()).unwrap_or("Unknown");
                        let agent_id = agent_config.get("id").and_then(|i| i.as_str()).unwrap_or("unknown");
                        let workspace_path_str = agent_config.get("workspace").and_then(|w| w.as_str()).unwrap_or("");
                        
                // Skip if already discovered from the Pane filesystem.
                if discovered_names.contains(agent_name) || workspace_path_str.is_empty() {
                            continue;
                        }
                        
                let requested_workspace = expand_openclaw_path(
                    workspace_path_str,
                    std::path::Path::new(&home),
                    config_root,
                )?;
                let Some(workspace_path) = authorize_workspace_candidate(
                    &requested_workspace,
                    config_root,
                    &declared_workspaces,
                )? else {
                            continue;
                };
                        
                let soul_canonical = authorized_fixed_workspace_file(&workspace_path, "SOUL.md")?;
                let soul_legacy = authorized_fixed_workspace_file(&workspace_path, "Soul.md")?;
                let memory_canonical = authorized_fixed_workspace_file(&workspace_path, "MEMORY.md")?;
                let memory_legacy = authorized_fixed_workspace_file(&workspace_path, "Memory.md")?;
                let identity_file = authorized_fixed_workspace_file(&workspace_path, "IDENTITY.md")?;

                let soul_exists = soul_canonical.is_some() || soul_legacy.is_some();
                let memory_exists = memory_canonical.is_some() || memory_legacy.is_some();
                let identity_exists = identity_file.is_some();

                // Read SOUL.md preview.
                let soul_preview = soul_canonical
                    .as_ref()
                    .or(soul_legacy.as_ref())
                    .and_then(|path| std::fs::read_to_string(path).ok())
                            .map(|content| {
                                let lines: Vec<&str> = content.lines().skip(1).collect();
                                let text = lines.join(" ").trim().to_string();
                                if text.len() > 200 { text[..200].to_string() + "..." } else { text }
                            })
                            .unwrap_or_default();
                        
                // Read MEMORY.md preview.
                let memory_preview = memory_canonical
                    .as_ref()
                    .or(memory_legacy.as_ref())
                    .and_then(|path| std::fs::read_to_string(path).ok())
                            .map(|content| {
                                let lines: Vec<&str> = content.lines().skip(1).collect();
                                let text = lines.join(" ").trim().to_string();
                                if text.len() > 200 { text[..200].to_string() + "..." } else { text }
                            })
                            .unwrap_or_default();
                        
                        let agent = serde_json::json!({
                            "id": agent_id,
                            "name": agent_name,
                            "has_soul": soul_exists,
                            "has_memory": memory_exists,
                            "has_identity": identity_exists,
                            "soul_preview": soul_preview,
                            "memory_preview": memory_preview,
                            "folder_path": workspace_path.to_string_lossy().to_string(),
                        });
                        
                        agents.push(agent);
                    }
                }
            }
    
    Ok(agents)
}

const MAX_GATEWAY_CHAT_BODY_BYTES: usize = 1024 * 1024;

/// Build a fixed-purpose URL from a user-configured service endpoint.
/// The WebView never supplies the request URL, path, query, or credentials.
fn configured_service_url(endpoint: &str, fixed_path: &str) -> Result<url::Url, String> {
    let mut parsed = url::Url::parse(endpoint)
        .map_err(|e| format!("Invalid configured service URL: {}", e))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Configured service URL must use http or https".to_string());
    }
    if parsed.host().is_none() {
        return Err("Configured service URL must include a host".to_string());
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err("Configured service URL must not include credentials".to_string());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("Configured service URL must not include a query or fragment".to_string());
    }

    let existing_path = parsed.path().trim_end_matches('/');
    let base_path = existing_path.strip_suffix("/v1").unwrap_or(existing_path);
    parsed.set_path(&format!("{}{}", base_path, fixed_path));
    parsed.set_query(None);
    parsed.set_fragment(None);
    Ok(parsed)
}

/// Send a chat request only to the configured, paired Pane Gateway.
/// The destination and bearer token are backend-owned; the WebView supplies only JSON.
#[tauri::command]
pub async fn gateway_chat_completions(
    body: String,
    db: tauri::State<'_, crate::database::DbState>,
) -> Result<String, String> {
    if body.len() > MAX_GATEWAY_CHAT_BODY_BYTES {
        return Err(format!("Gateway chat request exceeds {} byte limit", MAX_GATEWAY_CHAT_BODY_BYTES));
    }
    serde_json::from_str::<serde_json::Value>(&body)
        .map_err(|e| format!("Gateway chat body must be valid JSON: {}", e))?;

    let (url, token) = {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
        let endpoint = crate::database::get_ai_setting(&conn, "endpoint")
            .ok_or_else(|| "No Pane Gateway endpoint configured".to_string())?;
        let token = crate::ai::credential_migration::read_gateway_token(&conn)
            .ok_or_else(|| "No Pane Gateway token configured".to_string())?;
        (configured_service_url(&endpoint, "/v1/chat/completions")?, token)
    };

    let client = crate::gateway_client::build_gateway_client(&db)?;
    let response = client
        .post(url)
        .header("Content-Type", "application/json")
        .bearer_auth(token)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Gateway request failed: {}", e))?;
    let status = response.status();
    let text = response.text().await.map_err(|e| format!("Failed to read Gateway response: {}", e))?;
    if !status.is_success() {
        return Err(format!("Gateway HTTP {}: {}", status, text));
    }
    Ok(text)
}

/// Check the configured legacy Bridge health endpoint without exposing arbitrary fetch.
#[tauri::command]
pub async fn bridge_health_check(
    db: tauri::State<'_, crate::database::DbState>,
) -> Result<(), String> {
    let (mut url, token) = {
        let conn = db.0.lock().map_err(|e| format!("Failed to lock database: {}", e))?;
        let endpoint = crate::database::get_ai_setting(&conn, "bridge_url")
            .ok_or_else(|| "No Bridge URL configured".to_string())?;
        let token = crate::database::get_ai_setting(&conn, "bridge_token")
            .ok_or_else(|| "No Bridge token configured".to_string())?;
        (configured_service_url(&endpoint, "/sync/health")?, token)
    };
    url.query_pairs_mut().append_pair("code", &token);

    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?
        .get(url)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| format!("Bridge health request failed: {}", e))?;
    if !response.status().is_success() {
        return Err(format!("Bridge health returned HTTP {}", response.status()));
    }
    Ok(())
}

/// Create logs directory for a specific agent during onboarding
/// Used to set up the directory structure for daily logs
#[tauri::command]
pub fn ensure_logs_directory(agent_id: String) -> Result<(), String> {
    let db_path = crate::database::get_db_path();
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| format!("Failed to open DB: {}", e))?;
    
    let workspace_path: String = conn.query_row(
        "SELECT workspace_path FROM ai_agents WHERE id = ?1",
        rusqlite::params![&agent_id],
        |row| row.get(0),
    ).map_err(|e| format!("Agent not found: {}", e))?;
    crate::filesystem_policy::PaneFs::validate_name(&workspace_path)
        .map_err(|e| format!("Invalid agent workspace path: {}", e))?;

    let pane_fs = crate::filesystem_policy::PaneFs::from_pane_root(
        crate::database::get_pane_root(),
    ).map_err(|e| format!("Filesystem policy error: {}", e))?;
    let logs_relative = crate::filesystem_policy::PaneFs::relative_from_names([
        workspace_path.as_str(),
        "logs",
    ]).map_err(|e| format!("Invalid logs directory path: {}", e))?;
    pane_fs
        .create_dir_all(crate::filesystem_policy::PaneRoot::System, &logs_relative)
        .map_err(|e| format!("Failed to create logs dir: {}", e))?;
    Ok(())
}

/// Write a log file to an agent's logs/ subdirectory
/// Used during onboarding to import daily memory logs from OpenClaw
#[tauri::command]
pub fn write_log_file(agent_id: String, filename: String, content: String) -> Result<(), String> {
    let db_path = crate::database::get_db_path();
    let conn = rusqlite::Connection::open(&db_path)
        .map_err(|e| format!("Failed to open DB: {}", e))?;
    
    let workspace_path: String = conn.query_row(
        "SELECT workspace_path FROM ai_agents WHERE id = ?1",
        rusqlite::params![&agent_id],
        |row| row.get(0),
    ).map_err(|e| format!("Agent not found: {}", e))?;
    crate::filesystem_policy::PaneFs::validate_name(&workspace_path)
        .map_err(|e| format!("Invalid agent workspace path: {}", e))?;
    crate::filesystem_policy::PaneFs::validate_name(&filename)
        .map_err(|e| format!("Invalid log filename: {}", e))?;

    let pane_fs = crate::filesystem_policy::PaneFs::from_pane_root(
        crate::database::get_pane_root(),
    ).map_err(|e| format!("Filesystem policy error: {}", e))?;
    let logs_relative = crate::filesystem_policy::PaneFs::relative_from_names([
        workspace_path.as_str(),
        "logs",
    ]).map_err(|e| format!("Invalid logs directory path: {}", e))?;
    pane_fs
        .create_dir_all(crate::filesystem_policy::PaneRoot::System, &logs_relative)
        .map_err(|e| format!("Failed to create logs directory: {}", e))?;
    let log_relative = crate::filesystem_policy::PaneFs::relative_from_names([
        workspace_path.as_str(),
        "logs",
        filename.as_str(),
    ]).map_err(|e| format!("Invalid log file path: {}", e))?;
    let log_path = pane_fs
        .authorize_write(crate::filesystem_policy::PaneRoot::System, &log_relative)
        .map_err(|e| format!("Unsafe log file destination: {}", e))?;
    crate::file_ops::atomic_write(
        log_path.as_path(),
        &content,
        Some(&conn),
        crate::file_ops::WriteOrigin::Local,
    )
}

/// Create tasks directory for future agent task workspaces
/// Created during onboarding to set up the directory structure
#[tauri::command]
pub fn ensure_tasks_directory() -> Result<(), String> {
    let pane_fs = crate::filesystem_policy::PaneFs::from_pane_root(
        crate::database::get_pane_root(),
    ).map_err(|e| format!("Filesystem policy error: {}", e))?;
    pane_fs
        .create_dir_all(
            crate::filesystem_policy::PaneRoot::Internal,
            std::path::Path::new("tasks"),
        )
        .map_err(|e| format!("Failed to create tasks dir: {}", e))?;
    Ok(())
}

/// Fetch available models from OpenClaw gateway
/// Used during onboarding to populate model picker
/// Bypasses ATS by making the HTTP request from Rust instead of WebView
#[tauri::command]
pub async fn fetch_gateway_models(url: String, token: String, db: tauri::State<'_, crate::database::DbState>) -> Result<Vec<String>, String> {
    eprintln!("[GATEWAY_MODELS] Fetching models from: {}", url);
    
    // Use gateway client with cert pinning
    let client = crate::gateway_client::build_gateway_client(&db)?;
    
    let models_url = format!("{}/models", url.trim_end_matches('/'));
    
    let response = client
        .get(&models_url)
        .header("Authorization", format!("Bearer {}", token.trim()))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    
    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!("HTTP {}: {}", status, error_text));
    }
    
    let json: serde_json::Value = response.json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;
    
    // Extract model IDs from response { "data": [{"id": "..."}, ...] }
    let models = json.get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| "Invalid response format: missing 'data' array".to_string())?
        .iter()
        .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
        .collect::<Vec<String>>();
    
    if models.is_empty() {
        return Err("No models found in gateway response".to_string());
    }
    
    eprintln!("[GATEWAY_MODELS] Retrieved {} model(s)", models.len());
    Ok(models)
}

// Test-only override for `validate_openrouter_key`'s hardcoded auth endpoint
// (F2 stored-key-fallback integration tests, #2455 venice live-test v2 delta
// review) — same `#[cfg(test)]` thread_local carve-out shape as
// `web::test_set_brave_api_url`, since `validate_openrouter_key` (unlike
// `validate_openrouter_key_at`) hits a fixed URL with no injectable parameter.
#[cfg(test)]
thread_local! {
    static TEST_OPENROUTER_VALIDATE_URL: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    static TEST_ANTHROPIC_VALIDATE_URL: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn test_set_openrouter_validate_url(url: &str) {
    TEST_OPENROUTER_VALIDATE_URL.with(|c| *c.borrow_mut() = Some(url.to_string()));
}

#[cfg(test)]
pub(crate) fn test_clear_openrouter_validate_url() {
    TEST_OPENROUTER_VALIDATE_URL.with(|c| *c.borrow_mut() = None);
}

fn openrouter_validate_url() -> String {
    #[cfg(test)]
    {
        if let Some(u) = TEST_OPENROUTER_VALIDATE_URL.with(|c| c.borrow().clone()) {
            return u;
        }
    }
    "https://openrouter.ai/api/v1/auth/key".to_string()
}

#[cfg(test)]
pub(crate) fn test_set_anthropic_validate_url(url: &str) {
    TEST_ANTHROPIC_VALIDATE_URL.with(|c| *c.borrow_mut() = Some(url.to_string()));
}

#[cfg(test)]
pub(crate) fn test_clear_anthropic_validate_url() {
    TEST_ANTHROPIC_VALIDATE_URL.with(|c| *c.borrow_mut() = None);
}

fn anthropic_validate_url() -> String {
    #[cfg(test)]
    {
        if let Some(u) = TEST_ANTHROPIC_VALIDATE_URL.with(|c| c.borrow().clone()) {
            return u;
        }
    }
    "https://api.anthropic.com/v1/models?limit=1".to_string()
}

/// Validate OpenRouter API key by calling auth endpoint
/// Returns Ok(()) if valid, Err with message if invalid
///
/// F2 fix (#2455 venice live-test v2): `api_key` is now `Option<String>` —
/// `None` (or `Some("")`) falls back to the stored keychain credential via
/// `read_provider_key`, the same helper the card's masked-credential display
/// already uses. Before this fix, Test always sent whatever was in the
/// (usually blank, on an already-configured card) input field.
#[tauri::command]
pub async fn validate_openrouter_key(api_key: Option<String>, db: tauri::State<'_, crate::database::DbState>) -> Result<(), String> {
    eprintln!("[OPENROUTER_VALIDATE] Validating API key");

    let api_key = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => {
            eprintln!("[OPENROUTER_VALIDATE] No key provided — falling back to stored key");
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let stored = crate::ai::credential_migration::read_provider_key(&conn, "openrouter");
            drop(conn);
            match stored {
                Some(k) => k,
                None => {
                    eprintln!("[OPENROUTER_VALIDATE] Failed: no API key provided and no stored key found");
                    return Err("No API key provided and no stored key found".to_string());
                }
            }
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;
    
    let response = client
        .get(openrouter_validate_url())
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;
    
    let status = response.status();

    // 402 Payment Required — billing issue, NOT a bad key.
    // Distinguish from 401 so the user doesn't go hunting for a new key.
    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        let error_text = response.text().await.unwrap_or_default();
        eprintln!("[OPENROUTER_VALIDATE] 402 Payment Required: {}", error_text);
        return Err("Your OpenRouter account is suspended — no credits available. Add credits at openrouter.ai/settings/credits. If you just added credits, it can take up to 30 seconds to reflect — try again shortly.".to_string());
    }

    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        eprintln!("[OPENROUTER_VALIDATE] Failed: HTTP {} - {}", status, error_text);
        return Err("Invalid API key — check your key at openrouter.ai/keys".to_string());
    }

    // 200 OK — but check limit_remaining. A zero balance means the key is valid
    // but the account has no credits; advancing to the model picker would be
    // misleading. Spec §6.3.1 Step 2: surface no-credits copy with propagation hint.
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;
    if let Some(limit) = body.get("data").and_then(|d| d.get("limit_remaining")) {
        if limit.as_f64().unwrap_or(1.0) <= 0.0 {
            eprintln!("[OPENROUTER_VALIDATE] limit_remaining=0 — no credits");
            return Err("No credits on your OpenRouter account. If you just added credits at openrouter.ai/settings/credits, it can take up to 30 seconds to reflect — try again shortly.".to_string());
        }
    }

    eprintln!("[OPENROUTER_VALIDATE] Key validated successfully");
    Ok(())
}


/// Validate Anthropic API key by calling /v1/models endpoint
/// Returns Ok(()) if valid, Err with message if invalid
///
/// F2 fix (#2455 venice live-test v2): `api_key` is now `Option<String>` —
/// `None` (or `Some("")`) falls back to the stored keychain credential via
/// `read_provider_key`, the same helper the card's masked-credential display
/// already uses. Before this fix, Test always sent whatever was in the
/// (usually blank, on an already-configured card) input field.
#[tauri::command]
pub async fn validate_anthropic_key(api_key: Option<String>, db: tauri::State<'_, crate::database::DbState>) -> Result<(), String> {
    eprintln!("[ANTHROPIC_VALIDATE] Validating API key");

    let api_key = match api_key.filter(|k| !k.trim().is_empty()) {
        Some(k) => k,
        None => {
            eprintln!("[ANTHROPIC_VALIDATE] No key provided — falling back to stored key");
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let stored = crate::ai::credential_migration::read_provider_key(&conn, "anthropic");
            drop(conn);
            match stored {
                Some(k) => k,
                None => {
                    eprintln!("[ANTHROPIC_VALIDATE] Failed: no API key provided and no stored key found");
                    return Err("No API key provided and no stored key found".to_string());
                }
            }
        }
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let response = client
        .get(anthropic_validate_url())
        .header("x-api-key", api_key.trim())
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED {
        eprintln!("[ANTHROPIC_VALIDATE] Failed: 401 Unauthorized");
        return Err("Invalid API key — check your key at console.anthropic.com/settings/keys".to_string());
    }

    // Billing-error quirk per §6.4.3: 400 + invalid_request_error + "credit balance" → treat as billing
    // Delegate to shared classifier — both checks now consistent across inference + validation paths.
    if status == reqwest::StatusCode::BAD_REQUEST {
        let body = response.text().await.unwrap_or_default();
        let body_json: serde_json::Value =
            serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
        let error_type = body_json["error"]["type"].as_str().unwrap_or("");
        let error_msg = body_json["error"]["message"].as_str().unwrap_or("");
        if crate::ai::anthropic::is_anthropic_billing_error(status, error_type, error_msg) {
            return Err("Key valid but insufficient credits. Add credits at console.anthropic.com".to_string());
        }
        eprintln!("[ANTHROPIC_VALIDATE] Failed: 400 Bad Request - {}", body);
        return Err(format!("Validation failed: {}", body));
    }

    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        eprintln!("[ANTHROPIC_VALIDATE] Failed: HTTP {} - {}", status, error_text);
        return Err(format!("Validation failed (HTTP {})", status));
    }

    eprintln!("[ANTHROPIC_VALIDATE] Key validated successfully");
    Ok(())
}

// ---------------------------------------------------------------------------
// Internal helper — exposed only for tests so we can point at a mock server.
// ---------------------------------------------------------------------------
#[doc(hidden)]
// Production uses the fixed endpoint; this injectable endpoint is retained for validation tests.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn validate_openrouter_key_at(api_key: &str, url: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let response = client
        .get(url)
        .header("Authorization", format!("Bearer {}", api_key.trim()))
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    let status = response.status();

    if status == reqwest::StatusCode::PAYMENT_REQUIRED {
        let _ = response.text().await;
        return Err("Your OpenRouter account is suspended — no credits available. Add credits at openrouter.ai/settings/credits. If you just added credits, it can take up to 30 seconds to reflect — try again shortly.".to_string());
    }

    if !status.is_success() {
        let _ = response.text().await;
        return Err("Invalid API key — check your key at openrouter.ai/keys".to_string());
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;
    if let Some(limit) = body.get("data").and_then(|d| d.get("limit_remaining")) {
        if limit.as_f64().unwrap_or(1.0) <= 0.0 {
            return Err("No credits on your OpenRouter account. If you just added credits at openrouter.ai/settings/credits, it can take up to 30 seconds to reflect — try again shortly.".to_string());
        }
    }

    Ok(())
}

#[cfg(test)]
mod openclaw_path_policy_tests {
    use super::{
        authorize_openclaw_config, authorize_renderer_workspace, authorized_fixed_workspace_file,
        expand_openclaw_path,
    };

    #[test]
    fn renderer_workspace_requires_openclaw_root_or_default_config_declaration() {
        let temp = tempfile::TempDir::new().unwrap();
        let home = temp.path();
        let openclaw = home.join(".openclaw");
        let local_workspace = openclaw.join("workspace");
        let declared_workspace = home.join("declared-workspace");
        let unrelated_workspace = home.join("unrelated-workspace");
        std::fs::create_dir_all(&local_workspace).unwrap();
        std::fs::create_dir_all(&declared_workspace).unwrap();
        std::fs::create_dir_all(&unrelated_workspace).unwrap();
        std::fs::write(
            openclaw.join("openclaw.json"),
            serde_json::json!({
                "agents": {
                    "list": [{
                        "id": "declared",
                        "workspace": declared_workspace.to_string_lossy()
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();

        assert_eq!(
            authorize_renderer_workspace("~/.openclaw/workspace", home).unwrap(),
            std::fs::canonicalize(&local_workspace).unwrap()
        );
        assert_eq!(
            authorize_renderer_workspace(&declared_workspace.to_string_lossy(), home).unwrap(),
            std::fs::canonicalize(&declared_workspace).unwrap()
        );
        assert!(authorize_renderer_workspace(
            &unrelated_workspace.to_string_lossy(),
            home,
        )
        .is_err());
        assert!(authorize_renderer_workspace("~/.openclaw/../unrelated-workspace", home).is_err());
    }

    #[test]
    fn custom_config_must_canonicalize_beneath_openclaw_root() {
        let temp = tempfile::TempDir::new().unwrap();
        let home = temp.path();
        let openclaw = home.join(".openclaw");
        let nested = openclaw.join("configs");
        std::fs::create_dir_all(&nested).unwrap();
        let allowed = nested.join("custom.json");
        let rejected = home.join("shared-openclaw.json");
        std::fs::write(&allowed, "{}").unwrap();
        std::fs::write(&rejected, "{}").unwrap();

        assert_eq!(
            authorize_openclaw_config(Some(&allowed.to_string_lossy()), home)
                .unwrap()
                .0,
            std::fs::canonicalize(&allowed).unwrap()
        );
        assert!(authorize_openclaw_config(Some(&rejected.to_string_lossy()), home).is_err());
        assert!(authorize_openclaw_config(Some("~/.openclaw/../shared-openclaw.json"), home).is_err());
    }

    #[test]
    fn fixed_workspace_files_cannot_escape_authorized_workspace() {
        let temp = tempfile::TempDir::new().unwrap();
        // Canonicalize the temp dir before building the workspace path: on
        // macOS, `tempfile::TempDir` is created under `/var/folders/...`,
        // which itself resolves through the `/var` -> `/private/var` symlink.
        // `authorized_fixed_workspace_file` canonicalizes the file it checks,
        // so the workspace path passed in must share that same canonical
        // prefix or the check correctly (and rightly) rejects it as escaping
        // the workspace.
        let temp_root = temp.path().canonicalize().unwrap();
        let workspace = temp_root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("SOUL.md"), "# Soul").unwrap();

        assert_eq!(
            authorized_fixed_workspace_file(&workspace, "SOUL.md")
                .unwrap()
                .unwrap(),
            std::fs::canonicalize(workspace.join("SOUL.md")).unwrap()
        );
        assert!(authorized_fixed_workspace_file(&workspace, "../secret.md").is_err());
        assert!(expand_openclaw_path("~/../secret", temp.path(), temp.path()).is_err());
        assert!(expand_openclaw_path("/tmp/../secret", temp.path(), temp.path()).is_err());
    }
}

#[cfg(test)]
mod configured_service_url_tests {
    use super::configured_service_url;

    #[test]
    fn gateway_chat_path_is_backend_owned() {
        let url = configured_service_url("https://gateway.example:8443/v1", "/v1/chat/completions")
            .expect("configured Gateway URL should be valid");
        assert_eq!(url.as_str(), "https://gateway.example:8443/v1/chat/completions");
    }

    #[test]
    fn preserves_configured_base_path_but_replaces_v1_suffix() {
        let url = configured_service_url("https://gateway.example/pane/v1/", "/v1/chat/completions")
            .expect("configured Gateway URL should be valid");
        assert_eq!(url.as_str(), "https://gateway.example/pane/v1/chat/completions");
    }

    #[test]
    fn rejects_non_http_schemes_credentials_queries_and_fragments() {
        for endpoint in [
            "file:///etc/passwd",
            "https://user:password@gateway.example",
            "https://gateway.example?redirect=https://evil.example",
            "https://gateway.example/#evil",
        ] {
            assert!(configured_service_url(endpoint, "/v1/chat/completions").is_err());
        }
    }
}

#[cfg(test)]
mod validate_openrouter_key_tests {
    use super::validate_openrouter_key_at;

    /// 401 → "Invalid API key" copy (existing behavior preserved)
    #[tokio::test]
    async fn test_401_returns_invalid_api_key() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/v1/auth/key")
            .with_status(401)
            .with_body(r#"{"error":"Unauthorized"}"#)
            .create_async()
            .await;

        let url = format!("{}/api/v1/auth/key", server.url());
        let result = validate_openrouter_key_at("sk-or-v1-bad", &url).await;

        assert!(result.is_err(), "401 should return Err");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Invalid API key"),
            "401 error should say 'Invalid API key', got: {msg}"
        );
        assert!(
            !msg.contains("credits"),
            "401 error must NOT mention credits, got: {msg}"
        );
    }

    /// 402 → billing copy with propagation hint (Fix D)
    #[tokio::test]
    async fn test_402_returns_billing_copy() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/v1/auth/key")
            .with_status(402)
            .with_body(r#"{"error":"Payment Required"}"#)
            .create_async()
            .await;

        let url = format!("{}/api/v1/auth/key", server.url());
        let result = validate_openrouter_key_at("sk-or-v1-real-but-no-credits", &url).await;

        assert!(result.is_err(), "402 should return Err");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("credits"),
            "402 error should mention credits, got: {msg}"
        );
        assert!(
            msg.contains("openrouter.ai/settings/credits"),
            "402 error should include credits URL, got: {msg}"
        );
        assert!(
            msg.contains("30 seconds"),
            "402 error should include propagation-delay hint, got: {msg}"
        );
        assert!(
            !msg.contains("Invalid API key"),
            "402 error must NOT say 'Invalid API key', got: {msg}"
        );
    }

    /// 200 with limit_remaining = 0 → no-credits error (Fix C)
    #[tokio::test]
    async fn test_200_limit_remaining_zero_returns_billing_copy() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/v1/auth/key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"label":"My key","usage":0,"limit":null,"is_free_tier":false,"rate_limit":{"requests":200,"interval":"10s"},"limit_remaining":0}}"#)
            .create_async()
            .await;

        let url = format!("{}/api/v1/auth/key", server.url());
        let result = validate_openrouter_key_at("sk-or-v1-valid-but-no-credits", &url).await;

        assert!(result.is_err(), "200 with limit_remaining=0 should return Err");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("No credits on your OpenRouter account"),
            "error should say no credits on account, got: {msg}"
        );
        assert!(
            msg.contains("openrouter.ai/settings/credits"),
            "error should include credits URL, got: {msg}"
        );
        assert!(
            msg.contains("30 seconds"),
            "error should include propagation-delay hint, got: {msg}"
        );
    }

    /// 200 with limit_remaining = 0.0 (float) → same billing copy
    #[tokio::test]
    async fn test_200_limit_remaining_zero_float_returns_billing_copy() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/v1/auth/key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"limit_remaining":0.0}}"#)
            .create_async()
            .await;

        let url = format!("{}/api/v1/auth/key", server.url());
        let result = validate_openrouter_key_at("sk-or-v1-valid", &url).await;

        assert!(result.is_err(), "200 with limit_remaining=0.0 should return Err");
        assert!(result.unwrap_err().contains("No credits on your OpenRouter account"));
    }

    /// 200 with limit_remaining > 0 → success (Fix C — positive case)
    #[tokio::test]
    async fn test_200_limit_remaining_positive_returns_ok() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/v1/auth/key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"label":"My key","usage":0,"limit":null,"is_free_tier":false,"rate_limit":{"requests":200,"interval":"10s"},"limit_remaining":5.5}}"#)
            .create_async()
            .await;

        let url = format!("{}/api/v1/auth/key", server.url());
        let result = validate_openrouter_key_at("sk-or-v1-good-key", &url).await;

        assert!(result.is_ok(), "200 with limit_remaining>0 should return Ok, got: {:?}", result);
    }

    /// 200 with no limit_remaining field (free tier / unlimited) → success
    #[tokio::test]
    async fn test_200_no_limit_remaining_field_returns_ok() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/v1/auth/key")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"data":{"label":"My key","usage":0,"is_free_tier":true}}"#)
            .create_async()
            .await;

        let url = format!("{}/api/v1/auth/key", server.url());
        let result = validate_openrouter_key_at("sk-or-v1-free", &url).await;

        assert!(result.is_ok(), "200 with no limit_remaining should return Ok (unlimited/free), got: {:?}", result);
    }
}

/// F2 fix (#2455 venice live-test v2 delta review): real integration tests for
/// `validate_openrouter_key`'s `Option<String>` stored-key fallback, calling the
/// actual `#[tauri::command]` handler through a real `tauri::test::mock_app` +
/// fresh-migrated in-memory DB, exactly the shape `f3_venice_save_promote_global_demote_tests`
/// / `f5_sync_initial_import_role_self_heal_tests` already use. Flagged by Reed's
/// 2026-08-30 delta review as a documentation-accuracy gap: the PR body/CHANGELOG
/// claimed this coverage existed when it did not. These three tests close that gap
/// for the OpenRouter validator; `validate_anthropic_key_stored_key_fallback_tests`
/// (below) and `commands.rs`'s `validate_venice_key_stored_key_fallback_tests` cover
/// the other two.
#[cfg(test)]
mod validate_openrouter_key_stored_key_fallback_tests {
    use super::{test_clear_openrouter_validate_url, test_set_openrouter_validate_url, validate_openrouter_key};
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;

    fn setup() -> tauri::App<tauri::test::MockRuntime> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    /// Stored key present, no input (`None`): the validator must attempt
    /// validation with the stored key (reaching the mocked HTTP call, not
    /// short-circuiting with "No API key provided").
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_none_input_falls_back_to_stored_key_and_reaches_http_call() {
        let _kc = KeychainGuard::set();
        let app = setup();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-v1-stored-key-1234")
                .expect("failed to seed stored openrouter key");
        }

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/v1/auth/key")
            .match_header("authorization", "Bearer sk-or-v1-stored-key-1234")
            .with_status(401)
            .with_body(r#"{"error":"Unauthorized"}"#)
            .create_async()
            .await;
        test_set_openrouter_validate_url(&format!("{}/api/v1/auth/key", server.url()));

        let db_state = app.state::<DbState>();
        let result = validate_openrouter_key(None, db_state).await;

        test_clear_openrouter_validate_url();

        mock.assert_async().await;
        assert!(result.is_err(), "mocked 401 should surface as Err");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Invalid API key"),
            "expected the mocked HTTP 401 to be reached and translated, not a short-circuit error, got: {msg}"
        );
        assert!(
            !msg.contains("No API key provided"),
            "None input with a stored key present must NOT short-circuit with 'No API key provided', got: {msg}"
        );
    }

    /// No stored key AND no input: must return the clean fallback-miss error,
    /// not panic and not attempt an HTTP call at all.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_none_input_with_no_stored_key_returns_clean_error() {
        let _kc = KeychainGuard::set();
        let app = setup();

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/v1/auth/key")
            .with_status(200)
            .expect(0)
            .create_async()
            .await;
        test_set_openrouter_validate_url(&format!("{}/api/v1/auth/key", server.url()));

        let db_state = app.state::<DbState>();
        let result = validate_openrouter_key(None, db_state).await;

        test_clear_openrouter_validate_url();

        mock.assert_async().await;
        assert!(result.is_err(), "no input and no stored key must be an Err");
        assert_eq!(
            result.unwrap_err(),
            "No API key provided and no stored key found",
            "must be the clean fallback-miss message, not a panic or an HTTP-layer error"
        );
    }

    /// Typed input wins over stored: when both a stored key and a typed
    /// `Some(...)` key are present, the typed key must be the one sent —
    /// never the stored one.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_typed_input_wins_over_stored_key() {
        let _kc = KeychainGuard::set();
        let app = setup();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            crate::ai::credential_migration::write_provider_key(&conn, "openrouter", "sk-or-v1-stored-key-1234")
                .expect("failed to seed stored openrouter key");
        }

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/v1/auth/key")
            .match_header("authorization", "Bearer sk-or-v1-user-typed-key")
            .with_status(401)
            .with_body(r#"{"error":"Unauthorized"}"#)
            .create_async()
            .await;
        test_set_openrouter_validate_url(&format!("{}/api/v1/auth/key", server.url()));

        let db_state = app.state::<DbState>();
        let result = validate_openrouter_key(Some("sk-or-v1-user-typed-key".to_string()), db_state).await;

        test_clear_openrouter_validate_url();

        // The mockito `match_header` expectation above is the real assertion —
        // if the stored key had been sent instead, the mock would never match
        // and `.assert_async()` would fail here.
        mock.assert_async().await;
        assert!(result.is_err(), "mocked 401 for the typed key should surface as Err");
        assert!(
            result.unwrap_err().contains("Invalid API key"),
            "expected the typed key's mocked HTTP 401 to be reached"
        );
    }
}

/// F2 fix (#2455 venice live-test v2 delta review): real integration tests for
/// `validate_anthropic_key`'s `Option<String>` stored-key fallback — same shape
/// as `validate_openrouter_key_stored_key_fallback_tests` above.
#[cfg(test)]
mod validate_anthropic_key_stored_key_fallback_tests {
    use super::{test_clear_anthropic_validate_url, test_set_anthropic_validate_url, validate_anthropic_key};
    use crate::database::DbState;
    use crate::keychain::test_support::KeychainGuard;
    use serial_test::serial;
    use tauri::Manager;

    fn setup() -> tauri::App<tauri::test::MockRuntime> {
        let conn = rusqlite::Connection::open_in_memory().expect("in-memory db");
        crate::migrations::run_migrations(&conn).expect("fresh-install migrations should succeed");
        let app = tauri::test::mock_app();
        app.manage(DbState::new(conn));
        app
    }

    /// Stored key present, no input (`None`): must reach the mocked HTTP call
    /// with the stored key, not short-circuit with "No API key provided".
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_none_input_falls_back_to_stored_key_and_reaches_http_call() {
        let _kc = KeychainGuard::set();
        let app = setup();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-stored-key-1234")
                .expect("failed to seed stored anthropic key");
        }

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/models?limit=1")
            .match_header("x-api-key", "sk-ant-stored-key-1234")
            .with_status(401)
            .with_body(r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#)
            .create_async()
            .await;
        test_set_anthropic_validate_url(&format!("{}/v1/models?limit=1", server.url()));

        let db_state = app.state::<DbState>();
        let result = validate_anthropic_key(None, db_state).await;

        test_clear_anthropic_validate_url();

        mock.assert_async().await;
        assert!(result.is_err(), "mocked 401 should surface as Err");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("Invalid API key"),
            "expected the mocked HTTP 401 to be reached and translated, got: {msg}"
        );
        assert!(
            !msg.contains("No API key provided"),
            "None input with a stored key present must NOT short-circuit with 'No API key provided', got: {msg}"
        );
    }

    /// No stored key AND no input: clean fallback-miss error, no HTTP call attempted.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_none_input_with_no_stored_key_returns_clean_error() {
        let _kc = KeychainGuard::set();
        let app = setup();

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/models?limit=1")
            .with_status(200)
            .expect(0)
            .create_async()
            .await;
        test_set_anthropic_validate_url(&format!("{}/v1/models?limit=1", server.url()));

        let db_state = app.state::<DbState>();
        let result = validate_anthropic_key(None, db_state).await;

        test_clear_anthropic_validate_url();

        mock.assert_async().await;
        assert!(result.is_err(), "no input and no stored key must be an Err");
        assert_eq!(
            result.unwrap_err(),
            "No API key provided and no stored key found",
            "must be the clean fallback-miss message, not a panic or an HTTP-layer error"
        );
    }

    /// Typed input wins over stored: the typed key must be the one sent to the
    /// HTTP call, never the stored one.
    #[tokio::test]
    #[serial(keychain_override)]
    async fn test_typed_input_wins_over_stored_key() {
        let _kc = KeychainGuard::set();
        let app = setup();
        {
            let conn_state = app.state::<DbState>();
            let conn = conn_state.0.lock().unwrap();
            crate::ai::credential_migration::write_provider_key(&conn, "anthropic", "sk-ant-stored-key-1234")
                .expect("failed to seed stored anthropic key");
        }

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/v1/models?limit=1")
            .match_header("x-api-key", "sk-ant-user-typed-key")
            .with_status(401)
            .with_body(r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#)
            .create_async()
            .await;
        test_set_anthropic_validate_url(&format!("{}/v1/models?limit=1", server.url()));

        let db_state = app.state::<DbState>();
        let result = validate_anthropic_key(Some("sk-ant-user-typed-key".to_string()), db_state).await;

        test_clear_anthropic_validate_url();

        mock.assert_async().await;
        assert!(result.is_err(), "mocked 401 for the typed key should surface as Err");
        assert!(
            result.unwrap_err().contains("Invalid API key"),
            "expected the typed key's mocked HTTP 401 to be reached"
        );
    }
}
