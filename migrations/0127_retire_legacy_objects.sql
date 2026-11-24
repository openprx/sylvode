-- Retire objects that only served the legacy comment-anchor seal, the legacy `pages` import and the
-- retired plugin layout protocol. Forward-only: applied migrations stay as they are.
--
-- Every statement is safe to run again: the file may execute on a database whose ledger has no row
-- for it.

-- Comment anchors are bound when a comment is created; nothing is sealed afterwards.
ALTER TABLE flow_block_checkpoints DROP COLUMN IF EXISTS legacy_anchors_sealed;
