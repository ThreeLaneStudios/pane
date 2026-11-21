// Pane v11.29.4 — Daily Logs Commands
// Lists and reads daily log files from agent folders

use crate::database::get_pane_root;
use crate::filesystem_policy::{ExistingKind, PaneFs, PaneRoot};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

fn policy_error(context: &str, error: impl std::fmt::Display) -> String {
    format!("{}: {}", context, error)
}

#[tauri::command]
pub fn list_daily_logs() -> Result<Vec<Value>, String> {
    let pane_root = get_pane_root();
    let pane_fs = PaneFs::from_pane_root(&pane_root)
        .map_err(|e| policy_error("Filesystem policy error", e))?;
    let system_path = pane_root.join(".pane").join("system");

    let system_metadata = match fs::symlink_metadata(&system_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(format!("Failed to inspect system directory: {}", error)),
    };
    if system_metadata.file_type().is_symlink() {
        return Err("Symlink system directory rejected".to_string());
    }
    if !system_metadata.is_dir() {
        return Err("System path is not a directory".to_string());
    }

    let mut logs = Vec::new();

    // Scan all agent folders in system
    let agent_dirs = fs::read_dir(&system_path)
        .map_err(|e| format!("Failed to read system directory: {}", e))?;

    for agent_dir_entry in agent_dirs {
        let agent_dir_entry =
            agent_dir_entry.map_err(|e| format!("Failed to read agent directory entry: {}", e))?;
        let agent_type = agent_dir_entry
            .file_type()
            .map_err(|e| format!("Failed to inspect agent directory entry: {}", e))?;
        if agent_type.is_symlink() {
            return Err("Symlink agent directory rejected".to_string());
        }

        let agent_name = agent_dir_entry
            .file_name()
            .to_str()
            .ok_or_else(|| "Agent directory name is not valid UTF-8".to_string())?
            .to_string();
        PaneFs::validate_name(&agent_name)
            .map_err(|e| policy_error("Invalid agent directory name", e))?;

        if !agent_type.is_dir() {
            continue;
        }

        let agent_relative = PaneFs::relative_from_names([agent_name.as_str()])
            .map_err(|e| policy_error("Invalid agent directory path", e))?;
        let agent_dir = pane_fs
            .authorize_existing(PaneRoot::System, &agent_relative, ExistingKind::Directory)
            .map_err(|e| policy_error("Unsafe agent directory", e))?;

        let logs_path = agent_dir.as_path().join("logs");
        let logs_metadata = match fs::symlink_metadata(&logs_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "Failed to inspect logs directory for {}: {}",
                    agent_name, error
                ));
            }
        };
        if logs_metadata.file_type().is_symlink() {
            return Err(format!(
                "Symlink logs directory rejected for {}",
                agent_name
            ));
        }

        let logs_relative = PaneFs::relative_from_names([agent_name.as_str(), "logs"])
            .map_err(|e| policy_error("Invalid logs directory path", e))?;
        let logs_dir = pane_fs
            .authorize_existing(PaneRoot::System, &logs_relative, ExistingKind::Directory)
            .map_err(|e| policy_error("Unsafe logs directory", e))?;

        // Read all .md files in the logs folder
        let log_files = fs::read_dir(logs_dir.as_path())
            .map_err(|e| format!("Failed to read logs directory for {}: {}", agent_name, e))?;

        for log_file_entry in log_files {
            let log_file_entry =
                log_file_entry.map_err(|e| format!("Failed to read log file entry: {}", e))?;
            let log_file_type = log_file_entry
                .file_type()
                .map_err(|e| format!("Failed to inspect log file entry: {}", e))?;
            if log_file_type.is_symlink() {
                return Err(format!("Symlink log file rejected for {}", agent_name));
            }

            let filename = log_file_entry
                .file_name()
                .to_str()
                .ok_or_else(|| "Log filename is not valid UTF-8".to_string())?
                .to_string();
            PaneFs::validate_name(&filename)
                .map_err(|e| policy_error("Invalid log filename", e))?;

            if !log_file_type.is_file()
                || Path::new(&filename).extension().and_then(|e| e.to_str()) != Some("md")
            {
                continue;
            }

            let file_relative =
                PaneFs::relative_from_names([agent_name.as_str(), "logs", filename.as_str()])
                    .map_err(|e| policy_error("Invalid log file path", e))?;
            let log_file = pane_fs
                .authorize_existing(PaneRoot::System, &file_relative, ExistingKind::File)
                .map_err(|e| policy_error("Unsafe log file", e))?;
            let size = fs::metadata(log_file.as_path())
                .map_err(|e| format!("Failed to inspect log file: {}", e))?
                .len();
            let date = Path::new(&filename)
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let relative_path = Path::new(".pane")
                .join("system")
                .join(&file_relative)
                .to_str()
                .ok_or_else(|| "Log path is not valid UTF-8".to_string())?
                .to_string();

            logs.push(json!({
                "agent": agent_name,
                "date": date,
                "file_path": relative_path,
                "size": size
            }));
        }
    }

    // Sort by date DESC (newest first)
    logs.sort_by(|a, b| {
        let date_a = a["date"].as_str().unwrap_or("");
        let date_b = b["date"].as_str().unwrap_or("");
        date_b.cmp(date_a)
    });

    Ok(logs)
}

fn parse_daily_log_path(file_path: &str) -> Result<(&str, &str), String> {
    let mut components = file_path.split('/');
    let (workspace, filename) = match (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) {
        (Some(".pane"), Some("system"), Some(workspace), Some("logs"), Some(filename), None) => {
            (workspace, filename)
        }
        _ => return Err("Invalid log file path".to_string()),
    };

    PaneFs::validate_name(workspace)
        .map_err(|e| policy_error("Invalid agent workspace name", e))?;
    PaneFs::validate_name(filename).map_err(|e| policy_error("Invalid log filename", e))?;
    if !filename.ends_with(".md") {
        return Err("Invalid log file path".to_string());
    }

    Ok((workspace, filename))
}

#[tauri::command]
pub fn read_daily_log(file_path: String) -> Result<String, String> {
    let (workspace, filename) = parse_daily_log_path(&file_path)?;
    let pane_root = get_pane_root();
    let pane_fs = PaneFs::from_pane_root(pane_root)
        .map_err(|e| policy_error("Filesystem policy error", e))?;
    let file_relative = PaneFs::relative_from_names([workspace, "logs", filename])
        .map_err(|e| policy_error("Invalid log file path", e))?;
    let log_file = pane_fs
        .authorize_existing(PaneRoot::System, &file_relative, ExistingKind::File)
        .map_err(|e| policy_error("Unsafe log file", e))?;

    fs::read_to_string(log_file.as_path()).map_err(|e| format!("Failed to read log file: {}", e))
}

#[cfg(test)]
mod tests {
    use super::parse_daily_log_path;

    #[test]
    fn parses_exact_daily_log_path() {
        assert_eq!(
            parse_daily_log_path(".pane/system/agent/logs/2026-07-24.md"),
            Ok(("agent", "2026-07-24.md"))
        );
    }

    #[test]
    fn rejects_non_exact_or_unsafe_daily_log_paths() {
        for path in [
            "/.pane/system/agent/logs/2026-07-24.md",
            ".pane//system/agent/logs/2026-07-24.md",
            ".pane/system/agent/logs/2026-07-24.md/",
            ".pane/system/../logs/2026-07-24.md",
            ".pane/system/agent/logs/../2026-07-24.md",
            ".pane/system/agent/logs/2026-07-24.txt",
        ] {
            assert!(parse_daily_log_path(path).is_err(), "accepted {path:?}");
        }
    }
}
