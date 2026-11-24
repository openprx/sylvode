# REST API reference

This page explains the conventions every Sylvode API endpoint shares and lists the endpoints of the
areas an integration usually needs. The other areas are indexed at the end with their path
prefixes.

Related pages:

- [Webhooks and events](webhooks.md): outbound deliveries, payloads and signatures.
- [Alerts](alerts.md): the inbound alert endpoint, sources and rules.
- [OAuth authorization server](oauth.md): connected applications and delegated tokens.
- [`sylvode` command line](cli.md) and the [MCP server](../../apps/mcp-server/README.md): the same
  data through a command line or an MCP client.
- [Bots and agent work](../guide/agents.md): what bot connections and bot tasks do.

## Base paths

The API is served by the `api` process. All paths below are relative to its address, for example
`https://sylvode.example.com`.

| Prefix | Content |
|---|---|
| `/api/v1/` | Almost every endpoint: accounts, workspaces, projects, work items, forms, governance, bots, webhooks, alerts, notifications, search, OAuth, and the first Flow endpoints. |
| `/api/v2/flow/` | The scope-based Flow endpoints (personal space and workspace Flow). |
| `/health`, `/ready` | Liveness and readiness. No authentication. |
| `/uploads/` | Downloads of uploaded files (also served under `/api/v1/uploads/`). |
| `/.well-known/oauth-authorization-server`, `/.well-known/oauth-protected-resource` | OAuth discovery documents, see [OAuth](oauth.md). |

Request and response bodies are JSON (`Content-Type: application/json`) unless an endpoint says
otherwise (file uploads use `multipart/form-data`). Identifiers are UUIDs. Timestamps are RFC 3339
strings. The server accepts cross-origin requests from any origin.

## Authentication

An endpoint that needs a caller reads one credential from the `Authorization: Bearer <token>`
header or, for a person signed in through the web UI, from the `access_token` cookie. The kind of
credential is recognised by its form:

| Credential | Form | Who holds it | Where it is accepted |
|---|---|---|---|
| User access token | JWT, returned by login | A person | Every endpoint that takes a caller. |
| Workspace bot token | `opr_...` | A bot of one workspace | Endpoints that accept bots; see below. |
| Delegated access token | `sva_...` | A connected application acting for a person | Only the endpoints on the delegated allow-list; see [OAuth](oauth.md). Header only, never a cookie or query parameter. |
| Inbound source token | `svi_...` | A security product posting alerts | Only `POST /api/v1/inbound/events`; see [Alerts](alerts.md). |

A missing, unknown, expired or disabled credential is refused with code 401. A credential that is
valid but not allowed for the request is refused with code 403 (see [Errors](#responses-and-errors)
for how codes are carried).

### User sessions

| Method | Path | Body | Notes |
|---|---|---|---|
| POST | `/api/v1/auth/register` | `{"email", "password", "name"}` | Password at least 8 characters. The first account of an empty instance becomes the instance administrator. Every later registration must be made by an instance administrator (their token in the request). |
| POST | `/api/v1/auth/login` | `{"email", "password"}` | Bot users cannot log in with a password. |
| POST | `/api/v1/auth/refresh` | `{"refresh_token"}` (optional) | Without a body field the `refresh_token` cookie is used. |
| POST | `/api/v1/auth/logout` | none | Clears both cookies. |
| GET | `/api/v1/auth/me` | none | The signed-in user. |
| PUT | `/api/v1/auth/me/profile` | `{"name", "email", "avatar_url"}` | |
| PUT | `/api/v1/auth/me/password` | `{"current_password", "new_password"}` | |
| GET, PUT | `/api/v1/auth/me/preferences` | `{"notification_prefs": {"email_notification", "mention_only", "daily_digest"}}` | |

Register, login and refresh answer `{"user": {...}, "tokens": {"access_token", "refresh_token",
"token_type": "Bearer", "access_expires_in", "refresh_expires_in"}}` and also set the
`access_token` and `refresh_token` cookies (`HttpOnly`, `SameSite=Lax`, and `Secure` unless the
server runs on a loopback address with insecure cookies allowed). Token lifetimes are set in the
`[auth]` section of the [configuration](../configuration.md).

Changing the password or deactivating the account ends every existing session of that user: older
access and refresh tokens are refused from then on.

```sh
curl -s https://sylvode.example.com/api/v1/auth/login \
  -H 'Content-Type: application/json' \
  -d '{"email":"admin@example.com","password":"change-me-now"}'

curl -s https://sylvode.example.com/api/v1/auth/me \
  -H "Authorization: Bearer $SYLVODE_TOKEN"
```

### Bot tokens

Workspace owners and admins create bots with `POST /api/v1/workspaces/{workspace_id}/bots`
(`{"name", "permissions", "transport_surface", "expires_at"}`). The answer carries the token once;
only its prefix is shown afterwards.

- `permissions` is a list of `read`, `write` and `admin` (default `["read"]`). `write` includes
  `read`; `admin` includes both. Requests with `GET`, `HEAD`, `OPTIONS` or `TRACE` need `read`;
  every other method needs `write`.
- `transport_surface` defaults to `rest`, which is the value for a token used directly against this
  API. A token registered for another surface is accepted only when the request declares that
  surface, which the Sylvode MCP server does for its own tokens.
- A bot made by a project bot connection reaches only the routes and projects its connections
  allow. See [Bots and agent work](../guide/agents.md).
- Instance administration (`/admin/users`, `/admin/stats` under `/api/v1/`) and a few other
  user-only areas refuse bot tokens.

Every request a bot token makes is recorded in the workspace's bot operation log
(`GET /api/v1/workspaces/{workspace_id}/bot-operation-logs`).

## Responses and errors

Every JSON response uses one envelope:

```json
{ "code": 0, "message": "success", "data": { } }
```

`code` is `0` on success. On an error, `code` carries an HTTP-style status number (400, 401, 403,
404, 409, 500) and `message` describes the problem:

```json
{ "code": 403, "message": "Workspace admin or owner required" }
```

**The HTTP status of an error is usually `200 OK`.** Most endpoints report errors only in the
envelope, so a client must check `code`, not only the HTTP status. The health endpoints are the
exception: `GET /ready` answers HTTP 503 when the database cannot be reached.

Some errors carry a stable machine-readable name and structured details next to the message:

```json
{
  "code": 400,
  "message": "limit_exceeded: workspace_subscribers (at most 100 active subscribers per workspace)",
  "data": null,
  "error_code": "limit_exceeded",
  "details": { "limit_kind": "workspace_subscribers", "limit": 100, "observed": 101 }
}
```

| `error_code` | `code` |
|---|---|
| `unauthenticated` | 401 |
| `forbidden`, `feature_disabled`, `policy_rejected` | 403 |
| `not_found` | 404 |
| `unsupported_protocol`, `invalid_update`, `limit_exceeded`, `checksum_mismatch`, `unsupported_format` | 400 |
| `stale_frontier`, `resync_required`, `authorization_churn`, `server_draining` | 409 |
| `unclassified`, `server_rejected` | 500 |

The Flow endpoints report database failures this way too. Branch on `error_code` when it is
present, otherwise on `code`.

These answers use a real HTTP error status:

| Case | HTTP status | Body |
|---|---|---|
| A write refused because the resource changed since it was read (`error_code: "version_conflict"`, for example replacing an alert rule with a stale `expected_version`) | 409 | Envelope with `code: 409` and `details` `{"reason", "action": "review_and_retry", "write_state": "not_accepted"}` |
| `POST /api/v1/inbound/events` | the status in `code` | Envelope; see [Alerts](alerts.md#responses) |
| `PUT /api/v1/ai/runner/report` over its rate or size limit | 429 (with `Retry-After`) or 413 | Envelope |
| OAuth endpoints and refusals of a delegated `sva_` token | 400, 401, 403, 404, 429 | OAuth error bodies; see [OAuth](oauth.md) |
| A request the framework cannot parse (malformed JSON, a path parameter that is not a UUID, a missing JSON content type) | 4xx | Plain text |
| An unknown path or method | 404 or 405 | Empty |

### Pagination

List endpoints that page take `page` (from 1) and `per_page` and answer:

```json
{ "items": [ ], "total": 42, "page": 1, "per_page": 20, "total_pages": 3 }
```

Defaults and maximums are given per endpoint below. Some list endpoints return a plain array or
their own shape; the tables say so.

## Health

| Method | Path | Notes |
|---|---|---|
| GET | `/health` | `{"status": "ok", "service": "<app_name>"}` in the envelope. |
| GET | `/ready` | `{"status": "ready"}` when the database answers, otherwise HTTP 503 with `code: 503` and the message `database not ready`. |

## Webhooks

Workspace webhooks deliver events to an HTTP endpoint. Payloads, signatures and delivery rules are
in [Webhooks and events](webhooks.md).

| Method | Path | Who | Notes |
|---|---|---|---|
| POST | `/api/v1/workspaces/{workspace_id}/webhooks` | Workspace owner or admin | Body `{"name", "url", "events", "active", "secret", "bot_user_id"}`. Answers the webhook with its `secret`, once. |
| GET | `/api/v1/workspaces/{workspace_id}/webhooks` | Workspace member | Paginated shape with every webhook in one page. Never contains the secret. |
| GET | `/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}` | Workspace member | |
| PATCH | `/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}` | Workspace owner or admin | Any of `name`, `url`, `events`, `active`, `secret`, `bot_user_id`. |
| DELETE | `/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}` | Workspace owner or admin | |
| POST | `/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/rotate-secret` | Workspace owner or admin | Generates a new secret and answers it, once. |
| GET | `/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries` | Workspace member | Delivery log, newest first. `per_page` default 20, maximum 100. |
| GET | `/api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries/{delivery_id}` | Workspace member | One delivery with request headers, response status and body. |

## Bot connections

A bot connection runs a bot's tasks for one project on a `sylvode-runner`. What a connection does
is described in [Bots and agent work](../guide/agents.md); the runner side is in the
[runner README](../../apps/runner/README.md).

Only workspace owners and admins manage connections. Other project members read a reduced view.
Bot tokens and delegated tokens are refused on every connection route.

| Method | Path | Body or query | Answer |
|---|---|---|---|
| GET | `/api/v1/projects/{project_id}/bot-connections` | | `{"connections": [{"connection", "status"}]}` ordered by bot name; members get `{"connections": [{"bot": {"id", "name"}, "enabled", "cli", "channel"}]}`. |
| POST | `/api/v1/projects/{project_id}/bot-connections` | See below | `{"connection", "status", "config_block"}` |
| GET | `/api/v1/projects/{project_id}/bot-connections/{bot_id}` | | `{"connection", "status"}`; members get `{"connection": {...}}` in the reduced view. 404 `project has no connection for this bot`. |
| PATCH | `/api/v1/projects/{project_id}/bot-connections/{bot_id}` | `{"expected_version", ...}` with any of `runner_url`, `cli`, `working_directory`, `channel`, `on_complete_state`, `on_fail_label_id` (`null` clears it), `governance` | `{"connection", "status"}` |
| POST | `/api/v1/projects/{project_id}/bot-connections/{bot_id}/disable` | `{"expected_version"}` | `{"connection", "status"}` |
| POST | `/api/v1/projects/{project_id}/bot-connections/{bot_id}/enable` | `{"expected_version"}` | `{"connection", "status"}` |
| POST | `/api/v1/projects/{project_id}/bot-connections/{bot_id}/rotate` | `{"expected_version"}` | `{"connection", "status", "config_block"}` |
| DELETE | `/api/v1/projects/{project_id}/bot-connections/{bot_id}` | `?expected_version=N` | `{"removed": true, "bot_still_connected_to": [...]}` |

The connect body:

```json
{
  "bot": { "name": "Build bot" },
  "runner_url": "https://runner.example.com:9090",
  "cli": "claude",
  "channel": "tmux",
  "working_directory": "/srv/projects/example",
  "on_complete_state": "auto",
  "on_fail_label_id": null,
  "governance": { "max_domain_level": "voter", "can_veto_human_consensus": false,
                  "reason_min_length": 50, "domain_overrides": {} }
}
```

- `bot` is exactly one of `{"name"}` (a new bot, at most 100 characters) or `{"id"}` (a bot that is
  already connected to another project of the workspace).
- `cli` is required: `claude`, `codex` or `opencode`. `channel` is `tmux` (default) or `headless`;
  only `claude` can use `tmux`. `working_directory` is required.
- `on_complete_state` is `auto` (default), `none` or a state key of the project's workflow.
- `runner_url` must pass the outbound address policy (`[outbound]` in the
  [configuration](../configuration.md)).
- `governance` is optional; `reason_min_length` is 0 to 2000.

`config_block` is a ready `sylvode-runner` configuration (`{"format", "filename", "bot_id",
"token_prefix", "issued_at", "text"}`). Its `text` holds the bot token and the webhook signing
secret. It is returned only by connect (for a new bot) and rotate, with `Cache-Control: no-store`,
and cannot be read again. Connecting a bot that already exists answers `config_block: null`.

Every write carries the connection's `version` as `expected_version`. A stale version changes
nothing and answers `code: 409`, `message: "connection changed; reload"` and `data: {"version",
"current"}` with the connection as it stands.

The bot, token and webhook a connection makes are shown with `managed_by` on the Bots and Webhooks
endpoints and cannot be edited there (409).

## AI tasks and the task protocol

Bot tasks arise from assignments, mentions, vote requests and alert rules. The protocol endpoints
are used by the bot, or by the runner acting for it, to read and end its tasks. How a task moves
through its states is described in [Bots and agent work](../guide/agents.md).

The endpoints under `/api/v1/ai/tasks/{task_id}` accept only the task's own bot token. Any other caller,
a person or administrator included, gets 404 `ai task not found`. Each needs the attempt number
`?attempt=N` of the task view and acts only while that attempt is active, so a late call never ends
a later attempt.

| Method | Path | Body | Notes |
|---|---|---|---|
| GET | `/api/v1/ai/protocol` | | Protocol version, API version, features, `runner_report_interval_secs` and the lease settings (`processing_timeout_secs`, `lease_extension_secs`, `max_processing_secs`). |
| GET | `/api/v1/ai/tasks/active` | | The calling bot's active tasks, oldest first. Bots only. |
| GET | `/api/v1/ai/tasks/{task_id}?attempt=N` | | The task view. A task that ended at attempt N answers with `"active": false`. |
| POST | `/api/v1/ai/tasks/{task_id}/ack?attempt=N` | `{"plan"}` (1 to 4000 characters) | The agent has the task. Answers `{"task", "proceed": true}`. |
| POST | `/api/v1/ai/tasks/{task_id}/progress?attempt=N` | `{"note", "percent", "kind", "queue_position"}` | Extends the lease. `note` at most 1000 characters, `percent` 0 to 100, `queue_position` 0 to 10000, `kind` one of `agent` (default), `runner.queued`, `runner.pushed`, `runner.alive`, `runner.reminded`, `runner.blocked`. Answers `{"lease_expires_at", "coalesced"}`. |
| POST | `/api/v1/ai/tasks/{task_id}/complete?attempt=N` | `{"outcome", "summary", "artifacts", "vote", "vote_reason"}` | `outcome` is `done`, `needs_review` or `no_change`; `summary` 1 to 20000 characters; at most 20 `artifacts` of `{"kind", "ref", "label"}` with `kind` `commit`, `branch`, `pull_request`, `file` or `url`; `vote` `yes`, `no` or `abstain`. Writes the reply where the task came from. |
| POST | `/api/v1/ai/tasks/{task_id}/fail?attempt=N` | `{"reason", "retryable", "source"}` | `reason` 1 to 2000 characters. `retryable` defaults to `false`, which fails the task at once; `true` re-queues it while attempts remain. `source` is `agent` (default) or `runner`. |
| PUT | `/api/v1/ai/runner/report` | Runner self-report (JSON object, at most 16 KiB) | Only the bot of a project bot connection. Rate limited (429 with `Retry-After`). Answers `{"accepted": true, "interval_secs"}`. |

Repeating `complete` or `fail` after the attempt has ended changes nothing: the same result or reason
answers `code: 400` (`task is not active for attempt N`), a different one answers `code: 409`.

Task lists for people:

| Method | Path | Who | Notes |
|---|---|---|---|
| GET | `/api/v1/projects/{project_id}/ai/tasks` | Project member | `status` filter (`pending`, `processing`, `acknowledged`, `completed`, `failed`, `cancelled`), `page`, `per_page` (default 20, maximum 100). |
| POST | `/api/v1/projects/{project_id}/ai/tasks` | Workspace owner or admin, or instance administrator | `{"ai_participant_id", "task_type", "reference_type", "reference_id", "priority", "payload", "idempotency_key", "max_attempts"}`; `task_type` is `issue_assigned`, `review_requested`, `comment_requested` or `vote_requested`; `reference_type` is `work_item`, `proposal` or `comment`. |
| GET | `/api/v1/issues/{issue_id}/ai-tasks` | Project member | The tasks of one work item. |

## Alerts

Security products post events to Sylvode, and alert rules turn them into work items and bot tasks.
The full description, the event format and the rule fields are in [Alerts](alerts.md).

| Method | Path | Who |
|---|---|---|
| POST | `/api/v1/inbound/events` | Inbound source token (`svi_`) only |
| GET, POST | `/api/v1/projects/{project_id}/inbound-sources` | Workspace owner or admin |
| GET, PATCH, DELETE | `/api/v1/inbound-sources/{id}` | Workspace owner or admin |
| POST | `/api/v1/inbound-sources/{id}/rotate` | Workspace owner or admin |
| GET | `/api/v1/inbound-sources/{id}/events` | Workspace owner or admin |
| GET, POST | `/api/v1/projects/{project_id}/alert-rules` | Workspace owner or admin |
| POST | `/api/v1/projects/{project_id}/alert-rules/reorder` | Workspace owner or admin |
| POST | `/api/v1/projects/{project_id}/alert-rules/test` | Workspace owner or admin |
| GET, PUT, DELETE | `/api/v1/alert-rules/{id}` | Workspace owner or admin |
| GET | `/api/v1/issues/{id}/alert` | Workspace member |

The management routes refuse bot tokens and delegated tokens.

## Notifications

Notifications belong to the caller; nobody reads or changes another user's notifications.

| Method | Path | Notes |
|---|---|---|
| GET | `/api/v1/notifications` | `page` (default 1), `per_page` or `limit` (default 20, maximum 100), `unread_only` (default `false`). Newest first. Answers the paginated shape plus `unread_count`. |
| GET | `/api/v1/notifications/unread-count` | `{"count"}` |
| PATCH or PUT | `/api/v1/notifications/{id}/read` | 404 when the notification does not exist or is already read. |
| PATCH or PUT | `/api/v1/notifications/read-all` | |
| DELETE | `/api/v1/notifications/{id}` | |

A notification has `id`, `user_id`, `type`, `title`, `content`, `link`, `related_issue_id`,
`related_comment_id`, `related_project_id`, `is_read`, `created_at`, `read_at` and `issue_title`.
`type` is one of:

| `type` | Sent when |
|---|---|
| `mentioned` | Someone mentions the user in a work item comment or in Flow. For a comment, `related_comment_id` is that comment. |
| `assignment` | A work item is assigned to the user by someone else, when it is created or later. Assigning yourself, or a bot, sends none. |
| `alert_opened`, `alert_escalated`, `alert_task_completed`, `alert_task_failed`, `alert_suppressed`, `alert_source_throttled` | Milestones of an alert; see [Alerts](alerts.md#notifications). |

`title` is a message key for the web UI (for example `notification.mentionedTitle`) and `content`
holds its parameters.

## Work items, sprints and labels

Work items, sprints and labels are reached with a user token, a bot token or, for the routes on the
connected-application allow-list, a connected application. Every route checks that the caller can
reach the workspace that owns the object: a non-member gets `404`.

### Work item fields

Every answer that describes a work item carries `identifier`, the short identifier
`{project key}-{sequence number}` such as `WEB-12`. This covers a single item (read, create, update,
by identifier), the project issue list, the board (`columns[].issues[]`), `/my/issues` and the
issue results of `/search`. The same value is the `key` of the issue object in
[webhook payloads](webhooks.md). The full item answers (single item, create, update, list and by
identifier) also carry `labels`, an array of `{id, name, color}` sorted by name that is empty when
the item has no label.

### Listing and updating work items

`GET /projects/{project_id}/issues` filters by `state`, `priority`, `assignee_id`, `label_ids`,
`search` and `sprint_id`, and sorts with `sort_by` and `sort_order`. `sprint_id` is a sprint id, or
`none` for the items that are in no sprint; any other value is `400`. A query parameter the route
does not know is refused with HTTP 400.

`PUT /issues/{id}` changes only the fields that are present in the body. For `assignee_id`,
`sprint_id` and `due_at`, `null` clears the value: `{"sprint_id": null}` moves the item back to the
backlog, `{"assignee_id": null}` unassigns it and `{"due_at": null}` removes the due date. `null` on
`title`, `description`, `state` or `priority` is `400` with the message `<field> cannot be null`. A
body that names a field the route does not know, here and on create, is refused with HTTP 422 before
anything is written. The activity log records an unassignment as `assigned` and a removal from a
sprint as `sprint_changed`, each with `"to": null`.

### Sprints

`GET /projects/{project_id}/sprints` lists the sprints of a project, `GET /sprints/{id}` reads one,
`POST /projects/{project_id}/sprints` creates one and `PUT /sprints/{id}` and `DELETE /sprints/{id}`
change or delete it. A sprint has `name`, `description`, `start_date` and `end_date` (`YYYY-MM-DD` or
`null`) and `status`.

- A new sprint is `planned` (the default) or `active`; any other status is `400`.
- `status` moves `planned` to `active`, `active` to `completed`, and `planned` or `active` to
  `cancelled`. Completed and cancelled sprints are final, and `active` back to `planned` is
  refused. Any other change answers `400 sprint status cannot change from <from> to <to>` with
  `error_code: "sprint_invalid_transition"` and `details: {from, to}`. Sending the current status is
  accepted and does nothing.
- A project has at most one active sprint. Starting or creating a second one answers `409
  project already has an active sprint: <name>` with `error_code: "sprint_active_exists"` and
  `details: {active_sprint_id, active_sprint_name}` (the message carries the name only).
- `end_date` must not be before `start_date`, also when only one of them is edited (`400`,
  `error_code: "sprint_end_before_start"`).
  `start_date` and `end_date` accept `null`, which clears them; `null` on `name`, `description` or
  `status` is `400`.
- Names are unique within a project ignoring case (`409`, `error_code: "sprint_name_taken"`).
- Members can create sprints and edit their name, description and dates. Creating an active sprint,
  changing a sprint's status and deleting a sprint need an owner or admin (`403`).
- Completing or cancelling a sprint moves its unfinished work items in the same transaction: to the
  backlog, or to a planned sprint of the same project named by `rollover_sprint_id` in the update
  (`400` when it is not another planned sprint of the project, and `400` when it is sent without
  `completed` or `cancelled`). A work item is finished when its state is terminal or its category is
  `done` or `completed`; finished items stay in the sprint. The answer has
  `rollover: {moved_count, target_sprint_id}` (`null` target means the backlog). Each moved item
  gets a `sprint_changed` activity.
- Deleting a sprint moves all its work items to the backlog the same way (with the activity), then
  deletes it.
- A body with a field the route does not know is refused with HTTP 422.
- Each sprint in `GET /projects/{project_id}/sprints` carries `progress: {total, done}`: the work
  items in the sprint and how many of them are finished (same rule as above). Other answers do not
  carry `progress`.

### Comments

`GET /comments/{id}` reads one comment: `{id, work_item_id, project_id, author_id, author_name,
content, created_at, updated_at}`. It needs access to the workspace of the comment's project, like
the list at `GET /issues/{issue_id}/comments`; an unknown id or a caller outside the workspace gets
`404`. The route tells a caller which project owns a comment id.

### Labels

Labels belong to a workspace (`/workspaces/{workspace_id}/labels`, `/labels/{id}`). A `color` is
`#rrggbb` (any case is accepted and stored lower-case; anything else is `400 color must be a hex
colour #rrggbb`) and defaults to `#6b7280`. Names are unique in the workspace ignoring case (`409`).
A body with an unknown field is refused with HTTP 422, and `null` on `name`, `color` or
`description` is `400`.

`POST /issues/{issue_id}/labels/{label_id}` and `POST /issues/{issue_id}/labels/batch` succeed when
a label is already attached, and `DELETE /issues/{issue_id}/labels/{label_id}` succeeds when it is
not; the `label.added` and `label.removed` events are sent only for a real change, once per label in
a batch. Deleting a label sends `label.removed` for each work item it was on.

### Workflow states

`POST /workflows/{workflow_id}/states` and `PUT /workflow-states/{state_id}` take an optional
`position`, the 0-based place of the state in the workflow. A position below 0 is `400 position
must be 0 or greater`. A position that is already taken puts the state there and moves the states
from that place on down by one (an update takes the state out of its old place first); a position
past the end, or none on create, appends. Positions stay consecutive and the request never fails on
a taken place. `PUT /workflows/{workflow_id}/states/reorder` still sets the whole order at once.

### Routes open to users and bots

These routes used to require a bot and answered a user token with HTTP 500. They now answer both:

| Method | Path | Notes |
|---|---|---|
| GET | `/projects/{project_id}/labels` | The labels of the project's workspace. |
| POST | `/issues/{issue_id}/labels/batch` | Body `{"label_ids": [...]}`. Every label must belong to the item's workspace; nothing is written when one does not. |
| GET | `/issues/by-identifier/{identifier}` | A work item by its short identifier such as `WEB-12`, the key matched ignoring case. A bot or a connected application searches its own workspace. A user searches the workspaces they belong to; add `?workspace_id=` when the same identifier exists in more than one, otherwise the answer is `409`. |

## Search

`GET /api/v1/search` searches work items, projects and comments that the caller can see: the
workspaces the user is a member of, or the bot's own workspace.

| Parameter | Notes |
|---|---|
| `q` | Required, not empty. Matched as a case-insensitive substring. |
| `type` | `issue`, `project` or `comment`; all three when absent. |
| `project_id` | Limits the search to one project. A project outside the caller's reach answers 404. |
| `limit` | Results per type, default 50, maximum 100. |

The answer is `{"query", "total", "results"}`. Each result has a `type` field and the fields of its
kind:

| `type` | Fields | Matched against | Order |
|---|---|---|---|
| `issue` | `id`, `title`, `description`, `state`, `project_id`, `workspace_id` | title, description | most recently updated first |
| `project` | `id`, `key`, `name`, `description`, `workspace_id` | name, key, description | most recently updated first |
| `comment` | `id`, `body`, `issue_id`, `project_id`, `workspace_id`, `author_id`, `created_at` | body | newest first |

```sh
curl -s "https://sylvode.example.com/api/v1/search?q=login&type=issue" \
  -H "Authorization: Bearer $SYLVODE_TOKEN"
```

## Project export and import

| Method | Path | Who | Notes |
|---|---|---|---|
| GET | `/api/v1/export/project/{id}` | Workspace member or a bot of the workspace | `format=json` (default) or `csv`. |
| POST | `/api/v1/workspaces/{workspace_id}/import/project` | Workspace owner or admin (or a bot with the `admin` permission) | Adds the work items and comments of an export document to an existing project. |

The export answers in the envelope with `{"format", "filename", "content"}`; `content` is the
exported text. For `json` it is this document, which is also what the import reads:

```json
{
  "format": "sylvode.project_export",
  "version": 1,
  "exported_at": "2026-11-23T10:00:00Z",
  "project": { "key": "QLB", "name": "QA loop", "description": "" },
  "issues": [
    { "identifier": "QLB-3", "title": "Fix login", "description": "Steps to reproduce",
      "state": "todo", "priority": "medium", "due_at": null,
      "created_at": "2026-11-20T09:00:00Z", "updated_at": "2026-11-21T09:00:00Z" }
  ],
  "comments": [
    { "issue_identifier": "QLB-3", "author_name": "Alex", "body": "Seen on staging",
      "created_at": "2026-11-21T09:30:00Z" }
  ]
}
```

Assignees, labels and sprints are not exported. For `csv` the content has the work items only, with
the columns `Identifier`, `Title`, `State`, `Priority`, `Due`, `Description`, `Created At` and
`Updated At`; every field is quoted and a quote inside a field is doubled.

The import body is `{"project_key": "QLB", "document": <the export document>}` and nothing else (an
unknown field is refused with HTTP 422).

- The project must exist in the workspace (`404 project <key> not found`). A caller who is not an
  owner or admin gets `403 only owners and admins can import`.
- The document must have `format` `sylvode.project_export` and `version` 1 (`400 unsupported export
  format; expected sylvode.project_export version 1`).
- Everything is checked before anything is written: each issue needs a title, an identifier that
  appears once, a `state` of the project's workflow, a `priority` of `low`, `medium`, `high` or
  `urgent` (default `medium`) and, if present, an RFC 3339 `due_at`; each comment needs an
  `issue_identifier` of an issue in the document, a text and an RFC 3339 `created_at`. When anything
  fails the answer is `400 import refused: N problems` with the problems in `details.errors`, and
  nothing is written.
- Otherwise one transaction adds the issues (new sequence numbers, you as creator, no assignee, no
  sprint, no labels) and the comments (you as author, original times). No webhook, notification,
  activity or agent task is created for them.

The answer is `{"project_id", "project_key", "issues_created", "comments_created", "identifiers"}`,
where `identifiers` maps each identifier in the document to the one it got, such as
`{"QLB-3": "DST-41"}`.

## Uploads

| Method | Path | Notes |
|---|---|---|
| POST | `/api/v1/upload` | `multipart/form-data` with the file in a part named `file` (the first part). At most 50 MiB. Accepted types: png, jpg, gif, webp, mp4, webm, mov, avi, zip, gz, tar.gz, log, txt, pdf, json, csv, xml. Answers `{"url", "filename", "storage_backend", "object_key"}`, plus `thumbnail_url` and `thumbnail_object_key` for images. |
| GET | `/api/v1/uploads/{file_name}` | The file. Also `/api/v1/uploads/thumbnails/{file_name}`, `/api/v1/uploads/previews/{file_name}`, `/api/v1/uploads/variants/{file_name}` and the same paths under `/uploads/`. |
| GET | `/api/v1/uploads/signatures/{file_name}` | A form signature image. |
| POST | `/api/v1/uploads/signatures/{file_name}/signed-url` | `{"url", "expires_at"}`: a download link valid for 10 minutes. |

A download needs a session or bot token of a member of the workspace that owns the file, or a
signed link (`?expires=...&signature=...`), which works without a session so that images render in
the browser. A caller outside the owning workspace gets 404 `file not found`. The whole request body of any
endpoint is limited to 200 MiB.

## OAuth and connected applications

The OAuth endpoints (`/api/v1/oauth/token`, `/api/v1/oauth/revoke`,
`/api/v1/oauth/authorization-requests`, `/api/v1/oauth/events/stream`,
`/api/v1/oauth/connection`), the connected-apps endpoints (`/api/v1/me/connected-apps`,
`/api/v1/workspaces/{workspace_id}/connected-apps`, `/api/v1/admin/oauth-clients`) and the discovery
documents are described in [OAuth](oauth.md). They use OAuth error bodies and real HTTP statuses.
On a server without an `[oauth]` section the connected-apps endpoints answer `error_code:
"feature_disabled"` (`connected applications are not enabled on this server`).

## Other areas

The remaining endpoints follow the conventions above. Their paths, relative to `/api/v1/`:

| Area | Path prefixes |
|---|---|
| Instance administration | `/admin/users`, `/admin/stats` (instance administrator, user token only); Flow operations under `/admin/flow/...` and `/admin/workspaces/{workspace_id}/flow/...` |
| Workspaces and members | `/workspaces`, `/workspaces/{workspace_id}/members`, `/workspaces/{workspace_id}/users`, `/workspaces/{workspace_id}/bots`, `/workspaces/{workspace_id}/activities` |
| Projects and work items | `/workspaces/{workspace_id}/projects`, `/projects/{id}`, `/projects/{project_id}/issues`, `/issues/{id}`, `/issues/{issue_id}/comments`, `/comments/{id}`, `/projects/{project_id}/board`, `/projects/{project_id}/sprints`, `/sprints/{id}`, `/workspaces/{workspace_id}/labels`, `/labels/{id}`, `/my/issues`, `/my/activities` |
| Workflows | `/workspaces/{workspace_id}/workflows`, `/workflows/{workflow_id}`, `/workflow-states/{state_id}`, `/projects/{project_id}/workflow` |
| Project types, templates and context | `/project-types`, `/scenario-templates`, `/projects/{project_id}/resources`, `/projects/{project_id}/context`, `/projects/{project_id}/release-readiness`, `/projects/{project_id}/check-results`, `/check-results/{check_result_id}` |
| Forms | `/projects/{project_id}/forms`, `/forms/{form_id}`, `/form-records/{record_id}`, `/form-views/{view_id}`, `/form-attachments/{attachment_id}`, `/form-export-jobs/{job_id}`, `/form-import-jobs/{job_id}`, `/form-attachment-package-jobs/{job_id}`, `/form-import-mapping-templates/{template_id}`; see the [forms guide](../guide/forms.md); `GET /form-attachments/{attachment_id}` reads one attachment (archived ones too) with its `form_id` and `project_id` |
| Plugins | `/projects/{project_id}/plugins`, `/plugins/{plugin_id}`, `/projects/{project_id}/plugin-packages`, `/me/form-plugin-packages`; see [plugins](../plugins/README.md) |
| Governance | `/proposals`, `/proposal-comments/{id}`, `/proposal-templates`, `/decisions`, `/decision-domains`, `/projects/{project_id}/domains`, `/governance/config`, `/governance/audit-logs`, `/trust-scores`, `/users/{id}/trust`, `/impact-reviews`, `/vetoers`, `/projects/{pid}/audit-reports`, `/ai-learning/{review_id}/feedback`, `/projects/{project_id}/ai-participants/{bot_id}/learning`; see the [governance guide](../guide/governance.md) |
| Flow | `/workspaces/{workspace_id}/flow/...`, `/workspaces/{workspace_id}/features/flow`, `/flow/...`, `/collab/...`, and `/api/v2/flow/`; see the [Flow guide](../guide/flow.md) and the Flow command groups of the [`sylvode` command line](cli.md) |
