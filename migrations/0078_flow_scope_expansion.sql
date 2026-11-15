DO $migration$
BEGIN
  IF to_regclass('flow_scopes') IS NULL THEN
    EXECUTE $ddl$
-- Flow scope expansion. Legacy workspace settings remain the ONLY legacy epoch fence.
-- No personal content is enabled by this migration. The separate cutover is deliberately gated.
CREATE TABLE flow_scopes (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  kind TEXT NOT NULL CHECK (kind IN ('personal','legacy_managed')),
  owner_user_id UUID REFERENCES users(id) ON DELETE RESTRICT,
  lifecycle_status TEXT NOT NULL DEFAULT 'active' CHECK (lifecycle_status IN ('active','archived')),
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK ((kind = 'personal') = (owner_user_id IS NOT NULL))
);
-- One retained identity per user, including archival; reactivation cannot mint a second owner scope.
CREATE UNIQUE INDEX flow_scopes_personal_owner_key ON flow_scopes(owner_user_id) WHERE kind='personal';
CREATE TABLE flow_workspace_scope_bindings (
  workspace_id UUID PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  scope_id UUID NOT NULL UNIQUE REFERENCES flow_scopes(id) ON DELETE RESTRICT,
  authority_mode TEXT NOT NULL DEFAULT 'legacy_managed' CHECK(authority_mode='legacy_managed')
);
CREATE TABLE flow_scope_settings (
  scope_id UUID PRIMARY KEY REFERENCES flow_scopes(id) ON DELETE CASCADE,
  authz_revision BIGINT NOT NULL DEFAULT 0 CHECK(authz_revision>=0),
  writes_enabled BOOLEAN NOT NULL DEFAULT true,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE flow_scope_revocations (
  scope_id UUID NOT NULL REFERENCES flow_scopes(id) ON DELETE CASCADE,
  authz_revision BIGINT NOT NULL CHECK(authz_revision>=0),
  subtree_root_ids UUID[] NOT NULL DEFAULT '{}'::uuid[],
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY(scope_id,authz_revision)
);
CREATE TABLE flow_runtime_capabilities (
  singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK(singleton),
  minimum_reader_version INTEGER NOT NULL DEFAULT 2 CHECK(minimum_reader_version>=2),
  scope_native_legacy BOOLEAN NOT NULL DEFAULT false,
  personal_provisioning BOOLEAN NOT NULL DEFAULT false,
  personal_writes BOOLEAN NOT NULL DEFAULT false,
  external_mounts BOOLEAN NOT NULL DEFAULT false,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  CHECK(NOT personal_writes OR personal_provisioning),
  CHECK(NOT external_mounts OR personal_writes)
);
INSERT INTO flow_runtime_capabilities(singleton) VALUES(true);
CREATE TABLE flow_runtime_readers (
  instance_id UUID PRIMARY KEY,
  component TEXT NOT NULL CHECK(component IN ('api','worker','maintenance')),
  reader_version INTEGER NOT NULL,
  lease_expires_at TIMESTAMPTZ NOT NULL,
  updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE FUNCTION flow_ensure_legacy_scope(requested_workspace UUID) RETURNS UUID LANGUAGE plpgsql AS $$
DECLARE result UUID;
BEGIN
  SELECT scope_id INTO result FROM flow_workspace_scope_bindings WHERE workspace_id=requested_workspace;
  IF result IS NOT NULL THEN RETURN result; END IF;
  -- Existing workspace row serializes old root triggers, concurrent first use, and backfill.
  PERFORM 1 FROM workspaces WHERE id=requested_workspace FOR UPDATE;
  IF NOT FOUND THEN RAISE EXCEPTION 'Flow legacy workspace does not exist'; END IF;
  SELECT scope_id INTO result FROM flow_workspace_scope_bindings WHERE workspace_id=requested_workspace;
  IF result IS NOT NULL THEN RETURN result; END IF;
  INSERT INTO flow_scopes(kind) VALUES('legacy_managed') RETURNING id INTO result;
  INSERT INTO flow_workspace_scope_bindings(workspace_id,scope_id) VALUES(requested_workspace,result);
  RETURN result;
END $$;
SELECT flow_ensure_legacy_scope(id) FROM workspaces ORDER BY id;
ALTER TABLE flow_objects ADD COLUMN scope_id UUID REFERENCES flow_scopes(id) ON DELETE CASCADE;
UPDATE flow_objects o SET scope_id=b.scope_id FROM flow_workspace_scope_bindings b WHERE b.workspace_id=o.workspace_id;
CREATE FUNCTION flow_bridge_object_scope() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE expected UUID; kind_value TEXT;
BEGIN
  IF NEW.workspace_id IS NOT NULL THEN
    expected:=flow_ensure_legacy_scope(NEW.workspace_id);
    IF NEW.scope_id IS NOT NULL AND NEW.scope_id<>expected THEN RAISE EXCEPTION 'Flow workspace/scope mismatch'; END IF;
    NEW.scope_id:=expected;
  ELSE
    SELECT kind INTO kind_value FROM flow_scopes WHERE id=NEW.scope_id;
    IF kind_value IS DISTINCT FROM 'personal' OR NEW.project_id IS NOT NULL THEN
      RAISE EXCEPTION 'Flow personal object requires personal scope and no legacy project';
    END IF;
  END IF;
  IF TG_OP='UPDATE' AND OLD.scope_id IS DISTINCT FROM NEW.scope_id THEN RAISE EXCEPTION 'Flow canonical scope is immutable'; END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER flow_objects_bridge_scope BEFORE INSERT OR UPDATE OF scope_id,workspace_id,project_id ON flow_objects FOR EACH ROW EXECUTE FUNCTION flow_bridge_object_scope();
ALTER TABLE flow_objects ALTER COLUMN scope_id SET NOT NULL;
ALTER TABLE flow_objects ADD CONSTRAINT flow_objects_scope_id_key UNIQUE(scope_id,id);
ALTER TABLE flow_objects ADD CONSTRAINT flow_objects_parent_scope_fk FOREIGN KEY(scope_id,parent_id) REFERENCES flow_objects(scope_id,id) ON DELETE CASCADE;
CREATE INDEX flow_objects_scope_parent_idx ON flow_objects(scope_id,parent_id);
CREATE UNIQUE INDEX flow_objects_scope_root_key ON flow_objects(scope_id) WHERE object_type='navigator' AND parent_id IS NULL AND project_id IS NULL;
-- Old root provisioners create the binding via the object bridge, so concurrent old creation is safe.
CREATE FUNCTION flow_scope_authority_guard() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NOT EXISTS(SELECT 1 FROM flow_scopes WHERE id=NEW.scope_id AND kind='legacy_managed') THEN
    RAISE EXCEPTION 'Legacy authority requires a legacy-managed Flow scope';
  END IF;
  RETURN NEW;
END $$;
CREATE TRIGGER flow_workspace_scope_authority_guard BEFORE INSERT OR UPDATE ON flow_workspace_scope_bindings FOR EACH ROW EXECUTE FUNCTION flow_scope_authority_guard();
-- Scope metadata for old rows never changes the historical event replay namespace.
ALTER TABLE business_events ADD COLUMN source_flow_scope_id UUID REFERENCES flow_scopes(id) ON DELETE RESTRICT;
ALTER TABLE business_events ADD COLUMN source_scope_kind TEXT NOT NULL DEFAULT 'workspace' CHECK(source_scope_kind IN ('workspace','flow'));
CREATE TABLE flow_scope_migration_runs (
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  source_revision TEXT NOT NULL,
  stage TEXT NOT NULL,
  object_count BIGINT NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO flow_scope_migration_runs(source_revision,stage,object_count) SELECT 'd523d7c724b4122f7b83cb30e6614a36f51a1c65','scope_backfill',count(*) FROM flow_objects;

$ddl$;
  END IF;
END $migration$;
