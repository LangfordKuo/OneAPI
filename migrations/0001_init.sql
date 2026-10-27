-- OneAPI initial schema (M0)

CREATE TABLE IF NOT EXISTS admin (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    admin_id INTEGER NOT NULL REFERENCES admin(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);

CREATE TABLE IF NOT EXISTS channels (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('openai_chat', 'openai_responses', 'anthropic')),
    base_url TEXT NOT NULL,
    api_key_enc TEXT NOT NULL,
    auth_header TEXT NOT NULL DEFAULT 'bearer' CHECK (auth_header IN ('bearer', 'x-api-key')),
    models TEXT NOT NULL DEFAULT '[]',
    model_mapping TEXT NOT NULL DEFAULT '{}',
    extra_headers TEXT NOT NULL DEFAULT '{}',
    proxy_url TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    weight INTEGER NOT NULL DEFAULT 1,
    timeout_ms INTEGER,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    last_error TEXT,
    last_error_at INTEGER,
    disabled_until INTEGER,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_channels_enabled ON channels(enabled);

CREATE TABLE IF NOT EXISTS api_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    key_prefix TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    expires_at INTEGER,
    last_used_at INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_api_keys_enabled ON api_keys(enabled);

CREATE TABLE IF NOT EXISTS usage_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    request_id TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'gateway' CHECK (source IN ('gateway', 'playground')),
    api_key_id INTEGER REFERENCES api_keys(id) ON DELETE SET NULL,
    channel_id INTEGER REFERENCES channels(id) ON DELETE SET NULL,
    model TEXT NOT NULL,
    upstream_model TEXT,
    inbound_protocol TEXT NOT NULL CHECK (inbound_protocol IN ('chat', 'responses', 'anthropic')),
    upstream_protocol TEXT NOT NULL CHECK (upstream_protocol IN ('chat', 'responses', 'anthropic')),
    stream INTEGER NOT NULL DEFAULT 0 CHECK (stream IN (0, 1)),
    status_code INTEGER NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 1,
    error_type TEXT,
    error_message TEXT,
    prompt_tokens INTEGER,
    completion_tokens INTEGER,
    total_tokens INTEGER,
    cached_tokens INTEGER,
    reasoning_tokens INTEGER,
    latency_ms INTEGER NOT NULL,
    first_token_ms INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_usage_logs_created_at ON usage_logs(created_at);
CREATE INDEX IF NOT EXISTS idx_usage_logs_model ON usage_logs(model);
CREATE INDEX IF NOT EXISTS idx_usage_logs_channel_id ON usage_logs(channel_id);
CREATE INDEX IF NOT EXISTS idx_usage_logs_api_key_id ON usage_logs(api_key_id);
CREATE INDEX IF NOT EXISTS idx_usage_logs_request_id ON usage_logs(request_id);

CREATE TABLE IF NOT EXISTS responses (
    id TEXT PRIMARY KEY,
    api_key_id INTEGER NOT NULL REFERENCES api_keys(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    request_json TEXT NOT NULL,
    response_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'completed',
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_responses_api_key_id ON responses(api_key_id);
CREATE INDEX IF NOT EXISTS idx_responses_created_at ON responses(created_at);

CREATE TABLE IF NOT EXISTS request_details (
    request_id TEXT PRIMARY KEY,
    request_body TEXT,
    response_body TEXT,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT OR IGNORE INTO settings (key, value) VALUES
    ('site_name', 'OneAPI'),
    ('default_max_tokens', '4096'),
    ('nonstream_timeout_seconds', '600'),
    ('stream_idle_timeout_seconds', '120'),
    ('usage_log_retention_days', '30'),
    ('response_retention_days', '7'),
    ('store_request_bodies', 'false');
