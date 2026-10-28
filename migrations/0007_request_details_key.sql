-- Scope stored request/response bodies per API key. Two clients that reuse the
-- same client-supplied request id must not overwrite each other's details.
-- `api_key_id = 0` represents playground traffic (which has no key) and rows
-- migrated from the previous unscoped layout.
CREATE TABLE request_details_new (
    request_id TEXT NOT NULL,
    api_key_id INTEGER NOT NULL DEFAULT 0,
    request_body TEXT,
    response_body TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (request_id, api_key_id)
);

INSERT INTO request_details_new
    (request_id, api_key_id, request_body, response_body, created_at)
SELECT request_id, 0, request_body, response_body, created_at
FROM request_details;

DROP TABLE request_details;

ALTER TABLE request_details_new RENAME TO request_details;

-- The retention cleanup deletes by age; without this index the delete scans
-- the whole table while holding the SQLite write lock.
CREATE INDEX IF NOT EXISTS idx_request_details_created_at
    ON request_details(created_at);
