-- DIY navigation: one `sv.nav.v1` document per workspace and per project, and the per-user
-- preferences (`sv.nav.prefs.v1`) that sit on top of it. No row means the built-in defaults, so
-- nothing changes until somebody configures a scope.

CREATE TABLE nav_configs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NULL REFERENCES projects(id) ON DELETE CASCADE,
  doc JSONB NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  updated_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
  updated_by_bot UUID NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (jsonb_typeof(doc) = 'object')
);
CREATE UNIQUE INDEX nav_configs_scope
  ON nav_configs (workspace_id, COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid));

CREATE TABLE nav_user_prefs (
  user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id UUID NULL REFERENCES projects(id) ON DELETE CASCADE,
  prefs JSONB NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (jsonb_typeof(prefs) = 'object')
);
CREATE UNIQUE INDEX nav_user_prefs_scope
  ON nav_user_prefs (user_id, workspace_id, COALESCE(project_id, '00000000-0000-0000-0000-000000000000'::uuid));
