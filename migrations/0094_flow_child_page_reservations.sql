-- Preparation does not create a Page. Only an accepted content transaction finalizes it.
CREATE TABLE IF NOT EXISTS flow_child_page_reservations (
 id UUID PRIMARY KEY,
 scope_id UUID NOT NULL REFERENCES flow_scopes(id) ON DELETE CASCADE,
 source_object_id UUID NOT NULL,
 actor_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
 child_page_id UUID NOT NULL UNIQUE,
 title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 500),
 state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','finalized','cancelled')),
 expires_at TIMESTAMPTZ NOT NULL,
 finalized_seq BIGINT,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 finalized_at TIMESTAMPTZ,
 FOREIGN KEY(scope_id,source_object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE,
 CHECK((state='finalized')=(finalized_seq IS NOT NULL))
);
CREATE INDEX IF NOT EXISTS idx_flow_child_reservations_pending ON flow_child_page_reservations(source_object_id,actor_id,expires_at) WHERE state='pending';
-- target identity is retained when its Page is purged: old source refs become placeholders.
CREATE TABLE IF NOT EXISTS flow_page_child_refs (
 scope_id UUID NOT NULL,
 source_object_id UUID NOT NULL,
 block_cid TEXT NOT NULL CHECK(octet_length(block_cid) BETWEEN 1 AND 256),
 child_page_id UUID NOT NULL,
 active BOOLEAN NOT NULL,
 first_seen_seq BIGINT NOT NULL,
 last_seen_seq BIGINT NOT NULL CHECK(last_seen_seq>=first_seen_seq),
 PRIMARY KEY(scope_id,source_object_id,block_cid),
 FOREIGN KEY(scope_id,source_object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE
);
