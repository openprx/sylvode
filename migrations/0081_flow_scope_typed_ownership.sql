-- Compare final transaction state: typed commands atomically rewrite both event and dispatch type.
CREATE OR REPLACE FUNCTION flow_dispatch_source_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM event_dispatch WHERE id=NEW.id) AND NOT EXISTS(
  SELECT 1 FROM event_dispatch d JOIN business_events e ON e.id=d.event_id WHERE d.id=NEW.id
   AND e.workspace_id IS NOT DISTINCT FROM d.workspace_id
   AND e.source_flow_scope_id IS NOT DISTINCT FROM d.source_flow_scope_id AND e.event_type=d.event_type
 ) THEN RAISE EXCEPTION 'Dispatch source does not match source event'; END IF;
 RETURN NEW;
END $$;
CREATE OR REPLACE FUNCTION business_event_dispatch_type_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 IF EXISTS(SELECT 1 FROM event_dispatch d JOIN business_events e ON e.id=d.event_id WHERE e.id=NEW.id AND e.event_type<>d.event_type) THEN
  RAISE EXCEPTION 'Source event type does not match dispatch type';
 END IF;
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS business_events_dispatch_type_guard ON business_events;
CREATE CONSTRAINT TRIGGER business_events_dispatch_type_guard AFTER UPDATE OF event_type ON business_events DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION business_event_dispatch_type_guard();
DO $migration$
BEGIN
 IF NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='flow_collection_projections' AND column_name='scope_id') THEN
 EXECUTE $ddl$
ALTER TABLE flow_collection_projections ALTER COLUMN workspace_id DROP NOT NULL;
ALTER TABLE flow_collection_projections ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_collection_projections c SET scope_id=o.scope_id FROM flow_objects o WHERE o.id=c.collection_id;
CREATE TRIGGER flow_collection_projections_bridge_scope BEFORE INSERT OR UPDATE ON flow_collection_projections FOR EACH ROW EXECUTE FUNCTION flow_bridge_secondary_scope();
ALTER TABLE flow_collection_projections ALTER COLUMN scope_id SET NOT NULL;
ALTER TABLE flow_collection_projections ADD CONSTRAINT flow_collection_projections_scope_collection_key UNIQUE(scope_id,collection_id);
ALTER TABLE flow_collection_projections ADD CONSTRAINT flow_collection_projections_object_scope_fk FOREIGN KEY(scope_id,collection_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
ALTER TABLE flow_record_projections ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_record_projections r SET scope_id=o.scope_id FROM flow_objects o WHERE o.id=r.record_id;
CREATE FUNCTION flow_record_projection_scope_guard() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE canonical UUID;
BEGIN
 SELECT scope_id INTO canonical FROM flow_objects WHERE id=NEW.record_id;
 IF canonical IS NULL OR (NEW.scope_id IS NOT NULL AND NEW.scope_id<>canonical) THEN RAISE EXCEPTION 'Record projection scope mismatch'; END IF;
 NEW.scope_id:=canonical;
 RETURN NEW;
END $$;
CREATE TRIGGER flow_record_projections_bridge_scope BEFORE INSERT OR UPDATE ON flow_record_projections FOR EACH ROW EXECUTE FUNCTION flow_record_projection_scope_guard();
ALTER TABLE flow_record_projections ALTER COLUMN scope_id SET NOT NULL;
ALTER TABLE flow_record_projections ADD CONSTRAINT flow_record_projections_object_scope_fk FOREIGN KEY(scope_id,record_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
ALTER TABLE flow_record_projections ADD CONSTRAINT flow_record_projections_collection_scope_fk FOREIGN KEY(scope_id,collection_id) REFERENCES flow_collection_projections(scope_id,collection_id) ON DELETE CASCADE;
$ddl$;
 END IF;
END $migration$;
