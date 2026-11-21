# Runner task protocol

Operating notes for migration `0110_runner_task_protocol.sql` and the settings that go with it.

## What migration 0110 adds

- Table `project_execution_settings`: one optional row per project (enabled flag, channel, CLI,
  working directory on the runner machine, what happens to the issue on completion, an optional
  failure label, approval gate, a version for compare-and-set writes). Its checks mirror the API
  validation: absolute path of 2 to 1024 bytes without control characters, `.` or `..` segments or
  `//`; channel `tmux` or `headless`; CLI `claude`, `codex` or `opencode`; approval gate `none`.
- Five nullable columns on `ai_tasks`: `acknowledged_at`, `plan`, `last_progress_at`,
  `lease_expires_at`, `runner_phase`. No defaults, so no rewrite.
- Two checks on `ai_tasks`, added `NOT VALID` so a historical value cannot fail the migration:
  `ai_tasks_status_check` (`pending`, `processing`, `acknowledged`, `completed`, `failed`,
  `cancelled`, `retried`) and `ai_tasks_runner_phase_check` (`queued`, `pushed`, `blocked`). They
  apply to every new or updated row.
- Two partial indexes over the active statuses: `idx_ai_tasks_active_lease` and
  `idx_ai_tasks_participant_active`.

Existing `processing` rows are not touched. They carry no lease, and the worker sweep only ends
tasks whose lease has passed, so a `processing` row that predates the migration is never swept:
fail or re-queue such rows by hand before upgrading.

## Locks

The file runs in one transaction under `SET LOCAL lock_timeout = '10s'`. `ALTER TABLE ai_tasks`
takes `ACCESS EXCLUSIVE` for catalogue work only (nullable columns, `NOT VALID` checks), so it is
a few milliseconds when the table is idle. Behind a long transaction that already holds the table
the migration fails after ten seconds without any effect, and the next start retries it. Index
creation is not concurrent; the table is small. Running the file again changes nothing.

## Configuration

`[ai_tasks]` gains two keys:

| Key | Default | Range |
|---|---|---|
| `lease_extension_secs` | 1800 | 60..=86400 |
| `max_processing_secs` | 86400 | `processing_timeout_secs`..=604800 |

`max_processing_secs` must not be below `processing_timeout_secs`; the error names both keys.
Both the API and the worker read them from the same file.

## Behaviour decisions

- **Sweep.** The worker ends an active task only when its `lease_expires_at` has passed. Pick-up
  always sets a lease, so a task has one from the start. It also clears the previous attempt's
  `plan`, `acknowledged_at`, `last_progress_at` and `runner_phase`.
- **Configuration default.** If `max_processing_secs` is not set and `processing_timeout_secs` is
  above 86400, the cap defaults to the timeout instead of failing the configuration.
- **Execution settings versions.** A save is a compare-and-set on `version`. Without a stored row
  the caller sends no `expected_version`; with one it sends the stored version. Any other
  combination answers business code 409 `execution settings changed; reload` with the current
  version, and writes nothing.
- **Progress coalescing.** Reports of one kind (`progress`, `runner.alive`, ...) less than 30
  seconds apart extend the lease but write a single event. The window is per kind and is read from
  the events, so an acknowledge does not swallow the first progress report.
- **`reply_policy` text** in the task view: `comment` (mention, review), `comment+state:auto`,
  `comment+state:<key>` or `comment` (assignment, by the project's `on_complete_state`), `vote`
  (proposal vote), `none` (manual); failures: `comment`, `comment+label` (assignment with a
  failure label) or `none`.
- **Reply routing.** Completing or failing a task through `/api/v1/ai/tasks/{id}/complete|fail`
  writes the reply in the same transaction as the status change. It never creates a task, never
  extracts mentions from the text, and never assigns. A completed assignment moves the work item
  to the first done state (lowest position among states of category `done`) unless it is already
  in a terminal state; `none` and `state:<key>` follow the project's setting, and an unknown key
  leaves the state with the warning `state <key> not in workflow`.
- **Vote tasks.** Completing a `proposal_vote` task casts the bot's vote through the same service
  as `POST /api/v1/proposals/{id}/votes`, in the same transaction as the status change. The
  proposal id is `payload.proposal_id` (a `PROP-<uuid>` id cannot be the task reference). A refused
  vote (reason shorter than the project's minimum, 50 characters by default; closed voting window;
  proposal not in voting) is a 400 and the task stays active. A vote the bot already cast completes
  the task with the warning `vote already recorded`. `no_change` and a missing `vote` are 400.
- **Refused at delivery.** When the runner declines a task at delivery, the worker fails it and,
  for a project whose execution settings are enabled, posts `Could not start: <reason>` on the work
  item (or as a reply to the mention).

## Rolling back

Rolling back is manual and never run by the server. Stop the runner and the worker first, then run
the statements in this order:

```sql
DROP INDEX IF EXISTS idx_ai_tasks_participant_active;
DROP INDEX IF EXISTS idx_ai_tasks_active_lease;
ALTER TABLE ai_tasks DROP CONSTRAINT IF EXISTS ai_tasks_status_check;
ALTER TABLE ai_tasks DROP CONSTRAINT IF EXISTS ai_tasks_runner_phase_check;
UPDATE ai_tasks SET status = 'processing' WHERE status = 'acknowledged';
ALTER TABLE ai_tasks DROP COLUMN IF EXISTS runner_phase, DROP COLUMN IF EXISTS lease_expires_at,
  DROP COLUMN IF EXISTS last_progress_at, DROP COLUMN IF EXISTS plan, DROP COLUMN IF EXISTS acknowledged_at;
DROP TABLE IF EXISTS project_execution_settings;
```

Then remove the migration's row from `schema_migrations` (name
`0110_runner_task_protocol.sql`); otherwise the migration runner reports the missing objects at the
next start. Execution settings are lost with the table: export them first if they are needed
(`SELECT * FROM project_execution_settings`).

## Renumbering the migration

If another migration takes `0110` first, rename the file to the next free number and change the
three places that name it in `apps/api/src/main.rs`: the embedded list (`include_str!` path and
name), the schema probe table (`SchemaProbe::Relation("project_execution_settings")`), and the
list in the replay-safety test. Keep the numeric order in all three lists. A database that already
recorded the old name has the objects but no row under the new one: the probe finds the table, the
runner adopts it, and because the file is idempotent re-running it is harmless. Do not edit an
applied file for any other reason: the ledger stores a checksum.
