-- One shared counter per client address for anonymous Flow share reads.
-- The key is a hash, not the address. Every API process updates the same row.

CREATE TABLE IF NOT EXISTS flow_public_share_hits (
  client_hash TEXT PRIMARY KEY,
  window_started TIMESTAMPTZ NOT NULL,
  hit_count INTEGER NOT NULL,
  CONSTRAINT flow_public_share_hits_count_check CHECK (hit_count >= 0)
);

COMMENT ON TABLE flow_public_share_hits IS
  'Cross-instance rate limit for GET /flow/public-shares. One row per hashed client address.';
