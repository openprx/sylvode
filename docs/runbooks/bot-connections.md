# Bot connections

Operating notes for migration `0112_bot_connections.sql`: the schema under "Connect a bot".

## What migration 0112 changes

- `ai_participants` is keyed by `(project_id, id)` instead of `id` alone, so one bot can be an AI
  participant of several projects, each with its own governance level. A plain index on `id` keeps
  the per-bot lookups fast.
- The stored-only columns `model`, `provider`, `api_endpoint` and `capabilities` are dropped
  (`DROP COLUMN IF EXISTS`). Nothing in governance, permissions, task delivery, export or the MCP
  server read them.
- The foreign key from `ai_learning_records.ai_participant_id` to `ai_participants(id)` is dropped:
  the id alone is no longer unique, and learning records outlive a removed participant like the
  votes they describe. Learning reads are scoped to the project through its impact reviews
  (`GET /api/v1/projects/{project_id}/ai-participants/{bot_id}/learning` and `/alignment-stats`).
- Two new tables: `bot_runners` (one row per connected bot: managed webhook, runner address,
  credentials issue time, the runner's last report) and `project_bot_connections` (one row per
  connected project, with a version for compare-and-set writes).

The AI participants API (`/api/v1/projects/{project_id}/ai-participants` and its `{id}` routes) is
removed. A bot becomes a project's AI participant only through a project bot connection.

## Why migration 0092 changed

`0092_ai_participant_config_version.sql` creates the trigger that advances
`ai_participants.config_updated_at`. Its column list used to name `model`, `provider`,
`api_endpoint` and `capabilities`. PostgreSQL refuses to drop a column that a trigger's
`UPDATE OF` list names, and once the columns are gone the old list would fail with "column does not
exist" whenever `0092` runs again (a database restored without its ledger, a replay start). So
both files install the same trigger over the surviving columns: `name`, `domain_overrides`,
`max_domain_level`, `can_veto_human_consensus`, `reason_min_length`, `is_active`.

The ledger stores a checksum, so a database that already applied the old `0092` logs a warning that
the file changed and keeps what it has; `0112` replaces the trigger on every database before it
drops the columns. A test runs `0112`, then `0092`, then `0112` again and checks that the schema and
the row versions do not change.

## Rolling back

Without touching the schema, nothing needs to change: no connection exists until one is created.

Removing the schema, in this order:

```sql
DROP TABLE IF EXISTS project_bot_connections, bot_runners;
ALTER TABLE ai_participants
  ADD COLUMN IF NOT EXISTS model varchar(100),
  ADD COLUMN IF NOT EXISTS provider varchar(50),
  ADD COLUMN IF NOT EXISTS api_endpoint text,
  ADD COLUMN IF NOT EXISTS capabilities jsonb NOT NULL DEFAULT '[]'::jsonb;
-- Restore the single-column key only after duplicate ids are resolved:
--   SELECT id FROM ai_participants GROUP BY id HAVING count(*) > 1;
ALTER TABLE ai_participants DROP CONSTRAINT IF EXISTS ai_participants_pkey;
ALTER TABLE ai_participants ADD CONSTRAINT ai_participants_pkey PRIMARY KEY (id);
ALTER TABLE ai_learning_records
  ADD CONSTRAINT ai_learning_records_ai_participant_id_fkey
  FOREIGN KEY (ai_participant_id) REFERENCES ai_participants(id) ON DELETE CASCADE;
```

The three nullable columns come back empty (and `model` and `provider` were `NOT NULL` before: fill
them and add the constraint if an older build needs it). Learning records of a removed participant
have no parent row, so delete them or re-create the participants before adding the foreign key.
Also delete the migration's row from `schema_migrations`, otherwise the runner reports the missing
objects at the next start. The removed AI participants API does not come back with the schema.

## Renumbering the migration

The migration is `migrations/0112_bot_connections.sql`. If another migration takes `0112` first,
rename the file to the next free number and change the three places that name it in
`apps/api/src/main.rs`: the embedded list (`include_str!` path and name), the schema probe table
(`SchemaProbe::Relation("project_bot_connections")`), and the list in the replay-safety test
(`everything past the cutoff re-runs on an adopted database and must be idempotent`). Keep the
number order in all three. The migration test that replays `0092` after it names the file through
`include_str!` as well. A database that already recorded the old name has the objects but no ledger
row under the new one: the probe finds `project_bot_connections`, the runner adopts it, and because
the file is idempotent re-running it is harmless.
