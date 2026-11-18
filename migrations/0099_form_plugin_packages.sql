CREATE TABLE IF NOT EXISTS form_plugin_packages (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key text NOT NULL,
    version text NOT NULL,
    manifest jsonb NOT NULL,
    package_sha256 text NOT NULL,
    wasm_bytes bytea NOT NULL,
    html text NOT NULL,
    installed_by uuid NOT NULL,
    installed_at timestamptz NOT NULL DEFAULT now(),
    UNIQUE (project_id, key, version),
    UNIQUE (project_id, key, id)
);

CREATE TABLE IF NOT EXISTS form_plugin_bindings (
    project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key text NOT NULL,
    package_id uuid,
    generation bigint NOT NULL DEFAULT 0 CHECK (generation BETWEEN 0 AND 9007199254740991),
    grants text[] NOT NULL DEFAULT '{}',
    PRIMARY KEY (project_id, key),
    FOREIGN KEY (project_id, key, package_id) REFERENCES form_plugin_packages(project_id, key, id),
    CHECK (package_id IS NOT NULL OR cardinality(grants) = 0)
);

CREATE TABLE IF NOT EXISTS form_plugin_binding_changes (
    project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    key text NOT NULL,
    request_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    request jsonb NOT NULL,
    response jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, key, request_id)
);

CREATE TABLE IF NOT EXISTS form_plugin_invocations (
    project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    request_id uuid NOT NULL,
    actor_id uuid NOT NULL,
    record_id uuid NOT NULL,
    package_id uuid NOT NULL REFERENCES form_plugin_packages(id) ON DELETE CASCADE,
    generation bigint NOT NULL,
    capability text NOT NULL CHECK (capability IN ('amount.preview', 'amount.apply')),
    status text NOT NULL CHECK (status IN ('completed', 'failed', 'timeout')),
    amount_cents bigint,
    preview_request_id uuid,
    record_updated_at timestamptz NOT NULL,
    schema_version integer NOT NULL,
    error_kind text,
    duration_ms bigint,
    fuel_consumed bigint,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, actor_id, record_id, package_id, generation, capability, request_id),
    CHECK ((status = 'completed') = (amount_cents IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS idx_form_plugin_invocations_project_created
    ON form_plugin_invocations(project_id, created_at DESC);
