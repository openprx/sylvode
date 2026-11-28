-- The one-time rollout markers of 0106 and 0115 are no longer needed: each migration file runs
-- exactly once, so the "switch on once" guard has nothing left to guard. The plain column defaults
-- those files set stay in force (restated here so this file is self-contained); the singleton row
-- was switched on when they ran and is not touched again, so an operator's later choice stands.

ALTER TABLE flow_runtime_capabilities
  ALTER COLUMN minimum_reader_version SET DEFAULT 5,
  ALTER COLUMN personal_provisioning SET DEFAULT true,
  ALTER COLUMN personal_writes SET DEFAULT true,
  ALTER COLUMN external_mounts SET DEFAULT true,
  ALTER COLUMN personal_forms_conversion SET DEFAULT true,
  ALTER COLUMN block_transfers_enabled SET DEFAULT true,
  ALTER COLUMN portable_page_packages_enabled SET DEFAULT true,
  ALTER COLUMN portable_graph_packages_enabled SET DEFAULT true,
  ALTER COLUMN scope_native_legacy SET DEFAULT true,
  ALTER COLUMN personal_package_imports SET DEFAULT true,
  ALTER COLUMN personal_scope_package_exports SET DEFAULT true,
  ALTER COLUMN cross_workspace_forms_conversion SET DEFAULT true;

DROP TABLE IF EXISTS flow_runtime_capability_rollouts;
