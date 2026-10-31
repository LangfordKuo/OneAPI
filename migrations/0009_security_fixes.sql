-- Security hardening batch (M5-M8, M10-M14, L1-L11).
-- Additive only: published migrations 0001-0008 are never modified.

-- L5: Remember the API key that owned a request detail row at write time.
-- usage_logs.api_key_id becomes NULL when a key is deleted; this column keeps
-- the original owner so details never fall back to another key's rows.
ALTER TABLE usage_logs ADD COLUMN detail_api_key_id INTEGER;
UPDATE usage_logs SET detail_api_key_id = api_key_id WHERE detail_api_key_id IS NULL;

-- L10: Peppered API key hash. The legacy SHA-256 column is kept so existing
-- keys keep working and an older binary can still authenticate after rollback.
ALTER TABLE api_keys ADD COLUMN key_hash_v2 TEXT;
CREATE UNIQUE INDEX IF NOT EXISTS idx_api_keys_key_hash_v2
    ON api_keys(key_hash_v2);

-- M8: Admission-time quota reservation. Rows are settled by the usage write
-- transaction, cleared at startup, and expired by the janitor.
ALTER TABLE api_keys ADD COLUMN reserved_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN reserved_daily_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN reserved_day INTEGER NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN reserved_at INTEGER;

-- M10: Serve /v1/models' recent-model list per API key.
CREATE INDEX IF NOT EXISTS idx_usage_logs_api_key_created
    ON usage_logs(api_key_id, created_at);

-- M6: Per-key and global Responses storage accounting.
CREATE TABLE IF NOT EXISTS response_usage (
    api_key_id INTEGER PRIMARY KEY REFERENCES api_keys(id) ON DELETE CASCADE,
    bytes INTEGER NOT NULL DEFAULT 0,
    rows INTEGER NOT NULL DEFAULT 0
);
-- Sizes are bytes, matching the Rust-side quota checks: `length()` counts
-- characters for TEXT, so each column is cast to BLOB before measuring.
INSERT OR IGNORE INTO response_usage (api_key_id, bytes, rows)
SELECT api_key_id,
       SUM(length(CAST(COALESCE(request_json, '') AS BLOB))
           + length(CAST(COALESCE(response_json, '') AS BLOB))),
       COUNT(*)
FROM responses
GROUP BY api_key_id;
CREATE INDEX IF NOT EXISTS idx_responses_created_at
    ON responses(created_at);

-- L4: Deleting an API key removes its stored request/response bodies immediately.
CREATE TRIGGER IF NOT EXISTS trg_api_keys_delete_request_details
AFTER DELETE ON api_keys
BEGIN
    DELETE FROM request_details WHERE api_key_id = OLD.id;
END;

-- L11: Reject invalid JSON written outside the application validators.
CREATE TRIGGER IF NOT EXISTS trg_api_keys_json_insert
BEFORE INSERT ON api_keys
WHEN json_valid(NEW.allowed_channel_ids) = 0 OR json_valid(NEW.allowed_models) = 0
BEGIN
    SELECT RAISE(ABORT, 'invalid api_key json');
END;

CREATE TRIGGER IF NOT EXISTS trg_api_keys_json_update
BEFORE UPDATE OF allowed_channel_ids, allowed_models ON api_keys
WHEN json_valid(NEW.allowed_channel_ids) = 0 OR json_valid(NEW.allowed_models) = 0
BEGIN
    SELECT RAISE(ABORT, 'invalid api_key json');
END;

CREATE TRIGGER IF NOT EXISTS trg_channels_json_insert
BEFORE INSERT ON channels
WHEN json_valid(NEW.models) = 0
  OR json_valid(NEW.model_mapping) = 0
  OR json_valid(NEW.extra_headers) = 0
  OR json_valid(NEW.kinds) = 0
  OR json_valid(NEW.protocol_configs) = 0
BEGIN
    SELECT RAISE(ABORT, 'invalid channel json');
END;

CREATE TRIGGER IF NOT EXISTS trg_channels_json_update
BEFORE UPDATE OF models, model_mapping, extra_headers, kinds, protocol_configs ON channels
WHEN json_valid(NEW.models) = 0
  OR json_valid(NEW.model_mapping) = 0
  OR json_valid(NEW.extra_headers) = 0
  OR json_valid(NEW.kinds) = 0
  OR json_valid(NEW.protocol_configs) = 0
BEGIN
    SELECT RAISE(ABORT, 'invalid channel json');
END;
