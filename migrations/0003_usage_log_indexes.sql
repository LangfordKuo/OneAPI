-- Composite indexes for the most common log/dashboard filters.

CREATE INDEX IF NOT EXISTS idx_usage_logs_status_created
    ON usage_logs(status_code, created_at);
CREATE INDEX IF NOT EXISTS idx_usage_logs_source_created
    ON usage_logs(source, created_at);
CREATE INDEX IF NOT EXISTS idx_usage_logs_channel_created
    ON usage_logs(channel_id, created_at);
