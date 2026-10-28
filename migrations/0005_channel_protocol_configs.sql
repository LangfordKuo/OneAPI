-- Per-protocol upstream configuration. The legacy single-config columns remain
-- for backward compatibility; this column holds one entry per protocol with its
-- own Base URL, key, models and headers, plus its conversion target.
ALTER TABLE channels ADD COLUMN protocol_configs TEXT NOT NULL DEFAULT '[]';
