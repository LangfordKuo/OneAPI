-- Store the full API key encrypted with the master key so the admin can
-- display and copy it again. Rows created before this migration only have
-- irreversible hashes; their ciphertext stays NULL until the key is rotated.
ALTER TABLE api_keys ADD COLUMN key_ciphertext TEXT;
