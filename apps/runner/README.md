# Sylvode runner

The runner of [Sylvode](https://github.com/openprx/sylvode). It receives webhook deliveries
from a Sylvode deployment and dispatches them to AI agents, chat platforms or external services.

Built with **Rust** (Axum).

## How It Works

```
Sylvode ──webhook POST──▶ sylvode-runner ──dispatch──▶ OpenClaw (Signal/Telegram)
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
2. sylvode-runner verifies the HMAC-SHA256 signature
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
of the issue, or, for `comment.created`, is mentioned in the comment, and only when the same
action did not already give that bot its task another way. Sylvode gives one action at most one
bot task per bot:

- Assigning an issue to the bot (on create or later) and mentioning it in a comment arrive as a
  **worker AI task** (`trigger_kind` `assigned` or `mention`). The deliveries of that same request
  (`issue.created`, `issue.assigned`, `issue.state_changed`, `issue.updated`, `comment.created`)
  arrive without `bot_context` and are answered `not_bot_task`.
- Other activity on an issue assigned to the bot, by a person or another bot (a state change, an
  edit), arrives as a webhook delivery with `bot_context` (`status_changed`, `assigned`).
- The bot's own actions never come back to it as a task: its own state change, edit, comment,
  self-assignment or AI-task completion arrives without `bot_context`.

So a route that should run on assignment matches `trigger_kinds = ["assigned"]` on the worker AI
task; `created` and `mentioned` on a webhook delivery are no longer sent by current Sylvode
versions (older servers still send them, which is why the values are still accepted).

## Features

- **HMAC-SHA256 signature verification** — Validates webhook authenticity
- **Agent routing** — Each bot task goes to exactly one agent, chosen by bot identity, agent type
  and optional route constraints (project type, trigger kind, event, form key)
- **Agent types**:
  - `openclaw` — Send the rendered message via the OpenClaw CLI (`openclaw message send`)
  - `openprx` — Send the rendered message via an OpenPRX Signal API or a command
  - `webhook` — Forward the delivery JSON unchanged to an HTTP endpoint
  - `custom` — Run a command with the rendered message as an argument
  - `runner` — Run a CLI agent (claude, codex, opencode) in the project's directory, in a tmux session or headless, as Sylvode's execution settings say (see [`runner` agents](#runner-agents-project-execution))
- **Task protocol** — A `runner` agent ends its tasks through the task tools; a forwarding agent whose hand-over fails fails the task with `POST /api/v1/ai/tasks/{id}/fail` (see [Ending tasks](#ending-tasks))
- **WSS tunnel client (Phase B MVP)** — Active ws/wss connection with Bearer auth, heartbeat, auto-reconnect. No Sylvode server endpoint exists yet, so it has nothing to connect to (see [the tunnel section](#phase-b-tunnel-wss-mvp))
- **Tunnel envelope + HMAC** — Minimal envelope (`id/type/ts/agent_id/payload/sig`) with optional HMAC-SHA256
- **Task bridge** — Handles `task.dispatch` -> immediate `task.ack` -> async `task.result`
- **Message templates** — Customizable message and prompt format with placeholders
- **Configurable** — TOML-based configuration

## Quick Start

The runner lives in `apps/runner` of the [Sylvode](https://github.com/openprx/sylvode)
repository and is built as the `sylvode-runner` package of its Cargo workspace. Run these
commands from the repository root:

```bash
# Build
cargo build --release -p sylvode-runner

# Configure
cp apps/runner/config.example.toml config.toml
# Set webhook_secrets to the secret of your Sylvode webhook; start-up is refused while the
# example placeholder is still there

# Run
./target/release/sylvode-runner config.toml
# sylvode-runner listening on 127.0.0.1:9090
```

The runner runs on Unix only (Linux and macOS): it stops executors as process groups and handles
SIGTERM. On any other target the crate is empty and the executables print `sylvode-runner is not
supported on this platform` and exit 1.

The Rust toolchain is pinned by the workspace `rust-toolchain.toml`, which rustup installs on the
first build. The crate uses the workspace `[lints]`; CI runs clippy with `-D warnings`, so
building with a different compiler version can fail there on a lint that version added.

The example configuration is secure by default: it listens on loopback only, verifies every
signature (`allow_unsigned = false`) and contains no agent. Change `listen` once the secret is set, or put a reverse proxy in front.

`sylvode-runner [CONFIG]` reads `config.toml` in the working directory when no path is given.
`--help` and `--version` print usage and the version.

### Configure the Sylvode webhook

In Sylvode, create a webhook pointing to this receiver:

- **URL**: `http://your-server:9090/webhook`
- **Secret**: Must match one of `webhook_secrets` in `config.toml`. Leave it empty to have Sylvode
  generate one: the create response (`POST /api/v1/workspaces/{id}/webhooks`) returns it in
  `secret`, once, and `POST .../webhooks/{webhook_id}/rotate-secret` issues a new one the same way
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

# Feature gates (safe defaults).
[features]
tunnel_enabled = false

[runtime]
cli_max_concurrency = 1               # headless CLI runs of `runner` agents at once; others wait
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

```

`allow_unsigned = true` turns signature verification off and logs a warning at start-up. On a
`listen` address other than loopback (exactly `127.0.0.0/8`, `::1` or `localhost`; not
`localhost.example`, an IPv4-mapped address or any other name) it is refused unless
`allow_unsigned_non_loopback = true` acknowledges that anyone who can reach the port can trigger
agents. With `allow_unsigned = false` and no `webhook_secrets`, every request is answered `401`,
and start-up logs a warning saying so.

Start-up is refused, with a message naming the agent, when a `route.trigger_kinds` value is not one
Sylvode sends, or when an `openclaw`, `openprx` or `custom` `command` contains whitespace.

Commands run without a shell, so `command` is one program name and its arguments go in `args`:
`command = "openclaw"` with `args = ["message", "send"]`. A `command` with whitespace is refused
with ``agent `X`: openclaw.command must name one program; put its arguments in openclaw.args``.
Text inserted into arguments later is never split.

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

For a worker AI task, the bot's agent type is `ai_participant_agent_type` only; the task's inner
`payload` never chooses it. The issue that the prompt names and every callback writes to depends on
the task's `reference_type`:

| `reference_type` | Queued by Sylvode when | Issue |
|---|---|---|
| `work_item` | an issue is created with, or reassigned to, the bot | `reference_id`; `payload.issue_id` is ignored |
| `comment` | a comment mentions the bot | `payload.issue_id` (`reference_id` is the comment) |
| `proposal`, none, other | a proposal opens for voting, or a task created by hand | none (`unknown`) |

For `comment` tasks the issue comes from the payload because the task carries it nowhere else;
Sylvode's comment route sets it to the issue the comment was posted on. Residual trust: a project
admin can create a task by hand (`POST /api/v1/projects/{project_id}/ai/tasks`) with any reference
and any payload, so for such a task the issue is whatever that admin wrote, in `payload.issue_id`
for a `comment` reference and in `reference_id` for a `work_item` reference. Only the Sylvode API
can check, when it receives the callback, that the bot may write to that issue.

An agent without a route matches every task. A route matches when every non-empty list contains
the task's value, compared ignoring case and surrounding whitespace; a value missing from the task
fails a non-empty list. A list that is omitted or empty restricts nothing. `bot_names` and
`bot_ids` together form one constraint: when neither is set it restricts nothing, and when one or
both are set the task's bot must match at least one entry of a list that is set (either list is
enough when both are set). A route with only `bot_ids = ["bot-1"]` therefore rejects every other
bot; before the Sylvode-aligned release it matched every bot (see the root
`CHANGELOG.md`). When nothing matches, the answer is `{"status":"no_agent", ...}` and nothing runs.

`trigger_kinds` is compared with `bot_context.trigger_reason` (or `bot_context.trigger_kind`) on a
webhook delivery and with the top-level `trigger_kind` on a worker AI task, and with nothing else: a
task without that field fails a non-empty `trigger_kinds`. These are the values Sylvode sends; any other value refuses
start-up:

| Source | Values |
|--------|--------|
| Webhook delivery (`bot_context.trigger_reason`) | `assigned` (`issue.assigned`, `issue.updated` and every event not listed here), `created` (`issue.created`), `status_changed` (`issue.state_changed`), `mentioned` (`comment.created`), `completed` (`ai.task_completed`), `failed` (`ai.task_failed`). Current Sylvode sends only `assigned` (for an edit such as `issue.updated`) and `status_changed` on a webhook delivery; the assignment and the mention themselves arrive as worker AI tasks (see [How It Works](#how-it-works)) |
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
  any argument the message is appended as the last argument. If text from Sylvode would make an
  argument start with `-` (so it could act as an option), the command is not run, unless an earlier
  argument is `--`. A leading `-` you write yourself, in an argument such as `--text={message}` or
  at the start of `message_template` such as `- {title}`, is left alone.
- `webhook` posts the delivery JSON unchanged to `url`, signed with `X-Webhook-Signature:
  sha256=<hex>` when `secret` is set.

Commands are run directly, never through a shell, so `command` names one program and everything
else goes in `args`. Their stdin is `/dev/null`; they inherit the service's environment.

#### Stopping commands

On Linux and macOS every command (the commands of `openclaw`, `openprx`, `custom` agents and the
headless runs of `runner` agents) is started as the leader of its own process group. When it times out
(`max(http_timeout_secs, 60)` seconds for the forwarding agents), when the service
shuts down, and also when it exits by itself, the whole group is sent `SIGTERM` and, if anything
in it is still alive 5 seconds later, `SIGKILL`. This stops the processes an executor started
(an npm-installed `codex` launcher's native binary, an agent's tool calls), not only the executor
itself. A process that deliberately leaves its process group (`setsid`, `daemon`) is not
reached. The failure report of a headless run is sent only after the group is gone.

The group is signalled only while its leader (the command itself) has not been reaped, because
until then the kernel cannot give the group id to another process. On Linux the service notices
the command's exit without reaping it and reads the remaining members from `/proc`. On macOS a
command's exit can only be noticed by reaping it, so a command that exits by itself is not
followed by signals to its group (anything it left running there keeps running); on a timeout or
shutdown the group gets `SIGTERM`, the full 5 seconds, then `SIGKILL`. A group the service is not
permitted to signal (`EPERM`, such as a set-user-ID program) is reported and left alone instead of
being waited for.

In a container, run the image with an init process (`docker run --init`, or `init: true` in
compose): without one, processes orphaned by an executor are never reaped and pile up as
zombies.

### Shutdown

On `SIGTERM` or `SIGINT` the service stops accepting connections and lets the requests in flight
finish. Every running headless CLI run is stopped as above and its task is failed (`retryable`
when the run was cut short), so no task is left open by a run that no longer exists. tmux
sessions keep running. The whole shutdown, the requests in flight and these reports together,
takes at most `5 + 2 × http_timeout_secs + 5` seconds (40 seconds by default) from the signal: a
request still open then is closed, a report not yet sent is dropped, and the service exits 0. Give
the service manager at least that long: systemd's default `TimeoutStopSec` (90 s) is enough,
`docker stop` needs `--time 45` (its default is 10 s).

### Environment

| Variable | Effect |
|----------|--------|
| `SYLVODE_RUNNER_SAFE_MODE` | `1`/`true`/`yes`/`on` forces the tunnel and the `[ai_task_callback]` reports off at runtime. `openclaw`, `openprx` and `custom` agents still run their commands |
| `RUST_LOG` | Log filter. This service's events use the `sylvode_runner` target, e.g. `RUST_LOG=sylvode_runner=debug`; the default is `sylvode_runner=info` |

Logs are written to stdout.

## `runner` agents: project execution

Connecting a bot to a project in Sylvode issues a configuration block (`[security]`,
`[ai_task_callback]` with the bot's token, `[runner]`, and an `[[agents]]` entry with
`agent_type = "runner"`). A `runner` agent executes the task according to the project's execution
settings in Sylvode (channel and CLI), in the project's working directory; it takes no agent type table,
because the permissions come from `[runner.cli.*]` and the directory from Sylvode. (`agent_type =
"cli"` was removed: use a `runner` agent with the project's channel set to `headless`.) Every
`[runner]` table refuses unknown keys, so a block written for a newer runner is refused naming the
key.

```toml
[runner]
state_dir = "~/.local/state/sylvode-runner"   # macOS default: ~/Library/Application Support/sylvode-runner
allowed_roots = ["~/work"]                    # project directories must lie below one of these; state_dir must NOT
# A runner agent may carry its own `allowed_roots` in its [[agents]] table; with more than one
# runner agent each needs its own, and they must not overlap.
delivery_check_timeout_secs = 5               # task fetch and checks before the answer; slower is HTTP 503
heartbeat_secs = 600
# report_interval_secs = 600                  # overrides the interval Sylvode advertises

[runner.tmux]
enabled = false                               # the tmux channel is an operator opt-in (see below)

[runner.cli.claude]                           # claude is always enabled
binary = "claude"
permission_mode = "dontAsk"                   # the only mode a runner agent accepts
allow_write_call = false
[runner.cli.codex]
enabled = false
[runner.cli.opencode]
enabled = false
[runner.mcp]
tool_mode = "search"
```

A `runner` agent is refused at start-up (the other agents still start) unless
`[ai_task_callback] enabled = true`, `allowed_roots` is not empty, and every bot of its route has a
token registered for the `mcp_stdio` transport (`surface = "mcp_stdio"`): `bot <id>: runner bots
need a token registered for mcp_stdio`. The entry also carries `workspace_id`, the workspace the bot
belongs to: the runner writes it into the per-bot MCP file, and the MCP server it starts for the
CLI refuses to start without it.

**Allowed roots.** Each entry is checked once at start-up: only a leading `~/` is expanded; the path
must be absolute, exist and be a directory, and is canonicalised. `/`, the home directory (or a
directory that contains it), system directories (`/etc`, `/usr`, `/System`, `/Library`,
`/opt/homebrew`, `/private/etc` and the like) and any path containing `#` (tmux reads a directory as
a format) are refused. A world-writable root is warned about. One invalid entry makes every `runner`
agent refuse its tasks with `reason_code = "runner_agent_disabled"`, and the log says why.

**Per delivery**, before the answer, in this order: the `protocol` field (`protocol_mismatch` when
missing or different); the task view, fetched with the bot's token (a timeout or a 5xx is HTTP
`503` so the worker re-queues; a task that is no longer active is answered `ignored`); the cross-check
of project id and key (`delivery_mismatch`); `execution.enabled` (`execution_disabled`); the project
key `^[A-Z0-9]{1,32}$` (`invalid_project_key`) and that no other project on this runner uses it
(`session_name_collision`); the working directory (`path_missing`, `path_unreadable`,
`path_not_directory`, `path_outside_allowlist`); the channel (`tmux_unavailable`: tmux missing or older than 3.2) and the CLI (`cli_disabled`, `cli_unavailable`). The directory is
canonicalised (every symbolic link resolved) and must lie below an allowed root, compared by path
components (`/work/svwl-evil` is not under `/work/svwl`); only that canonical path is used from then
on. A refusal is `{"status":"refused","reason_code":"...","reason":"...","agent":"..."}`; the reason
never contains a path (the path is in the runner log at `info`). A directory replaced by a link
between the check and the use is out of scope: only a process that can write inside an allowed root
(the same OS user) can do that.

**What a task can do, and what protects what.** A `runner` agent keeps a CLI agent running in the project's working directory, as the runner's OS user. Anyone who can assign an issue to the bot, mention the bot in a comment, or request its vote (in Sylvode today, any workspace member) can make that agent act, and anyone who can edit an issue title, description or comment can put text in front of it. The defaults let the agent read files in the project directory (`Read`, `Glob` and `Grep` confined to it) and use the Sylvode task tools, nothing else. Read this plainly: **file modes do not protect one bot's token from an agent running as the same OS user.** The state directory is 0700 and the token files are 0600, but the CLI runs as that same user, so inside one OS user the only barrier is the CLI's permission posture (the allow list, the deny rule that keeps the state directory unreadable, `dontAsk`). If you allow shell commands (`Bash`) or file writes, a task can run commands with the runner user's rights, including everything that user's credentials reach and every other session on the tmux socket (`TMUX` is set in each pane, and any agent with a shell can drive the other sessions). Separate trust domains (workspaces, or bots whose assigners you do not trust equally) therefore need **separate OS users, or separate hosts**; one runner OS user is one trust domain. Widen the defaults only for a bot whose assigners you would trust with a shell on that machine. Per-agent `allowed_roots` and the refusal of a `state_dir` inside a root only keep one project from being *named* as another's directory; they are not a sandbox. codex's `read-only` sandbox restricts writes and network, not reads: it can read the whole disk, which is one reason codex stays refused on the tmux channel (`cli_unverified`) and why its headless use is for trusted bots only.

**Interactive session command lines.** claude runs as `/usr/bin/env CLAUDE_CONFIG_DIR=<state_dir>/claude/<bot> claude --permission-mode dontAsk --mcp-config <state_dir>/mcp/<bot>.claude.json --strict-mcp-config --setting-sources user --settings <state_dir>/sessions/<KEY>/claude-settings.json --allowedTools ... --disallowedTools Read(<state_dir>/**) ...`: the allow list is the eight search-mode Sylvode tools plus `Read`, `Glob` and `Grep` confined to the canonical working directory; `allow_write_call = true` adds `sylvode_write`; `allowed_tools` and `disallowed_tools` add entries (`{dir}` stands for the canonical directory); the state directory (in the configured and the canonical form) and the configuration file are always denied to reads, and a configuration file inside an allowed root refuses that root. Extra read rules count as confined only in the exact form `Read|Glob|Grep(/{dir}/...)` with no `..` or `~`; anything else is a widening and is reported as such. A project's own `.mcp.json` is ignored. A runner agent accepts only `permission_mode = "dontAsk"` (anything that would ask is denied); any other value is a configuration error. codex and opencode on the tmux channel are refused whatever the settings say. No command line holds a token, and none is run by a shell. The state directory is created with mode 0700; the per-bot MCP file that holds the token and the hook settings are created with mode 0600, exclusively, and are rewritten only when they differ.

**What a CLI run is given.** Both channels start claude with `--mcp-config <state_dir>/mcp/<bot>.claude.json --strict-mcp-config` (MCP servers only from the runner's file: no `.mcp.json`, no project or user servers), `--setting-sources user` (checked against `claude --help` of 2.1.330: the help names `user`, `project` and `local` as the sources; the project and local files of a project, which could add allow rules, hooks or MCP servers, are not loaded; this README does not claim that `CLAUDE.md`, project skills or commands are excluded: hand check V3) and an explicit `--allowedTools` list. **The operator's own `~/.claude` is not loaded**: the runner sets `CLAUDE_CONFIG_DIR` to `<state_dir>/claude/<bot>` (created 0700, one per bot, holding only what claude writes there), the documented way to relocate claude's configuration directory (user settings, hooks, plugins, credentials and trust state live there; `claude --help` of 2.1.330 lists no flag for it, so this rests on the documented environment variable and hand check V16). The consequence: claude has no login in that directory, so **once per bot** the operator logs in with it (`CLAUDE_CONFIG_DIR=<state_dir>/claude/<bot> claude`, then `/login`), or gives the CLI an API key through `[runner.cli.claude] env_passthrough = ["ANTHROPIC_API_KEY"]`. `CLAUDE_CONFIG_DIR` itself cannot be passed through. codex and opencode have no verified equivalent, so a headless run of them is refused (`workspace_untrusted`) in a directory with `.codex` or `opencode.json`/`.opencode` unless the project key is in `[runner] trust_project_files`. The CLI's environment is the runner's allow-list (`PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LANGUAGE`, `LC_*`, `TERM`, `TZ`, `SHELL`, `TMUX_TMPDIR`) plus the names in `[runner.cli.<cli>] env_passthrough` (an API key, a proxy); nothing else of the runner's environment reaches it. A tmux pane inherits the environment of the tmux server on the runner's socket: a server that was started by something other than the runner (a shell of the operator) is not covered by this list.

**The tmux channel (off by default).** `[runner.tmux] enabled = false` is the default: a project whose execution settings ask for the tmux channel is refused at delivery with `reason_code = tmux_channel_disabled`, and the headless channel keeps working. codex and opencode on the tmux channel are refused entirely (`cli_unverified`), whatever the switch says. **Do not enable it on a machine that matters before the hand checks V1 to V16 below have been done on the pinned claude version**: nothing in its screen and prompt logic has met a real CLI.

With the switch on, a delivery is queued on the session named by the project key (one worker per key, first in first out). The runner creates the session (`tmux -L <socket_name> -f /dev/null`, window 0, the CLI as one argument vector); on a socket of its own it never adopts a session it did not create (no matching `session.json`), and it checks the directory again before it creates or restarts one. It types a task only when the CLI is idle by its hooks, its prompt is on the screen and empty, and no attached client typed within `user_idle_secs`. Turn events come from the CLI's hooks through `sylvode-runner hook --cli C --session K --event E --socket P --nonce N`, registered in the per-session settings file (`SessionStart` for `startup` only and `Notification` for permission prompts only; a session start after compaction, `/clear` or resume means nothing); the nonce is random, written into the hook command of that launch only, compared in constant time, and the hook socket (`<state_dir>/hook.sock`, mode 0600) hears only the runner's own user. The push is recorded in `session.json`, synced, before it is typed, so a restart never types a task again: an acknowledged task, a recorded push or a message in the scrollback counts as pushed (supervised, or failed as retryable when its session is gone), and a second open task of one session is failed. The one Enter retry re-reads the screen and the foreground command and requires the pasted text to still be on the input line; otherwise nothing is pressed and the session is reported blocked. What the runner reads from the screen (busy, a choice, a dialog) is read from the CLI's own chrome at the bottom of the screen, never from the transcript above it, so text in an issue cannot pose as a marker. A screen that reads busy or waiting while the hooks say idle holds the queue back for at most `screen_hold_secs` (default 600); then the waiting tasks fail as retryable with the reason. A pushed task that the CLI acknowledged on the screen counts as running even if its `UserPromptSubmit` hook was lost. A hook message must carry the launch's nonce, come from the runner's own user, and (where the platform names the sender) from below the session's pane process; with the session record unreadable nothing is admitted, and connections are capped (32 at once). On Linux the sender's parents are read from `/proc`; macOS has no safe call for that in this code base, so one `ps -A` snapshot is read per admitted message. Hook messages are ordered by a per-session counter file (`sessions/<KEY>/hook.seq`, reset per launch), not by the clock. A trust, login or model-chooser dialog fails the waiting tasks without a retry and names `sylvode-runner trust KEY`. A second runner on the same state directory refuses to start (`runner.lock`). Sessions keep running when the runner stops; after a restart the runner rebuilds its state from `GET /ai/tasks/active`.

**Hand checks before the tmux channel is enabled** (pinned claude version, a throwaway directory, the runner user, a socket of its own):

- **V1.** Start claude exactly as the runner builds it (`--setting-sources user --settings <file with the four hooks>`); confirm that `SessionStart`, `UserPromptSubmit`, `Stop` and `Notification` each reach `hook.sock`.
- **V2.** In a directory with `.claude/settings.json` = `{"permissions":{"allow":["Bash(touch:*)"]},"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"touch /tmp/v2-hook"}]}]}}` and a `.mcp.json`, interactive and `-p`: ask for `touch /tmp/v2-bash`. Neither file may appear, and no MCP approval prompt may show.
- **V3.** In the same directory: `.claude/skills/x/SKILL.md` with `allowed-tools: Bash(touch:*)`, `.claude/commands/x.md` with `allowed-tools: Bash(touch:*)`, `.claude/agents/x.md`, and a `CLAUDE.md` with a marker sentence. Record which load under `--setting-sources user` and whether invoking the skill or command grants `Bash`; also whether the runner's own `.claude/skills/sylvode-mcp/` loads.
- **V4.** With the operator's real `~/.claude/settings.json`: confirm that its allow rules, hooks and plugins do NOT apply (the runner's `CLAUDE_CONFIG_DIR` is in effect).
- **V5.** With the default `--allowedTools` and `dontAsk`: commit `link -> <state_dir>/mcp/<bot>.toml` and `dirlink -> ~/.ssh` into the project. Ask the agent to Read `./link` and to Glob or Grep `./dirlink`. Both must be denied; repeat with the `--disallowedTools` rule on the state directory to see whether deny rules see resolved paths.
- **V6.** `/compact`, an automatic compaction (fill the context), `/clear` and `--resume`: does `SessionStart` fire for each, and does the `startup` matcher keep it away?
- **V7.** Leave the prompt idle for 70 s: does `Notification` fire, with which `notification_type`; does the narrow matcher keep it away?
- **V8.** `capture-pane -p` of each screen: idle; busy (spinner, "esc to interrupt", input box); busy with a typed draft; a multi-line draft whose first line is empty; the placeholder text in an empty box; the trust dialog and the login prompt; a permission dialog under `default` mode; an `AskUserQuestion` dialog; plan approval. For each, record what `input_is_empty` and `classify_dialog` return. Every dialog must give `None` or `Some(false)`, busy must be distinguishable, and an empty box with placeholder text must give `Some(true)`.
- **V9.** `#{pane_current_command}` for the native installer and for an npm install of claude, on macOS and Linux (`claude`, `claude.exe`, `node`, a version-number name?). Set `process_names` accordingly, and never include `node` or a shell.
- **V10.** `#{bracket_paste_flag}` with claude on tmux 3.7. On 3.2 to 3.6, confirm claude enables bracketed paste before the first prompt, so that the per-push confirmation is not a lie.
- **V11.** Paste a message and press Enter while claude is busy: queued, interrupting, or lost?
- **V12.** Interrupt a turn with Esc: does `Stop` fire?
- **V13.** codex `-s read-only -a never` (relevant before codex is ever allowed on tmux): can a command read `<state_dir>/mcp/*.toml`, connect to `hook.sock`, and run `tmux -L sylvode list-sessions`?
- **V14.** The trust flow: a fresh directory shows the dialog, `sylvode-runner trust KEY` attaches, answering it gives `SessionStart`, and the next task is typed.
- **V15.** Start the runner while a `-L sylvode` server already runs from a full shell; check `tmux -L sylvode show-environment -g` and a pane's `env`.
- **V16.** With the runner's `CLAUDE_CONFIG_DIR`: the one-time login persists (macOS keychain, Linux `.credentials.json`), and the trust state is kept per configuration directory.

**Operator commands.** `sylvode-runner sessions [--prune] [--config PATH]` lists the sessions this runner created (key, project id, CLI, state); `--prune` (runner stopped) ends the idle ones whose key is no longer reserved or whose project now uses headless, and never touches a session whose pane id and PID are not the ones in `session.json`. `sylvode-runner trust KEY` prints what to answer and attaches (`tmux -L <socket> attach -t =KEY`). `sylvode-runner release KEY` frees a reserved session name, only when its tmux session no longer exists and the runner is stopped.

**Signing secrets and URLs.** A runner that serves more than one bot needs `webhook_secret` on every `[[ai_task_callback.bots]]` entry: a delivery for a bot is verified only with that bot's secret (with one bot, `security.webhook_secrets` serves as before). The Sylvode API URL must be `https://` unless its host is loopback. A `runner` agent takes no `[agents.ai_task_callback]`, and a delivery for a bot the agent does not serve is refused (`runner_agent_disabled`). The report interval Sylvode advertises is kept between 60 and 3600 seconds. At start-up one `runner CLI posture` line per CLI says what it may do; opencode is warned about (its permissions are not controlled by sylvode-runner).

**Headless channel.** The CLI runs once in the canonical directory with the permissions of
`[runner.cli.*]`. Its prompt is `[Sylvode task] id=... attempt=n/max` followed by the task procedure
(get, ack, progress, complete or fail). The runner reports `runner.pushed` and, while the CLI runs,
`runner.alive` every `heartbeat_secs`. When the process ends and Sylvode still shows the task
active, the runner fails it (`source: "runner"`, not retryable): `agent ended without completing or
failing the task`.

**Handshake and report.** At start-up and every report interval the runner calls
`GET /api/v1/ai/protocol` with each bot's token and compares `task_protocol` with its own (1), then
sends `PUT /api/v1/ai/runner/report`: component names, statuses and versions only. A mismatch shows in
`/health` and refuses every `runner` agent with `protocol_mismatch`; other agents keep working. A bot
without a runner connection in Sylvode (404) is logged once and not reported again until restart; a
refused token is logged as `bot <id>: token refused by Sylvode (revoked, rotated or wrong surface)`.
Tokens are never logged, never part of an error, and never follow a redirect.

## Ending tasks

The Sylvode worker marks an AI task `processing` when it delivers it (a delivery that carries
`task_id` and `ai_participant_id`), and only the task protocol ends that state:
`POST /api/v1/ai/tasks/{task_id}/complete` or `.../fail?attempt=N`, with the bot's token. There is
one way to end a task:

- **`runner` agents.** The CLI agent ends the task itself with the task tools
  (`sylvode_task_complete` or `sylvode_task_fail`); the runner fails a task only when it cannot
  go on (the session was killed, a dialog is blocking, the agent did not acknowledge it; see the
  [`runner` agents](#runner-agents-project-execution) section).
- **Forwarding agents** (`webhook`, `openclaw`, `openprx`, `custom`). When the hand-over worked the
  runner completes the task (`outcome: done`, a summary naming the agent), so it runs once: its
  lease never expires into a redelivery that would run the forwarded work again. When the hand-over
  itself fails, or no agent matches the task, the runner fails the task with `source: "runner"`
  and the token of the task's bot (`retryable: true` for a failure a repeat may fix, `false` for
  a task no agent takes). The reason is a fixed summary (`error: the command exited with status
  3`), never command output.

The reports need `[ai_task_callback]` (the bots' tokens). A token works only for the tasks of the
bot that owns it, so a task whose bot has no token is not reported (and logged); the worker then
ends it by its processing timeout. The worker also fails a task this receiver declines in its
answer (`no_agent`, `ignored`, `refused`).

```toml
[ai_task_callback]
enabled = false                     # default
api_url = "http://127.0.0.1:8080"   # base URL of the Sylvode API; required when enabled
token = "opr_..."                   # the bot's token; bot_id = "<uuid>" limits it to that bot
surface = "rest"                    # the transport the token is registered with
max_attempts = 3                    # attempts per report, including the first (1 to 10)
retry_delay_ms = 1000               # before the first retry, doubled each time (at most 60000)

[[ai_task_callback.bots]]           # one entry per further bot; mcp_stdio for a `runner` agent
bot_id = "00000000-0000-0000-0000-000000000000"
workspace_id = "00000000-0000-0000-0000-000000000000"   # the bot's workspace; a `runner` agent needs it
token = "opr_..."
surface = "mcp_stdio"
```

A report names the attempt (`?attempt=N`, the delivery's `attempts`): it is accepted only while
that attempt is active. A refusal is never retried; a timeout, a transport error and an HTTP 5xx
are, `max_attempts` times in all.

## Outbound webhook signature

When forwarding via `agent_type = "webhook"` and `agents.webhook.secret` is configured,
sylvode-runner signs the outbound JSON body and sends:

- Header: `X-Webhook-Signature`
- Value format: `sha256=<hex_hmac>`

## Phase B: Tunnel (WSS) MVP

**Status: client only, nothing to connect to yet.** The tunnel client is implemented and tested
only against an in-process test peer (`tests/e2e/tunnel.rs`). No Sylvode server endpoint exists
yet: the Sylvode API has no `/api/v1/agent-tunnel` route (the URL below is a placeholder), and no
server implements the `task.dispatch` / `task.ack` / `task.result` / `heartbeat` / `error`
envelopes described here, which are this repository's draft protocol. Enabling the tunnel today
connects to nothing: the client retries with backoff and logs `tunnel connect failed`. Use the
webhook path for Sylvode.

Enable both `[features].tunnel_enabled = true` and `[tunnel].enabled = true` in `config.toml` to
let `sylvode-runner` actively connect to a control plane. The tunnel needs `url` (`wss://`, or
`ws://`), `agent_id` and `auth_token`. The tunnel runs no tasks any more: `agent_type = "cli"` was
removed, so every `task.dispatch` is acknowledged and answered with a `failed` `task.result` that
says so.
`ws://` sends the bearer token and every task unencrypted, so a `ws://` URL whose host is not
loopback refuses start-up unless `allow_insecure_transport = true` acknowledges it.

```toml
[tunnel]
enabled = true
url = "wss://sylvode.example.com/api/v1/agent-tunnel" # placeholder: no Sylvode endpoint exists yet
agent_id = "vano-qa"                                  # required; there is no default
auth_token = "opr_xxx"                                # Authorization: Bearer <token>
reconnect_secs = 3
heartbeat_secs = 20
hmac_secret = "a-long-random-shared-secret"           # optional; see "Signature behavior"
require_inbound_sig = false                           # only changes the error reason; see below
# allow_insecure_transport = true                     # acknowledge ws:// to a host other than loopback
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
3. Run nothing: the runner no longer runs tasks over the tunnel
4. Send `task.result` (`run_id`, `issue_id`, `status = failed`, `summary`)

Other envelope types received are ignored.

Signature behavior:

- With `tunnel.hmac_secret` set, every outbound envelope carries `sig` (HMAC-SHA256 over the
  envelope without `sig`), and every inbound envelope must carry a valid `sig`: one that is
  missing or wrong is answered with an `error` envelope (`reason: bad_signature`, or
  `missing_signature` when `require_inbound_sig` is also set) and dropped.
- With `hmac_secret` set, a validly signed envelope is also refused (and answered with an `error`
  envelope) when its `ts` is more than 300 seconds from the receiver's clock (`stale_envelope`) or
  its `id` repeats one accepted on this connection (`duplicate_envelope`, the last 4,096 ids are
  remembered).
- Every inbound envelope whose `agent_id` is not this agent's is refused (`wrong_agent`).
- A refused envelope of type `error` is dropped without an answer, so two ends that both answer
  refusals cannot keep answering each other.
- Without `hmac_secret`, inbound `sig` is not verified and inbound envelopes are not
  authenticated: anyone who can impersonate the control plane can dispatch tasks. Use `wss://` and
  set `hmac_secret`. A blank or example `hmac_secret` refuses start-up while the tunnel is enabled,
  and is a warning otherwise.
- `require_inbound_sig` adds nothing to security: with `hmac_secret` every envelope must already
  carry a valid `sig`, and without it start-up is refused. It only makes a missing `sig` reported
  as `missing_signature` instead of `bad_signature`.

Safety toggles:

- `SYLVODE_RUNNER_SAFE_MODE=1` forces `tunnel/cli/callback` off at runtime.
- This provides one-command rollback to legacy webhook-only behavior. It does **not** stop
  `openclaw`, `openprx` (command) or `custom` agents, which are part of that behaviour and run
  their configured command for every matching bot task; remove or comment out such agents to stop
  them.

## Templates

`message_template` sets the message of `openclaw`, `openprx` and `custom` agents; `webhook` agents
send no message. Without a template this default is used:

| Template | Default |
|----------|---------|
| `message_template` | `[{project}] {event}: {key} {title}` and, on a second line, `{actor} \| Trigger: {reason}` |

A template is rendered in one pass: a placeholder below is replaced by its value, any other text is
kept as written, and a placeholder that appears inside a value (such as an issue title containing
`{actor}`) stays literal.

| Placeholder | Value | In `message_template` |
|-------------|-------|:---:|
| `{event}` | Event (`issue.assigned`), or a worker task's `task_type` | yes |
| `{title}` | Issue title (`untitled` when absent) | yes |
| `{reason}` | Trigger reason (`assigned`, `mentioned`, ...; see [Agent selection](#agent-selection)) | yes |
| `{issue_id}` | Issue id | yes |
| `{project_id}` | Project id | yes |
| `{form_id}`, `{form_key}`, `{record_id}` | Form identifiers of a form event | yes |
| `{key}` | Issue key (`BIL-4C3B2A19`) | yes |
| `{actor}` | Name of the user who caused the event | yes |
| `{project}` | Project name | yes |
| `{workspace}` | Workspace name | yes |
| `{state}`, `{priority}` | Issue state and priority | yes |
| `{url}` | `issue/<issue_id>` | yes |

A value that is missing renders as an empty string (`unknown` for `{event}`, `{reason}` and
`{actor}`, `untitled` for `{title}`). In a `custom` agent's `args`, `{message}` stands for the
rendered message.

## API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/webhook` | POST | Receive a webhook delivery or worker AI task |
| `/health` | GET | Health check; answers `{"status":"ok","protocol":{"runner":1,"api":1,"state":"ok"}}`. `state` is `ok`, `mismatch`, `unreachable`, `pending` (not checked yet) or `unchecked` (no `runner` agent) |

`/webhook` answers `401` for a missing, invalid or conflicting signature or a stale delivery, `409`
for a replayed one, and `400` for a body that is not JSON. Otherwise it answers `200` with a JSON status: `ignored` (not a bot task), `no_agent`
(no agent matched), or `dispatched` with the agent id and a short `result`: `ok`, `error: the
command exited with status N`, `webhook: 200 OK`, the reason a dispatch was refused; for a `runner` agent `dispatched` also carries the channel and, for tmux, the queue position. The output of a command and the body of a
remote error are never part of the answer, because the sender stores it in its delivery log; they
are written to this service's log (`Dispatch result` with `detail=...`).

### Signature headers

The request signature is `sha256=<hex>` (the prefix is optional), an HMAC-SHA256 of the raw body
keyed with one of `webhook_secrets`. It is accepted in any of these headers:

| Header | Status |
|--------|--------|
| `X-Sylvode-Signature` | Accepted |
| `X-Webhook-Signature` | Accepted; the header the Sylvode API sends today |

Every signature header present is read. When they carry different signatures (or one header is
repeated with different values) the request is refused with `401`, even if one of them is valid: an
invalid signature cannot ride along a valid one. A non-ASCII signature value is refused the same
way. With `allow_unsigned = true` signature headers are not read at all.

### Replayed requests

A signature proves who wrote a body, not when it was sent. For a signed request, two fields inside
the signed body are therefore checked as well: a webhook delivery whose `timestamp` is more than
`max_delivery_age_secs` (default 300) away from the receiver's clock, in either direction, is
refused with `401`, and a delivery whose `id` was already received (for a worker AI task: the same
`task_id` and `attempts`) is refused with `409`. Keep the clocks of Sylvode and this receiver in
sync.

What this covers, and what it does not:

- It covers signed HTTP deliveries only (with `allow_unsigned = true` nothing is checked), and
  only through the fields a body carries at its top level.
- A webhook delivery from the Sylvode API carries both `id` and `timestamp`, so it is checked for
  freshness and uniqueness.
- A worker AI task carries `task_id` and `attempts` but **no timestamp**, so it is only checked for
  uniqueness, and only while its key is remembered: for twice `max_delivery_age_secs` (with the
  default 300, about 10 minutes; never less than 10 minutes), up to 10,000 keys. A captured worker
  task can be replayed after that.
- The memory is held in the process and **cleared on restart**: a delivery received before a
  restart is accepted once more afterwards (a webhook delivery only while its timestamp is fresh).
- A body without these top-level fields is not checked. Sylvode does send such bodies: a flow
  event delivery (`{"delivery": {...}, "event": {...}}`) has neither, and is ignored anyway because
  it is not a bot task.

## Deployment

### Installer (systemd / launchd)

```bash
./scripts/install.sh --binary-path /usr/local/bin/sylvode-runner --config-dir /etc/sylvode-runner
./scripts/uninstall.sh
```

The installer creates the user service `sylvode-runner.service` on Linux or the launch agent
`dev.sylvode.runner` on macOS. When no `--binary-path` is given it looks for `sylvode-runner` in the
working directory, in `./target/release` and in the workspace `target/release`. The unit or agent is
rewritten only when its content differs (an unchanged one is left alone and nothing is reloaded);
the configuration file is never written. The uninstaller removes the unit or agent.

Before anything is installed the installer runs `sylvode-runner doctor --mode <mode> --as-service`
with the `PATH` the service will have (not the shell's). A failed row prints its hint and stops the
installation: nothing is ever installed silently. The mode is always `auto`: the runner follows
its configuration, so tmux is required only with `[runner.tmux] enabled = true` (a headless-only
install, the default, needs neither tmux nor a flag). `--dry-run` stops after the check. On Linux a user without linger gets a warning with `loginctl enable-linger <user>` (the
service and its tmux server stop at logout otherwise). After the unit is installed, doctor runs
once more in the service's context: on Linux through `systemd-run --user`; on macOS through
`launchctl asuser`, which works for root only, so as a normal user that second check is skipped with
a warning (the first check already used the service's `PATH`). A problem there is a warning.

### Doctor

```bash
sylvode-runner doctor [--json] [--config PATH] [--mode tmux|headless|auto] [--as-service] [--project KEY] [--show-paths]
sylvode-runner --check                      # doctor --json with no network call and no secret
sylvode-runner config check [--json] [--config PATH]
```

`doctor` prints the `sylvode.doctor.v1` report of the shared prerequisite checker: tmux (version
against 3.2, server on the runner's socket), the CLIs, the MCP server (its `--check` version and
task protocol), this runner (configuration, service, linger), the API protocol, and with `--project
KEY` the project's row (directory against the allowed roots, trust, the state of the agent guide,
and the warning that a git worktree cannot get its exclude line). Exit code 0: no row failed; 2: a
row failed (warnings never change it); 1: the checker could not run. The output is meant for bug
reports: no token, no URL credentials, no configuration content, and project directories only as
labels unless `--show-paths` is given. In a container image the `tmux` row fails (`unsupported in
container image`) whenever a `runner` agent is configured.

`config check` says whether the configuration loads; it names the key (and table, and line) of each
problem and never prints a value.

A missing prerequisite does not stop the service: it starts degraded. `/health` is unauthenticated and
says nothing about it (only `status` and `protocol`); the log, `doctor` and the periodic report to
Sylvode carry the rows, every delivery that needs the
missing program is refused at once with a reason code (`tmux_unavailable`, `cli_unavailable`), and
start-up logs the resolved path of each program. The snapshot is probed at most once a minute, and
each delivery checks the program it needs again, so installing tmux takes effect without a restart.
An invalid `allowed_roots` or a protocol mismatch still refuses the `runner` agents only.

### Systemd

```ini
[Unit]
Description=Sylvode runner
After=network.target

[Service]
# A dedicated unprivileged user: cli executors (and any tool their permission settings allow) run as this user.
User=sylvode-runner
Group=sylvode-runner
ExecStart=/usr/local/bin/sylvode-runner /etc/sylvode-runner/config.toml
WorkingDirectory=/etc/sylvode-runner
Restart=always
# Running executors are stopped and report on SIGTERM; see Shutdown.
TimeoutStopSec=90

[Install]
WantedBy=multi-user.target
```

### Container image

```bash
docker run --init --stop-timeout 45 -p 9090:9090 \
  -v ./config.toml:/etc/sylvode-runner/config.toml:ro \
  ghcr.io/openprx/sylvode-runner:latest
```

Run it with `--init` (see [Stopping commands](#stopping-commands)) and `--stop-timeout 45` (see
[Shutdown](#shutdown)). The image runs `/app/sylvode-runner /etc/sylvode-runner/config.toml` as
the non-root user `sylvode` (uid 10001), so the mounted `config.toml` must be readable by uid
10001, and `listen` must not be loopback inside the container (use `0.0.0.0:9090`, with a secret).
The image contains only the runner executables and CA certificates: no `codex`, `claude`,
`opencode` or `openclaw`, so `runner`, `openclaw` and command-based agents need an image derived from
this one that adds them. See [`Dockerfile`](Dockerfile).

### Versioning

The runner is a Sylvode workspace member and carries the Sylvode version: `sylvode-runner
--version`, image tags and archive names all use the workspace version. The standalone 0.3.x line
ended with 0.3.53 (never released; the last published standalone release is 0.3.3). The first
Sylvode-versioned release is 0.5.0, which continues above that line.

### Release archives

Each release ships `sylvode-runner-<platform>.tar.gz` with a `.sha256`, for `linux-amd64`,
`linux-arm64`, `macos-amd64` and `macos-arm64`. Every archive contains the `sylvode-runner`
executable.

## Upgrading from 0.3.3

Upgrading from OpenPR Webhook 0.3.3 (the last published standalone release) is not a drop-in replacement for every configuration. Start-up
now refuses configurations that cannot work or are unsafe, and some behaviour changed. Before
upgrading, run the new binary once against your configuration; it exits 1 with one of the errors
below if something must change (`sylvode-runner config.toml`, then stop it with Ctrl-C).

#### Refused now

Each of these started with 0.3.3 and refuses to start now. The error is logged on stdout as
`failed to load config from <path>: <error>`, except where noted.

| # | Configuration | Error (excerpt) | Fix |
|---|---------------|-----------------|-----|
| 1 | `route.trigger_kinds` contains a value Sylvode never sends, such as `assignment` from the old examples | ``agent `X`: route.trigger_kinds contains `assignment`, which Sylvode never sends; valid values: assigned, created, ...`` | Use the values in [Agent selection](#agent-selection): `assigned`, and `mentioned` (not `mention`) for a comment mention on a webhook delivery |
| 2 | An `openclaw`, `openprx` or `custom` `command` with whitespace, such as `command = "openclaw message send"` | ``agent `X`: openclaw.command must name one program; put its arguments in openclaw.args`` | Write the program in `command` and its arguments in `args`: `command = "openclaw"`, `args = ["message", "send"]` |
| 3 | An enabled tunnel with `require_inbound_sig = true` and no `hmac_secret` | `tunnel.require_inbound_sig = true requires a non-empty tunnel.hmac_secret: ...` | Set `hmac_secret` to the secret shared with the control plane, or remove `require_inbound_sig` |
| 4 | An empty or blank entry in `security.webhook_secrets`, while signatures are verified (`allow_unsigned = false`) | `security.webhook_secrets[N] is blank; an empty HMAC key lets anyone sign a request. ...` | Set the real secret or remove the entry |
| 5 | A blank `tunnel.hmac_secret` while the tunnel is enabled (`[features].tunnel_enabled` and `[tunnel].enabled`) | `tunnel.hmac_secret is blank; an empty HMAC key lets anyone sign an envelope. ...` | Set the real secret or remove the setting |
| 6 | An example placeholder secret in `webhook_secrets` or `tunnel.hmac_secret`: one of `your-secret-here`, `shared-hmac-secret`, `replace_with_webhook_secret` (the Sylvode repository's compose example) or the new example's placeholders, or any value containing `replace_with`, `replace-with`, `your_`, `your-`, `changeme` or `example`, ignoring case and surrounding whitespace. Only a secret in use refuses: `webhook_secrets` unless `allow_unsigned = true`, `hmac_secret` while the tunnel is enabled; an unused one (such as the `hmac_secret = "shared-hmac-secret"` beside `enabled = false` in the old examples) is logged as a warning | `security.webhook_secrets[N] is still the example placeholder; ...` / `tunnel.hmac_secret is still an example placeholder; ...` | Set a long random secret, the same as on the Sylvode webhook or the control plane |
| 7 | `allow_unsigned = true` with a `listen` address other than loopback, as in the old `config.example.toml` (`0.0.0.0:9090`) | `security.allow_unsigned = true with server.listen = "0.0.0.0:9090", which is not a loopback address: ...` | Set `allow_unsigned = false` and a secret, or listen on `127.0.0.1`; only if unsigned access from the network is really intended, set `allow_unsigned_non_loopback = true` |
| 8 | An enabled tunnel whose `url` is `ws://` to a host other than loopback | `tunnel.url = "ws://..." uses unencrypted ws:// to a host other than loopback, ...` | Use `wss://`, or set `tunnel.allow_insecure_transport = true` if the network path is trusted |
| 9 | A config path that starts with `-` (including exactly `-h`, `--help`, `-V`, `--version`) | On stderr: ``error: unknown option `-x`; see --help (write a config path that starts with `-` as ./-x)``, or help or version output | Pass the path as `./-x` |

#### Behaves differently

These configurations still start, but the service behaves differently:

- **The tunnel client can complete a WebSocket handshake.** It never could before (its handshake
  request was rejected locally). It has been tested only against an in-process test peer: no
  Sylvode server endpoint exists yet, so an enabled tunnel still connects to nothing.
- **`{message}` in a `custom` agent's `args`** is replaced by the message. With 0.3.3 the literal
  `{message}` was passed and the message appended after it.
- **Agents without a template** get the rendered default message or prompt instead of literal
  placeholder text (`[TPL_PROJECT] TPL_EVENT: ...`).
- **Text from Sylvode that would start an argument with `-`** is not passed: a `custom` agent is
  not run (`custom_error: ...`). A leading `-`
  you wrote in `message_template` or `args` is allowed.
- **Replays are refused:** a signed delivery whose `id` was already received is answered `409`, and
  one whose `timestamp` is more than `max_delivery_age_secs` (300) from the receiver's clock `401`.
  Keep the clocks in sync, or set `max_delivery_age_secs = 0`.
- **Signature headers that disagree** are answered `401`, even if one of them is valid, and a
  non-ASCII signature value is refused.
- **The `/webhook` answer's `result`** is a short status; command output and remote error bodies
  are logged instead (`Dispatch result detail=...`).
- **Worker AI tasks:** the callback's issue is the task's `reference_id` for a `work_item`
  reference and `payload.issue_id` for a `comment` reference (a comment mentioning the bot), and
  none otherwise; the agent type is `ai_participant_agent_type`. For a `work_item` task the
  free-form `payload` no longer overrides the issue, and for no task the agent type.
  `route.trigger_kinds` is matched only against `bot_context.trigger_reason` and a worker task's
  `trigger_kind`, not against `task_type` or the payload.
- **Timeouts and shutdown stop the executor's whole process group** (see [Stopping
  commands](#stopping-commands)). On `SIGTERM` or `SIGINT` the service takes at most 40 seconds by
  default, open requests included, and exits 0; 0.3.3 had no signal handling and was killed. Give
  the service manager enough stop time (see [Shutdown](#shutdown)).
- **Tunnel envelopes** for another `agent_id` are refused, and with `hmac_secret` so are envelopes
  more than 300 seconds old and repeated envelope ids.
- **Logs:** the target is `sylvode_runner`, the listening line is written after binding and names
  the bound address, and values from deliveries or commands are logged as escaped fields
  (`event="..."`).
- **Commands** get `/dev/null` as stdin.

Unchanged on purpose: the configuration file format and its default path `config.toml`, the
`/webhook` and `/health` routes, the outbound `X-Webhook-Signature`, the `openprx` agent type
(it names the OpenPRX assistant, not this product), the tunnel `agent_id` (always taken from your
configuration).

## Links

- [Documentation](https://docs.openprx.dev/en/sylvode-webhook/) — Full documentation (10 languages)
- [Community](https://community.openprx.dev) — OpenPRX community forum

## Related

- [Sylvode](https://github.com/openprx/sylvode) — Project management platform
- [PRX](https://github.com/openprx/prx) — AI assistant framework

## License

Licensed under either of [Apache License, Version 2.0](../../LICENSE-APACHE) or [MIT license](../../LICENSE-MIT)
at your option.
