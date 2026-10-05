# Sylvode Webhook

Webhook receiver for [Sylvode](https://github.com/openprx/sylvode). It receives webhook deliveries
from a Sylvode deployment and dispatches them to AI agents, chat platforms or external services.

> **Formerly OpenPR Webhook.** The project was renamed together with the main product. Every old
> name (executable, container image, signature header, environment variable, service unit) keeps
> working for the whole 1.x line and is not removed before v2.0; see [Migrating from OpenPR
> Webhook](#migrating-from-openpr-webhook).

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

1. Sylvode fires a webhook on events (issue created, proposal submitted, comment added, etc.)
2. sylvode-webhook verifies the HMAC-SHA256 signature
3. Processes bot tasks where `bot_context.is_bot_task=true` and worker AI task envelopes
4. Dispatches formatted notifications to configured agents

## Features

- **HMAC-SHA256 signature verification** — Validates webhook authenticity
- **Multi-agent dispatch** — Route events to multiple agents simultaneously
- **Agent types**:
  - `openclaw` — Send via OpenClaw CLI (`openclaw message send`)
  - `openprx` — Send via OpenPRX Signal API or CLI
  - `webhook` — Forward to HTTP endpoints
  - `custom` — Execute arbitrary commands
  - `cli` — Execute codex/claude-code/opencode via strict whitelist templates
- **MCP closed-loop automation** — AI agents read full issue context (description, comments, labels) via Sylvode MCP tools and write results back directly
- **CLI callback loop** — Send issue execution result back via MCP/API (comment write-back ready)
- **Per-agent environment variables** — Inject extra environment variables into each CLI executor
- **WSS tunnel client (Phase B MVP)** — Active ws/wss connection with Bearer auth, heartbeat, auto-reconnect
- **Tunnel envelope + HMAC** — Minimal envelope (`id/type/ts/agent_id/payload/sig`) with optional HMAC-SHA256
- **Task bridge** — Handles `task.dispatch` -> immediate `task.ack` -> async `task.result`
- **Message templates** — Customizable notification format with placeholders
- **Configurable** — TOML-based configuration

## Quick Start

```bash
# Build
cargo build --release

# Configure
cp config.example.toml config.toml
# Edit config.toml with your settings

# Run
./target/release/sylvode-webhook config.toml
# sylvode-webhook listening on 0.0.0.0:9090
```

`sylvode-webhook [CONFIG]` reads `config.toml` in the working directory when no path is given.
`--help` and `--version` print usage and the version.

### Configure the Sylvode webhook

In Sylvode, create a webhook pointing to this receiver:

- **URL**: `http://your-server:9090/webhook`
- **Secret**: Must match `webhook_secrets` in `config.toml`
- **Events**: Select which events to receive

## Configuration

```toml
[server]
listen = "0.0.0.0:9090"

[security]
webhook_secrets = ["your-secret-here"]
allow_unsigned = false  # Set true only for development

# Feature gates (safe defaults)
# Keep new paths OFF unless you are explicitly enabling them.
[features]
tunnel_enabled = false
cli_enabled = false
callback_enabled = false

# Runtime guardrails
[runtime]
cli_max_concurrency = 1
http_timeout_secs = 15
tunnel_reconnect_backoff_max_secs = 60

# Agent: OpenClaw (AI assistant via Signal/Telegram)
[[agents]]
id = "david"
name = "David"
agent_type = "openclaw"
message_template = "🔔 [{project}] {event}: {key} {title}\n👤 {actor} | Trigger: {reason}"

[agents.openclaw]
command = "openclaw message send"
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
# Or use CLI instead:
# command = "openprx message send"
# channel = "signal"

# Agent: Forward to HTTP endpoint
[[agents]]
id = "slack-bot"
name = "Slack"
agent_type = "webhook"
message_template = "{event}: {title}"

[agents.webhook]
url = "https://hooks.slack.com/services/xxx"
secret = "optional-shared-secret" # if set, outbound header x-webhook-signature is added

# Optional: route this agent only for a scenario-aware Sylvode bot task.
# Empty lists mean "no constraint".
[agents.route]
bot_names = ["Document review connection"]
bot_agent_types = ["webhook"]
project_types = ["contract_review"]
trigger_kinds = ["mention"]
events = ["issue.assigned", "comment.created"]
form_keys = ["order", "print_job"]

# Agent: Custom command
[[agents]]
id = "logger"
name = "Logger"
agent_type = "custom"
message_template = "{event} {key}"

[agents.custom]
command = "echo"
args = ["{message}"]

# Agent: CLI executor with MCP closed-loop
[[agents]]
id = "ai-fixer"
name = "AI Issue Fixer"
agent_type = "cli"
message_template = "[{project}] {event}: {key} {title}"

[agents.cli]
executor = "claude-code" # codex | claude-code | opencode
workdir = "/path/to/your/repository"
timeout_secs = 900
max_output_chars = 12000
prompt_template = "Fix issue {issue_id}: {title}\nContext: {reason}"
callback = "mcp" # mcp | api
callback_url = "http://127.0.0.1:8090/mcp/rpc"
callback_token = "opr_xxx"

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

Default MCP instructions are injected automatically when the agent has MCP-related config (`mcp_instructions`, `mcp_config_path`, or `env_vars`). You can customize them via `mcp_instructions` in the agent config.

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

### Callback attribution

A plain API callback (`callback = "api"`) reports its surface as `cli` in both
`X-Sylvode-MCP-Surface` and the legacy `X-OpenPR-MCP-Surface`, with the same value, so the API
records it correctly whether it predates the rename or not. Both headers are sent for the whole
1.x line.

---

When forwarding via `agent_type = "webhook"` and `agents.webhook.secret` is configured, sylvode-webhook signs the outbound JSON body and sends:

- Header: `X-Webhook-Signature`
- Value format: `sha256=<hex_hmac>`

## Phase B: Tunnel (WSS) MVP

Enable both `[features].tunnel_enabled = true` and `[tunnel].enabled = true` in `config.toml` to let `sylvode-webhook` actively connect to a control plane.

```toml
[tunnel]
enabled = true
url = "wss://sylvode.example.com/api/v1/agent-tunnel" # ws:// also supported for LAN/dev
agent_id = "vano-qa"                                  # required; there is no default
auth_token = "opr_xxx"                                # Authorization: Bearer <token>
reconnect_secs = 3
heartbeat_secs = 20
hmac_secret = "shared-hmac-secret"                    # optional, signs envelope body
```

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

1. Receive `task.dispatch`
2. Send `task.ack` immediately (`run_id`, `issue_id`, `status=accepted`)
3. Reuse existing `cli` executor
4. Send `task.result` when done (`run_id`, `issue_id`, `status`, `summary`)

Signature behavior (MVP):

- If `tunnel.hmac_secret` is set: outbound envelopes include `sig` (HMAC-SHA256 over unsigned envelope body).
- Inbound verification is optional framework: when `sig` exists it is verified, missing `sig` is currently accepted.

Safety toggles:

- `SYLVODE_WEBHOOK_SAFE_MODE=1` forces `tunnel/cli/callback` off at runtime.
- This provides one-command rollback to legacy webhook-only behavior.

### Template Placeholders

| Placeholder | Description |
|-------------|-------------|
| `{project}` | Project name |
| `{event}` | Event type (e.g. `issue.created`) |
| `{key}` | Item identifier |
| `{title}` | Item title |
| `{actor}` | User who triggered the event |
| `{reason}` | Trigger reason |
| `{issue_id}` | Issue ID (from webhook payload) |

## API

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/webhook` | POST | Receive webhook events |
| `/health` | GET | Health check |

### Signature headers

The request signature is `sha256=<hex>` (the prefix is optional), an HMAC-SHA256 of the raw body
keyed with one of `webhook_secrets`. It is accepted in any of these headers:

| Header | Status |
|--------|--------|
| `X-Sylvode-Signature` | Canonical |
| `X-Webhook-Signature` | Canonical; the header the Sylvode API sends today |
| `X-OpenPR-Signature` | Legacy; accepted with a deprecation notice, not removed before v2.0 |

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

Nothing has to change on upgrade: every legacy name below keeps working for the whole 1.x line and
is not removed before v2.0. Using one writes a single deprecation line to stderr per process,
naming the replacement. stdout and exit codes are the same for the legacy and the canonical name.

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

- [Documentation](https://docs.openprx.dev/en/openpr-webhook/) — Full documentation (10 languages)
- [Community](https://community.openprx.dev) — OpenPRX community forum

## Related

- [Sylvode](https://github.com/openprx/sylvode) — Project management platform
- [PRX](https://github.com/openprx/prx) — AI assistant framework

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option.
