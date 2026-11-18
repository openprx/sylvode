-- Independent Page-owned private body blobs. Appearance assets remain unchanged.
-- Candidate migration: not registered/applied until the complete model5/reader4 rollout.
CREATE TABLE IF NOT EXISTS flow_page_assets (
  id UUID PRIMARY KEY,
  scope_id UUID NOT NULL REFERENCES flow_scopes(id) ON DELETE CASCADE,
  object_id UUID NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('image','attachment')),
  filename TEXT NOT NULL CHECK (octet_length(filename) BETWEEN 1 AND 255),
  content_type TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  content_sha256 TEXT NOT NULL CHECK (content_sha256 ~ '^[0-9a-f]{64}$'),
  bytes BYTEA NOT NULL,
  state TEXT NOT NULL DEFAULT 'staged' CHECK (state IN ('staged','active','retained','discarded')),
  first_referenced_seq BIGINT CHECK (first_referenced_seq >= 0),
  last_referenced_seq BIGINT CHECK (last_referenced_seq >= 0),
  created_by UUID REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT flow_page_assets_scope_object_fk FOREIGN KEY(scope_id,object_id)
    REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE,
  CONSTRAINT flow_page_assets_scope_identity UNIQUE(scope_id,object_id,id),
  CONSTRAINT flow_page_assets_type_check CHECK (
    (kind='image' AND content_type IN ('image/png','image/jpeg','image/webp','image/gif')) OR
    (kind='attachment' AND content_type='application/octet-stream')),
  CONSTRAINT flow_page_assets_bytes_check CHECK (
    byte_size > 0 AND byte_size <= 52428800 AND byte_size=octet_length(bytes)),
  CONSTRAINT flow_page_assets_retained_discard_check CHECK (
    state <> 'discarded' OR first_referenced_seq IS NULL)
);
CREATE INDEX IF NOT EXISTS idx_flow_page_assets_owner ON flow_page_assets(scope_id,object_id,id);
CREATE TABLE IF NOT EXISTS flow_page_asset_refs (
  scope_id UUID NOT NULL,
  object_id UUID NOT NULL,
  block_cid TEXT NOT NULL CHECK (octet_length(block_cid) BETWEEN 1 AND 256),
  asset_id UUID NOT NULL,
  active BOOLEAN NOT NULL DEFAULT true,
  first_seen_seq BIGINT NOT NULL CHECK (first_seen_seq >= 0),
  last_seen_seq BIGINT NOT NULL CHECK (last_seen_seq >= first_seen_seq),
  PRIMARY KEY(scope_id,object_id,block_cid,asset_id),
  FOREIGN KEY(scope_id,object_id,asset_id) REFERENCES flow_page_assets(scope_id,object_id,id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_flow_page_asset_refs_active
  ON flow_page_asset_refs(scope_id,object_id,asset_id) WHERE active;
