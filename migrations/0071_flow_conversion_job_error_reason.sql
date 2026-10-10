-- Why a failed Flow conversion job failed, when its stable error code alone does not say.
-- The stale-`started` sweep (flow::bridge::reap_stale_conversion_jobs) sets 'stale_started'
-- next to error_code 'server_rejected', so a client can tell a reaped job from a permanent
-- refusal; every other failure, every retry claim and every completion clears it.
-- Additive and idempotent: re-running leaves the same column and constraint.

ALTER TABLE flow_conversion_jobs ADD COLUMN IF NOT EXISTS error_reason TEXT;
ALTER TABLE flow_conversion_jobs
  DROP CONSTRAINT IF EXISTS flow_conversion_jobs_error_reason_check;
ALTER TABLE flow_conversion_jobs
  ADD CONSTRAINT flow_conversion_jobs_error_reason_check CHECK (
    error_reason IS NULL OR (status = 'failed' AND error_reason IN ('stale_started'))
  );
