-- Re-runnable.
-- Outbox of notification e-mails: the API queues a row in the transaction that creates the
-- notification, the worker leases due rows and sends them.
CREATE TABLE IF NOT EXISTS email_outbox (
  id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  user_id          UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  kind             TEXT NOT NULL CHECK (kind IN ('notification', 'digest', 'test')),
  notification_id  UUID NULL REFERENCES notifications(id) ON DELETE CASCADE,
  subject          TEXT NOT NULL,
  body_text        TEXT NOT NULL,
  status           TEXT NOT NULL DEFAULT 'pending'
                   CHECK (status IN ('pending', 'sending', 'sent', 'failed')),
  attempts         INT NOT NULL DEFAULT 0,
  next_attempt_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  lease_expires_at TIMESTAMPTZ NULL,
  last_error       TEXT NULL,
  created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
  sent_at          TIMESTAMPTZ NULL
);

CREATE INDEX IF NOT EXISTS email_outbox_due ON email_outbox (next_attempt_at) WHERE status = 'pending';

CREATE UNIQUE INDEX IF NOT EXISTS email_outbox_one_per_notification ON email_outbox (notification_id)
  WHERE notification_id IS NOT NULL;

ALTER TABLE users ADD COLUMN IF NOT EXISTS last_digest_at TIMESTAMPTZ NULL;
