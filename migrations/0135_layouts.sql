-- Layout store: layouts (sv.layout.v1 specs), their placements in slots, and per-user presentation
-- preferences. Scope ids are polymorphic (workspace, project, form, site) and have no foreign key;
-- the API resolves them inside the caller's workspace on every request. `installation_id` and
-- `generation` carry the package install flow (0138 adds the foreign key): only rows of the active
-- generation of an active installation may be shown.

CREATE TABLE layouts (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace','project','form','site')),
  scope_id UUID NOT NULL,
  layout_key TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  spec JSONB NOT NULL,
  source TEXT NOT NULL CHECK (source IN ('user','package','draft')),
  installation_id UUID NULL,
  generation INTEGER NULL,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','archived')),
  created_by UUID NULL REFERENCES users(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK (jsonb_typeof(spec) = 'object'),
  CHECK ((source = 'package') = (installation_id IS NOT NULL AND generation IS NOT NULL))
);
CREATE UNIQUE INDEX layouts_key ON layouts (scope_kind, scope_id, layout_key, COALESCE(generation, 0));
CREATE INDEX layouts_workspace_scope ON layouts (workspace_id, scope_kind, scope_id);

CREATE TABLE layout_placements (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  slot_id TEXT NOT NULL,
  scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace','project','form')),
  scope_id UUID NOT NULL,
  layout_id UUID NOT NULL REFERENCES layouts(id) ON DELETE CASCADE,
  position INTEGER NOT NULL DEFAULT 0,
  enabled BOOLEAN NOT NULL DEFAULT true,
  condition TEXT NULL CHECK (condition IS NULL OR length(condition) <= 1024),
  installation_id UUID NULL,
  generation INTEGER NULL,
  created_by UUID NULL REFERENCES users(id),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX layout_placements_slot ON layout_placements (slot_id, scope_kind, scope_id) WHERE enabled;
CREATE INDEX layout_placements_layout ON layout_placements (layout_id);

CREATE TABLE layout_user_prefs (
  user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  placement_id UUID NOT NULL REFERENCES layout_placements(id) ON DELETE CASCADE,
  hidden BOOLEAN NOT NULL DEFAULT false,
  collapsed BOOLEAN NOT NULL DEFAULT false,
  position INTEGER NULL,
  PRIMARY KEY (user_id, placement_id)
);
