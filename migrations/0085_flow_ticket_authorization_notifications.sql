-- Optional v2 negotiation, not an implicit upgrade of old clients or a new required WS capability.
ALTER TABLE collab_tickets ADD COLUMN IF NOT EXISTS authorization_notifications BOOLEAN NOT NULL DEFAULT false;
