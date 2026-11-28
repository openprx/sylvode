-- Public plugin index (contract M15, batch M15-3): the cache of fetched indexes, the per-workspace
-- hidden sources, the audit log of index events, the origin of keys and packages that came from an
-- index, and the `index_revoked` disabled reason.
--
-- `package_index_cache` holds one row per configured source: the last good validated document
-- (`body`, at most 8 MiB, parsed again on read), its ETag, when it last changed (`fetched_at`) and
-- was last checked (`checked_at`), the error of the last failed refresh (the good copy stays), and
-- the warnings of the last revocation sync. `workspace_index_sources` lets a workspace owner hide a
-- configured source; a workspace cannot add one. `package_index_events` is the audit log of the
-- index (the project-scoped `governance_audit_logs` cannot hold it).
--
-- Re-runnable: every statement tolerates the new state.

ALTER TABLE publisher_keys ADD COLUMN IF NOT EXISTS origin TEXT NULL;
ALTER TABLE publisher_keys DROP CONSTRAINT IF EXISTS publisher_keys_origin_check;
ALTER TABLE publisher_keys ADD CONSTRAINT publisher_keys_origin_check
  CHECK (origin IS NULL OR origin ~ '^index:[a-z][a-z0-9-]{0,31}$');

-- {source, plugin_id, version, url, sha256, imported_by, imported_at}
ALTER TABLE packages ADD COLUMN IF NOT EXISTS origin JSONB NULL;
ALTER TABLE packages DROP CONSTRAINT IF EXISTS packages_origin_check;
ALTER TABLE packages ADD CONSTRAINT packages_origin_check
  CHECK (origin IS NULL OR jsonb_typeof(origin) = 'object');
CREATE INDEX IF NOT EXISTS packages_origin_source ON packages ((origin ->> 'source'))
  WHERE origin IS NOT NULL;
CREATE INDEX IF NOT EXISTS publisher_keys_origin ON publisher_keys (origin) WHERE origin IS NOT NULL;

CREATE TABLE IF NOT EXISTS package_index_cache (
  source_id TEXT PRIMARY KEY CHECK (source_id ~ '^[a-z][a-z0-9-]{0,31}$'),
  url TEXT NOT NULL,
  etag TEXT NULL,
  body BYTEA NULL CHECK (body IS NULL OR length(body) <= 8388608),
  generated_at TIMESTAMPTZ NULL,
  plugin_count INTEGER NOT NULL DEFAULT 0 CHECK (plugin_count >= 0),
  fetched_at TIMESTAMPTZ NULL,
  checked_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  error TEXT NULL,
  warnings JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (jsonb_typeof(warnings) = 'array'),
  CHECK ((body IS NULL) = (fetched_at IS NULL))
);

CREATE TABLE IF NOT EXISTS workspace_index_sources (
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL CHECK (source_id ~ '^[a-z][a-z0-9-]{0,31}$'),
  hidden BOOLEAN NOT NULL DEFAULT false,
  PRIMARY KEY (workspace_id, source_id)
);

CREATE TABLE IF NOT EXISTS package_index_events (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL CHECK (source_id ~ '^[a-z][a-z0-9-]{0,31}$'),
  action TEXT NOT NULL CHECK (action IN
    ('import','import_refused','trust_key','key_revoked_sync','version_revoked_sync','refresh_failed')),
  actor_id UUID NULL REFERENCES users(id) ON DELETE SET NULL,
  details JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(details) = 'object'),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS package_index_events_workspace
  ON package_index_events (workspace_id, created_at DESC, id);
CREATE INDEX IF NOT EXISTS package_index_events_source
  ON package_index_events (source_id, created_at DESC, id);

ALTER TABLE installations DROP CONSTRAINT IF EXISTS installations_disabled_reason_check;
ALTER TABLE installations ADD CONSTRAINT installations_disabled_reason_check
  CHECK (disabled_reason IN ('admin','approver_lost','key_revoked','index_revoked'));
