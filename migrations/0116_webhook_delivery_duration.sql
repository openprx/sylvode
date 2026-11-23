-- Stores the measured duration of each legacy webhook delivery attempt.
ALTER TABLE webhook_deliveries ADD COLUMN IF NOT EXISTS duration_ms BIGINT;
