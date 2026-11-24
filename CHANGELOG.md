# Changelog

All notable changes to Sylvode (formerly OpenPR) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Every commit bumps the
patch version, so the entries below are grouped by published release tags rather than by
individual patch versions. Releases up to and including 0.2.21 were published under the OpenPR
name.

## [Unreleased]

### Changed

- **Report schemas moved to `scripts/schemas/`.** The JSON schemas that the delivery, readiness
  and Flow evidence scripts and the project release readiness endpoint read were moved from
  `docs/schemas/` to `scripts/schemas/`; every script, test and documentation index entry that names
  them uses the new path.
- **Documentation rewritten and reorganised.** The public documentation was rewritten against the
  current product. It now reads: `docs/install.md`, `docs/configuration.md`, `docs/operations.md`,
  the user guides under `docs/guide/`, the integration references under `docs/integrations/` and
  `docs/plugins/README.md`, with `docs/README.md` as the index. The runbooks under `docs/runbooks/`
  and the two universal forms documents were folded into these, and the API endpoint list is now
  `docs/integrations/api.md`. The root, MCP server, runner and frontend READMEs and both security
  policies were rewritten, and the comments of `config/sylvode.example.toml` were revised.
- **Runner example configurations.** The comments of `apps/runner/config.example.toml` follow the
  runner README. In `config/sylvode-runner.example.toml` one route selects tasks with
  `trigger_kinds` instead of `bot_agent_types = ["claude-code"]`.
- **Scenario template catalogue source.** The scenario catalogue JSON reporter and verifier read the
  template table from `docs/guide/scenario-templates.md`.

- **Work item answers carry an identifier and labels.** A single work item (read, create, update,
  by identifier) and the project issue list answer `identifier` (`WEB-12`) and `labels`, which is
  always an array, empty when the item has no label; before, the single-item answers had
  `labels: null` and none had an identifier. The board, `GET /my/issues` and the issue results of
  `GET /search` carry `identifier` too. The update answer is read inside the transaction that
  wrote it.

- **Work item update and list contract.** `PUT /issues/{id}` clears `assignee_id`, `sprint_id` and
  `due_at` when they are sent as `null` (before, `null` meant "unchanged", so an item could not be
  unassigned, taken out of a sprint or have its due date removed); `null` on `title`, `description`,
  `state` or `priority` is a 400. Item create and update bodies and the issue list query refuse a
  field or parameter they do not know instead of ignoring it. `GET /projects/{id}/issues` accepts
  `sprint_id=<uuid>|none`, which it used to ignore.

- **MCP and web client follow the item contract.** `work_items.update` clears `sprint_id` (as it
  already cleared `assignee_id` and `due_at`) when it is sent `null`, an empty string or `"null"`,
  and its schema says so; `work_items.list` takes `sprint_id` (a sprint id or `none`).
  `work_items.get_by_identifier` asks the API only: the client-side search by project and position
  that answered when the API lookup failed is removed, so an API error is returned as the tool
  error. The web app shows the identifier from the API, and unassigning an issue on its page sends
  `assignee_id: null`, which used to do nothing.

- **Assignment notifications and issue webhook payloads.** Creating a work item with a human assignee
  now notifies them, as assigning later does; assigning a work item to yourself or to a bot no longer
  writes a notification. In `issue.*`, `label.*` and `comment.*` webhook payloads `key` is the work
  item's identifier (`WEB-12`) instead of the project key and eight characters of its UUID, and the
  issue object carries `due_at`. `issue.updated` also reports `sprint_id` and `due_at` changes.
  `issue.assigned` is sent when a work item is created with any kind of assignee and when an
  assignee is removed (`changes.assignee_ids.new` is empty). `issue.deleted` carries the issue with
  the labels it had before the delete (it carried an empty `label_ids` and a key built from the
  UUID).

- **Comment deletion event, mention link and delivery duration.** `comment.deleted` carries the
  comment's author name and its original `created_at` and `updated_at` (it sent an empty author and
  the time of the deletion), and its `issue` has `id`, `key` and `title` like the other comment
  events. A `mentioned` notification for a comment now has `related_comment_id` set. The webhook
  delivery log's `duration_ms` is the measured time of the request; it was computed from two equal
  timestamps and so was always about zero. Migration `0116_webhook_delivery_duration.sql` adds the
  column.

- **Sprint rules.** A sprint moves `planned` to `active` to `completed`, and `planned` or `active`
  to `cancelled`; completed and cancelled are final and any other status change is a 400. A project
  can have one active sprint (starting a second answers 409 and names the active one), a sprint
  cannot end before it starts, and names are unique within a project ignoring case. A new sprint can
  only be `planned` or `active`. Members can still create and edit sprints, but creating an active
  sprint, changing the status and deleting a sprint need an owner or admin. A sprint body or update
  with an unknown field (for example `goal`) is refused, and `start_date` and `end_date` can be
  cleared with `null`. Migration `0117_sprint_rules.sql` repairs existing data (a second active
  sprint is set back to planned, case-twin names get a numeric suffix, an end before the start is
  raised to the start, unknown statuses become planned) and adds the constraints.
- **Completing, cancelling or deleting a sprint moves its work items.** Unfinished work items (not
  in a terminal state and not in a `done` or `completed` category) go to the backlog, or to a planned
  sprint of the project named by the new `rollover_sprint_id` of `PUT /sprints/{id}`, in the same
  transaction as the status change; the answer reports `rollover.moved_count` and
  `rollover.target_sprint_id`, each moved item gets a `sprint_changed` activity and an
  `issue.updated` webhook, and `sprint.completed` carries `data.rollover`. Deleting a sprint now
  moves all its items to the backlog with the same records instead of silently dropping their
  sprint.
- **Cycles page and MCP follow the sprint rules.** Start, Complete, Cancel and Delete are shown to
  owners and admins only; Complete and Cancel (new) open a dialog that offers the backlog or a
  planned sprint for the unfinished issues and reports how many were moved; a failed start shows the
  server's message naming the active sprint; cancelled sprints are listed with the completed ones
  under a Cancelled badge; sprints without dates are shown as such; the form field is `description`
  and the status labels are translated. The MCP `sprints.create` offers only `planned` and `active`,
  and `sprints.update` takes `description` and `rollover_sprint_id`.
- **Sprint and Due date on the issue page.** The issue page's properties have a Sprint select (No
  Sprint, planned and active sprints; an ended sprint is shown only while the issue is in it) and a
  Due date picker with a clear action, both saved like the assignee and priority controls. A due date
  is a calendar date: it is stored as midnight UTC of the day and read back from its UTC date, so
  it does not shift by a day between time zones.
- **Added: `GET /sprints/{id}`.** It reads one sprint, with the same access as the sprint list.
- **Cycles page sends `description`.** The page sent `goal`, which the API never stored; the field
  is now `description`, so a sprint's description is saved from the page.

- **Label rules.** Label colours must be `#rrggbb` (stored lower-case, default `#6b7280` instead of
  `#gray`; anything else is a 400), label names are unique in a workspace ignoring case, and label
  bodies refuse unknown fields. Attaching a label that is already on a work item and detaching one
  that is not now succeed, and `label.added` / `label.removed` are sent only when something changed.
  The batch attach route sends `label.added` for each label it attached, and deleting a label sends
  `label.removed` for each work item it was on, with the label's name and colour and the item's
  identifier. Migration `0118_label_rules.sql` rewrites invalid colours to the default, lower-cases
  colours, renames the later of two labels that differ only in case (adding eight characters of its
  id) and adds the constraints. Creating a project from a scenario template still adds the
  template's labels, skipping any whose name already exists in another case. The frontend `Label`
  type has `workspace_id` and `description` (it had `project_id`).

- **Runner status is current, and the issued block merges.** The runner sends its status report
  when a tmux session is created or recovered and after the first delivery to a session since it
  started, not only every report interval (default 600 seconds), so Sylvode shows the session within
  seconds. Requests are coalesced for two seconds and reports are never closer than ten seconds, so
  at most six extra reports a minute (the API accepts twelve). The configuration block Sylvode issues
  for a bot now carries `webhook_secret` in its `[[ai_task_callback.bots]]` table, so a second bot's
  block can be appended to a runner's file as its header says (a runner serving two bots refuses to
  start without a per-bot secret; the old text said to put the second secret in `[security]`).
  The `runner` service of `docker-compose.yml` runs with `init: true` and `stop_grace_period: 45s`.

- **Project export and import work again, with one file format.** Both queried columns that do not
  exist (`work_items.key`, `status`, `type`, `reporter_id`, `comments.content`, `created_by`), so
  every export and every import answered "database error". Export now writes the document
  `sylvode.project_export` version 1 (issues with identifier, title, description, state, priority and
  due date; comments with the issue identifier, author name, text and time), as JSON or as a CSV
  whose columns are `Identifier,Title,State,Priority,Due,Description,Created At,Updated At` with
  every field quoted. Import reads exactly that document, as `{project_key, document}`, into an
  **existing** project; the old body with `project_name` and `issues[].status`/`type`, and the
  creation of a project by import, are removed. Workspace owners can import now (only the `admin`
  role could); members still cannot. Everything is validated before the first write (state in the
  project's workflow, priority, dates, comment links) and the answer reports the new identifiers.
  The project overview page imports into the current project and says how many issues and comments
  it added.

- **One active version per plugin key; the restaurant template calculates for real.** Two active
  versions of one plugin key both ran their hooks. Installing a version as `active`, or setting one
  to `active`, now disables the other active version of the key in the same transaction (each disabled
  version gets a `plugin.updated` event), and a unique index guarantees it; migration
  `0119_plugin_single_active_version.sql` disables the older of any existing duplicates. The
  restaurant scenario template no longer installs the `restaurant_calc` plugin, a module that
  returned the constant `19.98`: the `line_total` of an `order_line` is a formula field (quantity
  times unit price), so it is calculated from the real inputs, and the template's usage guide and
  AI roles no longer name the plugin.

- **Compose passes both plugin switches.** `docker-compose.yml` passed only
  `SV_FORM_PLUGIN_PACKAGES_ENABLED` to the `api` service although form plugin packages and the Plugin
  Creator need `SV_FORM_PLUGIN_EXTENSIONS_ENABLED` as well; both are passed now (default `0`) and
  documented in `.env.example`, the configuration reference and the plugin guide.

- **Settings that nothing read are gone; unknown ones are refused plainly.** `auth.default_author_id`
  and `oauth.embed_frame_ancestors` were validated and never used; a file that still has either is
  refused as an unknown key, as is `mcp.auth_token` (which had a dedicated refusal message). The
  runner refuses an `agent_type` it does not dispatch to, `cli` included, listing the valid types.
  The Flow-only command line and MCP profile default to `http://localhost:8081`, like the rest of
  the MCP configuration (the Flow code defaulted to `http://127.0.0.1:3000`), the unused root
  `VITE_API_BASE_URL` of `.env.example` is removed, and the voting rule descriptions of the proposal
  form say what the server applies (the Absolute Majority and Consensus texts did not).

- **Chinese labels, hard-coded texts and the Agents destination.** The Agents entry of the left rail,
  its `/agents` page, its column, the `G` then `A` shortcut and its translation keys are removed
  (`/agents` is the application's not-found page); manage bots under **Members and bots** and connect
  them in a project's **Execution** settings. In the Chinese interface the roles, proposal statuses and
  progress steps, trust levels, the Search headings, several form labels and the impact review texts
  that were still English are translated, and the view type names of the view setup, the Flow editor's
  link controls and a few other labels now come from the locale files in both languages.

- **Runner runs codex on the tmux channel.** codex runs in the session `<KEY>-codex` with the MCP
  server and the turn hook passed as `-c` overrides, and `sylvode-runner setup codex` prints or
  applies the same. The runner report carries one posture row per CLI.
- **codex on the tmux channel in the API and the connection settings.** The API and the connection
  settings accept codex on the tmux channel (migration `0130` widens `pbc_tmux_cli_check`), and a
  connection's permission status follows the posture row of its own CLI.
- **MCP resolves sprint, comment and attachment owners through the API.** `sprints.update` and
  `sprints.delete` resolve the owning project through `GET /api/v1/sprints/{id}`; the comment and
  attachment tools through the new `GET /api/v1/comments/{id}` and
  `GET /api/v1/form-attachments/{attachment_id}`, which return the project the comment or attachment
  belongs to (attachments also when archived).
- **Sprint list reports progress.** `GET /api/v1/projects/{project_id}/sprints` reports
  `progress: {total, done}` for each sprint.
- **Client errors are shown in the user's language.** Transport and Flow client errors (network,
  cancelled, session changed, download, shared block) are shown in the user's language.
- **Source layout.** The API router, the runner bot route tables and the event dispatcher are split
  into one file per area, and the migration registry is generated from `migrations/*.sql`.

### Removed

- **Universal forms delivery and sign-off tooling.** The scripts and schemas that produced and
  checked delivery manifests, readiness, sign-off, completion and implementation map reports,
  acceptance packets and delivery bundles were removed, together with
  `docs/universal-forms-implementation-map.md`. The universal forms test suites, the smokes that run
  the product, the security scope audit and the source coverage and production readiness audits
  remain, and the Universal Forms Gates CI job runs them.
- **MCP contributor guide resource.** `apps/mcp-server/AGENTS.md`, the `sylvode://guides/agents`
  resource that served its build and test notes, and the `sylvode-skillgen` output for that file
  were removed. The skill, the tool reference and the workflow recipes are unchanged.
- **Internal documents.** The repository now holds only licences, notices, changelogs and feature
  descriptions. Removed: the contributor guide and the agent instruction file at the root, the
  planning, requirement, acceptance, audit and baseline documents and the v1 stable runbook under
  `docs/`, the design notes under `frontend/src/lib/ui/`, the OAuth end-to-end deployment notes and
  test plan, and the spike sign-off record and evidence source copies. The OAuth acceptance record
  template is now a TypeScript module next to its reporter, the vendored `loro-prosemirror` and
  SeaORM notes are reduced to provenance notices, and the universal forms audits no longer check the
  removed documents or the wording of the rewritten ones. The runbooks, `frontend/QUICKSTART.md`,
  `docs/API_ENDPOINTS_PHASE3.md`, `docs/scenario-templates.md`, `docs/universal-forms-and-plugins.md`
  and `docs/universal-forms-production.md` are gone; their content lives in the new layout.
- **Unused trigger reasons.** The `completed` and `failed` trigger reasons and runner trigger kinds,
  which no event could produce, were removed.

### Fixed

- **Web UI served over plain HTTP on a non-localhost address.** Flow live editing never started
  there and said nothing. Without `crypto.randomUUID` the client sent a client id that was not a UUID,
  the server could not parse its hello and dropped the connection silently, and the page ended in
  the permission, not-enabled or loading state. Every identifier the web client sends is now a
  version 4 UUID built from `crypto.getRandomValues` when `randomUUID` is missing (no `Math.random`
  fallbacks remain), the collaboration handshake answers an invalid or missing first frame, an
  invalid `open` and a handshake timeout with a `rejected` frame and a close, and logs the reason
  with the document id. The page shows "Live editing could not start" with a retry button when the
  handshake does not complete, after three attempts with a doubling wait of at least 1 s. Copy
  buttons use a hidden text field where the Clipboard API is missing and say so when copying fails,
  and the SHA-256 checks of package downloads and plugin panels run in script where Web Crypto is
  missing. `docs/install.md` lists what works and what is limited over plain HTTP.
- **Sprint refusals carry a code.** The `400` and `409` answers of the sprint API for an invalid
  status change, an end date before the start, a duplicate name and a second active sprint now carry
  `error_code` (`sprint_invalid_transition`, `sprint_end_before_start`, `sprint_name_taken`,
  `sprint_active_exists`) and structured `details`; the active-sprint answer names the sprint in
  `details.active_sprint_id` and `details.active_sprint_name`, and its message no longer contains the
  id. The Cycles page shows a translated text for each of them.
- **Cycles page.** Completing or cancelling a sprint now reports the sprint the unfinished issues
  moved to instead of "Backlog", and says that nothing needed moving when that is the case. Cancelled
  sprints have their own collapsed group with a heading and a count.
- **Create issue dialog.** Its Sprint options are the same as on the issue page: No Sprint, the active
  sprint and the planned sprints.
- **Chinese texts.** The remaining "Issue" wording in the Chinese texts is "工作项", and the built-in
  workflow states (Backlog, Todo, In Progress, Done) are shown with translated names on the issue,
  board and list pages; custom state names are shown as stored.
- **Not found page.** An unknown address shows a "page not found" page with a link back to the home
  page, inside the application for signed-in users and on its own for signed-out users.
- **Webhook deliveries connect to the checked address.** The API and the worker resolved a webhook
  host name for the outbound check and then again for the connection, so a name could answer a
  public address to the check and an internal one to the connection. Event-dispatcher and legacy
  webhook deliveries now connect only to the addresses the check saw.
- **`GET /ready` reports an unreachable database as HTTP 503.** It answered HTTP 200 with `code`
  500; it now answers HTTP 503 with `code` 503.
- **Project labels, batch label attach and lookup by identifier for user tokens.** A user token on
  `GET /projects/{project_id}/labels` or `POST /issues/{issue_id}/labels/batch` answered HTTP 500
  because the handlers required a bot. Both now accept any member of the workspace (and a bot of it),
  and `GET /issues/by-identifier/{identifier}` also answers users: it searches the workspaces the
  user belongs to, accepts `?workspace_id=`, and answers 409 when the identifier matches work items
  in more than one of them.
- **Windows build.** The thread CPU clock is read on Unix only, so every workspace binary now builds
  for the Windows target; CI checks that target in the `windows-build` job.
- **Workflow state positions.** Creating or moving a workflow state to an already taken position no
  longer fails with an internal error: later states shift down by one, a position past the end
  appends, and a negative position is a 400.
- **Connected applications without OAuth.** The connected-applications endpoints answer
  `feature_disabled` instead of a bare 404 when the server has no OAuth configuration, and the web
  panel says the feature is not enabled.
- **Cycles page load.** The page no longer loads up to 1000 work items in one request: progress comes
  from the server and a sprint's items load 50 at a time.
- **Small web fixes.** "1 unfinished issue moved" uses the singular in English; the not-found page
  title is a real heading; a duplicate sprint name is reported under the name field of the create
  dialog; the browser tab icon is linked.
- **Flow page object opened twice.** Opening the same Flow page object from two places at once no
  longer opens extra collaboration connections.
- **Timing-sensitive tests.** The tests of the text decode budget and the import CPU ceiling no
  longer depend on machine speed.

## [0.5.0] - 2026-11-23

### Added

- **Runner `doctor`, `--check` and `config check`; degraded start-up; installer checks.**
  `sylvode-runner doctor` prints the shared prerequisite report (exit 0, 2 or 1); a missing
  prerequisite degrades the service (`/health` `prereq` and `degraded`) instead of refusing it;
  `scripts/install.sh` runs doctor with the service's `PATH` before installing, takes
  `--headless-only` and `--dry-run`, warns about missing linger, rewrites an unchanged unit never,
  and runs doctor again in the service context.
- **Runner tmux channel.** `sylvode-runner` keeps one tmux session per project key, queues tasks
  first in first out, types each into an idle, empty prompt as a bracketed paste, follows turns
  through CLI hooks (`sylvode-runner hook`), reminds and fails forgotten acknowledgements and
  completions, and rebuilds its state from `GET /ai/tasks/active` after a restart. New
  subcommands: `sessions [--prune]`, `trust KEY`, `release KEY`.

### Removed

- **`mcp-server <command>` workspace form.** `mcp-server` offers only `serve` and `doctor`; the workspace
  commands exist only under `sylvode`, and the old form fails as an unknown command.
- **Legacy task callbacks and `agent_type = "cli"`.** The API no longer serves
  `POST /api/v1/ai/callbacks/task/{id}/complete`, `.../fail` or `.../progress`; a task ends through
  the task protocol (`POST /api/v1/ai/tasks/{id}/complete` or `.../fail?attempt=N`). The runner
  refuses `agent_type = "cli"` (use `agent_type = "runner"` with the headless channel), and the
  keys that only it used: `[agents.cli]`, `[agents.ai_task_callback]`, `[features] cli_enabled`
  and `callback_enabled`, and `[ai_task_callback] max_output_chars`. A forwarding agent
  (`webhook`, `openclaw`, `openprx`, `custom`) completes the task through the protocol when the hand-over works (so it runs once) and fails
  the task with `source: "runner"` when it does not. Tasks sent over the tunnel are answered
  `failed`.

### Changed

- **Flow is on by default everywhere.** A workspace has Flow on unless an administrator switches it
  off: `flow_workspace_settings.flow_enabled` defaults to `true`, a trigger creates the settings row
  with every workspace, and migration `0115_flow_default_on.sql` backfills the rows of existing
  workspaces without touching a row that is switched off. The four capabilities
  `scope_native_legacy`, `personal_package_imports`, `personal_scope_package_exports` and
  `cross_workspace_forms_conversion` now default on as well (switched on once for an existing
  database). The switches themselves stay. See `docs/runbooks/flow-runtime-capabilities.md`.
- **Runner hardening, third review.** Busy, choice and dialog detection reads only the CLI's own
  chrome at the bottom of the screen; a screen hold is bounded (`screen_hold_secs`, default 600) and
  then fails the waiting tasks as retryable; an acknowledged task is busy even if its
  `UserPromptSubmit` hook was lost; a CLI is restarted for a changed directory only when its screen
  is calm. Hook messages are checked against the launch's nonce before any work is spent, the
  sender's ancestry is read without a `ps` per hop (`/proc` on Linux, one `ps -A` snapshot on
  macOS), at most 32 connections are read at once, and an unreadable `session.json` admits nothing.
  Hooks are ordered by a per-session counter, not the clock. The configuration file and the
  canonical state directory are denied to the CLI, and no allowed root may contain the file. Extra
  read rules count as confined only as `Read|Glob|Grep(/{dir}/...)`. A pushed task that cannot be
  adopted after a restart fails as retryable. `--prune` never ends a working CLI. A successful
  forward completes the task. The installer follows `[runner.tmux] enabled` (`--headless-only` is
  removed), skips the macOS service-context check unless root, and `/health` returns only `status`
  and `protocol`. The stand-in CLI of the tests never writes environment values, and every probe
  starts with an allow-listed environment.
- **MCP search mode is the default tool listing.** `tools/list` offers nine tools:
  `sylvode_overview`, `sylvode_search`, `sylvode_read`, `sylvode_write` and the five task tools.
  Registry tools are found with `sylvode_search` and called through `sylvode_read` or
  `sylvode_write`; calling a registry name directly is refused unless the connection chose full
  mode (`[mcp] tool_mode = "full"`, `serve --tools full`, `?tools=full` or the
  `X-Sylvode-Tool-Mode: full` header). `[mcp] allow_write_call = false` disables every write tool except the five task lifecycle tools, in every mode and in the command line.
  Connected (OAuth) applications are not affected: they keep the scoped read tools.
- **The retired `X-OpenPR-MCP-*` attribution headers and the `openpr://` resource URIs are gone.**
  `mcp-server` sends only `X-Sylvode-MCP-Surface` and `X-Sylvode-MCP-Tool`, the API reads only
  those, and a resource is read by its `sylvode://` identity only (`resources/read` of an
  `openpr://` URI answers unknown resource, with no `_meta.deprecation`).
- **MCP security fixes.** `[mcp] allow_write_call = false` now refuses every write tool in every
  mode and in the command line; an ownership bearing id a tool does not declare is refused before
  any request; a tool with a closed schema is validated on every door; repeated or conflicting tool
  mode parameters are a 400; the task and entry tools are attributed to dotted names so the
  operation log records them; a bot asking for another bot's task gets the answer for a missing
  one; a repeated task end with other content is a 409.
- **The MCP skill is `sylvode-mcp`.** `skills/openpr-mcp` is renamed to `skills/sylvode-mcp` and
  rewritten for an agent that receives a task message: the task procedure (get, acknowledge with a
  plan, progress, complete or fail), what Sylvode does on completion (`reply_policy`), search mode
  as the default way of working and full mode as the opt-in. The resources are
  `sylvode://skills/sylvode-mcp` (the skill) and `sylvode://skills/sylvode-mcp/tools` (every tool);
  `sylvode://skills/openpr-mcp` is gone, with no alias. The server's `instructions` and the guide
  resources are the generated skill files. `sylvode-skillgen --check` (and the CI job
  `skills-sync`) fails when a tool list, count or the skill no longer follows the MCP server;
  `sylvode-skillgen --write` regenerates.
- The project-resource MCP tools (`project_resources.*`, `code.resources.list`,
  `code.directory.get`) and the `sylvode://projects/{project_id}/resources` resource template are
  parked: they are no longer registered, listed or callable.

### Added

- **Connect bots to a project.** A project is connected to one or several bots that run its agent
  tasks on runners; a connection is (project, bot) and holds its own execution settings (enabled,
  channel, CLI, working directory). `POST /api/v1/projects/{id}/bot-connections` creates (or
  reuses) the bot, its token, a managed webhook, the AI participant registration and the
  connection in one transaction and returns the runner configuration block (token and signing
  secret) once, with `Cache-Control: no-store`. `GET .../bot-connections`, `GET`, `PATCH`,
  `DELETE .../bot-connections/{bot_id}` and `.../{bot_id}/disable`, `/enable`, `/rotate` complete
  it; every write carries the connection's `version`. A project has at most one tmux connection per
  CLI type (409 naming the bot that holds it); headless connections are not limited. A task uses
  the connection of its project and its bot, and the task view names the tmux session
  (`session_name`, `<KEY>-<cli>`). Owners and admins only; bots and
  connected applications are refused. The runner address is checked with the outbound policy
  (`[outbound] allowed_hosts`). `GET` answers a status computed from the runner's report
  (`PUT /api/v1/ai/runner/report`, sanitised, 16 KiB limit), the token's use and the latest delivery.
  Rows a connection made are reported as `managed_by` on the Bots and Webhooks endpoints and are
  protected there. After a connection or a rotation the worker holds the bot's tasks until the runner
  has used the new token (at most one hour); runner refusals carry a stored `reason_code` and read
  "the runner refused the task (...)". Removing the last connection revokes the token and deletes
  the webhook and seat; deleting a project does the same. Alert rules warn with
  `bot_not_connected_to_project` instead of `bot_has_no_active_webhook`.
  Operations: `docs/runbooks/bot-connections.md`.
  Hardening: a runner bot receives a project's tasks only while the project has an enabled
  connection for it (task creation, worker pick-up and delivery; pending tasks without one are
  failed with a reason and an issue comment); its token reaches only the routes of a fixed table and
  only the projects it is connected to; it is never seated above member; a delivery connects to the
  address its URL check resolved, and a runner's answer body is never copied into an issue; plain
  `http://` runner addresses need a host in `[outbound] allowed_hosts`; rotation, the disable that
  switches the token off, removal and project deletion re-evaluate cached permissions and open
  sessions; disabling a bot's user stops its token.

- **MCP tool catalogue and task tools.** One catalogue entry per tool (category, read or write,
  summary, keywords), `annotations.readOnlyHint` in the full listing, `list-tools --json`, and the
  tools `sylvode_task_get`, `sylvode_task_ack`, `sylvode_task_progress`, `sylvode_task_complete` and
  `sylvode_task_fail` for the task protocol of a bot.

- **Sylvode as an OAuth 2.1 authorization server for its own API and MCP server (P4).** A person
  can connect Ramflux, the one pre-registered public client, to one workspace: Ramflux holds its own
  credentials and Sylvode proxies nothing. Off until `[oauth] issuer` is configured; see
  `docs/runbooks/oauth-authorization-server.md` for configuration, operation and rollback.
  - *Endpoints.* `POST /api/v1/oauth/authorization-requests` and `.../{request_id}/decision` (the
    consent flow with PKCE `S256`, exact redirect matching with loopback ports, one consent token
    per request), `POST /api/v1/oauth/token` (authorization code and refresh token grants, refresh
    rotation with reuse detection, code replay detection), `POST /api/v1/oauth/revoke` (RFC 7009),
    the RFC 8414 and RFC 9728 discovery documents, and `GET /api/v1/oauth/connection` for a
    connected application to describe its own grant. Tokens are opaque (`svc_`, `sva_`, `svr_`),
    stored as hashes, bound to one resource (API or MCP) and checked against live state on every
    request, so revocation is immediate.
  - *Scopes of the first release.* The client may be granted `projects:read`, `work_items:read`,
    `notifications:read`, `tasks:read` and `offline_access`. Write scopes, forms and surfaces are
    in the vocabulary and refused for this client. A delegated request acts as the person with the
    workspace role capped at `member`, inside the one workspace and, when chosen, the selected
    projects of the grant; routes outside a fixed allow-list are refused. Starting, assigning or
    re-dispatching an agent is never available to a connected application: the check sits in the
    function that creates an agent task.
  - *Event stream.* `GET /api/v1/oauth/events/stream` pushes `notification.created`,
    `notification.read`, `task.created` and `task.status_changed` as server-sent events, filtered
    when each is delivered (workspace, scope, owner, project set, present membership), with resume
    by `Last-Event-ID`, a bounded backlog, back-pressure, limits per grant, user and process, and
    closing control messages (`revoked`, `reauth`, `lagged`, `resync_required`). The worker writes
    the feed from a transactional outbox and prunes it. nginx serves the path unbuffered.
  - *Management.* A person lists and disconnects their connections; a workspace owner or admin
    lists and disconnects the workspace's and switches connected applications on or off; an
    instance administrator disables or re-enables the client. These accept a first-party session
    only. Password change, admin reset, disabling an account, removal from a workspace, disabling
    the client and switching a workspace off revoke the affected grants in the transaction of the
    change itself.
  - *MCP.* The MCP server accepts delegated tokens on `POST /mcp/rpc` and offers eight read tools
    (`projects.list`, `projects.get`, `context.get_project`, `work_items.list`, `work_items.get`,
    `work_items.get_by_identifier`, `work_items.list_labels`, `comments.list`); a project argument
    outside the grant is refused before the API is called. Search, the project label list, the forms
    tools and everything that writes stay available to bot and user tokens only.
  - *Deviations from the MCP authorization specification.* Only the pre-registered client exists:
    there is no dynamic client registration and no client metadata documents. The specification
    says an MCP server must not pass a received token through; this server forwards the caller's
    MCP-audience token to Sylvode's own API together with an internal forwarding credential
    (`oauth.internal_forward_secret`, the same value as `mcp.oauth_forward_secret`), and the API
    accepts an MCP-audience token only with it. The MCP server is a stateless facade over its own
    API, and the API is the issuer and the policy point for both audiences.
  - *Operations.* Migration `0109` (twelve tables, one seeded client, four triggers on
    `notifications` and `ai_tasks`); the worker prunes the event feed after
    `oauth.event_feed_retention_days`, audit rows after 180 days, operation logs after 90 days and
    expired requests, codes and tokens daily. Revocations, consent and token issue take row locks in
    one fixed order so they cannot deadlock or leave a usable token behind.

### Changed

- **Form plugin protocols 1 and 2 removed; protocol 3 is the only one.** Installing an
  `openpr.plugin.v2` manifest now fails with `manifest_protocol_unsupported`. The
  `plugin-panels` discovery and invoke routes, the host's bridge 1, the per-package layout code and
  the `SV_FORM_PLUGIN_LAYOUTS_ENABLED` switch are gone; the Plugin Creator's order amount template
  now builds a protocol 3 `wasm` package with the same `amount.*` capabilities and the same panel
  behaviour, so it needs `SV_FORM_PLUGIN_EXTENSIONS_ENABLED=1` like every other protocol 3
  package. No migration: `form_plugin_layouts` and rows of the retired protocols stay in the
  database, unread and unwritten (see `docs/universal-forms-and-plugins.md`).
- **Forms page restructured.** The Forms page is split into a state object, controllers, pure
  modules and components, with unit tests for the modules and a browser smoke for the
  interactions. Internal refactor, no behaviour change.
- **Flow writes no longer walk the whole page to guard the editor-model floor.** The isolated
  apply step used to read every editor block once before each import (twice on model 5 pages)
  only to refuse an update that lowers a page from editor model 5. It now reads the model 5
  marker before and after the import and scans the imported operations, which costs the same for
  a 200-block page and a 5,000-block page. The few updates that remove or re-type a block
  container, or touch the document root, fall back to an exact check. The same updates are
  refused as before (`editor_model_floor_downgrade`). The semantic snapshot now also yields the
  floor of the state it read, so the host no longer walks a second time. The guard never trusts
  the fields of an operation that a client chooses: every deletion on the top-level block list
  goes to the exact check, which decides from the document's actual state. Four differences
  affect invalid or hostile input only: a base document that fails the current rules is no
  longer re-checked before the import (the check after the import still rejects an invalid
  result); an update that is invalid in several ways at once may report a different first
  reason; and the editor document, the structure index and its metadata are read through their
  typed root containers everywhere, so a second root container with the same name and another
  type no longer makes the floor check and the page reader disagree (before, such a page could
  fail the floor check on replay until the next snapshot compaction); and, as the fourth, an
  update from a client that creates or writes a root container with a reserved name
  (`doc`, `flow_editor_structure`, `flow_editor_structure_meta`, `flow_editor_aliases`, `order`,
  `tree`, `meta`) and a container type other than the expected one is refused as an invalid update
  (`reserved_root_container_type`), as is an update whose operations cannot all be inspected
  (`reserved_root_scan_incomplete`). Such a second root made every later write of that page read
  the whole document more than once, and compaction does not remove it. No product client
  creates one. The rule applies to updates a client offers for acceptance only: replay of stored
  history, snapshot rebuilds and integrity checks still accept a page that already carries such a
  root, and the check costs the update, not the page. An uploaded package whose document carries
  such a root, in its snapshot or in its update tail, is refused the same way on import.
- **The webhook receiver is now the Sylvode runner.** The crate is `sylvode-runner` in
  `apps/runner`, the executable is `sylvode-runner`, the release archive is
  `sylvode-runner-<target>.tar.gz` and the image is `ghcr.io/openprx/sylvode-runner`. The old
  receiver names are not kept: the executables `sylvode-webhook` and `openpr-webhook`, their
  archives and images, the `SYLVODE_WEBHOOK_*` and `OPENPR_WEBHOOK_*` variables, the `sylvode_webhook`
  and `openpr_webhook` log targets, the `X-OpenPR-Signature` inbound header and the
  `X-OpenPR-MCP-Surface` callback header are gone. The callback `source` and the `no_agent` answer
  name `sylvode-runner`.
  - *Deployment.* The compose service is `runner` (`SYLVODE_RUNNER_PORT`, `SYLVODE_RUNNER_IMAGE`,
    `SYLVODE_RUNNER_CONFIG`), and the generated `[outbound] allowed_hosts` lists `runner:9090`. The
    example configuration is `config/sylvode-runner.example.toml`, the smoke scripts are
    `scripts/smoke-runner-routing.sh` and `scripts/smoke-browser-mention-runner.sh`
    (`SYLVODE_RUNNER_DIR`), and the installer manages `sylvode-runner.service` (Linux) or
    `dev.sylvode.runner` (macOS) only.

### Removed

- **The AI participants API and page.** `GET/POST /api/v1/projects/{id}/ai-participants`, its
  `{id}` routes and `/stats`, the AI participants page, the overview's assistant cards and the
  placeholder participants a scenario template used to create are gone: a bot is a project's AI
  participant only through a bot connection. Migration `0112` makes the participant key
  `(project_id, id)` (one bot, several projects), drops the stored-only columns `model`,
  `provider`, `api_endpoint` and `capabilities`, and drops the learning records' foreign key.
  Learning records are read per project: `GET /api/v1/projects/{id}/ai-participants/{bot_id}/learning`
  and `.../alignment-stats` replace `/api/v1/ai-participants/{id}/...`. The
  project-level execution settings endpoints are removed (each connection holds its settings), and
  the project context lists them as `executions`, one per connection. The worker's end reason `receiver_declined` is now `runner_declined`.

### Fixed

- **A Flow package import validates every uploaded document in the isolated worker.** The
  workspace package import used to decode the uploaded snapshot and update tail inside the API
  process and the import transaction, with no content limit and no meter, and stored the uploaded
  snapshot as it was. Both package imports (workspace and personal scope) now run each document
  through the import operation of the worker (load, replay, scans of the whole history for reserved
  roots and unresolved tree references, projection, every content limit) before the transaction
  opens, and store the head the worker returned after the API process has loaded it again and
  compared its own projection, title, reference verdict and fingerprint with the worker's report.
  A package that exceeds a content limit or an import ceiling is refused as `limit_exceeded`
  naming the limit, and nothing is stored. The import window has its own named budget
  (`IMPORT_DECODE_CPU_MS_MAX` 2,000 ms, `IMPORT_DECODE_WALL_MS_MAX` 5,000 ms,
  `IMPORT_ISOLATED_MEMORY_BYTES_MAX` 512 MiB), applied by that operation only; the write window
  keeps 50 ms, 100 ms and 128 MiB. Off Linux a package import is refused as
  `isolated_apply_unsupported_platform`. A portable graph package whose versions need more than
  16,384 mark edits to replay is refused (`portable_graph_replay_work`), at export too, and the
  replay no longer runs on the async executor.

- **Deleting a workspace or a project that holds content works.** A workspace with a Flow Page
  that had an accepted collaboration update answered `database error` and stayed, because the
  delete cascade reached the event row before the update that references it. Workspace and project
  deletion now remove the rows that block the cascade first (accepted updates, event dispatch and
  delivery rows, import jobs, scope command receipts, conversion jobs, previews and lineage,
  plugin bindings, activities and notifications) inside the same transaction, and remove the
  workspace's legacy Flow scope with it. No foreign key changed. If content outside the deleted
  workspace or project still restricts it, the request is refused with a typed `invalid_update`
  (`details.reason = "retained_by_external_reference"`) and nothing is deleted.
- **Flow: large Pages stay writable on slower databases.** Every statement of the block registry
  whose size depended on the Page (the locking read, the removal of absent blocks, the legacy
  comment seal and the bulk writes) now carries at most 500 rows. The first registration of an
  existing Page is resumable: each accepted write registers new blocks only while it fits its
  staging budget and records the rest in `flow_block_checkpoints.pending_registrations`
  (migration `0107`, additive). Comment lists, anchored comments, block shares and transfers
  finish the registration in short capped steps before they read block identities.


### Added

- **Plugin protocol 3: one package, several locations.** A package with `schema_version`
  `openpr.plugin.v3` declares 1 to 8 typed contributions, each with a stable id, a kind, a
  location (`slot`), a label and the entry view it opens, and all of them share one set of
  grants. Besides the two record locations (`form.record.detail`, `form.record.actions`) there
  are two new ones: `project.view`, a routed project app listed under the project page's apps
  and opened at `/workspace/{w}/projects/{p}/apps/{pluginKey}/{route}` (at most two per
  package, routes isolated by plugin key), and `form.list.toolbar`, a button in the Forms page
  header that opens the plugin in a side drawer for the selected Form. Protocol 1 and 2
  packages keep working and are ordered together with protocol 3 contributions in the record
  locations. New discovery endpoints (project, Form and record level) list the contributions a
  member can see, and a separate entry endpoint returns the HTML of one of them, so a retained
  direct link stops working once the contribution is revoked. The whole feature is behind
  `SV_FORM_PLUGIN_EXTENSIONS_ENABLED`; see **Upgrade notes**. A sample `ui` and `wasm`
  package and an isolated acceptance script are in `docs/plugins/extension-example`.
- **UI-only plugins.** A protocol 3 package with `profile` `ui` carries no WebAssembly module
  and declares no capabilities. An administrator grants it `ui.display` and, per declared read
  (at most four, of up to 32 fields each), a `read:<id>` grant. Through a read-only data
  endpoint it can describe a Form, page through its records (up to 50 per page, with a signed
  cursor bound to the Form and the read) and read one record. The server applies record scope
  and the field read policy, returns only declared fields of plain value types (never
  attachments, images, signatures, members, relations, child tables, AI summaries or the
  record title), limits each user to 30 requests per 10 seconds per project and plugin, and
  logs each read without its values. Packages with `profile` `wasm` keep the order-amount
  capabilities and are invoked through a contribution-aware endpoint that records the
  contribution in the invocation log.
- **Plugin bridge protocol 2.** Protocol 3 frames talk to the host over a message port with
  `ready`, `close`, `resize`, `request`, `result` and `event` messages. Requests are limited to
  an allow list (`host.context`, `host.navigate`, `data.describe`, `data.query`,
  `data.record`, `amount.preview`, `amount.apply`), each needing its grant; `host.navigate`
  accepts only an enabled view route of the same package, a record or the project, never a
  URL. The host sends `context` when theme or language changes and `suspended` when the
  contribution is revoked, its version changes or the session changes. Both sandbox layers
  and the content security policy of the plugin frame are unchanged, and a plugin still never
  receives a session or a token. Protocol 1 frames keep their previous rules.
- **Project layout and personal preferences for protocol 3.** Project administrators enable,
  disable and position each contribution; the layout is stored per project, plugin key and
  contribution, so it carries across upgrades and rollbacks between versions of the same
  plugin. Each member can hide, collapse into a "More" menu or reorder contributions for
  themselves (up to 256 preferences per project) from a preference menu at each location,
  without affecting anyone else. Hiding is a display choice, not an access control.
  **Configuration reset** (`POST .../plugin-packages/{key}/configuration/reset`,
  administrators, only while the plugin is disabled) deletes the project layout and every
  member's preferences for that key.
- **Creator support for protocol 3.** The plugin Creator picks a template from a registry. The
  new UI view template builds a `ui` package for any of the four locations; the order-amount
  template still produces a protocol 2 package. Protocol 3 versions can be previewed with
  sample data, saved to the personal library, requested for and admitted to projects, and are
  shown in the library, project settings and history.
- **Webhook receiver: AI task completion callback.** The worker marks an AI task `processing`
  when it delivers it, and nothing ended that state. With the new `[ai_task_callback]` section
  (disabled by default; per-agent `[agents.ai_task_callback]` overrides) the agent that ran a
  worker AI task calls `POST /api/v1/ai/callbacks/task/{id}/complete` after a successful run
  and `.../fail` otherwise, using the bot token; refusals are not retried, transport errors and
  server errors are retried with a doubling delay. With the callback enabled, a worker AI task
  that no agent matches is failed at once with a message naming the bot, using the token of
  that bot (see **Webhook receiver: one callback token per bot**). See
  `apps/webhook/README.md` and `config/sylvode-webhook.example.toml`.
- **Webhook receiver: one callback token per bot.** A bot token can end only the tasks of its
  own bot, so a receiver serving several bots needs several tokens. `[[ai_task_callback.bots]]`
  (`bot_id`, `token`, optional `surface`) gives a bot its token, used for its tasks that no
  agent token covers and for its tasks no agent matches; the new `ai_task_callback.bot_id`
  names the owner of the global token, which is then used only for that bot. A task whose bot
  has no token is no longer reported with another bot's token (which the API refused with
  403): it is logged once at warn level and answered `"ai_task_callback": "no_token"`.
  Callbacks now name their attempt (`?attempt=N`), the fail callback of a task the receiver
  did not run sends `"retry": false`, and a matched agent that cannot start (safe mode,
  `cli_enabled = false`, an incomplete agent section) is answered `refused` instead of
  `dispatched`.
- **Worker: processing timeout for AI tasks.** A delivered AI task that no callback ends within
  the new `[ai_tasks] processing_timeout_secs` (default 3600, range 60 to 604800) is re-queued
  while attempts remain and failed after the last one, with an error message naming the timeout
  and the attempt. The sweep runs on every worker tick, claims rows with `FOR UPDATE SKIP
  LOCKED` and is safe with several workers.
- **AI task callbacks: attempt guard and `retry`.** `POST /api/v1/ai/callbacks/task/{id}/complete`
  and `.../fail` accept an optional `?attempt=N`; with it, the call is accepted only while that
  attempt is `processing` and refused with code 400 otherwise. `fail` accepts `"retry": false`
  to fail the task at once instead of scheduling a retry. Who may call them is unchanged: the
  task's own AI participant or a system administrator.
- **Webhook signing secrets can be rotated.** The new
  `POST /api/v1/workspaces/{id}/webhooks/{webhook_id}/rotate-secret` (workspace admin or owner)
  replaces a webhook's signing secret with a generated one and returns it once. The web client
  has a matching `webhooksApi.rotateSecret`.

### Changed

- **Forms record list: opens as a table, simpler views, read-only record panel.** Every form
  now has a built-in **All records** table, and a form opens with it unless the URL names a
  view (`?view=`), the user chose another view before (remembered per user and form on this
  device), or an admin set a default view; template-created views no longer decide the landing
  view. The "Saved views" panel is replaced by a view bar: a view switcher, quick **Filter**,
  **Sort** and **Fields** controls and sortable column headers that change the list
  immediately and are kept for the user without saving, a **Modified** state with **Reset**,
  and **Save view…**, which asks only for a name (and, for form designers, whether everyone or
  only the user sees it; "Only me" is the default). Renaming, sharing, setting the default,
  updating and deleting (with a confirmation) live in the view's menu; the per-type parameters
  moved to a **Layout settings** sheet that shows only the controls of the current view type.
  Clicking a row (or Enter on it) opens the record read-only in a side panel beside the list:
  fields follow the detail layout and are rendered by type (formatted numbers, amounts and
  dates, option badges, members, links, image and signature previews, attachments with name and
  size, relation chips, rich text as plain text, never HTML), with previous / next, copy,
  **Edit** (opens the existing editor), **Open full page**, and plugin panels in a collapsed
  **Apps** section that is mounted only when expanded. The panel docks beside the list on wide
  screens, floats over it on medium widths and is a modal sheet on phones; `?record=` reflects
  the open record, Back closes it, the arrow keys move between records without adding history
  entries, and Esc returns focus to the row. Row edit and delete buttons are shown only to users
  who may use them. Links, child records and comments are edited from the panel's menu
  (**Links and child records…**).
- **Personal views no longer need form design permission.** Saving, renaming, updating and
  deleting a view that only its creator can see now needs only permission to view the form's
  records; creating or changing a shared view, or setting the default view, still needs
  `form.design`. A private view is listed, readable and usable as a `view_id` only by its
  creator (form admins and bots no longer see other people's private views), cannot be the
  default view, and can be made private only by its creator. Duplicating a form copies the
  caller's own private views but not other people's, a personal view no longer blocks removing
  a field it uses, and removing a member from a workspace archives their private views.

- **Webhook receiver: now part of this repository.** The receiver (crate `sylvode-webhook`,
  executables `sylvode-webhook` and the legacy `openpr-webhook`, formerly OpenPR Webhook) is
  imported with its history as `apps/webhook` and built as a workspace member, with its own
  path-filtered CI job on Linux and macOS. **Its version now follows Sylvode**:
  `sylvode-webhook --version` reports the workspace version, and the compose `webhook`
  service builds its image from `apps/webhook/Dockerfile` when no image of that name is
  present. The standalone 0.3.x line ends here: the last standalone version, 0.3.53, was never
  released, and the last published standalone release is 0.3.3
  (`ghcr.io/openprx/openpr-webhook`). The standalone history is frozen in
  `apps/webhook/CHANGELOG.md`, and the standalone release workflows were removed.
- **Webhook receiver: CLI executors no longer bypass their permission checks by default.**
  `claude-code` always ran with `--permission-mode bypassPermissions` and `codex` with
  `--dangerously-bypass-approvals-and-sandbox`, with no way to restrict tools. Permissions are
  now set per agent in `[agents.cli]`: claude-code `permission_mode`, `allowed_tools`,
  `disallowed_tools` and `strict_mcp_config`; codex `sandbox`. The defaults are claude-code
  `--permission-mode dontAsk` with `--allowedTools mcp__<server>` for every server named in
  `mcp_config_path` and `--strict-mcp-config`, and codex `--sandbox read-only`: an agent can
  use its MCP tools and read its working directory, but cannot run shell commands or edit
  files unless configured to. `opencode` is unchanged and accepts none of these keys. Turning
  the checks off needs `dangerously_bypass_permissions = true` (with
  `permission_mode = "bypassPermissions"` for claude-code; alone or with
  `sandbox = "danger-full-access"` for codex) and logs a warning at start-up. An unknown mode,
  a key of another executor, or a bypass without the flag refuses start-up, and start-up logs
  the settings of every `cli` agent. See "Executor permissions" in `apps/webhook/README.md`.
- **Webhook receiver: a route that sets `bot_ids` or `bot_names` matches only those bots.** An
  unset list used to count as a match, so a route with only `bot_ids = ["bot-1"]` (or only
  `bot_names`) accepted tasks for every bot. Now a configured list must contain the task's bot;
  an omitted or empty list restricts nothing; when both lists are set, either one is enough
  (unchanged). A task that no route matches any more is answered `no_agent`.
- **Webhook receiver: Unix only.** The receiver relies on process groups and signals. On a
  non-Unix target the library is empty and both executables print
  `sylvode-webhook is not supported on this platform` and exit 1, so `cargo build --workspace`
  still succeeds there; no Windows build of the receiver is shipped.
- **One assignment or mention of a bot is one bot task.** Creating an issue assigned to a bot
  ran the bot's agent three times (the `issue.created` and `issue.assigned` webhook deliveries
  with `bot_context`, and the worker `issue_assigned` AI task); assigning an existing issue
  ran it twice, and so did a mention (`comment.created` with `bot_context` and the
  `comment_requested` task). An assignment or mention of a bot now queues only the worker AI
  task, and every webhook delivery of the same request reaches that bot without
  `bot_context`, as a plain notification. Other activity on a bot's issue still reaches it with
  `bot_context`. Reassigning an issue back to a bot it was assigned to before now queues a new
  task instead of being dropped as a duplicate of the first one. See "Bot tasks" in
  `README.md`.
- **Token bots are ordinary bots.** `POST /api/v1/workspaces/{id}/bots` created the user behind
  an `opr_` token with `entity_type = 'bot_mcp'`, while assignment to a bot, a webhook's
  `bot_user_id`, AI participants, bot mentions, bot context on webhook deliveries and AI
  authorship in governance all require `entity_type = 'bot'`. There is now one automation
  identity type: the bots API creates `entity_type = 'bot'` with `agent_type = 'mcp'`, and
  migration `0105` moves every existing `bot_mcp` user to `bot` (agent type, membership, tokens
  and history unchanged; every issued token keeps working). Being a bot grants nothing by
  itself: a token's permissions are still decided by the token, and an AI task can still only
  be completed or failed by its own participant or a system administrator.
- **Project resource kinds are reduced to `directory` and `custom`.** The retired kinds
  (`repo`, `document_library`, `crm_account`, `erp_order`, `equipment`, `site`) are rejected on
  create and update by the API and the MCP tools. Migration `0104` converts existing `repo`
  rows to `directory` and every other retired kind to `custom`, keeping the original kind in
  the new column `project_resources.legacy_kind`, and remaps the suggested resources in project
  types, scenario templates and projects the same way, dropping entries that become exact
  duplicates (for example a `repo` and a `directory` suggestion that are otherwise equal).
- **PostgreSQL 18 is the supported database version.** CI, the benchmark workflow, the compose
  file, the restore drill and the documentation use `postgres:18`; older major versions are no
  longer supported. The compose data volume is now mounted at `/var/lib/postgresql` instead of
  `/var/lib/postgresql/data`, because the 18 image keeps its data directory at
  `/var/lib/postgresql/18/docker`. Backups and restores need the PostgreSQL 18 client tools.
- **A protocol 2 manifest that does not parse** is now refused with the typed reason
  `manifest_malformed` instead of an untyped error. Install and library save dispatch on
  `schema_version`; an unknown value is `manifest_protocol_unsupported`. The plugin catalog
  reports `extension_supported`, binding history entries carry `reason`, and invocation log
  entries carry `contribution_id` (`null` for protocol 1 and 2).
- **Flow write-path performance.** The isolated-apply worker's CPU window spends most of its
  time walking every editor block. The editor-model floor is now cached per document state and
  computed from only what it needs, placements are looked up through a hash index instead of a
  linear scan per block (which was quadratic on structured pages), each placement's block id is
  read once, and the semantic snapshot reads the editor document in a single pass instead of
  two. Output is unchanged: a pinned digest corpus of reader output and typed errors guards it,
  and walk counters assert the number of document walks per write and per rebuild. This
  restores headroom under the 50 ms `decode_apply_cpu_ms_max` limit for flat pages; it does not
  establish that structured (model 4 and 5) pages near 5,000 blocks stay within that limit.
  `cargo run --release -p collab-core --example flow_import_window` measures the window locally
  for 1,400 and 5,000 block pages.
- **The plugin package concurrency budget** (four concurrent runs per process) is now covered by
  a unit test.
- **Flow: personal Flow and the shipped installation-wide Flow capabilities are on by default.** Migration `0106_flow_capabilities_default_on.sql` defaults `personal_provisioning`, `personal_writes`, `external_mounts`, `personal_forms_conversion`, `block_transfers_enabled`, `portable_page_packages_enabled` and `portable_graph_packages_enabled` on and raises `minimum_reader_version` to 5, so a fresh installation needs no manual SQL before users can open a personal Flow space and write Pages. `scope_native_legacy`, `personal_package_imports`, `personal_scope_package_exports` and `cross_workspace_forms_conversion` were switched on later by `0115_flow_default_on.sql`. See `docs/runbooks/flow-runtime-capabilities.md`.

### Fixed

- **A worker AI task no longer stays `processing` for good.** When one webhook receiver served
  several bots, a task for a bot without an agent was answered `no_agent` and then failed with
  the receiver's global token, which belonged to another bot; the API refused it (403) and
  nothing else ever ended the task. The worker now fails a task the receiver declines in its
  answer (`no_agent`, `ignored`, `refused`, or an HTTP 4xx other than 408, 409, 425 and 429) at
  once, with an error message quoting the answer, and takes back every task left `processing`
  past the processing timeout (see **Added**). Each such end writes the `retried` or `failed`
  task event and business event with a `reason` (`receiver_declined`, `delivery_failed`,
  `processing_timeout`), and a failed task sends `ai.task_failed`, which tasks the worker failed
  after their last delivery attempt did not send before.
- **AI task callbacks no longer race.** `complete` updated the task without checking that it was
  still `processing`, so a complete and a fail arriving together, or a complete racing a worker
  retry, could overwrite one another. Both callbacks are now a compare-and-set on the status and
  attempt they checked, and the loser is refused with code 400 instead of overwriting the
  winner.

- **Spurious `stale_frontier` rejections in Flow.** Loro serializes a version vector in hash-map
  order, so the same causal version could encode to different bytes depending on whether a
  document was imported incrementally or rebuilt from a snapshot and its tail; history restore
  hit this. Frontiers are now encoded canonically (peer-sorted), and every API and
  isolated-apply worker check compares frontiers as version vectors, so frontiers already
  stored in the old byte order keep matching. **Client-visible:** an expected frontier that
  does not decode is now `invalid_update` (400) instead of `stale_frontier` (409); Page
  transfer prepare answers it with `policy_rejected` and `details.reason =
  "transfer_invalid_frontier"` instead of `transfer_source_changed` or
  `transfer_target_changed`; a collaboration reconnect that names such a frontier falls back to
  a full snapshot.
- **A generated webhook secret can be retrieved.** `POST /api/v1/workspaces/{id}/webhooks`
  generates a signing secret when none is supplied, but no response returned it, so a receiver
  could never verify deliveries signed with it. The create response now carries `secret`, once;
  list, get and update responses and the webhook's business events still never contain it. The
  web client types follow (`WebhookWithSecret`), and the edit form no longer pre-fills a secret
  it could never receive.
- **A bot no longer triggers itself.** A bot's own action (a state change, an edit, a comment,
  assigning an issue to itself, completing its task) was delivered back to that bot with
  `bot_context`, so an agent that set its issue to done ran again. Such deliveries now go out
  without `bot_context`, and a self-assignment queues no AI task. A person's or another bot's
  change to the issue still reaches the assigned bot.
- **A suspended plugin frame sees the `suspended` event.** The host posted the event and removed
  the frame in the same update, so the plugin never observed it. A suspended frame now stops
  being served at once, receives the event, stays mounted but inert for a fixed 300 ms grace
  period and is then removed, also when its parent unmounts it. `host.navigate` now replies
  before navigating and refuses other requests from the frame until the navigation settles.
- **Plugin panels and drawers reopen after a quick Back.** Leaving a page and coming back within
  the 300 ms grace period of a suspended frame left the record panel, the Forms toolbar drawer
  and the Creator preview with a "suspended" notice instead of the frame; closing and reopening
  one within the grace period had the same result. Each open now gets its own frame, and a frame
  retired that way reopens with a fresh entry, unless the contribution was revoked or changed
  meanwhile (then the usual notice shows) or the user had closed it. A pending apply
  confirmation is dismissed and the panel reopens idle.
- **Webhook receiver: macOS build.** The receiver no longer fails the build on macOS and other
  non-Linux targets with a dead-code error (`Leader::ExitedUnreaped` is built only on Linux).
  This failure also broke the standalone 0.3.53 release.
- **A Page edit while a reader lease is missing is refused as such.** It is now refused with
  `feature_disabled` (`scope_runtime_not_ready`) on every surface, as the runbook states, instead
  of `server_draining`/`contention`. Other typed refusals raised while a content write is staged
  (for example a policy refusal on a child reference) also reach the caller at once instead of
  being retried and reported as contention.

### Security

- **wasmtime 50.0.2.** Upgraded wasmtime from 49.0.5 to 50.0.2 (Cranelift 0.137.2, wasm-tools
  0.261), which clears RUSTSEC-2026-0566, RUSTSEC-2026-0567, RUSTSEC-2026-0568,
  RUSTSEC-2026-0569, RUSTSEC-2026-0570, RUSTSEC-2026-0571, RUSTSEC-2026-0572 and
  RUSTSEC-2026-0573. The plugin runtime keeps the same engine configuration and store limits,
  and the guest-visible WebAssembly feature set is unchanged. The unused compilation cache stays
  disabled; its zstd 0.13 chain now has patched releases, so that is no longer a security
  constraint. Wasmtime 50 also makes fuel accounting stricter in a few corner cases, so a
  plugin that runs close to its `runtime.fuel` budget can report slightly more fuel consumed.
- **Webhook receiver executors run restricted by default.** See **Changed**: the claude-code
  and codex executors no longer turn off their own permission checks unless a configuration
  asks for it explicitly.

### Upgrade notes

- **Apply the database migrations `0103`, `0104`, `0105` and `0106`.** `0103` adds the protocol 3
  layout and preference tables and the `contribution_id` column of the invocation log; `0104`
  converts project resource kinds; `0105` moves token bots to `bot`. All three are re-runnable
  and no-ops where nothing matches.
- **PostgreSQL 18.** Upgrade the database server to PostgreSQL 18 before deploying. A major
  version upgrade needs `pg_dump`/restore or `pg_upgrade`; PostgreSQL does not open a data
  directory of an older major version. Compose users with an existing `pgdata` volume: that
  volume holds a PostgreSQL 16 data directory at its root, which the new mount point and image
  do not use, so starting the new compose file against it does not bring your data up. Dump
  the database with the old compose file first, then restore it into a new volume (or a
  renamed one) under the new file. Use the PostgreSQL 18 `pg_dump` and `psql` for backups and
  restores from now on.
- **Protocol 3 plugins are off by default.** Set `SV_FORM_PLUGIN_EXTENSIONS_ENABLED=1`
  (together with `SV_FORM_PLUGIN_PACKAGES_ENABLED=1`) to enable them. Setting it back to `0`
  hides every protocol 3 contribution and refuses its data and invoke requests without losing
  layouts or preferences; administrators can still disable protocol 3 bindings.
- **Rolling back to 0.2.539.** If any protocol 3 package has been installed or saved to a
  personal library, start the older API with `SV_FORM_PLUGIN_PACKAGES_ENABLED=0`: its web pages
  cannot display a protocol 3 manifest. Running 0.2.539 against a database migrated by `0104`
  has not been verified; an older API that tries to create a resource with a retired kind gets
  a database error from the tightened constraint instead of a validation error.
- **Project resource kinds.** Clients and scripts that create or update resources with `repo`,
  `document_library`, `crm_account`, `erp_order`, `equipment` or `site` must use `directory` or
  `custom`. Existing rows are converted by `0104`, so code that filtered on a retired kind
  must filter on the new kind; the original kind is kept only in the database column
  `project_resources.legacy_kind`, which the API does not return.
- **Token bots are bots.** After `0105` existing token bots behave like every other bot:
  proposals, votes, proposal comments, vetoes and appeals they make are recorded as AI
  (`author_type = ai`) instead of human, and an AI vote or proposal comment by such a bot needs
  the AI participant permission for the domain. Grant that permission to token bots that take
  part in governance. Their mentions queue AI tasks, and they appear in the bot pickers of the
  web UI.
- **Bot assignments arrive as worker AI tasks only.** The assignment and the mention of a bot
  never arrive as a webhook delivery with `bot_context.trigger_reason` `created`, `assigned`
  (for `issue.assigned`) or `mentioned` any more. A `sylvode-webhook` route that relied on
  those, for example `trigger_kinds = ["created"]` or `events = ["issue.assigned"]`, no longer
  matches the assignment: match the worker AI task instead (`trigger_kinds = ["assigned",
  "mention"]`, no `events` list), and make sure the worker is running.
- **Webhook receiver executor permissions.** A `claude-code` or `codex` agent that edits code or
  runs commands stops being able to do so after the upgrade. Give it `allowed_tools` (for
  example `Edit(...)`, `Bash(cargo test *)`) or `permission_mode = "acceptEdits"`, or codex
  `sandbox = "workspace-write"`. To keep the old unrestricted behaviour, set
  `dangerously_bypass_permissions = true` with `permission_mode = "bypassPermissions"`
  (claude-code) or on its own (codex).
- **Webhook receiver route matching.** A route with only a `bot_ids` or only a `bot_names` list
  that served as a catch-all for every bot now matches only the listed bots; other bots' tasks
  are answered `no_agent` (and, with the AI task callback enabled, failed). To keep a
  catch-all, remove `bot_ids` / `bot_names` from that route, keeping `bot_agent_types` or other
  lists if you want it narrower.
- **Webhook receiver version numbers.** The receiver now carries the Sylvode version, starting
  with 0.5.0, which continues above the standalone 0.3.x line (last published standalone
  release 0.3.3; 0.3.53 was never released). When upgrading from 0.3.3, also read "Upgrading
  from 0.3.3" in `apps/webhook/CHANGELOG.md`, which lists the configurations that are now
  refused at start-up.
- **Flow capabilities are switched on once.** Upgrading an existing database switches the capabilities listed above on once, including on installations that had them off; values that were already on and a higher `minimum_reader_version` are kept. To keep any of them off, switch it off right after the migration, before opening traffic (personal Flow: `UPDATE flow_runtime_capabilities SET external_mounts=false, personal_writes=false, personal_provisioning=false, updated_at=now() WHERE singleton;`). The choice survives restarts and migration replays; do not delete the `flow_capabilities_default_on` row in `flow_runtime_capability_rollouts`.
- **Worker AI tasks: new end rules.** After the upgrade a task the receiver answers `no_agent`,
  `ignored`, `refused` or with an HTTP 4xx (other than 408, 409, 425, 429) is failed on the
  first delivery instead of being retried until its attempts run out; fix the receiver
  configuration and assign the task again. Tasks that are `processing` when the new worker
  starts, including tasks stuck by the defect above, are swept as soon as they are older than
  `[ai_tasks] processing_timeout_secs`: re-queued (and delivered again) while attempts remain,
  failed otherwise. To fail old stuck tasks instead of re-delivering them, call `.../fail` with
  `"retry": false` for them after the new API is up and before the new worker starts. Set the timeout well above the longest run of any receiver's agents,
  including the wait for a free `cli_max_concurrency` slot, or a slow run is delivered twice.
  No database migration is needed.
- **Webhook receiver serving several bots.** Set `ai_task_callback.bot_id` to the bot that owns
  `ai_task_callback.token` and add a `[[ai_task_callback.bots]]` entry for each other bot;
  start-up logs a warning while `bot_id` is not set. Integrations that read the `/webhook`
  answer should expect `refused` (agent not started) besides `dispatched`, and the new
  `reason` and `ai_task_callback` fields. Business events of attempts the worker ends now carry
  `source.type = "worker"` and `event_payload.reason`; their `metadata` no longer carries
  `worker_event`.
- **Run the worker alongside the API.** With the default `minimum_reader_version` of 5, every
  Page creation and every Page edit (in workspaces and personal spaces, including plain Pages
  created through the REST v1 API, MCP or the CLI), every personal-space write, block transfers
  and portable packages are refused with `scope_runtime_not_ready` until the API and the worker
  (which registers both the `worker` and the `maintenance` lease) hold live reader-5 leases. An
  installation that runs the API without the worker can read Flow content but cannot write
  Pages; a worker that is down for more than 30 seconds stops Page writes until it is back.

## [0.2.539] - 2026-11-18

### Added

- **Personal plugin library** at `/plugins`. Create or import a private, immutable plugin version, run its Web/WASM interface with personal input, and request admission to one or more projects. Each project reviews the submitted version and manages its own activation, permissions and layout.
- Paired Web and WASM releases, a template-based Form plugin Creator, two configurable record interface positions, and project administration for permissions, versions, rollback and operation history.
- Flow Page authoring, inline media and child links, block sharing, cross-page move/copy with undo, multi-object portable ZIPs with mapped references and retained history, mount permissions, and conversion from explicit Page fields to a runnable Form.

### Upgrade notes

- Apply the included database migrations through `0102`. Keep the API and its bundled `collab-isolated-apply-worker` together. Review the feature settings before enabling the new workflows.
- Set `SV_FORM_PLUGIN_PACKAGES_ENABLED=1` to enable paired packages and the personal plugin library. Set `SV_FORM_PLUGIN_LAYOUTS_ENABLED=1` for multi-position packages. Existing project-installed packages retain their bindings.
- When downgrading to an API that predates personal library links, set `SV_FORM_PLUGIN_PACKAGES_ENABLED=0` on that older API if library links exist. It cannot enforce their revocation state.


- Independent personal Flow scopes, scope-aware sharing, lifecycle, durable events and bounded import/export compatibility.
- Page-focused Flow navigation and editor tools, plus an eight-tab workspace with retained drafts, drag ordering and safe refresh.
- Atomic optional version checks for Forms, projects, resources and assistant configuration to prevent stale saves.

### Changed

- Flow Collection/Record UI entry points are hidden while existing data and validated backend APIs remain available.
- Browser CI and its binary producer/consumers use Ubuntu 24.04, supported by the locked Playwright version.
- Source Docker builds include the Flow benchmark workspace member. Page rich-text, block movement, presence and package-reference limitations remain explicitly bounded.

### Security

- Replaced Chrono with `time` and updated Loro to 1.18.0 (Rust and WebAssembly), which uses the maintained `imbl` implementation. Updated SeaORM's derive dependency to `sea-bae` 0.2.2. The lockfile no longer includes Chrono, im, bitmaps, sized-chunks or proc-macro-error2. Removed the four obsolete advisory exceptions. Patched event-listener and rustix for RUSTSEC-2026-0221 and RUSTSEC-2026-0486.
- Updated quinn-udp to 0.5.16 for RUSTSEC-2026-0538. The lock-only SQLx SQLite advisory RUSTSEC-2026-0459 is explicitly excepted only while a mandatory all-target/all-feature normal/build/dev dependency guard proves SQLite unreachable; SQLite itself was not upgraded or declared patched.

- Patched RustSec findings in the lockfile: rustls 0.23.46, rustls-webpki 0.103.16,
  webpki-roots 1.0.11, wasmtime 49.0.5, zip 9.2, tokio 1.54.1 (RUSTSEC-2026-0386),
  and quinn-proto 0.11.19 (RUSTSEC-2026-0338 through 0342). The unused wasmtime
  compilation cache is no longer enabled, which drops the unfixed zstd 0.13 stack
  (no published wasmtime release depends on zstd 0.14).
- User access and refresh tokens now carry `users.session_version`. Deactivating an account,
  deleting it, or changing its password increments that version, and login, refresh and the
  authentication middleware reject inactive accounts and tokens whose version no longer matches.
- Markdown rendering no longer re-inserts raw `<video>` HTML. The default frontend nginx config
  sends the same Content-Security-Policy as the reference Caddy file.
- `GET /api/v1/workspaces/{id}/users` returns the whole instance directory only to instance
  admins. Workspace admins see members of that workspace.
- The first registration becomes the instance admin under a transaction advisory lock, so two
  concurrent registrations cannot both observe an empty user table.
- Flow collaboration diagnostics and shallow verify require object `view`, not only workspace
  membership.

This release renames the product from OpenPR to Sylvode and adds Sylvode Flow. Read
**Breaking and behaviour changes** before upgrading. Every legacy
OpenPR name keeps working; see **Deprecated**.

### Fixed

- The static-build gate looks for SvelteKit 3's optimized client manifest under
  `.svelte-kit/generated/build/client-optimized`.
- The eight Flow comment and share MCP tools are recorded as `v0_9_rebase` in the
  tool-registry baseline, so the rebase chain still ends at the live tool count.
- MCP guides state the live registry size of 148 tools. The object panel keyboard
  test moves Info to Comments, then to Share.
- Public link create and revoke stay inside the share panel's manage gate, so a
  view-only member does not see those controls.
- A cancelled plugin invoke or event-handler hook still writes its invocation row.
  The recording task is scheduled before the handler waits on the database, so
  dropping the request can no longer skip it.
- Package export accepts a git committer date written with a `Z` UTC suffix. GitHub's
  git prints `%cI` that way; the build used to discard the commit as unknown and every
  package export then failed closed.
- The depth-32 lock-hold sample check accepts an extra sample when a lock timeout is
  resubmitted. The parallel suite was failing with 31 samples against the exact count of 30.
- A cancelled plugin invocation is allowed 60 seconds to land its row. A loaded CI
  runner was still writing it after the previous 20 second wait.

### Breaking and behaviour changes

- **Flow idempotency-key body drift is `invalid_update` with `details.reason =
  "idempotency_key_body_drift"`, no longer a 409 `Conflict`.** Reusing an idempotency key with a
  different request body (object/record create, embeds, grants, lifecycle and governance
  commands, package export/import, delivery replay, maintenance operations) now answers envelope
  `code` 400 with that reason, so it can be told apart from `stale_frontier` (still 409) without
  reading `message`. Internal failures that Flow code reports as such answer `server_rejected`
  (500, non-retryable) instead of the untyped 500. A database failure that reaches the response
  of a Flow route (`/flow/`, `/collab/`, `features/flow`, admin Flow routes) is typed as well:
  `server_rejected` with `details.reason = "deterministic_database_refusal"` when the database
  refused the statement for good (constraint, data or schema error, or an exception raised by a
  database function, `SQLSTATE` class `P0`), otherwise `server_draining` with
  `reason = "contention"` and `retry_after_ms` (connection, pool and transient errors). This
  holds for the credential lookup the authentication layer runs for a bot token too. A bot
  operation that ran but whose attribution record could not be written answers `server_rejected`
  with no `details` on a Flow route, never `server_draining`, so a client is not told to repeat
  an operation that has been carried out. The driver's text is logged, never sent. What
  still answers a legacy envelope on a Flow route is an authentication refusal that is not a
  database failure (a missing, invalid, disabled or expired credential, or a transport or
  permission the credential does not have: `code` 401 or 403, no `error_code`). Routes outside Flow keep the legacy envelopes (`code` 500, `message`
  "database error" or "internal server error", no `error_code`) unchanged. An identical request that is still
  running answers `server_draining` with `reason = "contention"` and `retry_after_ms`.

- **Every existing bot token only works with the REST API after the upgrade.** Migration
  `0062_flow_forms_bridge.sql` adds a transport surface to each bot token,
  `ALTER TABLE workspace_bots ADD COLUMN IF NOT EXISTS transport_surface TEXT NOT NULL DEFAULT 'rest';`,
  so every token that exists when upgrading from 0.2.21 is bound to `rest`. The API accepts a
  bot token only on the surface it is bound to, and the 0.2.21 MCP server and every later client
  declare their surface (`X-Sylvode-MCP-Surface` / `X-OpenPR-MCP-Surface`), so **after the
  upgrade every MCP integration (`http`, `sse`, `stdio`) and every command-line integration using
  an existing token is refused with 401** (`bot credential is not valid for the presented
  transport`). Plain REST calls with those tokens keep working. For each integration, create a
  new bot token bound to the matching surface — `mcp_http`, `mcp_sse`, `mcp_stdio`, `cli`, or
  `cli_tools_call` for `tools call` (see **`tools call` declares its own transport surface**) —
  on the Members page or with `POST /api/v1/workspaces/{id}/bots` and `"transport_surface"`, then
  put the new token in that client's configuration (`mcp.bot_token`, the agent's bearer token or
  `--bot-token`). Existing tokens are not rebound automatically.
- **Outbound webhook `User-Agent` changed** from `OpenPR-Webhook/1.0` to
  `Sylvode-Webhook/1.0 (compatible; OpenPR-Webhook/1.0)`. A `User-Agent` cannot be sent twice,
  so receivers that match the old value **exactly** or as a **prefix** no longer match and must
  be updated. Receivers that match `OpenPR-Webhook/1.0` as a substring are unaffected. The
  `request_headers` recorded with each delivery carry the same value that was sent. The payload
  and its `X-Webhook-Signature` are unchanged. Flow event deliveries (the event dispatcher) and
  AI-task deliveries (the worker), which sent no `User-Agent` of their own, now send the same
  value.
- **Plugin fuel accounting changed with wasmtime 49.** Wasmtime 49 charges fuel per byte or
  element for bulk memory and table operations (`memory.copy`, `memory.fill`, `memory.init`,
  `table.copy` and similar) and per page for `memory.grow`, which wasmtime 47 charged as single
  instructions. A plugin that moves large buffers or grows its memory
  with a tight `runtime.fuel` budget may now run out of fuel; raise the budget in its manifest
  (the maximum is 1,000,000,000). The wide-arithmetic proposal, which wasmtime 49 enables by
  default, is explicitly disabled, so the set of accepted modules is unchanged.
- **Plugin deadlines now stop the guest.** A plugin invocation that exceeds `runtime.timeout_ms`
  is interrupted at its deadline; previously the caller received the timeout error while the
  guest kept running on a blocking thread until its fuel ran out. Cancelling the request or
  shutting the API down now also stops the guest. Such invocations are recorded with status
  `timeout` instead of `failed`. Every failed invocation, whether timed out, out of fuel or
  trapped, now records its real `duration_ms` and `fuel_consumed` (previously `0` and `NULL`)
  in `plugin_invocations`, in the API response and in the `plugin.invoked` event. Guest trap
  messages include the full error chain, so fuel exhaustion is named. A plugin run whose request
  is cancelled (the client disconnects, or the form write that triggered a hook is dropped) is no
  longer stopped with the request: it runs to its own deadline and is recorded like any other
  run, where before nothing was recorded at all. A formula hook whose output the host refuses as
  a patch is now recorded as `completed` (the plugin ran) before the write is refused.
- **Plugin modules are limited to 4 MiB, and the deadline covers compilation.** Installing a
  plugin whose decoded module exceeds 4,194,304 bytes is refused with 400 before the module is
  decoded or compiled, and an installed module over the limit fails at invocation (`failed`).
  `runtime.timeout_ms` now bounds the whole invocation from the moment the request starts:
  waiting for a blocking thread, compiling and instantiating the module, and running the guest.
  Previously compilation ran outside the deadline, so a large module held the request for as
  long as compiling took, on every invocation. When the deadline passes before the guest has
  started, the invocation returns `timeout` at once with no fuel recorded, and the guest never
  runs. Each module is compiled on the invocation's own thread instead of the shared compilation
  pool, so one large module no longer delays other plugins, and install-time validation runs off
  the request threads.
- **Conflicting MCP attribution headers are rejected.** The API accepts
  `X-Sylvode-MCP-Surface` / `X-Sylvode-MCP-Tool` and the legacy `X-OpenPR-MCP-*` headers. If the
  canonical and legacy header of a field, or repeated occurrences of either, carry different
  values, the request is refused with envelope code 401 instead of one value being picked,
  because these values are recorded as audit attribution. A client that sends only one set, or
  both with equal values, is unaffected.
- **Two configuration files are an error.** The default configuration file is now
  `config/sylvode.toml`. `config/openpr.toml` is still discovered when the new file is absent,
  but if both exist the binaries refuse to start until `--config` selects one. `scripts/start.sh`
  applies the same rule to `config/sylvode.compose*.toml` and `config/openpr.compose*.toml`, and
  to a canonical and a legacy compose variable that are both set with different values.
- **Auth cookies always carry `Secure`.** Browsers do not send `Secure` cookies over plain HTTP
  except to `localhost`, so a deployment reached over plain HTTP at any other address needs
  HTTPS. `auth.allow_insecure_cookies = true` drops the attribute for local development and is
  refused at startup unless `server.bind_addr` is a loopback address.
- **Standalone MCP server archives are no longer published.** The `openpr-mcp-server-<target>`
  release archives are gone. The `mcp-server` binary ships in every `sylvode-<target>` archive
  (and in its `openpr-<target>` alias) together with `api`, `worker` and `sylvode`.
- **CLI exit codes on output errors.** `mcp-server` and `sylvode` no longer panic (exit 101) when
  stdout or stderr cannot be written: a failed command still exits 1, a successful command whose
  result cannot be written exits 1 with a line on stderr, and the Flow commands keep their own
  exit code. `list-tools` exits 1 when its listing cannot be written for any reason other than a
  closed pipe.
- **Messages name the Sylvode API.** MCP client refusals (rejected or missing credential) and
  structured Flow tool errors say "Sylvode API" instead of "OpenPR API". Help, log and error texts
  are not a compatibility contract; match on codes, not on message text.
- **Deprecation warnings on stderr and in logs.** Legacy entry points now announce themselves
  (see **Deprecated**). The warnings never change stdout or the exit code, but a wrapper that
  treats any stderr output as failure will notice them.
- **Flow commands report an unreachable API as `network_error`.** When the API gives no
  response at all (connection refused, DNS failure, TLS failure, timeout, a dropped connection, or
  a reply that is not the API's envelope, such as a proxy error page), the `sylvode` Flow commands
  (`features`, `objects`, `collections`, `records`, `collab`, `deliveries`) now report the CLI-local
  code `network_error` with `recoverable: true` and `details: {"reason": "unreachable"}`, as the
  error-mapping contract specifies. They previously reported `server_draining` with empty
  `details`, which broke that code's contract (its `details.reason` is required and is only `drain`
  or `contention`) and read like a server drain. The message now names the API URL and the cause
  and says the command can be retried, and table format prints
  `Error [network_error]: network failure: ...`. **Scripts that detect a network failure with
  `error.code == "server_draining"` must change to `network_error`; scripts that use exit code 9
  are unaffected.** A local problem that stops the request before anything is sent (no bot token
  configured, a package file that cannot be opened) is now `usage_error` with exit 2 instead of
  exit 9. The nine workspace groups keep the `mcp-server` behaviour (`Request failed: ...`,
  exit 1).
- **`tools call` declares its own transport surface.** `sylvode tools call` and
  `mcp-server tools call` now present the surface `cli_tools_call` to the API, as the MCP surface
  contract specifies; every other workspace and Flow command still presents `cli`. The API accepts
  a bot token only on the surface it was issued for, so a token issued for `cli` that was used
  with `tools call` is now refused with 401, and a `cli_tools_call` token does not run the native
  commands. Issue a separate `cli_tools_call` token (Members page, "CLI tools call") for
  `tools call`. Operation logs and Flow event origins record `cli_tools_call` for these calls.
- **A bot operation whose audit row cannot be written no longer reports success.** Every request
  made with a bot token writes a `bot_operation_logs` row, the record that attributes the call to
  its bot, surface and tool. The row was written in the background and a failed write only logged
  a warning, so the operation reported success with no record of who made it. The API now writes
  the row before it answers: when the write fails, a successful operation is answered with the
  internal-error response (`code` 500, `internal server error`) and the API logs the failure at
  `ERROR` with the database's reason; an operation that already failed keeps its own error. The
  operation itself has run by then, so retry it with the same idempotency key where the command
  takes one. Each bot request now also waits for that one insert before it is answered.
- **`work-items create` starts in the project workflow's initial state.** Under `mcp-server` and
  `sylvode` the command always sent `state=backlog`, so a project whose workflow has no `backlog`
  state refused the create (`state must be one of: ...`) and a project whose workflow starts at
  another state got its items in `backlog`. The command now sends a state only when `--state` is
  given, as the `work_items.create` tool does. A script that relied on new items landing in
  `backlog` in a workflow that starts elsewhere must pass `--state backlog`.
- **The webhook receiver example config is `config/sylvode-webhook.example.toml`.** It was
  `config/openpr-webhook.example.toml`. The compose `webhook` service and `scripts/start.sh` default
  to the new path; a `SYLVODE_WEBHOOK_CONFIG` (or legacy `OPENPR_WEBHOOK_CONFIG`) that names the old
  example file must be updated. Its secret placeholder is now `replace_with_webhook_secret`.
- **Demo account defaults.** `scripts/bootstrap-restaurant-demo.sh`, the deployed-environment smokes
  and the Playwright specs default to `demo@sylvode.local` / `SylvodeDemo123!` instead of
  `demo@openpr.local` / `OpenPRDemo123!`. A demo database bootstrapped earlier keeps its old
  account; set `SYLVODE_DEMO_EMAIL` and `SYLVODE_DEMO_PASSWORD` (or `TEST_EMAIL` /
  `TEST_PASSWORD` for the specs) to reuse it.

### Added

- MCP `objects.create` accepts an optional `after_block_id` and `sylvode objects create` an
  optional `--after-block`: with `embed_page_id` / `--embed-page` the new Collection's embed
  block is inserted right after that block of the Page, as the REST `create_collection_embed`
  command already allowed; without it the embed is appended as before. Either is refused locally
  without an embed Page, and a block id the Page does not have is the server's `invalid_update`.
- Flow Share panel: the add picker offers the workspace's active bots next to its members, so a
  grant can name a bot (`principal_kind = "bot"`). Listing bots needs a workspace owner or admin;
  other managers still see the members.
- Flow Collections on the Web: the record panel can archive a record (`record_archive`) after an
  in-place confirmation. Records are read with the active view's `view_id` (an unsaved draft
  query is still sent explicitly), an `invalid_field_value` refusal is shown on the cell its
  `details.field_id` names, a record whose content the server withholds under field secrecy shows
  that its body cannot be read instead of an empty, editable body, and the relations panel words a
  `cross_workspace_relation` refusal as such.
- Flow conversion jobs: a job the stale-`started` sweep failed carries
  `error_reason: "stale_started"` next to `error: "server_rejected"` in every job response
  (`GET /flow/conversions/{job_id}`, commit and retry), so a client can offer a retry for an
  interrupted run instead of treating it as a permanent refusal. Other failures, retry claims
  and completions have no `error_reason`. Migration `0071_flow_conversion_job_error_reason.sql`
  adds the column.
- **Sylvode Flow**: collaborative pages, objects, Collections and records inside a workspace.
  Flow is off by default and is enabled per workspace through
  `PUT /api/v1/workspaces/{id}/features/flow`, the MCP tool `flow.feature_set` or
  `sylvode features flow set`.
  - Objects with history, diff, schema, relations, grants and inheritance, cross-project moves
    of whole subtrees, a workspace and project navigator, permission-filtered search and
    projection lag reporting.
  - Collections and Collection records with typed queries and field-level secrecy.
  - Live collaborative editing over WebSocket on the Loro CRDT engine (`crates/collab-core`):
    sessions are opened with short-lived tickets bound to an allowed origin, resume from the
    last acknowledged sequence, open read-only for readers and close when access is revoked.
  - Conversion between Flow objects and universal-form records (preview, commit, status, retry).
  - Export and import packages for one object or a whole workspace; an import is previewed
    first and then committed as a whole.
  - Import of legacy pages through the `legacy_pages.*` MCP tools.
  - Administrative maintenance: document compaction and verification, projection rebuilds,
    quarantine repair, delivery replay, and workspace health, integrity and lag reports.
  - Web UI: a Flow navigator and an object canvas under `/workspace/{workspaceId}/flow`.
  - Web UI: a workspace Flow settings page under `/workspace/{workspaceId}/settings/flow` for
    workspace owners and admins, which shows the current flag, default member level and
    authorization epoch, and turns Flow on or off or changes the default member level after an
    explicit confirmation; the sidebar shows the Flow entry only while Flow is enabled.
  - Web UI: a workspace package export and import page under
    `/workspace/{workspaceId}/settings/flow/package` for workspace owners and admins. Export
    polls the job and downloads the `.sylvode-flow.zip`, hashing the downloaded bytes locally
    (SHA-256) and refusing the file unless the hash equals the job checksum; import is a
    five-step wizard (choose file, upload with progress and cancel, mapping and policies, server
    preview, exact-hash confirmation) that keeps checksum, format and policy errors on screen and
    out of confirm, and opens a report page that resumes polling after a reload.
  - Web UI: a workspace Flow operations panel under
    `/workspace/{workspaceId}/settings/flow/operations` for workspace owners and admins, usable
    while Flow is disabled. It shows health (auto-refreshed every 15 seconds, pausable, stale
    sample kept on a failed refresh, dead-letter counted apart from cancelled deliveries), lag
    with a projection-lag badge and per-object rows, and integrity per document; runs verify as a
    dry run; runs compact and rebuild-projection as a dry-run receipt followed by an execute that
    requires the exact target id typed by the user; and replays deliveries after a dry-run
    summary and an explicit acknowledgement, refusing windows older than 30 days locally.
  - Web UI: a conversion wizard under `/workspace/{workspaceId}/flow/{objectId}/convert`,
    opened from the object panel's "Convert to Forms…" action on a Page, that turns the Page into
    a Universal Forms record in four steps that cannot be skipped: source (frontier, document
    sequence and what the conversion does to ownership), target project, form and constant field
    values, server preview (schema version, object count, permission decision, countdown to
    expiry), and an acknowledged commit that sends the preview's own identifiers. A stale source
    is re-read and previewed again; an expired preview cannot be committed.
  - Web UI: a conversion job page under `/workspace/{workspaceId}/flow/conversions/{jobId}`,
    where the wizard lands after a commit. It polls the job until it is completed or failed and
    shows the status, the source Page (linked when the viewer can open it), source frontier,
    target schema version, lineage, warnings and error, and links each created record to its
    Forms record page (an id that is not a readable record is shown with a copy button). A failed
    job can be retried after an explicit confirmation, reusing one idempotency key per job; a
    permanent rejection withdraws the retry. Missing and inaccessible jobs look the same.
  - Web UI: a Share section in the object panel. It reads the object's grants and shows the
    caller's own level and the inheritance flag; with full access it also lists the grants
    inherited from parents apart from the grants on the object, and adds, changes and removes
    grants and turns inheritance off or on. Every change that can lower anyone's access
    (removing or lowering a grant, removing everyone, stopping inheritance) is first sent as a
    dry run and its `permission_changes` are shown; the real request is only sent on confirm,
    under the dry run's idempotency key. Losing one's own full access needs an explicit
    acknowledgement before `confirm_self_lockout` is sent. After a change the panel shows the
    server's response and reads the grants again. Without full access no grant list is shown and
    nothing can be changed.
  - Web UI: a Relations section in the object panel, loaded the first time it is opened and
    paged with the server's opaque cursor, with direction and relation-type filters. Each
    visible relation links to its target object; a relation whose target the viewer cannot see
    is shown as a placeholder with no link and no copy action. A relation that is not provably
    in the object's workspace, a malformed row, or a rejected read hides every relation and
    shows an integrity error instead.
  - Web UI: history entries in the object panel expand into their semantic diff (title change,
    added, removed, changed and moved nodes with their text and property changes; no node ids,
    peer ids or bytes), with errors worded by `error_code`. The Info section exports the object
    as JSON, Markdown, CSV (Collections only) or a package (optionally with history), polls the
    job and downloads the file after checking its locally computed SHA-256 against the job
    checksum.
  - Web UI: the object panel is split into Info, Share, Relations and History tabs
    (`role="tablist"`, arrow keys, Home and End); Share and Relations load when their tab is
    first opened.
  - Web UI adapters for the navigator: `flowApi.searchObjects` (`GET .../flow/search`, cancellable
    with an `AbortSignal`) behind `ObjectRepository.search`, a search controller (200ms debounce,
    the previous request aborted and its late answer dropped, stale hits marked, clamped keyboard
    index, `require_current` failures not retried as `allow_stale`), a projection-lag badge
    controller refreshed every 30 seconds, and a standalone Collection create session that keeps
    one idempotency key and one set of field and view ids per intent. `CommandService.createObject`
    accepts the caller's idempotency key and throws the typed `error_code`.
  - Web UI: the navigator searches Flow objects as you type (one request 200ms after the last
    keystroke, the previous one cancelled). Results are a keyboard listbox (`aria-activedescendant`,
    arrow keys, Home, End, Enter opens the object page, which reads the current object); a hit
    whose index lags the document is marked Stale, and a lagging index frontier is shown. An
    optional "up-to-date index only" switch sends `freshness=require_current` and shows the
    `stale_frontier` refusal instead of falling back. The navigator header shows the workspace
    projection lag (`max_lag`, `p95_lag`, refreshed every 30 seconds, `aria-live`), a New menu
    with New Page and New Collection, and 44px header buttons instead of the emoji glyphs. New
    Collection asks for a name, optional text fields (one default field) and a table or board
    view and creates it with one `POST .../flow/objects`; Collections are listed in the tree.
  - Web UI adapters for Collections: `flowApi.getObjectSchema`, `getCollection`,
    `listCollectionRecords` (filter, sort, group, cursor, field ids), `queryCollectionRecords` and
    `createCollectionRecord`, behind `ObjectRepository.getSchema` / `getCollection` /
    `listRecords` / `queryRecords` and `CommandService.createRecord`. `ObjectRepository.openRecord`
    / `closeRecord` reference-count the one read of a record's document; `open` accepts an object
    the caller already read. `CommandService.execute` now throws the envelope's typed
    `error_code` (with `details`) and falls back to the HTTP-status mapping only for a legacy
    envelope.
  - Web UI Collection view controller (`lib/flow/collection-view.ts`) and typed value codec
    (`lib/flow/collection-values.ts`): schema parsing (live fields and views by position, unknown
    types read-only, `config.restricted`, select options in `config.options`), editor/wire
    conversion for the seven field types (a number is sent as a JSON number, a date only as a
    real `YYYY-MM-DD`), placeholders for restricted, missing and malformed values that never
    fall back to another representation, one `record_patch` per edited cell with the record's
    frontier as `expected_frontier` (a `stale_frontier` keeps the draft and re-reads the
    records), drafts kept per record across view switches, a record document opened only for
    the record panel, record create with one key per intent, and every `field_*` / `view_*`
    command refused without a request unless schema mode is on.
  - Web UI: Collections open on the object page through the renderer registry
    (`collection:table`, `record:panel`, loaded on demand) instead of the unsupported fallback,
    without a collaboration ticket or bootstrap. The view switcher lists the Collection's saved
    views. The table renders only the visible rows, has resizable columns, is a `role="grid"`
    (arrow keys, Home, End, Enter edits, Escape cancels) with typed editors per field type
    (relations are read-only links), adds records from its footer, and turns into a card list
    below 768px. A record opens in a side panel with its properties and its body; unsaved edits
    survive switching views. **Design schema** (`?mode=schema`) is the only place to create,
    rename, retype, configure (select options), archive and reorder fields and to create, rename
    and reorder views; a refused change is shown by its `error_code`. Strings are under
    `flow.collection.*` and `flow.record.*`, which the i18n parity gate now requires.
  - Web UI: the Collection board (`collection:board`) groups records by a select field, one
    column per option plus "No value" (`role="list"`); dragging a card to another column, or its
    44px "move to previous / next column" buttons, writes that field with one `record_patch`, and
    each column adds a record with its option preset. Without a select field the board explains
    how to add one. A filter, sort and board grouping bar narrows the records with an unsaved
    draft query (sent as query parameters, never written to the document); in schema mode
    **Save view** stores the draft and the column widths in the view with `view_update`.
  - Web UI: the Page slash menu adds **Collection**, **Reference a Form**, **Embed a Form** and
    **Convert to Forms…**, each with one line on who owns the result. Collection asks for a title
    (default "Untitled collection") and sends one `create_collection_embed` command (never an
    object create followed by a block insert); while it runs a single provisional block is shown
    that is never written to the Page document, a failure removes it and offers Retry with the
    same idempotency key and body, and a permanent failure offers no retry. The new Collection is
    rendered in the Page as an embedded `collection:table` with an Open link, and the navigator
    lists it under its Page. Reference and Embed pick a project, a form and optionally a record
    and call `POST .../references` with `display.mode` `reference` or `embed`; the Page keeps only
    a placeholder block pointing at the `reference_id`. Reference cards show the title, type, a
    permission badge and a link to Forms; Embed cards also show the target's content when the
    request-time permission allows viewing and otherwise a placeholder without requesting any
    Forms data; a reference the bridge no longer returns is shown as unavailable. Removing a
    reference asks for confirmation and only calls `DELETE .../references/{id}`. Convert opens
    the conversion wizard. Strings are under `flow.canvas.slashMenu.*`, `flow.canvas.embed.*`
    and `flow.bridge.card.*`.
  - Web UI: collaboration on a Page. Other people's carets and selections are shown with their
    name and colour and a marker on the block they are in (`presence` frames, at most four per
    second, payload checked against the 8 KiB ceiling, expired locally after the 30 second TTL;
    presence is never written to the document and never locks a block). A peer's committed edit is
    applied in place from its `update`/`accepted` pair instead of reconnecting. Edits made while
    offline go to an IndexedDB outbox (update bytes, base frontier, idempotency key, intent); the
    sync indicator shows **Offline** with the pending count. On reconnect the Page fetches the
    accepted bootstrap, merges it, re-checks the user's permission and replays the queued edits in
    order under their original keys; an edit the accepted snapshot already contains is not sent
    again. An edit the server refuses (`policy_rejected`, `invalid_update`, `server_rejected`, a
    size limit) and every later edit that depends on it go to a **recovery draft** (download as
    JSON or copy as text, each entry with its reason), and the Page is rebuilt from the accepted
    state. A user whose level is `view` or `comment` gets a read-only canvas (no typing, slash menu
    or block handle); a revocation on a live connection (close 4403) or a refused write whose
    re-check finds no write access switches to read-only and keeps pending edits in the recovery
    draft. Three consecutive `server_rejected` answers close the connection with 4500 and stop
    reconnecting. `invalid_update` reasons are acted on: `subtree_spans_multiple_projects` points
    to the workspace operations page, `child_project_must_match_parent` asks for the parent's
    project, and `idempotency_key_body_drift` is resent once under a new key. The recovery draft
    download is a JSON file (`sylvode.flow.recovery-draft.v1`); it used to be the raw engine
    snapshot. Strings are under `flow.presence.*`, `flow.sync.pending`, `flow.recovery.*` and
    `flow.error.invalid_update_reason.*`.
  - Migrations `0054` to `0069`.
- **`sylvode` CLI**, a new binary in every release archive with fifteen command groups: the Flow
  commands `features`, `objects`, `collections`, `records`, `collab` and `deliveries`, which
  follow the `sylvode.cli.v1` JSON contract, and the workspace commands `projects`, `work-items`,
  `comments`, `labels`, `sprints`, `search`, `files`, `operation-logs` and `tools`, which are the
  same commands `mcp-server` runs, with byte-identical stdout and the same exit codes.
- **MCP**: 42 new tools for Flow (140 in total; the names and schemas of the existing tools are
  unchanged), `sylvode://` URIs for every resource, and Flow resource templates for objects,
  object history and schema, Collection records and the workspace navigator.
- **Protocol headers**: the MCP client sends `X-Sylvode-MCP-Surface` and `X-Sylvode-MCP-Tool`
  alongside the legacy headers, and attachment package downloads return
  `X-Sylvode-Attachment-Count` and `X-Sylvode-Attachment-File-Count` alongside the legacy
  headers, with identical values.
- **Configuration**: a `[flow]` section with `dispatch_max_attempts` (default `10`) and
  `collab_allowed_origins` (default empty, which refuses every collaboration ticket; list the
  web UI's origin, for example `"https://sylvode.example.com"`, to enable live editing), and
  `auth.allow_insecure_cookies` (see above). `config/sylvode.example.toml` is the annotated
  reference.
- **Release archives** named `sylvode-<target>` and `sylvode-frontend.tar.gz`, with
  `openpr-<target>` aliases that contain the same binaries.
- **Brand residue gate** `scripts/verify-sylvode-brand-residue.sh`: every `OpenPR` / `openpr` in
  the tracked files of this repository and of the webhook, documentation, website and organisation
  repositories must be covered by a reasoned entry of `scripts/contracts/sylvode-brand-allowlist.json`.
  CI runs it for this repository; the five-repository form is a local release gate.
- **Project policies in the repository**: `SECURITY.md` (how to report a vulnerability and what is
  in scope), this `CHANGELOG.md`, and a rewritten `CONTRIBUTING.md`.
- **Workspace home page** at `/workspace/{workspaceId}`, which used to render a 404. It shows the
  workspace name, slug and the viewer's role, the project total with the five most recently
  updated projects, a Sylvode Flow card (a link into Flow when it is enabled, a link to the Flow
  settings for admins when it is not, nothing for other members) and, for owners and admins,
  links to Members, Webhooks, Operation Records and Settings. Clicking a workspace on
  `/workspace` now opens this page instead of the project list. The page title is
  `Sylvode - <workspace name>`, and the Operation Records, Flow, Workflows and Forms record pages
  now carry the same `Sylvode - ` title prefix as every other page.
- **Browser E2E and image contents in CI.** `scripts/e2e-web-stack.sh up|down` starts the stack
  the browser specs run against (release API on a scratch database, a generated configuration
  with a random `jwt_secret` and `[flow] collab_allowed_origins` set to the UI origin, the built
  frontend behind a same-origin proxy that also forwards WebSockets, and an instance admin); `up`
  refuses to start when `collab-isolated-apply-worker` is not next to the API binary. The new
  **Web E2E** CI job uses it to run the `flow-settings`, `flow-package-roundtrip`,
  `flow-operations`, `flow-convert` and `workspace-home` specs with `E2E_REQUIRE_DATABASE=1` and
  fails on any skipped test. The new **Image contents** job builds the `api` and `worker` images
  from `Dockerfile.prebuilt` with `scripts/verify-prebuilt-images.sh` and checks that only the
  `api` image ships `collab-isolated-apply-worker` and that `/app/api --build-info` runs in it.
- **Form record detail shows its Flow lineage.** `GET /api/v1/form-records/{record_id}` (and the
  MCP tool `form_records.get`, which passes the response through) carries a read-only
  `flow_lineage` object: `references` lists the live Flow references that point at the record and
  `conversions` lists the conversion jobs that created it. Each entry is filtered by the caller's
  Flow read access to its source object; an unreadable source is reported only as
  `reference_id`/`job_id` with `source_available: false`. A readable conversion entry carries
  `job_available`, true when the caller may open the job (its actor or a workspace
  owner/admin). Both lists are empty when Flow or the
  Forms bridge is disabled for the workspace.
  The web record detail page shows this as a read-only **Flow** section: each reference with its
  source Page (linked), display mode and time, each conversion with its source Page, job status
  and a link to the job page when `job_available` is true. An unreadable source shows only a
  "no access to the source Page" placeholder with no link. The section is hidden when both
  lists are empty or the API does not send the field; references are still removed from the
  Page card in Flow, never from the record.

### Changed

- The product is now called **Sylvode**. The web UI, documentation, MCP server identity, help
  texts and release names use the new name. The canonical names are `config/sylvode.toml`,
  `config/sylvode.compose.toml`, `config/sylvode.compose.mcp.toml`, the `SYLVODE_*` compose
  variables, `sylvode://` resource URIs and `sylvode-<target>` archives. The GitHub repository
  moves to `openprx/sylvode`. These identifiers are deliberately **not** renamed and have no
  removal planned: REST `/api/v1` and its schema identifiers, MCP tool names, database names,
  roles, tables, columns and migrations, the plugin ABI (`openpr_alloc`, `openpr_invoke`,
  `openpr_plugin_abi_version`, `openpr-plugin-v1.wit`), the bot token prefix `opr_`, the compose
  service labels and network names, and `OPENPR_TEST_DATABASE_URL`.
- `--config` help shows `[default: config/sylvode.toml; legacy config/openpr.toml fallback]`
  without Markdown escapes.
- When `logging.output = "stdout"` is overridden because stdout carries the command's result or
  the MCP stdio protocol, `mcp-server` and `sylvode` log the notice under their own target, so
  the default filter shows it.
- The optional `webhook` compose service now defaults to the canonical Sylvode Webhook names:
  image `ghcr.io/openprx/sylvode-webhook:latest`, executable `/app/sylvode-webhook`, and the
  configuration mounted at `/etc/sylvode-webhook/config.toml`. `SYLVODE_WEBHOOK_IMAGE` and
  `SYLVODE_WEBHOOK_CONFIG` still override them, and `scripts/start.sh` still maps
  `OPENPR_WEBHOOK_IMAGE` / `OPENPR_WEBHOOK_CONFIG` onto them. The canonical executable ships in
  Sylvode Webhook images after 0.3.3; an image pinned to 0.3.3 or earlier needs its own
  `command: ["/app/openpr-webhook", "/etc/sylvode-webhook/config.toml"]`. The
  `ghcr.io/openprx/sylvode-webhook` image name exists only from the first Sylvode Webhook release
  after 0.3.3 onward (every earlier image is published only as `ghcr.io/openprx/openpr-webhook`),
  so until that release is published, starting the `connectors` profile with the defaults fails
  to pull; set `SYLVODE_WEBHOOK_IMAGE=ghcr.io/openprx/openpr-webhook:<tag>` together with the
  `command` above.
- The repository root carries exactly `LICENSE-MIT` and `LICENSE-APACHE`. `LICENSE-APACHE` is now
  the complete standard Apache License 2.0 text instead of a shortened rendering, the duplicate
  `LICENSE` file is gone, and `LICENSE-MIT` names OpenPRX Contributors as the copyright holder.
  `LICENSE-APACHE` is byte-identical to the text published at apache.org, and `LICENSE-MIT` gives
  2026, the year of the first commit.
  The licence itself (`MIT OR Apache-2.0`) is unchanged.
- `scripts/bootstrap-restaurant-demo.sh` reads its inputs as `SYLVODE_API_URL`,
  `SYLVODE_DEMO_*` and `SYLVODE_MCP_BOT_TOKEN`. The `OPENPR_*` names still work with one
  deprecation notice each; setting both names of one input to different values is an error. The
  default demo user name is now `Sylvode Demo`.
- The Universal Forms delivery reports, dashboards and their checkers say Sylvode instead of
  OpenPR in their headings.

### Deprecated

Each legacy entry point below keeps working, with the same behaviour as its replacement,
throughout the 1.x series. **None of them is removed before Sylvode v2.0**, and a removal
additionally requires its own accepted decision record. Every warning names the legacy name, its
replacement and the earliest removal.

| Legacy | Replacement | Where the warning appears |
| --- | --- | --- |
| `mcp-server <command>` for any command except `serve` | `sylvode <command>` | One line on stderr per process. `mcp-server serve` does not warn on any transport. |
| Default discovery of `config/openpr.toml` | `config/sylvode.toml` | One `warn` log line per process from `api`, `worker`, `mcp-server` and `sylvode` (on stderr for `mcp-server` and `sylvode`). An explicit `--config config/openpr.toml` does not warn. |
| `config/openpr.compose.toml` and `config/openpr.compose.mcp.toml` | `config/sylvode.compose.toml` and `config/sylvode.compose.mcp.toml` | One line on stderr per file from `scripts/start.sh` when it selects the legacy file. |
| Compose variables `OPENPR_BIND_HOST`, `OPENPR_API_PORT`, `OPENPR_FRONTEND_PORT`, `MCP_SERVER_PORT`, `OPENPR_RUNTIME_BASE`, `OPENPR_FRONTEND_DOCKERFILE` and `OPENPR_WEBHOOK_*` | `SYLVODE_BIND_HOST`, `SYLVODE_API_PORT`, `SYLVODE_FRONTEND_PORT`, `SYLVODE_MCP_PORT`, `SYLVODE_RUNTIME_BASE`, `SYLVODE_FRONTEND_DOCKERFILE` and `SYLVODE_WEBHOOK_*` | One line on stderr per variable from `scripts/start.sh`. Only `scripts/start.sh` reads the legacy names: `docker-compose.yml` interpolates `SYLVODE_*` only, so running `docker compose` directly with an old `.env` silently uses the defaults. |
| `openpr://` MCP resource URIs | The corresponding `sylvode://` URI | `_meta.deprecation = {"replaced_by": "<sylvode:// URI>", "earliest_removal": "2.0"}` in the `resources/read` result, next to `_meta.canonical_uri`. The resource body is unchanged and list endpoints return only `sylvode://` URIs. |
| `openpr-<target>` release archive aliases | `sylvode-<target>` | Release notes only. |
| `X-OpenPR-MCP-Surface` and `X-OpenPR-MCP-Tool` request headers | `X-Sylvode-MCP-Surface` and `X-Sylvode-MCP-Tool` | None (protocol headers). The MCP client sends both sets so that a new `mcp-server` attributes correctly against an older API. |
| `X-OpenPR-Attachment-Count` and `X-OpenPR-Attachment-File-Count` response headers | `X-Sylvode-Attachment-Count` and `X-Sylvode-Attachment-File-Count` | None (protocol headers). Both sets are always returned. |
| The `OpenPR-Webhook/1.0` token in the webhook `User-Agent` comment section | `Sylvode-Webhook/1.0` | None. |

### Security

- **Wasmtime upgraded from 47.0.4 to 49.0.2** for RUSTSEC-2026-0315, RUSTSEC-2026-0316,
  RUSTSEC-2026-0325, RUSTSEC-2026-0326 and RUSTSEC-2026-0327. Exposure of earlier builds:
  - Every release from 0.2.2 through 0.2.21, and source builds from commit `3159995` (the
    upgrade to wasmtime 47) up to 0.2.334, were exposed to **RUSTSEC-2026-0315** (`call_ref` and
    exception `catch` can drop fuel accounting, so a plugin can consume far more CPU than its
    fuel budget) and **RUSTSEC-2026-0326** (missing GC rooting across `try_call`, which can
    corrupt the GC heap). Both are reachable by anyone allowed to install a plugin, which is any
    workspace member and any bot token with write permission.
  - RUSTSEC-2026-0316 and RUSTSEC-2026-0327 affect only the WebAssembly component model, which
    Sylvode does not use: it loads core modules only.
  - RUSTSEC-2026-0325 requires a module that imports a tag; the plugin runtime instantiates
    modules with no imports, so such a module is refused.
  - Releases before 0.2.2 did not contain the plugin runtime.
- **rustls upgraded from 0.23.40 to 0.23.45** for RUSTSEC-2026-0285 (TLS 1.3 handshake messages
  accepted across encryption level boundaries). Every release up to and including 0.2.21, and
  builds up to 0.2.324, used an affected version for their TLS client connections (outbound HTTP, and PostgreSQL when TLS is used).
- **AI task references are checked against the project.** `POST /api/v1/projects/{id}/ai/tasks`
  accepted any `reference_id`, which is handed to the project's AI participant and recorded as the
  causation of the task's events, so a project admin could point the agent at an entity of another
  project or workspace. The reference must now be a work item of the project, a comment on one, or
  a proposal of the project's workspace; anything else, and a `reference_id` without a
  `reference_type`, is refused with 400. Present in 0.2.21.
- **Six cross-tenant reads and writes closed.** Draft, open, voting and rejected proposals were
  visible to any authenticated user of the instance, the project context's recent decisions had
  no tenant filter, and issue links, bot label batches, sprint assignment and bot mentions did not
  check the tenant of the id being attached. Empty scopes and missing tenant columns now fail
  closed, and a resource of another tenant is indistinguishable from an absent one. These were
  present in 0.2.21.
- Request tracing spans no longer contain query strings, and the reference Caddy and nginx
  configurations no longer log collaboration tickets or signed download signatures.
- Auth cookies carry `Secure` (see **Breaking and behaviour changes**).
- Details of real deployments are removed from the tree: the legacy-pages inventory spikes
  (`spikes/legacy-pages-inventory/`) named a deployment host's ssh alias, public domain, admin
  e-mail and install path, now placeholders, and `playwright.config.ts` and the eleven
  `scripts/smoke-universal-forms-*.mjs` defaulted to a maintainer's private VPN address; they
  now default to `http://localhost:3000` (and `http://localhost:8090/mcp/rpc`), still
  overridable with `BASE_URL` / `OPENPR_FRONTEND_URL` / `OPENPR_MCP_URL`.
  `scripts/test-no-instance-literals.sh` (run in CI) fails on private IPv4 addresses and bot
  token literals in any tracked text file, and `scripts/test-no-machine-paths.sh` now also
  scans `apps/`, `crates/` and `docs/schemas/`.
- `skills/openpr-mcp/scripts/mcp-regression.py` contained a bot token and the workspace and
  project ids of a test instance since 0.1.x. They are removed; the script reads them from
  `SYLVODE_MCP_REGRESSION_TOKEN`, `SYLVODE_MCP_REGRESSION_WORKSPACE_ID` and
  `SYLVODE_MCP_REGRESSION_PROJECT_ID` and exits when one is missing. The token remains in the
  public git history and must be treated as compromised: revoke it on the instance that issued
  it. Its output is now in English. The production readiness audit now checks the script's
  default API URL in its new form.
- **A plugin could allocate unbounded host memory through table growth.** The plugin store
  limited linear memory to `runtime.memory_bytes` but left the table element count at
  wasmtime's default, which is unlimited, so a guest could `table.grow` hundreds of millions of
  elements (about 8 bytes of host memory each) within one invocation's fuel budget. A plugin
  table now holds at most 65,536 elements, its initial size included: a larger initial table
  fails to instantiate and a larger `table.grow` returns `-1` to the guest. Present in every
  release since 0.2.2.

### Removed

- The `openpr-mcp-server-<target>` release archives (see **Breaking and behaviour changes**).

### Fixed

- **Four more Flow database tests no longer fail when the shared test database is slow to
  commit**: the move lock-order symmetry test, the mixed move / grant / subscription / content
  lock-rank test, the epoch fencing test and the snapshot soft-trigger test. Each resubmits the
  same request only for an answer that is retryable and proven unwritten, and still fails on
  everything else. A sequential move is resubmitted only for a raw lock timeout on `COMMIT`
  (a move calls `pg_notify`, so its commit queues on the server-wide notification lock behind
  commits in unrelated databases); typed `server_draining` / `contention`, a deadlock and every
  other error still fail it. A content write is resubmitted for `contention` only after all
  three bounded rebase attempts with the document head unmoved, and snapshot advancement only
  for a lock-wait or statement timeout that left `snapshot_seq` and the tail untouched.
- **The collaborative write path logs the `SQLSTATE` of a locked phase that failed before
  commit**, so the server log tells a lock-wait timeout (`55P03`) on the document row from a
  statement timeout (`57014`) when a write ends in `server_draining` / `contention`. The value
  stays in the log and is never put on the wire. The two-instance sequence allocation test no
  longer fails when the database holds the document row past the lock-wait budget under load:
  each instance now resubmits a `contention` answer under the same update id, as the protocol
  asks of a client, and the test requires that such an answer came only after all three bounded
  rebase attempts and that the committed sequences, stored updates and fan-out notices are still
  exactly 1 and 2.
- Flow REST routes no longer answer the legacy `database error` envelope: a database failure
  propagated to the response of a Flow route is `server_rejected`
  (`deterministic_database_refusal`) or `server_draining` (`contention`, `retry_after_ms`), so
  clients can classify it by `error_code`. One response boundary around the Flow router does
  this; routes outside Flow are byte for byte unchanged.
  - A bot token gets the same answers: a database failure of its credential lookup on a Flow
    route is typed by the same rule instead of the legacy `internal server error` envelope. When
    the operation ran and only its attribution record could not be written, the Flow route
    answers `server_rejected` with no `details` whatever the database failure was, and never the
    retryable `server_draining`: the operation has been carried out and must not be repeated. The only legacy envelopes left on a Flow route are
    authentication refusals that are not database failures (a missing, invalid, disabled or
    expired credential, or a transport or permission the credential does not have).
  - An exception raised by one of the application's database functions (`SQLSTATE` class `P0`)
    is answered as `server_rejected` (`deterministic_database_refusal`) instead of
    `server_draining`: `GET /api/v1/workspaces/{workspace_id}/flow/navigator` with a `project_id`
    that does not exist no longer tells the client to retry a request that is refused every
    time. Retry loops inside the server classify database errors as before.
  - The boundary covers the Flow routes only. A path no route matches is answered by the
    application's fallback outside it, as before the boundary existed.
- Flow conversion jobs: the stale-`started` sweep measures a run from a dedicated `started_at`
  column instead of `updated_at`, so a write to a `started` job no longer postpones its reclaim.
  `started_at` is set when the job is created and reset when a retry claims a `failed` job;
  nothing else moves it. Migration `0072_flow_conversion_job_started_at.sql` adds and backfills
  the column. Job responses are unchanged.
- Flow logout waits at most three seconds for the Flow IndexedDB deletes: a tab of an older build
  that ignores `versionchange` leaves its delete queued (it completes when that tab closes)
  instead of holding the sign-out page.
- Flow Web client follows the server's 2026-10-10 answers: a create or move target outside the
  workspace is shown as not found (the `cross_workspace_relation` wording now describes a stored
  relation found by the relations read, its only producer); a conversion retry answered
  `server_draining` / `contention` says another request is running the job, asks to try again
  after `retry_after_ms`, keeps retry available and follows the job; a failed commit's
  `details.job_status` (`failed`, `started`, `completed`) decides how the job page takes over; a
  Record keeps the package export option and a `field_secrecy` refusal says to export JSON or
  Markdown instead.
- Flow conversion E2E observes the real reclaim path: the Web E2E stack runs the API with
  `[flow] conversion_reap_interval_secs = 2`, and the spec only moves a stuck `started` job's
  start 16 minutes back, then waits for the API's periodic reaper to fail it with
  `server_rejected` / `stale_started` instead of writing those result fields itself.
- Flow Collection table virtualisation and records paging have real checks: a server-side render
  of the table with 250 records must put only the virtual window in the DOM, and a browser test
  over 55 REST-created records requires a first read of `limit=50`, the next page by cursor
  after scrolling, 55 loaded, and fewer than 55 rows in the DOM; it also opens a record on its
  own route and requires the `GET /flow/objects/{record_id}/schema` read.
- Flow test gates gained the checks whose absence let regressions through: the i18n parity gate
  covers the Web-local error codes (`network_error`), the reference card tests pin the
  `unconfigured` permission badge and admit cards by `visibility` only, the Collection create
  tests require a fresh intent after a success, and the named-reason test compares against an
  independent table; the unused `REPREVIEW_REASONS` export is removed; the CI Web E2E job fails
  when any test was flaky (passed only on a retry).
- Flow recovery draft "clear" deletes only the drafts that tab showed, so a draft another tab of
  the same user added meanwhile is kept; an unknown `limits.version` in a bootstrap makes the
  canvas read-only as well as the outbox, so no edit is made that can be neither sent nor kept;
  the `network_error` message says the outcome is unknown and that retrying the same change is
  safe, instead of claiming nothing was committed.
- Flow standalone Record route reads its owning Collection's schema through
  `GET /flow/objects/{record_id}/schema`; the uncalled `ObjectRepository.queryRecords` and
  `openRecordIds` adapters are removed.
- Flow writes are never resent automatically under a new idempotency key: an
  `idempotency_key_body_drift` refusal from object create, commands or record create reaches the
  caller as a typed error and is shown as such (an automatic re-key could execute an already
  committed intent twice); a retry after `stale_frontier` stays an explicit new intent.
- Flow presence identifies a peer by the frame's server-attested `actor.user_id` (the payload's
  self-reported `userId` only seeds the colour for a frame without `actor` and is never shown as
  the peer's identity); a peer colour that is not `#rrggbb` is replaced by the palette colour and
  a peer name is reduced to plain text of at most 64 characters, so no peer-controlled value
  reaches the canvas or editor styles.
- Flow offline outbox, recovery drafts and the snapshot cache are kept per signed-in user (one
  IndexedDB database per user, `sylvode-flow-outbox-v1:<user_id>` and
  `sylvode-flow-cache-v1:<user_id>`), so another account on the same browser never replays,
  drafts or reads them; only the user's own sign-out deletes every Flow IndexedDB database of
  the browser, while an expired session or a failed session check (network failure, 5xx) keeps
  them so the same user's unsent edits replay after signing in again.
- Flow Share panel: retrying a permission downgrade whose dry run failed (unavailable, network
  error, draining) runs the dry run again and stops at its preview; a downgrade is only ever
  committed after its `permission_changes` were shown.
- Flow package wizard, export scope and the Forms pickers list every project of the workspace
  (read page by page up to `total_pages`) instead of only the first 100.
- Flow operations page: the per-document lag rows and the integrity document list page through
  every document with the server's cursors (at most 100 per page) instead of stopping at the
  first page.
- Flow conversion job page: a job the server reclaimed after it stayed `started` too long
  (`error_reason = "stale_started"`) is shown as interrupted and can be retried, while any other
  `server_rejected` job stays permanent; a stored `stale_frontier` is worded for the job page.
- Flow Web commands report the Web-local `network_error` (`recoverable`, retried under the same
  idempotency key) when the API gives no answer or a 5xx without an `error_code`, instead of a
  permanent `invalid_update`; an unknown 4xx is still `invalid_update`. The Page slash menu shows
  a retryable transport failure apart from a server refusal.
- Flow Web collaboration consumes the collaboration contract directly now that the server
  answers it: a replayed `update_id` is resent and acknowledged from the server's original
  receipt (no longer guessed from the snapshot), a `rejected` frame is matched only by its own
  `update_id` (a refusal that names no update is no longer pinned on the update in flight), and
  `policy_rejected` keeps the intent in the recovery draft and turns the page read-only without
  reconnecting to retry it. `invalid_update` reasons `causal_dependency_missing` and
  `cross_workspace_relation` have their own wording, and the offline-recovery evidence is written
  under the repository's `.flow-gate/cache/frontend/` (or `SYLVODE_SCRATCH_ROOT`).
- Flow: `GET /flow/objects/{id}?render=markdown` renders an object whose projection state is
  empty again (the secret-Record redaction had skipped rendering for any empty
  `semantic_content`, not only for redacted Records).
- Flow collaboration: an `update` whose causal dependencies are not in the document (the engine
  could only buffer it as pending) is refused with `invalid_update` and
  `details.reason = "causal_dependency_missing"`; it gets no seq and nothing is stored. It used to
  be accepted with a seq while its content stayed invisible. Replaying already-stored history is
  unchanged.
- Flow collaboration: a `rejected` frame answering an `update` carries that update's
  `update_id` when the update is refused by the `update_rate` or `frame_rate` limiter or is not
  a well-formed frame. Only the `type` and `update_id` members of a frame refused before
  decoding are read; a frame over `websocket_frame_bytes` is still refused on its length alone.
- Flow collaboration: re-sending an `update` whose `update_id` is already committed is answered
  with the original `accepted` receipt (same `head_seq`, `head_frontier` and `event_id`), on the
  connection that committed it and after a reconnect. The receipt used to be dropped as a
  duplicate seq, so a client retrying an update in `write_state: "unknown"` with the same
  `update_id` never got an answer.
- Flow collaboration: an open WebSocket connection re-evaluates its authorization when the
  workspace `authz_epoch` has moved since it opened. A write refused by the commit-time epoch
  fence is re-sourced in place: if the actor still has `edit` on the document the connection
  takes the new epoch and the update is retried (and accepted); only an actor whose level is now
  below `edit` gets `policy_rejected`. A connection opened read-only that has since been granted
  `edit` is re-sourced the same way. Before, any authorization change anywhere in the workspace
  made the next write on every existing connection `policy_rejected`, including the owner's.
- Flow: a stored cross-workspace `flow_relations` row met while listing relations answers
  `invalid_update` with `details.reason = "cross_workspace_relation"`, so a client can show it as
  an integrity failure without reading `message`. The integrity record is written as before.
- Flow (security): a create whose `project_id` or `parent_object_id`, or a `move_object` whose
  target, names an object in another workspace answers the same `not_found` envelope, byte for
  byte, as an id that does not exist; a missing `project_id`, `parent_object_id` or move target
  now answers `not_found` instead of `invalid_update`. A cross-workspace id is still recorded as
  a `cross_workspace_relation` integrity record. The write path no longer reveals that an id
  exists in another workspace.
- Flow: `GET /flow/objects/{id}/bootstrap` needs `view`, the level the WebSocket `open` and
  `snapshot` (which use the same loader) already need; it used to require `edit`, so a read-only
  member could watch a document live but not download its bootstrap.
- Flow Collections: `GET /flow/collections/{id}/records` honours `view_id`. Records are queried
  with the view's saved `config.filter`, `config.sort` and `config.group_field_id`; an explicit
  `filter`, `sort` or `group` parameter overrides the saved value for that part only, and a
  `view_id` that is not a view of the Collection is `invalid_update`. The parameter used to be
  ignored.
- Flow Collections: `GET /flow/objects/{record_id}/schema` returns the owning Collection's
  schema (the same body as the Collection's own schema) to a caller who can read the Record and
  its Collection; it used to answer `not_found` for every Record. `GET /flow/collections/{id}`
  still accepts only Collections.
- Flow Pages: `create_collection_embed` accepts `after_block_id` and inserts the embed block
  right after that block (under the same parent); without it the block is still appended. An
  `after_block_id` that names no live block of the Page is refused with `invalid_update` and
  nothing is created. `after_block_id` is part of the idempotent request body, so replaying the
  key with another position is body drift.
- Flow Collections: a record value that has the right JSON type but is not a valid value of its
  field is refused as `invalid_update` with `details.reason = "invalid_field_value"` and
  `details.field_id`, on records create and `record_patch`: an unparseable date (it used to fail
  in the typed index cast and answer an untyped HTTP 500), a `select`/`multi_select` value that
  is not in the field's `config.options`, and a relation element that is not a UUID or names an
  object outside the Collection's workspace. A query filter with an unparseable date is a typed
  `invalid_update` instead of a 500. A `select`/`multi_select` field's `config.options` must be
  an array of distinct, non-empty strings on `field_create`, `field_update` and an initial
  schema; fields without `options` stay unconstrained.
- Flow Collections (security): under `field_secrecy_enabled`, the single-object read
  `GET /flow/objects/{record_id}` in both renders, the object list, the object diff, and Record
  command responses (`object.semantic_content` and `semantic_diff`) apply the records query's
  redaction (`semantic_content` is `{}`). They used to return every `record_property` value,
  including restricted ones, and the Record body.
- Flow maintenance operations: `OperationReceipt.audit_event_id` is never null. A replayed
  operation whose audit row cannot be found, or an execute that produced none, answers
  `server_rejected` instead of a receipt with `audit_event_id: null`.
- Flow conversions: when a commit or retry fails after its job exists, `details.job_status` is
  always present and is the status the job row really has: `failed` normally, `started` when
  recording the failure itself failed (the periodic sweep fails the job later), `completed` when
  the run had committed before the error.
- Flow Collections: a `relation` value that names the same object more than once is accepted
  when that object is in the Collection's workspace; it used to be refused as
  `invalid_field_value` because repeated ids were counted as missing ones.
- Flow collaboration (security): every `presence` frame the server fans out, live or in the
  snapshot a joining session receives, carries `actor: {"user_id": ...}`, the user of the
  connection that sent it as authenticated by its collaboration ticket. The payload is still
  relayed as sent; an `actor` a client puts on its own frame is ignored. Clients must take the
  peer's identity from `actor`, never from the payload's self-reported `userId`/`name`.
- Flow REST: a query string that does not parse on any `/flow` route (a non-numeric `limit`,
  `before_seq` or `at_seq`, a non-UUID `view_id` or `project_id`, ...) answers `invalid_update`
  in the business envelope instead of axum's plain-text HTTP 400.
- Flow authorization: `PUT /flow/objects/{id}/grants` and `PUT /flow/objects/{id}/inheritance`
  compare a replayed `idempotency_key`'s request body (roster, levels, `inherit_from_parent`,
  `initial_grants`, `confirm_self_lockout`) with the first request's. A different body answers
  `invalid_update` with `details.reason = "idempotency_key_body_drift"` and changes nothing; it
  used to return the first request's receipt with `applied: true` and silently drop the new
  roster. A key first used before this release cannot be verified and is refused the same way.
- Flow exports (security): an object export of a Record under `field_secrecy_enabled` applies the
  same redaction. `json` carries `semantic_content: {}`, `markdown` and `csv` carry the title
  only, and `package` (which would contain the Record's full collaboration snapshot) is refused
  with `policy_rejected` and `details.reason = "field_secrecy"`. A workspace package skips such
  Records' documents and lists each one in the manifest `warnings[]` as
  `secret_record_excluded:<record_id>`. Object exports used to return the restricted values
  verbatim to any member who could view the Record, including through MCP `objects.export`.
- Browser E2E stack: `scripts/e2e-web-stack.sh up` also starts the background worker on the
  same configuration (it is the only writer of the Flow search index), stops it on `down`, and
  refuses to start when `worker` is missing from `E2E_BIN_DIR`. The CI Web E2E job runs every
  `tests/e2e/web/flow-*.spec.ts` plus the workspace home spec.
- Web UI, Flow object page: an object whose type has no registered renderer and no collaboration
  session (a Collection, for which the server refuses a collab ticket) shows its title, the
  "view isn't available yet" fallback and the object panel, instead of the "Flow is not enabled"
  / permission error the failed session open produced.
- Web UI, Flow operations panel: maintenance receipts show the `OperationReceipt` contract
  fields — `mode`, `scope`, observed head, start and finish times, `audit_event_id`, the
  `changes[]` before/after of a compaction or projection rebuild, and the two warning codes. The
  integrity table shows the workspace-wide `counts.total` and lists failed documents with their
  `failure` (`storage_inconsistent`, `projection_missing`, `limit_exceeded`,
  `document_missing`, `replay_failed`) and error code instead of only counting them, and the lag
  section shows the admin lag endpoint's per-document rows.
- Web UI, Flow conversion job page: shows the job's `target_type`, words the stored `error`
  through `flow.error.<code>`, shows each warning by its code, and words a refused retry by
  cause (`stale_frontier`, `policy_rejected` with `target_schema_changed` or
  `permission_unconfigured`) before reading the job again. The retry confirmation no longer
  says the retry depends on the original preview. The browser spec now builds failed jobs
  through the production path (permission revoked between preview and commit) and proves a
  retry succeeds after the preview TTL has passed.
- Web UI, Flow convert wizard: `policy_rejected` is worded by its `details.reason`. A commit
  refused with `preview_expired` or `target_schema_changed` returns to the preview step and
  previews again under a new key (after reading the form again for a schema change);
  `permission_unconfigured` asks for an admin to configure the form's policy. A commit that
  fails after its job exists (`details.job_id`) opens the conversion job page, where it can be
  retried. Preview warnings are shown one per line, worded by their `<code>:<field_key>` code.
- Web UI, Flow package import: the wizard now follows the preview's `conflict_details`. A
  `project_unmapped` conflict returns to the mapping step with one row per unmapped source
  project (by title) that needs an explicit target or Unprojected and a new preview; a
  `lineage_exists` conflict under "Reject the import" lists the already-imported objects, keeps
  Confirm closed and offers a re-preview with "Reuse the earlier import". The preview shows the
  package's `manifest_summary`, and a commit refused with `invalid_update` /
  `reason = "project_unmapped"` or `policy_rejected` has its own wording. A workspace export
  whose POST answers `status: "failed"` is shown as failed with its `error` code's text.
- Flow conversions: a job left `started` for more than 15 minutes (its run crashed after the
  job was created, or recording its failure failed) is now reaped to `failed` with
  `error = "server_rejected"` and one business `flow.conversion.failed` event whose payload
  carries `reason = "stale_started"`, so it can be retried instead of being stuck. The API
  reaps such jobs at startup and then periodically, every `[flow] conversion_reap_interval_secs`
  seconds (default 60, 1..=86400). A retry only accepts a `failed` job: a retry of a job that is
  still `started`, stale or not, answers `server_draining` with `reason = "contention"` and
  `retry_after_ms` and never reaps it in place. The 15-minute threshold is an implementation
  constant, not a contract limit.
- Flow conversions: when a commit or retry fails after its job exists, the typed error's
  `details` now also carry `job_id` and `job_status` (normally `"failed"`), so a client can open
  the job and retry it without a second request. `error_code`, `message` and the existing
  `details` (`reason`, `permission_state`, `retry_after_ms`) are unchanged.
- Flow conversions: `warnings` on the preview and on a completed record-conversion job are no
  longer always empty. Each entry is a stable `<code>:<field_key>` string:
  `target_value_missing` for a target field the mapping leaves empty (Forms fields have no
  default), `required_target_value_missing` when that field is required (the commit will be
  refused by Forms validation), and `unknown_target_field` for a mapped value that names no field
  of the target schema (it is dropped, as before, not rejected). `autonumber` and `child_table`
  fields are not reported. Form-creating conversions keep `warnings: []`.
- Flow conversions: retrying a `failed` job no longer depends on the preview's 15-minute TTL,
  so a job can be retried after its preview expired (previously that retry always answered
  `policy_rejected`). The retry re-authorizes every side and re-runs against the
  `source_frontier` and `target_schema_version` recorded on the job: a source that has moved on
  answers `stale_frontier` and a changed target schema `policy_rejected` with
  `details.reason = "target_schema_changed"`, and either one leaves the job `failed` with that
  code. Retry now also checks that the caller can read the source before answering, and a retry
  key already used by another job answers `invalid_update` with
  `details.reason = "idempotency_key_body_drift"`.
- Flow conversions: `policy_rejected` now says why with `details.reason` — `preview_expired`
  when the commit's preview is past its 15-minute TTL, `target_schema_changed` when the target
  form's schema version differs from the preview or job (at commit and at retry), and
  `permission_unconfigured` when the target form has no policy row and is therefore read-only
  through the bridge (ADR-0019 BR-4), at preview and at commit. The existing
  `details.permission_state` is kept where it was returned before.
- Flow conversions can now actually end `failed` in production. A commit first persists the job
  as `started` (with `flow.conversion.started`), then runs it; any failure from then on — a
  revoked Forms or Flow permission, an archived target form, a frontier or schema change found
  under the lock, a Forms validation error, a database error — marks the job `failed`, records
  `flow.conversion.failed`, and answers the caller with the typed error. Previously every such
  failure rolled the job row back, so `failed` (and therefore retry) was reachable only through
  a test-only fault hook. The job's `error` is now always one of the 14 stable error codes
  (`policy_rejected`, `stale_frontier`, `invalid_update`, `server_rejected`, ...) instead of an
  internal name such as `injected_before_target_create`; the internal cause is logged
  server-side only. A commit refused before the job exists (expired preview, a frontier or
  schema version that already differs, a caller who cannot see the source) still creates no job.
- Flow conversions: committing a preview that already has a job, under a different idempotency
  key, now returns that same job instead of a 500 from the one-job-per-preview constraint; two
  concurrent commits of one preview both get the single job. A replay of a commit (same key or
  same preview) returns its job even after the preview's 15-minute TTL. A key already used for
  another preview answers `invalid_update` with `details.reason = "idempotency_key_body_drift"`.
- Flow conversions: the job returned by commit, `GET /api/v1/flow/conversions/{job_id}` and
  retry now carries `target_type` (`form` or `form_record`), as the contract's job shape requires.
- Flow conversions: a malformed id in `GET /api/v1/flow/conversions/{job_id}`,
  `GET /api/v1/flow/conversions/{id}/owner` and `POST /api/v1/flow/conversions/{job_id}/retry`
  now answers the typed `not_found` envelope, like an unknown id, instead of a plain-text 400
  without `error_code`.
- Flow admin integrity: `GET /api/v1/admin/workspaces/{id}/flow/integrity` accepts `cursor`
  (previously any cursor answered 400) and pages documents in id order with `limit` 1..=100
  (default 50); `next_cursor` is present only when another page exists. With
  `scope=documents`, `documents[]` now lists every checked document: healthy rows carry
  `status: "ok"` and `checked_at` next to their fingerprint, and failing rows (previously only
  counted) carry `status: "failed"`, `checked_at`, a stable `failure` class
  (`storage_inconsistent`, `projection_missing`, `limit_exceeded`, `document_missing`,
  `replay_failed`) and `error_code`. `counts` (`total`, `checked`, `healthy`, `failed`) always
  cover every document in the workspace, whatever the `scope`, `cursor` or `limit`. A database
  error while checking now fails the request instead of being counted as a failed document.
- Flow admin lag: `GET /api/v1/admin/workspaces/{id}/flow/lag` now honours `cursor` and `limit`
  (1..=100, default 50) and lists one row per document in `projection.items`, `search.items` and
  `fanout.items` (`object_id`, `document_id`, `head_seq`, the consumer's seq, `lag`), paged in
  document-id order. `next_cursor` is present only when another page exists. The workspace-wide
  `max`/`p95` aggregates are unchanged. An invalid cursor or an out-of-range limit answers
  `invalid_update`.
- Flow maintenance operations: the `OperationReceipt` returned by document compact, projection
  rebuild and document verify now carries the contract's full shape — `mode`
  (`dry_run`/`execute`), `scope`, `observed_head_seq`, `changes[]` (before/after boundary and
  checksum for compaction, before/after projection hash for rebuild), `warnings[]`,
  `started_at`, `finished_at` and `audit_event_id` — alongside the existing fields. An executed
  compaction or rebuild now writes its audit `business_events` row (`flow.compaction.completed` /
  `flow.repair.completed`) in the same transaction as the change, and `audit_event_id` is that
  row's id (also on idempotent replay). A dry-run (compact, rebuild, verify) writes one
  `delivery_class=audit_only` event (`flow.compaction.previewed` / `flow.repair.previewed`, no
  dispatch row, no webhook) and its `audit_event_id` is that row's id.
- Flow export: an accepted, authorized export that fails while rendering (a canonical
  snapshot/update integrity check, or a server-side refusal) is now recorded as a `failed` job
  whose `error` carries the stable code (for example `checksum_mismatch`). `POST` returns that
  job receipt with `status: "failed"` instead of the bare error, an idempotent replay returns the
  same job, and `GET /api/v1/flow/exports/{job_id}` reports `error` and omits `download_url`
  (`checksum` is empty and `size` is 0, since there is no artifact). Request, permission and
  precondition errors still answer the request directly.
- Flow package import: previewing a package whose objects already have import lineage in the
  target workspace under `conflict_policy = "reject_existing"` succeeds and lists the hits as
  `lineage_exists` conflicts instead of failing; committing that preview (or any
  `reject_existing` commit whose objects gained lineage after the preview) is refused with
  `policy_rejected` and writes nothing.
- Flow package import: a preview whose `project_mapping` leaves a package source project
  unmapped (including an empty mapping) now succeeds and lists each such project in `conflicts`
  and in the new `conflict_details[]` as `{kind:"project_unmapped",source_project_id,title}`;
  it used to fail with a bare 400 that did not say which projects exist. Committing such a
  preview answers `invalid_update` with `details.reason = "project_unmapped"`. A mapping value
  may be the contract literal `"unprojected"` (same as `null`). `conflict_details[]` also
  describes import-lineage hits as `{kind:"lineage_exists",source_object_id,existing_target_id}`;
  `conflicts` keeps its string-array shape.
- Flow package import: `POST .../flow/imports/preview` returns the contract's `manifest_summary`
  (`projects:[{source_project_id,title}]`, `object_count`, `document_count`, `package_schema`,
  `engine`), so a caller can see which source projects to map before committing. Workspace
  package exports carry the referenced project titles in a new optional `projects` manifest
  field (omitted when empty, so packages without projects are byte-identical to before); for
  packages without it the title is `null`.
- Tests: two measurement assertions that only mean something under controlled conditions no
  longer fail on shared CI runners. The depth-32 inheritance-evaluation regression ceilings are
  enforced only with `OPENPR_FLOW_QUIET_PG_QUALIFIED=1` and otherwise recorded in the evidence
  line as `diagnostic_only_unqualified_environment`; the isolated-apply memory-ceiling test pins
  the exact-byte boundary on the counting allocator directly and accepts either resource ceiling
  for the end-to-end over-ceiling refusal, whose abort can lose the race to the 100 ms wall
  watchdog on a loaded host.
- `GET /api/v1/workspaces/{workspace_id}/projects` honours `page` and `per_page` (default 20,
  at most 100) and reports the real `total` / `total_pages`; it used to ignore both and always
  return every project. A request with neither parameter still returns the whole list.
- Flow: every error the Flow REST surface constructs (`/api/v1/flow/**`, the workspace Flow
  feature, projection-lag, conversion, package import/export and operations endpoints, and the
  collab ticket/WebSocket routes) is now a typed envelope carrying `error_code` (and `details`
  where the contract defines them) instead of the legacy `{code,message}` shape. A
  `flow_typed_error_gate` test lists any new untyped construction point on that surface.
  "Flow is not enabled for this workspace" is `feature_disabled` everywhere (it was a bare 403
  on `projection-lag` and every other route behind the feature check), workspace-admin and
  tool-policy refusals are `forbidden`, a missing `confirm` on dangerous operations is
  `policy_rejected`, and the not-found answers keep collapsing "no access" into `not_found`.
- Packaging: the `api` container image and every release archive now ship
  `collab-isolated-apply-worker` beside `api`. The API spawns that process for every Flow
  collaborative write and looks for it next to its own executable; the image built by
  `scripts/start.sh` (`Dockerfile.prebuilt`), the source `Dockerfile` and the release archives
  shipped `api` alone, so the first edit of a Page showed "Sync error", the text was lost on
  reload and `POST /flow/objects/{id}/commands` returned 500 (api log: `worker binary
  'collab-isolated-apply-worker' not found next to '/app/api'`). `scripts/start.sh --no-build`
  now also requires it in `target/release`, and the release job refuses an archive without it.
- Release: the macOS and Windows targets build again. `api` and `collab-isolated-apply-worker`
  used the Linux-only isolated-apply module (ADR-0014) unconditionally, so every non-Linux release
  target failed to compile. The module's types, wire codec and limits are now platform-neutral;
  off Linux the isolated-apply entry points refuse every Flow collaborative write and object diff
  with `server_rejected` (`details.reason = "isolated_apply_unsupported_platform"`,
  `write_state: not_applied`) instead of applying anything outside the boundary, `api` logs one
  startup `warn`, and `collab-isolated-apply-worker` is a stub that exits with status 2.
- Flow collaboration: publishing the cross-instance fanout notice after a committed update no
  longer reports a failure on every write. The statement filtered on `pg_notify(..) IS NULL`,
  which is never true (`pg_notify` returns `void`), so it returned no row and the API logged
  `committed update fanout publication failed ... internal server error` for each accepted
  update even though the notice row and the notification were written. The notification is now
  emitted from a LATERAL subquery in the same single round trip.
- Web UI: on screens narrower than 1024px the Flow navigator is a drawer, closed by default and
  opened from a 44px "Show navigator" button (`aria-expanded`, Escape closes it), so the object,
  convert and conversion job pages use the full width. At 390px the navigator used to sit beside
  the content and leave it about 70px wide. The object page stacks its context panel under the
  canvas there, and the convert and job pages' buttons are at least 44x44px.
- Web UI: the Flow operations panel is usable while Flow is disabled. It showed the "Workspace
  admins only" state because the per-object `GET .../flow/projection-lag`, which is refused while
  Flow is off, was allowed to decide the whole page. Only the admin check and the three
  `/admin/.../flow/{health,lag,integrity}` endpoints decide it now; the lag section shows its own
  notice that there is no per-object lag while Flow is off. Leaving the panel before its first
  load finished no longer starts the 15-second health poll after the page is gone.
- Web UI: `server_rejected` is now one of the client's registered stable Flow error codes, so
  the zh/en parity gate covers its keys, and a `rejected` frame carrying it on a live editing
  session fails the pending write as a permanent error instead of being ignored and leaving the
  write pending. The operations panel and the conversion job page classify it through the same
  registry; it is still shown as a permanent failure with no retry.
- Web UI: the sidebar's workspace-admin links (Members, Webhook, Operation Records, Workspace
  Settings, Flow settings) follow the viewer's member role in the workspace on screen. The role
  was read once when the app first loaded and an instance admin counted as a workspace admin, so
  a member could see another workspace's admin links and an owner who opened a workspace from the
  workspace list saw none until a reload. The role is now re-read on every workspace change, the
  links stay hidden until it arrives, and only `owner`/`admin` members see them, as on the server.
- `mcp-server serve` had no SIGTERM or SIGINT handler: it died of the signal (exit 143) and
  dropped the call it was serving, and as PID 1 of the compose `mcp-server` container it ignored
  the signal and was killed by the runtime after 10 s. On SIGTERM or SIGINT it now stops accepting
  connections (`http`, `sse`) or stops reading stdin (`stdio`), finishes the requests in flight,
  delivers an SSE result before it ends that stream, flushes stdout and exits 0. Requests still
  running 8 s after the signal are abandoned and logged, which keeps the stop inside the 10 s a
  container runtime waits before SIGKILL.
- `tools call` with a `cli_tools_call` token left no operation log: `bot_operation_logs` only
  accepted the surfaces `mcp_http`, `mcp_sse`, `mcp_stdio`, `cli` and `rest`, so every audit row of
  such a call was rejected by the table and only a warning was logged. Migration
  `0070_bot_operation_logs_surface_check.sql` adds `cli_tools_call` (expand-only; existing rows are
  untouched), the API takes its accepted bot surfaces from one list, and a test writes a token and
  an operation log row for every surface in that list against a real database.
- Documentation facts: the README's WASM plugin section had lost its heading and introduction and
  now states the current limits; it gave 2155 translation keys (2192) and 245 `.route()` calls
  (246), still mentioned the removed outbox, and described `playwright.config.ts` as targeting an
  internal address. `frontend/README.md`, `frontend/QUICKSTART.md`, `docs/API_ENDPOINTS_PHASE3.md`
  and `docs/frontend-requirements.md` are now in English. The Universal Forms acceptance guide and
  production runbook say that the `.flow-gate/universal-forms/` files they name are not in a fresh
  checkout and where each comes from. The v1.0 stable runbook and CONTRIBUTING list every input
  the v1.0 gate reads from outside the repository, and the bash and GNU tool requirement.
- The v0.9 and v1.0 reports and the v0.9 verifier read the predecessor receipt from a fixed path
  under the checkout even when `--evidence-root` named another directory; the default is now
  `<evidence root>/../<previous release>/gate-result.json`, as for v0.6-v0.8.
- `scripts/test-no-instance-literals.sh` no longer reports a private range written in CIDR
  notation (`10.0.0.0/8`), which names a range rather than an address.
- The root `Dockerfile` did not copy `Cargo.lock` or the spike crates that are workspace members,
  so it could not load the workspace; it now copies both and builds with `--locked`. A root
  `.dockerignore` keeps the build context to what the two Dockerfiles copy (no `target/` apart from
  the four release binaries, no `.flow-gate/`, no generated configuration or `.env`), and
  `backups/`, where `scripts/backup-db.sh` writes database dumps, is ignored by git.
- `SECURITY.md` named a private reporting channel that is not enabled until the release is
  published and called the release line "pre-1.0" next to a 1.x compatibility commitment. It now
  says private vulnerability reporting is enabled with the release, to use the e-mail contact
  while the **Report a vulnerability** button is absent, and that only the latest release is
  supported.
- `scripts/start.sh` copied a quoted `.env` value (`SYLVODE_FRONTEND_PORT="3999"`) with its quotes
  into the generated TOML, which then failed to load; values are unquoted as docker-compose does,
  and a port that is not a number is refused before any file is written. A legacy variable set
  only in the environment conflicted with the canonical values `.env.example` preset; those lines
  are now commented out (the defaults are unchanged). An unknown argument prints a usage text and
  exits 2 instead of building and starting the stack.
- The compose comment, the production runbook and `.env.example` presented
  `ghcr.io/openprx/sylvode-webhook` as published. They now say it exists only from the first
  Sylvode Webhook release after 0.3.3, how to pin a published image until then, and to pin a
  version tag rather than `latest`.
- The release workflow published a release, with all six platforms listed, even when a build or
  package job had failed (the arm64 leg was allowed to fail), and never checked the tag against
  the version. It now fails before building unless the tag is `v<version>` equal to the
  `Cargo.toml` workspace version and `frontend/package.json`, publishes only when every build and
  package job succeeded and every archive and checksum is present, checks out the tag for manual
  runs, writes `sha256sum`-compatible checksums on Windows and for the frontend archive, pins
  `cross` and installs frontend dependencies from the lockfile. All workflow actions are pinned by
  commit, CI has read-only permissions, and only the release job can write. CI also runs
  `cargo check --all-features` and the frontend unit and contract suites.
- The brand residue gate (`scripts/verify-sylvode-brand-residue.sh`) saw only `OpenPR` and
  lowercase `openpr` in UTF-8 file content, so `OPENPR_*`, `OpenPr*`, file names, symbolic link
  targets and UTF-16 text were never checked. It now matches `openpr` in any letter case in file
  content (UTF-8 and UTF-16), tracked paths and link targets, adds the reason
  `internal_identifier` (identifier tokens in source files only), and under `--release` fails on a
  checkout with uncommitted changes. The allow-list classifies the newly visible names: legacy
  compose variables, `OPENPR_TEST_DATABASE_URL`, the scripts' and tests' `OPENPR_*` inputs and the
  Rust client types; four entries that were wider than their reason were narrowed. Every kept
  identifier is listed under "Not renamed" in the v1.0 compatibility matrix.
- The bun tests for the bot token form and the webhook event form only searched the page source,
  so a form that always sent `rest`, or a checkbox that did nothing, passed them. The request the
  members page sends is built by `createBotRequest` and the event selection by
  `toggleWebhookEvent`, both tested for their behaviour, and the page checks ignore comments.
- `auth.allow_insecure_cookies` treated any `server.bind_addr` host starting with `127.` as
  loopback, including host names such as `127.example.org`. Only loopback IP addresses and
  `localhost` count now.
- `sylvode --help <workspace group>` printed internal source documentation as the program
  description and listed only the nine workspace groups. A help or version flag before the first
  group now always shows the top-level help of all fifteen groups, and no parser prints source
  documentation.
- `--version` / `-V` were refused (exit 2) by `mcp-server` and `sylvode`; they now print the
  program name and version and exit 0.
- The legacy configuration notice's "once per process" guarantee had no test of its own; a test
  now asks for it twice (and again after a second load) in a fresh process.
- Nothing pinned the plugin ABI export names against a rename applied to the runtime and its
  tests together. A compiled fixture, `apps/api/tests/fixtures/plugin-abi-v1.wasm` (source
  `plugin-abi-v1.wat` beside it, hash checked by the test), exports exactly `memory`,
  `openpr_alloc`, `openpr_invoke` and `openpr_plugin_abi_version` and must load and round-trip
  its input.
- `projects create` under `mcp-server` and `sylvode` failed with ``missing field `key` ``
  because it offered no way to pass the required project key; it now takes `--key`.
- `mcp-server` and `sylvode` no longer panic when stdout or stderr cannot be written. This
  includes log lines: any log event (the legacy configuration notice, a `warn` from `tools call`,
  the stdio server's startup lines) written to an unwritable stderr (`2>/dev/full`, a full disk
  behind a redirected log) made the logger report the failure on that same stderr and panic with
  exit 101. Log lines that cannot be written are now dropped, in `api`, `worker`, `mcp-server` and
  `sylvode`, and the command exits as it would with a writable stderr.
- `--config`, `--transport` and `--bind-addr` help printed backslash-escaped brackets.
- The notice that `logging.output = "stdout"` was overridden was logged under a target the
  default filter drops, so it was never shown.
- Failed plugin invocations recorded `duration_ms = 0` and no fuel, and a deadline expiry was
  recorded as `failed`.
- Bot tokens created on the Members page could only be used with the REST API: the form sent
  no transport surface, so the API bound every token to `rest` and refused it with 401 through
  MCP (HTTP, SSE, stdio) and the command-line tools. The form now requires choosing where the
  token will be used, the token list shows each token's surface, and the token-reveal dialog
  says where the token works.
- The Webhooks page offered only `issue.created`, `issue.updated` and `comment.created`,
  although the API accepts 14 events. The form now offers all of them, grouped by work items,
  comments, labels, sprints and AI tasks, with translated labels.
- `scripts/start.sh --check-config` (and every `start.sh` run) refused valid configuration
  files that use `[flow]`, `[audit]` or `auth.allow_insecure_cookies`. Its key table now lives in
  `scripts/lib/sylvode_config_schema.json`, which a test holds equal to the keys the binaries
  accept.
- A compose deployment generated by `scripts/start.sh` could not edit collaboratively: the
  generated file set no `[flow] collab_allowed_origins`, and the empty default refuses every live
  editing session. Newly generated files list the origins the frontend is published at, derived
  from `SYLVODE_BIND_HOST` and `SYLVODE_FRONTEND_PORT`, with a comment to replace them with the
  public origin. Existing files are not rewritten; `--check-config` now warns when the list is
  empty and rejects entries that are not `scheme://host[:port]`.
- `work-items create/update --priority` offered `none`, which the API rejects; the accepted values
  are now exactly the tool's (`low`, `medium`, `high`, `urgent`) and anything else is refused
  before a request is sent.
- `sylvode <group> --help` described `--config` and `--bot-token` in terms of a `sylvode serve`
  command that does not exist.
- `scripts/bootstrap-restaurant-demo.sh` refused `http://[::1]:<port>` as a non-local API URL.
- The documentation index listed four `docs/prd/` files that are not in the repository, and
  `docs/universal-forms-and-plugins.md` still described connectors, connector suggestions and the
  event outbox, all removed in 0.2.21. The documented `bun --cwd frontend run ...` commands only
  printed bun's usage; they now read `bun run --cwd frontend ...`.
- The production readiness audit checked for a `localhost:3000` API default in
  `apps/mcp-server/src/main.rs`, which no longer holds that code, so the check could not fail. It
  now checks `DEFAULT_MCP_API_URL` and `apps/mcp-server/src/cli.rs`, and every audit's
  `not_contains` fails when the file it inspects does not exist.
- Scripts under `scripts/` defaulted to paths on one maintainer's machine: the Universal Forms
  report generators, render smokes and screenshot collector wrote to a report directory outside
  the repository and overwrote what was there, the Flow gate scripts read their contracts from and
  wrote evidence into a fixed contracts checkout, and scratch trees, cargo targets and the Bun
  binary were looked up at fixed absolute paths. Generated output now defaults to the ignored
  `.flow-gate/` directory of the checkout, outside inputs are named through `--contracts-root`,
  `SYLVODE_CONTRACTS_ROOT` and similar variables (see CONTRIBUTING, "Where scripts read and
  write") and a missing one is an error, the render smokes and the screenshot collector refuse a
  non-empty target directory without `--overwrite`, and `scripts/test-no-machine-paths.sh` runs
  in CI.
- The README said the files `scripts/start.sh` generates are `chmod 600`; only `.env` is. The two
  compose configuration files are `644` so the container user can read the bind mount, and the
  README now says so and recommends restricting the `config/` directory instead.
  `docs/universal-forms-production.md` no longer describes connector receivers, connector
  receipts, the event outbox or the `connectors.*` tools, all removed in 0.2.21, and its audit
  stopped requiring the outbox sentence. The bundled MCP skill no longer lists a fixed work-item
  state enum or the `none` priority: states come from the project's workflow.

## [0.2.21] - 2026-08-19

### Added

- Bot operation records: a metadata-only record of every bot call (migration `0051`) and the MCP
  tool `bot_operation_logs.list`, with a retention period set by
  `[audit] operation_log_retention_days`.

### Removed

- Connectors and agent invocations, including the eight `connectors.*` MCP tools and the
  `[connectors.secrets]` configuration section; their tables are dropped by migration `0052`.
- The write-only event outbox (migration `0053`).

### Fixed

- The frontend API proxy passes WebSocket upgrades.
- The system default workflow's states open read-only.
- AI task dispatch requests are signed.
- Migrations `0048` and `0049` are guarded against the relations that `0052` and `0053` drop.

## [0.2.8] - 2026-08-16

### Changed

- Release build fix only: the release target is installed into the pinned toolchain. No
  user-visible change.

## [0.2.7] - 2026-08-16

### Changed

- The Rust toolchain is pinned in `rust-toolchain.toml`, the workspace builds with no clippy
  warnings, and numeric casts were reviewed at each site. No user-visible change.

## [0.2.2] - 2026-08-15

Version 0.2.1 was not tagged; its changes first shipped in 0.2.2. 0.2.1 is a breaking release:
an existing deployment will not start without configuration changes, and several operations that
previously succeeded now return `403`. Follow `docs/upgrade-0.2.1.md` (shipped with that release)
before upgrading from 0.1.x.

### Breaking changes

- **All configuration moved from environment variables to a TOML file.** `api`, `worker` and
  `mcp-server` read no environment variables at all. The file is named with `--config <PATH>`,
  defaulting to `config/openpr.toml` relative to the working directory (`config/sylvode.toml`
  since the Sylvode rename); sections are `[server]`, `[database]`, `[auth]`, `[logging]`,
  `[storage]`, `[migrations]`, `[outbound]`, `[mcp]` and `[connectors.secrets]` (the last was
  removed in 0.2.21). `DATABASE_URL`, `JWT_SECRET`, `RUST_LOG`, `BIND_ADDR`, `APP_NAME`,
  `OPENPR_OBJECT_STORAGE_*`, `OPENPR_MIGRATIONS_*`, `OPENPR_OUTBOUND_*`, `OPENPR_API_URL`,
  `OPENPR_BOT_TOKEN`, `OPENPR_WORKSPACE_ID`, `OPENPR_MCP_TRANSPORT` and `OPENPR_INVOCATION_ID` are
  inert. Unknown keys, placeholders and unexpanded `${...}` templates are startup errors, and
  validation reports every unusable value at once. `[database]` and `[auth]` are validated
  lazily, so an MCP-only deployment can omit both sections. Compose mounts two generated files,
  one for api and worker and one for mcp-server; `scripts/start.sh` generates and validates both.
- **The MCP shared inbound secret `mcp.auth_token` was removed.** A configuration file still
  carrying the key is refused at startup with an explanation. Under the `http` and `sse`
  transports each caller presents its own MCP-type account token in
  `Authorization: Bearer opr_...`, which the server forwards to the API unchanged and never
  verifies itself; a request without one is answered `401`, and only `/health` is exempt.
  `stdio` and the CLI subcommands act as `mcp.bot_token`. A calling bot outside the workspace
  named by `mcp.workspace_id` is refused `403`.
- **Bot token permissions are enforced.** `workspace_bots.permissions` was previously stored and
  ignored; safe methods now require `read`, everything else requires `write`, and `admin` implies
  both. Because the column defaults to `["read"]`, every pre-existing bot becomes read-only on
  upgrade and has to be granted write explicitly. Issuance rejects an empty permission array and
  unknown or wrongly cased names.
- **Connector credentials are referenced by bare name.** The retired
  `env:OPENPR_CONNECTOR_SECRET_W_<uuid>_<NAME>` form is refused with an explanation. Values are
  filed per workspace under `[connectors.secrets."<workspace-uuid>"]`, and a lookup is confined
  to the owning workspace, so no reference can reach another tenant's credentials. Stored
  `auth_policy.secret_ref` values must be rewritten by hand.
- **Outbound requests are SSRF-filtered.** Connector and webhook targets resolving to loopback,
  private, link-local or CGNAT addresses are rejected on create and update (`400`) and again at
  delivery time unless the host is listed in `[outbound] allowed_hosts`. Compose service names
  must be listed. Creating a webhook now also requires the workspace `owner` or `admin` role,
  matching update and delete.
- **Migrations are tracked in a ledger and a failure aborts startup.** The new
  `schema_migrations` table records each migration as `applied`, `adopted` or `failed`, and an
  applied migration is no longer replayed on every start. A failure used to be downgraded to a
  warning; it now stops the service, with `[migrations] continue_on_error` and
  `[migrations] replay` as recovery escape hatches. An existing database has its historical
  migrations marked `adopted` on first start.
- **Proposal endpoints are authenticated and tenant-scoped.** `GET /api/v1/proposals` and its
  sub-resources had no authentication and returned every proposal in the instance. Migration
  `0050` adds `proposals.workspace_id` and reads are filtered by it; rows whose owning workspace
  cannot be derived are left `NULL` and are visible to instance administrators only. Creating a
  proposal requires an explicit `workspace_id` when the author belongs to more than one
  workspace. Settlement of expired proposals moved from the API read path into the worker, so a
  deployment that runs the API without the worker no longer settles proposals.
- **`PUT /api/v1/labels/:id` requires the `owner` or `admin` role**, and the `admin` permission
  for a bot token. Labels are workspace-level and one edit changes them across every project.
  Delete is held to the same bar.
- **Attachment downloads require authorization.** `/uploads/*` and `/api/v1/uploads/*`,
  including the `thumbnails/`, `previews/`, `variants/` and `signatures/` sub-paths, were public.
  They now admit either a session (JWT or bot token) or a valid, unexpired signed download URL,
  and the object is then scoped to the workspace that owns it.

### Added

- Universal forms: project-defined business data types with grid and detail views, record links
  and child tables, formulas, per-role permissions, import and export, attachments and
  electronic signatures; WASM plugins for field validation, formulas and event handlers;
  configurable workflows; scenario templates. The MCP server grew from 34 to 105 tools.
- `scripts/start.sh --no-build` starts a deployment from prebuilt binaries in `target/release`,
  for hosts with no Rust toolchain, and refuses to proceed naming any binary that is missing.
- `frontend/Dockerfile.prebuilt` serves an existing `frontend/build/` for hosts with no Bun
  toolchain.
- `Dockerfile.prebuilt` takes its runtime base image from a compose variable, which must ship a
  glibc at least as new as the build host; `scripts/start.sh` derives it from the host and
  persists it to `.env`.
- `scripts/start.sh --check-config` validates both generated configuration files and `.env`
  without starting anything.
- MCP list results carry a `pagination_hint` (`total`, `total_pages`, `has_more`, `next_page`),
  `form_records.list` forwards paging, view, sort and filter parameters, and `serverInfo`
  reports the real version.
- An optional `project_id` filter on `/api/v1/search`, validated against the caller's scope
  before any result query runs.
- Test coverage for the MCP server over real SSE sockets and the CLI, caller identity and HTTP
  auth, policy bypass attempts, configuration validation, the migration ledger against a live
  PostgreSQL, delivery pickup, and a frontend round trip over all 27 form field types.

### Fixed

- **Universal forms, cross-tenant reads.** Record link targets are validated against the source
  record's workspace, project and `form.view` permission; child aggregate, child listing and
  parent recalculation queries are scoped to the parent record's workspace and project; and
  `record_scope=owned` is honoured on every record endpoint, not only on list.
- **Universal forms, async job artifacts.** Export, import and attachment package job results are
  restricted to the job's creator, and bulk result bodies are no longer returned in job listings.
- **Universal forms, events bypassing field-level permissions.** Event payload redaction is
  declared per event type instead of by a key-name denylist, and an event type with no
  declaration withholds its payload.
- Attachment claims and record writes that point at an upload owned by another workspace are
  rejected.
- Stored values of hidden or unreadable fields are preserved on record update, and formulas are
  evaluated against the stored record merged with the payload.
- Required `child_table` fields are enforced where the row count changes instead of rejecting
  every update after creation.
- **MCP errors are no longer reported as success.** The API returns HTTP 200 for every error and
  carries the status in the `{code, message, data}` envelope; the client now validates the
  envelope on every verb, and two tools that returned a hardcoded `{"success": true}` return the
  backend payload.
- `CallToolResult.is_error` is serialized as `isError`, the name MCP clients read, and tool
  results are no longer silently truncated.
- **The MCP project policy gate covers all 105 registered tools**, previously only the form tools.
  Project identifiers are validated as UUIDs and percent-encoded, every tool is classified
  against the live registry, the owning project is resolved from the object rather than taken
  from call arguments, and `resources/read` runs through the same policy.
- An SSE `POST /messages` checks that its stream still exists before running the tool.
- **Delivery reliability.** Expired leases are reclaimed, completion writes must present the
  lease token, exhausted rows are dead-lettered, failures retry with capped backoff, and a
  delivered payload is not re-sent by crash recovery.
- **Delivery signing is enforced.** A signed delivery without a resolvable credential is refused
  instead of being sent unsigned; signatures use `X-Webhook-Signature: sha256=<hex>` over the raw
  body. The legacy webhook sender no longer follows redirects, re-validates the target before each
  request and caps what it reads and stores.
- The worker installs the runtime configuration the API library reads, so form jobs and retention
  use the configured object store.
- Logging defaults to stderr, so it can no longer corrupt the MCP stdio protocol.
- `/api/v1/search` is scoped to the caller's projects, and bot tokens to their own workspace.
- Frontend record updates no longer drop fields the editor did not round-trip, and `multi_select`,
  `number`, `rating`, `progress` and `location` values are submitted correctly.
- Attachment URLs with unsupported schemes are not rendered, and `frontend/nginx.conf` no longer
  publishes the MCP server on the web origin.
- Migration `0048` adds partial indexes for lease recovery and delivery pickup.

### Security

- Dependency security updates; `.cargo/audit.toml` and `deny.toml` document each ignored advisory
  that had no upstream fix, with the reason.
- Credentials are held in a `Secret` wrapper that redacts them in every `Debug` and error output.

## [0.1.4] - 2026-03-18

### Added

- Configurable workflows with an effective-workflow resolver; the board and issue status pickers
  are driven by the workflow API.
- MCP server CLI subcommands for direct tool invocation, including `files upload`.
- A release workflow for all platforms that ships `api`, `worker`, `mcp-server` and the frontend.

### Changed

- Licensed under MIT OR Apache-2.0.
- Outbound HTTP uses rustls, which makes the ARM64 builds work.

## [0.1.3] - 2026-03-01

### Added

- Pagination for the `work_items.list` MCP tool.

### Fixed

- The workspace admin sidebar guard, and image and video paste upload in issues.

## [0.1.2] - 2026-02-28

Same source as 0.1.1, republished.

## [0.1.1] - 2026-02-28

First public release of the MCP server binaries.

### Added

- Project management API and web UI: workspaces, projects, issues, comments, labels, sprints,
  board, activity feed, notifications, governance and AI task callbacks.
- MCP server as an HTTP client of the API, with 34 tools, resources and resource templates over
  HTTP, stdio (including Content-Length framing) and SSE.
- Workspace bot tokens (`opr_` prefix) and their management endpoints.

[Unreleased]: https://github.com/openprx/sylvode/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/openprx/sylvode/compare/v0.2.539...v0.5.0
[0.2.539]: https://github.com/openprx/sylvode/compare/v0.2.21...v0.2.539
[0.2.21]: https://github.com/openprx/sylvode/compare/v0.2.8...v0.2.21
[0.2.8]: https://github.com/openprx/sylvode/compare/v0.2.7...v0.2.8
[0.2.7]: https://github.com/openprx/sylvode/compare/v0.2.2...v0.2.7
[0.2.2]: https://github.com/openprx/sylvode/compare/v0.1.4...v0.2.2
[0.1.4]: https://github.com/openprx/sylvode/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/openprx/sylvode/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/openprx/sylvode/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/openprx/sylvode/releases/tag/v0.1.1
