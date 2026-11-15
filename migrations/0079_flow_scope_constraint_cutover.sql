DO $migration$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM information_schema.columns WHERE table_name='event_dispatch' AND column_name='source_flow_scope_id') THEN
    EXECUTE $ddl$
-- Reader-compatible schema cutover. Flags remain disabled until an explicit runtime readiness
-- transaction registers compatible API/worker/maintenance readers. Old-binary rollback stops
-- being supported after the first independent personal object/event is committed.
ALTER TABLE flow_objects ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE business_events ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE business_events ADD CONSTRAINT business_events_source_scope_check CHECK(
 (source_scope_kind='workspace' AND workspace_id IS NOT NULL AND source_flow_scope_id IS NULL)
 OR (source_scope_kind='flow' AND workspace_id IS NULL AND source_flow_scope_id IS NOT NULL AND project_id IS NULL)
);
CREATE UNIQUE INDEX business_events_flow_idempotency ON business_events(source_flow_scope_id,idempotency_key) WHERE source_scope_kind='flow' AND idempotency_key IS NOT NULL;
ALTER TABLE event_dispatch ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE event_dispatch ADD COLUMN source_flow_scope_id UUID REFERENCES flow_scopes(id) ON DELETE RESTRICT;
ALTER TABLE event_dispatch ADD CONSTRAINT event_dispatch_source_scope_check CHECK((workspace_id IS NOT NULL) <> (source_flow_scope_id IS NOT NULL));
ALTER TABLE flow_object_grants ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE flow_object_grants ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_object_grants g SET scope_id=o.scope_id FROM flow_objects o WHERE o.id=g.object_id;
CREATE FUNCTION flow_bridge_grant_scope() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE canonical UUID; legacy_workspace UUID;
BEGIN
 SELECT scope_id,workspace_id INTO canonical,legacy_workspace FROM flow_objects WHERE id=NEW.object_id;
 IF canonical IS NULL OR (NEW.scope_id IS NOT NULL AND NEW.scope_id<>canonical) OR NEW.workspace_id IS DISTINCT FROM legacy_workspace THEN RAISE EXCEPTION 'Flow grant scope mismatch'; END IF;
 NEW.scope_id:=canonical;
 RETURN NEW;
END $$;
CREATE TRIGGER flow_object_grants_bridge_scope BEFORE INSERT OR UPDATE ON flow_object_grants FOR EACH ROW EXECUTE FUNCTION flow_bridge_grant_scope();
ALTER TABLE flow_object_grants ALTER COLUMN scope_id SET NOT NULL;
ALTER TABLE flow_object_grants ADD CONSTRAINT flow_object_grants_scope_fk FOREIGN KEY(scope_id,object_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
ALTER TABLE collab_tickets ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE collab_tickets ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
ALTER TABLE collab_tickets ADD COLUMN authz_revision BIGINT;
UPDATE collab_tickets t SET scope_id=o.scope_id FROM collab_documents d JOIN flow_objects o ON o.id=d.object_id WHERE d.id=t.document_id;
CREATE FUNCTION flow_bridge_ticket_scope() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE canonical UUID; legacy_workspace UUID;
BEGIN
 SELECT o.scope_id,o.workspace_id INTO canonical,legacy_workspace FROM collab_documents d JOIN flow_objects o ON o.id=d.object_id WHERE d.id=NEW.document_id;
 IF canonical IS NULL OR (NEW.scope_id IS NOT NULL AND NEW.scope_id<>canonical) OR NEW.workspace_id IS DISTINCT FROM legacy_workspace THEN RAISE EXCEPTION 'Flow ticket scope mismatch'; END IF;
 NEW.scope_id:=canonical;
 RETURN NEW;
END $$;
CREATE TRIGGER collab_tickets_bridge_scope BEFORE INSERT OR UPDATE ON collab_tickets FOR EACH ROW EXECUTE FUNCTION flow_bridge_ticket_scope();
ALTER TABLE collab_tickets ALTER COLUMN scope_id SET NOT NULL;
-- A dispatch source must equal its source event; recipient workspaces live only in deliveries.
CREATE FUNCTION flow_dispatch_source_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM business_events e WHERE e.id=NEW.event_id
  AND e.workspace_id IS NOT DISTINCT FROM NEW.workspace_id
  AND e.source_flow_scope_id IS NOT DISTINCT FROM NEW.source_flow_scope_id
  AND e.event_type=NEW.event_type) THEN
  RAISE EXCEPTION 'Dispatch source does not match source event';
 END IF;
 RETURN NEW;
END $$;
CREATE CONSTRAINT TRIGGER event_dispatch_source_guard AFTER INSERT OR UPDATE ON event_dispatch
 DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION flow_dispatch_source_guard();
CREATE FUNCTION business_event_source_immutable() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF OLD.workspace_id IS DISTINCT FROM NEW.workspace_id OR OLD.source_flow_scope_id IS DISTINCT FROM NEW.source_flow_scope_id OR OLD.source_scope_kind IS DISTINCT FROM NEW.source_scope_kind THEN
  RAISE EXCEPTION 'Business event source is immutable';
 END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER business_events_source_immutable BEFORE UPDATE OF workspace_id,source_flow_scope_id,source_scope_kind ON business_events FOR EACH ROW EXECUTE FUNCTION business_event_source_immutable();
CREATE TABLE flow_scope_command_receipts (
 scope_id UUID NOT NULL REFERENCES flow_scopes(id) ON DELETE CASCADE,
 idempotency_key TEXT NOT NULL CHECK(length(idempotency_key) BETWEEN 1 AND 128),
 actor_id UUID NOT NULL REFERENCES users(id) ON DELETE RESTRICT,
 request_body JSONB NOT NULL,
 object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
 event_id UUID NOT NULL REFERENCES business_events(id),
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 PRIMARY KEY(scope_id,idempotency_key)
);
ALTER TABLE flow_integrity_records ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE flow_integrity_records ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_integrity_records r SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=r.workspace_id;
CREATE TABLE flow_mounts (
 id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
 source_object_id UUID NOT NULL REFERENCES flow_objects(id) ON DELETE CASCADE,
 destination_workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
 destination_project_id UUID REFERENCES projects(id) ON DELETE CASCADE,
 grant_level TEXT NOT NULL CHECK(grant_level IN ('view','comment','edit')),
 created_by UUID REFERENCES users(id) ON DELETE SET NULL,
 created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 UNIQUE NULLS NOT DISTINCT(source_object_id,destination_workspace_id,destination_project_id)
);
CREATE INDEX flow_mounts_source_idx ON flow_mounts(source_object_id);
CREATE INDEX flow_mounts_destination_idx ON flow_mounts(destination_workspace_id,destination_project_id);
-- No mount can move the canonical tree. Destination deletion only removes its placement.
CREATE FUNCTION flow_mount_destination_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF NEW.destination_project_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM projects WHERE id=NEW.destination_project_id AND workspace_id=NEW.destination_workspace_id) THEN RAISE EXCEPTION 'Flow mount destination mismatch'; END IF;
 RETURN NEW;
END $$;
CREATE TRIGGER flow_mounts_destination_guard BEFORE INSERT OR UPDATE ON flow_mounts FOR EACH ROW EXECUTE FUNCTION flow_mount_destination_guard();

$ddl$;
  END IF;
END $migration$;
