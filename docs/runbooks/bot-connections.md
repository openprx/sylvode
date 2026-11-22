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
  (project, bot), with a version for compare-and-set writes and the execution settings of the
  connection: enabled flag, channel, CLI, working directory). `0114_multi_bot_connections.sql`
  brings a database that holds the earlier shape (one connection per project, settings in
  `project_execution_settings`) forward; a new database has the final shape from `0112`. A project
  has at most one tmux connection per CLI type (partial unique index `pbc_one_tmux_per_cli`);
  headless connections are not limited.

- A trigger on `workspace_members` (`trg_workspace_members_runner_bot_seat`) refuses a role other
  than `member` for a bot that has a `bot_runners` row. A runner bot executes agents on untrusted
  issue text, so it is never a workspace admin or owner; the Members API also answers 409 when its
  seat role is changed.

## What a runner bot's token may reach

The token of a bot made by a project bot connection is limited to the projects where the bot has
an enabled connection. `middleware/runner_bot_scope.rs` enforces this in
`bot_or_user_auth_middleware`, once, for every route, from the table in
`middleware/runner_bot_routes.rs` (one entry per method and path of `main.rs`; 445 today). A route
has one of five classes:

| Class | Routes | Runner bot |
|---|---|---|
| no project (17) | the task protocol and callbacks, runner report, catalogues | allowed; the handler limits the bot to its own tasks |
| project-resolved (290) | issues, comments, labels of an issue, sprints, board, forms, form records, attachments and jobs, plugins, domains, check results, audit reports, search with `project_id`, proposals, impact reviews, Flow objects, collections, records, exports, conversions, block shares | allowed only when the target resolves to a project that has an enabled connection for the bot |
| own resource (13) | the bot's notifications, uploads, preferences, personal Flow home, a proposal it authored that no project claims | allowed; the handler filters by the caller |
| admin (85) | members, roles, bots, webhooks, workspace settings and workflows, labels (workspace), project creation and deletion, connections and execution settings, alert sources and rules, Flow administration and workspace import/export | refused |
| refused (40) | workspace-wide listings (projects, labels, activities, proposals, trust scores, workspace Flow), personal pages of the user, block transfers, Flow collab tickets, `my/*` | refused |

The target is found the way the permission layer finds it for that route family: the project in the
path; an issue, comment, form, view, record, attachment, job, plugin, sprint, check result or impact
review by its own row; a Flow object, collection or record by `flow_objects.project_id`; a Flow
conversion, export or block share by its source object; a proposal by the one project its linked
issues belong to; a request that names the project in its JSON body (`project_id`, the parent
object, the conversion's source object or preview) or in the `project_id` query parameter by that
value. A target that does not exist, belongs to no project (except the bot's own personal space or
its own unlinked proposal), spans two projects, or is in another project is refused. Every refusal
answers 403 `this bot may act only in projects it is connected to`; it never says which part failed.
A bot without a runner row (a bot made on the Bots page) is not affected.

A task that was running when its connection was disabled can still report how it ended (the task
routes need no project; the handler requires the bot to be the task's participant). A route that is
not in the table, or a new route added to `main.rs`, is refused until it is classified:
`every_route_of_main_is_classified` fails the build first.

## Tasks, rotation and removal

- A runner bot's tasks reach it only while the task's project has an enabled connection for it. The
  gate is applied when a task is created (assignment and mention create nothing, an alert rule
  records `bot_unavailable`, a manual start answers 409), when the worker picks tasks up, and again
  just before the delivery. The worker also fails pending tasks without an enabled connection with
  the reason `connection_unavailable` (`bot_not_connected_to_project` or `connection_disabled`) and
  tells the issue; a task already running continues until it ends.
- Rotation (I1): the old token and secret stop working when the rotation commits, so a task that is
  running on the runner can no longer report; its lease runs out and it is retried or failed. Rotate
  when no task is running, or accept that running tasks fail.
- Hold (I2): after a connection or a rotation the worker waits until the runner has used the new
  token, for at most one hour after the last rotation. Rotating repeatedly restarts the wait but
  gives an owner or admin nothing they could not do by disabling the connection.
- A runner address that starts with `http://` is accepted only when its host is listed in
  `[outbound] allowed_hosts` (or private targets are switched on); use `https://` otherwise. The
  address is stored as parsed (`HTTPS://Host:443/` is `https://host`).
- Lock order, everywhere: the project row, then the `bot_runners` rows (bot id order), then the
  `project_bot_connections` rows. Connect and the release for a deleted project both start with the
  project row; the release removes the connections one bot at a time.
- Events that a connection writes carry the runner's working directory. They are not delivered to
  webhooks (their payload is withheld, as for every event type without a delivery policy) and are not
  part of the connected-application stream.

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

`0114` has no down file: restore the disposable database from the redeploy procedure.

Removing the schema, in this order:

```sql
DROP TABLE IF EXISTS project_bot_connections, bot_runners, project_execution_settings;
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
