-- Server-side Playground conversation history.
-- Additive only: published migrations 0001-0009 are never modified.

CREATE TABLE IF NOT EXISTS playground_sessions (
    id TEXT PRIMARY KEY,
    title TEXT NOT NULL DEFAULT '',
    protocol TEXT NOT NULL CHECK (protocol IN ('chat', 'responses', 'anthropic')),
    model TEXT NOT NULL DEFAULT '',
    system TEXT NOT NULL DEFAULT '',
    stream INTEGER NOT NULL DEFAULT 1 CHECK (stream IN (0, 1)),
    max_tokens INTEGER NOT NULL DEFAULT 4096,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_playground_sessions_updated_at
    ON playground_sessions(updated_at DESC);

CREATE TABLE IF NOT EXISTS playground_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES playground_sessions(id) ON DELETE CASCADE,
    role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
    content TEXT NOT NULL DEFAULT '',
    reasoning TEXT NOT NULL DEFAULT '',
    tools_json TEXT NOT NULL DEFAULT '[]',
    status TEXT NOT NULL DEFAULT 'done'
        CHECK (status IN ('done', 'streaming', 'error', 'stopped')),
    error TEXT NOT NULL DEFAULT '',
    meta_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_playground_messages_session
    ON playground_messages(session_id, id);
