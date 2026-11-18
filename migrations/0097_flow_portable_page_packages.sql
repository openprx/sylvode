-- The portable current-Page format has an independent, default-off rollout.
ALTER TABLE flow_runtime_capabilities ADD COLUMN IF NOT EXISTS portable_page_packages_enabled boolean NOT NULL DEFAULT false;
