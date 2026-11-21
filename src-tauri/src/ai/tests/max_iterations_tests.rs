//! Unit tests for §4.5.2 — max_tool_iterations setting
//!
//! Verifies that the agentic loop iteration cap is read from ai_settings,
//! clamped to 1–30, and falls back to 30 when the key is absent or unparseable.
//!
//! fix(runtime): checkpoint default 10 → 30 (#3050 PR 3 follow-up, §36.2.2
//! Ruling 4, Matt 2026-10-27) — the fallback/seed value only moved from 10 to
//! 30; the clamp range (1–30) is unchanged, and an existing stored value is
//! never rewritten (see `test_stored_value_survives_default_change_to_30`
//! below).
//!
//! GitHub Issue: #1589, #3050

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

    /// Simulate the exact chain used in ai_chat_stream for resolving max_tool_iterations
    fn resolve_max_iterations(conn: &Connection) -> u32 {
        crate::database::get_ai_setting(conn, "max_tool_iterations")
            .and_then(|v| v.parse::<u32>().ok())
            .map(|v| v.clamp(1, 30))
            .unwrap_or(30)
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
    fn test_reads_configured_value() {
        let conn = setup_db();
        set_setting(&conn, "max_tool_iterations", "15");
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 15, "should return the configured value 15");
    }

    #[test]
    fn test_fallback_to_default_when_key_absent() {
        let conn = setup_db();
        // No key inserted — should fall back to 30 (was 10 before #3050 PR 3's
        // checkpoint-default fix; §36.2.2 Ruling 4, Matt 2026-10-27)
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 30, "should fall back to default 30 when key absent");
    }

    #[test]
    fn test_fallback_to_default_on_unparseable_value() {
        let conn = setup_db();
        set_setting(&conn, "max_tool_iterations", "not-a-number");
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 30, "should fall back to 30 when value is unparseable");
    }

    #[test]
    fn test_fallback_to_default_on_empty_string() {
        let conn = setup_db();
        set_setting(&conn, "max_tool_iterations", "");
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 30, "should fall back to 30 when value is empty string");
    }

    /// fix(runtime): checkpoint default 10 → 30 (#3050 PR 3 follow-up) — the
    /// change is to the SEED/FALLBACK only. An existing installation that
    /// already has a stored `max_tool_iterations` row (e.g. the pre-PR-3
    /// default of 10, written before this fix shipped) must keep reading
    /// back exactly that stored value — never silently rewritten/migrated
    /// to the new default. No migration touches this key (confirmed:
    /// `grep -rn "max_tool_iterations" migrations.rs database.rs` has zero
    /// hits outside `get_ai_setting`/`set_ai_setting` themselves); this test
    /// pins that behavior at the read-chain level so a future change can't
    /// reintroduce a write-back.
    #[test]
    fn test_stored_value_survives_default_change_to_30() {
        let conn = setup_db();
        // Simulates an existing user's row, written under the OLD default
        // (10) before this fix — still a perfectly valid, in-range value.
        set_setting(&conn, "max_tool_iterations", "10");
        let result = resolve_max_iterations(&conn);
        assert_eq!(
            result, 10,
            "an existing stored value of 10 must survive the default-seed \
             change unchanged — the new 30 default applies only when the \
             key is absent, never as a write-back over a stored row"
        );
    }

    // --- Clamp boundaries ---

    #[test]
    fn test_clamp_below_minimum_to_1() {
        let conn = setup_db();
        set_setting(&conn, "max_tool_iterations", "0");
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 1, "0 should clamp to minimum of 1");
    }

    #[test]
    fn test_clamp_above_maximum_to_30() {
        let conn = setup_db();
        set_setting(&conn, "max_tool_iterations", "50");
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 30, "50 should clamp to maximum of 30");
    }

    #[test]
    fn test_boundary_value_1_passes_through() {
        let conn = setup_db();
        set_setting(&conn, "max_tool_iterations", "1");
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 1, "boundary value 1 should pass through unchanged");
    }

    #[test]
    fn test_boundary_value_30_passes_through() {
        let conn = setup_db();
        set_setting(&conn, "max_tool_iterations", "30");
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 30, "boundary value 30 should pass through unchanged");
    }

    #[test]
    fn test_mid_range_value_passes_through() {
        let conn = setup_db();
        set_setting(&conn, "max_tool_iterations", "15");
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 15, "mid-range value 15 should pass through unchanged");
    }

    // --- Negative integers (u32 parse fails → fallback) ---

    #[test]
    fn test_negative_value_falls_back_to_default() {
        let conn = setup_db();
        // u32::parse("-5") fails → falls back to 30
        set_setting(&conn, "max_tool_iterations", "-5");
        let result = resolve_max_iterations(&conn);
        assert_eq!(result, 30, "negative value should fall back to default 30");
    }
}
