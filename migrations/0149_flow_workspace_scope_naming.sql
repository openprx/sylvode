-- Flow naming clean-up (compat C5): the scope kind of a workspace-bound Flow scope is `workspace`
-- (was `legacy_managed`), its ensure function is `flow_ensure_workspace_scope` (was
-- `flow_ensure_legacy_scope`), and the editor initialization switch of the capability singleton is
-- `workspace_editor_initialization` (was `scope_native_legacy`). Migration 0078 is not edited.
--
-- Order matters: the authority guard checks the scope kind, so the scope rows change before the
-- binding rows. Re-runnable: every statement tolerates the new state.

DO $migration$
BEGIN
  ALTER TABLE flow_scopes DROP CONSTRAINT IF EXISTS flow_scopes_kind_check;
  ALTER TABLE flow_workspace_scope_bindings DROP CONSTRAINT IF EXISTS flow_workspace_scope_bindings_authority_mode_check;

  CREATE OR REPLACE FUNCTION flow_scope_authority_guard() RETURNS trigger LANGUAGE plpgsql AS $$
  BEGIN
    IF NOT EXISTS(SELECT 1 FROM flow_scopes WHERE id=NEW.scope_id AND kind='workspace') THEN
      RAISE EXCEPTION 'Workspace authority requires a workspace Flow scope';
    END IF;
    RETURN NEW;
  END $$;

  UPDATE flow_scopes SET kind='workspace' WHERE kind='legacy_managed';
  UPDATE flow_workspace_scope_bindings SET authority_mode='workspace' WHERE authority_mode='legacy_managed';

  ALTER TABLE flow_scopes ADD CONSTRAINT flow_scopes_kind_check CHECK (kind IN ('personal','workspace'));
  ALTER TABLE flow_workspace_scope_bindings ALTER COLUMN authority_mode SET DEFAULT 'workspace';
  ALTER TABLE flow_workspace_scope_bindings ADD CONSTRAINT flow_workspace_scope_bindings_authority_mode_check CHECK (authority_mode='workspace');

  IF to_regprocedure('flow_ensure_legacy_scope(uuid)') IS NOT NULL THEN
    ALTER FUNCTION flow_ensure_legacy_scope(uuid) RENAME TO flow_ensure_workspace_scope;
  END IF;

  CREATE OR REPLACE FUNCTION flow_ensure_workspace_scope(requested_workspace UUID) RETURNS UUID LANGUAGE plpgsql AS $$
  DECLARE result UUID;
  BEGIN
    SELECT scope_id INTO result FROM flow_workspace_scope_bindings WHERE workspace_id=requested_workspace;
    IF result IS NOT NULL THEN RETURN result; END IF;
    -- Existing workspace row serializes root triggers, concurrent first use, and backfill.
    PERFORM 1 FROM workspaces WHERE id=requested_workspace FOR UPDATE;
    IF NOT FOUND THEN RAISE EXCEPTION 'Flow workspace does not exist'; END IF;
    SELECT scope_id INTO result FROM flow_workspace_scope_bindings WHERE workspace_id=requested_workspace;
    IF result IS NOT NULL THEN RETURN result; END IF;
    INSERT INTO flow_scopes(kind) VALUES('workspace') RETURNING id INTO result;
    INSERT INTO flow_workspace_scope_bindings(workspace_id,scope_id) VALUES(requested_workspace,result);
    RETURN result;
  END $$;

  CREATE OR REPLACE FUNCTION flow_bridge_object_scope() RETURNS trigger LANGUAGE plpgsql AS $$
  DECLARE expected UUID; kind_value TEXT;
  BEGIN
    IF NEW.workspace_id IS NOT NULL THEN
      expected:=flow_ensure_workspace_scope(NEW.workspace_id);
      IF NEW.scope_id IS NOT NULL AND NEW.scope_id<>expected THEN RAISE EXCEPTION 'Flow workspace/scope mismatch'; END IF;
      NEW.scope_id:=expected;
    ELSE
      SELECT kind INTO kind_value FROM flow_scopes WHERE id=NEW.scope_id;
      IF kind_value IS DISTINCT FROM 'personal' OR NEW.project_id IS NOT NULL THEN
        RAISE EXCEPTION 'Flow personal object requires personal scope and no project';
      END IF;
    END IF;
    IF TG_OP='UPDATE' AND OLD.scope_id IS DISTINCT FROM NEW.scope_id THEN RAISE EXCEPTION 'Flow canonical scope is immutable'; END IF;
    RETURN NEW;
  END $$;

  IF EXISTS (SELECT 1 FROM information_schema.columns
             WHERE table_schema = current_schema() AND table_name = 'flow_runtime_capabilities'
               AND column_name = 'scope_native_legacy') THEN
    ALTER TABLE flow_runtime_capabilities RENAME COLUMN scope_native_legacy TO workspace_editor_initialization;
  END IF;
END
$migration$;
