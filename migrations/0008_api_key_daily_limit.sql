-- Per-API-key daily token budget. 0/NULL means unlimited.
-- usage_day is a UTC day index (unix_ms / 86_400_000); counters roll over
-- lazily on the first usage write of a new day.
ALTER TABLE api_keys ADD COLUMN daily_token_limit INTEGER;
ALTER TABLE api_keys ADD COLUMN daily_token_usage INTEGER NOT NULL DEFAULT 0;
ALTER TABLE api_keys ADD COLUMN daily_usage_day INTEGER NOT NULL DEFAULT 0;

-- The dashboard's 24h failover count and retention cleanup both scan by time.
CREATE INDEX IF NOT EXISTS idx_usage_logs_created_at ON usage_logs(created_at);
