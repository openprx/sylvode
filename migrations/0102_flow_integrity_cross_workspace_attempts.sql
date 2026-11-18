-- Refused cross-workspace commands belong to the requesting workspace. Their subject
-- identifies the foreign target for internal diagnosis, without changing target ownership.
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
  IF actual IS NOT NULL AND actual IS DISTINCT FROM binding THEN
   IF NEW.kind IS DISTINCT FROM 'cross_workspace_relation'
    OR NEW.subject_kind IS DISTINCT FROM 'flow_object'
    OR COALESCE(NEW.detected_by, '') NOT IN ('flow.command.create_object', 'flow.command.move_object')
    OR NEW.details_redacted->>'requesting_workspace_id' IS DISTINCT FROM NEW.workspace_id::text
    OR NOT EXISTS (
     SELECT 1 FROM flow_workspace_scope_bindings w
     WHERE w.scope_id=actual AND w.workspace_id::text=NEW.details_redacted->>'referenced_workspace_id'
    )
   THEN RAISE EXCEPTION 'Integrity subject/workspace mismatch'; END IF;
  END IF;
  actual:=binding;
 END IF;
 IF actual IS NULL THEN actual:=NEW.scope_id; END IF;
 IF actual IS NULL OR (NEW.scope_id IS NOT NULL AND NEW.scope_id IS DISTINCT FROM actual) THEN RAISE EXCEPTION 'Integrity owner scope mismatch'; END IF;
 NEW.scope_id:=actual;
 IF TG_OP='UPDATE' AND OLD.scope_id IS DISTINCT FROM NEW.scope_id THEN RAISE EXCEPTION 'Integrity ownership is immutable'; END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS flow_integrity_bind_owner ON flow_integrity_records;
CREATE TRIGGER flow_integrity_bind_owner BEFORE INSERT OR UPDATE OF
 workspace_id,subject_kind,subject_id,scope_id,kind,detected_by,details_redacted
 ON flow_integrity_records FOR EACH ROW EXECUTE FUNCTION flow_integrity_bind_owner();
