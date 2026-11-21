// Test utility to build v74 upgrade fixture (pre-launch, no migration arm)
//
// v74 adds the `templates` table (§32, ADR-0058, #193) and enables CRR on it.
// Because this is a pre-launch schema bump (no shipped users on v73), there is
// NO v73→v74 migration arm. The smoke-upgrade-db fixture must therefore be
// stamped at v74 so the app sees "Schema verified" instead of triggering the
// SCHEMA_MISMATCH fallthrough.
//
// Run with: cargo test --test build_v74_fixture -- --ignored --nocapture
// (must be run from src-tauri/ directory)

use rusqlite::{Connection, Result};
use std::path::PathBuf;

#[test]
#[ignore] // Run manually with --ignored flag when schema version bumps to v74
fn build_v74_fixture() -> Result<()> {
    let fixture_path = PathBuf::from("../tests/fixtures/upgrade-from-v73.db");
    let crsqlite_path = PathBuf::from("target/debug/resources/crsqlite.dylib");

    // Remove existing fixture so we start fresh
    if fixture_path.exists() {
        std::fs::remove_file(&fixture_path).unwrap();
        eprintln!("Removed existing fixture");
    }

    let conn = Connection::open(&fixture_path)?;

    // Load cr-sqlite extension (required for crsql_as_crr)
    unsafe {
        conn.load_extension(&crsqlite_path, None)
            .expect("Failed to load crsqlite.dylib — ensure 'cargo build' has run first");
    }
    eprintln!("✓ Loaded cr-sqlite extension");

    // ── Create all v74 tables ──────────────────────────────────────────────
    // These match init_schema() in migrations.rs exactly.
    // Mirrors build_v73_fixture.rs pattern with templates table added.

    create_all_tables(&conn)?;
    eprintln!("✓ Created all v74 tables");

    // ── Enable CRR on 23 tables (v74 adds templates) ─────────────────────
    let crr_tables = [
        "projects",
        "notes",
        "tags",
        "note_tags",
        "note_links",
        "ai_context_markers",
        "ai_pinned_messages",
        "ai_conversation_summaries",
        "ai_memory",
        "ai_actions",
        "user_profiles",
        "user_dashboard_config",
        "dashboard_modules",
        "module_items",
        "daily_entries",
        "habit_entries",
        "reflection_entries",
        "gratitude_entries",
        "sync_state",
        "note_versions",
        "attachments",
        "subtasks",
        "templates", // NEW in v74
    ];

    for table in &crr_tables {
        conn.query_row(
            &format!("SELECT crsql_as_crr('{}');", table),
            [],
            |_| Ok(()),
        )?;
        eprintln!("✓ CRR enabled: {}", table);
    }

    // ── Stamp schema version as 74 ────────────────────────────────────────
    conn.execute(
        "INSERT OR REPLACE INTO ai_settings (key, value) VALUES ('schema_version', '74')",
        [],
    )?;
    eprintln!("✓ Stamped schema_version = 74");

    // ── Insert minimal seed data (required by verify-upgrade-fixture.sh) ──
    conn.execute(
        "INSERT INTO projects (id, name, created_at, updated_at)
         VALUES ('seed-project-01', 'Smoke Test Project', datetime('now'), datetime('now'))",
        [],
    )?;
    conn.execute(
        "INSERT INTO notes (id, title, body, project_id, created_at, updated_at)
         VALUES ('seed-note-01', 'Smoke Test Note', 'Fixture seed data for upgrade smoke test.', 'seed-project-01', datetime('now'), datetime('now'))",
        [],
    )?;
    conn.execute(
        "INSERT INTO templates (id, name, content, checklist_json, created_at, updated_at)
         VALUES ('seed-tmpl-01', 'Smoke Test Template', '# Template Content', '[]', datetime('now'), datetime('now'))",
        [],
    )?;
    eprintln!("✓ Inserted seed project + note + template");

    // ── Verify ────────────────────────────────────────────────────────────
    let version: String = conn.query_row(
        "SELECT value FROM ai_settings WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;

    let project_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM projects", [], |row| row.get(0))?;

    let note_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM notes", [], |row| row.get(0))?;

    let template_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM templates", [], |row| row.get(0))?;

    let crr_clock_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE '%__crsql_clock'",
        [],
        |row| row.get(0),
    )?;

    let templates_exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='templates'",
        [],
        |row| row.get(0),
    )?;

    eprintln!("\n✓ v74 fixture created successfully!");
    eprintln!("  Path: {}", fixture_path.display());
    eprintln!("  Schema version: {}", version);
    eprintln!(
        "  Seed data: {} projects, {} notes, {} template(s)",
        project_count, note_count, template_count
    );
    eprintln!("  CRR clock tables: {}", crr_clock_count);
    eprintln!("  templates table: {}", templates_exists);

    assert_eq!(version, "74", "Fixture must be stamped at v74");
    assert!(project_count > 0, "Fixture must have seed project(s)");
    assert!(note_count > 0, "Fixture must have seed note(s)");
    assert_eq!(
        crr_clock_count, 23,
        "Should have 23 CRR clock tables (22 prior + templates)"
    );
    assert!(templates_exists, "templates table must exist in v74 fixture");

    Ok(())
}

// ── Table creation — mirrors init_schema() in migrations.rs ──────────────────
// Keep in sync with the canonical schema. When bumping SCHEMA_VERSION again,
// update this function to match the new init_schema().

fn create_all_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch("
        -- CORE CONTENT TABLES
        CREATE TABLE IF NOT EXISTS projects (
            id                    TEXT NOT NULL PRIMARY KEY,
            name                  TEXT NOT NULL DEFAULT '',
            description           TEXT,
            color                 TEXT,
            parent_id             TEXT,
            sort_order            INTEGER NOT NULL DEFAULT 0,
            owner_id              TEXT,
            created_at            TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at            TEXT NOT NULL DEFAULT (datetime('now')),
            category              TEXT,
            is_system             INTEGER NOT NULL DEFAULT 0,
            is_hidden             INTEGER NOT NULL DEFAULT 0,
            is_favorite           INTEGER NOT NULL DEFAULT 0,
            default_model         TEXT,
            smart_routing_enabled INTEGER,
            router_model          TEXT,
            simple_tier_model     TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name);
        CREATE INDEX IF NOT EXISTS idx_projects_parent ON projects(parent_id);
        CREATE INDEX IF NOT EXISTS idx_projects_hidden ON projects(is_hidden) WHERE is_hidden = 1;
        CREATE INDEX IF NOT EXISTS idx_projects_favorite ON projects(is_favorite) WHERE is_favorite = 1;

        CREATE TABLE IF NOT EXISTS notes (
            id              TEXT NOT NULL PRIMARY KEY,
            title           TEXT NOT NULL DEFAULT '',
            body            TEXT,
            content_preview TEXT NOT NULL DEFAULT '',
            file_path       TEXT NOT NULL DEFAULT '',
            project_id      TEXT,
            folder_id       TEXT,
            status          TEXT NOT NULL DEFAULT 'cabinet',
            priority        TEXT,
            due_date        TEXT,
            assignee        TEXT,
            completed_at    TEXT,
            sort_order      INTEGER NOT NULL DEFAULT 0,
            word_count      INTEGER NOT NULL DEFAULT 0,
            pinned          INTEGER NOT NULL DEFAULT 0,
            owner_id        TEXT,
            tags_json       TEXT NOT NULL DEFAULT '[]',
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
            deleted_at      TEXT,
            parent_id       TEXT,
            note_type       TEXT,
            is_vital        INTEGER NOT NULL DEFAULT 0,
            agent_id        TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_notes_project ON notes(project_id);
        CREATE INDEX IF NOT EXISTS idx_notes_folder ON notes(folder_id);
        CREATE INDEX IF NOT EXISTS idx_notes_status ON notes(status) WHERE status != 'cabinet';
        CREATE INDEX IF NOT EXISTS idx_notes_priority ON notes(priority) WHERE priority IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_due_date ON notes(due_date) WHERE due_date IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_updated ON notes(updated_at);
        CREATE INDEX IF NOT EXISTS idx_notes_deleted ON notes(deleted_at) WHERE deleted_at IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notes_agent_id ON notes(agent_id);
        CREATE INDEX IF NOT EXISTS idx_notes_type ON notes(note_type);
        CREATE INDEX IF NOT EXISTS idx_notes_vital ON notes(is_vital) WHERE is_vital = 1;

        CREATE TABLE IF NOT EXISTS tags (
            id    TEXT PRIMARY KEY NOT NULL,
            name  TEXT NOT NULL DEFAULT '',
            color TEXT NOT NULL DEFAULT '#6B7280'
        );
        CREATE INDEX IF NOT EXISTS idx_tags_name ON tags(name);

        CREATE TABLE IF NOT EXISTS note_tags (
            note_id TEXT NOT NULL DEFAULT '',
            tag_id  TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (note_id, tag_id)
        );
        CREATE INDEX IF NOT EXISTS idx_note_tags_note ON note_tags(note_id);
        CREATE INDEX IF NOT EXISTS idx_note_tags_tag ON note_tags(tag_id);

        CREATE TABLE IF NOT EXISTS note_links (
            source_note_id TEXT NOT NULL DEFAULT '',
            target_note_id TEXT NOT NULL DEFAULT '',
            PRIMARY KEY (source_note_id, target_note_id)
        );
        CREATE INDEX IF NOT EXISTS idx_note_links_source ON note_links(source_note_id);
        CREATE INDEX IF NOT EXISTS idx_note_links_target ON note_links(target_note_id);

        -- USERS & AUTH
        CREATE TABLE IF NOT EXISTS users (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            email       TEXT,
            avatar_url  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS user_profiles (
            id                      TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            user_id                 TEXT NOT NULL DEFAULT '',
            display_name            TEXT,
            bio                     TEXT,
            working_style           TEXT,
            ai_provider             TEXT,
            ai_model                TEXT,
            onboarding_method       TEXT,
            onboarding_completed    INTEGER NOT NULL DEFAULT 0,
            profile_json            TEXT DEFAULT '{}',
            created_at              TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at              TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS user_roles (
            id          TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            user_id     TEXT NOT NULL,
            project_id  TEXT,
            role        TEXT NOT NULL DEFAULT 'viewer',
            granted_by  TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            UNIQUE(user_id, project_id)
        );
        CREATE INDEX IF NOT EXISTS idx_user_roles_user ON user_roles(user_id);
        CREATE INDEX IF NOT EXISTS idx_user_roles_project ON user_roles(project_id);

        CREATE TABLE IF NOT EXISTS role_model_permissions (
            role    TEXT NOT NULL,
            model   TEXT NOT NULL,
            allowed INTEGER NOT NULL DEFAULT 1,
            PRIMARY KEY (role, model)
        );

        -- AI CHAT
        CREATE TABLE IF NOT EXISTS chat_messages_local (
            id          TEXT NOT NULL PRIMARY KEY,
            project_id  TEXT NOT NULL DEFAULT '',
            role        TEXT NOT NULL DEFAULT 'user',
            content     TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            is_pinned   INTEGER NOT NULL DEFAULT 0,
            attachment_ids TEXT,
            tool_calls  TEXT,
            tool_results TEXT,
            model       TEXT,
            token_count INTEGER,
            thread_id   TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_chat_local_project ON chat_messages_local(project_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_chat_local_thread ON chat_messages_local(thread_id, created_at);

        CREATE TABLE IF NOT EXISTS ai_context_markers (
            id                TEXT NOT NULL PRIMARY KEY,
            project_id        TEXT NOT NULL DEFAULT '',
            marker_type       TEXT NOT NULL DEFAULT '',
            label             TEXT NOT NULL DEFAULT '',
            target_note_id    TEXT,
            target_folder_id  TEXT,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            inserted_after    TEXT
        );

        CREATE TABLE IF NOT EXISTS ai_pinned_messages (
            id          TEXT NOT NULL PRIMARY KEY,
            project_id  TEXT NOT NULL DEFAULT '',
            message_id  TEXT NOT NULL DEFAULT '',
            pinned_at   TEXT NOT NULL DEFAULT (datetime('now')),
            sort_order  INTEGER DEFAULT 0
        );
        CREATE INDEX IF NOT EXISTS idx_ai_pinned_project_message ON ai_pinned_messages(project_id, message_id);

        CREATE TABLE IF NOT EXISTS ai_conversation_summaries (
            id                TEXT NOT NULL PRIMARY KEY,
            project_id        TEXT NOT NULL DEFAULT '',
            summary_text      TEXT NOT NULL DEFAULT '',
            start_message_id  TEXT NOT NULL DEFAULT '',
            end_message_id    TEXT NOT NULL DEFAULT '',
            token_count       INTEGER,
            created_at        TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS chat_archive (
            id          TEXT PRIMARY KEY,
            thread_id   TEXT NOT NULL,
            project_id  TEXT,
            role        TEXT NOT NULL,
            content     TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            metadata    TEXT,
            archived_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_chat_archive_project ON chat_archive(project_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_chat_archive_thread ON chat_archive(thread_id);

        CREATE VIRTUAL TABLE IF NOT EXISTS chat_archive_fts USING fts5(
            content,
            content_rowid='rowid',
            tokenize='porter unicode61'
        );

        CREATE TABLE IF NOT EXISTS chat_sessions (
            id             TEXT NOT NULL PRIMARY KEY,
            project_id     TEXT NOT NULL,
            title          TEXT NOT NULL DEFAULT '',
            origin         TEXT NOT NULL DEFAULT 'user',
            created_at     TEXT NOT NULL DEFAULT (datetime('now')),
            last_active_at TEXT NOT NULL DEFAULT (datetime('now')),
            hidden_at      TEXT,
            is_active      INTEGER NOT NULL DEFAULT 0
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_active_per_surface
            ON chat_sessions(project_id) WHERE is_active = 1;
        CREATE INDEX IF NOT EXISTS idx_sessions_surface
            ON chat_sessions(project_id, last_active_at DESC);

        -- AI MEMORY & AUDIT
        CREATE TABLE IF NOT EXISTS ai_memory (
            id                TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            layer             TEXT NOT NULL DEFAULT '',
            project_id        TEXT,
            user_id           TEXT,
            fact              TEXT NOT NULL DEFAULT '',
            category          TEXT,
            source_message_id TEXT,
            is_active         INTEGER NOT NULL DEFAULT 1,
            created_at        TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at        TEXT,
            last_referenced   TEXT,
            routed_at         TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_memory_layer ON ai_memory(layer);
        CREATE INDEX IF NOT EXISTS idx_memory_project ON ai_memory(project_id) WHERE project_id IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_memory_user ON ai_memory(user_id) WHERE user_id IS NOT NULL;

        CREATE TABLE IF NOT EXISTS ai_actions (
            id              TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            user_id         TEXT NOT NULL DEFAULT '',
            action_type     TEXT NOT NULL DEFAULT '',
            target_id       TEXT,
            target_type     TEXT,
            details_json    TEXT NOT NULL DEFAULT '{}',
            project_id      TEXT,
            model           TEXT,
            message_id      TEXT,
            status          TEXT NOT NULL DEFAULT 'success',
            error_message   TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_ai_actions_user ON ai_actions(user_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_ai_actions_project ON ai_actions(project_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_ai_actions_type ON ai_actions(action_type);
        CREATE INDEX IF NOT EXISTS idx_ai_actions_target ON ai_actions(target_id);
        CREATE INDEX IF NOT EXISTS idx_ai_actions_date ON ai_actions(created_at);

        CREATE TABLE IF NOT EXISTS ai_settings (
            key   TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS ai_message_queue (
            id           TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            project_id   TEXT NOT NULL,
            content      TEXT NOT NULL,
            context_json TEXT NOT NULL,
            queued_at    TEXT NOT NULL DEFAULT (datetime('now')),
            sent_at      TEXT,
            status       TEXT NOT NULL DEFAULT 'queued'
        );

        -- AI AGENTS & TASKS
        CREATE TABLE IF NOT EXISTS ai_agents (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            name            TEXT NOT NULL,
            slug            TEXT NOT NULL UNIQUE,
            role            TEXT NOT NULL DEFAULT 'sub',
            description     TEXT,
            model           TEXT,
            is_main         INTEGER NOT NULL DEFAULT 0,
            is_active       INTEGER NOT NULL DEFAULT 1,
            workspace_path  TEXT NOT NULL,
            soul_hash       TEXT,
            memory_hash     TEXT,
            config_json     TEXT DEFAULT '{}',
            owner_id        TEXT,
            source          TEXT DEFAULT 'import',
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
            oc_agent_id     TEXT
        );
        CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_main ON ai_agents(is_main) WHERE is_main = 1;
        CREATE INDEX IF NOT EXISTS idx_ai_agents_slug ON ai_agents(slug);
        CREATE INDEX IF NOT EXISTS idx_ai_agents_active ON ai_agents(is_active) WHERE is_active = 1;
        CREATE INDEX IF NOT EXISTS idx_ai_agents_owner ON ai_agents(owner_id);
        CREATE UNIQUE INDEX IF NOT EXISTS idx_ai_agents_oc_id_unique ON ai_agents(oc_agent_id);

        CREATE TABLE IF NOT EXISTS pane_agent_tasks (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            parent_message_id TEXT,
            task_description TEXT NOT NULL,
            model           TEXT,
            status          TEXT NOT NULL DEFAULT 'spawned',
            progress        TEXT,
            result_note_id  TEXT,
            result_summary  TEXT,
            tokens_used     INTEGER,
            cost_estimate   REAL,
            tier            TEXT NOT NULL,
            started_at      TEXT NOT NULL DEFAULT (datetime('now')),
            completed_at    TEXT,
            error           TEXT
        );

        CREATE TABLE IF NOT EXISTS pane_task_workspaces (
            task_id         TEXT PRIMARY KEY,
            workspace_path  TEXT NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            cleaned_up      INTEGER NOT NULL DEFAULT 0
        );

        -- AI PROCESSING
        CREATE TABLE IF NOT EXISTS extraction_queue (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            thread_id       TEXT NOT NULL,
            message_id      TEXT NOT NULL,
            user_message_id TEXT,
            status          TEXT NOT NULL DEFAULT 'pending',
            attempts        INTEGER NOT NULL DEFAULT 0,
            max_attempts    INTEGER NOT NULL DEFAULT 3,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            processed_at    TEXT,
            error           TEXT,
            UNIQUE(message_id)
        );
        CREATE INDEX IF NOT EXISTS idx_extraction_pending ON extraction_queue(status, created_at) WHERE status IN ('pending', 'failed');

        CREATE TABLE IF NOT EXISTS compaction_summaries (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            thread_id       TEXT NOT NULL,
            summary         TEXT NOT NULL,
            message_count   INTEGER NOT NULL,
            compacted_before TEXT NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_compaction_thread ON compaction_summaries(thread_id, created_at DESC);

        -- DASHBOARD
        CREATE TABLE IF NOT EXISTS user_dashboard_config (
            id                    TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            onboarding_completed  INTEGER DEFAULT 0,
            onboarding_template   TEXT,
            theme_variant         TEXT DEFAULT 'default',
            created_at            TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS dashboard_modules (
            id              TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            module_type     TEXT NOT NULL DEFAULT '',
            sort_order      INTEGER NOT NULL DEFAULT 0,
            is_enabled      INTEGER NOT NULL DEFAULT 1,
            config_json     TEXT DEFAULT '{}',
            static_content  TEXT,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_modules_sort ON dashboard_modules(sort_order) WHERE is_enabled = 1;

        CREATE TABLE IF NOT EXISTS module_items (
            id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            module_id   TEXT NOT NULL DEFAULT '',
            item_type   TEXT NOT NULL DEFAULT '',
            label       TEXT NOT NULL DEFAULT '',
            description TEXT,
            emoji       TEXT,
            config_json TEXT DEFAULT '{}',
            sort_order  INTEGER NOT NULL DEFAULT 0,
            is_active   INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_items_module ON module_items(module_id, sort_order);

        CREATE TABLE IF NOT EXISTS daily_entries (
            id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            module_id   TEXT NOT NULL DEFAULT '',
            entry_date  TEXT NOT NULL DEFAULT '',
            data_json   TEXT NOT NULL DEFAULT '{}',
            completed_at TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_daily_date ON daily_entries(entry_date);
        CREATE INDEX IF NOT EXISTS idx_daily_module_date ON daily_entries(module_id, entry_date);

        CREATE TABLE IF NOT EXISTS habit_entries (
            id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            item_id     TEXT NOT NULL DEFAULT '',
            entry_date  TEXT NOT NULL DEFAULT '',
            completed   INTEGER NOT NULL DEFAULT 0,
            completed_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_habit_date ON habit_entries(item_id, entry_date);

        CREATE TABLE IF NOT EXISTS reflection_entries (
            id              TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            question_id     TEXT NOT NULL DEFAULT '',
            entry_date      TEXT NOT NULL DEFAULT '',
            value_numeric   REAL,
            value_bool      INTEGER,
            value_text      TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_reflection_date ON reflection_entries(question_id, entry_date);

        CREATE TABLE IF NOT EXISTS gratitude_entries (
            id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
            entry_date  TEXT NOT NULL DEFAULT '',
            entry_index INTEGER NOT NULL DEFAULT 0,
            text        TEXT NOT NULL DEFAULT '',
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_gratitude_date ON gratitude_entries(entry_date);

        -- SYNC & VERSIONING
        CREATE TABLE IF NOT EXISTS sync_state (
            id          TEXT NOT NULL PRIMARY KEY,
            device_id   TEXT NOT NULL DEFAULT '',
            device_name TEXT,
            platform    TEXT,
            last_sync_at TEXT,
            sync_version INTEGER NOT NULL DEFAULT 0,
            is_current  INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS sync_changelog (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            table_name      TEXT NOT NULL,
            row_id          TEXT NOT NULL,
            operation       TEXT NOT NULL,
            changeset_json  TEXT,
            device_id       TEXT NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_sync_changelog_time ON sync_changelog(created_at);

        CREATE TABLE IF NOT EXISTS sync_queue (
            id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
            agent           TEXT NOT NULL,
            filename        TEXT NOT NULL,
            content         TEXT NOT NULL,
            checksum        TEXT NOT NULL,
            created_at      TEXT NOT NULL DEFAULT (datetime('now')),
            retry_count     INTEGER DEFAULT 0,
            next_retry_at   TEXT,
            UNIQUE(agent, filename, checksum)
        );
        CREATE INDEX IF NOT EXISTS idx_sync_queue_retry ON sync_queue(next_retry_at);

        CREATE TABLE IF NOT EXISTS note_versions (
            id         TEXT NOT NULL PRIMARY KEY,
            note_id    TEXT NOT NULL DEFAULT '',
            content    TEXT NOT NULL DEFAULT '',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            source     TEXT NOT NULL DEFAULT 'manual'
        );
        CREATE INDEX IF NOT EXISTS idx_note_versions_note_id ON note_versions(note_id);

        -- ATTACHMENTS & SEARCH
        CREATE TABLE IF NOT EXISTS attachments (
            id                TEXT PRIMARY KEY NOT NULL,
            project_id        TEXT NOT NULL DEFAULT '',
            project_name      TEXT NOT NULL DEFAULT '',
            original_filename TEXT NOT NULL DEFAULT '',
            mime_type         TEXT NOT NULL DEFAULT '',
            size_bytes        INTEGER NOT NULL DEFAULT 0,
            created_at        TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS embeddings (
            id TEXT PRIMARY KEY,
            source TEXT NOT NULL,
            embedding BLOB NOT NULL,
            content_hash TEXT NOT NULL,
            engine TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_embeddings_source ON embeddings(source);

        CREATE TABLE IF NOT EXISTS note_context_pins (
            note_id TEXT NOT NULL PRIMARY KEY,
            pinned_at TEXT NOT NULL DEFAULT (datetime('now')),
            pinned_by TEXT NOT NULL DEFAULT 'user'
        );
        CREATE INDEX IF NOT EXISTS idx_note_context_pins_pinned_at ON note_context_pins(pinned_at);

        -- SUBTASKS (§1.9, ADR-0057, #2092) — added in v73
        CREATE TABLE IF NOT EXISTS subtasks (
            id           TEXT NOT NULL PRIMARY KEY,
            note_id      TEXT NOT NULL DEFAULT '',
            text         TEXT NOT NULL DEFAULT '',
            due_date     TEXT,
            done         INTEGER NOT NULL DEFAULT 0,
            completed_at TEXT,
            sort_order   INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_subtasks_note ON subtasks(note_id);
        CREATE INDEX IF NOT EXISTS idx_subtasks_due_date ON subtasks(due_date) WHERE due_date IS NOT NULL;

        -- TEMPLATES (§32, ADR-0058, #193) — NEW IN v74
        CREATE TABLE IF NOT EXISTS templates (
            id             TEXT NOT NULL PRIMARY KEY,
            name           TEXT NOT NULL DEFAULT '',
            content        TEXT NOT NULL DEFAULT '',
            checklist_json TEXT NOT NULL DEFAULT '[]',
            created_at     TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at     TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE INDEX IF NOT EXISTS idx_templates_name ON templates(name);

        -- OG CACHE (#1780, ADR-0049)
        CREATE TABLE IF NOT EXISTS og_cache (
            url             TEXT PRIMARY KEY NOT NULL,
            og_title        TEXT,
            og_description  TEXT,
            og_image        TEXT,
            fetched_at      INTEGER NOT NULL DEFAULT 0,
            status          TEXT NOT NULL DEFAULT 'ok'
        );

        -- SYSTEM TABLES
        CREATE TABLE IF NOT EXISTS category_colors (
            category TEXT PRIMARY KEY,
            color TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS notifications (
            id           TEXT PRIMARY KEY,
            type         TEXT NOT NULL DEFAULT 'info',
            title        TEXT NOT NULL,
            body         TEXT NOT NULL DEFAULT '',
            source       TEXT NOT NULL DEFAULT 'system',
            read         INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now')),
            dismissed_at TEXT,
            expires_at   TEXT,
            target_kind  TEXT,
            target_id    TEXT,
            idem_key     TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_notifications_active
            ON notifications (created_at DESC) WHERE dismissed_at IS NULL;
        CREATE INDEX IF NOT EXISTS idx_notifications_idem
            ON notifications (idem_key) WHERE idem_key IS NOT NULL;
        CREATE INDEX IF NOT EXISTS idx_notifications_expires
            ON notifications (expires_at);

        CREATE TABLE IF NOT EXISTS extensions (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL UNIQUE,
            version     TEXT NOT NULL,
            manifest    TEXT NOT NULL,
            enabled     INTEGER NOT NULL DEFAULT 1,
            settings    TEXT DEFAULT '{}',
            installed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- MIGRATION FLAGS
        CREATE TABLE IF NOT EXISTS migration_flags (
            flag         TEXT PRIMARY KEY NOT NULL,
            completed_at TEXT NOT NULL DEFAULT (datetime('now'))
        );

        -- FTS5 VIRTUAL TABLES (triggers created after CRR)
        CREATE VIRTUAL TABLE IF NOT EXISTS notes_fts USING fts5(
            title,
            body,
            content='notes',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS ai_memory_fts USING fts5(
            category,
            fact,
            content='ai_memory',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS chat_fts USING fts5(
            content,
            content='chat_messages_local',
            content_rowid='rowid',
            tokenize='porter unicode61'
        );
    ")?;

    Ok(())
}
