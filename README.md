# Sylvode Webhook

Webhook receiver for [Sylvode](https://github.com/openprx/sylvode). It receives webhook deliveries
from a Sylvode deployment and dispatches them to AI agents, chat platforms or external services.

> **Formerly OpenPR Webhook.** The project was renamed together with the main product. Every old
> name (executable, container image, signature header, environment variable, service unit) keeps
> working for the whole Sylvode 1.x line and is not removed before Sylvode v2.0; see [Migrating
> from OpenPR Webhook](#migrating-from-openpr-webhook).

Built with **Rust** (Axum).

## How It Works

```
Sylvode ──webhook POST──▶ sylvode-webhook ──dispatch──▶ OpenClaw (Signal/Telegram)
                                          ──dispatch──▶ HTTP endpoint
                                          ──dispatch──▶ Custom command
                                          ──dispatch──▶ CLI agent (codex/claude-code)
                                                            │
                                                            ▼
                                                       Sylvode MCP server
                                                       (read issue → fix → write back)
```

1. Sylvode sends a signed webhook delivery for one of the events a webhook subscribes to, or its
   worker dispatches a queued AI task to the webhook of the bot it is assigned to
2. sylvode-webhook verifies the HMAC-SHA256 signature
3. It acts only on bot tasks: a delivery whose `bot_context.is_bot_task` is `true`, or a worker AI
   task envelope (`task_id` and `ai_participant_id`). Anything else is answered
   `{"status":"ignored","reason":"not_bot_task"}`
4. It selects exactly one agent (see [Agent selection](#agent-selection)) and dispatches to it

A webhook can subscribe to these 14 events, the full list the Sylvode API accepts:

| Group | Events |
|-------|--------|
| Issues | `issue.created`, `issue.updated`, `issue.assigned`, `issue.deleted`, `issue.state_changed` |
| Comments | `comment.created`, `comment.updated`, `comment.deleted` |
| Labels | `label.added`, `label.removed` |
| Sprints | `sprint.started`, `sprint.completed` |
| AI tasks | `ai.task_completed`, `ai.task_failed` |

A delivery carries `bot_context` only when the webhook has a bot user and that bot is an assignee
of the issue, or, for `comment.created`, is mentioned in the comment.

## Features

- **HMAC-SHA256 signature verification** — Validates webhook authenticity
- **Agent routing** — Each bot task goes to exactly one agent, chosen by bot identity, agent type
  and optional route constraints (project type, trigger kind, event, form key)
- **Agent types**:
  - `openclaw` — Send the rendered message via the OpenClaw CLI (`openclaw message send`)
  - `openprx` — Send the rendered message via an OpenPRX Signal API or a command
  - `webhook` — Forward the delivery JSON unchanged to an HTTP endpoint
  - `custom` — Run a command with the rendered message as an argument
  - `cli` — Run codex, claude-code or opencode with a rendered prompt (fixed executor whitelist)
- **MCP closed-loop automation** — AI agents read full issue context (description, comments, labels) via Sylvode MCP tools and write results back directly
- **CLI callbacks** — Report a CLI run back to Sylvode via MCP or the REST API (see [Callbacks](#callbacks))
- **Per-agent environment variables** — Inject extra environment variables into each CLI executor
- **WSS tunnel client (Phase B MVP)** — Active ws/wss connection with Bearer auth, heartbeat, auto-reconnect
- **Tunnel envelope + HMAC** — Minimal envelope (`id/type/ts/agent_id/payload/sig`) with optional HMAC-SHA256
- **Task bridge** — Handles `task.dispatch` -> immediate `task.ack` -> async `task.result`
- **Message templates** — Customizable message and prompt format with placeholders
- **Configurable** — TOML-based configuration

## Quick Start

```bash
# Build
cargo build --release

# Configure
cp config.example.toml config.toml
# Set webhook_secrets to the secret of your Sylvode webhook; start-up is refused while the
# example placeholder is still there

# Run
./target/release/sylvode-webhook config.toml
# sylvode-webhook listening on 127.0.0.1:9090
```

The example configuration is secure by default: it listens on loopback only, verifies every
signature (`allow_unsigned = false`) and contains no agent that runs a command without
`[features].cli_enabled`. Change `listen` once the secret is set, or put a reverse proxy in front.

`sylvode-webhook [CONFIG]` reads `config.toml` in the working directory when no path is given.
`--help` and `--version` print usage and the version.

### Configure the Sylvode webhook

In Sylvode, create a webhook pointing to this receiver:

- **URL**: `http://your-server:9090/webhook`
- **Secret**: Must match one of `webhook_secrets` in `config.toml`
- **Events**: Select which events to receive (see the list above)
- **Bot user**: The bot whose tasks this receiver handles. Without one, deliveries carry no
  `bot_context` and are ignored, and the worker has no webhook to dispatch that bot's AI tasks to

## Configuration

```toml
[server]
listen = "0.0.0.0:9090"

[security]
webhook_secrets = ["a-long-random-secret"]  # blank entries and example placeholders refuse start-up
allow_unsigned = false  # true only for local development; see below
max_delivery_age_secs = 300  # refuse signed deliveries whose timestamp is further from now; 0 = off

# Feature gates (safe defaults). A cli agent does nothing unless cli_enabled is
# true, and no callback is sent unless callback_enabled is true.
[features]
tunnel_enabled = false
cli_enabled = false
callback_enabled = false

[runtime]
cli_max_concurrency = 1               # cli executors running at once, webhook and tunnel together; others wait
http_timeout_secs = 15                # outbound HTTP; commands get max(this, 60) seconds
tunnel_reconnect_backoff_max_secs = 60

# Agent: OpenClaw (AI assistant via Signal/Telegram)
[[agents]]
id = "david"
name = "David"
agent_type = "openclaw"
message_template = "🔔 [{project}] {event}: {key} {title}\n👤 {actor} | Trigger: {reason}"

[agents.openclaw]
command = "openclaw"
args = ["message", "send"] # runs: openclaw message send --channel ... --target ... --message ...
channel = "signal"
target = "uuid:your-user-uuid"

# Agent: OpenPRX (AI assistant via Signal)
[[agents]]
id = "vano"
name = "Vano"
agent_type = "openprx"
message_template = "[{project}] {event}: {key} {title}\n{actor} | {reason}"

[agents.openprx]
signal_api = "http://127.0.0.1:8686"
account = "+1234567890"
target = "uuid:your-user-uuid"
# Or send through a command instead (used only when signal_api is not set):
# command = "openprx"
# args = ["message", "send"]
# channel = "signal"   # default

# Agent: forward the delivery JSON, unchanged, to an HTTP endpoint.
# No message is rendered, so message_template has no effect here.
[[agents]]
id = "review-bot"
name = "Document review connection"
agent_type = "webhook"

[agents.webhook]
url = "https://automation.example.com/hooks/sylvode"
secret = "optional-shared-secret" # if set, the X-Webhook-Signature header is added

# Optional: route constraints (see "Agent selection"). Every non-empty list
# must contain the task's value; empty or omitted lists mean "no constraint".
[agents.route]
bot_names = ["Document review connection"]
bot_agent_types = ["webhook"]
project_types = ["contract_review"]
trigger_kinds = ["assigned", "mentioned"]
events = ["issue.assigned", "comment.created"]
form_keys = ["order", "print_job"]

# Agent: Custom command
[[agents]]
id = "logger"
name = "Logger"
agent_type = "custom"
message_template = "{event} {key}"

[agents.custom]
command = "logger"                     # one program, no shell: put arguments in args
args = ["-t", "sylvode", "--", "{message}"]

# Agent: CLI executor with MCP closed-loop
[[agents]]
id = "ai-fixer"
name = "AI Issue Fixer"
agent_type = "cli"

[agents.cli]
executor = "claude-code" # codex | claude-code | opencode
workdir = "/path/to/your/repository"
timeout_secs = 900
max_output_chars = 12000
prompt_template = "Fix issue {issue_id}: {title}\nContext: {reason}"
callback = "mcp" # mcp (default) | api
callback_url = "http://127.0.0.1:8090/mcp/rpc" # no callback is sent without it
callback_token = "opr_xxx"                     # sent as Authorization: Bearer <token>
# Issue state to set through the callback (ignored when skip_callback_state = true):
# update_state_on_start = "in_progress"
# update_state_on_success = "done"
# update_state_on_fail = "todo"

# MCP closed-loop: AI reads full issue context and updates state via MCP tools,
# so skip_callback_state prevents duplicate state updates from the callback.
skip_callback_state = true

# Optional: custom MCP instructions (overrides built-in default).
# mcp_instructions = "Use work_items.get to read issue {issue_id}, then fix it."

# Path to an MCP config for claude-code (--mcp-config flag). Setting it (or
# mcp_instructions, or env_vars) also appends the default MCP instructions.
mcp_config_path = "/path/to/mcp-config.json"

# Extra environment variables injected into the executor subprocess.
# [agents.cli.env_vars]
# HTTPS_PROXY = "http://proxy.internal:3128"
```

`allow_unsigned = true` turns signature verification off and logs a warning at start-up. On a
`listen` address other than loopback (`127.0.0.0/8`, `::1` or `localhost`) it is refused unless
`allow_unsigned_non_loopback = true` acknowledges that anyone who can reach the port can trigger
agents. With `allow_unsigned = false` and no `webhook_secrets`, every request is answered `401`,
and start-up logs a warning saying so.

Start-up is refused, with a message naming the agent, when a `route.trigger_kinds` value is not one
Sylvode sends, or when an `openclaw`, `openprx` or `custom` `command` contains whitespace but names
neither an existing file nor a program on `PATH`.

### Agent selection

A bot task is dispatched to exactly one agent, or to none. The bot key is `bot_context.bot_name`
(or `bot_context.bot_id` when the name is empty) for a webhook delivery, and `ai_participant_id`
(the bot user's id) for a worker AI task. The first match in this order wins, in configuration
order within each step:

1. an agent whose `id` equals the bot key, or whose `name` equals it ignoring case, and whose route
   matches;
2. otherwise an agent whose `agent_type` equals the bot's agent type (`bot_context.bot_agent_type`,
   which Sylvode sets to `custom` for a bot without one, or `ai_participant_agent_type` for a worker
   task) and whose route matches;
3. otherwise an agent that has an `[agents.route]` section and whose route matches.

For a worker AI task, the bot's agent type is `ai_participant_agent_type` only, and the issue that
every callback writes to is the task's `reference_id` (when `reference_type` is `work_item`). The
task's inner `payload` is free-form data from whoever created the task; it never chooses the agent
type or the callback's issue.

An agent without a route matches every task. A route matches when every non-empty list contains
the task's value, compared ignoring case and surrounding whitespace; a value missing from the task
fails a non-empty list. `bot_names` and `bot_ids` together form one constraint that either list can
satisfy. When nothing matches, the answer is `{"status":"no_agent", ...}` and nothing runs.

`trigger_kinds` is compared with `bot_context.trigger_reason` (or `bot_context.trigger_kind`) on a
webhook delivery and with the top-level `trigger_kind` on a worker AI task, and with nothing else: a
task without that field fails a non-empty `trigger_kinds`. These are the values Sylvode sends; any other value refuses
start-up:

| Source | Values |
|--------|--------|
| Webhook delivery (`bot_context.trigger_reason`) | `assigned` (`issue.assigned`, `issue.updated` and every event not listed here), `created` (`issue.created`), `status_changed` (`issue.state_changed`), `mentioned` (`comment.created`), `completed` (`ai.task_completed`), `failed` (`ai.task_failed`) |
| Worker AI task (`trigger_kind`) | `assigned` (`issue_assigned`), `mention` (`review_requested`, `comment_requested`), `proposal_vote` (`vote_requested`), `manual` (anything else) |

`events` is compared with the delivery's `event` (a worker AI task has none, so a route with
`events` never matches one). `project_types` is compared with a `project_type` field, which
Sylvode currently sends only in the payload of a worker AI task for a bot mentioned in a comment;
`form_keys` with a `form_key` field, which no current Sylvode bot task carries. A non-empty list
for either of them therefore matches only those tasks, or none.

### What each agent receives

- `openclaw` runs `command [args...] --channel <channel> --target <target> --message <message>`.
- `openprx` posts `{"recipients": [target], "message": message}` to
  `<signal_api>/api/v1/send/<account>`; without `signal_api` it runs `command` like `openclaw`.
- `custom` runs `command` with `args`. Every argument containing `{message}` has it replaced by the
  rendered message and stays one argument, whatever the message contains; without `{message}` in
  any argument the message is appended as the last argument. If the message would make an argument
  start with `-` (so text from Sylvode could act as an option), the command is not run, unless an
  earlier argument is `--`. Arguments you write with a leading `-`, such as `--text={message}`, are
  left alone.
- `webhook` posts the delivery JSON unchanged to `url`, signed with `X-Webhook-Signature:
  sha256=<hex>` when `secret` is set.
- `cli` runs the whitelisted executor with the rendered prompt in the background and answers at
  once: `codex exec --dangerously-bypass-approvals-and-sandbox --skip-git-repo-check <prompt>`,
  `claude -p <prompt> --permission-mode bypassPermissions [--mcp-config <path>]` or
  `opencode run <prompt>`, in `workdir`, stopped after `timeout_secs` (see below). A rendered prompt that
  starts with `-` would be read as an option of the executor, so such a run is not started and
  reports `failed`; begin `prompt_template` with fixed text (the default does).

Commands are run directly, never through a shell, so `command` names one program and everything
else goes in `args`. Their stdin is `/dev/null`; they inherit the service's environment (a `cli`
agent adds its `env_vars`).

#### Stopping commands

On Linux and macOS every command (`cli` executors and the commands of `openclaw`, `openprx` and
`custom` agents) is started as the leader of its own process group. When it times out
(`timeout_secs` for `cli`, `max(http_timeout_secs, 60)` seconds for the others), when the service
shuts down, and also when it exits by itself, the whole group is sent `SIGTERM` and, if anything
in it is still alive 5 seconds later, `SIGKILL`. This stops the processes an executor started
(an npm-installed `codex` launcher's native binary, an agent's tool calls), not only the executor
itself. A process that deliberately leaves its process group (`setsid`, `daemon`) is not
reached. The timeout or cancellation callback is sent only after the group is gone.

In a container, run the image with an init process (`docker run --init`, or `init: true` in
compose): without one, processes orphaned by an executor are never reaped and each stop waits the
full 5 seconds.

### Shutdown

On `SIGTERM` or `SIGINT` the service stops accepting connections and finishes the requests in
flight. Every running `cli` executor (webhook-triggered or tunnel task) is stopped as above and
then sends its terminal callback with `status = "cancelled"`, which sets `update_state_on_fail`
like a failure, so no issue is left `in_progress` by a run that no longer exists. On the webhook
path this callback follows the same rules as a failure (it is not sent with
`skip_callback_state = true`, which also suppresses the start callback). The service waits for
these callbacks up to `5 + 2 × http_timeout_secs + 5` seconds (40 seconds by default), then
exits 0. Give the service manager at least that long: systemd's default `TimeoutStopSec` (90 s)
is enough, `docker stop` needs `--time 45` (its default is 10 s).

### Environment

| Variable | Effect |
|----------|--------|
| `SYLVODE_WEBHOOK_SAFE_MODE` | `1`/`true`/`yes`/`on` forces the tunnel, CLI and callback paths off at runtime — a one-switch rollback to webhook-only behavior |
| `RUST_LOG` | Log filter. This service's events use the `sylvode_webhook` target, e.g. `RUST_LOG=sylvode_webhook=debug`; the default is `sylvode_webhook=info` |

Logs are written to stdout. Deprecation notices for legacy names are written to stderr, once per
process.

## MCP Closed-Loop Automation

When a CLI agent has Sylvode MCP tools available (via global config or `mcp_config_path`), it can autonomously:

1. **Read full issue context** — title, description, comments, labels, state, priority via `work_items.get` / `comments.list`
2. **Fix the problem** — analyze context, write code, run tests
3. **Write results back** — post a summary comment via `comments.create`, update state via `work_items.update`

This eliminates the need for the webhook callback to update issue state (use `skip_callback_state = true`).

Default MCP instructions are appended to the prompt when the agent has MCP-related config
(`mcp_instructions`, `mcp_config_path`, or `env_vars`); a form event gets form-specific
instructions instead. You can replace them with `mcp_instructions`. With MCP-related config the
prompt also asks the agent to read the project context (`context.get_project`) when the task
carries a project id, and describes the form record for a form event.

### MCP Setup

The Sylvode MCP server reads its API URL, bot token and workspace from its own configuration
file, not from environment variables. Point the agent's MCP client at it with an absolute path; see
the [Sylvode README](https://github.com/openprx/sylvode#readme) for the `[mcp]` section.

For **Codex**, add to `~/.codex/config.toml`:

```toml
[mcp_servers.sylvode]
type = "stdio"
command = "/path/to/mcp-server"
args = ["serve", "--config", "/absolute/path/to/config/sylvode.toml"]
```

For **Claude Code**, add to `~/.claude.json`:

```json
"sylvode": {
  "type": "stdio",
  "command": "/path/to/mcp-server",
  "args": ["serve", "--config", "/absolute/path/to/config/sylvode.toml"]
}
```

### Callbacks

A callback is sent only when `[features].callback_enabled = true` and the agent has a
`callback_url`.

- **When.** For a `cli` agent selected from a webhook delivery: before the run, a start callback
  carrying `update_state_on_start`, if that is set; after the run, a final callback only when the
  run failed or timed out (a successful agent reports through MCP itself). For a tunnel task: a
  final callback after every run. With `skip_callback_state = true` no callback carries a state, so
  no start callback is sent.
- **`callback = "mcp"`** (the default) sends JSON-RPC `tools/call` requests: `comments.create`
  with an execution report, then `work_items.update` when there is a state to set
  (`update_state_on_success`, or `update_state_on_fail` for a failure or timeout).
- **`callback = "api"`** posts the result as JSON (`issue_id`, `run_id`, `executor`, `status`,
  `summary`, `exit_code`, `duration_ms`, `stdout_tail`, `stderr_tail`, `state`).

`callback_token` is sent as `Authorization: Bearer <token>`.

#### Callback attribution

A plain API callback (`callback = "api"`) reports its surface as `cli` in both
`X-Sylvode-MCP-Surface` and the legacy `X-OpenPR-MCP-Surface`, with the same value, so the API
records it correctly whether it predates the rename or not. Both headers are sent for the whole
Sylvode 1.x line.

## Outbound webhook signature

When forwarding via `agent_type = "webhook"` and `agents.webhook.secret` is configured,
sylvode-webhook signs the outbound JSON body and sends:

- Header: `X-Webhook-Signature`
- Value format: `sha256=<hex_hmac>`

## Phase B: Tunnel (WSS) MVP

Enable both `[features].tunnel_enabled = true` and `[tunnel].enabled = true` in `config.toml` to
let `sylvode-webhook` actively connect to a control plane. The tunnel needs `url` (`ws://`
or `wss://`), `agent_id` and `auth_token`, and its tasks also need `[features].cli_enabled = true`.

```toml
[tunnel]
enabled = true
url = "wss://sylvode.example.com/api/v1/agent-tunnel" # ws:// also supported for LAN/dev
agent_id = "vano-qa"                                  # required; there is no default
auth_token = "opr_xxx"                                # Authorization: Bearer <token>
reconnect_secs = 3
heartbeat_secs = 20
hmac_secret = "shared-hmac-secret"                    # optional; see "Signature behavior"
require_inbound_sig = false                           # refuse inbound envelopes without sig; needs hmac_secret
```

Reconnects back off from `reconnect_secs`, doubling up to
`[runtime].tunnel_reconnect_backoff_max_secs`.

Envelope schema (minimal):

```json
{
  "id": "uuid",
  "type": "task.dispatch|task.ack|task.result|heartbeat|error",
  "ts": 1710000000,
  "agent_id": "vano-qa",
  "payload": {},
  "sig": "sha256=<hex>"
}
```

Current task bridge behavior:

1. Receive `task.dispatch`; its payload may carry `run_id`, `issue_id`, `agent` and `body`
2. Send `task.ack` immediately (`run_id`, `issue_id`, `status=accepted`)
3. Run the `cli` agent whose `id` equals `agent`, or the first `cli` agent when `agent` is absent,
   on `body` (or the whole payload); at most `[runtime].cli_max_concurrency` executors run at once,
   counting webhook-triggered runs too
4. Send `task.result` when done (`run_id`, `issue_id`, `status`, `summary`)

Other envelope types received are ignored.

Signature behavior:

- With `tunnel.hmac_secret` set, every outbound envelope carries `sig` (HMAC-SHA256 over the
  envelope without `sig`), and every inbound envelope must carry a valid `sig`: one that is
  missing or wrong is answered with an `error` envelope (`reason: bad_signature`, or
  `missing_signature` when `require_inbound_sig` is also set) and dropped.
- Without `hmac_secret`, inbound `sig` is not verified and inbound envelopes are not
  authenticated. `require_inbound_sig = true` needs a non-empty `hmac_secret`: an enabled tunnel
  with `require_inbound_sig = true` and no `hmac_secret` is refused at start-up.

Safety toggles:

- `SYLVODE_WEBHOOK_SAFE_MODE=1` forces `tunnel/cli/callback` off at runtime.
- This provides one-command rollback to legacy webhook-only behavior.

## Templates

`message_template` sets the message of `openclaw`, `openprx` and `custom` agents; `webhook` agents
send no message and `cli` agents use `prompt_template`. Without a template these defaults are used:

| Template | Default |
|----------|---------|
| `message_template` | `[{project}] {event}: {key} {title}` and, on a second line, `{actor} \| Trigger: {reason}` |
| `prompt_template` | `Fix issue {issue_id}: {title}` and, on a second line, `Context: {reason}` |

A template is rendered in one pass: a placeholder below is replaced by its value, any other text is
kept as written, and a placeholder that appears inside a value (such as an issue title containing
`{actor}`) stays literal.

| Placeholder | Value | In `message_template` | In `prompt_template` |
|-------------|-------|:---:|:---:|
| `{event}` | Event (`issue.assigned`), or a worker task's `task_type` | yes | yes |
| `{title}` | Issue title (`untitled` when absent) | yes | yes |
| `{reason}` | Trigger reason (`assigned`, `mentioned`, ...; see [Agent selection](#agent-selection)) | yes | yes |
| `{issue_id}` | Issue id | yes | yes |
| `{project_id}` | Project id | yes | yes |
| `{form_id}`, `{form_key}`, `{record_id}` | Form identifiers of a form event | yes | yes |
| `{key}` | Issue key (`BIL-4C3B2A19`) | yes | |
| `{actor}` | Name of the user who caused the event | yes | |
| `{project}` | Project name | yes | |
| `{workspace}` | Workspace name | yes | |
| `{state}`, `{priority}` | Issue state and priority | yes | |
| `{url}` | `issue/<issue_id>` | yes | |

A value that is missing renders as an empty string in `message_template` (`unknown` for `{event}`,
`{reason}` and `{actor}`, `untitled` for `{title}`) and as `<project_id>`, `<form_id>`, and so on in
`prompt_template` (`unknown` for `{issue_id}`, `{event}` and `{reason}`). In a `custom` agent's
`args`, `{message}` stands for the rendered message.

## API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/webhook` | POST | Receive a webhook delivery or worker AI task |
| `/health` | GET | Health check; answers `ok` |

`/webhook` answers `401` for a missing, invalid or conflicting signature or a stale delivery, `409`
for a replayed one, and `400` for a body that is not JSON. Otherwise it answers `200` with a JSON status: `ignored` (not a bot task), `no_agent`
(no agent matched), or `dispatched` with the agent id and the dispatch result (for a `cli` agent,
the run continues in the background).

### Signature headers

The request signature is `sha256=<hex>` (the prefix is optional), an HMAC-SHA256 of the raw body
keyed with one of `webhook_secrets`. It is accepted in any of these headers:

| Header | Status |
|--------|--------|
| `X-Sylvode-Signature` | Canonical |
| `X-Webhook-Signature` | Canonical; the header the Sylvode API sends today |
| `X-OpenPR-Signature` | Legacy; accepted with a deprecation notice, not removed before Sylvode v2.0 |

### Replayed requests

A signature proves who wrote a body, not when it was sent. For a signed request, two fields inside
the signed body are therefore checked as well: a webhook delivery whose `timestamp` is more than
`max_delivery_age_secs` (default 300) away from the receiver's clock, in either direction, is
refused with `401`, and a delivery whose `id` was already received (for a worker AI task: the same
`task_id` and `attempts`) is refused with `409`. Received keys are remembered for twice the window
(at least 10 minutes), up to 10,000 of them. Keep the clocks of Sylvode and this receiver in sync.
A body without these fields, which Sylvode does not send, is not checked.

Every signature header present is read. When they carry different signatures (or one header is
repeated with different values) the request is refused with `401`, even if one of them is valid: an
invalid signature cannot ride along a valid one. A non-ASCII signature value is refused the same
way. With `allow_unsigned = true` signature headers are not read at all.

## Deployment

### Installer (systemd / launchd)

```bash
./scripts/install.sh --binary-path /usr/local/bin/sylvode-webhook --config-dir /etc/sylvode-webhook
./scripts/uninstall.sh
```

The installer creates the user service `sylvode-webhook.service` on Linux or the launch agent
`dev.sylvode.webhook` on macOS. An existing `openpr-webhook.service` or `dev.openpr.webhook` is
updated in place under its old name, so an enabled unit keeps working; run the uninstaller and
install again to move to the new name. The uninstaller removes both.

### Systemd

```ini
[Unit]
Description=Sylvode Webhook Receiver
After=network.target

[Service]
ExecStart=/usr/local/bin/sylvode-webhook /etc/sylvode-webhook/config.toml
WorkingDirectory=/etc/sylvode-webhook
Restart=always

[Install]
WantedBy=multi-user.target
```

### Container image

```bash
docker run -v ./config.toml:/etc/sylvode-webhook/config.toml:ro -p 9090:9090 \
  ghcr.io/openprx/sylvode-webhook:latest
```

The image runs `/app/sylvode-webhook /etc/sylvode-webhook/config.toml` as a non-root user and
contains the legacy `/app/openpr-webhook` as well, so a compose file whose `command` names
`/app/openpr-webhook` keeps working. It is also published as `ghcr.io/openprx/openpr-webhook` with
the same digest. See [`Dockerfile`](Dockerfile).

### Release archives

Each release ships `sylvode-webhook-<platform>.tar.gz` with a `.sha256`, for `linux-amd64`,
`linux-arm64`, `macos-amd64` and `macos-arm64`. Every archive contains both the `sylvode-webhook`
and the legacy `openpr-webhook` executable. The same archive is also published as
`openpr-webhook-<platform>.tar.gz` with its own `.sha256`.

## Migrating from OpenPR Webhook

Nothing has to change on upgrade: every legacy name below keeps working for the whole Sylvode 1.x
line and is not removed before Sylvode v2.0. Using one writes a single deprecation line to stderr
per process, naming the replacement. stdout and exit codes are the same for the legacy and the
canonical name.

| Legacy | Canonical | When both are supplied |
|--------|-----------|------------------------|
| Executable `openpr-webhook` | `sylvode-webhook` | — |
| Image `ghcr.io/openprx/openpr-webhook` | `ghcr.io/openprx/sylvode-webhook` (same digest) | — |
| Archive `openpr-webhook-<platform>.tar.gz` | `sylvode-webhook-<platform>.tar.gz` (same bytes) | — |
| Header `X-OpenPR-Signature` | `X-Sylvode-Signature` (or `X-Webhook-Signature`) | Different values are refused with `401` |
| Env `OPENPR_WEBHOOK_SAFE_MODE` | `SYLVODE_WEBHOOK_SAFE_MODE` | Disagreeing values refuse to start |
| `RUST_LOG` target `openpr_webhook` | `sylvode_webhook` | A directive for the same target with a different level refuses to start |
| Service `openpr-webhook.service` / `dev.openpr.webhook` | `sylvode-webhook.service` / `dev.sylvode.webhook` | The installer refuses to proceed |
| Outbound header `X-OpenPR-MCP-Surface` | `X-Sylvode-MCP-Surface` | Both are always sent with the same value |

Unchanged on purpose: the configuration file format and its default path `config.toml`, the
`/webhook` and `/health` routes, the outbound `X-Webhook-Signature`, the `openprx` agent type
(it names the OpenPRX assistant, not this product), the tunnel `agent_id` (always taken from your
configuration) and the repository URL `https://github.com/openprx/openpr-webhook`.

## Links

- [Documentation](https://docs.openprx.dev/en/sylvode-webhook/) — Full documentation (10 languages)
- [Community](https://community.openprx.dev) — OpenPRX community forum

## Related

- [Sylvode](https://github.com/openprx/sylvode) — Project management platform
- [PRX](https://github.com/openprx/prx) — AI assistant framework

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
