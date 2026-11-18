CREATE TABLE IF NOT EXISTS form_plugin_layouts (
    package_id uuid PRIMARY KEY REFERENCES form_plugin_packages(id) ON DELETE CASCADE,
    placements jsonb NOT NULL CHECK (jsonb_typeof(placements) = 'array' AND jsonb_array_length(placements) = 2),
    updated_at timestamptz NOT NULL DEFAULT clock_timestamp()
);
