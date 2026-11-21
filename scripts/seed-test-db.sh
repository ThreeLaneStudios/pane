#!/bin/bash
# Seed the test database with sample data
# Per spec §30 - Phase 3: Test Database Isolation

set -e

TEST_DB_DIR="$HOME/.pane-test"
TEST_DB="$TEST_DB_DIR/test.db"

echo "[TEST-SEED] Seeding test database at $TEST_DB"

# Create directory if it doesn't exist
mkdir -p "$TEST_DB_DIR"

# Remove existing test DB for fresh state
if [ -f "$TEST_DB" ]; then
    echo "[TEST-SEED] Removing existing test database"
    rm "$TEST_DB"
fi

# Create database with schema (using sqlite3)
echo "[TEST-SEED] Creating database schema..."

# Note: This assumes you have a schema.sql or migration files
# For now, we'll create the core tables manually based on the schema snapshot

sqlite3 "$TEST_DB" <<EOF
-- Core tables
CREATE TABLE users (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    email       TEXT,
    avatar_url  TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE projects (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    color       TEXT,
    parent_id   TEXT,
    sort_order  INTEGER NOT NULL DEFAULT 0,
    owner_id    TEXT,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (parent_id) REFERENCES projects(id) ON DELETE SET NULL,
    UNIQUE(name, parent_id)
);

CREATE TABLE notes (
    id              TEXT PRIMARY KEY,
    title           TEXT NOT NULL DEFAULT '',
    content_preview TEXT NOT NULL DEFAULT '',
    file_path       TEXT NOT NULL DEFAULT '',
    project_id      TEXT NOT NULL,
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
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE,
    FOREIGN KEY (folder_id) REFERENCES projects(id) ON DELETE SET NULL
);

CREATE TABLE ai_chat_messages (
    id                TEXT PRIMARY KEY,
    project_id        TEXT NOT NULL,
    role              TEXT NOT NULL,
    content           TEXT NOT NULL,
    context_note_id   TEXT,
    context_folder_id TEXT,
    context_mode      TEXT DEFAULT 'project',
    token_count       INTEGER,
    model             TEXT,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (project_id) REFERENCES projects(id)
);

CREATE TABLE ai_memory (
    id                TEXT PRIMARY KEY DEFAULT (hex(randomblob(16))),
    layer             TEXT NOT NULL,
    project_id        TEXT,
    user_id           TEXT,
    fact              TEXT NOT NULL,
    category          TEXT,
    source_message_id TEXT,
    is_active         INTEGER NOT NULL DEFAULT 1,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at        TEXT
);

-- Insert test user
INSERT INTO users (id, name, email) VALUES 
    ('test-user-1', 'Test User', 'test@example.com');

-- Insert 3 test projects
INSERT INTO projects (id, name, description, color, sort_order, owner_id) VALUES
    ('test-proj-a', 'Test Project A', 'Active test project', '#818CF8', 1, 'test-user-1'),
    ('test-proj-b', 'Test Project B', 'Another active project', '#F59E0B', 2, 'test-user-1'),
    ('test-proj-archived', 'Archived Project', 'Old archived project', '#6B7280', 3, 'test-user-1');

-- Insert 10 test notes with various statuses
INSERT INTO notes (id, title, content_preview, file_path, project_id, status, priority, sort_order, owner_id) VALUES
    ('note-1', 'Implement login flow', 'Need to build authentication', 'notes/note-1.md', 'test-proj-a', 'todo', 'high', 1, 'test-user-1'),
    ('note-2', 'Design landing page', 'Wireframes are ready', 'notes/note-2.md', 'test-proj-a', 'in-progress', 'medium', 2, 'test-user-1'),
    ('note-3', 'Write documentation', 'API docs need updating', 'notes/note-3.md', 'test-proj-a', 'todo', 'low', 3, 'test-user-1'),
    ('note-4', 'Fix bug #123', 'Critical production bug', 'notes/note-4.md', 'test-proj-a', 'in-progress', 'high', 4, 'test-user-1'),
    ('note-5', 'Setup CI/CD', 'Configure GitHub Actions', 'notes/note-5.md', 'test-proj-a', 'done', 'medium', 5, 'test-user-1'),
    ('note-6', 'Database migration', 'Add user_profiles table', 'notes/note-6.md', 'test-proj-b', 'done', 'high', 1, 'test-user-1'),
    ('note-7', 'Code review', 'Review PR #456', 'notes/note-7.md', 'test-proj-b', 'todo', NULL, 2, 'test-user-1'),
    ('note-8', 'Meeting notes', 'Weekly standup notes', 'notes/note-8.md', 'test-proj-b', NULL, NULL, 3, 'test-user-1'),
    ('note-9', 'Research task', 'Evaluate new framework', 'notes/note-9.md', 'test-proj-b', 'in-progress', 'low', 4, 'test-user-1'),
    ('note-10', 'Old task', 'Legacy task from last year', 'notes/note-10.md', 'test-proj-archived', 'done', NULL, 1, 'test-user-1');

-- Insert 20 chat messages across 2 threads (projects)
INSERT INTO ai_chat_messages (id, project_id, role, content, context_mode, model) VALUES
    ('msg-1', 'test-proj-a', 'user', 'Can you help me plan the login flow?', 'project', 'claude-sonnet-4'),
    ('msg-2', 'test-proj-a', 'assistant', 'I can help with that. Let me outline a secure authentication flow...', 'project', 'claude-sonnet-4'),
    ('msg-3', 'test-proj-a', 'user', 'What about password requirements?', 'project', 'claude-sonnet-4'),
    ('msg-4', 'test-proj-a', 'assistant', 'For security, I recommend: minimum 12 characters, mix of uppercase...', 'project', 'claude-sonnet-4'),
    ('msg-5', 'test-proj-a', 'user', 'Should we add 2FA?', 'project', 'claude-sonnet-4'),
    ('msg-6', 'test-proj-a', 'assistant', 'Yes, 2FA is highly recommended. TOTP is a good standard approach.', 'project', 'claude-sonnet-4'),
    ('msg-7', 'test-proj-a', 'user', 'How do we handle session management?', 'project', 'claude-sonnet-4'),
    ('msg-8', 'test-proj-a', 'assistant', 'Use JWT tokens with short expiry and refresh token rotation...', 'project', 'claude-sonnet-4'),
    ('msg-9', 'test-proj-a', 'user', 'What about password reset flow?', 'project', 'claude-sonnet-4'),
    ('msg-10', 'test-proj-a', 'assistant', 'Email-based reset with time-limited tokens is standard...', 'project', 'claude-sonnet-4'),
    ('msg-11', 'test-proj-b', 'user', 'I need to design the database schema', 'project', 'claude-sonnet-4'),
    ('msg-12', 'test-proj-b', 'assistant', 'Let me help you design a normalized schema...', 'project', 'claude-sonnet-4'),
    ('msg-13', 'test-proj-b', 'user', 'What indexes should I create?', 'project', 'claude-sonnet-4'),
    ('msg-14', 'test-proj-b', 'assistant', 'Index foreign keys and frequently queried columns...', 'project', 'claude-sonnet-4'),
    ('msg-15', 'test-proj-b', 'user', 'How should I handle migrations?', 'project', 'claude-sonnet-4'),
    ('msg-16', 'test-proj-b', 'assistant', 'Use versioned migration files with up/down scripts...', 'project', 'claude-sonnet-4'),
    ('msg-17', 'test-proj-b', 'user', 'Should I use an ORM?', 'project', 'claude-sonnet-4'),
    ('msg-18', 'test-proj-b', 'assistant', 'ORMs are convenient but add overhead. For Rust, consider sqlx...', 'project', 'claude-sonnet-4'),
    ('msg-19', 'test-proj-b', 'user', 'What about database backups?', 'project', 'claude-sonnet-4'),
    ('msg-20', 'test-proj-b', 'assistant', 'Implement automated daily backups with point-in-time recovery...', 'project', 'claude-sonnet-4');

-- Insert 5 memory entries
INSERT INTO ai_memory (id, layer, project_id, user_id, fact, category, is_active) VALUES
    ('mem-1', 'user', NULL, 'test-user-1', 'User prefers TypeScript over JavaScript', 'preference', 1),
    ('mem-2', 'project', 'test-proj-a', NULL, 'This project uses Tauri for desktop app framework', 'technical', 1),
    ('mem-3', 'project', 'test-proj-a', NULL, 'Authentication must support OAuth providers', 'requirement', 1),
    ('mem-4', 'user', NULL, 'test-user-1', 'User is in EST timezone', 'context', 1),
    ('mem-5', 'project', 'test-proj-b', NULL, 'Database uses SQLite with CR-SQLite for sync', 'technical', 1);

EOF

echo "[TEST-SEED] Test database seeded successfully"
echo "[TEST-SEED] Location: $TEST_DB"
echo "[TEST-SEED] Created: 1 user, 3 projects, 10 notes, 20 chat messages, 5 memory entries"
