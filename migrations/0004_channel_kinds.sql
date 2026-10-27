-- A channel may expose several upstream protocols. `kinds` holds the supported
-- set and `fallback_kind` picks the conversion target when the inbound protocol
-- is not supported. `kind` remains the primary kind for older clients.
ALTER TABLE channels ADD COLUMN kinds TEXT NOT NULL DEFAULT '[]';
ALTER TABLE channels ADD COLUMN fallback_kind TEXT;

UPDATE channels
SET kinds = CASE kind
    WHEN 'openai_responses' THEN '["openai_responses"]'
    WHEN 'anthropic' THEN '["anthropic"]'
    ELSE '["openai_chat"]'
END
WHERE kinds = '[]' OR kinds IS NULL;

UPDATE channels SET fallback_kind = kind WHERE fallback_kind IS NULL;
