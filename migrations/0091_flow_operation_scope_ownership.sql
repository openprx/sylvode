-- Operation target scope_id remains a document/workspace identifier. Ownership is independent.
ALTER TABLE flow_operation_runs ADD COLUMN IF NOT EXISTS owner_flow_scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_operation_runs r SET owner_flow_scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE r.workspace_id=b.workspace_id AND r.owner_flow_scope_id IS NULL;
ALTER TABLE flow_operation_runs ALTER COLUMN owner_flow_scope_id SET NOT NULL;
ALTER TABLE flow_operation_runs ALTER COLUMN workspace_id DROP NOT NULL;
CREATE OR REPLACE FUNCTION flow_operation_bind_owner() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE actual UUID; binding UUID;
BEGIN
 IF NEW.scope_kind='document' THEN
   SELECT o.scope_id INTO actual FROM collab_documents d JOIN flow_objects o ON o.id=d.object_id WHERE d.id=NEW.scope_id;
 END IF;
 IF NEW.workspace_id IS NOT NULL THEN
   SELECT scope_id INTO binding FROM flow_workspace_scope_bindings WHERE workspace_id=NEW.workspace_id;
   IF actual IS NOT NULL AND actual IS DISTINCT FROM binding THEN RAISE EXCEPTION 'Operation document/workspace mismatch'; END IF;
   actual:=binding;
 END IF;
 IF actual IS NULL OR (NEW.owner_flow_scope_id IS NOT NULL AND NEW.owner_flow_scope_id IS DISTINCT FROM actual) THEN RAISE EXCEPTION 'Operation owner scope mismatch'; END IF;
 NEW.owner_flow_scope_id:=actual;
 IF TG_OP='UPDATE' AND OLD.owner_flow_scope_id IS DISTINCT FROM NEW.owner_flow_scope_id THEN RAISE EXCEPTION 'Operation ownership is immutable'; END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS flow_operation_bind_owner ON flow_operation_runs;
CREATE TRIGGER flow_operation_bind_owner BEFORE INSERT OR UPDATE OF workspace_id,scope_kind,scope_id,owner_flow_scope_id ON flow_operation_runs FOR EACH ROW EXECUTE FUNCTION flow_operation_bind_owner();
CREATE UNIQUE INDEX IF NOT EXISTS flow_operation_runs_owner_request ON flow_operation_runs(owner_flow_scope_id,operation,principal_kind,principal_id,idempotency_key) WHERE idempotency_key IS NOT NULL;
CREATE INDEX IF NOT EXISTS flow_operation_runs_owner_status ON flow_operation_runs(owner_flow_scope_id,status,created_at,id);
-- Integrity ownership is populated for legacy writers as well as scope-native loaders.
UPDATE flow_integrity_records r SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE r.workspace_id=b.workspace_id AND r.scope_id IS NULL;
ALTER TABLE flow_integrity_records ALTER COLUMN scope_id SET NOT NULL;
CREATE OR REPLACE FUNCTION flow_integrity_bind_owner() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE actual UUID; binding UUID;
BEGIN
 IF NEW.subject_kind='collab_document' THEN
  SELECT o.scope_id INTO actual FROM collab_documents d JOIN flow_objects o ON o.id=d.object_id WHERE d.id::text=NEW.subject_id;
 ELSIF NEW.subject_kind='flow_object' THEN
  SELECT scope_id INTO actual FROM flow_objects WHERE id::text=NEW.subject_id;
 END IF;
 IF NEW.workspace_id IS NOT NULL THEN
  SELECT scope_id INTO binding FROM flow_workspace_scope_bindings WHERE workspace_id=NEW.workspace_id;
  IF actual IS NOT NULL AND actual IS DISTINCT FROM binding THEN RAISE EXCEPTION 'Integrity subject/workspace mismatch'; END IF;
  actual:=binding;
 END IF;
 IF actual IS NULL THEN actual:=NEW.scope_id; END IF;
 IF actual IS NULL OR (NEW.scope_id IS NOT NULL AND NEW.scope_id IS DISTINCT FROM actual) THEN RAISE EXCEPTION 'Integrity owner scope mismatch'; END IF;
 NEW.scope_id:=actual;
 IF TG_OP='UPDATE' AND OLD.scope_id IS DISTINCT FROM NEW.scope_id THEN RAISE EXCEPTION 'Integrity ownership is immutable'; END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS flow_integrity_bind_owner ON flow_integrity_records;
CREATE TRIGGER flow_integrity_bind_owner BEFORE INSERT OR UPDATE OF workspace_id,subject_kind,subject_id,scope_id ON flow_integrity_records FOR EACH ROW EXECUTE FUNCTION flow_integrity_bind_owner();
CREATE INDEX IF NOT EXISTS flow_integrity_records_scope_status ON flow_integrity_records(scope_id,status,detected_at,id);
