ALTER TABLE flow_export_jobs ADD COLUMN IF NOT EXISTS source_authz_revision BIGINT;
ALTER TABLE flow_export_jobs ADD COLUMN IF NOT EXISTS policy_collection_id UUID REFERENCES flow_objects(id) ON DELETE CASCADE;
ALTER TABLE flow_export_jobs ADD COLUMN IF NOT EXISTS policy_schema_seq BIGINT;
ALTER TABLE flow_export_jobs ADD COLUMN IF NOT EXISTS included_record_ids UUID[] NOT NULL DEFAULT '{}'::uuid[];
ALTER TABLE flow_export_jobs ADD COLUMN IF NOT EXISTS audience_authorities JSONB NOT NULL DEFAULT '[]'::jsonb;
