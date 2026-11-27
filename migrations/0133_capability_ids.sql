-- Capability ids of the operation catalogue (contract C1 section 2.6) and the stored-data half of the
-- containment of the old plugin system.
--
-- The ids are written at every API start from the catalogue in the code; the grant tables that
-- reference them arrive with 0138.
--
-- Safe to run again: the table is created only when missing, and both updates reach a fixed point.

CREATE TABLE IF NOT EXISTS capability_ids (
    id          TEXT PRIMARY KEY
                CHECK (id ~ '^[a-z][a-z0-9_]{0,31}\.[a-z][a-z0-9_]{0,47}\.[a-z][a-z0-9_]{0,31}$'),
    risk        TEXT NOT NULL CHECK (risk IN ('read','write','destructive','admin','secret')),
    plugin_exposed BOOLEAN NOT NULL DEFAULT false,
    -- SHA-256 of the sorted plugin-callable op keys of the id; a grant made against another digest is
    -- not honoured until it is approved again.
    plugin_ops_digest TEXT CHECK (plugin_ops_digest ~ '^sha256:[0-9a-f]{64}$'),
    first_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Legacy invocation payloads never stay in the database: the log keeps digests from now on.
UPDATE plugin_invocations SET input = '{}'::jsonb, output = '{}'::jsonb, error_message = NULL
 WHERE input <> '{}'::jsonb OR output <> '{}'::jsonb OR error_message IS NOT NULL;

-- Old-system plugins installed by a member or a bot (a bot install stores installed_by = NULL), or by
-- someone who is no longer a workspace owner or admin, stop running. An owner or admin re-enables
-- one with PATCH /plugins/{id}.
DO $$
DECLARE
    disabled_count BIGINT;
BEGIN
    WITH disabled AS (
        UPDATE plugins p SET status = 'disabled', updated_at = now()
         WHERE p.status = 'active'
           AND (p.installed_by IS NULL
                OR NOT EXISTS (SELECT 1 FROM workspace_members m
                                WHERE m.workspace_id = p.workspace_id
                                  AND m.user_id = p.installed_by
                                  AND m.role IN ('owner', 'admin')))
        RETURNING 1
    )
    SELECT count(*) INTO disabled_count FROM disabled;
    RAISE NOTICE 'disabled % old-system plugin(s) not installed by a current workspace owner or admin', disabled_count;
END $$;
