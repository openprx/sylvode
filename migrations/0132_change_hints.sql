-- Change hints: database triggers that send `pg_notify` when something a bot may want to re-read
-- changed, for the API's `GET /api/v1/bot/change-hints` stream.
--
-- Three channels, all carrying ids only (never contents), each payload far below the 8,000 byte
-- limit of a notification:
--   sv_change_hint         {"k":"task"|"issue"|"tools", "id", "p" (project), "w" (workspace), "b" (bot)}
--   sv_bot_access_changed  {"b": <bot id>}   the projects a bot may read changed
--   sv_bot_token_changed   {"b": <bot id>}   the bot's token, its state or its user changed
--
-- The migration creates no table. Every trigger function only calls `pg_notify`, inside a nested
-- block that traps every error and reports it as a warning carrying only the SQLSTATE (the message
-- of an error can quote row values), so a hint can never make a domain write fail. A notification
-- is delivered at commit, never for a transaction that rolls back.
--
-- Safe on a database that already applied every earlier migration, and safe to run again: the
-- functions are replaced in place and a trigger is created only when its table does not carry it.
--
-- Locks. Each CREATE TRIGGER takes SHARE ROW EXCLUSIVE on its table, held until the migration
-- transaction commits, so the triggers are created in one block at the end of the file and a lock
-- timeout keeps a long transaction that is already writing to one of the tables from stalling
-- every later writer: the migration then fails after the timeout and is retried by the next start,
-- with no partial effect (the pattern of 0109).
--
-- Down script (run by hand; leaving the objects in place is harmless, a notification with no
-- listener is dropped):
--   DROP TRIGGER IF EXISTS trg_sv_hint_ai_task_insert ON ai_tasks;
--   DROP TRIGGER IF EXISTS trg_sv_hint_ai_task_update ON ai_tasks;
--   DROP TRIGGER IF EXISTS trg_sv_hint_work_item_update ON work_items;
--   DROP TRIGGER IF EXISTS trg_sv_hint_work_item_delete ON work_items;
--   DROP TRIGGER IF EXISTS trg_sv_hint_comment ON comments;
--   DROP TRIGGER IF EXISTS trg_sv_hint_project_type_key ON projects;
--   DROP TRIGGER IF EXISTS trg_sv_hint_project_type_caps ON project_types;
--   DROP TRIGGER IF EXISTS trg_sv_hint_bot_grants ON bot_project_grants;
--   DROP TRIGGER IF EXISTS trg_sv_hint_bot_connections ON project_bot_connections;
--   DROP TRIGGER IF EXISTS trg_sv_hint_bot_access ON workspace_bots;
--   DROP TRIGGER IF EXISTS trg_sv_hint_bot_token ON workspace_bots;
--   DROP TRIGGER IF EXISTS trg_sv_hint_bot_token_delete ON workspace_bots;
--   DROP TRIGGER IF EXISTS trg_sv_hint_bot_user_active ON users;
--   DROP FUNCTION IF EXISTS sv_hint_task();
--   DROP FUNCTION IF EXISTS sv_hint_work_item();
--   DROP FUNCTION IF EXISTS sv_hint_comment();
--   DROP FUNCTION IF EXISTS sv_hint_project_type_key();
--   DROP FUNCTION IF EXISTS sv_hint_project_type_caps();
--   DROP FUNCTION IF EXISTS sv_hint_bot_access();
--   DROP FUNCTION IF EXISTS sv_hint_bot_token();
--   DROP FUNCTION IF EXISTS sv_hint_bot_user_active();

-- An ai task changed: the bot that owns it re-reads the task and its inbox. The workspace is the
-- project's, else the bot's own. Lease-only updates are not in the column list of the trigger.
CREATE OR REPLACE FUNCTION sv_hint_task() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  v_workspace uuid;
BEGIN
  BEGIN
    SELECT workspace_id INTO v_workspace FROM projects WHERE id = NEW.project_id;
    IF v_workspace IS NULL THEN
      SELECT workspace_id INTO v_workspace FROM workspace_bots WHERE id = NEW.ai_participant_id;
    END IF;
    IF v_workspace IS NOT NULL THEN
      PERFORM pg_notify('sv_change_hint', jsonb_build_object(
        'k', 'task', 'id', NEW.id, 'p', NEW.project_id, 'w', v_workspace, 'b', NEW.ai_participant_id)::text);
    END IF;
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for an ai task (sqlstate %)', SQLSTATE;
  END;
  RETURN NEW;
END $$;

-- A work item was updated or deleted.
CREATE OR REPLACE FUNCTION sv_hint_work_item() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  v_id uuid;
  v_project uuid;
  v_workspace uuid;
BEGIN
  BEGIN
    IF TG_OP = 'DELETE' THEN
      v_id := OLD.id;
      v_project := OLD.project_id;
    ELSE
      v_id := NEW.id;
      v_project := NEW.project_id;
    END IF;
    SELECT workspace_id INTO v_workspace FROM projects WHERE id = v_project;
    IF v_workspace IS NOT NULL THEN
      PERFORM pg_notify('sv_change_hint', jsonb_build_object(
        'k', 'issue', 'id', v_id, 'p', v_project, 'w', v_workspace)::text);
    END IF;
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for a work item (sqlstate %)', SQLSTATE;
  END;
  IF TG_OP = 'DELETE' THEN
    RETURN OLD;
  END IF;
  RETURN NEW;
END $$;

-- A comment was added, changed or deleted: the work item it belongs to changed. When the work item
-- itself is being deleted its project can no longer be resolved, and the work item's own hint
-- covers it.
CREATE OR REPLACE FUNCTION sv_hint_comment() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  v_item uuid;
  v_project uuid;
  v_workspace uuid;
BEGIN
  BEGIN
    IF TG_OP = 'DELETE' THEN
      v_item := OLD.work_item_id;
    ELSE
      v_item := NEW.work_item_id;
    END IF;
    SELECT w.project_id, p.workspace_id INTO v_project, v_workspace
      FROM work_items w JOIN projects p ON p.id = w.project_id WHERE w.id = v_item;
    IF v_workspace IS NOT NULL THEN
      PERFORM pg_notify('sv_change_hint', jsonb_build_object(
        'k', 'issue', 'id', v_item, 'p', v_project, 'w', v_workspace)::text);
    END IF;
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for a comment (sqlstate %)', SQLSTATE;
  END;
  IF TG_OP = 'DELETE' THEN
    RETURN OLD;
  END IF;
  RETURN NEW;
END $$;

-- A project changed its type: the tools a bot may use there change.
CREATE OR REPLACE FUNCTION sv_hint_project_type_key() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  BEGIN
    PERFORM pg_notify('sv_change_hint', jsonb_build_object(
      'k', 'tools', 'p', NEW.id, 'w', NEW.workspace_id)::text);
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for a project type key (sqlstate %)', SQLSTATE;
  END;
  RETURN NEW;
END $$;

-- A project type was saved with capabilities (changed or not): global, no project, no workspace.
CREATE OR REPLACE FUNCTION sv_hint_project_type_caps() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  BEGIN
    PERFORM pg_notify('sv_change_hint', '{"k":"tools"}');
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for project type capabilities (sqlstate %)', SQLSTATE;
  END;
  RETURN NEW;
END $$;

-- What a bot may read changed: a grant, a connection, or the bot's own scope, level or permissions.
-- `bot_project_grants` and `project_bot_connections` carry `bot_id`; `workspace_bots` is the bot.
CREATE OR REPLACE FUNCTION sv_hint_bot_access() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  v_row jsonb;
  v_bot text;
BEGIN
  BEGIN
    IF TG_OP = 'DELETE' THEN
      v_row := to_jsonb(OLD);
    ELSE
      v_row := to_jsonb(NEW);
    END IF;
    IF TG_TABLE_NAME = 'workspace_bots' THEN
      v_bot := v_row ->> 'id';
    ELSE
      v_bot := v_row ->> 'bot_id';
    END IF;
    IF v_bot IS NOT NULL THEN
      PERFORM pg_notify('sv_bot_access_changed', jsonb_build_object('b', v_bot)::text);
    END IF;
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for a bot access change (sqlstate %)', SQLSTATE;
  END;
  IF TG_OP = 'DELETE' THEN
    RETURN OLD;
  END IF;
  RETURN NEW;
END $$;

-- A bot's token (rotated), its state or its expiry changed, or the bot was deleted.
CREATE OR REPLACE FUNCTION sv_hint_bot_token() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE
  v_bot uuid;
BEGIN
  BEGIN
    IF TG_OP = 'DELETE' THEN
      v_bot := OLD.id;
    ELSE
      v_bot := NEW.id;
    END IF;
    PERFORM pg_notify('sv_bot_token_changed', jsonb_build_object('b', v_bot)::text);
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for a bot token change (sqlstate %)', SQLSTATE;
  END;
  IF TG_OP = 'DELETE' THEN
    RETURN OLD;
  END IF;
  RETURN NEW;
END $$;

-- A user was enabled or disabled: when that user is a bot, its token stops or resumes working.
CREATE OR REPLACE FUNCTION sv_hint_bot_user_active() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  BEGIN
    IF EXISTS (SELECT 1 FROM workspace_bots WHERE id = NEW.id) THEN
      PERFORM pg_notify('sv_bot_token_changed', jsonb_build_object('b', NEW.id)::text);
    END IF;
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'change hint skipped for a bot user change (sqlstate %)', SQLSTATE;
  END;
  RETURN NEW;
END $$;

-- The triggers, last in the file so the table locks are held for the shortest tail.
DO $$
DECLARE
  t record;
BEGIN
  SET LOCAL lock_timeout = '10s';

  FOR t IN SELECT * FROM (VALUES
    ('ai_tasks', 'trg_sv_hint_ai_task_insert',
     'CREATE TRIGGER trg_sv_hint_ai_task_insert AFTER INSERT ON ai_tasks
        FOR EACH ROW EXECUTE FUNCTION sv_hint_task()'),
    ('ai_tasks', 'trg_sv_hint_ai_task_update',
     'CREATE TRIGGER trg_sv_hint_ai_task_update
        AFTER UPDATE OF status, attempts, acknowledged_at, plan, last_progress_at, result, error_message, runner_phase
        ON ai_tasks FOR EACH ROW EXECUTE FUNCTION sv_hint_task()'),
    ('work_items', 'trg_sv_hint_work_item_update',
     'CREATE TRIGGER trg_sv_hint_work_item_update AFTER UPDATE ON work_items
        FOR EACH ROW EXECUTE FUNCTION sv_hint_work_item()'),
    ('work_items', 'trg_sv_hint_work_item_delete',
     'CREATE TRIGGER trg_sv_hint_work_item_delete AFTER DELETE ON work_items
        FOR EACH ROW EXECUTE FUNCTION sv_hint_work_item()'),
    ('comments', 'trg_sv_hint_comment',
     'CREATE TRIGGER trg_sv_hint_comment AFTER INSERT OR UPDATE OR DELETE ON comments
        FOR EACH ROW EXECUTE FUNCTION sv_hint_comment()'),
    ('projects', 'trg_sv_hint_project_type_key',
     'CREATE TRIGGER trg_sv_hint_project_type_key AFTER UPDATE OF type_key ON projects
        FOR EACH ROW EXECUTE FUNCTION sv_hint_project_type_key()'),
    ('project_types', 'trg_sv_hint_project_type_caps',
     'CREATE TRIGGER trg_sv_hint_project_type_caps AFTER UPDATE OF enabled_capabilities ON project_types
        FOR EACH ROW EXECUTE FUNCTION sv_hint_project_type_caps()'),
    ('bot_project_grants', 'trg_sv_hint_bot_grants',
     'CREATE TRIGGER trg_sv_hint_bot_grants AFTER INSERT OR UPDATE OR DELETE ON bot_project_grants
        FOR EACH ROW EXECUTE FUNCTION sv_hint_bot_access()'),
    ('project_bot_connections', 'trg_sv_hint_bot_connections',
     'CREATE TRIGGER trg_sv_hint_bot_connections AFTER INSERT OR UPDATE OR DELETE ON project_bot_connections
        FOR EACH ROW EXECUTE FUNCTION sv_hint_bot_access()'),
    ('workspace_bots', 'trg_sv_hint_bot_access',
     'CREATE TRIGGER trg_sv_hint_bot_access
        AFTER UPDATE OF project_scope, default_project_level, permissions ON workspace_bots
        FOR EACH ROW EXECUTE FUNCTION sv_hint_bot_access()'),
    ('workspace_bots', 'trg_sv_hint_bot_token',
     'CREATE TRIGGER trg_sv_hint_bot_token AFTER UPDATE OF token_hash, is_active, expires_at ON workspace_bots
        FOR EACH ROW EXECUTE FUNCTION sv_hint_bot_token()'),
    ('workspace_bots', 'trg_sv_hint_bot_token_delete',
     'CREATE TRIGGER trg_sv_hint_bot_token_delete AFTER DELETE ON workspace_bots
        FOR EACH ROW EXECUTE FUNCTION sv_hint_bot_token()'),
    ('users', 'trg_sv_hint_bot_user_active',
     'CREATE TRIGGER trg_sv_hint_bot_user_active AFTER UPDATE OF is_active ON users
        FOR EACH ROW WHEN (OLD.is_active IS DISTINCT FROM NEW.is_active)
        EXECUTE FUNCTION sv_hint_bot_user_active()')
  ) AS v(tbl, name, ddl)
  LOOP
    IF NOT EXISTS (SELECT 1 FROM pg_trigger
                   WHERE tgrelid = to_regclass(t.tbl) AND tgname = t.name AND NOT tgisinternal) THEN
      EXECUTE t.ddl;
    END IF;
  END LOOP;
END $$;
