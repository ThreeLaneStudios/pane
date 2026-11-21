//! Unit tests for §4.5.2 — loop_timeout_seconds setting
//!
//! Verifies that the agentic loop's no-progress (idle) timeout limit is read
//! from ai_settings, clamped to 30–300, and falls back to 120 when the key is
//! absent or unparseable. #3053 D+E repurposed this setting from a total
//! wall-clock cap to a no-progress limit, WITHOUT renaming the ai_settings
//! key (`loop_timeout_seconds`) — renaming a persisted key is a bigger blast
//! radius than relabeling it (see commands.rs's `idle_timeout_seconds` read
//! site), so this resolution chain is unchanged; only how the resolved value
//! is USED inside the loop changed (see `timeout_behavior_tests` in this same
//! module for that half).
//!
//! GitHub Issue: #1588, #3053

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// Set up an in-memory DB with the ai_settings table
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        conn.execute_batch(
            "CREATE TABLE ai_settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )
        .expect("Failed to create ai_settings table");
        conn
    }

    /// Simulate the exact chain used in ai_chat_stream for resolving loop_timeout_seconds
    fn resolve_loop_timeout(conn: &Connection) -> u64 {
        crate::database::get_ai_setting(conn, "loop_timeout_seconds")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|v| v.clamp(30, 300))
            .unwrap_or(120)
    }

    /// Insert a value into ai_settings
    fn set_setting(conn: &Connection, key: &str, value: &str) {
        conn.execute(
            "INSERT OR REPLACE INTO ai_settings (key, value) VALUES (?1, ?2)",
            rusqlite::params![key, value],
        )
        .expect("Failed to insert ai_setting");
    }

    // --- Happy path ---

    #[test]
    fn test_reads_loop_timeout_from_settings() {
        let conn = setup_db();
        set_setting(&conn, "loop_timeout_seconds", "60");
        let result = resolve_loop_timeout(&conn);
        assert_eq!(result, 60, "should return the configured value 60");
    }

    #[test]
    fn test_fallback_timeout_when_key_absent() {
        let conn = setup_db();
        // No key inserted — should fall back to 120
        let result = resolve_loop_timeout(&conn);
        assert_eq!(result, 120, "should fall back to default 120 when key absent");
    }

    // --- Clamp boundaries ---

    #[test]
    fn test_clamp_timeout_below_minimum_to_30() {
        let conn = setup_db();
        set_setting(&conn, "loop_timeout_seconds", "5");
        let result = resolve_loop_timeout(&conn);
        assert_eq!(result, 30, "5 should clamp to minimum of 30");
    }

    #[test]
    fn test_clamp_timeout_above_maximum_to_300() {
        let conn = setup_db();
        set_setting(&conn, "loop_timeout_seconds", "999");
        let result = resolve_loop_timeout(&conn);
        assert_eq!(result, 300, "999 should clamp to maximum of 300");
    }

    #[test]
    fn test_timeout_boundary_30_passes_through() {
        let conn = setup_db();
        set_setting(&conn, "loop_timeout_seconds", "30");
        let result = resolve_loop_timeout(&conn);
        assert_eq!(result, 30, "boundary value 30 should pass through unchanged");
    }

    #[test]
    fn test_timeout_boundary_300_passes_through() {
        let conn = setup_db();
        set_setting(&conn, "loop_timeout_seconds", "300");
        let result = resolve_loop_timeout(&conn);
        assert_eq!(result, 300, "boundary value 300 should pass through unchanged");
    }

    // --- Fallback on bad input ---

    #[test]
    fn test_fallback_timeout_on_unparseable_value() {
        let conn = setup_db();
        set_setting(&conn, "loop_timeout_seconds", "not-a-number");
        let result = resolve_loop_timeout(&conn);
        assert_eq!(result, 120, "should fall back to 120 when value is unparseable");
    }

    #[test]
    fn test_fallback_timeout_on_empty_string() {
        let conn = setup_db();
        set_setting(&conn, "loop_timeout_seconds", "");
        let result = resolve_loop_timeout(&conn);
        assert_eq!(result, 120, "should fall back to 120 when value is empty string");
    }
}
