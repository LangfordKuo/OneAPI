-- Record Anthropic cache_creation_input_tokens separately from cache reads.

ALTER TABLE usage_logs ADD COLUMN cache_creation_tokens INTEGER;
