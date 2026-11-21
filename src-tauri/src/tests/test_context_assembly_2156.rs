// Tests for #2156: Project brief and pinned notes injection in build_context_prompt

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    /// Create an in-memory DB with the minimal schema for context assembly tests.
    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory DB");
        conn.execute_batch(
            "
            CREATE TABLE projects (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT,
                parent_id   TEXT,
                is_system   INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE notes (
                id          TEXT PRIMARY KEY,
                project_id  TEXT NOT NULL,
                title       TEXT NOT NULL,
                file_path   TEXT NOT NULL DEFAULT '',
                body        TEXT,
                deleted_at  TEXT
            );
            CREATE TABLE note_context_pins (
                note_id   TEXT PRIMARY KEY,
                pinned_at TEXT NOT NULL DEFAULT (datetime('now')),
                pinned_by TEXT NOT NULL DEFAULT 'user'
            );
            ",
        )
        .expect("schema");
        conn
    }

    fn insert_project(conn: &Connection, id: &str, name: &str, description: Option<&str>) {
        conn.execute(
            "INSERT INTO projects (id, name, description) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, description],
        )
        .expect("insert project");
    }

    fn insert_note(
        conn: &Connection,
        id: &str,
        project_id: &str,
        title: &str,
        file_path: &str,
        body: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO notes (id, project_id, title, file_path, body) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![id, project_id, title, file_path, body],
        )
        .expect("insert note");
    }

    fn pin_note(conn: &Connection, note_id: &str) {
        conn.execute(
            "INSERT INTO note_context_pins (note_id, pinned_at, pinned_by) VALUES (?1, datetime('now'), 'user')",
            rusqlite::params![note_id],
        )
        .expect("pin note");
    }

    // ── Fix A: Project Brief Injection ─────────────────────────────────────────

    /// Replicate the injected brief logic from build_context_prompt (#2156).
    fn build_brief_snippet(conn: &Connection, project_id: &str) -> String {
        let mut ctx = String::new();
        if let Ok(description) = conn.query_row(
            "SELECT description FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get::<_, Option<String>>(0),
        ) {
            if let Some(ref desc) = description {
                let trimmed = desc.trim();
                if !trimmed.is_empty() {
                    let injected = if trimmed.chars().count() > 500 {
                        format!("{}…", trimmed.chars().take(500).collect::<String>())
                    } else {
                        trimmed.to_string()
                    };
                    ctx.push_str(&format!("Project brief: {}\n", injected));
                }
            }
        }
        ctx
    }

    #[test]
    fn test_brief_injected_when_present() {
        let conn = setup_db();
        insert_project(&conn, "proj-1", "Acme App", Some("Our flagship product brief."));
        let ctx = build_brief_snippet(&conn, "proj-1");
        assert!(
            ctx.contains("Project brief: Our flagship product brief."),
            "Brief should appear in context; got {:?}",
            ctx
        );
    }

    #[test]
    fn test_brief_skipped_when_empty_string() {
        let conn = setup_db();
        insert_project(&conn, "proj-2", "Empty App", Some(""));
        let ctx = build_brief_snippet(&conn, "proj-2");
        assert!(
            !ctx.contains("Project brief:"),
            "Empty brief should NOT appear in context"
        );
    }

    #[test]
    fn test_brief_skipped_when_whitespace_only() {
        let conn = setup_db();
        insert_project(&conn, "proj-3", "Whitespace App", Some("   "));
        let ctx = build_brief_snippet(&conn, "proj-3");
        assert!(
            !ctx.contains("Project brief:"),
            "Whitespace-only brief should NOT appear in context"
        );
    }

    #[test]
    fn test_brief_skipped_when_null() {
        let conn = setup_db();
        insert_project(&conn, "proj-4", "Null App", None);
        let ctx = build_brief_snippet(&conn, "proj-4");
        assert!(
            !ctx.contains("Project brief:"),
            "NULL brief should NOT appear in context"
        );
    }

    #[test]
    fn test_brief_truncated_at_500_chars() {
        let conn = setup_db();
        let long_brief = "x".repeat(550);
        insert_project(&conn, "proj-5", "Long App", Some(&long_brief));
        let ctx = build_brief_snippet(&conn, "proj-5");

        assert!(ctx.contains("Project brief:"), "Brief line should be present");

        // Should contain exactly 500 'x' chars followed by ellipsis
        let expected_value = format!("{}…", "x".repeat(500));
        assert!(
            ctx.contains(&expected_value),
            "Should contain 500 chars + ellipsis; got {:?}",
            ctx
        );

        // Storage is untouched: the DB still has the full 550-char value
        let stored: String = conn
            .query_row(
                "SELECT description FROM projects WHERE id = ?1",
                ["proj-5"],
                |row| row.get(0),
            )
            .expect("query storage");
        assert_eq!(stored.len(), 550, "Storage should be unmodified (550 chars)");
    }

    #[test]
    fn test_brief_not_truncated_at_exactly_500_chars() {
        let conn = setup_db();
        let exact_brief = "y".repeat(500);
        insert_project(&conn, "proj-6", "Exact App", Some(&exact_brief));
        let ctx = build_brief_snippet(&conn, "proj-6");

        assert!(ctx.contains("Project brief:"), "Brief line should be present");
        // Exactly 500 chars — no ellipsis
        assert!(
            !ctx.contains('…'),
            "Exactly 500 chars should NOT be truncated"
        );
    }

    // ── Fix B: Pinned Notes Body-First ──────────────────────────────────────────

    /// Replicate the pinned notes body-first logic from build_context_prompt (#2156).
    fn build_pinned_notes_snippet(conn: &Connection, project_id: &str) -> (String, Vec<String>) {
        let mut ctx = String::new();
        let mut missing_logs: Vec<String> = Vec::new();

        if let Ok(mut stmt) = conn.prepare(
            "SELECT n.title, n.file_path, n.body FROM note_context_pins p
             JOIN notes n ON p.note_id = n.id
             WHERE n.project_id = ?1 AND n.deleted_at IS NULL
             ORDER BY p.pinned_at DESC",
        ) {
            if let Ok(rows) = stmt.query_map(rusqlite::params![project_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            }) {
                let pins: Vec<(String, String, Option<String>)> = rows.flatten().collect();
                if !pins.is_empty() {
                    ctx.push_str(
                        "\n\n## Pinned Notes\nThe following notes are pinned for context:\n",
                    );
                    for (title, file_path, body) in &pins {
                        let content = body
                            .as_deref()
                            .filter(|b| !b.trim().is_empty())
                            .map(|b| b.to_string())
                            .or_else(|| crate::database::read_note_content(file_path).ok());

                        if let Some(content) = content {
                            let truncated = if content.chars().count() > 4000 {
                                let safe: String = content.chars().take(4000).collect();
                                format!("{}... [truncated]", safe)
                            } else {
                                content
                            };
                            ctx.push_str(&format!("\n### {}\n{}\n", title, truncated));
                        } else {
                            let log = format!(
                                "[context] PINNED NOTE CONTENT MISSING: '{}' (file_path='{}', body=None/empty)",
                                title, file_path
                            );
                            missing_logs.push(log);
                        }
                    }
                }
            }
        }

        (ctx, missing_logs)
    }

    #[test]
    fn test_pinned_note_body_appears_in_context() {
        let conn = setup_db();
        insert_project(&conn, "proj-10", "Body App", None);
        insert_note(
            &conn,
            "note-10",
            "proj-10",
            "My Note",
            "", // empty file_path — old code silently dropped this
            Some("This is the note body content."),
        );
        pin_note(&conn, "note-10");

        let (ctx, missing) = build_pinned_notes_snippet(&conn, "proj-10");

        assert!(
            ctx.contains("This is the note body content."),
            "Body content should appear; got {:?}",
            ctx
        );
        assert!(ctx.contains("### My Note"), "Title header should appear");
        assert!(missing.is_empty(), "No missing-content logs expected");
    }

    #[test]
    fn test_pinned_note_empty_body_falls_back_to_file() {
        // When body is None/empty and file_path is also invalid,
        // content should be None and the MISSING log should fire.
        let conn = setup_db();
        insert_project(&conn, "proj-11", "Fallback App", None);
        insert_note(
            &conn,
            "note-11",
            "proj-11",
            "Fallback Note",
            "/nonexistent/path/note.md",
            None, // no body
        );
        pin_note(&conn, "note-11");

        let (ctx, missing) = build_pinned_notes_snippet(&conn, "proj-11");

        // File doesn't exist, body is None → MISSING log fires
        assert!(
            !ctx.contains("Fallback Note"),
            "Content should NOT appear when both body and file are missing"
        );
        assert_eq!(missing.len(), 1, "Should have one MISSING log entry");
        assert!(
            missing[0].contains("PINNED NOTE CONTENT MISSING"),
            "Log should contain greppable marker; got {:?}",
            missing[0]
        );
        assert!(
            missing[0].contains("Fallback Note"),
            "Log should contain note title"
        );
    }

    #[test]
    fn test_pinned_note_no_content_logs_greppable_message() {
        let conn = setup_db();
        insert_project(&conn, "proj-12", "Ghost App", None);
        insert_note(
            &conn,
            "note-12",
            "proj-12",
            "Ghost Note",
            "",   // empty file_path
            None, // no body
        );
        pin_note(&conn, "note-12");

        let (_ctx, missing) = build_pinned_notes_snippet(&conn, "proj-12");

        assert_eq!(missing.len(), 1, "Should have one MISSING log");
        assert!(
            missing[0].contains("[context] PINNED NOTE CONTENT MISSING:"),
            "Log must contain greppable marker '[context] PINNED NOTE CONTENT MISSING:'; got {:?}",
            missing[0]
        );
        assert!(
            missing[0].contains("Ghost Note"),
            "Log should name the note"
        );
    }

    #[test]
    fn test_pinned_note_body_preferred_over_file() {
        // Verifies that when body is present, the or_else branch is never reached.
        // We do this by using a body value and an invalid file_path.
        let conn = setup_db();
        insert_project(&conn, "proj-13", "Priority App", None);
        insert_note(
            &conn,
            "note-13",
            "proj-13",
            "Priority Note",
            "/nonexistent/should-not-read.md", // would fail if reached
            Some("Body wins over file."),
        );
        pin_note(&conn, "note-13");

        let (ctx, missing) = build_pinned_notes_snippet(&conn, "proj-13");

        assert!(
            ctx.contains("Body wins over file."),
            "Body should appear; got {:?}",
            ctx
        );
        assert!(missing.is_empty(), "No MISSING logs when body present");
    }
}
