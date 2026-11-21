-- Alert chain: inbound sources, alert rules, alerts and inbound events.
--
-- Four new tables, no change to any existing table. They reference `workspaces`, `projects`,
-- `users`, `work_items` and `ai_tasks` by foreign key only.
--
-- Safe on a database that already applied every earlier migration, on a fresh one, and safe to run
-- again: every table and index is created only when missing, and every CHECK constraint is added
-- in a DO block only when the table does not carry a constraint of that name yet, so a table left
-- behind by an interrupted earlier run is completed rather than rejected.
--
-- Locks on existing tables. Creating a foreign key takes SHARE ROW EXCLUSIVE on the referenced
-- table, which blocks writes to it (reads continue) until the migration transaction commits. The
-- lock is held for catalogue work only. A lock timeout keeps a long transaction that is already
-- writing to one of those tables from stalling every later writer behind the queued statement:
-- the migration fails after the timeout and is retried by the next start, with no partial effect.
-- A re-run creates nothing and takes no lock on the referenced tables.
--
-- Rollback is an operator action and is not run automatically: remove the four tables in the
-- reverse order of their creation (events, alerts, rules, sources). Work items, tasks and
-- notifications that alerts created are ordinary rows and stay.

DO $mig$
BEGIN
  SET LOCAL lock_timeout = '10s';
END
$mig$;

CREATE TABLE IF NOT EXISTS inbound_sources (
  id                      UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id            UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id              UUID        NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name                    TEXT        NOT NULL,
  format                  TEXT        NOT NULL DEFAULT 'native_v1',
  format_settings         JSONB       NOT NULL DEFAULT '{}'::jsonb,
  token_hash              CHAR(64)    NOT NULL UNIQUE,
  token_prefix            TEXT        NOT NULL,
  signing_secret          TEXT        NULL,
  require_signature       BOOLEAN     NOT NULL DEFAULT TRUE,
  ip_allowlist            CIDR[]      NOT NULL DEFAULT '{}',
  acting_user_id          UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  enabled                 BOOLEAN     NOT NULL DEFAULT TRUE,
  rate_per_minute         INTEGER     NOT NULL DEFAULT 120,
  daily_cap               INTEGER     NOT NULL DEFAULT 20000,
  last_used_at            TIMESTAMPTZ NULL,
  last_used_ip            INET        NULL,
  last_throttle_notice_at TIMESTAMPTZ NULL,
  created_by              UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at              TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_inbound_sources_project ON inbound_sources (project_id);

CREATE TABLE IF NOT EXISTS alert_rules (
  id                          UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  workspace_id                UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id                  UUID        NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name                        TEXT        NOT NULL,
  position                    INTEGER     NOT NULL,
  enabled                     BOOLEAN     NOT NULL DEFAULT TRUE,
  source_id                   UUID        NULL REFERENCES inbound_sources(id) ON DELETE CASCADE,
  kind_patterns               TEXT[]      NOT NULL DEFAULT '{}',
  min_severity                TEXT        NOT NULL DEFAULT 'high',
  action                      TEXT        NOT NULL DEFAULT 'work_item_and_task',
  bot_user_id                 UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  instructions                TEXT        NOT NULL DEFAULT '',
  priority_map                JSONB       NOT NULL DEFAULT
    '{"critical":"urgent","high":"high","medium":"medium","low":"low","info":"low"}'::jsonb,
  label_ids                   UUID[]      NOT NULL DEFAULT '{}',
  notify_user_ids             UUID[]      NOT NULL DEFAULT '{}',
  max_tasks_per_hour          INTEGER     NOT NULL DEFAULT 6,
  max_active_tasks            INTEGER     NOT NULL DEFAULT 2,
  dedup_window_secs           INTEGER     NOT NULL DEFAULT 3600,
  rearm_after_secs            INTEGER     NOT NULL DEFAULT 900,
  task_max_attempts           INTEGER     NOT NULL DEFAULT 3,
  last_suppression_notice_at  TIMESTAMPTZ NULL,
  version                     INTEGER     NOT NULL DEFAULT 1,
  created_by                  UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  updated_by                  UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_alert_rules_project ON alert_rules (project_id, position, id) WHERE enabled;

CREATE TABLE IF NOT EXISTS inbound_alerts (
  id                       UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  source_id                UUID        NOT NULL REFERENCES inbound_sources(id) ON DELETE CASCADE,
  project_id               UUID        NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  rule_id                  UUID        NULL REFERENCES alert_rules(id) ON DELETE SET NULL,
  dedup_key                TEXT        NOT NULL,
  work_item_id             UUID        NOT NULL REFERENCES work_items(id) ON DELETE CASCADE,
  max_severity_rank        SMALLINT    NOT NULL,
  occurrences              INTEGER     NOT NULL DEFAULT 1,
  first_seen_at            TIMESTAMPTZ NOT NULL,
  last_seen_at             TIMESTAMPTZ NOT NULL,
  task_seq                 INTEGER     NOT NULL DEFAULT 0,
  task_id                  UUID        NULL REFERENCES ai_tasks(id) ON DELETE SET NULL,
  task_state               TEXT        NOT NULL DEFAULT 'none',
  task_ended_at            TIMESTAMPTZ NULL,
  outcome_notified_task_id UUID        NULL,
  open                     BOOLEAN     NOT NULL DEFAULT TRUE,
  created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at               TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_inbound_alerts_open ON inbound_alerts (source_id, dedup_key) WHERE open;
CREATE UNIQUE INDEX IF NOT EXISTS ux_inbound_alerts_work_item ON inbound_alerts (work_item_id);
CREATE INDEX IF NOT EXISTS idx_inbound_alerts_task ON inbound_alerts (task_id) WHERE task_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_inbound_alerts_rule ON inbound_alerts (rule_id) WHERE rule_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS inbound_events (
  id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  source_id     UUID        NOT NULL REFERENCES inbound_sources(id) ON DELETE CASCADE,
  workspace_id  UUID        NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  project_id    UUID        NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  event_id      TEXT        NOT NULL,
  body_sha256   CHAR(64)    NOT NULL,
  kind          TEXT        NOT NULL,
  severity_rank SMALLINT    NOT NULL,
  dedup_key     TEXT        NOT NULL,
  occurred_at   TIMESTAMPTZ NOT NULL,
  received_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  title         TEXT        NOT NULL,
  summary       TEXT        NOT NULL DEFAULT '',
  subject       JSONB       NOT NULL DEFAULT '{}'::jsonb,
  payload       JSONB       NOT NULL DEFAULT '{}'::jsonb,
  signed        BOOLEAN     NOT NULL,
  client_ip     INET        NULL,
  outcome       TEXT        NOT NULL,
  rule_id       UUID        NULL REFERENCES alert_rules(id) ON DELETE SET NULL,
  alert_id      UUID        NULL REFERENCES inbound_alerts(id) ON DELETE SET NULL,
  task_id       UUID        NULL REFERENCES ai_tasks(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_inbound_events_source_time ON inbound_events (source_id, received_at DESC);
CREATE INDEX IF NOT EXISTS idx_inbound_events_rule_time   ON inbound_events (rule_id, received_at) WHERE task_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_inbound_events_rule_opened ON inbound_events (rule_id, received_at) WHERE outcome = 'alert_opened';
CREATE INDEX IF NOT EXISTS idx_inbound_events_received    ON inbound_events (received_at);

-- Constraints, each added only when the table does not carry one of that name.
DO $mig$
DECLARE
  spec RECORD;
BEGIN
  SET LOCAL lock_timeout = '10s';

  FOR spec IN
    SELECT * FROM (VALUES
      ('inbound_sources', 'inbound_sources_name_check',
       $d$CHECK (char_length(name) BETWEEN 1 AND 100)$d$),
      ('inbound_sources', 'inbound_sources_format_check',
       $d$CHECK (format IN ('native_v1', 'prx_waf_v1', 'prx_sd_generic_v1'))$d$),
      ('inbound_sources', 'inbound_sources_rate_check',
       $d$CHECK (rate_per_minute BETWEEN 1 AND 6000 AND daily_cap BETWEEN 1 AND 1000000)$d$),
      ('inbound_sources', 'inbound_sources_sig_check',
       $d$CHECK (NOT require_signature OR signing_secret IS NOT NULL)$d$),
      ('alert_rules', 'alert_rules_name_check',
       $d$CHECK (char_length(name) BETWEEN 1 AND 100)$d$),
      ('alert_rules', 'alert_rules_severity_check',
       $d$CHECK (min_severity IN ('info', 'low', 'medium', 'high', 'critical'))$d$),
      ('alert_rules', 'alert_rules_action_check',
       $d$CHECK (action IN ('work_item', 'work_item_and_task'))$d$),
      ('alert_rules', 'alert_rules_instr_check',
       $d$CHECK (char_length(instructions) <= 4000)$d$),
      ('alert_rules', 'alert_rules_caps_check',
       $d$CHECK (max_tasks_per_hour BETWEEN 1 AND 120 AND max_active_tasks BETWEEN 1 AND 20
          AND dedup_window_secs BETWEEN 60 AND 604800 AND rearm_after_secs BETWEEN 0 AND 86400
          AND task_max_attempts BETWEEN 1 AND 10)$d$),
      ('alert_rules', 'alert_rules_patterns_check',
       $d$CHECK (cardinality(kind_patterns) <= 32)$d$),
      ('inbound_alerts', 'inbound_alerts_task_state_check',
       $d$CHECK (task_state IN ('none', 'created', 'suppressed', 'waiting'))$d$),
      ('inbound_alerts', 'inbound_alerts_rank_check',
       $d$CHECK (max_severity_rank BETWEEN 0 AND 4)$d$),
      ('inbound_events', 'inbound_events_unique_id',
       $d$UNIQUE (source_id, event_id)$d$),
      ('inbound_events', 'inbound_events_rank_check',
       $d$CHECK (severity_rank BETWEEN 0 AND 4)$d$),
      ('inbound_events', 'inbound_events_outcome_check',
       $d$CHECK (outcome IN ('no_rule', 'alert_opened', 'alert_attached', 'suppressed_task_cap',
          'suppressed_active_cap', 'suppressed_workspace_cap', 'suppressed_alert_cap', 'bot_unavailable'))$d$)
    ) AS t(tbl, conname, definition)
  LOOP
    IF NOT EXISTS (SELECT 1 FROM pg_constraint
                   WHERE conrelid = to_regclass(spec.tbl) AND conname = spec.conname) THEN
      EXECUTE format('ALTER TABLE %I ADD CONSTRAINT %I %s', spec.tbl, spec.conname, spec.definition);
    END IF;
  END LOOP;
END
$mig$;
