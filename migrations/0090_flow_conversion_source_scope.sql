-- Conversion ownership follows its source Flow scope. workspace_id is the genuine destination
-- Forms workspace, which may differ from a legacy source binding or have no source binding.
ALTER TABLE flow_runtime_capabilities ADD COLUMN IF NOT EXISTS personal_forms_conversion BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE flow_runtime_capabilities ADD COLUMN IF NOT EXISTS cross_workspace_forms_conversion BOOLEAN NOT NULL DEFAULT true;
DO $$ DECLARE tab TEXT; BEGIN
 FOREACH tab IN ARRAY ARRAY['flow_conversion_previews','flow_conversion_jobs','flow_object_lineage'] LOOP
  IF NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name=tab AND column_name='scope_id') THEN
   EXECUTE format('ALTER TABLE %I ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE',tab);
   EXECUTE format('UPDATE %I t SET scope_id=o.scope_id FROM flow_objects o WHERE o.id=t.source_object_id',tab);
   EXECUTE format('ALTER TABLE %I ALTER COLUMN scope_id SET NOT NULL',tab);
   EXECUTE format('ALTER TABLE %I ADD CONSTRAINT %I FOREIGN KEY(scope_id,source_object_id) REFERENCES flow_objects(scope_id,id) ON DELETE RESTRICT',tab,tab||'_source_scope_fk');
  END IF;
 END LOOP;
END $$;
CREATE OR REPLACE FUNCTION flow_conversion_bind_source_scope() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE actual UUID;
BEGIN
 SELECT scope_id INTO actual FROM flow_objects WHERE id=NEW.source_object_id;
 IF actual IS NULL OR (NEW.scope_id IS NOT NULL AND NEW.scope_id<>actual) THEN RAISE EXCEPTION 'Conversion source scope mismatch'; END IF;
 NEW.scope_id:=actual;
 IF TG_OP='UPDATE' AND OLD.scope_id IS DISTINCT FROM NEW.scope_id THEN RAISE EXCEPTION 'Conversion source ownership is immutable'; END IF;
 RETURN NEW;
END $$;
DO $$ DECLARE tab TEXT; BEGIN
 FOREACH tab IN ARRAY ARRAY['flow_conversion_previews','flow_conversion_jobs','flow_object_lineage'] LOOP
  EXECUTE format('DROP TRIGGER IF EXISTS flow_conversion_bind_source_scope ON %I',tab);
  EXECUTE format('CREATE TRIGGER flow_conversion_bind_source_scope BEFORE INSERT OR UPDATE OF scope_id,source_object_id ON %I FOR EACH ROW EXECUTE FUNCTION flow_conversion_bind_source_scope()',tab);
 END LOOP;
END $$;
ALTER TABLE flow_conversion_previews ADD COLUMN IF NOT EXISTS request_key TEXT;
ALTER TABLE flow_conversion_jobs ADD COLUMN IF NOT EXISTS request_key TEXT;
UPDATE flow_conversion_previews SET request_key=idempotency_key WHERE request_key IS NULL;
UPDATE flow_conversion_jobs SET request_key=idempotency_key WHERE request_key IS NULL;
ALTER TABLE flow_conversion_previews ALTER COLUMN request_key SET NOT NULL;
ALTER TABLE flow_conversion_jobs ALTER COLUMN request_key SET NOT NULL;
CREATE OR REPLACE FUNCTION flow_conversion_bind_request_key() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN IF NEW.request_key IS NULL THEN NEW.request_key:=NEW.idempotency_key; END IF; RETURN NEW; END $$;
DROP TRIGGER IF EXISTS flow_conversion_bind_request_key ON flow_conversion_previews;
CREATE TRIGGER flow_conversion_bind_request_key BEFORE INSERT ON flow_conversion_previews FOR EACH ROW EXECUTE FUNCTION flow_conversion_bind_request_key();
DROP TRIGGER IF EXISTS flow_conversion_bind_request_key ON flow_conversion_jobs;
CREATE TRIGGER flow_conversion_bind_request_key BEFORE INSERT ON flow_conversion_jobs FOR EACH ROW EXECUTE FUNCTION flow_conversion_bind_request_key();
CREATE UNIQUE INDEX IF NOT EXISTS flow_conversion_previews_scope_key ON flow_conversion_previews(scope_id,request_key);
CREATE UNIQUE INDEX IF NOT EXISTS flow_conversion_jobs_scope_key ON flow_conversion_jobs(scope_id,request_key);
