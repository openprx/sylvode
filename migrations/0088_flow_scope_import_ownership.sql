ALTER TABLE flow_runtime_capabilities ADD COLUMN IF NOT EXISTS personal_package_imports BOOLEAN NOT NULL DEFAULT true;
ALTER TABLE flow_runtime_capabilities ADD COLUMN IF NOT EXISTS personal_scope_package_exports BOOLEAN NOT NULL DEFAULT true;
-- Import destination ownership is a real Flow scope; optional workspace is only its legacy
-- binding. Source scope/workspace IDs are opaque archive provenance, never local ownership.
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='flow_import_jobs' AND column_name='scope_id') THEN
  ALTER TABLE flow_import_jobs ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
  UPDATE flow_import_jobs j SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=j.workspace_id;
  ALTER TABLE flow_import_jobs ALTER COLUMN scope_id SET NOT NULL;
  ALTER TABLE flow_import_jobs ALTER COLUMN workspace_id DROP NOT NULL;
  ALTER TABLE flow_import_jobs ADD COLUMN source_scope_id UUID;
  CREATE TRIGGER flow_import_jobs_bind_scope BEFORE INSERT OR UPDATE OF scope_id,workspace_id ON flow_import_jobs FOR EACH ROW EXECUTE FUNCTION flow_bridge_secondary_scope();
  ALTER TABLE flow_import_previews ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
  UPDATE flow_import_previews p SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=p.workspace_id;
  ALTER TABLE flow_import_previews ALTER COLUMN scope_id SET NOT NULL;
  ALTER TABLE flow_import_previews ALTER COLUMN workspace_id DROP NOT NULL;
  CREATE TRIGGER flow_import_previews_bind_scope BEFORE INSERT OR UPDATE OF scope_id,workspace_id ON flow_import_previews FOR EACH ROW EXECUTE FUNCTION flow_bridge_secondary_scope();
  ALTER TABLE flow_import_lineage ADD COLUMN target_scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
  UPDATE flow_import_lineage l SET target_scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=l.target_workspace_id;
  ALTER TABLE flow_import_lineage ALTER COLUMN target_scope_id SET NOT NULL;
  ALTER TABLE flow_import_lineage ALTER COLUMN target_workspace_id DROP NOT NULL;
  ALTER TABLE flow_import_lineage ADD COLUMN source_scope_id UUID;
 END IF;
END $$;
CREATE OR REPLACE FUNCTION flow_import_lineage_bind_scope() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE expected UUID;
BEGIN
 IF NEW.target_workspace_id IS NOT NULL THEN
  SELECT scope_id INTO expected FROM flow_workspace_scope_bindings WHERE workspace_id=NEW.target_workspace_id;
  IF expected IS NULL OR (NEW.target_scope_id IS NOT NULL AND NEW.target_scope_id<>expected) THEN RAISE EXCEPTION 'Import lineage destination scope mismatch'; END IF;
  NEW.target_scope_id:=expected;
 ELSIF NEW.target_scope_id IS NULL OR NOT EXISTS(SELECT 1 FROM flow_scopes WHERE id=NEW.target_scope_id AND kind='personal') THEN
  RAISE EXCEPTION 'Personal import lineage requires its actual destination scope';
 END IF;
 IF TG_OP='UPDATE' AND OLD.target_scope_id IS DISTINCT FROM NEW.target_scope_id THEN RAISE EXCEPTION 'Import lineage destination is immutable'; END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS flow_import_lineage_bind_scope ON flow_import_lineage;
CREATE TRIGGER flow_import_lineage_bind_scope BEFORE INSERT OR UPDATE OF target_scope_id,target_workspace_id ON flow_import_lineage FOR EACH ROW EXECUTE FUNCTION flow_import_lineage_bind_scope();
CREATE UNIQUE INDEX IF NOT EXISTS flow_import_jobs_personal_key ON flow_import_jobs(scope_id,kind,idempotency_key) WHERE workspace_id IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS flow_import_previews_personal_key ON flow_import_previews(scope_id,actor_kind,actor_id,idempotency_key) WHERE workspace_id IS NULL;
CREATE UNIQUE INDEX IF NOT EXISTS flow_import_artifacts_personal_key ON flow_package_artifacts(scope_id,actor_kind,actor_id,purpose,idempotency_key) WHERE workspace_id IS NULL AND idempotency_key IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS flow_import_lineage_personal_key ON flow_import_lineage(target_scope_id,package_sha256,source_kind,source_id) WHERE source_kind='flow_package' AND target_workspace_id IS NULL;
