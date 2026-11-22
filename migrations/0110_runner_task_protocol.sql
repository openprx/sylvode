-- 0110: AI task acknowledge and lease, runner phase.
--
-- Re-runs on every adopted database (it is past MIGRATION_ADOPTION_CUTOFF), so every object is
-- created only when missing. Adding nullable columns without a default and NOT VALID checks does
-- not rewrite or scan ai_tasks; ALTER TABLE holds ACCESS EXCLUSIVE for catalogue work only. A lock
-- timeout makes the file fail without effect behind a long writer; the next start retries it.
-- Rollback: docs/runbooks/runner-task-protocol.md.

SET LOCAL lock_timeout = '10s';

ALTER TABLE ai_tasks
  ADD COLUMN IF NOT EXISTS acknowledged_at  TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS plan             TEXT,
  ADD COLUMN IF NOT EXISTS last_progress_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS lease_expires_at TIMESTAMPTZ,
  ADD COLUMN IF NOT EXISTS runner_phase     TEXT;

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'ai_tasks'::regclass AND conname = 'ai_tasks_runner_phase_check') THEN
    ALTER TABLE ai_tasks ADD CONSTRAINT ai_tasks_runner_phase_check
      CHECK (runner_phase IS NULL OR runner_phase IN ('queued', 'pushed', 'blocked')) NOT VALID;
  END IF;
  -- Documented status set. NOT VALID so a historical value cannot fail the migration
  -- ('retried' is still counted by release_readiness.rs:258).
  IF NOT EXISTS (SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'ai_tasks'::regclass AND conname = 'ai_tasks_status_check') THEN
    ALTER TABLE ai_tasks ADD CONSTRAINT ai_tasks_status_check
      CHECK (status IN ('pending', 'processing', 'acknowledged', 'completed', 'failed', 'cancelled', 'retried')) NOT VALID;
  END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_ai_tasks_active_lease
  ON ai_tasks (lease_expires_at) WHERE status IN ('processing', 'acknowledged');
CREATE INDEX IF NOT EXISTS idx_ai_tasks_participant_active
  ON ai_tasks (ai_participant_id, started_at) WHERE status IN ('processing', 'acknowledged');
