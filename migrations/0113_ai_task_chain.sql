-- 0113: the chain an AI task belongs to.
--
-- A task a human or an inbound alert starts is the root of a chain at depth 0. A task a bot's
-- action causes while it runs a task is a child of that task, one deeper. The dispatch guard
-- refuses a hop beyond [ai_tasks] max_chain_depth and a bot that is already in the chain for the
-- same work item. Existing rows are their own root at depth 0.
--
-- Re-runs on every adopted database (it is past MIGRATION_ADOPTION_CUTOFF), so every object is
-- created only when missing. Adding nullable columns and a constant default does not rewrite
-- ai_tasks; ALTER TABLE holds ACCESS EXCLUSIVE for catalogue work only. A lock timeout makes the
-- file fail without effect behind a long writer; the next start retries it.

SET LOCAL lock_timeout = '10s';

ALTER TABLE ai_tasks
  ADD COLUMN IF NOT EXISTS chain_root_id   UUID,
  ADD COLUMN IF NOT EXISTS chain_parent_id UUID,
  ADD COLUMN IF NOT EXISTS chain_depth     INTEGER NOT NULL DEFAULT 0;

-- Rows from before this file: their own root. Rows the guard inserts always carry a root.
UPDATE ai_tasks SET chain_root_id = id WHERE chain_root_id IS NULL;

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'ai_tasks'::regclass AND conname = 'ai_tasks_chain_depth_check') THEN
    ALTER TABLE ai_tasks ADD CONSTRAINT ai_tasks_chain_depth_check
      CHECK (chain_depth >= 0) NOT VALID;
  END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_ai_tasks_chain_root ON ai_tasks (chain_root_id);
