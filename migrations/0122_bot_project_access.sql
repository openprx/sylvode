-- Per-project access of a workspace bot token. A bot is workspace-wide (the token permissions apply
-- to every project, as before) or project-scoped: a level per project in bot_project_grants and a
-- default level for every other project. Idempotent.
ALTER TABLE workspace_bots
  ADD COLUMN IF NOT EXISTS project_scope TEXT NOT NULL DEFAULT 'workspace'
    CHECK (project_scope IN ('workspace', 'projects')),
  ADD COLUMN IF NOT EXISTS default_project_level TEXT NOT NULL DEFAULT 'none'
    CHECK (default_project_level IN ('none', 'read', 'comment'));

CREATE TABLE IF NOT EXISTS bot_project_grants (
  bot_id     UUID NOT NULL REFERENCES workspace_bots(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  level      TEXT NOT NULL CHECK (level IN ('read', 'comment', 'write')),
  granted_by UUID NULL REFERENCES users(id) ON DELETE SET NULL,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (bot_id, project_id)
);

CREATE INDEX IF NOT EXISTS bot_project_grants_project_idx ON bot_project_grants (project_id);
