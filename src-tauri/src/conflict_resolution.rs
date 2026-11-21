use chrono::{DateTime, Utc};

/// Resolution decision for a file conflict
#[derive(Debug, Clone, PartialEq)]
pub enum Resolution {
    /// Accept the remote version (overwrite local)
    AcceptRemote,
    /// Keep the local version (do not overwrite)
    KeepLocal,
    /// Skip — content is identical, no write needed
    Skip,
}

/// Input context for a conflict resolution decision
pub struct ConflictContext {
    /// Agent identifier (for logging)
    pub agent_id: String,
    /// Filename being resolved
    pub filename: String,
    /// Local file's last-modified timestamp (None if file doesn't exist locally)
    pub local_modified: Option<DateTime<Utc>>,
    /// Remote file's queued/modified timestamp (None if file doesn't exist remotely)
    pub remote_timestamp: Option<DateTime<Utc>>,
    /// Local file content (None if file doesn't exist locally)
    pub local_content: Option<String>,
    /// Remote file content (None if file doesn't exist remotely)
    pub remote_content: Option<String>,
}

/// Resolve a file conflict using LWW (Last Writer Wins) with content-identity optimization
pub fn resolve(ctx: &ConflictContext) -> Resolution {
    // Case 1: Remote missing, local exists → KeepLocal
    // (Local file exists but remote has no corresponding file — do NOT delete local)
    if ctx.remote_content.is_none() && ctx.local_content.is_some() {
        eprintln!("[CONFLICT] {}/{}: local exists, remote missing → KeepLocal",
            ctx.agent_id, ctx.filename);
        return Resolution::KeepLocal;
    }

    // Case 2: Local missing, remote exists → AcceptRemote
    if ctx.local_content.is_none() && ctx.remote_content.is_some() {
        eprintln!("[CONFLICT] {}/{}: local missing, remote exists → AcceptRemote",
            ctx.agent_id, ctx.filename);
        return Resolution::AcceptRemote;
    }

    // Case 3: Both missing → Skip
    if ctx.local_content.is_none() && ctx.remote_content.is_none() {
        return Resolution::Skip;
    }

    // Case 4: Both exist — check content identity first
    if let (Some(local), Some(remote)) = (&ctx.local_content, &ctx.remote_content) {
        if local == remote {
            eprintln!("[CONFLICT] {}/{}: content identical → Skip",
                ctx.agent_id, ctx.filename);
            return Resolution::Skip;
        }
    }

    // Case 5: Both exist with different content — use LWW timestamps
    match (ctx.local_modified, ctx.remote_timestamp) {
        (Some(local_time), Some(remote_time)) => {
            if remote_time > local_time {
                eprintln!("[CONFLICT] {}/{}: remote newer ({} > {}) → AcceptRemote",
                    ctx.agent_id, ctx.filename, remote_time, local_time);
                Resolution::AcceptRemote
            } else if remote_time == local_time {
                // Tie-breaking: Pane wins per ADR-0042 (Pane-PRIMARY / OC-REPLICA model, §22C.8.2)
                eprintln!("[CONFLICT] {}/{}: timestamps equal, Pane wins → KeepLocal",
                    ctx.agent_id, ctx.filename);
                Resolution::KeepLocal
            } else {
                eprintln!("[CONFLICT] {}/{}: local newer ({} > {}) → KeepLocal",
                    ctx.agent_id, ctx.filename, local_time, remote_time);
                Resolution::KeepLocal
            }
        }
        (None, Some(_)) => {
            // No local timestamp (file exists but can't get mtime) — accept remote
            Resolution::AcceptRemote
        }
        (Some(_), None) => {
            // Can't parse remote timestamp — fall back to content diff
            // Content already checked above, so content must differ → AcceptRemote as safe default
            // (This matches current behavior: when remote timestamp unparseable, use content diff)
            eprintln!("[CONFLICT] {}/{}: can't parse remote timestamp, content differs → AcceptRemote",
                ctx.agent_id, ctx.filename);
            Resolution::AcceptRemote
        }
        (None, None) => {
            // Neither timestamp available, content differs → AcceptRemote as safe default
            Resolution::AcceptRemote
        }
    }
}
