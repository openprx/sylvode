# Bots and agent work

Bots are workspace users that act for an agent: a CLI agent, a chat assistant or another service.
People hand work to a bot the same way they hand it to a colleague (by assigning an issue,
mentioning it, or asking for its vote), and Sylvode turns each such action into an **AI task**
that it delivers to the bot's runner. This guide explains the kinds of bots, how tasks arise,
how they end, and how to connect a bot to a project so that a CLI agent works on its tasks.

The program that receives tasks on your machines is the Sylvode runner; its installation,
configuration and agent types are described in the [runner README](../../apps/runner/README.md).

## Bots

| Kind | Created where | Agent type | Token |
|---|---|---|---|
| Token bot | workspace settings, **Members and bots**, section **API Tokens**, **Create Token** | `mcp` | one `opr_` token, shown once |
| Connection bot | a project's **Execution** settings, **Connect a bot** | `mcp` | one `opr_` token for `mcp_stdio`, inside the configuration block, shown once |
| Bot user created by an administrator | instance administration, users | `openclaw`, `webhook` or `custom` | none |

Every token creates its own bot user: the token's name is the bot's display name in activity
feeds, comments and the audit trail. A bot can be assigned issues, mentioned, named as a
webhook's bot user and, as a connection bot, take part in governance votes.

The administrator form also stores an OpenClaw command, channel and target for a bot user; nothing
reads these values. The runner chooses agents by bot id, name or route, not by these settings
(see [Choosing the agent for a task](../../apps/runner/README.md#choosing-the-agent-for-a-task)).

### Token permissions

A token carries one or more of `read`, `write` and `admin` (default `read`); each includes the
ones below it. Safe HTTP methods (`GET`, `HEAD`, `OPTIONS`) need `read`; every other method needs
`write`. A bot with `admin` is seated as a workspace administrator, any other bot as a member. A
refused call answers 403 `bot token lacks the '<permission>' permission`. A token may have an
expiry date. Only workspace owners and administrators can create, list and revoke tokens.

### Token surfaces

Every token works on exactly one transport surface, chosen when it is created, and is refused on
every other. Create one token per place you connect from; the surface cannot be changed, so issue
a new token instead.

| Surface | UI label | Where the token works |
|---|---|---|
| `rest` | REST API | direct REST calls with `Authorization: Bearer opr_...` |
| `mcp_http` | MCP over HTTP | the MCP server's HTTP endpoint, sent by the client |
| `mcp_sse` | MCP over SSE | the MCP server's SSE endpoint |
| `mcp_stdio` | MCP over stdio | an MCP server started over stdio, such as the one the runner starts for a CLI agent |
| `cli` | Command line | the `sylvode` command, other than `tools call` |
| `cli_tools_call` | CLI tools call | `sylvode tools call` |

The web form requires a choice; `POST /api/v1/workspaces/{workspace_id}/bots` takes it as
`transport_surface` and defaults to `rest`. See the [MCP server](../../apps/mcp-server/README.md)
and the [command line](../integrations/cli.md) for how each surface is used.

## How tasks arise

| Action | Task type | `trigger_kind` | Goes to |
|---|---|---|---|
| An issue is created with a bot as assignee, or reassigned to a bot | `issue_assigned` | `assigned` | the assigned bot |
| A comment mentions a bot | `comment_requested` | `mention` | each mentioned bot |
| A proposal starts voting (by hand or by the governance schedule) | `vote_requested` | `proposal_vote` | every bot that is an active AI participant of the project |
| An alert rule matches an inbound alert | `issue_assigned` | `assigned` | the rule's bot, on the alert's work item ([Alerts](../integrations/alerts.md)) |
| A workspace owner or administrator creates a task by hand (`POST /api/v1/projects/{project_id}/ai/tasks`) | `issue_assigned`, `review_requested`, `comment_requested` or `vote_requested` | follows the type | a bot that is an active AI participant of the project |

Rules that apply to every task:

- **A bot never gets a task from its own action.** Assigning an issue to itself, mentioning
  itself, or completing its own task creates nothing for it.
- **One action gives a bot at most one task.** The assignment or mention is the bot's task; the
  webhook deliveries of the same request reach the bot's webhook without `bot_context`. Other
  activity on an issue assigned to a bot (someone else changes its state, edits it, comments on
  it) is delivered to the bot's webhook with `bot_context`; see
  [Webhooks](../integrations/webhooks.md).
- **Bot-to-bot chains are limited.** A task started by a person or an alert is depth 0; a task
  caused by a bot while it runs a task is one level deeper. When a new task would go deeper than
  `[ai_tasks] max_chain_depth` (default 3), or the same bot already took part in the chain for the
  same work item, no task is created and an `ai_task.chain_refused` event records why
  (`chain_too_deep` or `chain_cycle`). A task created by hand is then refused with 409.
- **A connection bot needs an enabled connection.** For a bot made by a project connection, an
  assignment or mention in a project where the bot has no enabled connection stands (the bot is
  the assignee) but creates no task; an alert rule records `bot_unavailable`. Pending tasks of such
  a bot are failed with the reason `bot_not_connected_to_project` or `connection_disabled`.

By default a task may be attempted 3 times (an alert rule sets its own number). Assignments and
mentions are picked up before vote tasks.

## How a task is delivered

The Sylvode worker picks pending tasks up, highest priority first, marks each `processing` with
one attempt more and a lease of `[ai_tasks] processing_timeout_secs`, and delivers it as a signed
`POST` to the bot's webhook: the managed webhook of a connection bot, or a workspace webhook whose
bot user is the bot. The body carries `task_id`, `project_id`, `project_key`,
`ai_participant_id`, `ai_participant_name`, `ai_participant_agent_type`, `task_type`,
`trigger_kind`, `reference_type`, `reference_id`, `payload`, `attempts`, `max_attempts` and
`protocol`.

After a bot is connected, and after its credentials are rotated, the worker holds the bot's
pending tasks until the runner has used the new token (the runner's start-up handshake does), for
at most one hour.

## How a task ends

A delivered task stays open (`processing`, then `acknowledged`) until one of these happens; the
first to commit wins, and the others find the attempt closed and change nothing.

| What happens | Result |
|---|---|
| The bot ends it through the task protocol: `complete` | `completed`; the reply is written ([Replies](#replies-and-state-changes)) |
| The bot ends it: `fail` with `retryable: false` (the default) | `failed` at once, even if attempts remain |
| The bot ends it: `fail` with `retryable: true` | back to `pending` while attempts remain, else `failed` |
| The runner declines it in its answer: `no_agent`, `ignored`, `refused`, or an HTTP 4xx other than 408, 409, 425 and 429 | `failed` at once, with a message such as `the runner refused the task (path_outside_allowlist): ...`; fix the runner and assign again |
| The delivery fails: no active webhook, a transport error, HTTP 5xx, 408, 409, 425 or 429 | back to `pending` after 30 seconds times the attempt number (at most 10 minutes) while attempts remain, else `failed` |
| The lease expires: nothing acknowledged or reported progress in time | back to `pending` while attempts remain, else `failed`, with a message naming the lease and the attempt |

Acknowledging and reporting progress extend the lease by `[ai_tasks] lease_extension_secs`, but
never past `max_processing_secs` after the attempt started. The `[ai_tasks]` keys and their
defaults are in the [configuration reference](../configuration.md). A failed task emits the
`ai.task_failed` webhook event, a completed one `ai.task_completed`.

### The task protocol

The bot calls these routes with its own token. Only the task's own bot may call the task routes;
anyone else gets 404 `ai task not found`. Every task route names the attempt
(`?attempt=N`, the delivery's `attempts`) and is refused with 400
`task is not active for attempt N (current attempt M)` for any other attempt, so a late call never
ends a later attempt. A CLI agent run by the runner makes these calls through the MCP task tools
(`sylvode_task_get`, `sylvode_task_ack`, `sylvode_task_progress`, `sylvode_task_complete`,
`sylvode_task_fail`).

| Method and path | Purpose |
|---|---|
| `GET /api/v1/ai/tasks/{task_id}?attempt=N` | the task view: source, reply policy, execution settings, `active` |
| `POST /api/v1/ai/tasks/{task_id}/ack?attempt=N` | acknowledge with a short `plan` |
| `POST /api/v1/ai/tasks/{task_id}/progress?attempt=N` | report progress (reports of one kind less than 30 seconds apart extend the lease but write one event) |
| `POST /api/v1/ai/tasks/{task_id}/complete?attempt=N` | end the task with an outcome and a summary |
| `POST /api/v1/ai/tasks/{task_id}/fail?attempt=N` | end the attempt with a `reason`, `source` (`agent` or `runner`) and `retryable` |
| `GET /api/v1/ai/tasks/active` | the bot's open tasks |
| `GET /api/v1/ai/protocol` | the task protocol version and lease settings |
| `PUT /api/v1/ai/runner/report` | the runner's status report (connection bots only) |

The [API reference](../integrations/api.md) lists the request and response fields.

## Replies and state changes

Completing or failing a task writes the reply in the same transaction as the status change. A
reply never creates a task, never turns mentions in its text into tasks, and never assigns anyone.

| Task | On completion | On final failure |
|---|---|---|
| Assignment | a comment with the summary; with outcome `done`, the issue moves as the connection's **On completion** setting says | a comment `Could not complete: <reason>`, and the connection's **Failure label** if one is set |
| Mention or review request | a reply comment (`Re: comment <id>`) | a comment `Could not complete: <reason>` |
| Vote request | the bot's vote is cast | nothing |
| Created by hand with another type | nothing | nothing |

**On completion** has three choices: **Move the issue to the first Done state** (`auto`: the
done-category state with the lowest position, unless the issue is already in a terminal state),
**Leave the issue where it is** (`none`), and **Move to state** (a state key of the workflow). A
state that is not in the workflow leaves the issue where it is, with the warning
`state <key> not in workflow`. Without a connection (a token bot) an assignment behaves like
`auto`.

A vote task names its proposal in `payload.proposal_id`. Completing it casts the bot's vote
(`yes`, `no` or `abstain`) through the same rules as a person's vote: a reason shorter than the
minimum (50 characters by default for a connection bot), a closed voting window or a proposal
that is not in voting is refused with 400 and the task stays open. A vote the bot already cast
completes the task with the warning `vote already recorded`.

When the runner declines a task, or a pending task is failed because its connection is disabled,
the issue gets a comment `Could not start: <reason>` (or a reply to the mention), for assignment,
mention and review tasks of a bot that has a connection to the project, enabled or not.

## Connecting a bot to a project

A connection lets a CLI agent (claude, codex or opencode) on your machine work on a project's
tasks through the runner. Workspace owners and administrators manage connections in the project's
settings under **Execution**; other members see which bots are connected, with their CLI and
channel.

1. Open the project's **Execution** settings and choose **Connect a bot**.
2. Choose **New bot** and give it a **Bot name**, or pick an already connected bot.
3. Enter the **Runner address**: the base URL at which Sylvode reaches your runner (tasks go to
   `<address>/webhook`). It must be `https://` unless an administrator lists its host in
   `[outbound] allowed_hosts`; it may not carry credentials, a query or a fragment, and it is
   checked against the outbound address policy. A connected bot keeps its one runner address
   for every project.
4. Choose the **CLI** (**Claude Code**, **Codex (experimental)** or **opencode (experimental)**),
   the **Channel** (**Interactive session (tmux)** or **One-shot (headless)**; tmux is for Claude
   Code only) and the **Working directory** on the runner host.
5. For a new bot, copy the configuration block shown under **Copy this configuration now**
   (**Copy configuration** or **Download file**) and confirm **I have stored this configuration**.
   Sylvode keeps no copy and cannot show it again; **Rotate credentials** issues a new one.
6. Merge the block into the runner's configuration, check `[runner] allowed_roots`, and start the
   runner ([runner README](../../apps/runner/README.md#configure)). To add a second bot to a
   runner that already serves one, append the new block's `[[ai_task_callback.bots]]` and
   `[[agents]]` tables (the bot table already has its own `webhook_secret`), give each `runner`
   agent its own `allowed_roots`, and keep the other sections once.

### Execution settings

| Setting | Values |
|---|---|
| Channel | `tmux` (default) or `headless`; `tmux` only with `claude` |
| CLI | `claude`, `codex` or `opencode` |
| Working directory | an absolute path of 2 to 1024 bytes, without `#`, control characters, empty segments, or `.` and `..` segments |
| On completion | `auto` (default), `none`, or a state of the project's workflow |
| Failure label | a label of the workspace, or none |
| Enabled | changed with **Disable** and **Enable** |

A project can have several connected bots, but at most one tmux connection per CLI: a second
tmux connection for the same CLI is refused with 409
`channel: project already has a tmux connection for <cli> (bot <holder>)`. Headless connections
are not limited. On the runner a tmux connection uses the session `<KEY>-<cli>` (for Claude Code,
`<KEY>-claude`).

Every change names the version of the connection it was made against. If someone else changed
the connection meanwhile, the change is refused (`connection changed; reload`) and the page says
**Someone else changed this connection. Reload to see the current values.**

### Status

The connection shows what the runner last reported: **Online**, **Waiting for the runner**,
**Not reporting** or **Unreachable**; the readiness of tmux and each CLI; the project's tmux
session; a protocol mismatch; and **Permission checks off** when the runner's launch command or
headless arguments turn off the CLI's permission checks (the default does).

### Disable, rotate, remove

- **Disable**: the bot stops receiving tasks for the project and stops voting in it; its pending
  tasks there are failed. A task already running continues until it ends. If this was the bot's
  last enabled connection, its token and webhook are switched off as well, so a running task can
  not report any more and ends when its lease expires.
- **Rotate credentials**: a new token, a new signing secret and a new configuration block. The
  old token and secret stop working at once for every project the bot serves, so a task running
  on the runner cannot report any more and ends when its lease expires: rotate when no task is
  running. New deliveries wait up to one hour for the runner to use the new configuration.
- **Remove connection**: deletes the connection and the bot's participation in the project. If
  it was the bot's last project, the token is revoked, the managed webhook is deleted and the bot
  leaves the workspace; its comments, votes and task history are kept.

### What a connection bot may do

A connection bot is a workspace member, never an administrator or owner: its seat role cannot be
changed and its token cannot be revoked on the members page (it shows
**Managed by project connection**; change it in the project's Execution settings). It is an AI
participant of each connected project with the governance level of a voter.

Its token is limited to what a runner needs, in the projects where the bot has an enabled
connection: the task protocol and its own resources (notifications, uploads, preferences, personal
Flow space), and project resources (issues, comments, forms, records, proposals, Flow objects and
the like) whose project has such a connection. Administration routes and workspace-wide listings
are refused. Every refusal answers 403 `this bot may act only in projects it is connected to`. A
task that was running when its connection was disabled can still report how it ended while its
token works. Token bots and bot users created by an administrator are not limited this way.

On the runner the CLI agent runs as the runner's OS user with that user's permissions; read the
runner's [security notes](../../apps/runner/SECURITY.md) before connecting a bot.

## Bots without a connection

A token bot (or a bot user created by an administrator) receives its tasks through a workspace
webhook whose bot user it is. Point that webhook at a runner and configure an `openclaw`,
`openprx`, `webhook` or `custom` agent for the bot to notify someone or forward the task
([Using each agent type](../../apps/runner/README.md#using-each-agent-type)). The runner ends such
a task once the hand-over is done, with the bot's token; a bot user without a token cannot end
its tasks, so they end when the lease expires.

Sylvode gives a bot the agent type `openclaw`, `webhook` or `custom` (created by an administrator)
or `mcp` (token and connection bots). The runner's agent types are `runner`, `openclaw`,
`openprx`, `webhook` and `custom`, so no bot carries `openprx` or `runner`: select those agents by
bot id, name or route.
