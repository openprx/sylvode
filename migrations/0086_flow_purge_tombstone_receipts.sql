-- Purge receipts must outlive the object FK cascade. They contain only request identity and
-- the deletion summary, never document content. The shared advisory key claim is unchanged.
CREATE TABLE IF NOT EXISTS flow_purge_receipts (
 scope_id UUID NOT NULL REFERENCES flow_scopes(id) ON DELETE CASCADE,
 idempotency_key TEXT NOT NULL CHECK (octet_length(idempotency_key) BETWEEN 1 AND 128),
 actor_id UUID NOT NULL,
 object_id UUID NOT NULL,
 request_body JSONB NOT NULL,
 result JSONB NOT NULL,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 PRIMARY KEY(scope_id,idempotency_key)
);
CREATE INDEX IF NOT EXISTS flow_purge_receipts_actor_object_idx ON flow_purge_receipts(actor_id,object_id);
