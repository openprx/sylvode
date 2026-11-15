ALTER TABLE flow_export_jobs ADD COLUMN IF NOT EXISTS warnings JSONB NOT NULL DEFAULT '[]'::jsonb;
ALTER TABLE flow_export_jobs ADD COLUMN IF NOT EXISTS object_types TEXT[];
ALTER TABLE flow_export_jobs DROP CONSTRAINT IF EXISTS flow_export_jobs_scope_check;
ALTER TABLE flow_export_jobs ADD CONSTRAINT flow_export_jobs_scope_check CHECK (
 (scope_kind='object' AND object_id IS NOT NULL AND project_id IS NULL) OR
 (scope_kind='workspace' AND object_id IS NULL AND workspace_id IS NOT NULL) OR
 (scope_kind='scope' AND object_id IS NULL AND project_id IS NULL AND workspace_id IS NULL)
);
ALTER TABLE flow_export_jobs DROP CONSTRAINT IF EXISTS flow_export_jobs_selection_check;
ALTER TABLE flow_export_jobs ADD CONSTRAINT flow_export_jobs_selection_check CHECK (
 jsonb_typeof(warnings)='array' AND
 (object_types IS NULL OR (cardinality(object_types)>0 AND array_position(object_types,NULL) IS NULL AND object_types <@ ARRAY['page','navigator']::text[]))
);
