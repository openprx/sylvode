-- Package store: publisher keys, shared package blobs and package rows (contract C3 section 4.3).
-- Publisher 'sv' is reserved: its keys have no workspace and are synced from configuration at boot.
-- A blob is shared content only and is never listed; every lookup of a package row is filtered to
-- built-in packages or the caller's workspace.

CREATE TABLE publisher_keys (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  publisher TEXT NOT NULL CHECK (publisher ~ '^[a-z0-9][a-z0-9-]{0,38}$'),
  key_id TEXT NOT NULL CHECK (key_id ~ '^ed25519:[0-9a-f]{32}$'),
  public_key BYTEA NOT NULL CHECK (length(public_key) = 32),
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','revoked')),
  created_by UUID NULL REFERENCES users(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  revoked_at TIMESTAMPTZ NULL,
  CHECK ((publisher = 'sv') = (workspace_id IS NULL)),
  UNIQUE NULLS NOT DISTINCT (workspace_id, key_id)
);

CREATE TABLE package_blobs (
  blob_digest TEXT PRIMARY KEY CHECK (blob_digest ~ '^sha256:[0-9a-f]{64}$'),
  size BIGINT NOT NULL CHECK (size > 0),
  bytes BYTEA NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE packages (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  digest TEXT NOT NULL CHECK (digest ~ '^sha256:[0-9a-f]{64}$'),
  blob_digest TEXT NOT NULL REFERENCES package_blobs(blob_digest),
  package_key TEXT NOT NULL,
  version TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('form-template','page-template','app','plugin')),
  publisher_key_id UUID NOT NULL REFERENCES publisher_keys(id),
  manifest JSONB NOT NULL,
  custom_code BOOLEAN NOT NULL,
  tier TEXT NOT NULL CHECK (tier IN ('builtin','private')),
  installable BOOLEAN NOT NULL DEFAULT true,
  created_by UUID NULL REFERENCES users(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK ((tier = 'builtin') = (workspace_id IS NULL)),
  UNIQUE NULLS NOT DISTINCT (digest, workspace_id),
  UNIQUE NULLS NOT DISTINCT (package_key, version, workspace_id)
);
CREATE INDEX packages_workspace ON packages (workspace_id, created_at DESC);
CREATE INDEX packages_key ON packages (publisher_key_id);
CREATE INDEX packages_blob ON packages (blob_digest);
