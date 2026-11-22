-- 0112: project bot connections.
--
-- Links a bot (token, managed webhook, runner address) to the projects it serves, makes an AI
-- participant registration per project, and drops the stored-only participant fields model,
-- provider, api_endpoint and capabilities. Re-runs on every adopted database (past
-- MIGRATION_ADOPTION_CUTOFF): every statement is guarded. Rollback and renumbering:
-- docs/runbooks/bot-connections.md.

SET LOCAL lock_timeout = '10s';

-- 1. The config-version trigger stops naming the columns dropped below (0092 lists the same).
CREATE OR REPLACE FUNCTION touch_ai_participant_config_version() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
 NEW.config_updated_at:=GREATEST(clock_timestamp(),OLD.config_updated_at+interval '1 microsecond');
 RETURN NEW;
END $$;
DROP TRIGGER IF EXISTS ai_participant_config_version ON ai_participants;
CREATE TRIGGER ai_participant_config_version BEFORE UPDATE OF
 name,domain_overrides,max_domain_level,
 can_veto_human_consensus,reason_min_length,is_active
 ON ai_participants FOR EACH ROW EXECUTE FUNCTION touch_ai_participant_config_version();

-- 2. Stored-only fields (CONNECT-A-BOT-RULES.md section 2 and question Q3).
ALTER TABLE ai_participants
  DROP COLUMN IF EXISTS model,
  DROP COLUMN IF EXISTS provider,
  DROP COLUMN IF EXISTS api_endpoint,
  DROP COLUMN IF EXISTS capabilities;

-- 3. One registration per bot per project. Learning records keep no foreign key to the
-- participant: the id alone is no longer unique, and the records outlive a removed connection.
ALTER TABLE ai_learning_records DROP CONSTRAINT IF EXISTS ai_learning_records_ai_participant_id_fkey;
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_index i
    WHERE i.indrelid = 'ai_participants'::regclass AND i.indisprimary
      AND i.indnatts = 2
  ) THEN
    ALTER TABLE ai_participants DROP CONSTRAINT IF EXISTS ai_participants_pkey;
    ALTER TABLE ai_participants ADD CONSTRAINT ai_participants_pkey PRIMARY KEY (project_id, id);
  END IF;
END $$;
CREATE INDEX IF NOT EXISTS idx_ai_participants_id ON ai_participants (id);

-- 4. Shared per bot.
CREATE TABLE IF NOT EXISTS bot_runners (
  bot_id                 UUID        PRIMARY KEY REFERENCES workspace_bots(id) ON DELETE CASCADE,
  workspace_id           UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  webhook_id             UUID        NOT NULL UNIQUE REFERENCES webhooks(id),
  runner_url             TEXT        NOT NULL,
  credentials_issued_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  report                 JSONB       NULL,
  reported_at            TIMESTAMPTZ NULL,
  created_by             UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
  CONSTRAINT bot_runners_url_check CHECK (
        runner_url ~ '^https?://[^/?#@[:space:]]+(/[^?#[:space:]]*)?$'
    AND runner_url !~ '/$'
    AND octet_length(runner_url) <= 2048
  ),
  CONSTRAINT bot_runners_report_check CHECK (
    report IS NULL OR (jsonb_typeof(report) = 'object' AND octet_length(report::text) <= 16384)
  )
);

-- 5. Per project.
CREATE TABLE IF NOT EXISTS project_bot_connections (
  project_id   UUID        PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  bot_id       UUID        NOT NULL REFERENCES bot_runners(bot_id) ON DELETE CASCADE,
  version      INTEGER     NOT NULL DEFAULT 1,
  created_by   UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  updated_by   UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_project_bot_connections_bot ON project_bot_connections (bot_id);

-- 6. A runner bot is a member of its workspace and never more: its token runs agents on untrusted
-- text, so no path may seat it as admin or owner (the Members API refuses it too, with a 409).
CREATE OR REPLACE FUNCTION workspace_members_runner_bot_seat() RETURNS trigger AS $fn$
BEGIN
  IF NEW.role <> 'member' AND EXISTS (SELECT 1 FROM bot_runners WHERE bot_id = NEW.user_id) THEN
    RAISE EXCEPTION 'a runner bot is seated as member only' USING ERRCODE = '23514';
  END IF;
  RETURN NEW;
END
$fn$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_workspace_members_runner_bot_seat ON workspace_members;
CREATE TRIGGER trg_workspace_members_runner_bot_seat
  BEFORE INSERT OR UPDATE OF role ON workspace_members
  FOR EACH ROW EXECUTE FUNCTION workspace_members_runner_bot_seat();
