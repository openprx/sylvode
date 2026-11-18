CREATE TABLE IF NOT EXISTS form_plugin_library (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_id uuid NOT NULL,
    key text NOT NULL,
    version text NOT NULL,
    manifest jsonb NOT NULL,
    package_sha256 text NOT NULL,
    wasm_bytes bytea NOT NULL,
    html text NOT NULL,
    installed_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    archived_at timestamptz,
    UNIQUE (owner_id, key, version)
);

ALTER TABLE form_plugin_packages
    ADD COLUMN IF NOT EXISTS library_source_id uuid REFERENCES form_plugin_library(id),
    ADD COLUMN IF NOT EXISTS library_revoked_at timestamptz;

CREATE UNIQUE INDEX IF NOT EXISTS idx_form_plugin_packages_library_source
    ON form_plugin_packages(project_id, library_source_id)
    WHERE library_source_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS form_plugin_library_requests (
    project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    source_id uuid NOT NULL REFERENCES form_plugin_library(id),
    status text NOT NULL CHECK (status IN ('pending', 'attached', 'rejected', 'revoked')),
    submitted_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    decided_by uuid,
    decided_at timestamptz,
    PRIMARY KEY (project_id, source_id)
);

CREATE INDEX IF NOT EXISTS idx_form_plugin_library_requests_source
    ON form_plugin_library_requests(source_id);
