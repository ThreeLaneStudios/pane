-- Pane v61 Schema Fixture Builder
-- Creates a v61-stamped database for testing v61→v62 migration
-- This is the schema state BEFORE ADR-0017's chat_archive tables

-- === CORE CONTENT TABLES ===

CREATE TABLE projects (
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
    default_model         TEXT,
    smart_routing_enabled INTEGER,
    router_model          TEXT,
    simple_tier_model     TEXT
);

CREATE INDEX idx_projects_name ON projects(name);
CREATE INDEX idx_projects_parent ON projects(parent_id);
CREATE INDEX idx_projects_hidden ON projects(is_hidden) WHERE is_hidden = 1;

CREATE TABLE notes (
    id              TEXT NOT NULL PRIMARY KEY,
    title           TEXT NOT NULL DEFAULT '',
    body            TEXT,
    content_preview TEXT NOT NULL DEFAULT '',
    file_path       TEXT NOT NULL DEFAULT '',
    project_id      TEXT,
    folder_id       TEXT,
    status          TEXT,
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

CREATE INDEX idx_notes_project ON notes(project_id);
CREATE INDEX idx_notes_folder ON notes(folder_id);
CREATE INDEX idx_notes_status ON notes(status) WHERE status IS NOT NULL;
CREATE INDEX idx_notes_priority ON notes(priority) WHERE priority IS NOT NULL;
CREATE INDEX idx_notes_due_date ON notes(due_date) WHERE due_date IS NOT NULL;
CREATE INDEX idx_notes_updated ON notes(updated_at);
CREATE INDEX idx_notes_deleted ON notes(deleted_at) WHERE deleted_at IS NOT NULL;
CREATE INDEX idx_notes_agent_id ON notes(agent_id);
CREATE INDEX idx_notes_type ON notes(note_type);
CREATE INDEX idx_notes_vital ON notes(is_vital) WHERE is_vital = 1;

CREATE TABLE tags (
    id    TEXT PRIMARY KEY NOT NULL,
    name  TEXT NOT NULL DEFAULT '',
    color TEXT NOT NULL DEFAULT '#6B7280'
);

CREATE INDEX idx_tags_name ON tags(name);

CREATE TABLE note_tags (
    note_id TEXT NOT NULL DEFAULT '',
    tag_id  TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (note_id, tag_id)
);

CREATE INDEX idx_note_tags_note ON note_tags(note_id);
CREATE INDEX idx_note_tags_tag ON note_tags(tag_id);

CREATE TABLE note_links (
    source_note_id TEXT NOT NULL DEFAULT '',
    target_note_id TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (source_note_id, target_note_id)
);

CREATE INDEX idx_note_links_source ON note_links(source_note_id);
CREATE INDEX idx_note_links_target ON note_links(target_note_id);

-- === USERS & AUTH ===

CREATE TABLE users (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    email       TEXT,
    avatar_url  TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE user_profiles (
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

CREATE TABLE user_roles (
    id          TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    user_id     TEXT NOT NULL,
    project_id  TEXT,
    role        TEXT NOT NULL DEFAULT 'viewer',
    granted_by  TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(user_id, project_id)
);

CREATE INDEX idx_user_roles_user ON user_roles(user_id);
CREATE INDEX idx_user_roles_project ON user_roles(project_id);

CREATE TABLE role_model_permissions (
    role    TEXT NOT NULL,
    model   TEXT NOT NULL,
    allowed INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (role, model)
);

-- === AI CHAT (v61 — NO chat_archive tables) ===

CREATE TABLE chat_messages_local (
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

CREATE INDEX idx_chat_local_project ON chat_messages_local(project_id, created_at);

CREATE TABLE ai_context_markers (
    id                TEXT NOT NULL PRIMARY KEY,
    project_id        TEXT NOT NULL DEFAULT '',
    marker_type       TEXT NOT NULL DEFAULT '',
    label             TEXT NOT NULL DEFAULT '',
    target_note_id    TEXT,
    target_folder_id  TEXT,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    inserted_after    TEXT
);

CREATE TABLE ai_pinned_messages (
    id          TEXT NOT NULL PRIMARY KEY,
    project_id  TEXT NOT NULL DEFAULT '',
    message_id  TEXT NOT NULL DEFAULT '',
    pinned_at   TEXT NOT NULL DEFAULT (datetime('now')),
    sort_order  INTEGER DEFAULT 0
);

CREATE INDEX idx_ai_pinned_project_message ON ai_pinned_messages(project_id, message_id);

CREATE TABLE ai_conversation_summaries (
    id                TEXT NOT NULL PRIMARY KEY,
    project_id        TEXT NOT NULL DEFAULT '',
    summary_text      TEXT NOT NULL DEFAULT '',
    start_message_id  TEXT NOT NULL DEFAULT '',
    end_message_id    TEXT NOT NULL DEFAULT '',
    token_count       INTEGER,
    created_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

-- === AI MEMORY & AUDIT ===

CREATE TABLE ai_memory (
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

CREATE INDEX idx_memory_layer ON ai_memory(layer);
CREATE INDEX idx_memory_project ON ai_memory(project_id) WHERE project_id IS NOT NULL;
CREATE INDEX idx_memory_user ON ai_memory(user_id) WHERE user_id IS NOT NULL;

CREATE TABLE ai_actions (
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

CREATE INDEX idx_ai_actions_user ON ai_actions(user_id, created_at);
CREATE INDEX idx_ai_actions_project ON ai_actions(project_id, created_at);
CREATE INDEX idx_ai_actions_type ON ai_actions(action_type);
CREATE INDEX idx_ai_actions_target ON ai_actions(target_id);
CREATE INDEX idx_ai_actions_date ON ai_actions(created_at);

CREATE TABLE ai_settings (
    key   TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE ai_message_queue (
    id           TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    project_id   TEXT NOT NULL,
    content      TEXT NOT NULL,
    context_json TEXT NOT NULL,
    queued_at    TEXT NOT NULL DEFAULT (datetime('now')),
    sent_at      TEXT,
    status       TEXT NOT NULL DEFAULT 'queued'
);

-- === AI AGENTS & TASKS ===

CREATE TABLE ai_agents (
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

CREATE UNIQUE INDEX idx_ai_agents_main ON ai_agents(is_main) WHERE is_main = 1;
CREATE INDEX idx_ai_agents_slug ON ai_agents(slug);
CREATE INDEX idx_ai_agents_active ON ai_agents(is_active) WHERE is_active = 1;
CREATE INDEX idx_ai_agents_owner ON ai_agents(owner_id);
CREATE UNIQUE INDEX idx_ai_agents_oc_id_unique ON ai_agents(oc_agent_id);

CREATE TABLE pane_agent_tasks (
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

CREATE TABLE pane_task_workspaces (
    task_id         TEXT PRIMARY KEY,
    workspace_path  TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    cleaned_up      INTEGER NOT NULL DEFAULT 0
);

-- === AI PROCESSING ===

CREATE TABLE extraction_queue (
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

CREATE INDEX idx_extraction_pending ON extraction_queue(status, created_at) WHERE status IN ('pending', 'failed');

CREATE TABLE compaction_summaries (
    id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    thread_id       TEXT NOT NULL,
    summary         TEXT NOT NULL,
    message_count   INTEGER NOT NULL,
    compacted_before TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_compaction_thread ON compaction_summaries(thread_id, created_at DESC);

-- === DASHBOARD ===

CREATE TABLE user_dashboard_config (
    id                    TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
    onboarding_completed  INTEGER DEFAULT 0,
    onboarding_template   TEXT,
    greeting_name         TEXT,
    theme_variant         TEXT DEFAULT 'default',
    created_at            TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at            TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE dashboard_modules (
    id              TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
    module_type     TEXT NOT NULL DEFAULT '',
    sort_order      INTEGER NOT NULL DEFAULT 0,
    is_enabled      INTEGER NOT NULL DEFAULT 1,
    config_json     TEXT DEFAULT '{}',
    static_content  TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_modules_sort ON dashboard_modules(sort_order) WHERE is_enabled = 1;

CREATE TABLE module_items (
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

CREATE INDEX idx_items_module ON module_items(module_id, sort_order);

CREATE TABLE daily_entries (
    id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
    module_id   TEXT NOT NULL DEFAULT '',
    entry_date  TEXT NOT NULL DEFAULT '',
    data_json   TEXT NOT NULL DEFAULT '{}',
    completed_at TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_daily_date ON daily_entries(entry_date);
CREATE INDEX idx_daily_module_date ON daily_entries(module_id, entry_date);

CREATE TABLE habit_entries (
    id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
    item_id     TEXT NOT NULL DEFAULT '',
    entry_date  TEXT NOT NULL DEFAULT '',
    completed   INTEGER NOT NULL DEFAULT 0,
    completed_at TEXT
);

CREATE INDEX idx_habit_date ON habit_entries(item_id, entry_date);

CREATE TABLE reflection_entries (
    id              TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
    question_id     TEXT NOT NULL DEFAULT '',
    entry_date      TEXT NOT NULL DEFAULT '',
    value_numeric   REAL,
    value_bool      INTEGER,
    value_text      TEXT
);

CREATE INDEX idx_reflection_date ON reflection_entries(question_id, entry_date);

CREATE TABLE gratitude_entries (
    id          TEXT NOT NULL PRIMARY KEY DEFAULT (hex(randomblob(16))),
    entry_date  TEXT NOT NULL DEFAULT '',
    entry_index INTEGER NOT NULL DEFAULT 0,
    text        TEXT NOT NULL DEFAULT '',
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_gratitude_date ON gratitude_entries(entry_date);

-- === SYNC & VERSIONING ===

CREATE TABLE sync_state (
    id          TEXT NOT NULL PRIMARY KEY,
    device_id   TEXT NOT NULL DEFAULT '',
    device_name TEXT,
    platform    TEXT,
    last_sync_at TEXT,
    sync_version INTEGER NOT NULL DEFAULT 0,
    is_current  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE sync_changelog (
    id              TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    table_name      TEXT NOT NULL,
    row_id          TEXT NOT NULL,
    operation       TEXT NOT NULL,
    changeset_json  TEXT,
    device_id       TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_sync_changelog_time ON sync_changelog(created_at);

CREATE TABLE sync_queue (
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

CREATE INDEX idx_sync_queue_retry ON sync_queue(next_retry_at);

CREATE TABLE note_versions (
    id         TEXT NOT NULL PRIMARY KEY,
    note_id    TEXT NOT NULL DEFAULT '',
    content    TEXT NOT NULL DEFAULT '',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    source     TEXT NOT NULL DEFAULT 'manual'
);

CREATE INDEX idx_note_versions_note_id ON note_versions(note_id);

-- === ATTACHMENTS & SEARCH ===

CREATE TABLE attachments (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    original_filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE embeddings (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    embedding BLOB NOT NULL,
    content_hash TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_embeddings_source ON embeddings(source);

CREATE TABLE note_context_pins (
    note_id TEXT NOT NULL PRIMARY KEY,
    pinned_at TEXT NOT NULL DEFAULT (datetime('now')),
    pinned_by TEXT NOT NULL DEFAULT 'user'
);

CREATE INDEX idx_note_context_pins_pinned_at ON note_context_pins(pinned_at);

-- === SYSTEM TABLES ===

CREATE TABLE category_colors (
    category TEXT PRIMARY KEY,
    color TEXT NOT NULL
);

CREATE TABLE notifications (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL DEFAULT 'info',
    title TEXT NOT NULL,
    body TEXT NOT NULL DEFAULT '',
    source TEXT NOT NULL DEFAULT 'system',
    read INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE extensions (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL UNIQUE,
    version     TEXT NOT NULL,
    manifest    TEXT NOT NULL,
    enabled     INTEGER NOT NULL DEFAULT 1,
    settings    TEXT DEFAULT '{}',
    installed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- === SEED DATA ===

-- Set schema version to 61 (fixture purpose: test v61→v62 migration)
-- No seed projects/notes/chat/memory — fixture tests SCHEMA migration only.
-- Backfill logic has separate tests; seed data caused [CRITICAL] failures
-- when backfill tried to write Memory.md to non-existent workspace directories.
INSERT INTO ai_settings (key, value) VALUES ('schema_version', '61');

-- FTS tables intentionally omitted — they are corrupt/incompatible across SQLite versions
-- and cause "database disk image is malformed" errors. The app rebuilds FTS on every launch,
-- so shipping FTS state in a fixture is useless baggage.
