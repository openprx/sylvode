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
| GET | `/api/v1/auth/setup-status` | none | No credentials needed. `{"needs_setup": true}` while the instance has no human account yet, so a first-run page can be offered; `false` afterwards. |
| POST | `/api/v1/auth/register` | `{"email", "password", "name"}` | Password at least 8 characters. The first account of an empty instance becomes the instance administrator. Every later registration must be made by an instance administrator (their token in the request). |
| POST | `/api/v1/auth/login` | `{"email", "password"}` | Bot users cannot log in with a password. |
| POST | `/api/v1/auth/refresh` | `{"refresh_token"}` (optional) | Without a body field the `refresh_token` cookie is used. |
| POST | `/api/v1/auth/logout` | none | Clears both cookies. |
| GET | `/api/v1/auth/me` | none | The signed-in user: `{"user", "bot"}`. `bot` is `null` for a person; for a bot token it is `{"bot_id", "workspace_id", "permissions", "project_scope"}` (`permissions` as the token acts: a project-scoped bot's `admin` shows as `write`). |
| PUT | `/api/v1/auth/me/profile` | `{"name", "email", "avatar_url"}` | |
| PUT | `/api/v1/auth/me/password` | `{"current_password", "new_password"}` | |
| GET, PUT | `/api/v1/auth/me/preferences` | `{"notification_prefs": {"email_notification", "mention_only", "daily_digest", "language"}}` | `language` is `en` or `zh` (default `en`), the language of e-mails; an unknown key or language is refused. Both methods answer `{"notification_prefs", "email_available"}`; `email_available` is `false` when the server has no e-mail delivery. |

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
- A bot is workspace-wide (its permissions apply to every project) or project-scoped: a level per
  project, `read`, `comment` or `write`, and a default level (`none`, `read` or `comment`) for every
  other project. `GET` and `PUT /api/v1/workspaces/{workspace_id}/bots/{bot_id}/project-access`
  (owners and admins) read and replace it as `{"project_scope": "workspace" | "projects",
  "default_level", "grants": [{"project_id", "level"}]}` in one transaction; a workspace-wide bot
  carries no levels (`default_level` `none`, no grants). A project outside the workspace is a 400;
  a bot made by a project bot connection is a 409. `read` lets a safe method through,
  `comment` also the comment routes (creating, editing, deleting and resolving comments), `write` every
  other route. The token's permissions stay a ceiling: a token without `write` never changes
  anything, so a bot that comments needs `write` in its permissions and a `comment` level. A
  project-scoped bot is never an administrator and never reaches administration routes. Workspace-wide
  listings are open for safe methods only when the default level is at least `read`.
  Four workspace-level reads answer a scoped bot with the projects it can read and nothing else: the
  project list (`GET /api/v1/workspaces/{workspace_id}/projects`), `GET /api/v1/search` (with or
  without `project_id`), `GET /api/v1/my/issues` and `GET /api/v1/my/activities`. Rows, counts and
  page totals cover only those projects; for a bot made by a project bot connection they are the
  projects where it has an enabled connection, for a project-scoped bot those whose level is at least
  `read`. A `project_id` outside them answers 404 `project not found or access denied`. Every
  other workspace-level route stays refused for such a bot. The bot list
  (`GET .../bots`) shows `project_scope`, `default_project_level` and `grant_count`. Every
  refusal answers 403 `this bot may not do this in this project`.
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
| `POST /api/v1/auth/login`, `/register` and `/refresh`, and `GET /api/v1/auth/setup-status`, over their per-address limit (login also per account) | 429 (with `Retry-After`) | Envelope |
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
| GET | `/api/v1/ai/tasks/{task_id}[?attempt=N]` | | The task view. Without `attempt`: the current attempt, in any status. With `attempt=N`: a task that ended at attempt N answers with `"active": false`; another attempt or state is a 400. |
| GET | `/api/v1/bot/change-hints` | | Server-sent events for the calling bot token (a person or a connected application gets 403). `hello` first (`bot_id`, `workspace_id`), then `task` (`id`) and `inbox` for the bot's own tasks, `issue` (`id`) for work items and comments of projects the bot can read, `tools` when the tool set may have changed, `access_changed` when the projects it can read changed; events carry ids only, read the object with the normal endpoints. The stream ends with `revoked` (token revoked, rotated, disabled or expired; `{"reason":"expired"}` at expiry), `lagged` (re-read everything) or `shutdown`. A comment line every 15 seconds keeps it open. 64 streams per bot and `[api] max_change_hint_streams` per process; beyond that `429` with `Retry-After`. |
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
| GET | `/api/v1/issues/{issue_id}/ai-tasks` | Project member | The tasks of one work item, newest first, paged (`page`, `per_page`; default 5, maximum 50); each task carries its newest 100 events. |

## Alerts

Security products post events to Sylvode, and alert rules turn them into work items and bot tasks.
The full description, the event format and the rule fields are in [Alerts](alerts.md).

| Method | Path | Who |
|---|---|---|
| POST | `/api/v1/inbound/events` | Inbound source token (`svi_`) only |
| GET, POST | `/api/v1/projects/{project_id}/inbound-sources` | Workspace owner or admin (the list answers `{sources, inbound_enabled, endpoint_url}`) |
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
| `issue_state_changed` | Someone else changes the state of a work item assigned to the user. `content` is `issue_state_changed:<title>\|<old>\|<new>`. Not sent when the same request also assigns the item to the user (the assignment shows it). |
| `issue_priority_changed` | The same for the priority: `issue_priority_changed:<title>\|<old>\|<new>`. |
| `alert_opened`, `alert_escalated`, `alert_task_completed`, `alert_task_failed`, `alert_suppressed`, `alert_source_throttled` | Milestones of an alert; see [Alerts](alerts.md#notifications). |

`title` is a message key for the web UI (for example `notification.mentionedTitle`) and `content`
holds its parameters.

A person's preferences apply when a notification is created. With `mention_only` only `mentioned`
and the alert types are created; the other types are not created at all. When the server has e-mail
delivery (`[email]` in [configuration.md](../configuration.md#email)), a human user with
`email_notification` also gets an e-mail for each notification, or, with `daily_digest`, one e-mail
a day. Bots never get e-mail.

`POST /api/v1/admin/email/test` (instance administrators) queues a test e-mail to the caller's own
address and answers `{"queued": true}`; it answers 409 with `error_code` `feature_disabled` when
e-mail delivery is not set up. The answer means queued, not delivered.

## Work items, sprints and labels

Work items, sprints and labels are reached with a user token, a bot token or, for the routes on the
connected-application allow-list, a connected application. Every route checks that the caller can
reach the workspace that owns the object: a non-member gets `404`.

### Work item fields

Every answer that describes a work item carries `identifier`, the short identifier
`{project key}-{sequence number}` such as `WEB-12`. This covers a single item (read, create, update,
by identifier), the project issue list, the board (`columns[].issues[]`), `/my/issues`, the entries
of `/my/activities`, the linked work items of a proposal (`/proposals/{id}/issues`) and the
issue results of `/search`. The same value is the `key` of the issue object in
[webhook payloads](webhooks.md). The full item answers (single item, create, update, list and by
identifier) also carry `labels`, an array of `{id, name, color}` sorted by name that is empty when
the item has no label. They also carry `milestone_id`, the id of the item's [milestone](#milestones)
or `null`. A single item (read, create, update, by identifier) carries `page_links`, its
[linked Flow pages](#linked-flow-pages); the list answers carry `page_link_count` instead.

### Listing and updating work items

`GET /projects/{project_id}/issues` filters by `state`, `priority`, `assignee_id`, `label_ids`,
`search`, `sprint_id` and `milestone_id`, and sorts with `sort_by` and `sort_order`. `sprint_id` is a
sprint id, or `none` for the items that are in no sprint; `milestone_id` works the same way for
[milestones](#milestones); any other value is `400`. A query parameter the route does not know is
refused with HTTP 400.

`PUT /issues/{id}` changes only the fields that are present in the body. For `assignee_id`,
`sprint_id`, `milestone_id` and `due_at`, `null` clears the value: `{"sprint_id": null}` moves the
item back to the backlog, `{"assignee_id": null}` unassigns it, `{"milestone_id": null}` takes it
out of its milestone and `{"due_at": null}` removes the due date. `null` on
`title`, `description`, `state` or `priority` is `400` with the message `<field> cannot be null`. A
body that names a field the route does not know, here and on create, is refused with HTTP 422 before
anything is written. The activity log records an unassignment as `assigned`, a removal from a
sprint as `sprint_changed` and a removal from a milestone as `milestone_changed`, each with
`"to": null`.

### Sprints

`GET /projects/{project_id}/sprints` lists the sprints of a project, `GET /sprints/{id}` reads one,
`POST /projects/{project_id}/sprints` creates one and `PUT /sprints/{id}` and `DELETE /sprints/{id}`
change or delete it. A sprint has `name`, `description`, `start_date` and `end_date` (`YYYY-MM-DD` or
`null`) and `status`.

- Sprints send the webhook events `sprint.created`, `sprint.updated`, `sprint.started`,
  `sprint.completed`, `sprint.cancelled` and `sprint.deleted` (see [Webhooks and events](webhooks.md#events)).
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

### Milestones

A milestone belongs to a workspace and spans its projects ("Beta"); a work item is in at most one.
A milestone has `name` (1 to 100 characters, unique in the workspace ignoring case), `description`,
`target_date` (`YYYY-MM-DD` or `null`) and `status`, `open` or `closed`.

| Method | Path | Who | Notes |
|---|---|---|---|
| GET | `/workspaces/{workspace_id}/milestones` | members | Optional `?status=open` or `closed` (any other value is `400`), `page` (default 1) and `per_page` (default 15, at most 100; larger values are cut to 100). Ordered by target date (none last), name, id. Unknown query parameters are `400`. |
| POST | `/workspaces/{workspace_id}/milestones` | owners and admins | Body `{name, description?, target_date?}`. A new milestone is `open`. |
| GET | `/milestones/{id}` | members | The milestone plus its work items. |
| PUT | `/milestones/{id}` | owners and admins | Any of `name`, `description`, `target_date` (`null` clears it) and `status`. `null` on another field is `400`; a body with no field is `400`. |
| DELETE | `/milestones/{id}` | owners and admins | Its work items lose the milestone. |

Every milestone answer carries `id`, `workspace_id`, `name`, `description`, `target_date`, `status`,
`created_at`, `updated_at` and `progress: {total, done, by_project: [{project_id, project_key,
total, done}]}`. The list answers `{items, total, page, per_page, total_pages}` like the other lists, with `progress` for the returned page only. A work item is done
when its state is finished in its own project's workflow (a terminal state, or one in the `done` or
`completed` category). `GET /milestones/{id}` adds `items_by_project: [{project_id, project_key,
items: [{id, identifier, title, state, done}]}]`, ordered by project key and item number, at most
500 items in all, and `truncated: true` when there were more; `progress` always counts every item.

- A taken name is `409` with `error_code: "milestone_name_taken"`. A body with a field the route
  does not know is refused with HTTP 422; a name that is empty after trimming or longer than 100
  characters, and a `target_date` that is not a calendar date, are `400`.
- Members cannot create, change or delete milestones (`403`); they read them, and set an item's
  milestone with `milestone_id` on create and update when they may edit the item. The milestone
  must exist in the item's workspace and be `open`, otherwise `400 milestone must be an open
  milestone of this workspace` (the same answer for a missing, foreign and closed milestone). An
  item that already is in a closed milestone keeps it, and may be edited with the same value or
  moved out of it.
- Changing an item's milestone records a `milestone_changed` activity (`{from, to}`) and sends
  `issue.updated` with `changes.milestone_id: {old, new}`. Issue payloads carry `milestone_id`.
- Deleting a milestone clears it from its items in one transaction, records `milestone_changed`
  (`to: null`) on each, and sends `issue.updated` for each item after the commit.
- Runner and project-scoped bot tokens cannot use the milestone routes, and connected applications
  cannot reach them.

### Linked Flow pages

A work item can link to Flow pages: the spec page of a feature, the notes of a meeting. A link is a
pair (work item, page); the page title is never stored with it but read when the link is shown, so a
rename shows at once. A page can be linked to many items and an item to at most 50 pages.

| Method | Path | Who | Answer |
|---|---|---|---|
| GET | `/issues/{issue_id}/pages` | members | The links, oldest first: `[{object_id, title, access, lifecycle, linked_at}]`. |
| POST | `/issues/{issue_id}/pages` | members | Body `{object_id}` (any other field is `422`). The link, in the same shape. |
| DELETE | `/issues/{issue_id}/pages/{object_id}` | members | `{}`. |
| GET | `/api/v2/flow/objects/{object_id}/work-items` | who can read the page | `{items: [{id, identifier, title, state, project_id}], hidden_count, truncated}`. |

- Linking needs the same right as editing the item and at least `view` on the page. Unlinking needs
  the right on the item only: someone who can edit the item can drop a link to a page they can no
  longer read.
- The page must be a Flow `page` of the item's workspace, or a personal page that its owner has placed
  into that workspace; any other readable page is `400 page is not available in this workspace` and a
  navigator or other object is `400 only a Flow page can be linked`. A page the caller cannot read, a
  missing one and one of another workspace are all `404 page not found`: the route never says which.
  More than 50 links is `400`.
- Linking a pair that is already linked, and unlinking a pair that is not, change nothing, send no
  event and answer as if they had just worked.
- What a link shows depends on the reader. `access` is `visible` when the caller may read the page and
  `restricted` when not; only a visible link has a `title` (`null` otherwise, never an empty string).
  `lifecycle` is `active`, or `archived` while the page is in the trash. A page in the trash keeps its
  links; restoring it shows it as active again; purging the page, or deleting the item, removes the
  links (no activity entry for a purge). A connected application sees every link as `restricted`.
- `GET /api/v2/flow/objects/{object_id}/work-items` lists the items linked to a page that the caller
  may read; the others are only counted in `hidden_count`. A person reads the items of every workspace
  they belong to, a project-scoped bot those of projects where its level is at least `read`.
  `truncated` is `true` when the page has more than 500 links and only the first 500 were examined. A
  page the caller cannot read is `404`.
- The activity log of the item records `page_linked` and `page_unlinked` with `{object_id}`. The
  activity answer adds `detail.page` to those entries, resolved for the reader: `{title, access,
  lifecycle}` with `access` `visible`, `restricted` or `removed` (the page was purged); `title` is
  present only when `visible`.
- A project-scoped bot needs `read` in the item's project to list the links and `write` to link or
  unlink; its own Flow rights decide which titles it sees. A connected application can list the links
  of an item with `work_items:read`.
- A change of the links sends `issue.updated` with `changes.page_links: {added: [object_id],
  removed: [object_id]}`; see [webhooks](webhooks.md).

### Comments and mentions

`GET /comments/{id}` reads one comment: `{id, work_item_id, project_id, author_id, author_name,
author, mentions, content, created_at, updated_at}`. It needs access to the workspace of the
comment's project, like the list at `GET /issues/{issue_id}/comments`; an unknown id or a caller
outside the workspace gets `404`. The route tells a caller which project owns a comment id.

Every comment answer (create, edit, read, list; the list rows use `body` where the single read uses
`content`) carries two people fields:

- `author`: `{id, name, is_bot, state}`, or `null` when the account is gone and no author id is left.
- `mentions`: the people the comment mentions, in the order stored, each `{id, name, is_bot, state,
  label}`. `label` is the text stored when the mention was made; a client highlights `@label` as well
  as `@name`, so a mention still shows after the person was renamed.

`state` is `active` while the person is a member of the workspace and `removed` otherwise. `name` is
the person's current name; for someone who has left it is the name the mention was written with (a
connected application never sees the current name of a person who has left), and `null` when there
is none. A name is never an id: a client shows `name`, a badge for `is_bot`, and its own wording for
`removed` without a name.

`POST /issues/{issue_id}/comments` and `PUT /comments/{id}` take `body` (or `content`) and an
optional `mentions`, a list of user ids picked in a composer. Names typed in the text are resolved
by the server: `@Name` mentions the workspace member (human or bot) with exactly that name, ignoring
case, longest name first (`@Ann Lee` before `@Ann`). The `@` must start the text or follow a
character that is not a letter or digit (an e-mail address is not a mention) and, after a name that
ends in a letter or digit, the next character must not continue the word. A name two members share
mentions nobody: pick the person with `mentions`. Ids that are not workspace members are ignored,
and a comment mentions at most 20 people (`400` beyond that). On `PUT` the mentions are derived
again from the new text and `mentions`, and only people who were not mentioned before are notified.
The author is shown among the mentions when named but is never notified.

A mention notifies the person (`mentioned`, `content` `mentioned_by:<name of the author>`; the name
is empty when the author has none, and the client shows "Someone"; the e-mail says "Someone
mentioned you"). Mentioning a bot of the project starts its task as before.

Comments on a proposal (`POST /proposals/{id}/comments`, which also takes `mentions`, and the list)
and on a Flow page (v1 and v2 comment routes) work the same way: `author` and `mentions` in the
answers, typed `@Name` resolved by the server, `mentioned` notifications. For a proposal the people
are the members of its workspace and the notification link is `/proposals/{id}`. For a Flow page
they are the members of the workspace and the guests of the page (a personal page: the people who
already take part in it); picked ids are accepted as before, and `state` is `active` for an enabled
account. A Flow comment's `mentions` are the same person objects (they were ids before), and its
`resolved_by` is a person `{id, name, is_bot, state}` too, `null` while the comment is open.

The comments of a work item (`GET /issues/{issue_id}/comments`) and its activity (`GET /issues/{issue_id}/activities`) are listed newest first and paged with `page` and `per_page` (comments: default 20, activity: default 30, maximum 100 each). The activity records `created`, `title_changed`, `description_changed`, `status_changed`, `priority_changed`, `assigned`, `due_date_changed`, `sprint_changed`, `milestone_changed`, `label_added`, `label_removed`, `page_linked`, `page_unlinked`, `comment_added`, `comment_edited` and `comment_deleted`, one entry for each change of the item. The activity answer adds `people` to the paged answer:
an object keyed by user id with `{id, name, is_bot, state}` for every person an entry names (its
actor, and the assignee before and after in `assigned` entries). The activity feed of the caller
across workspaces (`GET /my/activities`) answers the same way: each entry's actor and `assigned`
from/to people are in `people`, read against the workspace of the entry that names them.

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
| GET | `/api/v1/projects/{project_id}/export` | Workspace member or a bot of the workspace | `format=json` (default) or `csv`. Answers the file. |
| POST | `/api/v1/workspaces/{workspace_id}/import/project` | Workspace owner or admin (or a bot with the `admin` permission) | Adds the work items and comments of an export document to an existing project. |

The export answers the file itself, not an envelope: `Content-Type` is `application/json;
charset=utf-8` or `text/csv; charset=utf-8`, `Content-Disposition` is `attachment;
filename="project_<KEY>_export.<json|csv>"` and `Cache-Control` is `no-store`. Errors (an unknown
format, a missing project, no access) are still the usual JSON envelope. For `json` the body is this
document, which is also what the import reads, so a downloaded file can be uploaded as it is:

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

Assignees, labels and sprints are not exported. For `csv` the body has the work items only, with
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
| Project types, templates and context | `/project-types`, `/project-templates/preview`, `/projects/{project_id}/template-install`, `/projects/{project_id}/resources`, `/projects/{project_id}/context`, `/projects/{project_id}/release-readiness`, `/projects/{project_id}/check-results`, `/check-results/{check_result_id}` |
| Forms | `/projects/{project_id}/forms`, `/forms/{form_id}`, `/form-records/{record_id}`, `/form-views/{view_id}`, `/form-attachments/{attachment_id}`, `/form-export-jobs/{job_id}`, `/form-import-jobs/{job_id}`, `/form-attachment-package-jobs/{job_id}`, `/form-import-mapping-templates/{template_id}`; see the [forms guide](../guide/forms.md); `GET /form-attachments/{attachment_id}` reads one attachment (archived ones too) with its `form_id` and `project_id`; a delegated token with `forms:read` or `forms:write` reaches the form list, one form, its records, one record, and creating and updating a record, and nothing else of forms (see [OAuth](oauth.md#calling-the-api-with-a-delegated-token)) |
| Plugins | `/projects/{project_id}/plugins`, `/plugins/{plugin_id}`, `/projects/{project_id}/plugin-packages`, `/me/form-plugin-packages`; see [plugins](../plugins/README.md) |
| Governance | `/proposals`, `/proposal-comments/{id}`, `/proposal-templates`, `/decisions`, `/decision-domains`, `/projects/{project_id}/domains`, `/governance/config`, `/governance/audit-logs`, `/trust-scores`, `/users/{id}/trust`, `/impact-reviews`, `/vetoers`, `/projects/{pid}/audit-reports`, `/ai-learning/{review_id}/feedback`, `/projects/{project_id}/ai-participants/{bot_id}/learning`; see the [governance guide](../guide/governance.md) |
| Flow | `/workspaces/{workspace_id}/features/flow` under `/api/v1/`; every other Flow route is under `/api/v2/flow/`, and the Flow operations of the first row; see the [Flow guide](../guide/flow.md), the collaboration section below and the Flow command groups of the [`sylvode` command line](cli.md) |

## Flow collaboration

Flow has one route surface, `/api/v2/flow/`. A workspace is reached through the scope bound to it:
`GET /api/v2/flow/workspaces/{workspace_id}/scope` answers `{scope: {id, ...}, roots}`, and the workspace
level routes (`/scopes/{scope_id}/objects`, `/search`, `/trash`, `/navigator`, `/projection-lag`,
`/exports`, `/import-artifacts`, `/imports/...`) hang off that scope id. Object routes take the object
id directly, for personal and workspace content alike.

A workspace bot token is accepted on the routes that serve workspace content, with the bot's own
permissions and the workspace's Flow switch; it never reaches a personal scope, and the routes that
only a person can use (personal home, collaboration tickets, bootstrap, mounts, block shares, body
assets, child reservations, operations, packages, transfers) refuse it.

Live collaboration needs a user access token, never a bot token:

- `POST /api/v2/flow/collab/tickets` with `{object_id, document_id, client_id, origin}` returns a
  one-time ticket (valid for 60 seconds) and the WebSocket path. `client_id` must be a UUID that the
  client generates; any other value is refused (`400`/`422`). `origin` must be allowed by the server's
  collaboration origins. The caller needs at least view access to the object.
- `GET /api/v2/flow/collab/ws?ticket=...&client_id=...` upgrades to the collaboration WebSocket. It
  takes no bearer credential: the ticket is the credential, it is consumed by the upgrade, and it is
  bound to the same `client_id` (a UUID) and `Origin` it was issued for. A `client_id` that is not a
  UUID, a used, expired or foreign ticket, and a disabled workspace are all refused before the upgrade.
- `GET /api/v2/flow/objects/{object_id}/collab` reads the document diagnostics and
  `POST /api/v2/flow/objects/{object_id}/collab/verify` runs the shallow integrity check; both accept a
  user or a workspace bot token.
