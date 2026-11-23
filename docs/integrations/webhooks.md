# Webhooks and events

Sylvode tells other systems about changes in two ways:

- **Webhooks** push selected work item, comment, label, sprint and AI task events to an HTTP
  endpoint as they happen.
- **The business event ledger** records business writes. It is read on demand; nothing is pushed
  from it.

The endpoints are listed in the [API reference](api.md#webhooks); the conventions (authentication,
envelope, errors) are explained there as well.

## Managing webhooks

A webhook belongs to a workspace. Workspace owners and admins create, change, rotate and delete
webhooks; every workspace member can list them and read their delivery log. The web UI offers the
same in the workspace settings.

```sh
curl -s https://sylvode.example.com/api/v1/workspaces/$WORKSPACE_ID/webhooks \
  -H "Authorization: Bearer $SYLVODE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "Build notifications",
    "url": "https://hooks.example.com/sylvode",
    "events": ["issue.created", "issue.state_changed", "comment.created"],
    "active": true
  }'
```

| Field | Notes |
|---|---|
| `url` | Required. Checked against the [outbound address policy](#outbound-address-policy) when the webhook is saved. |
| `events` | Required on create. Each entry must be one of the [events](#events); anything else is refused with `Invalid event type`. |
| `name` | Optional; defaults to the URL. |
| `active` | Optional, default `true`. Only active webhooks receive deliveries. |
| `secret` | Optional signing key. When it is absent the server generates a 64-character secret. |
| `bot_user_id` | Optional. Names the bot whose tasks this webhook carries (see [Bot context](#bot-context)). Must be a bot user. |

The answer to create contains the webhook and its `secret`. This is the only time the secret is
returned: list, get and update never include it, so store it in the receiver's configuration at
once. `POST /api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/rotate-secret` replaces the secret with a new generated one
and returns it, once; deliveries are signed with the new secret from that moment.
`sylvode-runner` accepts several secrets at the same time (`webhook_secrets`), so add the new
secret to the runner before rotating and remove the old one afterwards.

A workspace can have at most 100 active webhooks. Creating or switching on one more answers
`error_code: "limit_exceeded"` with `details.limit_kind: "workspace_subscribers"`.

A webhook that a project bot connection created is shown with `managed_by` and cannot be changed,
rotated or deleted through these endpoints (409 `managed by a project bot connection; change it in
the project's Execution settings`). It is managed through the connection; see
[Bots and agent work](../guide/agents.md).

## Events

A webhook can subscribe to these events:

| Event | Sent when | `data` |
|---|---|---|
| `issue.created` | A work item is created. | `issue` |
| `issue.updated` | A work item's fields change. | `issue`, `changes` |
| `issue.assigned` | A work item's assignees change. | `issue`, `changes.assignee_ids` (`old`, `new`) |
| `issue.state_changed` | A work item moves to another state. | `issue`, `changes.state` (`old`, `new`) |
| `issue.deleted` | A work item is deleted. | `issue` |
| `comment.created` | A comment is added to a work item. | `comment`, `issue`, `mentions` (user ids) |
| `comment.updated` | A comment is edited. | `comment`, `issue` |
| `comment.deleted` | A comment is deleted. | `comment`, `issue` |
| `label.added` | A label is added to a work item. | `issue` (`id`, `key`, `title`), `label` (`id`, `name`, `color`) |
| `label.removed` | A label is removed from a work item. | `issue`, `label` |
| `sprint.started` | A sprint starts. | `sprint` (`id`, `name`, `description`, `status`, `start_date`, `end_date`, timestamps) |
| `sprint.completed` | A sprint is completed. | `sprint` |
| `ai.task_completed` | A bot task completes. | `task_id`, `task_type`, `status`, `reference_type`, `reference_id`, `result` |
| `ai.task_failed` | A bot task fails for good. | `task_id`, `task_type`, `status`, `reference_type`, `reference_id`, `error_message` |

An `issue` object has `id`, `key`, `title`, `description`, `state`, `priority`, `assignee_ids`,
`label_ids`, `sprint_id`, `created_at` and `updated_at`. A `comment` object has `id`, `issue_id`,
`content`, `author_id`, `author_name`, `created_at` and `updated_at`; the `issue` next to it has
`id`, `key` and `title`. In these payloads `key` is the project key followed by the first eight
characters of the work item id (for example `WEB-5B0C4F0E`), not the work item's sequential
identifier.

An alert rule that opens a work item also sends `issue.created` for it (see [Alerts](alerts.md)).

## Deliveries

Each event is sent once to every active webhook of the workspace that subscribes to it, as an HTTP
`POST` with a JSON body:

```json
{
  "id": "5b0c4f0e-2a7e-4f43-9a51-0e4f5c3d9a10",
  "event": "issue.state_changed",
  "timestamp": "2026-11-23T09:30:12.418Z",
  "workspace": { "id": "...", "name": "Example" },
  "project": { "id": "...", "name": "Website", "key": "WEB" },
  "actor": { "id": "...", "name": "Alex", "email": "alex@example.com", "entity_type": "human" },
  "data": {
    "issue": { "id": "...", "key": "WEB-5B0C4F0E", "title": "Fix login", "state": "in_progress" },
    "changes": { "state": { "old": "todo", "new": "in_progress" } }
  },
  "bot_context": null
}
```

The example is shortened; the [events table](#events) lists the fields of `data`. `id` identifies
the delivery. For an event without a project, `project` is
`{"id": "00000000-0000-0000-0000-000000000000", "name": "workspace", "key": "WORKSPACE"}`.

Request headers:

| Header | Value |
|---|---|
| `Content-Type` | `application/json` |
| `User-Agent` | `Sylvode-Webhook/1.0` |
| `X-Webhook-Event` | The event name. |
| `X-Webhook-Delivery` | The delivery id (the body's `id`). |
| `X-Webhook-Signature` | `sha256=<hex>`: HMAC-SHA256 of the raw request body with the webhook secret as key. |

Delivery rules:

- The request times out after 10 seconds (5 seconds to connect). Redirects are not followed.
- A 2xx answer counts as success. Anything else, and a transport error, counts as a failure.
- **There is no retry.** A failed delivery is recorded and not sent again.
- The first 64 KiB of the response body are read; the stored body and error text are cut to 2,000
  characters.
- Every attempt, including a refused target, is recorded in the delivery log
  (`GET /api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/deliveries`) with the payload, the request headers as sent,
  `response_status`, `response_body`, `error`, `duration_ms` and `success`. The webhook's
  `last_triggered_at` is updated after each attempt.

Bot tasks that the worker hands to a bot (assignments, mentions, vote requests, alert tasks) are
also posted to the bot's webhook, with the same `User-Agent` and `X-Webhook-Signature`. Their
payload and retry rules are described in [Bots and agent work](../guide/agents.md).

### Verifying the signature

Compute HMAC-SHA256 over the exact bytes of the body, before any JSON parsing, and compare it with
the header in constant time:

```python
import hashlib
import hmac

def verify(secret: str, body: bytes, header: str) -> bool:
    expected = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, header)
```

The body carries no signed timestamp, so use `X-Webhook-Delivery` to discard a delivery seen
before.

## Outbound address policy

A webhook URL must be an absolute `http` or `https` URL with a host and without user name or
password. Unless the host is allowed explicitly, it must not point at a loopback, private,
link-local, shared (`100.64.0.0/10`), documentation, multicast, broadcast or otherwise reserved
address; a host name is resolved and every address it resolves to is checked.

The policy is set in the `[outbound]` section of the [configuration](../configuration.md):

- `allowed_hosts` lists hosts (`host` or `host:port`, matched literally) that are exempt from the
  address check, for a receiver inside your network.
- `allow_private = true` turns the address check off. Use it only on a closed network.

The URL is checked when the webhook is saved and again before every delivery. A delivery to a
target that fails the check is not sent and is recorded with `error: "outbound target refused:
..."`. At start-up the API logs a warning for every active webhook whose stored URL fails the
current policy. The same policy applies to the runner address of a bot connection.

## Bot context

A webhook with a `bot_user_id` carries the tasks of that bot. A delivery to such a webhook is a bot
task when it is an `issue.*` event about a work item the bot is assigned to, or a `comment.created`
event whose comment mentions the bot. Then the payload carries:

```json
"bot_context": {
  "is_bot_task": true,
  "bot_id": "...",
  "bot_name": "Build bot",
  "bot_agent_type": "custom",
  "trigger_reason": "status_changed",
  "webhook_id": "..."
}
```

`trigger_reason` is `created` (`issue.created`), `status_changed` (`issue.state_changed`),
`mentioned` (`comment.created`) or `assigned` (`issue.updated`, `issue.assigned`,
`issue.deleted`). `bot_agent_type` is the bot user's agent type, `custom` when it has
none.

One action gives a bot at most one bot task, and never one caused by the bot itself:

- When the action already queued a worker task for the bot (assigning a work item to it, mentioning
  it in a comment), that task is the bot's task for the action, and every delivery of the same
  action reaches the bot's webhook without `bot_context`.
- When the bot performed the action (it changed the state, edited, commented, completed its own
  task), the delivery reaches its webhook without `bot_context`.

A delivery without `bot_context` is still sent, so the webhook keeps receiving every event it
subscribes to as a plain notification. `sylvode-runner` answers such a delivery `not_bot_task`.

## Business event ledger

Business writes append an entry to the business event ledger in the same transaction as the
change. The ledger is read, not pushed:

- The MCP tool and `sylvode tools call` target `events.tail` return the recent events of one form
  or one form record, newest first, with an optional exact `event_type` filter (for example
  `form_record.created`) and `per_page` up to 200.
- The same data is available through the API at `GET /api/v1/forms/{form_id}/events` and
  `GET /api/v1/form-records/{record_id}/events` (`event_type`, `page`, `per_page`).

Secrets never enter the ledger: rotating a webhook secret records `webhook.secret_rotated` without
the secret.
