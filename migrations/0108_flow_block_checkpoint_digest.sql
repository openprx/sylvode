-- Witness of the registry state at accepted_head: SHA-256 over the sorted (physical_cid, content_hash)
-- pairs of the block set the last complete reconcile registered. NULL means "unknown": the next
-- reconcile of the Page takes the full path. Existing rows stay NULL, so every Page is fully
-- reconciled once before an incremental reconcile can be used.
-- Re-running the file is a no-op.
ALTER TABLE flow_block_checkpoints
  ADD COLUMN IF NOT EXISTS registry_digest BYTEA
  CHECK (registry_digest IS NULL OR octet_length(registry_digest) = 32);
