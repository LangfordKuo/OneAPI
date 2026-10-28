-- Optional per-key restrictions. Empty arrays mean "no restriction".
ALTER TABLE api_keys ADD COLUMN allowed_channel_ids TEXT NOT NULL DEFAULT '[]';
ALTER TABLE api_keys ADD COLUMN allowed_models TEXT NOT NULL DEFAULT '[]';
ALTER TABLE api_keys ADD COLUMN token_limit INTEGER;
ALTER TABLE api_keys ADD COLUMN token_usage INTEGER NOT NULL DEFAULT 0;

-- Seed the counter with historical usage so limits apply to the key's lifetime.
UPDATE api_keys
SET token_usage = COALESCE(
    (SELECT SUM(usage_logs.total_tokens) FROM usage_logs WHERE usage_logs.api_key_id = api_keys.id),
    0
);
