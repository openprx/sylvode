-- Project resource kinds are reduced to `directory` and `custom`.
--
-- Removed kinds: repo, document_library, crm_account, erp_order, equipment, site.
-- Existing resource rows are kept: `repo` rows become `directory`, every other removed kind
-- becomes `custom`; the original kind is preserved in the
-- nullable `legacy_kind` column (the table has no neutral metadata column, and `locator` /
-- `permission_policy` hold user-facing data that should not carry migration bookkeeping).
-- Suggested resources in project types, scenario templates and the template copy stored on
-- projects are remapped: repo -> directory, every other removed kind -> custom. Names, labels,
-- descriptions and `required` flags are untouched.
-- Re-running this file is a no-op: nothing matches once the removed kinds are gone.
-- Bulk kind changes do not emit per-row audit events or outbox rows.

ALTER TABLE project_resources
  ADD COLUMN IF NOT EXISTS legacy_kind TEXT NULL;

UPDATE project_resources
SET legacy_kind = COALESCE(legacy_kind, kind),
    kind = CASE WHEN kind = 'repo' THEN 'directory' ELSE 'custom' END,
    updated_at = NOW()
WHERE kind IN ('repo', 'document_library', 'crm_account', 'erp_order', 'equipment', 'site');

ALTER TABLE project_resources
  DROP CONSTRAINT IF EXISTS project_resources_kind_check;

ALTER TABLE project_resources
  ADD CONSTRAINT project_resources_kind_check CHECK (kind IN ('directory', 'custom'));

-- Helper functions, dropped at the end of this file.
CREATE OR REPLACE FUNCTION sv_resource_kinds_has_removed(arr JSONB) RETURNS BOOLEAN
LANGUAGE sql IMMUTABLE AS $$
  SELECT jsonb_typeof(arr) = 'array' AND EXISTS (
    SELECT 1 FROM jsonb_array_elements(arr) AS x
    WHERE jsonb_typeof(x) = 'object'
      AND lower(x ->> 'kind') IN ('repo', 'document_library', 'crm_account', 'erp_order', 'equipment', 'site')
  )
$$;

-- Remaps removed kinds and drops entries that become exact duplicates of an earlier one
-- (for example code_project's repo + directory pair).
CREATE OR REPLACE FUNCTION sv_resource_kinds_remap(arr JSONB) RETURNS JSONB
LANGUAGE sql IMMUTABLE AS $$
  SELECT COALESCE(jsonb_agg(m.e ORDER BY m.first_ord), '[]'::jsonb)
  FROM (
    SELECT r.e, MIN(r.ord) AS first_ord
    FROM (
      SELECT
        CASE
          WHEN jsonb_typeof(x) <> 'object' THEN x
          WHEN lower(x ->> 'kind') = 'repo' THEN jsonb_set(x, '{kind}', '"directory"')
          WHEN lower(x ->> 'kind') IN ('document_library', 'crm_account', 'erp_order', 'equipment', 'site')
            THEN jsonb_set(x, '{kind}', '"custom"')
          ELSE x
        END AS e,
        t.ord
      FROM jsonb_array_elements(arr) WITH ORDINALITY AS t(x, ord)
    ) AS r
    GROUP BY r.e
  ) AS m
$$;

UPDATE project_types
SET field_schema = jsonb_set(field_schema, '{resources}', sv_resource_kinds_remap(field_schema -> 'resources')),
    updated_at = NOW()
WHERE sv_resource_kinds_has_removed(field_schema -> 'resources');

UPDATE scenario_templates
SET resource_schema = jsonb_set(resource_schema, '{resources}', sv_resource_kinds_remap(resource_schema -> 'resources')),
    updated_at = NOW()
WHERE sv_resource_kinds_has_removed(resource_schema -> 'resources');

UPDATE projects
SET type_settings = jsonb_set(
  type_settings,
  '{scenario_template,resource_schema,resources}',
  sv_resource_kinds_remap(type_settings #> '{scenario_template,resource_schema,resources}')
)
WHERE sv_resource_kinds_has_removed(type_settings #> '{scenario_template,resource_schema,resources}');

DROP FUNCTION IF EXISTS sv_resource_kinds_remap(JSONB);
DROP FUNCTION IF EXISTS sv_resource_kinds_has_removed(JSONB);
