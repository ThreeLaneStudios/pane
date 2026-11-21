//! The single data-change refresh event (#2972, tracked on #2874).
//!
//! `docs/spec/data-refresh.md` §34 is the authority. §34.1's schema block is
//! the single source of truth for the payload shape below — do not invent or
//! rename fields. This module is Stage 1 scope only (the backend-emit half,
//! §34.11 point 1): the app-side sender, origin-tag echo suppression
//! (§34.4), and every listener's migration are Stages 2/3, separate PRs.
//!
//! Both senders (backend here, and the future app `CustomEvent` sender of
//! Stage 3) must emit the byte-identical shape described in §34.1 — these
//! constants exist so neither sender (nor any future one) retypes the event
//! name or the `reason`/`entity_type` string literals by hand.

use tauri::{Emitter, Runtime};

/// Event name — reused `pane-data-changed` (§34.1: "the most-referenced of
/// the six names it replaces; renaming it a seventh time would just be more
/// churn for no behavior gain").
pub const EVENT_DATA_CHANGED: &str = "pane-data-changed";

/// `entity_type` values — exactly these five (§34.1's schema block; #3018
/// adds `category` as the fifth — see that section's "category as a fifth
/// entity_type" ruling). `entity_id` for `category` is the category name
/// itself (there is no other stable identifier — `category_colors` has no
/// surrogate id column). The same four `reason` values apply unchanged:
/// add = `created`, rename/recolor = `edited`, delete = `deleted`. No new
/// `reason` value — a rename is still an edit of the same logical entity,
/// not a delete-then-create.
pub const ENTITY_NOTE: &str = "note";
pub const ENTITY_SHEET: &str = "sheet";
pub const ENTITY_FOLDER: &str = "folder";
pub const ENTITY_PROJECT: &str = "project";
pub const ENTITY_CATEGORY: &str = "category";

/// `reason` values — exactly these four, no fifth (§34.1: "A tool/command
/// that doesn't cleanly map to one of the four... does not get a special
/// case; if the implementation PR finds one, it is a sign the operation is
/// mis-modeled, not a sign the enum needs a fifth value.")
pub const REASON_CREATED: &str = "created";
pub const REASON_EDITED: &str = "edited";
pub const REASON_MOVED: &str = "moved";
pub const REASON_DELETED: &str = "deleted";

/// Emit the one data-change event, §34.1's payload shape.
///
/// `origin_tag` is required on every emit (§34.1) — backend emits are never
/// the "same editor instance" an app-side save would need to suppress an
/// echo against (§34.4), so a fresh UUID per call is sufficient here; §34.4's
/// actual echo-suppression *comparison* logic is Stage 3 scope (nothing
/// reads this field yet — Stage 2 hasn't landed a listener at all).
///
/// Generic over `R: Runtime` (rather than the concrete `tauri::AppHandle` /
/// `AppHandle<Wry>` alias most `#[tauri::command]` handlers in this crate
/// use) specifically so this function — and every call site that forwards
/// to it — stays testable against `tauri::test::mock_app()`'s
/// `AppHandle<MockRuntime>` without constructing a real `AppHandle<Wry>`,
/// the same constraint noted throughout this crate's existing tests (e.g.
/// `ai/commands.rs`'s `sheets_ai_tools_tests` module).
pub fn emit_data_changed<R: Runtime>(
    app: &tauri::AppHandle<R>,
    entity_id: &str,
    entity_type: &str,
    reason: &str,
) {
    let origin_tag = uuid::Uuid::new_v4().to_string();
    let _ = app.emit(
        EVENT_DATA_CHANGED,
        serde_json::json!({
            "entity_id": entity_id,
            "entity_type": entity_type,
            "reason": reason,
            "origin_tag": origin_tag,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_name_is_pane_data_changed() {
        assert_eq!(EVENT_DATA_CHANGED, "pane-data-changed");
    }

    #[test]
    fn test_entity_category_constant_is_category() {
        // #3018: category is a fifth entity_type; `entity_id` is the
        // category name.
        assert_eq!(ENTITY_CATEGORY, "category");
    }

    #[test]
    fn test_emit_data_changed_fires_exactly_once_with_correct_payload() {
        use tauri::Listener;

        let app = tauri::test::mock_app();
        let handle = app.handle();

        let fired: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let fired2 = fired.clone();
        handle.listen(EVENT_DATA_CHANGED, move |event| {
            let payload: serde_json::Value =
                serde_json::from_str(event.payload()).expect("payload must be valid JSON");
            fired2.lock().unwrap().push(payload);
        });

        emit_data_changed(handle, "note-123", ENTITY_NOTE, REASON_EDITED);

        let events = fired.lock().unwrap();
        assert_eq!(events.len(), 1, "must fire exactly once, got {:?}", *events);
        assert_eq!(events[0]["entity_id"], "note-123");
        assert_eq!(events[0]["entity_type"], "note");
        assert_eq!(events[0]["reason"], "edited");
        assert!(
            events[0]["origin_tag"].as_str().is_some_and(|s| !s.is_empty()),
            "origin_tag must be present and non-empty on every emit (§34.1)"
        );
    }

    // #2972 stage 4, Reed's test gap: "Category names with quotes, emoji,
    // slashes and long strings — these are user-controlled text used as
    // `entity_id`; prove they survive the wire and match on the listener
    // side." `emit_data_changed` passes `entity_id` straight into
    // `serde_json::json!` without any sanitization/escaping of its own
    // (correctly — `serde_json` already JSON-escapes quotes/backslashes/
    // control characters and UTF-8-encodes multi-byte characters like
    // emoji per the JSON spec), but nothing previously proved that round
    // trip end-to-end through a REAL `app.emit()` + `Listener::listen()`
    // pair for anything other than a plain ASCII id like `"note-123"`
    // above.
    #[test]
    fn test_emit_data_changed_survives_special_character_entity_ids() {
        use tauri::Listener;

        let special_ids: Vec<&str> = vec![
            "Say \"Hello\" Category",         // embedded double quotes
            "Matt's Category",                 // apostrophe
            "\u{1F389} Launch \u{1F680} Category \u{1F525}", // emoji (multi-byte UTF-8)
            "Work/Personal/Side-Project",       // forward slashes
            "C:\\Projects\\\"Pane\"'s Category", // backslash + mixed quotes
            "Line One\nLine\tTwo",              // embedded newline/tab
        ];

        for special_id in &special_ids {
            let app = tauri::test::mock_app();
            let handle = app.handle();

            let fired: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let fired2 = fired.clone();
            handle.listen(EVENT_DATA_CHANGED, move |event| {
                // Round-trips through the REAL wire shape: `event.payload()`
                // is the raw JSON string Tauri's event system actually
                // delivers, re-parsed here exactly as the frontend's
                // `listen()` callback (refreshEvent.ts's subscribeDataChanged)
                // does on the other side of the IPC boundary.
                let payload: serde_json::Value = serde_json::from_str(event.payload())
                    .expect("payload must be valid JSON even with special characters in entity_id");
                fired2.lock().unwrap().push(payload);
            });

            emit_data_changed(handle, special_id, ENTITY_CATEGORY, REASON_EDITED);

            let events = fired.lock().unwrap();
            assert_eq!(events.len(), 1, "must fire exactly once for id {:?}, got {:?}", special_id, *events);
            assert_eq!(
                events[0]["entity_id"].as_str(),
                Some(*special_id),
                "entity_id must survive the emit/listen round-trip byte-for-byte, unmangled, for {:?}",
                special_id
            );
        }
    }

    #[test]
    fn test_emit_data_changed_survives_a_long_entity_id() {
        use tauri::Listener;

        let long_id: String = "X".repeat(500);

        let app = tauri::test::mock_app();
        let handle = app.handle();

        let fired: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let fired2 = fired.clone();
        handle.listen(EVENT_DATA_CHANGED, move |event| {
            let payload: serde_json::Value = serde_json::from_str(event.payload())
                .expect("payload must be valid JSON for a long entity_id");
            fired2.lock().unwrap().push(payload);
        });

        emit_data_changed(handle, &long_id, ENTITY_CATEGORY, REASON_EDITED);

        let events = fired.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0]["entity_id"].as_str().map(|s| s.len()),
            Some(long_id.len()),
            "a long entity_id must not be truncated by the emit/listen round-trip"
        );
        assert_eq!(events[0]["entity_id"].as_str(), Some(long_id.as_str()));
    }
}
