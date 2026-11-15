-- A Forms reference belongs to its Flow source scope. workspace_id remains the real target
-- Forms workspace; for legacy references source and target workspaces were the same.
DO $$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='flow_bridge_references' AND column_name='scope_id') THEN
  ALTER TABLE flow_bridge_references ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
  UPDATE flow_bridge_references r SET scope_id=o.scope_id FROM flow_objects o WHERE o.id=r.source_object_id;
  ALTER TABLE flow_bridge_references ALTER COLUMN scope_id SET NOT NULL;
  ALTER TABLE flow_bridge_references ADD CONSTRAINT flow_reference_source_scope_fk FOREIGN KEY(scope_id,source_object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
 END IF;
END $$;
CREATE OR REPLACE FUNCTION flow_reference_bind_source_scope() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.scope_id IS NULL THEN SELECT scope_id INTO NEW.scope_id FROM flow_objects WHERE id=NEW.source_object_id; END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS flow_reference_bind_source_scope ON flow_bridge_references;
CREATE TRIGGER flow_reference_bind_source_scope BEFORE INSERT OR UPDATE OF source_object_id,scope_id ON flow_bridge_references FOR EACH ROW EXECUTE FUNCTION flow_reference_bind_source_scope();
CREATE UNIQUE INDEX IF NOT EXISTS flow_reference_scope_request_idx ON flow_bridge_references(scope_id,idempotency_key);
