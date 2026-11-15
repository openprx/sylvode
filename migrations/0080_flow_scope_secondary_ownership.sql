DO $migration$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='flow_export_jobs' AND column_name='scope_id') THEN
 EXECUTE $ddl$
ALTER TABLE flow_scope_command_receipts ADD COLUMN result JSONB NOT NULL DEFAULT '{}'::jsonb;
CREATE FUNCTION flow_bridge_secondary_scope() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE expected UUID;
BEGIN
 IF NEW.workspace_id IS NOT NULL THEN
  SELECT scope_id INTO expected FROM flow_workspace_scope_bindings WHERE workspace_id=NEW.workspace_id;
  IF NEW.scope_id IS NOT NULL AND NEW.scope_id<>expected THEN RAISE EXCEPTION 'Flow secondary scope mismatch'; END IF;
  NEW.scope_id:=expected;
 ELSIF NEW.scope_id IS NULL THEN RAISE EXCEPTION 'Flow secondary canonical scope required'; END IF;
 IF NEW.workspace_id IS NULL AND NOT EXISTS(SELECT 1 FROM flow_scopes WHERE id=NEW.scope_id AND kind='personal') THEN RAISE EXCEPTION 'Independent secondary ownership requires a personal scope'; END IF;
 IF TG_OP='UPDATE' AND OLD.scope_id IS DISTINCT FROM NEW.scope_id THEN RAISE EXCEPTION 'Flow secondary scope is immutable'; END IF;
 RETURN NEW;
END $$;
ALTER TABLE flow_package_artifacts ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE flow_package_artifacts ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_package_artifacts a SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=a.workspace_id;
CREATE TRIGGER flow_package_artifacts_bridge_scope BEFORE INSERT OR UPDATE ON flow_package_artifacts FOR EACH ROW EXECUTE FUNCTION flow_bridge_secondary_scope();
ALTER TABLE flow_package_artifacts ALTER COLUMN scope_id SET NOT NULL;
ALTER TABLE flow_export_jobs ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE flow_export_jobs ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_export_jobs a SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=a.workspace_id;
CREATE TRIGGER flow_export_jobs_bridge_scope BEFORE INSERT OR UPDATE ON flow_export_jobs FOR EACH ROW EXECUTE FUNCTION flow_bridge_secondary_scope();
ALTER TABLE flow_export_jobs ALTER COLUMN scope_id SET NOT NULL;
CREATE UNIQUE INDEX flow_export_jobs_scope_idempotency ON flow_export_jobs(scope_id,actor_kind,actor_id,idempotency_key) WHERE workspace_id IS NULL;
ALTER TABLE flow_comments ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE flow_comments ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_comments a SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=a.workspace_id;
CREATE TRIGGER flow_comments_bridge_scope BEFORE INSERT OR UPDATE ON flow_comments FOR EACH ROW EXECUTE FUNCTION flow_bridge_secondary_scope();
ALTER TABLE flow_comments ALTER COLUMN scope_id SET NOT NULL;
ALTER TABLE flow_share_links ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE flow_share_links ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_share_links a SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=a.workspace_id;
CREATE TRIGGER flow_share_links_bridge_scope BEFORE INSERT OR UPDATE ON flow_share_links FOR EACH ROW EXECUTE FUNCTION flow_bridge_secondary_scope();
ALTER TABLE flow_share_links ALTER COLUMN scope_id SET NOT NULL;
ALTER TABLE flow_appearance_assets ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE flow_appearance_assets ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_appearance_assets a SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=a.workspace_id;
CREATE TRIGGER flow_appearance_assets_bridge_scope BEFORE INSERT OR UPDATE ON flow_appearance_assets FOR EACH ROW EXECUTE FUNCTION flow_bridge_secondary_scope();
ALTER TABLE flow_appearance_assets ALTER COLUMN scope_id SET NOT NULL;
ALTER TABLE flow_package_artifacts ADD CONSTRAINT flow_package_artifacts_scope_id_key UNIQUE(scope_id,id);
ALTER TABLE flow_export_jobs ADD CONSTRAINT flow_export_jobs_scope_object_fk FOREIGN KEY(scope_id,object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
ALTER TABLE flow_export_jobs ADD CONSTRAINT flow_export_jobs_scope_artifact_fk FOREIGN KEY(scope_id,artifact_id) REFERENCES flow_package_artifacts(scope_id,id) ON DELETE SET NULL (artifact_id);
ALTER TABLE flow_comments ADD CONSTRAINT flow_comments_scope_object_fk FOREIGN KEY(scope_id,object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
ALTER TABLE flow_share_links ADD CONSTRAINT flow_share_links_scope_object_fk FOREIGN KEY(scope_id,object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
ALTER TABLE flow_appearance_assets ADD CONSTRAINT flow_appearance_assets_scope_object_fk FOREIGN KEY(scope_id,object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
ALTER TABLE flow_scope_command_receipts ADD CONSTRAINT flow_scope_command_receipts_object_fk FOREIGN KEY(scope_id,object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
$ddl$;
 END IF;
END $migration$;
