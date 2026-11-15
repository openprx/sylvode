-- Complete accepted resource/schema dependencies for new legacy export artifacts.
ALTER TABLE flow_export_jobs ADD COLUMN IF NOT EXISTS artifact_resource_ids UUID[] NOT NULL DEFAULT '{}'::uuid[];
ALTER TABLE flow_export_jobs ADD COLUMN IF NOT EXISTS artifact_schema_revisions JSONB NOT NULL DEFAULT '[]'::jsonb;
