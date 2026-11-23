# Alerts

The alert chain lets an external security product (a web application firewall, an intrusion
detection system, a scanner) report events to a Sylvode project. Sylvode stores each event, and an
alert rule of the project can turn it into an alert work item, notify people and hand the work item
to a bot.

```
security product  ->  POST /api/v1/inbound/events  ->  Sylvode
    stores the event (once per source and event_id)
    finds the first matching alert rule of the project
    attaches the event to the open alert of its dedup key, or opens an alert work item
    notifies the rule's recipients
    optionally starts one bot task for the alert (within safety caps)
worker follow-up
    starts queued tasks, announces task outcomes, closes finished alerts, prunes old events
```

Sources and rules are managed through the API only; the web UI has no pages for them. Alert work
items appear in the project like any other work item. The endpoints are summarised in the
[API reference](api.md#alerts), which also explains authentication and the response envelope.

## Enabling the endpoint

The inbound endpoint is off by default and answers 503 `inbound_disabled` to every caller until the
`[inbound]` section of the [configuration](../configuration.md) enables it:

```toml
[inbound]
enabled = true
```

The other `[inbound]` keys bound resources (see [Limits](#limits)). The worker process must run: it
starts queued tasks, announces task outcomes, closes alerts and deletes old events.

Sylvode limits and records requests by client address. Behind a reverse proxy, list the proxy in
`[server] trusted_proxies` so that the client address is taken from `X-Forwarded-For`; otherwise
the proxy's own address is used for every request.

## Sources

A source is the credential one product uses to post into one project. Workspace owners and admins
manage sources; bot tokens and delegated tokens are refused.

```sh
curl -s https://sylvode.example.com/api/v1/projects/$PROJECT_ID/inbound-sources \
  -H "Authorization: Bearer $SYLVODE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{"name": "edge-waf-01", "require_signature": true, "ip_allowlist": ["198.51.100.0/24"]}'
```

| Field | Default | Notes |
|---|---|---|
| `name` | required | 1 to 100 characters. Appears in alert titles. |
| `format` | `native_v1` | The only format: the [event envelope](#sending-events) below. |
| `require_signature` | `true` | When `true`, every request must be signed. When `false`, unsigned requests are accepted and signed ones are still checked. |
| `rate_per_minute` | 120 | Events per minute, 1 to 6000. |
| `daily_cap` | 20000 | Events per day, 1 to 1000000. |
| `ip_allowlist` | empty | Up to 64 address blocks (`198.51.100.0/24`, or a bare address for one host). Empty admits every address. A block with host bits set (`198.51.100.5/24`) or a prefix of 0 is refused. |

The answer is `{"source", "token", "signing_secret"}`. The token (`svi_` followed by 64 hex
characters) and the signing secret (`svs_` followed by 64 hex characters) are shown only in this
answer; afterwards a source shows `token_prefix` (the first 12 characters) and `has_signing_secret`.
Give both values to the product.

The person who creates a source is the person it acts for (`acting_user_id`): alert work items are
created in their name. When that person is not an owner or admin of the workspace any more, or their
account is inactive, the source is suspended and every request answers 403 `source_suspended`.
Another owner or admin takes the source over with `PATCH` and `{"take_over": true}`.

| Method | Path | Notes |
|---|---|---|
| GET | `/api/v1/projects/{project_id}/inbound-sources` | The project's sources, oldest first. |
| POST | `/api/v1/projects/{project_id}/inbound-sources` | Creates a source; answers the token and signing secret once. |
| GET | `/api/v1/inbound-sources/{id}` | One source. Also shows `last_used_at` and `last_used_ip`. |
| PATCH | `/api/v1/inbound-sources/{id}` | Any of `name`, `enabled`, `require_signature`, `rate_per_minute`, `daily_cap`, `ip_allowlist` (replaces the list), `take_over`. |
| POST | `/api/v1/inbound-sources/{id}/rotate` | New token and new signing secret, answered once. The old token stops working at once. |
| DELETE | `/api/v1/inbound-sources/{id}` | Deletes the source. |
| GET | `/api/v1/inbound-sources/{id}/events` | Stored events of the source, newest first, with payload and outcome. `limit` (default 50, maximum 100) and `before` (RFC 3339 timestamp; events received before it) page through older events. |

Every change of a source is recorded as a business event with the old and new settings, never with
a secret.

## Sending events

A product posts one event per request:

```
POST /api/v1/inbound/events
Authorization: Bearer svi_...
Content-Type: application/json
X-Sylvode-Timestamp: 1795426212
X-Sylvode-Signature: v1=<hex>
```

The endpoint accepts only an inbound source token. A user token, bot token or delegated token is
refused like an unknown token, and a token in the query string (`?token=`) is refused with 400
`credential_in_query`.

### The event envelope

```json
{
  "spec": "sylvode.inbound_event.v1",
  "event_id": "waf-01:2026-11-23:000123",
  "occurred_at": "2026-11-23T09:30:00Z",
  "kind": "waf.sqli",
  "severity": "high",
  "title": "SQL injection attempt blocked",
  "summary": "Blocked 14 requests matching rule 942100.",
  "subject": { "host": "shop.example.com", "ip": "203.0.113.7" },
  "payload": { "rule": "942100", "count": 14 }
}
```

| Field | Required | Rules |
|---|---|---|
| `spec` | yes | Exactly `sylvode.inbound_event.v1`. |
| `event_id` | yes | 1 to 128 characters of `A-Z a-z 0-9 . _ : @ / -`. Unique per source. |
| `occurred_at` | yes | RFC 3339. At most 5 minutes in the future and at most 7 days old. |
| `kind` | yes | Lower-case segments of `a-z 0-9 _` separated by dots: at most 4 segments of at most 32 characters, at most 64 characters in all, for example `waf.sqli`. |
| `severity` | yes | `info`, `low`, `medium`, `high` or `critical` (lower case). |
| `title` | yes | 1 to 200 characters. |
| `summary` | no | At most 2000 characters. |
| `dedup_key` | no | 1 to 200 printable characters. Default: `<kind>:<subject.host>` when the subject has a `host`, otherwise `<kind>`. |
| `subject` | no | A flat object of at most 8 string values; keys are 1 to 32 characters of `a-z` and `_`, values at most 200 characters. |
| `payload` | no | Any JSON value of at most 32 KiB, nested at most 16 levels. |

Any other top-level field is refused. Control characters are removed from the text fields (`summary`
keeps line breaks). All text from the event is treated as untrusted data (see
[What an alert creates](#what-an-alert-creates)).

### Signing

Every source has a signing secret. To sign a request, compute HMAC-SHA256 with the secret as key over
the decimal Unix timestamp, a dot and the exact request body, and send:

- `X-Sylvode-Timestamp`: the same timestamp. It must be within `signature_tolerance_secs` of the
  server clock (300 seconds by default).
- `X-Sylvode-Signature`: `v1=` followed by the lower-case hex digest.

```sh
body='{"spec":"sylvode.inbound_event.v1","event_id":"test-1","occurred_at":"2026-11-23T09:30:00Z","kind":"waf.sqli","severity":"high","title":"Test event"}'
ts=$(date +%s)
sig=$(printf '%s.%s' "$ts" "$body" | openssl dgst -sha256 -hmac "$SIGNING_SECRET" -hex | sed 's/^.* //')
curl -s https://sylvode.example.com/api/v1/inbound/events \
  -H "Authorization: Bearer $SOURCE_TOKEN" \
  -H 'Content-Type: application/json' \
  -H "X-Sylvode-Timestamp: $ts" \
  -H "X-Sylvode-Signature: v1=$sig" \
  --data-binary "$body"
```

A request with only one of the two headers, a timestamp outside the window or a wrong digest is
refused with 401 `invalid_signature`, also when the source does not require signatures.

### Responses

The inbound endpoint answers with real HTTP statuses. The body uses the API envelope, with `code`
equal to the status and `message` a stable name:

```json
{ "code": 202, "message": "accepted", "data": { "status": "accepted", "inbound_event_id": "..." } }
```

| Status | `message` | Meaning |
|---|---|---|
| 202 | `accepted` | The event was stored and processed. |
| 200 | `duplicate` | The source already sent this `event_id` with the same body. Nothing changes. |
| 400 | `invalid_envelope` | `data` is `{"field", "reason"}`, naming the field to fix. |
| 400 | `credential_in_query` | The token was sent in the query string. |
| 401 | `invalid_credentials` | Missing, malformed, unknown, rotated or disabled token. |
| 401 | `invalid_signature` | Signature missing (when required) or wrong. |
| 403 | `ip_not_allowed` | The client address is outside the source's allow-list. |
| 403 | `source_suspended` | The person the source acts for is not an owner or admin of the workspace any more. |
| 409 | `event_id_conflict` | The source already sent this `event_id` with a different body. |
| 413 | `body_too_large` | The body is larger than `max_body_bytes`. |
| 415 | `unsupported_media_type` | `Content-Type` is not `application/json`. |
| 422 | `occurred_at_out_of_range` | `occurred_at` is too far in the future or too old. |
| 429 | `rate_limited` | A source allowance or the client address's failed-authentication allowance is used up. `Retry-After` gives the seconds to wait. |
| 503 | `inbound_disabled` | The endpoint is not enabled. |
| 503 | `service_unavailable` | The database is not reachable. Retry later. |

An accepted answer never says which rule matched or whether a task was created. Each failed
authentication counts against the client address (`auth_failures_per_minute_per_ip`); a source's
per-minute and per-day allowances are counted before the body is read, so invalid bodies count too.
Rejections of a known source are recorded as `inbound_event.rejected` business events, at most one
per source and reason per minute.

## Alert rules

A rule decides what happens to the events of a project. Rules are evaluated in their order; the
first enabled rule that matches the event's source, kind and severity handles it, and an event
without a matching rule is only stored.

```sh
curl -s https://sylvode.example.com/api/v1/projects/$PROJECT_ID/alert-rules \
  -H "Authorization: Bearer $SYLVODE_TOKEN" \
  -H 'Content-Type: application/json' \
  -d '{
    "name": "WAF high and critical",
    "kind_patterns": ["waf.*"],
    "min_severity": "high",
    "bot_user_id": "'"$BOT_ID"'",
    "instructions": "Investigate {alert_identifier} ({severity}, {occurrences} occurrences) and report what you blocked."
  }'
```

| Field | Default | Rules |
|---|---|---|
| `name` | required | 1 to 100 characters. |
| `enabled` | `true` | |
| `source_id` | none | Limits the rule to one source of the project. Without it the rule applies to every source of the project. |
| `kind_patterns` | empty | Up to 32 patterns: an exact kind, or a prefix wildcard such as `waf.*` (matches `waf.sqli`, not `waf`). Empty matches every kind. |
| `min_severity` | `high` | The lowest severity the rule handles. |
| `action` | `work_item_and_task` when `min_severity` is `high` or `critical`, otherwise `work_item` | `work_item` opens a work item and notifies; `work_item_and_task` also hands the alert to a bot. |
| `bot_user_id` | none | Required with `work_item_and_task`, refused with `work_item`. Must be an active bot that is a member of the workspace. |
| `instructions` | empty | At most 4000 characters, written into the work item for the bot. Placeholders: `{alert_identifier}`, `{severity}`, `{kind}`, `{source_name}`, `{occurrences}`, `{first_seen}`, `{last_seen}`. |
| `priority_map` | `critical`: `urgent`, `high`: `high`, `medium`: `medium`, `low` and `info`: `low` | Work item priority per severity; values `low`, `medium`, `high`, `urgent`. Severities you leave out keep the default. |
| `label_ids` | empty | Up to 50 labels of the workspace, added to new alert work items. |
| `notify_user_ids` | the rule's author | Up to 50 people who are members of the workspace. They receive the alert notifications. |
| `max_tasks_per_hour` | 6 | 1 to 120. Bot tasks this rule may start per hour. |
| `max_active_tasks` | 2 | 1 to 20. Bot tasks of this rule that may be active at once. |
| `dedup_window_secs` | 3600 | 60 to 604800. How long after its last event an alert stays open for new events. |
| `rearm_after_secs` | 900 | 0 to 86400. After an alert's task ends, how long further events on the same alert start no new task. |
| `task_max_attempts` | 3 | 1 to 10. Attempts of each bot task. |

Saving a rule whose bot was made by a project bot connection but has no enabled connection in this
project succeeds with the warning `bot_not_connected_to_project`: such a bot gets no alert tasks
until it is connected.

| Method | Path | Notes |
|---|---|---|
| GET | `/api/v1/projects/{project_id}/alert-rules` | The rules in evaluation order. |
| POST | `/api/v1/projects/{project_id}/alert-rules` | Creates a rule at the end of the order. Answers `{"rule", "warnings"}`. |
| GET | `/api/v1/alert-rules/{id}` | One rule, with its `version`. |
| PUT | `/api/v1/alert-rules/{id}` | Replaces the whole rule. The body needs `expected_version`; a stale version answers HTTP 409 `version_conflict` and changes nothing. Answers `{"rule", "warnings"}`. |
| DELETE | `/api/v1/alert-rules/{id}` | |
| POST | `/api/v1/projects/{project_id}/alert-rules/reorder` | `{"rule_ids": [...]}` listing every rule of the project exactly once, in the new order. |
| POST | `/api/v1/projects/{project_id}/alert-rules/test` | Dry run; see below. |

### Testing rules

`POST /api/v1/projects/{project_id}/alert-rules/test` shows what an event would do, without storing
anything or counting against any limit. The body names a source of the project and exactly one of a
sample envelope or a stored event of that source:

```json
{ "source_id": "...", "envelope": { "spec": "sylvode.inbound_event.v1", "event_id": "t-1",
  "occurred_at": "2026-11-23T09:30:00Z", "kind": "waf.sqli", "severity": "critical",
  "title": "Test" } }
```

or `{"source_id": "...", "inbound_event_id": "..."}`. The answer:

| Field | Meaning |
|---|---|
| `matched`, `rule` | Whether a rule matched, and which. |
| `rules_evaluated` | Every rule with `matched` and `reason`: `matched`, `disabled`, `other_source`, `kind_not_listed` or `below_minimum_severity`. |
| `alert` | `no_rule`, `would_open`, `would_attach` or `would_refuse_alert_cap`. |
| `task` | `none`, `bot_unavailable`, `would_start`, `would_queue`, `already_active`, `already_waiting` or `rearm_delay`. |
| `queue_cause` | The cap that would queue the task. |
| `would_escalate` | Whether the event would raise an open alert to `high` or above. |
| `caps` | The rule's and workspace's current counts and limits. |
| `notify_recipients` | How many of the rule's recipients would be notified. |
| `title` | The title a new alert work item would get. |

## What an alert creates

### The alert and its work item

Events of one source with the same `dedup_key` belong to one alert while it is open. An event that
matches a rule either joins the open alert or opens a new one:

- **It joins** when the alert received an event within the rule's `dedup_window_secs` and its work
  item is not in a terminal workflow state. The alert's occurrence count and last-seen time go up,
  and its highest severity is kept.
- **It opens a new alert** otherwise. The rule may open at most `max_alerts_per_rule_per_hour` new
  alerts per hour; past that the event is stored with the outcome `suppressed_alert_cap` and the
  recipients get an `alert_suppressed` notice.

A new alert work item:

- is created in the project's initial workflow state, in the name of the source's acting person;
- has the title `[SEVERITY] <source name> · <kind>`, followed by ` · <host>` when `subject.host`
  looks like a host name or address. The event's own `title` never enters the work item title;
- has a description built from a fixed template: the alert facts, the rule's instructions with the
  placeholders filled in, and the event's title, summary, subject and payload inside a fenced block
  marked as untrusted (cut at 16 KiB). The fence is longer than any run of fence characters inside
  it, so event text cannot close it;
- gets the priority from the rule's `priority_map` and the rule's labels;
- is assigned to the rule's bot when the rule hands alerts to a bot;
- is announced to webhooks as `issue.created` (see [Webhooks](webhooks.md)).

`GET /api/v1/issues/{id}/alert` shows the alert facts of a work item to any workspace member:
source, kind, highest severity, occurrences, first and last seen, whether the alert is open, the
rule, the latest task and its status, `task_state` (`none`, `created`, `waiting` or `suppressed`)
and, for a waiting task, `queued_at` and `queue_expires_at`. It answers 404 `issue is not an alert`
for an ordinary work item. Secrets, instructions and event payloads are not shown.

The worker closes an open alert when its work item reaches a terminal state or when its dedup window
has passed; the next matching event then opens a new alert.

### The bot task

When the rule's action is `work_item_and_task`, an alert gets one bot task at a time:

- The task is an `issue_assigned` task for the work item, with `"trigger": "alert_rule"` and an
  `alert` object (`alert_id`, `rule_id`, `source_id`, `inbound_event_id`, `severity`, `kind`,
  `occurrences`) in its payload, the rule's `task_max_attempts` and a priority that follows the
  severity. It reaches the bot like any other bot task (see
  [Bots and agent work](../guide/agents.md)).
- While the alert's task is active, or for `rearm_after_secs` after it ended, further events on the
  alert start no new task. After that, the next event on the still-open alert starts a new task.
- A bot made by a project bot connection needs an enabled connection in the rule's project. Without
  one, the alert still opens, no task starts, the event's outcome is `bot_unavailable` and the
  recipients get an `alert_suppressed` notice. The same happens when the rule's bot has left the
  workspace or is inactive.

### Safety caps and the queue

A task is not started when one of these is reached, checked in this order:

1. the rule's `max_tasks_per_hour`;
2. the rule's `max_active_tasks`;
3. `[inbound] max_active_alert_tasks_per_workspace`, counted over every rule of the workspace.

The task is then queued instead of dropped: the alert's `task_state` becomes `waiting` and the
event's outcome is `task_queued`. The worker starts waiting tasks as capacity frees up, oldest first
and fairly across rules, and checks the rule, the bot, the source and the work item again before it
starts one. A task that has waited for one hour is suppressed (`task_state: "suppressed"`) and the
recipients get an `alert_suppressed` notice, at most one per rule per hour.

### Event outcomes

Every stored event records its outcome, shown by `GET /api/v1/inbound-sources/{id}/events`:

| Outcome | Meaning |
|---|---|
| `no_rule` | No enabled rule matched. The event is only stored. |
| `alert_opened` | The event opened a new alert. |
| `alert_attached` | The event joined an open alert. |
| `task_queued` | A cap queued the alert's task. |
| `suppressed_alert_cap` | The rule's hourly alert cap refused a new alert. |
| `bot_unavailable` | The rule's bot cannot receive the task. |

### Notifications

Notifications go to the rule's `notify_user_ids` who are still active people in the workspace:

| Notification type | When |
|---|---|
| `alert_opened` | A new alert work item is opened. |
| `alert_escalated` | An event raises an open alert's highest severity to `high` or `critical`. |
| `alert_task_completed`, `alert_task_failed` | The alert's bot task completed or failed for good. A failed attempt that is retried notifies nobody. |
| `alert_suppressed` | No alert or task was started because of the hourly alert cap, a task that waited in the queue for an hour, or an unavailable bot. At most one per rule per hour. |
| `alert_source_throttled` | A source hit its per-minute or per-day allowance. Sent to the source's acting person, at most once per source per hour. |

They appear in the notification centre and through the [notifications API](api.md#notifications).

## Limits

Per source, set on the source: `rate_per_minute`, `daily_cap`, `ip_allowlist`. Per rule, set on the
rule: `max_tasks_per_hour`, `max_active_tasks`. For the whole instance, in `[inbound]`:

| Key | Default | Effect |
|---|---|---|
| `enabled` | `false` | Whether the endpoint answers. |
| `max_body_bytes` | 65536 | Largest request body. |
| `signature_tolerance_secs` | 300 | Allowed difference between `X-Sylvode-Timestamp` and the server clock. |
| `auth_failures_per_minute_per_ip` | 30 | Failed authentications one client address may make per minute before it gets 429. |
| `max_active_alert_tasks_per_workspace` | 5 | Alert tasks active at once in one workspace. |
| `max_alerts_per_rule_per_hour` | 60 | New alerts one rule may open per hour. |
| `event_retention_days` | 30 | Stored events older than this are deleted (checked hourly by the worker). |
| `followup_interval_secs` | 2 | How often the worker runs the follow-up. |

Each key has an accepted range; the [configuration reference](../configuration.md) lists them.
