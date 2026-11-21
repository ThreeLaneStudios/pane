// Topology detection tests

use rusqlite::Connection;
use crate::sync_client::{SyncTopology, detect_topology, get_gateway_url, get_gateway_token, is_remote_url};

fn create_test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ai_settings (
            key TEXT PRIMARY KEY,
            value TEXT
        );
        CREATE TABLE IF NOT EXISTS ai_agents (
            id TEXT PRIMARY KEY,
            runtime_type TEXT,
            is_active INTEGER NOT NULL DEFAULT 1
        );"
    ).unwrap();
    conn
}

#[test]
fn test_detect_topology_defaults_to_split_machine() {
    let conn = create_test_db();

    // Empty DB should default to SplitMachine (SameMachine removed per #1108)
    let topology = detect_topology(&conn);
    assert_eq!(topology, SyncTopology::SplitMachine);
}

#[test]
fn test_detect_topology_split_machine() {
    let conn = create_test_db();

    // Set topology to split_machine
    conn.execute(
        "INSERT INTO ai_settings (key, value) VALUES ('tier3_topology', 'split_machine')",
        [],
    ).unwrap();

    let topology = detect_topology(&conn);
    assert_eq!(topology, SyncTopology::SplitMachine);
}

#[test]
fn test_detect_topology_infers_split_from_remote_endpoint() {
    let conn = create_test_db();

    // Set endpoint to remote IP
    conn.execute(
        "INSERT INTO ai_settings (key, value) VALUES ('endpoint', 'https://100.64.0.1:8443/v1')",
        [],
    ).unwrap();

    let topology = detect_topology(&conn);
    assert_eq!(topology, SyncTopology::SplitMachine, "Should infer SplitMachine from remote endpoint");
}

#[test]
fn test_detect_topology_infers_split_from_localhost_endpoint() {
    let conn = create_test_db();

    // Set endpoint to localhost — SameMachine removed, now always SplitMachine
    conn.execute(
        "INSERT INTO ai_settings (key, value) VALUES ('endpoint', 'http://localhost:18789/v1')",
        [],
    ).unwrap();

    let topology = detect_topology(&conn);
    assert_eq!(topology, SyncTopology::SplitMachine, "Should return SplitMachine from localhost endpoint (SameMachine removed #1108)");
}

#[test]
fn test_detect_topology_infers_split_from_127_endpoint() {
    let conn = create_test_db();

    // Set endpoint to 127.0.0.1 — SameMachine removed, now always SplitMachine
    conn.execute(
        "INSERT INTO ai_settings (key, value) VALUES ('endpoint', 'http://127.0.0.1:18789/v1')",
        [],
    ).unwrap();

    let topology = detect_topology(&conn);
    assert_eq!(topology, SyncTopology::SplitMachine, "Should return SplitMachine from 127.0.0.1 endpoint (SameMachine removed #1108)");
}

#[test]
fn test_detect_topology_explicit_override_wins() {
    let conn = create_test_db();

    // Set both explicit topology AND endpoint to localhost
    conn.execute(
        "INSERT INTO ai_settings (key, value) VALUES ('tier3_topology', 'split_machine')",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO ai_settings (key, value) VALUES ('endpoint', 'http://localhost:18789/v1')",
        [],
    ).unwrap();

    let topology = detect_topology(&conn);
    assert_eq!(topology, SyncTopology::SplitMachine, "Explicit override should win over inferred topology");
}

#[test]
fn test_detect_topology_no_endpoint_defaults_split() {
    let conn = create_test_db();

    // No settings at all — should default to SplitMachine (SameMachine removed #1108)
    let topology = detect_topology(&conn);
    assert_eq!(topology, SyncTopology::SplitMachine, "Should default to SplitMachine when no endpoint exists");
}

#[test]
fn test_detect_topology_maps_stored_same_machine_to_split_machine() {
    let conn = create_test_db();

    // Simulate a user DB with legacy 'same_machine' value written by a prior build.
    // #2649: the `SameMachine` enum variant is gone entirely (OpenClaw is
    // split-machine only). This test proves the legacy DB string value is still
    // mapped to SplitMachine for backward compatibility rather than being unmapped.
    conn.execute(
        "INSERT INTO ai_settings (key, value) VALUES ('tier3_topology', 'same_machine')",
        [],
    ).unwrap();

    // detect_topology() must return SplitMachine (backward-compat mapping per #1108, #2649)
    let topology = detect_topology(&conn);
    assert_eq!(
        topology,
        SyncTopology::SplitMachine,
        "Legacy 'same_machine' DB value must map to SplitMachine (SameMachine variant removed #2649)"
    );
}

#[test]
fn test_is_remote_url() {
    // Remote URLs
    assert!(is_remote_url("https://100.64.0.1:8443/v1"), "IP address should be remote");
    assert!(is_remote_url("https://example.com/v1"), "Domain should be remote");
    assert!(is_remote_url("http://192.168.1.100:8080"), "Private IP should be remote");

    // Local URLs
    assert!(!is_remote_url("http://localhost:18789/v1"), "localhost should be local");
    assert!(!is_remote_url("http://127.0.0.1:18789/v1"), "127.0.0.1 should be local");
    assert!(!is_remote_url("http://[::1]:18789/v1"), "::1 (IPv6 localhost) should be local");

    // Invalid URLs
    assert!(!is_remote_url("not-a-url"), "Invalid URL should default to local");
    assert!(!is_remote_url(""), "Empty string should default to local");
}

#[test]
fn test_gateway_url_helpers() {
    let conn = create_test_db();

    // Initially should be None
    assert_eq!(get_gateway_url(&conn), None);
    assert_eq!(get_gateway_token(&conn), None);

    // Set canonical keys.
    // #2564 B1 fix (Path B): get_gateway_url now requires an active
    // ai_agents.runtime_type='openclaw' row instead of ai_settings.provider='openclaw'
    // (the retired scalar) — seed that row instead of the old provider key.
    conn.execute(
        "INSERT INTO ai_agents (id, runtime_type, is_active) VALUES ('agent-oc', 'openclaw', 1)",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO ai_settings (key, value) VALUES ('endpoint', 'https://192.168.1.100:8443')",
        [],
    ).unwrap();
    conn.execute(
        "INSERT INTO ai_settings (key, value) VALUES ('api_key', 'test-token-123')",
        [],
    ).unwrap();

    // Should now return values
    assert_eq!(get_gateway_url(&conn), Some("https://192.168.1.100:8443".to_string()));
    assert_eq!(get_gateway_token(&conn), Some("test-token-123".to_string()));
}
