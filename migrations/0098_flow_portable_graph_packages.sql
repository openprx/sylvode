-- The multi-object and retained-timeline profile rolls out independently.
ALTER TABLE flow_runtime_capabilities ADD COLUMN IF NOT EXISTS portable_graph_packages_enabled boolean NOT NULL DEFAULT false;
