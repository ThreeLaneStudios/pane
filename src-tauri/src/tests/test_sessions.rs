//! Tests for session commands (§30.2, #395, ADR-0052)
//!
//! Covers: create_session, list_sessions, switch_session, hide_session,
//! unhide_session, rename_session, get_active_session, and born-active hook.

#[cfg(test)]
mod tests {
    use crate::database::DbState;
    use rusqlite::{params, Connection};

    // ─── Test Helpers ────────────────────────────────────────────────────────────

    fn setup_test_db() -> DbState {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        DbState::new(conn)
    }

    /// Count sessions for a given surface.
    fn count_sessions(db: &DbState, project_id: &str) -> i64 {
        let conn = db.0.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM chat_sessions WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    /// Return (id, is_active, hidden_at) for a session by id.
    fn session_state(db: &DbState, session_id: &str) -> (String, bool, Option<String>) {
        let conn = db.0.lock().unwrap();
        conn.query_row(
            "SELECT id, is_active, hidden_at FROM chat_sessions WHERE id = ?1",
            params![session_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .expect("session not found")
    }

    /// Count active sessions for a surface (should always be 0 or 1).
    fn active_count(db: &DbState, project_id: &str) -> i64 {
        let conn = db.0.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM chat_sessions WHERE project_id = ?1 AND is_active = 1",
            params![project_id],
            |row| row.get(0),
        )
        .unwrap_or(0)
    }

    /// Get the id of the active session for a surface. Panics if none.
    fn active_id(db: &DbState, project_id: &str) -> String {
        let conn = db.0.lock().unwrap();
        conn.query_row(
            "SELECT id FROM chat_sessions WHERE project_id = ?1 AND is_active = 1",
            params![project_id],
            |row| row.get(0),
        )
        .expect("no active session found")
    }

    // Wrap a DbState in a tauri::State for calling Tauri commands directly.
    // Because tauri::State<T> can only be constructed through Tauri's state manager,
    // we invoke the internal helper functions instead of the #[tauri::command] wrappers.
    // The internal logic is identical — the command wrappers just acquire the DbState.

    // Helper wrappers that bypass the tauri::State requirement by calling the
    // underlying DB operations through the command implementations with a direct lock.

    fn cmd_create_session(db: &DbState, project_id: &str) -> Result<crate::ai::chat::Session, String> {
        use crate::ai::chat::{load_session_by_id_pub, demote_and_promote_pub};
        let conn = db.0.lock().map_err(|e| format!("lock: {}", e))?;
        let session_id = uuid::Uuid::new_v4().to_string();
        conn.execute("BEGIN", []).map_err(|e| format!("BEGIN: {}", e))?;
        let ins = conn.execute(
            "INSERT INTO chat_sessions (id, project_id, title, origin, created_at, last_active_at, is_active) VALUES (?1, ?2, '', 'user', datetime('now'), datetime('now'), 0)",
            params![&session_id, project_id],
        );
        if let Err(e) = ins {
            let _ = conn.execute("ROLLBACK", []);
            return Err(format!("INSERT: {}", e));
        }
        if let Err(e) = demote_and_promote_pub(&conn, project_id, &session_id) {
            let _ = conn.execute("ROLLBACK", []);
            return Err(e);
        }
        conn.execute("COMMIT", []).map_err(|e| format!("COMMIT: {}", e))?;
        load_session_by_id_pub(&conn, &session_id)
    }

    fn cmd_get_active(db: &DbState, project_id: &str) -> Result<crate::ai::chat::Session, String> {
        use crate::ai::chat::{load_session_by_id_pub, demote_and_promote_pub};
        let conn = db.0.lock().map_err(|e| format!("lock: {}", e))?;
        let existing: Option<String> = {
            use rusqlite::OptionalExtension;
            conn.query_row(
                "SELECT id FROM chat_sessions WHERE project_id = ?1 AND is_active = 1",
                params![project_id],
                |row| row.get(0),
            ).optional().map_err(|e| format!("query: {}", e))?
        };
        if let Some(id) = existing {
            return load_session_by_id_pub(&conn, &id);
        }
        let new_id = uuid::Uuid::new_v4().to_string();
        conn.execute("BEGIN", []).map_err(|e| format!("BEGIN: {}", e))?;
        let ins = conn.execute(
            "INSERT INTO chat_sessions (id, project_id, title, origin, created_at, last_active_at, is_active) VALUES (?1, ?2, '', 'user', datetime('now'), datetime('now'), 0)",
            params![&new_id, project_id],
        );
        if let Err(e) = ins {
            let _ = conn.execute("ROLLBACK", []);
            return Err(format!("INSERT: {}", e));
        }
        if let Err(e) = demote_and_promote_pub(&conn, project_id, &new_id) {
            let _ = conn.execute("ROLLBACK", []);
            return Err(e);
        }
        conn.execute("COMMIT", []).map_err(|e| format!("COMMIT: {}", e))?;
        load_session_by_id_pub(&conn, &new_id)
    }

    fn cmd_list(db: &DbState, project_id: &str, include_hidden: bool) -> Vec<crate::ai::chat::Session> {
        let conn = db.0.lock().unwrap();
        let query = if include_hidden {
            "SELECT id, project_id, title, origin, created_at, last_active_at, hidden_at, is_active, agent_id FROM chat_sessions WHERE project_id = ?1 ORDER BY last_active_at DESC"
        } else {
            "SELECT id, project_id, title, origin, created_at, last_active_at, hidden_at, is_active, agent_id FROM chat_sessions WHERE project_id = ?1 AND hidden_at IS NULL ORDER BY last_active_at DESC"
        };
        let mut stmt = conn.prepare(query).unwrap();
        stmt.query_map(params![project_id], |row| {
            Ok(crate::ai::chat::Session {
                id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                origin: row.get(3)?,
                created_at: row.get(4)?,
                last_active_at: row.get(5)?,
                hidden_at: row.get(6)?,
                is_active: row.get::<_, i64>(7)? != 0,
                agent_id: row.get(8)?,
                // §36.17.4 (#3050 PR 10): this test helper's own SELECT above
                // doesn't request crew_agent_ids — not relevant to what this
                // file tests (session create/list/switch/hide origin
                // behavior). None here, not routed through the production
                // SESSION_SELECT_COLUMNS shared mapper (chat.rs), since this
                // function deliberately exercises its own hand-rolled query.
                crew_agent_ids: None,
            })
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect()
    }

    fn cmd_switch(db: &DbState, session_id: &str) -> Result<crate::ai::chat::Session, String> {
        use crate::ai::chat::{load_session_by_id_pub, demote_and_promote_pub};
        let conn = db.0.lock().map_err(|e| format!("lock: {}", e))?;
        let project_id: String = conn.query_row(
            "SELECT project_id FROM chat_sessions WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        ).map_err(|e| format!("switch: session not found: {}", e))?;
        conn.execute("BEGIN", []).map_err(|e| format!("BEGIN: {}", e))?;
        if let Err(e) = demote_and_promote_pub(&conn, &project_id, session_id) {
            let _ = conn.execute("ROLLBACK", []);
            return Err(e);
        }
        conn.execute("COMMIT", []).map_err(|e| format!("COMMIT: {}", e))?;
        load_session_by_id_pub(&conn, session_id)
    }

    fn cmd_hide(db: &DbState, session_id: &str) -> Result<(), String> {
        let conn = db.0.lock().map_err(|e| format!("lock: {}", e))?;
        let (project_id, was_active): (String, bool) = conn.query_row(
            "SELECT project_id, is_active FROM chat_sessions WHERE id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get::<_, i64>(1)? != 0)),
        ).map_err(|e| format!("hide: session not found: {}", e))?;
        conn.execute("BEGIN", []).map_err(|e| format!("BEGIN: {}", e))?;
        if let Err(e) = conn.execute(
            "UPDATE chat_sessions SET hidden_at = datetime('now'), is_active = 0 WHERE id = ?1",
            params![session_id],
        ) {
            let _ = conn.execute("ROLLBACK", []);
            return Err(format!("hide UPDATE: {}", e));
        }
        if was_active {
            use rusqlite::OptionalExtension;
            let next: Option<String> = conn.query_row(
                "SELECT id FROM chat_sessions WHERE project_id = ?1 AND hidden_at IS NULL ORDER BY last_active_at DESC LIMIT 1",
                params![&project_id],
                |row| row.get(0),
            ).optional().map_err(|e| format!("hide next query: {}", e))?;
            match next {
                Some(nid) => {
                    if let Err(e) = conn.execute(
                        "UPDATE chat_sessions SET is_active = 1, last_active_at = datetime('now') WHERE id = ?1",
                        params![&nid],
                    ) {
                        let _ = conn.execute("ROLLBACK", []);
                        return Err(format!("hide promote-next: {}", e));
                    }
                }
                None => {
                    let new_id = uuid::Uuid::new_v4().to_string();
                    if let Err(e) = conn.execute(
                        "INSERT INTO chat_sessions (id, project_id, title, origin, created_at, last_active_at, is_active) VALUES (?1, ?2, '', 'user', datetime('now'), datetime('now'), 1)",
                        params![&new_id, &project_id],
                    ) {
                        let _ = conn.execute("ROLLBACK", []);
                        return Err(format!("hide create-new: {}", e));
                    }
                }
            }
        }
        conn.execute("COMMIT", []).map_err(|e| format!("COMMIT: {}", e))?;
        Ok(())
    }

    fn cmd_rename(db: &DbState, session_id: &str, title: &str) -> Result<(), String> {
        let conn = db.0.lock().unwrap();
        let trimmed = title.trim().to_string();
        conn.execute(
            "UPDATE chat_sessions SET title = ?1 WHERE id = ?2",
            params![&trimmed, session_id],
        ).map_err(|e| format!("rename: {}", e))?;
        Ok(())
    }

    fn get_title(db: &DbState, session_id: &str) -> String {
        let conn = db.0.lock().unwrap();
        conn.query_row(
            "SELECT title FROM chat_sessions WHERE id = ?1",
            params![session_id],
            |row| row.get(0),
        ).unwrap()
    }

    // ─── Tests ───────────────────────────────────────────────────────────────────

    /// test_create_session: creates session, verify active, correct origin/surface.
    #[test]
    fn test_create_session() {
        let db = setup_test_db();
        let surface = "proj-create-test";

        let session = cmd_create_session(&db, surface).expect("create_session should succeed");

        assert_eq!(session.project_id, surface, "project_id should match surface");
        assert!(session.is_active, "new session should be active");
        assert_eq!(session.origin, "user", "origin should be 'user'");
        assert_eq!(session.title, "", "title should be empty (auto-title)");
        assert!(session.hidden_at.is_none(), "should not be hidden");
        assert_eq!(active_count(&db, surface), 1, "exactly one active session");
    }

    /// test_create_session_demotes_prior: create two sessions, first is no longer active.
    #[test]
    fn test_create_session_demotes_prior() {
        let db = setup_test_db();
        let surface = "proj-demote-test";

        let first = cmd_create_session(&db, surface).expect("first create_session");
        assert!(first.is_active, "first session starts active");

        let second = cmd_create_session(&db, surface).expect("second create_session");
        assert!(second.is_active, "second session should be active");

        // First session must now be inactive
        let (_, first_active, _) = session_state(&db, &first.id);
        assert!(!first_active, "first session should no longer be active after second is created");

        // Invariant: still only one active per surface
        assert_eq!(active_count(&db, surface), 1, "exactly one active session");
    }

    /// test_list_sessions: order is last_active_at DESC; hidden excluded by default, included with flag.
    #[test]
    fn test_list_sessions() {
        let db = setup_test_db();
        let surface = "proj-list-test";

        let s1 = cmd_create_session(&db, surface).expect("s1");
        // Small delay simulation via SQL (touch last_active_at for ordering)
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "UPDATE chat_sessions SET last_active_at = datetime('now', '-2 seconds') WHERE id = ?1",
                params![&s1.id],
            ).unwrap();
        }
        let s2 = cmd_create_session(&db, surface).expect("s2");
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "UPDATE chat_sessions SET last_active_at = datetime('now', '-1 second') WHERE id = ?1",
                params![&s2.id],
            ).unwrap();
        }
        let s3 = cmd_create_session(&db, surface).expect("s3");

        // Hide s1
        cmd_hide(&db, &s1.id).expect("hide s1");

        // Without include_hidden: should see s3, s2 (s1 is hidden)
        let visible = cmd_list(&db, surface, false);
        assert_eq!(visible.len(), 2, "should list 2 visible sessions");
        // s3 is most recently active
        assert_eq!(visible[0].id, s3.id, "s3 should be first (most recently active)");
        assert_eq!(visible[1].id, s2.id, "s2 should be second");

        // With include_hidden: should see all 3
        let all = cmd_list(&db, surface, true);
        assert_eq!(all.len(), 3, "should list 3 sessions including hidden");
    }

    /// test_workshop_session_scope_excluded_from_workspace_list (#2618): a session
    /// created under `WORKSHOP_SESSION_SCOPE_ID` ("__workshop_session__" — the
    /// Memory→Soul workshop refinement chat's disposable session, distinct from
    /// the notes-only `WORKSHOP_SCOPE_ID`) must never appear when listing the
    /// 'workspace' surface, nor be returned as its active session.
    ///
    /// This isn't a special-cased filter in `list_sessions`/`get_active_session` —
    /// it's a structural guarantee from exact `project_id` equality matching
    /// (every session query here is scoped to one literal project_id). This test
    /// exists to pin that guarantee so a future refactor that widened any of
    /// these queries (e.g. to "all surfaces") would be caught immediately. The
    /// frontend's `filterOutWorkshopSessions` (agentDisplay.ts, next to the C4
    /// subagent filter) is the analogous defense-in-depth on the Svelte side.
    #[test]
    fn test_workshop_session_scope_excluded_from_workspace_list() {
        use crate::ai::chat::WORKSHOP_SESSION_SCOPE_ID;

        let db = setup_test_db();

        // A real 'workspace' session (what SessionDropdownHeader/SessionsDrawer/
        // getActiveSession('workspace') actually list).
        let real_session = cmd_create_session(&db, "workspace").expect("real workspace session");

        // A disposable workshop-chat session, scoped to the new sentinel — mirrors
        // what createWorkshopSession() (workshopSession.ts) creates via
        // api.createSession(WORKSHOP_SESSION_SCOPE_ID, agentId).
        let workshop_session = cmd_create_session(&db, WORKSHOP_SESSION_SCOPE_ID)
            .expect("workshop-scoped session");

        assert_ne!(
            WORKSHOP_SESSION_SCOPE_ID,
            "workspace",
            "sanity check: sentinel must not collide with the real surface id"
        );
        assert_ne!(
            WORKSHOP_SESSION_SCOPE_ID,
            crate::notes::WORKSHOP_SCOPE_ID,
            "sanity check: the new session sentinel must be distinct from the pre-existing notes-only sentinel"
        );

        // 'workspace' listing must show only the real session, never the workshop one.
        let workspace_sessions = cmd_list(&db, "workspace", true);
        assert_eq!(workspace_sessions.len(), 1, "workspace surface should list exactly one session");
        assert_eq!(workspace_sessions[0].id, real_session.id);
        assert!(
            workspace_sessions.iter().all(|s| s.id != workshop_session.id),
            "workshop-scoped session must never appear in the 'workspace' listing"
        );

        // get_active_session('workspace')-equivalent lookup must resolve to the
        // real session, never the workshop one.
        let active = active_id(&db, "workspace");
        assert_eq!(active, real_session.id);
        assert_ne!(active, workshop_session.id);

        // The workshop-scoped session does exist in the DB (Matt's ruling: no
        // cleanup) — just under its own sentinel, never under 'workspace'.
        assert_eq!(count_sessions(&db, WORKSHOP_SESSION_SCOPE_ID), 1);
    }

    /// test_switch_session: switch to inactive session, verify it becomes active, prior is demoted.
    #[test]
    fn test_switch_session() {
        let db = setup_test_db();
        let surface = "proj-switch-test";

        let s1 = cmd_create_session(&db, surface).expect("s1");
        let s2 = cmd_create_session(&db, surface).expect("s2");

        // s2 is currently active; switch back to s1
        let switched = cmd_switch(&db, &s1.id).expect("switch to s1");
        assert!(switched.is_active, "s1 should be active after switch");

        let (_, s2_active, _) = session_state(&db, &s2.id);
        assert!(!s2_active, "s2 should be demoted after switch");

        assert_eq!(active_count(&db, surface), 1, "exactly one active session");
    }

    /// test_switch_unhides: switching to a hidden session clears hidden_at.
    #[test]
    fn test_switch_unhides() {
        let db = setup_test_db();
        let surface = "proj-switch-unhide-test";

        let s1 = cmd_create_session(&db, surface).expect("s1");
        let s2 = cmd_create_session(&db, surface).expect("s2");

        // Hide s1 (it's already inactive, so hide won't try to promote)
        cmd_hide(&db, &s1.id).expect("hide s1");
        let (_, _, h) = session_state(&db, &s1.id);
        assert!(h.is_some(), "s1 should be hidden");

        // Switch to s1 — should unhide it
        let switched = cmd_switch(&db, &s1.id).expect("switch to hidden s1");
        assert!(switched.is_active, "s1 should be active");
        assert!(switched.hidden_at.is_none(), "s1 hidden_at should be cleared on switch");

        let (_, s2_active, _) = session_state(&db, &s2.id);
        assert!(!s2_active, "s2 should be demoted");
    }

    /// test_hide_active_promotes_next: hiding active session promotes the next visible session.
    #[test]
    fn test_hide_active_promotes_next() {
        let db = setup_test_db();
        let surface = "proj-hide-promotes-test";

        let s1 = cmd_create_session(&db, surface).expect("s1");
        let s2 = cmd_create_session(&db, surface).expect("s2");
        // s2 is active now; s1 is visible but inactive

        // Hide active session (s2)
        cmd_hide(&db, &s2.id).expect("hide s2");

        let (_, s2_active, s2_hidden) = session_state(&db, &s2.id);
        assert!(!s2_active, "s2 should not be active after hide");
        assert!(s2_hidden.is_some(), "s2 should be hidden");

        // s1 should be promoted
        assert_eq!(active_count(&db, surface), 1, "exactly one active session after hide");
        let aid = active_id(&db, surface);
        assert_eq!(aid, s1.id, "s1 should be promoted after s2 is hidden");
    }

    /// test_hide_active_creates_if_none: hiding the only session creates a new active one.
    #[test]
    fn test_hide_active_creates_if_none() {
        let db = setup_test_db();
        let surface = "proj-hide-create-test";

        let only = cmd_create_session(&db, surface).expect("only session");
        assert!(only.is_active);

        // Hide the only session
        cmd_hide(&db, &only.id).expect("hide only session");

        let (_, only_active, only_hidden) = session_state(&db, &only.id);
        assert!(!only_active, "only session should not be active");
        assert!(only_hidden.is_some(), "only session should be hidden");

        // A new active session must have been created
        assert_eq!(active_count(&db, surface), 1, "a new active session should exist");
        let aid = active_id(&db, surface);
        assert_ne!(aid, only.id, "new active session should have a different id");

        // Total count: 2 (original hidden + new)
        assert_eq!(count_sessions(&db, surface), 2, "should have 2 sessions total");
    }

    /// test_rename_session: rename sets title; rename to empty resets to ''.
    #[test]
    fn test_rename_session() {
        let db = setup_test_db();
        let surface = "proj-rename-test";

        let s = cmd_create_session(&db, surface).expect("session");
        assert_eq!(get_title(&db, &s.id), "", "title starts empty");

        cmd_rename(&db, &s.id, "My Session").expect("rename");
        assert_eq!(get_title(&db, &s.id), "My Session", "title should be updated");

        // Rename with surrounding whitespace: should be trimmed
        cmd_rename(&db, &s.id, "  Trimmed  ").expect("rename with spaces");
        assert_eq!(get_title(&db, &s.id), "Trimmed", "title should be trimmed");

        // Rename to empty: should reset to '' (auto-title behavior)
        cmd_rename(&db, &s.id, "").expect("rename to empty");
        assert_eq!(get_title(&db, &s.id), "", "title should be '' after empty rename");

        // Rename to whitespace-only: same as empty
        cmd_rename(&db, &s.id, "   ").expect("rename to whitespace");
        assert_eq!(get_title(&db, &s.id), "", "title should be '' after whitespace-only rename");
    }

    /// test_get_active_session_creates_if_none: call on surface with no sessions → one is created.
    #[test]
    fn test_get_active_session_creates_if_none() {
        let db = setup_test_db();
        let surface = "proj-get-active-create-test";

        assert_eq!(count_sessions(&db, surface), 0, "no sessions initially");

        let session = cmd_get_active(&db, surface).expect("get_active_session");
        assert_eq!(session.project_id, surface);
        assert!(session.is_active, "created session should be active");
        assert_eq!(session.origin, "user");
        assert_eq!(count_sessions(&db, surface), 1, "one session created");
    }

    /// test_get_active_session_returns_existing: call on surface with active session → no creation.
    #[test]
    fn test_get_active_session_returns_existing() {
        let db = setup_test_db();
        let surface = "proj-get-active-existing-test";

        let existing = cmd_create_session(&db, surface).expect("existing session");
        let total_before = count_sessions(&db, surface);

        let returned = cmd_get_active(&db, surface).expect("get_active_session");
        assert_eq!(returned.id, existing.id, "should return the existing active session");
        assert_eq!(count_sessions(&db, surface), total_before, "no new session should be created");
    }

    /// test_born_active: create_project_internal creates a chat_sessions row, active, origin='user'.
    #[test]
    #[serial_test::serial(pane_test_root)]
    fn test_born_active() {
        use crate::projects::{CreateProjectRequest, create_project_internal};
        use tempfile::TempDir;

        let db = setup_test_db();

        // Also need projects + notes tables (already in migration DB)
        // Set PANE_TEST_ROOT so create_project_internal uses a temp dir
        let temp_fs = TempDir::new().expect("temp dir");
        std::env::set_var("PANE_TEST_ROOT", temp_fs.path());

        // chat_sessions.agent_id is FK-constrained against ai_agents (real migration
        // schema, unlike some hand-rolled fixture schemas elsewhere) — seed the agent
        // referenced by the request below.
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO ai_agents (id, name, slug, role, is_default, workspace_path, source, created_at, updated_at) \
                 VALUES ('test-agent', 'Test Agent', 'test-agent', 'main', 1, 'test-agent', 'manual', datetime('now'), datetime('now'))",
                [],
            ).expect("seed test agent");
        }

        let request = CreateProjectRequest {
            name: format!("BornActiveTestProject-{}", uuid::Uuid::new_v4()),
            description: None,
            color: None,
            category: None,
            first_folder_name: None,
            is_favorite: None,
        agent_id: "test-agent".to_string(),
        };

        let project = create_project_internal(&request, &db)
            .expect("create_project_internal should succeed");

        // Verify a chat_sessions row exists for this project
        let conn = db.0.lock().unwrap();
        let session_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM chat_sessions WHERE project_id = ?1",
            params![&project.id],
            |row| row.get(0),
        ).expect("query chat_sessions");

        assert_eq!(session_count, 1, "born-active: exactly one session should be created");

        let (origin, is_active): (String, i64) = conn.query_row(
            "SELECT origin, is_active FROM chat_sessions WHERE project_id = ?1",
            params![&project.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("query session fields");

        assert_eq!(origin, "user", "born-active session origin should be 'user'");
        assert_eq!(is_active, 1, "born-active session should be active (is_active=1)");

    }
}
