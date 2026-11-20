CREATE TABLE IF NOT EXISTS form_plugin_contribution_layouts (
    project_id uuid NOT NULL,
    key text NOT NULL,
    contribution_id text NOT NULL CHECK (length(contribution_id) BETWEEN 1 AND 64),
    enabled boolean NOT NULL,
    position smallint NOT NULL CHECK (position BETWEEN 0 AND 99),
    updated_by uuid NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (project_id, key, contribution_id),
    FOREIGN KEY (project_id, key) REFERENCES form_plugin_bindings(project_id, key) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS form_plugin_user_preferences (
    user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    project_id uuid NOT NULL,
    key text NOT NULL,
    contribution_id text NOT NULL CHECK (length(contribution_id) BETWEEN 1 AND 64),
    hidden boolean NOT NULL DEFAULT false,
    collapsed boolean NOT NULL DEFAULT false,
    position smallint CHECK (position IS NULL OR position BETWEEN 0 AND 99),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp(),
    PRIMARY KEY (user_id, project_id, key, contribution_id),
    FOREIGN KEY (project_id, key) REFERENCES form_plugin_bindings(project_id, key) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_form_plugin_user_preferences_project_key
    ON form_plugin_user_preferences(project_id, key);

ALTER TABLE form_plugin_invocations ADD COLUMN IF NOT EXISTS contribution_id text;
