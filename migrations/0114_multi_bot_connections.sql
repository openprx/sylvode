-- 0114: several bot connections per project; execution settings fold into the connection.
--
-- A connection is (project, bot) and carries its own enabled flag, channel, CLI and working
-- directory. 0110 and 0112 already create the final shape on a new database, so every statement
-- here is a no-op there. A database that holds the earlier shape (one connection per project,
-- settings in project_execution_settings) is brought forward: the columns are added, the values
-- are copied (the earlier shape has at most one connection per project, so the copy is exact),
-- the primary key becomes (project_id, bot_id), the checks and the one-tmux-connection-per-CLI
-- index are added, and project_execution_settings is dropped.
--
-- Retirement migration: it drops a table, so it is listed in RETIREMENT_MIGRATIONS. It re-runs
-- on every database that has no ledger row for it, so every statement is idempotent and the
-- copy sits behind a to_regclass('project_execution_settings') guard.
-- Rollback: restore the disposable database from the redeploy procedure; there is no down file.

SET LOCAL lock_timeout = '10s';

-- 1. Execution columns. working_directory is added nullable, filled and then made NOT NULL.
ALTER TABLE project_bot_connections
  ADD COLUMN IF NOT EXISTS enabled            BOOLEAN NOT NULL DEFAULT FALSE,
  ADD COLUMN IF NOT EXISTS channel            TEXT    NOT NULL DEFAULT 'tmux',
  ADD COLUMN IF NOT EXISTS cli                TEXT    NOT NULL DEFAULT 'claude',
  ADD COLUMN IF NOT EXISTS working_directory  TEXT,
  ADD COLUMN IF NOT EXISTS on_complete_state  TEXT    NOT NULL DEFAULT 'auto',
  ADD COLUMN IF NOT EXISTS on_fail_label_id   UUID    NULL REFERENCES labels(id) ON DELETE SET NULL,
  ADD COLUMN IF NOT EXISTS approval_gate      TEXT    NOT NULL DEFAULT 'none';

-- 2. Copy the settings of each project into its connection. Only rows that were just added
-- (working_directory still NULL) are touched, so a second run changes nothing.
DO $$
BEGIN
  IF to_regclass('project_execution_settings') IS NOT NULL THEN
    UPDATE project_bot_connections c
       SET enabled           = s.enabled,
           channel           = s.channel,
           cli               = s.cli,
           working_directory = s.working_directory,
           on_complete_state = s.on_complete_state,
           on_fail_label_id  = s.on_fail_label_id,
           approval_gate     = s.approval_gate
      FROM project_execution_settings s
     WHERE s.project_id = c.project_id
       AND c.working_directory IS NULL;
  END IF;
END $$;

-- A connection without a settings row cannot exist in the earlier shape (connect wrote both in
-- one transaction). Should one exist anyway, it stays visible and cannot run: disabled, with a
-- directory that no runner accepts.
UPDATE project_bot_connections
   SET enabled = FALSE, working_directory = '/unconfigured'
 WHERE working_directory IS NULL;

ALTER TABLE project_bot_connections ALTER COLUMN working_directory SET NOT NULL;

-- The tmux channel is claude only; an earlier row naming another CLI could not be delivered
-- (the runner refused it), so it moves to the headless channel before the check is added.
UPDATE project_bot_connections SET channel = 'headless' WHERE channel = 'tmux' AND cli <> 'claude';

-- 3. Primary key (project_id, bot_id).
DO $$
BEGIN
  IF NOT EXISTS (
    SELECT 1 FROM pg_index i
    WHERE i.indrelid = 'project_bot_connections'::regclass AND i.indisprimary AND i.indnatts = 2
  ) THEN
    ALTER TABLE project_bot_connections DROP CONSTRAINT IF EXISTS project_bot_connections_pkey;
    ALTER TABLE project_bot_connections ADD CONSTRAINT project_bot_connections_pkey PRIMARY KEY (project_id, bot_id);
  END IF;
END $$;

-- 4. Checks.
DO $$
BEGIN
  IF NOT EXISTS (SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'project_bot_connections'::regclass AND conname = 'pbc_channel_check') THEN
    ALTER TABLE project_bot_connections ADD CONSTRAINT pbc_channel_check
      CHECK (channel IN ('tmux', 'headless'));
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'project_bot_connections'::regclass AND conname = 'pbc_cli_check') THEN
    ALTER TABLE project_bot_connections ADD CONSTRAINT pbc_cli_check
      CHECK (cli IN ('claude', 'codex', 'opencode'));
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'project_bot_connections'::regclass AND conname = 'pbc_dir_check') THEN
    ALTER TABLE project_bot_connections ADD CONSTRAINT pbc_dir_check CHECK (
          working_directory ~ '^/'
      AND octet_length(working_directory) BETWEEN 2 AND 1024
      AND working_directory !~ '[[:cntrl:]]'
      AND working_directory !~ '(^|/)\.\.?(/|$)'
      AND working_directory !~ '//'
    );
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'project_bot_connections'::regclass AND conname = 'pbc_complete_state_check') THEN
    ALTER TABLE project_bot_connections ADD CONSTRAINT pbc_complete_state_check CHECK (
      on_complete_state IN ('auto', 'none') OR on_complete_state ~ '^state:[a-z0-9_]{1,64}$'
    );
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'project_bot_connections'::regclass AND conname = 'pbc_gate_check') THEN
    ALTER TABLE project_bot_connections ADD CONSTRAINT pbc_gate_check CHECK (approval_gate IN ('none'));
  END IF;
  IF NOT EXISTS (SELECT 1 FROM pg_constraint
                 WHERE conrelid = 'project_bot_connections'::regclass AND conname = 'pbc_tmux_cli_check') THEN
    ALTER TABLE project_bot_connections ADD CONSTRAINT pbc_tmux_cli_check
      CHECK (channel <> 'tmux' OR cli = 'claude');
  END IF;
END $$;

-- 5. At most one tmux connection per CLI type in a project.
CREATE UNIQUE INDEX IF NOT EXISTS pbc_one_tmux_per_cli
  ON project_bot_connections (project_id, cli) WHERE channel = 'tmux';
CREATE INDEX IF NOT EXISTS idx_project_bot_connections_bot ON project_bot_connections (bot_id);

-- 6. The settings now live in the connection row.
DROP TABLE IF EXISTS project_execution_settings;
