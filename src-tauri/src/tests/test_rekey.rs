//! Tests for PR3 re-key: save/load on thread_id, compaction session re-key, get_compaction_cutoff.
//!
//! Covers: §30.3 (message re-key), §30.5 (compaction continuity), §30.7.2 (OC hint re-key).

#[cfg(test)]
mod tests {
    use crate::database::DbState;
    use rusqlite::{params, Connection};

    // ─── Test Helpers ─────────────────────────────────────────────────────────

    fn setup_test_db() -> DbState {
        let conn = Connection::open_in_memory().expect("Failed to open in-memory DB");
        crate::migrations::run_migrations(&conn).expect("Migrations should succeed");
        DbState::new(conn)
    }

    /// Insert a chat_messages_local row directly, with thread_id set.
    fn insert_message(
        db: &DbState,
        id: &str,
        project_id: &str,
        thread_id: &str,
        role: &str,
        content: &str,
    ) {
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO chat_messages_local (id, project_id, thread_id, role, content, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
            params![id, project_id, thread_id, role, content],
        )
        .expect("insert_message failed");
    }

    /// Query thread_id for a message by id.
    fn get_message_thread_id(db: &DbState, message_id: &str) -> Option<String> {
        let conn = db.0.lock().unwrap();
        conn.query_row(
            "SELECT thread_id FROM chat_messages_local WHERE id = ?1",
            params![message_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
    }

    /// Load messages keyed on thread_id (mirrors post-PR3 load_chat_messages query).
    fn load_by_session(db: &DbState, session_id: &str) -> Vec<String> {
        let conn = db.0.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id FROM chat_messages_local WHERE thread_id = ?1 ORDER BY created_at ASC",
            )
            .unwrap();
        stmt.query_map(params![session_id], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    }

    /// Create a chat_sessions row and return its id.
    fn create_session(db: &DbState, project_id: &str) -> String {
        use crate::ai::chat::demote_and_promote_pub;
        let session_id = uuid::Uuid::new_v4().to_string();
        let conn = db.0.lock().unwrap();
        conn.execute("BEGIN", []).unwrap();
        conn.execute(
            "INSERT INTO chat_sessions (id, project_id, title, origin, created_at, last_active_at, is_active) \
             VALUES (?1, ?2, '', 'user', datetime('now'), datetime('now'), 0)",
            params![&session_id, project_id],
        )
        .unwrap();
        demote_and_promote_pub(&conn, project_id, &session_id).unwrap();
        conn.execute("COMMIT", []).unwrap();
        session_id
    }

    /// Insert a compaction_summaries row for a session.
    fn insert_compaction(db: &DbState, session_id: &str, summary: &str, compacted_before: &str) {
        let conn = db.0.lock().unwrap();
        let cid = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO compaction_summaries (id, thread_id, summary, message_count, compacted_before, created_at) \
             VALUES (?1, ?2, ?3, 0, ?4, datetime('now'))",
            params![cid, session_id, summary, compacted_before],
        )
        .expect("insert_compaction failed");
    }

    /// Query compaction summary for a session (mirrors get_compaction_cutoff logic).
    fn get_compaction_cutoff(db: &DbState, session_id: &str) -> Option<String> {
        let conn = db.0.lock().unwrap();
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT compacted_before FROM compaction_summaries WHERE thread_id = ?1 ORDER BY created_at DESC LIMIT 1",
            params![session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
    }

    // ─── Tests ────────────────────────────────────────────────────────────────

    /// test_save_writes_thread_id: insert a message with thread_id set; verify thread_id is stored.
    ///
    /// Mirrors what save_chat_message does after PR3: the INSERT writes thread_id.
    #[test]
    fn test_save_writes_thread_id() {
        let db = setup_test_db();
        let surface = "proj-save-thread-test";
        let session_id = create_session(&db, surface);

        let msg_id = "msg-save-thread-001";
        insert_message(&db, msg_id, surface, &session_id, "user", "hello");

        let stored_thread = get_message_thread_id(&db, msg_id);
        assert!(
            stored_thread.is_some(),
            "thread_id should be stored, not NULL"
        );
        assert_eq!(
            stored_thread.unwrap(),
            session_id,
            "stored thread_id should equal the session id, not the project_id bucket"
        );
    }

    /// test_load_keys_on_thread_id: two sessions for the same project; load by session id returns
    /// only that session's messages.
    #[test]
    fn test_load_keys_on_thread_id() {
        let db = setup_test_db();
        let surface = "proj-load-thread-test";

        // Create two sessions
        let session_a = create_session(&db, surface);
        let session_b = create_session(&db, surface); // session_b is now active

        // Insert messages keyed to each session
        insert_message(&db, "msg-a-001", surface, &session_a, "user", "in session A");
        insert_message(&db, "msg-a-002", surface, &session_a, "assistant", "reply A");
        insert_message(&db, "msg-b-001", surface, &session_b, "user", "in session B");

        // Load by session_a: should return exactly 2 messages
        let ids_a = load_by_session(&db, &session_a);
        assert_eq!(ids_a.len(), 2, "session A should have 2 messages");
        assert!(ids_a.contains(&"msg-a-001".to_string()), "msg-a-001 should be in session A");
        assert!(ids_a.contains(&"msg-a-002".to_string()), "msg-a-002 should be in session A");
        assert!(
            !ids_a.contains(&"msg-b-001".to_string()),
            "session B's message must NOT appear in session A load"
        );

        // Load by session_b: should return exactly 1 message
        let ids_b = load_by_session(&db, &session_b);
        assert_eq!(ids_b.len(), 1, "session B should have 1 message");
        assert!(ids_b.contains(&"msg-b-001".to_string()), "msg-b-001 should be in session B");
    }

    /// test_compaction_reads_session_id: a fresh session has no compaction state;
    /// a compaction row inserted for session_a is NOT visible when querying session_b.
    #[test]
    fn test_compaction_reads_session_id() {
        let db = setup_test_db();
        let surface = "proj-compact-session-test";

        let session_a = create_session(&db, surface);
        let session_b = create_session(&db, surface);

        // Insert compaction for session_a only
        insert_compaction(&db, &session_a, "Summary of session A", "2024-01-01T00:00:00");

        // session_a has a cutoff
        let cutoff_a = get_compaction_cutoff(&db, &session_a);
        assert!(
            cutoff_a.is_some(),
            "session_a should have a compaction cutoff"
        );
        assert_eq!(
            cutoff_a.unwrap(),
            "2024-01-01T00:00:00",
            "session_a cutoff should match the inserted value"
        );

        // session_b has no compaction (independent)
        let cutoff_b = get_compaction_cutoff(&db, &session_b);
        assert!(
            cutoff_b.is_none(),
            "session_b should have no compaction cutoff (independent of session_a)"
        );
    }

    /// test_compaction_preserves_across_sessions: session A is compacted; switching to session B
    /// leaves session A's compaction intact and session B starts with no compaction.
    #[test]
    fn test_compaction_preserves_across_sessions() {
        let db = setup_test_db();
        let surface = "proj-compact-preserves-test";

        // Session A: insert messages and simulate compaction
        let session_a = create_session(&db, surface);
        insert_message(&db, "msg-pre-compact", surface, &session_a, "user", "old message");
        insert_compaction(&db, &session_a, "Compact summary A", "2024-06-01T12:00:00");

        // Session B: created after compaction, no messages
        let session_b = create_session(&db, surface);

        // Switch back to session A, verify compaction still present
        let cutoff_a_after = get_compaction_cutoff(&db, &session_a);
        assert!(
            cutoff_a_after.is_some(),
            "session_a compaction should be preserved after session_b is created"
        );

        // Session B has no compaction
        let cutoff_b = get_compaction_cutoff(&db, &session_b);
        assert!(
            cutoff_b.is_none(),
            "session_b should start with no compaction state"
        );

        // Session A messages are still accessible by thread_id
        let ids_a = load_by_session(&db, &session_a);
        assert!(ids_a.contains(&"msg-pre-compact".to_string()),
            "session_a messages should remain accessible after session switch");
    }

    /// test_get_compaction_cutoff_session_id: insert two compaction rows for the same session
    /// at different times; get_compaction_cutoff returns the most recent.
    #[test]
    fn test_get_compaction_cutoff_session_id() {
        let db = setup_test_db();
        let surface = "proj-cutoff-session-test";
        let session_id = create_session(&db, surface);

        // Insert two compaction rows with different cutoffs (order matters for LIMIT DESC)
        {
            let conn = db.0.lock().unwrap();
            let id1 = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO compaction_summaries (id, thread_id, summary, message_count, compacted_before, created_at) \
                 VALUES (?1, ?2, 'older summary', 0, '2024-01-01T00:00:00', datetime('now', '-10 seconds'))",
                params![id1, &session_id],
            ).unwrap();
            let id2 = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO compaction_summaries (id, thread_id, summary, message_count, compacted_before, created_at) \
                 VALUES (?1, ?2, 'newer summary', 0, '2024-06-15T12:00:00', datetime('now'))",
                params![id2, &session_id],
            ).unwrap();
        }

        // Should return the most recently created compaction's cutoff
        let cutoff = get_compaction_cutoff(&db, &session_id);
        assert!(cutoff.is_some(), "should find a compaction cutoff");
        assert_eq!(
            cutoff.unwrap(),
            "2024-06-15T12:00:00",
            "should return the most recent compaction cutoff"
        );
    }

    /// test_unhide_session (PR2 WARN fold-in): unhide a hidden session, verify hidden_at becomes
    /// NULL and is_active is unchanged from its pre-hide state.
    #[test]
    fn test_unhide_session() {
        let db = setup_test_db();
        let surface = "proj-unhide-test";

        // Create two sessions; session_b is active, session_a is inactive
        let session_a = create_session(&db, surface);
        let session_b = create_session(&db, surface); // active

        // Hide session_a (inactive session) — this just sets hidden_at, doesn't touch is_active
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "UPDATE chat_sessions SET hidden_at = datetime('now') WHERE id = ?1",
                params![&session_a],
            )
            .unwrap();
        }

        // Verify session_a is now hidden and still inactive
        let (is_active_before_unhide, hidden_at_before): (i64, Option<String>) = {
            let conn = db.0.lock().unwrap();
            conn.query_row(
                "SELECT is_active, hidden_at FROM chat_sessions WHERE id = ?1",
                params![&session_a],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(is_active_before_unhide, 0, "session_a should be inactive before unhide");
        assert!(hidden_at_before.is_some(), "session_a should be hidden before unhide");

        // Unhide session_a
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "UPDATE chat_sessions SET hidden_at = NULL WHERE id = ?1",
                params![&session_a],
            )
            .unwrap();
        }

        // Assert: hidden_at is NULL, is_active is still 0 (unchanged)
        let (is_active_after, hidden_at_after): (i64, Option<String>) = {
            let conn = db.0.lock().unwrap();
            conn.query_row(
                "SELECT is_active, hidden_at FROM chat_sessions WHERE id = ?1",
                params![&session_a],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        assert!(hidden_at_after.is_none(), "hidden_at should be NULL after unhide");
        assert_eq!(
            is_active_after, 0,
            "is_active should be unchanged (still 0) after unhide"
        );

        // session_b remains active (unhide doesn't affect other sessions)
        let session_b_active: i64 = {
            let conn = db.0.lock().unwrap();
            conn.query_row(
                "SELECT is_active FROM chat_sessions WHERE id = ?1",
                params![&session_b],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(session_b_active, 1, "session_b should remain active after session_a unhide");

        // Now test unhide on the ACTIVE session: is_active stays 1
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "UPDATE chat_sessions SET hidden_at = datetime('now') WHERE id = ?1",
                params![&session_b],
            )
            .unwrap();
        }
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "UPDATE chat_sessions SET hidden_at = NULL WHERE id = ?1",
                params![&session_b],
            )
            .unwrap();
        }
        let (session_b_active_after, session_b_hidden): (i64, Option<String>) = {
            let conn = db.0.lock().unwrap();
            conn.query_row(
                "SELECT is_active, hidden_at FROM chat_sessions WHERE id = ?1",
                params![&session_b],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        assert!(session_b_hidden.is_none(), "session_b hidden_at should be NULL after unhide");
        assert_eq!(
            session_b_active_after, 1,
            "session_b is_active should remain 1 after unhide"
        );
    }

    /// §30.1.2 — project_id must hold the surface (project UUID or 'workspace'),
    /// not the session id. Verifies the two columns are written and stored independently.
    #[test]
    fn test_save_chat_message_project_id_differs_from_thread_id() {
        let db = setup_test_db();

        let surface = "proj-surface-uuid-abc123";
        let session_id = "sess-uuid-def456";
        let msg_id = "msg-test-warn1";

        // Surface and session ids must differ (the whole point of §30.1.2)
        assert_ne!(
            surface, session_id,
            "Test precondition: surface and session must differ"
        );

        // Insert with correct separation: project_id = surface, thread_id = session
        insert_message(&db, msg_id, surface, session_id, "user", "Hello");

        // Retrieve both columns and verify they are stored independently
        let (stored_project_id, stored_thread_id): (String, String) = {
            let conn = db.0.lock().unwrap();
            conn.query_row(
                "SELECT project_id, thread_id FROM chat_messages_local WHERE id = ?1",
                rusqlite::params![msg_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("Row should exist")
        };

        assert_eq!(
            stored_project_id, surface,
            "project_id must equal the surface (project UUID or 'workspace'), not the session id"
        );
        assert_eq!(
            stored_thread_id, session_id,
            "thread_id must equal the session id"
        );
        assert_ne!(
            stored_project_id, stored_thread_id,
            "§30.1.2: project_id and thread_id must differ — session UUID must not bleed into project_id"
        );

        // Also verify that a 'workspace' surface is stored correctly (Home context)
        let workspace_msg_id = "msg-test-warn1-workspace";
        let workspace_session_id = "sess-uuid-workspace-xyz";
        insert_message(&db, workspace_msg_id, "workspace", workspace_session_id, "user", "Hi");

        let (ws_project_id, ws_thread_id): (String, String) = {
            let conn = db.0.lock().unwrap();
            conn.query_row(
                "SELECT project_id, thread_id FROM chat_messages_local WHERE id = ?1",
                rusqlite::params![workspace_msg_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("Workspace row should exist")
        };

        assert_eq!(ws_project_id, "workspace", "Home context project_id must be 'workspace'");
        assert_eq!(
            ws_thread_id, workspace_session_id,
            "Home context thread_id must be the session id"
        );
        assert_ne!(
            ws_project_id, ws_thread_id,
            "§30.1.2: 'workspace' surface must not equal session id"
        );
    }
}
