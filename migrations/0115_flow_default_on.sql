-- Every Flow switch that used to start off now starts on.
--
--   flow_workspace_settings.flow_enabled          a workspace has Flow on unless an admin switched it off
--   flow_runtime_capabilities                     scope_native_legacy, personal_package_imports,
--                                                 personal_scope_package_exports and
--                                                 cross_workspace_forms_conversion default on
--
-- A workspace always owns a flow_workspace_settings row: the authorization epoch fence locks that
-- row, so a workspace without one could be "enabled" but could not take a collaboration ticket,
-- a grant change or an export. The trigger below creates the row (at the column defaults) with the
-- workspace, and the backfill creates it for workspaces that never had one. A row that already
-- exists, in particular one with flow_enabled = false, is never touched.
--
-- The file re-runs whenever the ledger has no row for it, so everything here is idempotent and the
-- capability flags are switched on exactly once, recorded in flow_runtime_capability_rollouts. A
-- re-run must never undo an operator who has since turned a capability off. The one-time update
-- only ever sets a flag to true.

ALTER TABLE flow_workspace_settings
  ALTER COLUMN flow_enabled SET DEFAULT true;

ALTER TABLE flow_runtime_capabilities
  ALTER COLUMN scope_native_legacy SET DEFAULT true,
  ALTER COLUMN personal_package_imports SET DEFAULT true,
  ALTER COLUMN personal_scope_package_exports SET DEFAULT true,
  ALTER COLUMN cross_workspace_forms_conversion SET DEFAULT true;

CREATE TABLE IF NOT EXISTS flow_runtime_capability_rollouts (
  name TEXT PRIMARY KEY,
  applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

WITH claimed AS (
  INSERT INTO flow_runtime_capability_rollouts(name)
  VALUES ('flow_default_on')
  ON CONFLICT (name) DO NOTHING
  RETURNING name
)
UPDATE flow_runtime_capabilities
   SET scope_native_legacy = true,
       personal_package_imports = true,
       personal_scope_package_exports = true,
       cross_workspace_forms_conversion = true,
       updated_at = now()
 WHERE singleton
   AND EXISTS (SELECT 1 FROM claimed);

CREATE OR REPLACE FUNCTION flow_provision_workspace_settings() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  INSERT INTO flow_workspace_settings(workspace_id) VALUES (NEW.id)
  ON CONFLICT (workspace_id) DO NOTHING;
  RETURN NEW;
END $$;

DROP TRIGGER IF EXISTS flow_provision_workspace_settings ON workspaces;
CREATE TRIGGER flow_provision_workspace_settings
  AFTER INSERT ON workspaces
  FOR EACH ROW EXECUTE FUNCTION flow_provision_workspace_settings();

INSERT INTO flow_workspace_settings(workspace_id)
SELECT id FROM workspaces
ON CONFLICT (workspace_id) DO NOTHING;
