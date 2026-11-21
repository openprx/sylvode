-- OAuth authorization server for the API and the MCP server.
--
-- Twelve tables (clients, per-workspace delegation settings, authorization requests, grants and
-- their project sets, authorization codes, tokens, audit events, operation logs, rate windows,
-- the event outbox and the event feed), one seeded client row, and four triggers that append to
-- the event outbox when a notification or an AI task changes.
--
-- Safe on a database that already applied every earlier migration and on a fresh one, and safe to
-- run again: every table and index is created only when missing, the seed row is inserted only
-- when absent, the two trigger functions are replaced in place, and a trigger is created only when
-- the table does not carry it yet.
--
-- Locks on existing tables. Nothing here rewrites, scans or alters `notifications` or `ai_tasks`.
-- Each CREATE TRIGGER takes SHARE ROW EXCLUSIVE on its table, which blocks writes to that table
-- (reads continue) from the moment it is granted until the migration transaction commits, and it
-- is held only for catalogue work, so a few milliseconds. The two statements are the last ones in
-- the file so the locks are held for as short a tail as possible. A lock timeout keeps a long
-- transaction that is already writing to either table from stalling every later writer behind
-- the queued CREATE TRIGGER: the migration fails after the timeout and is retried by the next
-- start, with no partial effect. Running the file again takes no table lock at all, because the
-- existence check reads `pg_trigger` and the trigger is not created a second time.
--
-- Trigger failure policy. The outbox is a convenience for a downstream consumer, never a
-- precondition of the write that fed it. Each trigger body therefore runs its outbox insert in a
-- nested block that traps every error and reports it as a warning carrying only the SQLSTATE (the
-- message of the underlying error can quote row values, so it is not repeated). A full disk, a
-- renamed table or a failing constraint on the outbox can not make a notification or a task state
-- change fail. The cost is one subtransaction per trigger call, which these low-volume tables can
-- afford; the consumer detects a lost row by its absence, and the warning marks the time.
--
-- Operational rollback without a schema change: remove `[oauth] issuer` from the configuration
-- or disable the client. Removing the schema is documented in
-- docs/runbooks/oauth-authorization-server.md.

CREATE TABLE IF NOT EXISTS oauth_clients (
  client_id                  TEXT        PRIMARY KEY,
  display_name               TEXT        NOT NULL,
  description                TEXT        NOT NULL DEFAULT '',
  homepage_url               TEXT        NULL,
  client_type                TEXT        NOT NULL,
  token_endpoint_auth_method TEXT        NOT NULL,
  redirect_uris              TEXT[]      NOT NULL,
  allowed_scopes             TEXT[]      NOT NULL,
  allowed_resources          TEXT[]      NOT NULL,
  embed_origins              TEXT[]      NOT NULL DEFAULT '{}',
  status                     TEXT        NOT NULL DEFAULT 'active',
  status_changed_by          UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  status_changed_at          TIMESTAMPTZ NULL,
  created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT oauth_clients_id_check       CHECK (client_id ~ '^[a-z][a-z0-9_-]{2,63}$'),
  CONSTRAINT oauth_clients_type_check     CHECK (client_type IN ('public', 'confidential')),
  CONSTRAINT oauth_clients_auth_check     CHECK (token_endpoint_auth_method IN ('none')),
  CONSTRAINT oauth_clients_status_check   CHECK (status IN ('active', 'disabled')),
  CONSTRAINT oauth_clients_redirect_check CHECK (cardinality(redirect_uris) BETWEEN 1 AND 10),
  CONSTRAINT oauth_clients_resource_check CHECK (allowed_resources <@ ARRAY['api', 'mcp']::TEXT[])
);
-- allowed_resources holds symbolic names; 'api' and 'mcp' resolve to the configured
-- oauth.api_resource and oauth.mcp_resource, so the seed is valid on every deployment.

-- The first release grants what the reference scenario needs: reading projects and work items,
-- notifications and task state, plus the right to stay connected. Write scopes and forms access
-- are part of the vocabulary but are not offered to this client yet; widening the list is a
-- deliberate later change (a client row is never overwritten by this file).
INSERT INTO oauth_clients (client_id, display_name, description, homepage_url, client_type,
  token_endpoint_auth_method, redirect_uris, allowed_scopes, allowed_resources)
VALUES ('ramflux', 'Ramflux', 'Ramflux desktop', NULL, 'public', 'none',
  ARRAY['http://127.0.0.1/oauth/sylvode/callback', 'http://[::1]/oauth/sylvode/callback'],
  ARRAY['projects:read','work_items:read','notifications:read','tasks:read','offline_access'],
  ARRAY['api','mcp'])
ON CONFLICT (client_id) DO NOTHING;

CREATE TABLE IF NOT EXISTS workspace_delegation_settings (
  workspace_id          UUID        PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  allow_connected_apps  BOOLEAN     NOT NULL DEFAULT TRUE,
  updated_by            UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- No row means allowed.

CREATE TABLE IF NOT EXISTS oauth_authorization_requests (
  id                   UUID        PRIMARY KEY,
  client_id            TEXT        NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
  redirect_uri         TEXT        NOT NULL,
  state                TEXT        NOT NULL,
  code_challenge       TEXT        NOT NULL,
  requested_scopes     TEXT[]      NOT NULL,
  resources            TEXT[]      NOT NULL,
  device_name          TEXT        NULL,
  workspace_hint       UUID        NULL,
  consent_token_hash   CHAR(64)    NULL,
  consent_user_id      UUID        NULL REFERENCES users(id) ON DELETE CASCADE,
  status               TEXT        NOT NULL DEFAULT 'pending',
  created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at           TIMESTAMPTZ NOT NULL,
  CONSTRAINT oauth_ar_status_check CHECK (status IN ('pending', 'decided')),
  CONSTRAINT oauth_ar_state_check  CHECK (octet_length(state) BETWEEN 16 AND 512),
  CONSTRAINT oauth_ar_device_check CHECK (device_name IS NULL OR char_length(device_name) BETWEEN 1 AND 64)
);
CREATE INDEX IF NOT EXISTS idx_oauth_ar_expires ON oauth_authorization_requests (expires_at);

CREATE TABLE IF NOT EXISTS oauth_grants (
  id                    UUID        PRIMARY KEY,
  client_id             TEXT        NOT NULL REFERENCES oauth_clients(client_id) ON DELETE CASCADE,
  user_id               UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  workspace_id          UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  scopes                TEXT[]      NOT NULL,
  resources             TEXT[]      NOT NULL,
  project_mode          TEXT        NOT NULL,
  device_name           TEXT        NULL,
  user_session_version  BIGINT      NOT NULL,
  status                TEXT        NOT NULL DEFAULT 'active',
  consented_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at            TIMESTAMPTZ NOT NULL,
  last_used_at          TIMESTAMPTZ NULL,
  revoked_at            TIMESTAMPTZ NULL,
  revoked_by            UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  revoke_reason         TEXT        NULL,
  CONSTRAINT oauth_grants_mode_check   CHECK (project_mode IN ('all', 'selected')),
  CONSTRAINT oauth_grants_status_check CHECK (status IN ('active', 'revoked')),
  CONSTRAINT oauth_grants_revoked_check CHECK ((status = 'revoked') = (revoked_at IS NOT NULL)),
  CONSTRAINT oauth_grants_reason_check CHECK (revoke_reason IS NULL OR revoke_reason IN (
    'user_revoked','client_revoked','workspace_admin_revoked','workspace_disabled','password_changed',
    'account_disabled','membership_removed','refresh_reuse','code_replay','client_disabled','expired'))
);
CREATE INDEX IF NOT EXISTS idx_oauth_grants_user_active
  ON oauth_grants (user_id, client_id) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS idx_oauth_grants_workspace_active
  ON oauth_grants (workspace_id) WHERE status = 'active';

CREATE TABLE IF NOT EXISTS oauth_grant_projects (
  grant_id   UUID NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
  project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  PRIMARY KEY (grant_id, project_id)
);
CREATE INDEX IF NOT EXISTS idx_oauth_grant_projects_project ON oauth_grant_projects (project_id);

CREATE TABLE IF NOT EXISTS oauth_authorization_codes (
  code_hash       CHAR(64)    PRIMARY KEY,
  grant_id        UUID        NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
  client_id       TEXT        NOT NULL,
  redirect_uri    TEXT        NOT NULL,
  code_challenge  TEXT        NOT NULL,
  resources       TEXT[]      NOT NULL,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at      TIMESTAMPTZ NOT NULL,
  consumed_at     TIMESTAMPTZ NULL
);

CREATE TABLE IF NOT EXISTS oauth_tokens (
  id            UUID        PRIMARY KEY,
  token_hash    CHAR(64)    NOT NULL UNIQUE,
  kind          TEXT        NOT NULL,
  grant_id      UUID        NOT NULL REFERENCES oauth_grants(id) ON DELETE CASCADE,
  client_id     TEXT        NOT NULL,
  resource      TEXT        NULL,
  scopes        TEXT[]      NOT NULL,
  parent_id     UUID        NULL REFERENCES oauth_tokens(id) ON DELETE SET NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  expires_at    TIMESTAMPTZ NOT NULL,
  consumed_at   TIMESTAMPTZ NULL,
  revoked_at    TIMESTAMPTZ NULL,
  last_used_at  TIMESTAMPTZ NULL,
  CONSTRAINT oauth_tokens_kind_check     CHECK (kind IN ('access', 'refresh')),
  CONSTRAINT oauth_tokens_resource_check CHECK ((kind = 'access') = (resource IS NOT NULL)),
  CONSTRAINT oauth_tokens_resource_value CHECK (resource IS NULL OR resource IN ('api', 'mcp')),
  CONSTRAINT oauth_tokens_consumed_check CHECK (consumed_at IS NULL OR kind = 'refresh')
);
CREATE INDEX IF NOT EXISTS idx_oauth_tokens_grant ON oauth_tokens (grant_id);
CREATE INDEX IF NOT EXISTS idx_oauth_tokens_expires ON oauth_tokens (expires_at);

CREATE TABLE IF NOT EXISTS oauth_audit_events (
  id            UUID        PRIMARY KEY,
  occurred_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  event         TEXT        NOT NULL,
  client_id     TEXT        NULL,
  grant_id      UUID        NULL,
  token_id      UUID        NULL,
  user_id       UUID        NULL,
  workspace_id  UUID        NULL,
  actor_id      UUID        NULL,
  client_ip     INET        NULL,
  user_agent    TEXT        NULL,
  detail        JSONB       NOT NULL DEFAULT '{}'::jsonb,
  CONSTRAINT oauth_audit_event_check  CHECK (event ~ '^[a-z][a-z_]{2,63}$'),
  CONSTRAINT oauth_audit_detail_check CHECK (jsonb_typeof(detail) = 'object'),
  CONSTRAINT oauth_audit_ua_check     CHECK (user_agent IS NULL OR char_length(user_agent) <= 256)
);
-- No foreign keys on purpose: audit rows outlive the grants, users and tokens they describe.
CREATE INDEX IF NOT EXISTS idx_oauth_audit_grant ON oauth_audit_events (grant_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_oauth_audit_user  ON oauth_audit_events (user_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS idx_oauth_audit_time  ON oauth_audit_events (occurred_at);

CREATE TABLE IF NOT EXISTS oauth_operation_logs (
  id             UUID        PRIMARY KEY,
  grant_id       UUID        NOT NULL,
  client_id      TEXT        NOT NULL,
  user_id        UUID        NOT NULL,
  workspace_id   UUID        NOT NULL,
  surface        TEXT        NOT NULL,
  tool_name      TEXT        NULL,
  method         TEXT        NOT NULL,
  path           TEXT        NOT NULL,
  business_code  INTEGER     NOT NULL,
  outcome        TEXT        NOT NULL,
  error_message  TEXT        NULL,
  duration_ms    BIGINT      NOT NULL,
  request_id     UUID        NOT NULL,
  created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT oauth_oplog_outcome_check CHECK (outcome IN ('ok', 'error')),
  CONSTRAINT oauth_oplog_surface_check CHECK (surface IN ('rest', 'mcp_http'))
);
CREATE INDEX IF NOT EXISTS idx_oauth_oplog_grant ON oauth_operation_logs (grant_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_oauth_oplog_time  ON oauth_operation_logs (created_at);

CREATE TABLE IF NOT EXISTS oauth_rate_hits (
  bucket          TEXT        NOT NULL,
  key_hash        CHAR(64)    NOT NULL,
  window_started  TIMESTAMPTZ NOT NULL,
  hit_count       INTEGER     NOT NULL,
  PRIMARY KEY (bucket, key_hash)
);

CREATE TABLE IF NOT EXISTS oauth_event_outbox (
  id            BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  source        TEXT        NOT NULL,
  source_id     UUID        NOT NULL,
  event_type    TEXT        NOT NULL,
  workspace_id  UUID        NOT NULL,
  project_id    UUID        NULL,
  user_id       UUID        NULL,
  detail        JSONB       NOT NULL DEFAULT '{}'::jsonb,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT oauth_outbox_source_check CHECK (source IN ('notification', 'ai_task')),
  CONSTRAINT oauth_outbox_type_check   CHECK (event_type IN
    ('notification.created', 'notification.read', 'task.created', 'task.status_changed'))
);

CREATE TABLE IF NOT EXISTS oauth_event_feed (
  seq           BIGINT      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
  event_type    TEXT        NOT NULL,
  workspace_id  UUID        NOT NULL,
  project_id    UUID        NULL,
  user_id       UUID        NULL,
  subject       JSONB       NOT NULL,
  occurred_at   TIMESTAMPTZ NOT NULL,
  created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT oauth_feed_subject_check CHECK (jsonb_typeof(subject) = 'object')
);
CREATE INDEX IF NOT EXISTS idx_oauth_feed_workspace_seq ON oauth_event_feed (workspace_id, seq);
CREATE INDEX IF NOT EXISTS idx_oauth_feed_created ON oauth_event_feed (created_at);

-- Trigger functions. They read only NEW and OLD and run constant SQL. `pg_notify` inside a
-- trigger is delivered when the transaction commits, so the wake-up never precedes the data and
-- is never sent for a rolled-back write. Replacing a function takes no lock on the tables that
-- use it.
CREATE OR REPLACE FUNCTION oauth_outbox_notification() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
  IF NEW.workspace_id IS NULL THEN
    RETURN NEW;
  END IF;
  BEGIN
    IF TG_OP = 'INSERT' THEN
      INSERT INTO oauth_event_outbox (source, source_id, event_type, workspace_id, project_id, user_id)
      VALUES ('notification', NEW.id, 'notification.created', NEW.workspace_id,
              COALESCE(NEW.related_project_id,
                       (SELECT w.project_id FROM work_items w WHERE w.id = NEW.related_issue_id)),
              NEW.user_id);
    ELSIF OLD.is_read = false AND NEW.is_read = true THEN
      INSERT INTO oauth_event_outbox (source, source_id, event_type, workspace_id, project_id, user_id)
      VALUES ('notification', NEW.id, 'notification.read', NEW.workspace_id,
              COALESCE(NEW.related_project_id,
                       (SELECT w.project_id FROM work_items w WHERE w.id = NEW.related_issue_id)),
              NEW.user_id);
    ELSE
      RETURN NEW;
    END IF;
    PERFORM pg_notify('sv_oauth_outbox', '');
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'oauth event outbox insert skipped for a notification (sqlstate %)', SQLSTATE;
  END;
  RETURN NEW;
END $$;

CREATE OR REPLACE FUNCTION oauth_outbox_ai_task() RETURNS trigger LANGUAGE plpgsql AS $$
DECLARE ws UUID;
BEGIN
  BEGIN
    SELECT p.workspace_id INTO ws FROM projects p WHERE p.id = NEW.project_id;
    IF ws IS NULL THEN
      RETURN NEW;
    END IF;
    IF TG_OP = 'INSERT' THEN
      INSERT INTO oauth_event_outbox (source, source_id, event_type, workspace_id, project_id, detail)
      VALUES ('ai_task', NEW.id, 'task.created', ws, NEW.project_id, jsonb_build_object('to', NEW.status));
    ELSIF OLD.status IS DISTINCT FROM NEW.status THEN
      INSERT INTO oauth_event_outbox (source, source_id, event_type, workspace_id, project_id, detail)
      VALUES ('ai_task', NEW.id, 'task.status_changed', ws, NEW.project_id,
              jsonb_build_object('from', OLD.status, 'to', NEW.status, 'attempts', NEW.attempts));
    ELSE
      RETURN NEW;
    END IF;
    PERFORM pg_notify('sv_oauth_outbox', '');
  EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'oauth event outbox insert skipped for an ai task (sqlstate %)', SQLSTATE;
  END;
  RETURN NEW;
END $$;

-- The four triggers, last in the file so the table locks they take are held for the shortest
-- possible tail of the migration transaction (see the header). The WHEN clauses keep the function
-- from being called at all for writes that can not produce an event.
DO $$
BEGIN
  SET LOCAL lock_timeout = '10s';

  IF NOT EXISTS (SELECT 1 FROM pg_trigger
                 WHERE tgrelid = to_regclass('notifications')
                   AND tgname = 'trg_oauth_outbox_notification_insert' AND NOT tgisinternal) THEN
    CREATE TRIGGER trg_oauth_outbox_notification_insert AFTER INSERT ON notifications
      FOR EACH ROW WHEN (NEW.workspace_id IS NOT NULL)
      EXECUTE FUNCTION oauth_outbox_notification();
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_trigger
                 WHERE tgrelid = to_regclass('notifications')
                   AND tgname = 'trg_oauth_outbox_notification_read' AND NOT tgisinternal) THEN
    CREATE TRIGGER trg_oauth_outbox_notification_read AFTER UPDATE OF is_read ON notifications
      FOR EACH ROW WHEN (OLD.is_read = false AND NEW.is_read = true)
      EXECUTE FUNCTION oauth_outbox_notification();
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_trigger
                 WHERE tgrelid = to_regclass('ai_tasks')
                   AND tgname = 'trg_oauth_outbox_ai_task_insert' AND NOT tgisinternal) THEN
    CREATE TRIGGER trg_oauth_outbox_ai_task_insert AFTER INSERT ON ai_tasks
      FOR EACH ROW EXECUTE FUNCTION oauth_outbox_ai_task();
  END IF;

  IF NOT EXISTS (SELECT 1 FROM pg_trigger
                 WHERE tgrelid = to_regclass('ai_tasks')
                   AND tgname = 'trg_oauth_outbox_ai_task_status' AND NOT tgisinternal) THEN
    CREATE TRIGGER trg_oauth_outbox_ai_task_status AFTER UPDATE OF status ON ai_tasks
      FOR EACH ROW WHEN (OLD.status IS DISTINCT FROM NEW.status)
      EXECUTE FUNCTION oauth_outbox_ai_task();
  END IF;
END $$;
