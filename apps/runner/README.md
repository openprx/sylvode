# Sylvode runner

`sylvode-runner` receives bot tasks from a [Sylvode](https://github.com/openprx/sylvode)
deployment and hands each one to exactly one configured **agent**. There are five agent types.
Only one of them, `runner`, runs a CLI agent itself; the other four hand the task over to
something else and are done.

| Agent type | What it does with a task | Runs a CLI agent |
|---|---|---|
| `runner` | Runs `claude`, `codex` or `opencode` in the project's working directory, in a tmux session or as a one-off headless process, as the project's execution settings in Sylvode say. The CLI reads and ends the task through the Sylvode MCP server. | yes |
| `openclaw` | Notification: sends a rendered message with the OpenClaw CLI (`openclaw message send ...`). | no |
| `openprx` | Notification: sends a rendered message through an OpenPRX Signal API, or through a command. | no |
| `webhook` | Forwarding: posts the delivery JSON, unchanged, to another HTTP endpoint. | no |
| `custom` | Notification or forwarding: runs one program of your choice with the rendered message as an argument. | no |

The runner is built from `apps/runner` of the Sylvode repository and carries the Sylvode version
(`sylvode-runner --version`, image tags and archive names). It runs on Linux and macOS only: it
stops executors as process groups and handles `SIGTERM`. On any other target the executable
prints `sylvode-runner is not supported on this platform` and exits 1.

For what a bot is, how tasks arise in Sylvode and how they end there, see the user guide
[Bots and agent work](../../docs/guide/agents.md). For the security posture of this service, see
[SECURITY.md](SECURITY.md).

## Contents

- [How it works](#how-it-works)
- [Install](#install)
- [Configure](#configure)
- [Using each agent type](#using-each-agent-type)
- [Choosing the agent for a task](#choosing-the-agent-for-a-task)
- [`runner` agents](#runner-agents)
- [Ending tasks](#ending-tasks)
- [Forwarding agents in detail](#forwarding-agents-in-detail)
- [Configuration reference](#configuration-reference)
- [Operator commands](#operator-commands)
- [HTTP interface](#http-interface)
- [Running the service](#running-the-service)
- [Tunnel client](#tunnel-client)

## How it works

```
                                             ┌─▶ runner    ─▶ claude / codex / opencode
                                             │               (tmux session or headless run,
Sylvode ── signed POST /webhook ─▶ sylvode-runner             reads and ends the task through
 (worker AI task, or a webhook               │                the Sylvode MCP server)
  delivery with bot_context)                 ├─▶ openclaw  ─▶ OpenClaw CLI (message)
                                             ├─▶ openprx   ─▶ Signal API or command (message)
                                             ├─▶ webhook   ─▶ another HTTP endpoint (JSON)
                                             └─▶ custom    ─▶ any one program (message)
```

1. Sylvode sends a signed HTTP `POST` to `/webhook`: either a **worker AI task** (a body with
   `task_id` and `ai_participant_id`, sent by the Sylvode worker to the bot the task is assigned
   to) or a **webhook delivery** for one of the events a webhook subscribes to.
2. The runner verifies the HMAC-SHA256 signature and refuses replays.
3. It acts only on bot tasks: a worker AI task, or a webhook delivery whose
   `bot_context.is_bot_task` is `true`. Anything else is answered
   `{"status":"ignored","reason":"not_bot_task"}`.
4. It selects exactly one agent ([Choosing the agent for a task](#choosing-the-agent-for-a-task))
   and dispatches to it.
5. A worker AI task is open in Sylvode until it is ended through the task protocol. A `runner`
   agent's CLI ends it itself; for the other agent types the runner ends it when the hand-over
   is done ([Ending tasks](#ending-tasks)).

A webhook can subscribe to these events:

| Group | Events |
|---|---|
| Issues | `issue.created`, `issue.updated`, `issue.assigned`, `issue.deleted`, `issue.state_changed` |
| Comments | `comment.created`, `comment.updated`, `comment.deleted` |
| Labels | `label.added`, `label.removed` |
| Sprints | `sprint.started`, `sprint.completed` |
| AI tasks | `ai.task_completed`, `ai.task_failed` |

A webhook delivery carries `bot_context` when the webhook has a bot user and that bot is an
assignee of the issue the event is about, or, for `comment.created`, is mentioned in the comment.
Assigning an issue to a bot and mentioning a bot in a comment arrive as worker AI tasks
(`trigger_kind` `assigned` or `mention`), so the webhook deliveries of that same request arrive
without `bot_context` and are answered `not_bot_task`. Other activity on an issue assigned to a
bot, by a person or another bot (a state change, an edit, a comment), arrives as a webhook
delivery with `bot_context`. A bot's own actions never come back to it as a task. The guide's
[How tasks arise](../../docs/guide/agents.md) section has the full list.

## Install

### Release archive

Each Sylvode release ships `sylvode-runner-<platform>.tar.gz` with a `.sha256` file for
`linux-amd64`, `linux-arm64`, `macos-amd64` and `macos-arm64`. The archive contains the
`sylvode-runner` executable.

```bash
sha256sum -c sylvode-runner-linux-amd64.tar.gz.sha256
tar xzf sylvode-runner-linux-amd64.tar.gz
sudo install -m 0755 sylvode-runner /usr/local/bin/
```

`runner` agents also need, on the same host and on the service's `PATH`: tmux 3.2 or later (for
the tmux channel), the CLIs you enable (`claude`, and `codex` or `opencode` if you turn them
on), and the Sylvode MCP server executable `mcp-server` from the Sylvode server archive (see
[Install](../../docs/install.md)). The forwarding agent types need only the programs they run.

### Container image

```bash
docker run --init --stop-timeout 45 -p 9090:9090 \
  -v /srv/sylvode-runner/config.toml:/etc/sylvode-runner/config.toml:ro \
  ghcr.io/openprx/sylvode-runner:<sylvode version>
```

The image runs `/app/sylvode-runner /etc/sylvode-runner/config.toml` as the non-root user
`sylvode` (uid 10001), so the mounted file must be readable by uid 10001, and `listen` must not
be loopback inside the container (use `0.0.0.0:9090`, with a secret). Run it with `--init`
([Stopping commands](#stopping-commands)) and `--stop-timeout 45` ([Shutdown](#shutdown)).

The image contains only the runner and CA certificates: no tmux, `claude`, `codex`, `opencode`,
`openclaw` or `mcp-server`. Use it for the forwarding agent types (`webhook`, `openprx` with
`signal_api`), or derive an image that adds the programs your agents run. With a `runner` agent
configured, `doctor` reports the `tmux` row as failed (`unsupported in container image`): run
`runner` agents on a host.

The repository's `docker-compose.yml` has an optional `runner` service (profile `connectors`)
that mounts `config/sylvode-runner.example.toml`; see [Install](../../docs/install.md).

### From source

`cargo build --release -p sylvode-runner` in the repository root builds `target/release/sylvode-runner`;
see [Install](../../docs/install.md) for the toolchain.

### Installer (systemd user service or launchd agent)

```bash
apps/runner/scripts/install.sh --binary-path /usr/local/bin/sylvode-runner --config-dir /srv/sylvode-runner
apps/runner/scripts/uninstall.sh
```

The installer creates the user service `sylvode-runner.service` on Linux or the launch agent
`dev.sylvode.runner` on macOS. Without `--binary-path` it looks for `sylvode-runner` in the
working directory, in `./target/release` and in the workspace `target/release`. The unit or agent
is rewritten only when its content differs; the configuration file is never written. The
uninstaller removes the unit or agent.

Before anything is installed, the installer runs `sylvode-runner doctor --mode auto --as-service`
with the `PATH` the service will have. A failed row prints its hint and stops the installation.
tmux is required whenever the configuration has a `runner` agent (a project may choose the tmux
channel at any time). `--dry-run` stops after the check. On Linux a user without linger gets a
warning with `loginctl enable-linger <user>`: without linger the service and its tmux server stop
at logout. After installing, doctor runs once more in the service's context (`systemd-run --user`
on Linux; `launchctl asuser` on macOS, which works for root only, so a normal user gets a
warning instead); a problem there is a warning.

### systemd system unit

```ini
[Unit]
Description=Sylvode runner
After=network.target

[Service]
# A dedicated unprivileged user: CLI agents and every command run as this user.
User=sylvode-runner
Group=sylvode-runner
ExecStart=/usr/local/bin/sylvode-runner /srv/sylvode-runner/config.toml
WorkingDirectory=/srv/sylvode-runner
Restart=always
# Running headless runs are stopped and reported on SIGTERM; see Shutdown.
TimeoutStopSec=90

[Install]
WantedBy=multi-user.target
```

When the runner runs under systemd and has to start a tmux server, it starts the server through
`systemd-run --user --scope --unit=sylvode-tmux-default` (`sylvode-tmux-<socket>` with
`[runner.tmux] socket_name`), so the server and its sessions outlive a restart of the service.

## Configure

`sylvode-runner [CONFIG]` reads `config.toml` in the working directory when no path is given.
`sylvode-runner config check [--config PATH]` says whether a file loads and names the key (and
table, and line) of each problem without printing a value. Every table refuses keys it does not
know, naming the key. Two annotated examples ship with the repository:

- [`config.example.toml`](config.example.toml): secure defaults (loopback, signatures required,
  no agent). Start-up is refused until its placeholder secret is replaced.
- [`../../config/sylvode-runner.example.toml`](../../config/sylvode-runner.example.toml): the
  file the compose `runner` service mounts.

### A complete example

One runner that serves five bots, one per agent type. Each bot is a user in the Sylvode
workspace; its id is the bot's user id (a UUID). Replace every value with your own.

```toml
[server]
# Behind a TLS reverse proxy that forwards https://runner.example.com/webhook here.
listen = "127.0.0.1:9090"

[security]
# The secret of the workspace webhook(s) that carry the forwarding bots' tasks.
webhook_secrets = ["4c1f9e0a7b2d4e6f8a1b3c5d7e9f0a2b"]
allow_unsigned = false
max_delivery_age_secs = 300

[runtime]
cli_max_concurrency = 1
http_timeout_secs = 15

# The bots' tokens: a runner agent reads and ends its tasks with them; for the other agent types
# the runner ends each task once the hand-over is done.
[ai_task_callback]
enabled = true
api_url = "https://sylvode.example.com"

# "Build bot", connected to a project in Sylvode: this entry comes from the configuration block
# the connection issues.
[[ai_task_callback.bots]]
bot_id = "6f2b8c1e-3d4a-4b5c-9e8f-7a6b5c4d3e21"
workspace_id = "0d9c8b7a-6f5e-4d3c-8b2a-1f0e9d8c7b6a"
token = "opr_build_bot_token"
surface = "mcp_stdio"
webhook_secret = "9b8a7c6d5e4f3a2b1c0d9e8f7a6b5c4d"

# The four bots of the forwarding agents: token bots (workspace settings, Members and bots,
# API Tokens); each token is its own bot user.
[[ai_task_callback.bots]]
bot_id = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"
token = "opr_notify_bot_token"
surface = "rest"

[[ai_task_callback.bots]]
bot_id = "2b3c4d5e-6f7a-4b8c-9d0e-1f2a3b4c5d6e"
token = "opr_signal_bot_token"
surface = "rest"

[[ai_task_callback.bots]]
bot_id = "3c4d5e6f-7a8b-4c9d-8e1f-2a3b4c5d6e7f"
token = "opr_review_bot_token"
surface = "rest"

[[ai_task_callback.bots]]
bot_id = "4d5e6f7a-8b9c-4d0e-9f2a-3b4c5d6e7f80"
token = "opr_log_bot_token"
surface = "rest"

[runner]
# Project working directories must lie below one of these.
allowed_roots = ["/srv/work"]

# runner: runs a CLI agent for Build bot's tasks.
[[agents]]
id = "6f2b8c1e-3d4a-4b5c-9e8f-7a6b5c4d3e21"
name = "Build bot"
agent_type = "runner"

[agents.route]
bot_ids = ["6f2b8c1e-3d4a-4b5c-9e8f-7a6b5c4d3e21"]

# openclaw: a chat notification for Notify bot's tasks.
[[agents]]
id = "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"
name = "Notify bot"
agent_type = "openclaw"
message_template = "[{project}] {event}: {key} {title}\n{actor} | {reason}"

[agents.openclaw]
command = "openclaw"
args = ["message", "send"]
channel = "signal"
target = "uuid:00000000-0000-4000-8000-000000000001"

# openprx: a Signal message through a signal-cli HTTP API for Signal bot's tasks.
[[agents]]
id = "2b3c4d5e-6f7a-4b8c-9d0e-1f2a3b4c5d6e"
name = "Signal bot"
agent_type = "openprx"

[agents.openprx]
signal_api = "http://127.0.0.1:8686"
account = "+15550100"
target = "+15550101"

# webhook: Review bot's tasks go, unchanged, to a review service that takes them from here.
[[agents]]
id = "3c4d5e6f-7a8b-4c9d-8e1f-2a3b4c5d6e7f"
name = "Review bot"
agent_type = "webhook"

[agents.webhook]
url = "https://review.example.com/hooks/sylvode"
secret = "5e4d3c2b1a0f9e8d7c6b5a4f3e2d1c0b"

# custom: Log bot's tasks are written to the system log.
[[agents]]
id = "4d5e6f7a-8b9c-4d0e-9f2a-3b4c5d6e7f80"
name = "Log bot"
agent_type = "custom"
message_template = "{event} {key} {title}"

[agents.custom]
command = "logger"
args = ["-t", "sylvode", "--", "{message}"]
```

Notes on this example:

- Each forwarding agent's `id` is its bot's user id and its `name` is the bot's display name. A
  worker AI task names the bot by id (`ai_participant_id`) and a webhook delivery names it by
  name (`bot_context.bot_name`), so an agent set up this way is chosen for both.
- A runner that serves more than one bot verifies a delivery for a bot that has a
  `webhook_secret` with that secret only; deliveries for the other bots are verified with
  `security.webhook_secrets`. With more than one `runner` bot, every runner bot needs its own
  `webhook_secret`.
- `logger`, `openclaw` and the Signal API must exist on the host; the runner starts degraded, not
  refused, when a program is missing.

### Connecting Sylvode to the runner

There are two ways a bot's tasks reach this runner.

- **A bot connected to a project (for `runner` agents).** In Sylvode, connecting a new bot to a
  project (the project's **Execution** settings) creates the bot, its `mcp_stdio` token, a
  signing secret and a managed webhook that points at `<runner address>/webhook`, and shows a
  configuration block once (Sylvode keeps no copy; rotating the credentials issues a new one).
  The block holds `[server]`, `[security]`, `[ai_task_callback]` with one
  `[[ai_task_callback.bots]]` entry, `[runner]` with `allowed_roots`, `[runner.cli.codex]` or
  `[runner.cli.opencode]` with `enabled = true` when the connection uses that CLI, and an
  `[[agents]]` entry with `agent_type = "runner"`. See
  [Connecting a bot to a project](../../docs/guide/agents.md) for the steps in the web interface.
  To add a further connection's block to a runner that already serves a `runner` bot, append its
  `[[ai_task_callback.bots]]` and `[[agents]]` tables, put its signing secret in that bot's
  `webhook_secret` (the runner refuses to start otherwise), give each `runner` agent its own
  `allowed_roots` (they must not overlap), and keep the other sections once.
- **A workspace webhook with a bot user (for the other agent types).** Create a webhook in the
  workspace whose URL is the runner's `/webhook` (for example
  `https://runner.example.com/webhook`), select the events, and choose the bot user whose tasks
  this runner handles. Put the webhook's secret in `security.webhook_secrets`. If you leave the
  secret empty when you create the webhook, Sylvode generates one and returns it once in the
  create response (`POST /api/v1/workspaces/{workspace_id}/webhooks`, field `secret`);
  `POST /api/v1/workspaces/{workspace_id}/webhooks/{webhook_id}/rotate-secret` issues a new one the
  same way. Without a bot user, deliveries carry no `bot_context` and the worker has nowhere to
  deliver that bot's AI tasks. Webhooks are described in [Webhooks](../../docs/integrations/webhooks.md).
  The bot user is either a token bot (workspace settings, **Members and bots**, **API Tokens**:
  every token is its own bot user, agent type `mcp`) or a bot user an administrator created
  (agent type `openclaw`, `webhook` or `custom`). Only a token bot has a token, so only a token
  bot's tasks can be ended by the runner ([Ending tasks](#ending-tasks)); the tasks of a bot
  without one are ended by the worker's processing timeout.

## Using each agent type

### `runner`: run a CLI agent

1. In Sylvode, open the project's **Execution** settings, choose **Connect a bot**, and set the
   connection: a new or connected bot, the runner address, the CLI (`claude`, `codex` or
   `opencode`), the channel (`tmux` or `headless`; tmux accepts `claude` only) and the working
   directory on the runner host. Copy the configuration block it shows once.
2. Merge the issued block into the runner's configuration and check `[runner] allowed_roots`: the
   working directory must lie below one of them.
3. Install the CLI and log it in as the runner's OS user, install `mcp-server`, and run
   `sylvode-runner doctor`.
4. Start the runner. Sylvode holds the bot's tasks until the runner first uses the new token
   (its start-up handshake does), for at most one hour.
5. Assign an issue to the bot or mention it in a comment. On the tmux channel the task is typed
   into the session `<KEY>-claude`; attach with `tmux attach -t <KEY>-claude` to watch. On the
   headless channel the CLI runs once and exits.

[`runner` agents](#runner-agents) describes the checks, the sessions and the timers.

### `openclaw`: send a chat notification

```toml
[[agents]]
id = "<bot user id>"
name = "<bot display name>"
agent_type = "openclaw"
message_template = "[{project}] {event}: {key} {title}"

[agents.openclaw]
command = "openclaw"           # one program name, no arguments
args = ["message", "send"]     # placed between command and --channel
channel = "signal"
target = "uuid:<recipient>"
```

The runner runs `openclaw message send --channel <channel> --target <target> --message <message>`.
The OpenClaw CLI must be installed and configured for the runner's OS user. The task is complete
in Sylvode once the command exits with status 0.

### `openprx`: send a Signal message

```toml
[[agents]]
id = "<bot user id>"
name = "<bot display name>"
agent_type = "openprx"

[agents.openprx]
signal_api = "http://127.0.0.1:8686"   # a signal-cli HTTP API
account = "+15550100"                  # the sending account
target = "+15550101"                   # the recipient
# Without signal_api, a command is run instead, like openclaw:
# command = "openprx"
# args = ["message", "send"]
# channel = "signal"                   # the default
```

With `signal_api` the runner posts `{"recipients": [target], "message": message}` to
`<signal_api>/api/v1/send/<account>`; without it, it runs `command` with
`--channel <channel> --target <target> --message <message>`. One of the two is required.

### `webhook`: forward the task to another service

```toml
[[agents]]
id = "<bot user id>"
name = "<bot display name>"
agent_type = "webhook"

[agents.webhook]
url = "https://review.example.com/hooks/sylvode"
secret = "<shared secret>"   # optional: adds X-Webhook-Signature: sha256=<hex>
```

The delivery JSON is posted unchanged; no message is rendered, so `message_template` has no
effect. The task is complete in Sylvode once the endpoint answers 2xx: from then on the receiving
service owns the work. If it needs to report back, give it the bot's own token and the Sylvode API.

### `custom`: run any program

```toml
[[agents]]
id = "<bot user id>"
name = "<bot display name>"
agent_type = "custom"
message_template = "{event} {key} {title}"

[agents.custom]
command = "logger"                         # one program name, no shell
args = ["-t", "sylvode", "--", "{message}"]
```

Every argument that contains `{message}` has it replaced by the rendered message and stays one
argument; without `{message}` in any argument, the message is appended as the last argument. The
task is complete in Sylvode once the program exits with status 0.

## Choosing the agent for a task

A bot task goes to exactly one agent, or to none. The **bot key** is `bot_context.bot_name` (or
`bot_context.bot_id` when the name is empty) for a webhook delivery, and `ai_participant_id` (the
bot's user id) for a worker AI task. The first match in this order wins, in configuration order
within each step:

1. an agent whose `id` equals the bot key, or whose `name` equals it ignoring case, and whose
   route matches;
2. otherwise an agent whose `agent_type` equals the bot's agent type and whose route matches. The
   bot's agent type is `bot_context.bot_agent_type` on a webhook delivery and
   `ai_participant_agent_type` on a worker AI task (never the task's inner `payload`). Sylvode
   gives a bot one of the agent types `openclaw`, `webhook` or `custom` (bots an administrator
   creates) or `mcp` (token bots and connection bots), so this step can pick an `openclaw`,
   `webhook` or `custom` agent but never a `runner` or `openprx` one;
3. otherwise an agent that has an `[agents.route]` table and whose route matches.

When nothing matches, the answer is `{"status":"no_agent", ...}`, nothing runs, and a worker AI
task is failed ([Ending tasks](#ending-tasks)).

### Routes

An agent without `[agents.route]` matches every task. A route matches when every non-empty list
contains the task's value, compared ignoring case and surrounding whitespace; a value the task
does not carry fails a non-empty list. An omitted or empty list restricts nothing. `bot_names`
and `bot_ids` together form one constraint: with neither set it restricts nothing; with one or
both set, the task's bot must match an entry of a list that is set. A route with only
`bot_ids = ["<id>"]` therefore rejects every other bot.

| Key | Compared with |
|---|---|
| `bot_names`, `bot_ids` | The bot's name and id (see above). A `runner` agent serves the bots in `bot_ids`, or the bot whose id is the agent's `id` when `bot_ids` is empty; they must be UUIDs. |
| `bot_agent_types` | The bot's agent type (see step 2). |
| `trigger_kinds` | `bot_context.trigger_reason` (or `bot_context.trigger_kind`) on a webhook delivery; the top-level `trigger_kind` on a worker AI task. |
| `events` | The delivery's `event`. A worker AI task has none, so a route with `events` never matches one. |
| `project_types` | A `project_type` field. Sylvode sends one only in the payload of a worker AI task for a bot mentioned in a comment. |
| `form_keys` | A `form_key` field, which no Sylvode bot task carries; a non-empty list matches nothing. |

`trigger_kinds` accepts only the values Sylvode sends; any other value refuses start-up, naming
the agent and listing the valid values:

| Source | Values |
|---|---|
| Webhook delivery (`bot_context.trigger_reason`, by event) | `created` (`issue.created`), `status_changed` (`issue.state_changed`), `mentioned` (`comment.created`, also for a comment on an issue assigned to the bot that does not mention it), `completed` (`ai.task_completed`), `failed` (`ai.task_failed`), `assigned` (every other event) |
| Worker AI task (`trigger_kind`) | `assigned` (assignment), `mention` (comment mention or review request), `proposal_vote` (vote request), `manual` (anything else) |

### The issue a worker AI task is about

The issue that a forwarding agent's message names (`{issue_id}`, `{key}`) depends on the task's
`reference_type`:

| `reference_type` | Queued by Sylvode when | Issue |
|---|---|---|
| `work_item` | an issue is created with, or reassigned to, the bot | `reference_id`; `payload.issue_id` is ignored |
| `comment` | a comment mentions the bot | `payload.issue_id` (`reference_id` is the comment) |
| `proposal`, none, other | a proposal opens for voting, or a task created by hand | none |

A workspace owner or administrator can create a task by hand
(`POST /api/v1/projects/{project_id}/ai/tasks`) with a reference and payload of their choosing, so
for such a task the issue is whatever they wrote. Only the Sylvode API can check, when the bot writes, that the bot may write to that issue.

## `runner` agents

A `runner` agent runs the task's CLI as the runner's OS user, in the project's working directory,
on the channel the project's execution settings in Sylvode choose for the bot's connection:

- **tmux**: the task is typed into the project's tmux session, where a long-running `claude`
  works on it. The session belongs to the operator ([tmux sessions](#tmux-sessions)).
- **headless**: the CLI runs once for the task and exits ([Headless runs](#headless-runs)).

The CLI reads the task and ends it itself with the Sylvode task tools (`sylvode_task_get`,
`sylvode_task_ack`, `sylvode_task_progress`, `sylvode_task_complete`, `sylvode_task_fail`)
through the Sylvode MCP server, which the runner configures per bot. Everything the CLI does
through the `sylvode` tools is done as the connection's bot.

### Requirements, checked at start-up

A configuration with a `runner` agent is refused, with a message naming the agent or bot, unless:

- `[ai_task_callback] enabled = true`, and an `api_url` that is `https://` (or `http://` to a
  loopback host);
- every bot the agent serves (its `route.bot_ids`, else its `id`) is a UUID and has an
  `[[ai_task_callback.bots]]` entry with a token registered for `mcp_stdio`
  (`bot <id>: runner bots need a token registered for mcp_stdio`) and a `workspace_id`, the UUID
  of the bot's workspace (the MCP server the runner starts needs it);
- the agent has no `[agents.openclaw]`, `[agents.openprx]`, `[agents.webhook]` or
  `[agents.custom]` table;
- with more than one `runner` agent, each has its own `allowed_roots`;
- with more than one `runner` bot, every runner bot's entry has a `webhook_secret`.

Other problems start the service but make every `runner` agent refuse its tasks with
`runner_agent_disabled` while the other agents keep working: an invalid allowed root, two agents
whose `allowed_roots` overlap, a state directory inside, equal to or above an allowed root, a
configuration file inside an allowed root, or a state directory that cannot be used. The log says
why. A second runner on the same state directory refuses to start (`runner.lock`).

### Allowed roots

`allowed_roots` (in `[runner]`, or per `runner` agent) lists the directories below which a project
working directory may lie. Each entry is checked once at start-up: only a leading `~/` is
expanded; the path must be absolute, exist and be a directory, and is canonicalised. Refused:
`/`, the home directory or a directory that contains it, a system directory (`/etc`, `/usr`,
`/bin`, `/System`, `/Library`, `/private/etc`, the Homebrew prefix and similar) or anything inside or
around one, a top-level data directory itself (`/home`, `/Users`, `/srv`, `/tmp`, `/var` and
similar; a directory below them is fine), and any path containing `#` (tmux reads a directory as
a format). A world-writable root is warned about.

An empty `allowed_roots` is valid: no session is then created and no headless run started, but
sessions that already exist are served. Allowed roots keep one project from naming another
directory; they are not a sandbox.

### Delivery checks

Before it answers, the runner checks each delivery to a `runner` agent, in this order. A refusal
is answered `{"status":"refused","reason_code":"...","reason":"...","agent":"..."}` and the reason
never contains a path (the path is in the runner log at `info`).

| Step | Refusal (`reason_code`) |
|---|---|
| The delivery's bot is one the agent serves, the runner holds a usable token and workspace for it, safe mode is off, and the session registry is usable | `runner_agent_disabled` |
| The `protocol` field equals the runner's task protocol (1), and the start-up handshake found no mismatch | `protocol_mismatch` |
| The allowed roots are valid | `runner_agent_disabled` |
| `task_id` and `project_id` are UUIDs | `delivery_mismatch` |
| The task view, fetched with the bot's token within `delivery_check_timeout_secs`. A timeout or a 5xx is answered HTTP `503` so the worker re-queues; an attempt that is not active is answered `ignored`; a token Sylvode refuses is `runner_agent_disabled`; any other refusal is `delivery_mismatch` | see left |
| The delivery matches Sylvode's record (task id, project id, project key) | `delivery_mismatch` |
| The connection's execution settings exist and are enabled | `execution_disabled` |
| The project key matches `^[A-Z0-9]{1,32}$` | `invalid_project_key` |
| No other project on this runner uses the key | `session_name_collision` |
| The channel is `tmux` or `headless` | `delivery_mismatch` |
| tmux channel: the CLI is `claude` | `cli_unverified` |
| tmux channel: tmux is found and at least `[runner.tmux] min_version`; the session can be looked up | `tmux_unavailable` |
| The working directory, unless the tmux session already exists (always for a headless run) | `path_outside_allowlist`, `path_missing`, `path_unreadable`, `path_not_directory` |
| The CLI is enabled (`[runner.cli.codex] enabled`, `[runner.cli.opencode] enabled`; claude always is) | `cli_disabled` |
| The CLI and `mcp-server` are found | `cli_unavailable` |
| The task view can be turned into a task message | `delivery_mismatch` |

The working directory is checked syntactically, then for lying below an allowed root as written,
without touching the file system (so a refusal says nothing about the host outside the roots),
then canonicalised (every symbolic link resolved) and compared again by path components
(`/srv/work-other` is not below `/srv/work`). Only the canonical path is used from then on. Once
every check has passed, the project key is bound to the project on this runner;
`sylvode-runner release KEY` frees it.

An accepted delivery is answered `dispatched` with the channel (`tmux` with the session name and
the queue position, or `headless`).

### tmux sessions

A project's tmux connection has one session, `<KEY>-<cli>`: the project key exactly as Sylvode
stores it, a hyphen, and the CLI. The tmux channel accepts `claude` only (codex and opencode are
refused with `cli_unverified`), so the session is `<KEY>-claude`. The session lives on the
operator's default tmux server (`[runner.tmux] socket_name = ""`), with the operator's
`tmux.conf`, environment and login. Attach with `tmux attach -t <KEY>-claude`.

On every task the runner:

- **uses the session if it exists, whoever started it.** The one pane whose foreground command is
  one of `[runner.cli.claude] process_names` receives the task. A session whose CLI is not running
  (a shell is left, or the pane is dead) fails the task, retryably, with
  `the CLI is not running in tmux session <name>; attach with tmux attach -t <name> and start it,
  or end the session so the runner starts it`. More than one pane running the CLI fails it without
  a retry. A session the runner recorded for another bot is refused
  (`session_bot_mismatch: tmux session <name> serves another bot`).
- **creates the session if it does not exist**, with the launch command
  (`new-session -d -s <name> -n cli -x <window_width> -y <window_height> -c <working directory> --
  <launch command>`), in the connection's working directory, which must then lie inside the allowed
  roots. When the CLI exits, tmux ends the session and the next task creates it again. A change of
  directory or CLI in Sylvode takes effect the next time the runner creates the session.
- **never kills, restarts, respawns, renames or resizes** a session, window or pane, sets no tmux
  option, and never types a launch command into a shell. Sessions keep running when the runner
  stops. The only tmux operations are reads, `new-session`, `load-buffer`/`paste-buffer` of the
  task text and `send-keys Enter`, addressed to the exact session name and to the CLI's pane by
  pane id and process id, checked at every push.

The runner records each session it uses in `<state_dir>/sessions/<KEY>-claude/session.json`
(bot, pane id and process id, directory, who started it).

A session the operator starts gets the Sylvode MCP server and the hooks through
[`setup claude`](#setup-claude-for-a-session-you-start-yourself). The first start of the CLI in a
new directory or account shows dialogs (trusting the folder, the bypass-permissions confirmation,
login). The runner answers none: it fails the waiting task, retryably, with
`the CLI in tmux session <name> shows a dialog; attach with tmux attach -t <name> and answer it`
(or `... is not logged in; attach and log in`), leaves the session running, and the next attempt
goes in after you answered.

### `setup claude` for a session you start yourself

```bash
sylvode-runner setup claude --project KEY [--dir D] [--bot ID] [--apply] [--config PATH]
```

`setup` gives a claude you start yourself the same Sylvode MCP server and hooks that a session
the runner creates gets. It never touches a tmux session.

- Without `--apply` it prints and writes nothing: the session name, a
  `tmux new-session -d -s <KEY>-claude -c <dir>` line, the full launch line, the MCP server entry
  (it names the file that holds the bot's token, never the token) and the hooks.
- With `--apply` it runs `claude mcp add-json --scope local sylvode ...` in the directory (an
  existing `sylvode` entry that points elsewhere is reported and left alone, exit status 2) and
  merges the hooks and the permissions for the Sylvode tools into
  `<dir>/.claude/settings.local.json`. Hooks the runner writes carry `"sylvode-runner": 1`, so a
  second run replaces only those; every other key stays. Restart your claude afterwards: MCP
  servers and hooks are read when it starts.

`--dir` defaults to the directory recorded for the project's session; `--bot` is needed only when
the runner serves several bots and has no session record for the project.

### Typing a task

A task is typed only when the CLI is idle, its prompt is on the screen and empty, and no attached
client typed within `[runner.tmux] user_idle_secs`. One task at a time per session; the others
wait in the session's queue and report `runner.queued` every `heartbeat_secs`. Before typing, the
runner checks that Sylvode still shows the task active, and records the push in `session.json`
so a restart never types a task twice. The text is pasted, then Enter is pressed after
`push_settle_ms`. If the turn does not start, Enter is pressed once more, only if the pasted text
is still on the input line; otherwise the session is reported blocked.

What the runner reads from the screen (busy, a choice, a dialog) comes from the CLI's own
interface at the bottom of the pane, never from the transcript above it, so text in an issue
cannot pose as a marker.

The task message starts with `[Sylvode task] id=<task id> attempt=<n>/<max>`, names the issue or
proposal, and tells the agent to call `sylvode_task_get`, `sylvode_task_ack`,
`sylvode_task_progress` and to end with `sylvode_task_complete` or `sylvode_task_fail`. While a
task is typed and not ended, input in the session is the operator's responsibility.

### Hooks and turn tracking

Turn events come from claude's hooks (`SessionStart` for `startup`, `UserPromptSubmit`, `Stop`,
and `Notification` for permission prompts), which run
`sylvode-runner hook --cli claude --event <event> --socket <state_dir>/hook.sock`. They are in the
settings file of a session the runner creates (`--settings`, unless `[runner.cli.claude] hooks =
false`) or, for a claude you start, in `<dir>/.claude/settings.local.json` written by `setup`.
The hook command holds no secret: it sends the event with the tmux socket path and pane id it
reads from `TMUX` and `TMUX_PANE`, and does nothing outside tmux. The runner admits a hook message
only from its own OS user, from a pane a session recorded, and, where the platform names the
sender, from a process below the recorded pane process (Linux reads `/proc`; macOS reads one
`ps -A` snapshot per admitted message).

A session starts in screen mode; the first admitted hook from its pane switches it to hook mode.
Without hooks, turns are read from the screen (a changing screen is a working CLI, a still screen
with an empty prompt an idle one): turn start and end are less precise and the permission-prompt
signal is lost, but typing still waits for an idle, empty prompt.

### Timers of a tmux task

| Situation | What happens | Setting |
|---|---|---|
| The runner created the session and the CLI is not ready | the first waiting task fails, retryably (`cli did not become ready within <n> s`) | `ready_timeout_secs` (90) |
| A dialog shows while the CLI starts | the waiting tasks fail with the dialog's reason | |
| The CLI is idle after a push and has not acknowledged the task | a reminder is typed (`[Sylvode reminder] ... is not acknowledged`) | `ack_reminder_secs` (180) |
| Still not acknowledged after the reminder's turn, or since the push | the task fails, retryably (`agent did not acknowledge the task`) | `report_unacked_after_secs` (900) |
| Acknowledged, a turn ended, the task is still open | after the grace period a reminder is typed (`... is still open`); if the next turn ends with the task open, the task fails, not retryably | `completion_grace_secs` (60) |
| The CLI waits for a permission decision or dialog | the task is reported `runner.blocked`, and fails, not retryably, after 5 minutes; the session is kept | |
| The screen reads busy or waiting while the hooks say idle | the waiting tasks fail, retryably, after the hold | `screen_hold_secs` (600) |
| The session or the CLI ends while a task runs | the task fails, retryably | |
| While a task runs | `runner.alive` progress | `heartbeat_secs` (600) |

### Headless runs

The CLI runs once in the canonical working directory, as the runner's OS user. At most
`[runtime] cli_max_concurrency` headless runs go at once; a task waits up to 30 minutes for a run
slot and is then failed, retryably. The directory is checked again right before the process
starts. The runner reports `runner.pushed`, then `runner.alive` every `heartbeat_secs` while the
CLI runs. A run is stopped after one hour. When the process ends and Sylvode still shows the task
active, the runner fails it with `source: "runner"`: not retryable when the agent ended on its own
(`agent ended without completing or failing the task`), retryable when the runner cut the run
short by its time limit or shutdown.

### Command lines

| Channel and CLI | Command |
|---|---|
| tmux, claude (a session the runner creates) | the words of `[runner.cli.claude] launch_command`, then `--mcp-config <state_dir>/mcp/<bot>.claude.json` and, with `hooks = true`, `--settings <state_dir>/sessions/<KEY>-claude/claude-settings.json` |
| headless, claude | `claude -p <prompt> <headless_args...> --mcp-config <file> --allowedTools <the Sylvode tools>` |
| headless, codex | `codex exec --skip-git-repo-check -c mcp_servers.sylvode.command=... -c mcp_servers.sylvode.args=... <headless_args...> <prompt>` |
| headless, opencode | `opencode run <prompt>` |

In `launch_command`, a word that is exactly `{mcp_config}` or `{settings}` is replaced by that
file's path, and nothing is then appended for that file. `--mcp-config` and `--settings` add to
the operator's own MCP servers and settings; they restrict nothing. The headless claude allow
list holds `sylvode_overview`, `sylvode_search`, `sylvode_read` and the five task tools as
`mcp__sylvode__<tool>`, plus `mcp__sylvode__sylvode_write` with `[runner.mcp] allow_write_call`.
opencode gets no MCP configuration from the runner: it follows its own configuration, and the
start-up log warns that its permissions are not controlled by the runner.

The environment of a headless run, and of a tmux server the runner starts (and so of a session it
creates), is the runner's own minus `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT` and `TMUX`: the CLI has
the operator's configuration and login. The runner's other tmux calls get an allow-listed
environment. The per-bot MCP files are written at start-up and on every delivery, so an operator
can start a session before the first task.

### State directory

| Path (below `state_dir`) | Content |
|---|---|
| `runner.lock` | one runner per state directory |
| `hook.sock` | the hook socket |
| `mcp/<bot id>.toml` | mode 0600: the bot's token and the MCP server settings (`tool_mode`, write call) |
| `mcp/<bot id>.claude.json` | mode 0600: the claude MCP entry; names the TOML, holds no token |
| `sessions/<KEY>-<cli>/session.json` | what the runner knows about a session |
| `sessions/<KEY>-<cli>/claude-settings.json` | the hook commands of a session the runner creates |
| `panes/` | per-pane hook counters |

The directory is created with mode 0700. The default is `~/.local/state/sylvode-runner` on Linux
and `~/Library/Application Support/sylvode-runner` on macOS.

### Agent guide

With `[runner] install_agent_guide = true` (the default), the runner installs the Sylvode agent
guide into the project directory before a task runs: for claude the skill directory
`.claude/skills/sylvode-mcp/`, for codex and opencode a managed section in `AGENTS.md`. Only
files it manages are changed; anything else is a conflict and is left alone (logged, and shown by
`doctor --project KEY`). With `git_exclude_managed = true` (the default), the claude skill
directory is added to `.git/info/exclude` when the project has a `.git` directory; a git worktree
(whose `.git` is a file) cannot get that line, and `doctor` warns.

### What a task can do

A `runner` agent keeps a CLI running as the runner's OS user. Anyone who can assign an issue to
the bot, mention it, or request its vote can make that CLI act, and anyone who can edit an issue
or comment can put text in front of it. With the default `launch_command` and `headless_args`
(`--permission-mode bypassPermissions`) every permission check of the CLI is off. Read
[SECURITY.md](SECURITY.md) before connecting a bot; at start-up one `runner CLI posture` line per
CLI states what it may do, with a warning while checks are off, and `doctor` and the connection's
status in Sylvode say the same.

### Handshake and report

At start-up and every report interval, the runner calls `GET /api/v1/ai/protocol` with each
runner bot's token and compares `task_protocol` with its own (1), then sends
`PUT /api/v1/ai/runner/report`: component names, statuses and versions, the sessions and the
posture. The report interval is the one Sylvode advertises, kept between 60 and 3600 seconds, or
`[runner] report_interval_secs`. A mismatch shows in `/health` and refuses every `runner` agent
with `protocol_mismatch`; other agents keep working. A bot without a runner connection in Sylvode
(404) is logged once and not reported again until restart; a token Sylvode refuses is logged as
`bot <id>: token refused by Sylvode (revoked, rotated or wrong surface)`. Tokens are never logged,
never part of an error, and never follow a redirect.

After a restart the runner rebuilds its tmux state from Sylvode (`GET /api/v1/ai/tasks/active`
per bot): a task that was typed and whose session lives is supervised again, one whose session is
gone is failed retryably (`runner restarted and session <name> was gone`), and one that was never
typed is queued again.

## Ending tasks

The Sylvode worker marks an AI task `processing` when it delivers it, and only the task protocol
ends it: `POST /api/v1/ai/tasks/{task_id}/complete` or `.../fail?attempt=N`, with the token of the
task's bot.

- **`runner` agents.** The CLI ends the task with `sylvode_task_complete` or `sylvode_task_fail`.
  The runner fails a task only when it cannot go on (see the timers above); it then fails it only
  if Sylvode still shows it active.
- **`openclaw`, `openprx`, `webhook`, `custom`.** When the hand-over worked, the runner completes
  the task (outcome `done`, summary `handed over to the <agent id> agent`), so it runs once and its
  lease never expires into a redelivery. When the hand-over fails, the runner fails the task with
  `source: "runner"`, retryable, with a fixed summary such as
  `error: the command exited with status 3` (never command output). An agent that could not be
  started (an incomplete agent table) and a task that no agent matches are failed, not retryable.

These reports need `[ai_task_callback] enabled = true` and a token of the task's bot (an
`[[ai_task_callback.bots]]` entry, or the global `token`). A token works only for the tasks of the
bot that owns it: a task whose bot has no token is not reported (and logged), and the worker ends
it by its processing timeout. Safe mode turns these reports off. The worker also fails a task that
this runner declines in its answer (`no_agent`, `ignored`, `refused`).

A report names the attempt (`?attempt=N`, the delivery's `attempts`) and is accepted only while
that attempt is active. A refusal is never retried; a timeout, a transport error and an HTTP 5xx
are, `max_attempts` times in all, with a delay of `retry_delay_ms` doubled after each failure (at
most 30 seconds between two attempts).

## Forwarding agents in detail

### What each agent receives

- `openclaw` runs `command [args...] --channel <channel> --target <target> --message <message>`.
- `openprx` posts `{"recipients": [target], "message": message}` to
  `<signal_api>/api/v1/send/<account>`; without `signal_api` it runs `command` like `openclaw`.
- `custom` runs `command` with `args`, `{message}` replaced as described above. If text from
  Sylvode would make an argument start with `-`, the command is not run (`custom_error: ...`),
  unless an earlier argument is `--`. A leading `-` you write yourself, in an argument such as
  `--text={message}` or at the start of `message_template`, is left alone.
- `webhook` posts the delivery JSON unchanged to `url`, with `X-Webhook-Signature: sha256=<hex>`
  (HMAC-SHA256 of the body with `secret`) when `secret` is set.

Commands run directly, never through a shell, so `command` names one program and its arguments go
in `args`. A `command` with whitespace refuses start-up
(``agent `X`: openclaw.command must name one program; put its arguments in openclaw.args``).
Commands get `/dev/null` as stdin and inherit the service's environment. A command may run for
`max(http_timeout_secs, 60)` seconds; HTTP requests time out after `http_timeout_secs`.

### Message templates

`message_template` sets the message of `openclaw`, `openprx` and `custom` agents. Without it the
message is `[{project}] {event}: {key} {title}` and, on a second line, `{actor} | Trigger: {reason}`.
A template is rendered in one pass: each placeholder below is replaced by its value, any other
text is kept, and a placeholder inside a value (an issue title containing `{actor}`) stays literal.

| Placeholder | Value |
|---|---|
| `{event}` | the event (`issue.assigned`), or a worker task's `task_type` |
| `{title}` | the issue title (`untitled` when absent) |
| `{reason}` | the trigger reason or kind (`assigned`, `mention`, ...) |
| `{key}` | the issue key |
| `{issue_id}`, `{project_id}` | the issue and project ids |
| `{form_id}`, `{form_key}`, `{record_id}` | the form identifiers of a form event |
| `{actor}` | the name of the user who caused the event |
| `{project}`, `{workspace}` | the project and workspace names |
| `{state}`, `{priority}` | the issue state and priority |
| `{url}` | `issue/<issue_id>` |

A missing value renders as an empty string (`unknown` for `{event}`, `{reason}` and `{actor}`,
`untitled` for `{title}`).

### Stopping commands

Every command (of the `openclaw`, `openprx` and `custom` agents, and every headless run of a
`runner` agent) is started as the leader of its own process group. On a time-out, on shutdown,
and when it exits by itself, the whole group is sent `SIGTERM` and, if anything in it is still
alive 5 seconds later, `SIGKILL`. This also stops the processes the command started. A process
that leaves its process group (`setsid`, `daemon`) is not reached. On macOS a command's own exit
can only be noticed by reaping it, so the group of a command that exits by itself is not
signalled there. A group the service may not signal (`EPERM`) is reported and left alone.

In a container, run the image with an init process (`docker run --init`, or `init: true` in
compose): without one, orphaned processes are never reaped.

## Configuration reference

Every table refuses unknown keys. Values in parentheses are defaults.

### `[server]`

| Key | Type | Meaning |
|---|---|---|
| `listen` | string, required | `host:port` to listen on, for example `127.0.0.1:9090` |

### `[security]`

| Key | Default | Meaning |
|---|---|---|
| `webhook_secrets` | `[]` | HMAC secrets that verify a delivery (any one may match). A blank entry or an example placeholder refuses start-up while signatures are verified. With no secret and `allow_unsigned = false`, every request is answered 401 and the start-up log says so. |
| `allow_unsigned` | `false` | `true` turns signature verification off (local development only); refused on a `listen` address other than loopback (`127.0.0.0/8`, `::1`, `localhost`) unless `allow_unsigned_non_loopback = true` |
| `allow_unsigned_non_loopback` | `false` | acknowledges unsigned requests on a non-loopback address |
| `max_delivery_age_secs` | `300` | refuse a signed delivery whose `timestamp` is further from now, either way; `0` turns the check off |

A secret counts as a placeholder when it is one of the example values or contains
`replace_with`, `replace-with`, `your_`, `your-`, `changeme` or `example` (ignoring case).

### `[features]` and `[runtime]`

| Key | Default | Meaning |
|---|---|---|
| `features.tunnel_enabled` | `false` | one of the two switches of the [tunnel client](#tunnel-client) |
| `runtime.cli_max_concurrency` | `1` | headless runs of `runner` agents at once |
| `runtime.http_timeout_secs` | `15` | outbound HTTP of forwarding agents; commands get `max(this, 60)` seconds; shutdown takes at most `5 + 2 × this + 5` seconds |
| `runtime.tunnel_reconnect_backoff_max_secs` | `60` | longest tunnel reconnect delay |

### `[ai_task_callback]` and `[[ai_task_callback.bots]]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `false` | report task outcomes and, for `runner` agents, read tasks with the bots' tokens; required by `runner` agents |
| `api_url` | none | base URL of the Sylvode API (no credentials, query or fragment); required when enabled; `https://` unless the host is loopback when there is a `runner` agent |
| `token` | none | an `opr_` bot token used for every task no `bots` entry covers |
| `bot_id` | none | the bot that owns `token`; with it, `token` is used only for that bot's tasks (without it the start-up log warns) |
| `surface` | `rest` | the transport the token is registered with: `rest`, `mcp_http`, `mcp_sse`, `mcp_stdio`, `cli`, `cli_tools_call`; any value but `rest` is sent as `X-Sylvode-MCP-Surface` |
| `max_attempts` | `3` | attempts per report, 1 to 10 |
| `retry_delay_ms` | `1000` | delay before the first retry, at most 60000 |
| `bots[].bot_id` | required | the bot's user id (UUID); one token per bot |
| `bots[].workspace_id` | none | the bot's workspace (UUID); required for a `runner` bot |
| `bots[].token` | required | the bot's `opr_` token |
| `bots[].surface` | `surface` above | the token's transport; `mcp_stdio` for a `runner` bot |
| `bots[].webhook_secret` | none | verifies deliveries for this bot only; required on every runner bot when there is more than one |

### `[[agents]]`

| Key | Meaning |
|---|---|
| `id`, `name` | required; matched against the bot key ([Choosing the agent](#choosing-the-agent-for-a-task)) |
| `agent_type` | required: `runner`, `openclaw`, `openprx`, `webhook` or `custom` |
| `message_template` | the message of `openclaw`, `openprx`, `custom` |
| `allowed_roots` | `runner` only: this agent's own roots instead of `[runner] allowed_roots` |
| `[agents.route]` | `bot_names`, `bot_ids`, `bot_agent_types`, `project_types`, `trigger_kinds`, `events`, `form_keys` (lists, default empty) |
| `[agents.openclaw]` | `command` (required), `args` (`[]`), `channel` (required), `target` (required) |
| `[agents.openprx]` | `signal_api`, `account`, `target` (required), `command`, `args` (`[]`), `channel` (`signal`) |
| `[agents.webhook]` | `url` (required), `secret` |
| `[agents.custom]` | `command` (required), `args` |

An agent type that is not one of the five is answered `refused` (`unknown agent type: ...`) when
it is selected. `agent_type = "cli"` refuses start-up with a message that points to a `runner`
agent on the headless channel.

### `[runner]`

| Key | Default | Meaning |
|---|---|---|
| `state_dir` | platform default ([State directory](#state-directory)) | `~/` is expanded; must lie outside every allowed root |
| `allowed_roots` | `[]` | [Allowed roots](#allowed-roots) |
| `delivery_check_timeout_secs` | `5` | 1 to 60: the synchronous checks; slower is HTTP 503 |
| `heartbeat_secs` | `600` | at least 1: `runner.queued` and `runner.alive` interval |
| `report_interval_secs` | none | 1 to 86400: overrides the report interval Sylvode advertises |
| `install_agent_guide` | `true` | [Agent guide](#agent-guide) |
| `git_exclude_managed` | `true` | add the claude skill directory to `.git/info/exclude` |

### `[runner.tmux]`

| Key | Default | Meaning |
|---|---|---|
| `binary` | `tmux` | program name or absolute path |
| `socket_name` | `""` | `-L` socket; empty is the operator's default server; `^[A-Za-z0-9_-]{0,32}$` |
| `min_version` | `3.2` | refuse an older tmux; cannot be below 3.2 |
| `window_width`, `window_height` | `200`, `50` | size of a session the runner creates (20 to 1000, 5 to 500) |
| `ready_timeout_secs` | `90` | a created session's CLI must become ready within this |
| `push_settle_ms` | `300` | wait between paste and Enter |
| `user_idle_secs` | `10` | no typing while an attached client typed more recently |
| `ack_reminder_secs` | `180` | [Timers](#timers-of-a-tmux-task) |
| `report_unacked_after_secs` | `900` | [Timers](#timers-of-a-tmux-task) |
| `completion_grace_secs` | `60` | [Timers](#timers-of-a-tmux-task) |
| `screen_hold_secs` | `600` | [Timers](#timers-of-a-tmux-task); at least 30 is applied |

### `[runner.cli.claude]`, `[runner.cli.codex]`, `[runner.cli.opencode]`

| Key | Default | Meaning |
|---|---|---|
| `claude.binary` | `claude` | the program of a headless run and of `setup` |
| `claude.launch_command` | `["claude", "--permission-mode", "bypassPermissions"]` | the command of a session the runner creates; must start with a program |
| `claude.headless_args` | `["--permission-mode", "bypassPermissions"]` | arguments of a headless run, after the prompt |
| `claude.hooks` | `true` | pass the hook settings to a session the runner creates |
| `claude.process_names` | `["claude", "claude.exe"]` | foreground command names that identify the CLI's pane |
| `codex.enabled` | `false` | allow codex (headless only) |
| `codex.binary` | `codex` | |
| `codex.headless_args` | `[]` | arguments before the prompt (a `--sandbox` choice, say) |
| `opencode.enabled` | `false` | allow opencode (headless only) |
| `opencode.binary` | `opencode` | |

claude is always enabled: `[runner.cli.claude]` has no `enabled` key. Command words may not hold
control characters.

### `[runner.mcp]`

| Key | Default | Meaning |
|---|---|---|
| `server_binary` | `mcp-server` | the Sylvode MCP server executable |
| `tool_mode` | `search` | `search` or `full`, written into the per-bot MCP file; see the [MCP server](../mcp-server/README.md) |
| `allow_write_call` | `false` | turn on the `sylvode_write` call in the MCP file and allow it in headless claude runs |

### `[tunnel]`

`enabled` (`false`), `url`, `agent_id`, `auth_token`, `reconnect_secs` (`3`), `heartbeat_secs`
(`20`), `hmac_secret`, `require_inbound_sig` (`false`), `allow_insecure_transport` (`false`). See
[Tunnel client](#tunnel-client).

## Operator commands

```text
sylvode-runner [CONFIG]
sylvode-runner doctor [--json] [--config PATH] [--mode tmux|headless|auto] [--as-service]
                      [--project KEY] [--show-paths]
sylvode-runner --check
sylvode-runner config check [--json] [--config PATH]
sylvode-runner sessions [--config PATH]
sylvode-runner release KEY [--config PATH]
sylvode-runner setup claude --project KEY [--dir D] [--bot ID] [--apply] [--config PATH]
sylvode-runner --help | --version
```

A config path that starts with `-` is written `./-name`.

- **`sessions`** lists the sessions the runner recorded: name, project id, CLI, state (`idle`,
  `busy`, `dead`, `gone`) and who started it (`runner` or `operator`). It changes nothing and
  works while the runner is stopped.
- **`release KEY`** frees a project key bound on this runner, only when no tmux session of the key
  exists and the runner is stopped.
- **`setup claude`**: see [above](#setup-claude-for-a-session-you-start-yourself).
- **`config check`** says whether the configuration loads, naming the keys that do not, never
  printing a value.

### Doctor

`doctor` prints the `sylvode.doctor.v1` report: tmux (version against 3.2, server on the runner's
socket), the CLIs, the MCP server (its version and task protocol), this runner (configuration,
service, linger, the launch command and headless arguments and whether they turn permission
checks off), and the API protocol. `--check` is `doctor --json` with no network call and no
secret. Exit status 0: no row failed; 2: a row failed (warnings never change it); 1: the checker
could not run. The output is meant for bug reports: no token, no URL credentials, no
configuration content, and project directories only as labels unless `--show-paths` is given.

With `--project KEY`:

- `project.<KEY>`: the directory from the runner's session record against the allowed roots, and
  the state of the agent guide (`current`, `stale`, `conflict`, `absent`). The row also carries a
  `trusted` field that is always `unknown`, so the row is at best a warning.
- `session.<KEY>-claude`: state (`running`, `absent`, `cli_not_running`, `ambiguous`, `dialog`),
  who started it, pane, foreground command, hooks seen, `permissions` (`bypass`, `normal`,
  `unknown`) and `mcp` (`ok`, `missing`, `other`, `stale`), each with a hint.
- a warning when the project is a git worktree, which cannot get its exclude line.

A missing prerequisite does not stop the service: it starts degraded, logs the row, and every
delivery that needs the missing program is refused at once (`tmux_unavailable`,
`cli_unavailable`). The prerequisites are probed at most once a minute and each delivery checks
the program it needs again, so installing tmux takes effect without a restart.

## HTTP interface

| Endpoint | Method | Description |
|---|---|---|
| `/webhook` | POST | receive a worker AI task or a webhook delivery |
| `/health` | GET | unauthenticated: `{"status":"ok","protocol":{"runner":1,"api":1,"state":"ok"}}`; `state` is `ok`, `mismatch`, `unreachable`, `pending` (not checked yet) or `unchecked` (no `runner` agent). It says nothing about missing prerequisites. |

`/webhook` answers `401` for a missing, invalid or conflicting signature or a stale delivery,
`409` for a replayed one, `400` for a body that is not JSON, and `503` when a `runner` agent's
task view could not be fetched in time. Otherwise it answers `200` with a JSON `status`:
`ignored`, `no_agent`, `refused` (with `reason_code` for a `runner` agent, or the reason a
forwarding agent was not started), or `dispatched` with the agent id and a short `result` (`ok`,
`error: the command exited with status N`, `webhook: 200 OK`) or, for a `runner` agent, the
channel. For a worker AI task the answer also says whether its outcome will be reported
(`ai_task_callback`: `report`, `no_token` or `off`). Command output and remote error bodies are
never part of the answer, because the sender stores it in its delivery log; they are written to
this service's log (`Dispatch result` with `detail=...`).

### Signatures

The request signature is `sha256=<hex>` (the prefix is optional), an HMAC-SHA256 of the raw body.
It is read from `X-Sylvode-Signature` and `X-Webhook-Signature` (the header the Sylvode API
sends). When the headers present carry different signatures, or one header is repeated with
different values, the request is refused with `401`, even if one of them is valid; a non-ASCII
value is refused the same way. With `allow_unsigned = true` the headers are not read.

### Replayed requests

For a signed request, a webhook delivery whose `timestamp` is more than `max_delivery_age_secs`
away from the runner's clock, either way, is refused with `401`, and a delivery whose `id` was
already received (for a worker AI task: the same `task_id` and `attempts`) is refused with `409`.
Keep the clocks of Sylvode and the runner in sync.

- A worker AI task carries no timestamp, so it is only checked for uniqueness, while its key is
  remembered: twice `max_delivery_age_secs`, at least 10 minutes, and at most 10,000 of them.
- The memory is held in the process and cleared on restart.
- Only top-level fields are checked; a body without them (such as a Flow event delivery, which is
  not a bot task anyway) is not.

## Running the service

### Shutdown

On `SIGTERM` or `SIGINT` the service stops accepting connections and lets the requests in flight
finish. Every running headless run is stopped as described in
[Stopping commands](#stopping-commands) and its task is failed, retryably. tmux sessions keep
running. The whole shutdown takes at most `5 + 2 × http_timeout_secs + 5` seconds (40 seconds by
default): a request still open then is closed, a report not yet sent is dropped, and the service
exits 0. systemd's default stop timeout (90 seconds) is enough; `docker stop` needs `--time 45`.

### Environment and logs

| Variable | Effect |
|---|---|
| `SYLVODE_RUNNER_SAFE_MODE` | `1`, `true`, `yes` or `on` turns the tunnel and the `[ai_task_callback]` reports off. `runner` agents then refuse their tasks (`runner_agent_disabled`); `openclaw`, `openprx`, `webhook` and `custom` agents still run, without ending tasks in Sylvode. |
| `RUST_LOG` | log filter; the default is `sylvode_runner=info`, for example `RUST_LOG=sylvode_runner=debug` |

Logs go to standard output. Values from deliveries and commands are logged as escaped fields.

## Tunnel client

The runner contains a WebSocket tunnel client (`[features] tunnel_enabled` and
`[tunnel] enabled`, both `true`, with `url`, `agent_id` and `auth_token`). The Sylvode server has
no endpoint for it, so an enabled tunnel connects to nothing and retries with backoff; leave it
disabled. Every task it would receive is acknowledged and answered as failed.

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or
[MIT license](../../LICENSE-MIT) at your option.
