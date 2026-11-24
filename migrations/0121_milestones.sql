-- Re-runnable.
-- Workspace-level milestones: one milestone spans the projects of a workspace, and a work item
-- belongs to at most one of them.
CREATE TABLE IF NOT EXISTS milestones (
  id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name         TEXT NOT NULL CHECK (length(btrim(name)) BETWEEN 1 AND 100),
  description  TEXT NOT NULL DEFAULT '',
  target_date  DATE NULL,
  status       TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'closed')),
  created_by   UUID NULL REFERENCES users(id) ON DELETE SET NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX IF NOT EXISTS milestones_workspace_lower_name ON milestones (workspace_id, lower(name));

ALTER TABLE work_items ADD COLUMN IF NOT EXISTS milestone_id UUID NULL REFERENCES milestones(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS work_items_milestone ON work_items (milestone_id) WHERE milestone_id IS NOT NULL;
