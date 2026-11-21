// Conflict resolution module tests
// Tests all resolution branches for LWW conflict handling

use crate::conflict_resolution::{resolve, ConflictContext, Resolution};
use chrono::{Duration, Utc};

#[test]
fn test_case_1_remote_missing_local_exists() {
    // Case 1: Remote missing, local exists → KeepLocal
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: Some(Utc::now()),
        remote_timestamp: None,
        local_content: Some("# Local content".to_string()),
        remote_content: None,
    };
    assert_eq!(resolve(&ctx), Resolution::KeepLocal);
}

#[test]
fn test_case_2_local_missing_remote_exists() {
    // Case 2: Local missing, remote exists → AcceptRemote
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: None,
        remote_timestamp: Some(Utc::now()),
        local_content: None,
        remote_content: Some("# Remote content".to_string()),
    };
    assert_eq!(resolve(&ctx), Resolution::AcceptRemote);
}

#[test]
fn test_case_3_both_missing() {
    // Case 3: Both missing → Skip
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: None,
        remote_timestamp: None,
        local_content: None,
        remote_content: None,
    };
    assert_eq!(resolve(&ctx), Resolution::Skip);
}

#[test]
fn test_case_4_both_exist_content_identical() {
    // Case 4: Both exist, content identical → Skip
    let content = "# Identical content\n\nSame everywhere.".to_string();
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: Some(Utc::now() - Duration::hours(1)),
        remote_timestamp: Some(Utc::now()),
        local_content: Some(content.clone()),
        remote_content: Some(content),
    };
    assert_eq!(resolve(&ctx), Resolution::Skip);
}

#[test]
fn test_case_5a_both_exist_remote_newer() {
    // Case 5a: Both exist, remote newer → AcceptRemote
    let now = Utc::now();
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: Some(now - Duration::hours(2)),
        remote_timestamp: Some(now),
        local_content: Some("# Old local content".to_string()),
        remote_content: Some("# New remote content".to_string()),
    };
    assert_eq!(resolve(&ctx), Resolution::AcceptRemote);
}

#[test]
fn test_case_5b_both_exist_local_newer() {
    // Case 5b: Both exist, local newer → KeepLocal
    let now = Utc::now();
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: Some(now),
        remote_timestamp: Some(now - Duration::hours(2)),
        local_content: Some("# New local content".to_string()),
        remote_content: Some("# Old remote content".to_string()),
    };
    assert_eq!(resolve(&ctx), Resolution::KeepLocal);
}

#[test]
fn test_case_5c_both_exist_timestamps_equal() {
    // Case 5c: Both exist, timestamps equal → KeepLocal (Pane wins per ADR-0042)
    let now = Utc::now();
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: Some(now),
        remote_timestamp: Some(now),
        local_content: Some("# Local version".to_string()),
        remote_content: Some("# Remote version".to_string()),
    };
    assert_eq!(resolve(&ctx), Resolution::KeepLocal);
}

#[test]
fn test_case_6_local_exists_no_local_timestamp() {
    // Case 6: Local exists, no local timestamp, remote has timestamp → AcceptRemote
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: None,
        remote_timestamp: Some(Utc::now()),
        local_content: Some("# Local content".to_string()),
        remote_content: Some("# Remote content".to_string()),
    };
    assert_eq!(resolve(&ctx), Resolution::AcceptRemote);
}

#[test]
fn test_case_7_both_exist_no_remote_timestamp() {
    // Case 7: Both exist, no remote timestamp, content differs → AcceptRemote
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: Some(Utc::now()),
        remote_timestamp: None,
        local_content: Some("# Local content".to_string()),
        remote_content: Some("# Remote content".to_string()),
    };
    assert_eq!(resolve(&ctx), Resolution::AcceptRemote);
}

#[test]
fn test_case_8_no_timestamps_content_differs() {
    // Case 8: No timestamps at all, content differs → AcceptRemote
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: None,
        remote_timestamp: None,
        local_content: Some("# Local content".to_string()),
        remote_content: Some("# Remote content".to_string()),
    };
    assert_eq!(resolve(&ctx), Resolution::AcceptRemote);
}

#[test]
fn test_placeholder_file_kept_when_remote_missing() {
    // Placeholder file created by verify_agents_integrity + no remote file
    // → Must resolve to KeepLocal, NOT delete the placeholder
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "SOUL.md".to_string(),
        local_modified: Some(Utc::now()),
        remote_timestamp: None,
        remote_content: None,
        local_content: Some("# SOUL.md\n\n*(auto-created by integrity check)*".to_string()),
    };
    assert_eq!(resolve(&ctx), Resolution::KeepLocal);
}

#[test]
fn test_remote_exists_no_timestamp_no_local() {
    // Edge case: Remote exists with no timestamp, local doesn't exist
    // Should still resolve to AcceptRemote (ingest the new file)
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "new-file.md".to_string(),
        local_modified: None,
        remote_timestamp: None,
        local_content: None,
        remote_content: Some("# New file from remote".to_string()),
    };
    assert_eq!(resolve(&ctx), Resolution::AcceptRemote);
}

#[test]
fn test_identical_content_different_timestamps() {
    // Content is identical, but timestamps differ
    // Should resolve to Skip (content optimization)
    let content = "# Unchanged content".to_string();
    let now = Utc::now();
    let ctx = ConflictContext {
        agent_id: "test-agent".to_string(),
        filename: "unchanged.md".to_string(),
        local_modified: Some(now - Duration::hours(5)),
        remote_timestamp: Some(now),
        local_content: Some(content.clone()),
        remote_content: Some(content),
    };
    assert_eq!(resolve(&ctx), Resolution::Skip);
}
