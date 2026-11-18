-- Next independent candidate only. No content snapshot/oplog/private comments or ACL are stored.
CREATE TABLE IF NOT EXISTS flow_page_block_transfers (
 id UUID PRIMARY KEY,
 actor_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 source_object_id UUID NOT NULL,
 target_object_id UUID NOT NULL,
 kind TEXT NOT NULL CHECK(kind IN ('move','copy')),
 state TEXT NOT NULL CHECK(state IN ('prepared','accepted','not_applied','undone','expired','cancelled')),
 prepare_key TEXT NOT NULL CHECK(octet_length(prepare_key) BETWEEN 1 AND 128),
 prepare_body JSONB NOT NULL,
 source_frontier BYTEA NOT NULL,
 target_frontier BYTEA NOT NULL,
 selection_digest TEXT NOT NULL CHECK(length(selection_digest)=64),
 -- Selected identity/placement descriptors and staged asset mappings, never paragraph bodies.
 selection_metadata JSONB NOT NULL,
 asset_mapping JSONB NOT NULL DEFAULT '[]'::jsonb,
 receipt JSONB,
 commit_key TEXT,
 commit_body JSONB,
 undo_key TEXT,
 undo_body JSONB,
 cancel_key TEXT,
 cancel_body JSONB,
 failure_reason TEXT,
 expires_at TIMESTAMPTZ NOT NULL,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 UNIQUE(actor_id,source_object_id,prepare_key),
 CHECK(source_object_id<>target_object_id),
 CHECK((state IN ('accepted','undone'))=(receipt IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS flow_page_transfer_pending ON flow_page_block_transfers(expires_at,id) WHERE state='prepared';

-- Private copy Undo proof; older accepted candidate rows have no safe proof to reconstruct.
ALTER TABLE flow_page_block_transfers ADD COLUMN IF NOT EXISTS copied_placements JSONB;

-- Gate the candidate independently of all previously adopted Flow surfaces.
ALTER TABLE flow_runtime_capabilities
 ADD COLUMN IF NOT EXISTS block_transfers_enabled BOOLEAN NOT NULL DEFAULT FALSE;
