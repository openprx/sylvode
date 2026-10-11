-- When the current run of a Flow conversion job entered `started`.
-- The stale-`started` sweep (flow::bridge::reap_stale_conversion_jobs) selects on this column
-- instead of `updated_at`, so a statement that touches a `started` job no longer postpones its
-- reclaim. Set when the job is created and reset when a retry claims a `failed` job back to
-- `started`; no other statement changes it.
-- Existing rows are backfilled: a `started` row took its last write when it was claimed, so its
-- `updated_at` is its start; every other row keeps the time it was created.
-- Additive and idempotent: re-running leaves the same column, values, default and constraint.

ALTER TABLE flow_conversion_jobs ADD COLUMN IF NOT EXISTS started_at TIMESTAMPTZ;
UPDATE flow_conversion_jobs
   SET started_at = CASE WHEN status = 'started' THEN updated_at ELSE created_at END
 WHERE started_at IS NULL;
ALTER TABLE flow_conversion_jobs ALTER COLUMN started_at SET DEFAULT now();
ALTER TABLE flow_conversion_jobs ALTER COLUMN started_at SET NOT NULL;
