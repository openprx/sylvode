-- Flow capabilities are on by default.
--
-- Until now every row of flow_runtime_capabilities started off, so a fresh installation could not
-- open a personal Flow space, create a rich Page or use the other shipped Flow surfaces until an
-- operator flipped the singleton row by hand. Every running component (api, worker, maintenance)
-- now registers reader version 5, so the capabilities they implement start on:
--
--   minimum_reader_version 5        rich Pages (model5) need >= 4; block transfers need exactly 5;
--                                   portable packages need >= 5
--   personal_provisioning           lazily create the caller's own personal scope
--   personal_writes                 write in a personal scope (requires personal_provisioning)
--   external_mounts                 mount content into a personal scope (requires personal_writes)
--   personal_forms_conversion       convert personal Flow content into a Forms workspace
--   block_transfers_enabled         move or copy blocks between Pages
--   portable_page_packages_enabled  portable single-Page packages
--   portable_graph_packages_enabled portable multi-object packages
--
-- scope_native_legacy, personal_package_imports, personal_scope_package_exports and
-- cross_workspace_forms_conversion are left alone here; 0115_flow_default_on.sql switches them on.
--
-- The column defaults change unconditionally. The singleton row is switched on exactly once,
-- recorded in flow_runtime_capability_rollouts: this file re-runs whenever the ledger has no row
-- for it (an adopted database, the migrations.replay switch), and a re-run must never undo an
-- operator who has since turned a capability off. The one-time update only ever sets a flag to
-- true and never lowers minimum_reader_version, so it cannot turn off anything an operator turned
-- on. Every value it writes satisfies the CHECK constraints between the flags.

ALTER TABLE flow_runtime_capabilities
  ALTER COLUMN minimum_reader_version SET DEFAULT 5,
  ALTER COLUMN personal_provisioning SET DEFAULT true,
  ALTER COLUMN personal_writes SET DEFAULT true,
  ALTER COLUMN external_mounts SET DEFAULT true,
  ALTER COLUMN personal_forms_conversion SET DEFAULT true,
  ALTER COLUMN block_transfers_enabled SET DEFAULT true,
  ALTER COLUMN portable_page_packages_enabled SET DEFAULT true,
  ALTER COLUMN portable_graph_packages_enabled SET DEFAULT true;

CREATE TABLE IF NOT EXISTS flow_runtime_capability_rollouts (
  name TEXT PRIMARY KEY,
  applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

WITH claimed AS (
  INSERT INTO flow_runtime_capability_rollouts(name)
  VALUES ('flow_capabilities_default_on')
  ON CONFLICT (name) DO NOTHING
  RETURNING name
)
UPDATE flow_runtime_capabilities
   SET minimum_reader_version = GREATEST(minimum_reader_version, 5),
       personal_provisioning = true,
       personal_writes = true,
       external_mounts = true,
       personal_forms_conversion = true,
       block_transfers_enabled = true,
       portable_page_packages_enabled = true,
       portable_graph_packages_enabled = true,
       updated_at = now()
 WHERE singleton
   AND EXISTS (SELECT 1 FROM claimed);
