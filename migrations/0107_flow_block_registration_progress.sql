-- Bounded first registration: snapshot blocks still without an active placement at accepted_head.
-- Existing checkpoints were written by complete registrations, so 0 is correct for them.
-- Re-running the file is a no-op, so it is safe on a database that already applied 0001-0106
-- and on a fresh one.
ALTER TABLE flow_block_checkpoints
  ADD COLUMN IF NOT EXISTS pending_registrations INTEGER NOT NULL DEFAULT 0
  CHECK (pending_registrations >= 0);
