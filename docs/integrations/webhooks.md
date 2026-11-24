# Webhooks and events

Sylvode tells other systems about changes in two ways:

- **Webhooks** push selected work item, comment, label, sprint, milestone, governance, project,
  member, form record and Flow events, and AI task events, to an HTTP endpoint as they happen.
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
| `issue.updated` | A work item's title, description, priority, sprint, milestone or due date change, or a Flow page is linked to it or unlinked. | `issue`, `changes` (each changed field as `old` and `new`; `sprint_id` and `due_at` included; a change of linked pages is `page_links: {added, removed}`, lists of page ids) |
| `issue.assigned` | A work item is created with an assignee, or its assignee changes, including to nobody. | `issue`, `changes.assignee_ids` (`old`, `new`, `added`, `removed`; `new` is empty after an unassignment) |
| `issue.state_changed` | A work item moves to another state. | `issue`, `changes.state` (`old`, `new`) |
| `issue.deleted` | A work item is deleted. | `issue` as it was before the delete, labels included |
| `comment.created` | A comment is added to a work item. | `comment`, `issue`, `mentions` (user ids) |
| `comment.updated` | A comment is edited. | `comment`, `issue` |
| `comment.deleted` | A comment is deleted. | `comment`, `issue` |
| `label.added` | A label is added to a work item that did not have it. | `issue` (`id`, `key`, `title`), `label` (`id`, `name`, `color`) |
| `label.removed` | A label is removed from a work item that had it, and once for every work item that had it when the label is deleted. | `issue`, `label` (the name and colour it had) |
| `sprint.started` | A sprint starts. | `sprint` (`id`, `name`, `description`, `status`, `start_date`, `end_date`, timestamps) |
| `sprint.completed` | A sprint is completed. | `sprint`, `rollover` (`moved_count`, `target_sprint_id`: what happened to the unfinished work items) |
| `sprint.created` | A sprint is created (planned or active). | `sprint` |
| `sprint.updated` | A sprint's name, description, start date or end date change to another value. A status change alone does not send it. | `sprint`, `changes` (each changed field as `old` and `new`) |
| `sprint.cancelled` | A sprint is cancelled. | `sprint`, `rollover` |
| `sprint.deleted` | A sprint is deleted. | `sprint` as it was before the delete |
| `milestone.created` | A workspace milestone is created. | `milestone` |
| `milestone.updated` | A milestone's name, description, due date or status change. | `milestone`, `changes` |
| `milestone.deleted` | A milestone is deleted. | `milestone` as it was, `cleared_item_count` (work items that had it) |
| `ai.task_completed` | A bot task completes. | `task_id`, `task_type`, `status`, `reference_type`, `reference_id`, `result` |
| `ai.task_failed` | A bot task fails for good. | `task_id`, `task_type`, `status`, `reference_type`, `reference_id`, `error_message` |
| `proposal.created`, `proposal.updated`, `proposal.deleted`, `proposal.submitted`, `proposal.voting_started`, `proposal.archived`, `proposal.vote_cast` | A governance proposal is created, edited, deleted, submitted, opened for voting, archived, or receives a vote. | The proposal and what changed |
| `veto.exercised`, `veto.withdrawn` | A vetoer exercises or withdraws a veto on a proposal. | The proposal and the veto |
| `escalation.started`, `appeal.created` | A decision is escalated, or an appeal against it is filed. | The proposal and the escalation or appeal |
| `governance_config.updated` | The workspace's governance configuration changes. | The configuration and what changed |
| `project.created`, `project.updated`, `project.deleted` | A project is created, changed or deleted. | The project |
| `member.added`, `member.removed` | A person is added to, or removed from, the workspace. | The member |
| `form.record.created`, `form.record.updated`, `form.record.archived`, `form.record.restored` | A form record is created, edited, archived or restored. Sent by the [event dispatcher](#form-and-flow-events). | `form_id`, `form_key`, `record_id`, `changed_fields` (keys only) |
| `flow.object.created`, `flow.object.archived`, `flow.object.restored`, `flow.object.moved`, `flow.content.accepted`, `flow.relation.linked`, `flow.relation.unlinked`, `flow.record.created`, `flow.record.updated`, `flow.record.archived`, `flow.schema.changed`, `flow.view.created`, `flow.view.updated`, `flow.view.reordered`, `flow.reference.created`, `flow.reference.removed`, `flow.comment.created`, `flow.comment.updated`, `flow.comment.deleted`, `flow.feature.enabled`, `flow.feature.disabled` | Something changes in the workspace's Flow pages, collections or settings. Sent by the [event dispatcher](#form-and-flow-events). | Ids, enums and counts only |

An `issue` object has `id`, `key`, `title`, `description`, `state`, `priority`, `assignee_ids`,
`label_ids`, `sprint_id`, `milestone_id`, `page_links`, `due_at`, `created_at` and `updated_at`. `page_links` is
`[{object_id, title}]`, the Flow pages linked to the item; a `title` is present only for a page of the
item's workspace that every ordinary member can read, and is `null` for a restricted page and for a
personal page placed into the workspace (a delivery has no reader to check the title against). A `comment` object has `id`, `issue_id`,
`content`, `author_id`, `author_name`, `created_at` and `updated_at`; the `issue` next to it has
`id`, `key` and `title`, in `comment.deleted` too, where the `comment` is the one that was deleted,
with its author and its original timestamps. In these payloads `key` is the work item's identifier: the project key and the
work item's number, for example `WEB-12`.

The work items that completing, cancelling or deleting a sprint moves out of it each send an
`issue.updated` with `changes.sprint_id` (`old`, `new`), after the sprint's own event
(`sprint.completed`, `sprint.cancelled` or `sprint.deleted`).

An alert rule that opens a work item also sends `issue.created` for it (see [Alerts](alerts.md)).

## Deliveries

The form record and Flow events use a different delivery path, described under
[Form and Flow events](#form-and-flow-events). Every other event is sent as described here.

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
    "issue": { "id": "...", "key": "WEB-12", "title": "Fix login", "state": "in_progress" },
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
  `response_status`, `response_body`, `error`, `duration_ms` and `success`. `duration_ms` is the
  measured time of the request (0 for a refused target; absent for entries from before it was
  measured). The webhook's `last_triggered_at` is updated after each attempt.

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

## Form and Flow events

`form.record.*` and `flow.*` events are delivered by the event dispatcher, not by the path above.
They have retries and their own body, and they are sent to the same webhooks, which subscribe to
them in the same `events` list.

The body is a delivery wrapper around the event envelope:

```json
{
  "delivery": { "id": "8f1d6c52-...", "attempt": 1, "coalesced": false },
  "event": {
    "version": "sylvode.event.v1",
    "event_id": "...",
    "event_type": "form.record.updated",
    "workspace_id": "...",
    "project_id": "...",
    "aggregate": { "type": "form.staff", "id": "<record id>" },
    "actor_id": "...",
    "source": { "type": "user", "actor_id": "..." },
    "payload": {
      "form_id": "...",
      "form_key": "staff",
      "record_id": "...",
      "changed_fields": ["salary"]
    },
    "metadata": {},
    "correlation_id": null,
    "causation_id": null,
    "created_at": "2026-11-24T09:30:12.418Z"
  }
}
```

- `delivery.id` stays the same across the retries of one delivery; `attempt` counts them from 1.
  Flow content events that arrive in quick succession for one document can be sent as one delivery
  (`coalesced: true`, with `delivery.source_event_ids`).
- A form record payload names the form, the record and the keys of the fields that changed
  (`changed_fields`: for a new record every field that has a value, for an edit the fields whose
  value differs, an empty list for an archive and a restore). It never carries a field value, a
  record title or any other text of the record: a webhook is read by the whole workspace, so the
  field permissions of individual people cannot be applied to it. A receiver that needs the values
  reads the record through the API with its own credentials.
- A Flow payload carries ids, enums and counts, such as the object id and type, the collection and
  record ids, the ids of the changed blocks. No page text and no title. An event type that has no
  public payload declared is sent with `{"field_permission_redacted": true}` instead of a payload.
- `source` keeps only `type`, `actor_id` and `origin_event`.

Request headers:

| Header | Value |
|---|---|
| `Content-Type` | `application/json` |
| `User-Agent` | `Sylvode-Webhook/1.0` |
| `X-Webhook-Signature` | `sha256=<hex>`: HMAC-SHA256 of the raw request body with the webhook secret, as for the other events. |
| `X-Sylvode-Delivery-Id` | The delivery id (`delivery.id`). |

A 2xx answer counts as success. Anything else, and a transport error, is retried: after the first
failure the dispatcher waits 30 seconds, then 60, and so on in steps of 30 seconds up to 5 minutes,
and gives up after 10 attempts. The delivery id does not change, so a receiver can discard a
delivery it has already handled. The request times out after 10 seconds. The same
[outbound address policy](#outbound-address-policy) applies, checked before every attempt. These
deliveries are not listed in the webhook's delivery log (`.../deliveries`), which records the
other events.

## Outbound address policy

A webhook URL must be an absolute `http` or `https` URL with a host and without user name or
password. Unless the host is allowed explicitly, it must not point at a loopback, private,
link-local, shared (`100.64.0.0/10`), documentation, multicast, broadcast or otherwise reserved
address; a host name is resolved and every address it resolves to is checked. A delivery connects
to exactly the addresses that were checked; the name is not resolved a second time.

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
- When the bot performed the action (it changed the state, edited or commented), the delivery
  reaches its webhook without `bot_context`.

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
