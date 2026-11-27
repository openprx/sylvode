-- Plugin installations (contract C1 section 6): installations, their generations, the grants an
-- administrator approved, and the invocation log of the plugin host.
--
-- A generation is one version of a package asked for and, once approved, served. A new version is a
-- new generation that stays `pending` while the active one serves. Grants hang on the generation
-- they were approved for and remember the `plugin_ops_digest` of their capability id at approval, so
-- a widened id is not honoured until it is approved again.
-- The file is safe to run again: every object is guarded.

CREATE TABLE IF NOT EXISTS installations (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace','project')),
  project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
  package_key TEXT NOT NULL CHECK (package_key ~ '^[a-z0-9][a-z0-9-]{0,38}/[a-z0-9][a-z0-9-]{0,63}$'),
  status TEXT NOT NULL CHECK (status IN ('pending','active','disabled','revoked')),
  disabled_reason TEXT CHECK (disabled_reason IN ('admin','approver_lost','key_revoked')),
  active_generation INTEGER,
  created_by_kind TEXT NOT NULL CHECK (created_by_kind IN ('user','bot')),
  created_by UUID NOT NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  revoked_at TIMESTAMPTZ,
  revoked_by UUID REFERENCES users(id) ON DELETE SET NULL,
  CHECK ((scope_kind = 'project') = (project_id IS NOT NULL)),
  CHECK (status NOT IN ('active','disabled') OR active_generation IS NOT NULL),
  CHECK ((status = 'disabled') = (disabled_reason IS NOT NULL))
);
CREATE UNIQUE INDEX IF NOT EXISTS installations_live_per_scope ON installations
  (workspace_id, COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid), package_key)
  WHERE status <> 'revoked';
CREATE INDEX IF NOT EXISTS installations_workspace ON installations (workspace_id, created_at DESC);
CREATE INDEX IF NOT EXISTS installations_project ON installations (project_id) WHERE project_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS installation_generations (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  installation_id UUID NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
  generation INTEGER NOT NULL CHECK (generation >= 1),
  package_row_id UUID NOT NULL REFERENCES packages(id) ON DELETE RESTRICT,
  status TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','superseded')),
  requested_capabilities TEXT[] NOT NULL,
  bind_map JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (jsonb_typeof(bind_map) = 'object'),
  requested_by_kind TEXT NOT NULL CHECK (requested_by_kind IN ('user','bot')),
  requested_by UUID NOT NULL,
  requested_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  reviewed_by UUID REFERENCES users(id) ON DELETE SET NULL,
  reviewed_at TIMESTAMPTZ,
  UNIQUE (installation_id, generation),
  CHECK (status = 'pending' OR reviewed_at IS NOT NULL)
);
CREATE UNIQUE INDEX IF NOT EXISTS installation_generations_one_pending
  ON installation_generations (installation_id) WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS installation_generations_package ON installation_generations (package_row_id);

DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'installations_active_generation_fk') THEN
    ALTER TABLE installations ADD CONSTRAINT installations_active_generation_fk
      FOREIGN KEY (id, active_generation) REFERENCES installation_generations (installation_id, generation)
      DEFERRABLE INITIALLY DEFERRED;
  END IF;
  -- Layouts and placements of a package carry the installation and generation they were created
  -- for (0135); deleting the installation deletes them.
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'layouts_installation_generation_fk') THEN
    ALTER TABLE layouts ADD CONSTRAINT layouts_installation_generation_fk
      FOREIGN KEY (installation_id, generation) REFERENCES installation_generations (installation_id, generation)
      ON DELETE CASCADE;
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'layout_placements_installation_generation_fk') THEN
    ALTER TABLE layout_placements ADD CONSTRAINT layout_placements_installation_generation_fk
      FOREIGN KEY (installation_id, generation) REFERENCES installation_generations (installation_id, generation)
      ON DELETE CASCADE;
  END IF;
END $$;

CREATE TABLE IF NOT EXISTS installation_grants (
  generation_id UUID NOT NULL REFERENCES installation_generations(id) ON DELETE CASCADE,
  capability TEXT NOT NULL REFERENCES capability_ids(id) ON DELETE RESTRICT,
  selector JSONB NOT NULL DEFAULT '{}'::jsonb
    CHECK (jsonb_typeof(selector) = 'object' AND (selector - 'project' - 'form') = '{}'::jsonb),
  plugin_ops_digest TEXT NOT NULL CHECK (plugin_ops_digest ~ '^sha256:[0-9a-f]{64}$'),
  granted_by UUID REFERENCES users(id) ON DELETE SET NULL,
  granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (generation_id, capability, selector)
);

-- One row per invocation: digests and sizes only, never the input, the output, a guest message or
-- a log line. Rows older than 30 days are deleted by an API task.
CREATE TABLE IF NOT EXISTS extension_invocations (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  installation_id UUID NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
  generation INTEGER NOT NULL,
  world TEXT NOT NULL CHECK (world IN ('field-rule','action','provider','job')),
  entry TEXT NOT NULL CHECK (entry ~ '^[a-z0-9_.-]{1,64}$'),
  principal_kind TEXT NOT NULL CHECK (principal_kind IN ('user','bot','installation','none')),
  principal_id UUID,
  input_digest TEXT NOT NULL CHECK (input_digest ~ '^hmac-sha256:[0-9a-f]{64}$'),
  input_size INTEGER NOT NULL,
  output_digest TEXT,
  output_size INTEGER,
  outcome TEXT NOT NULL CHECK (outcome IN ('ok','guest_error','refused','limit','trap')),
  error_code TEXT CHECK (error_code ~ '^[a-z0-9_.]{1,128}$'),
  duration_ms INTEGER NOT NULL,
  fuel_used BIGINT,
  host_calls INTEGER NOT NULL DEFAULT 0,
  log_lines INTEGER NOT NULL DEFAULT 0,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS extension_invocations_installation ON extension_invocations (installation_id, created_at DESC);
CREATE INDEX IF NOT EXISTS extension_invocations_created ON extension_invocations (created_at);
