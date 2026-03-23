# openpr-webhook

Webhook receiver for [OpenPR](https://github.com/openprx/openpr). Receives webhook events from OpenPR and dispatches notifications to AI agents, chat platforms, or external services.

Built with **Rust** (Axum).

## How It Works

```
OpenPR ──webhook POST──▶ openpr-webhook ──dispatch──▶ OpenClaw (Signal/Telegram)
                                         ──dispatch──▶ HTTP endpoint
                                         ──dispatch──▶ Custom command
                                         ──dispatch──▶ CLI agent (codex/claude-code)
                                                           │
                                                           ▼
                                                      OpenPR MCP Server
                                                      (read issue → fix → write back)
```

1. OpenPR fires a webhook on events (issue created, proposal submitted, comment added, etc.)
2. openpr-webhook verifies the HMAC-SHA256 signature
3. Only processes bot tasks where `bot_context.is_bot_task=true` (non-bot events are ignored)
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
- **MCP closed-loop automation** — AI agents read full issue context (description, comments, labels) via OpenPR MCP tools and write results back directly
- **CLI callback loop** — Send issue execution result back via MCP/API (comment write-back ready)
- **Per-agent environment variables** — Inject `OPENPR_BOT_TOKEN`, `OPENPR_API_URL`, etc. per agent
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
./target/release/openpr-webhook
# Listening on 0.0.0.0:9090
```

### Configure OpenPR Webhook

In OpenPR, create a webhook pointing to this receiver:

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
method = "POST"

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
workdir = "/opt/worker/code/openpr"
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

# Optional: path to MCP config for claude-code (--mcp-config flag).
# mcp_config_path = "/path/to/mcp-config.json"

# Extra environment variables injected into the executor subprocess.
[agents.cli.env_vars]
OPENPR_API_URL = "http://localhost:3000"
OPENPR_BOT_TOKEN = "opr_xxx"
OPENPR_WORKSPACE_ID = "e5166fd1-..."
```

## MCP Closed-Loop Automation

When a CLI agent has OpenPR MCP tools available (via global config or `mcp_config_path`), it can autonomously:

1. **Read full issue context** — title, description, comments, labels, state, priority via `work_items.get` / `comments.list`
2. **Fix the problem** — analyze context, write code, run tests
3. **Write results back** — post a summary comment via `comments.create`, update state via `work_items.update`

This eliminates the need for the webhook callback to update issue state (use `skip_callback_state = true`).

Default MCP instructions are injected automatically when the agent has MCP-related config (`mcp_instructions`, `mcp_config_path`, or `env_vars`). You can customize them via `mcp_instructions` in the agent config.

### MCP Setup

For **Codex**, add to `~/.codex/config.toml`:

```toml
[mcp_servers.openpr]
type = "stdio"
command = "/path/to/mcp-server"
args = ["--transport", "stdio"]
env = { OPENPR_API_URL = "http://localhost:3000", OPENPR_BOT_TOKEN = "opr_xxx" }
```

For **Claude Code**, add to `~/.claude.json`:

```json
"openpr": {
  "type": "stdio",
  "command": "/path/to/mcp-server",
  "args": ["--transport", "stdio"],
  "env": {
    "OPENPR_API_URL": "http://localhost:3000",
    "OPENPR_BOT_TOKEN": "opr_xxx"
  }
}
```

---

When forwarding via `agent_type = "webhook"` and `agents.webhook.secret` is configured, openpr-webhook signs the outbound JSON body and sends:

- Header: `X-Webhook-Signature`
- Value format: `sha256=<hex_hmac>`

## Phase B: Tunnel (WSS) MVP

Enable both `[features].tunnel_enabled = true` and `[tunnel].enabled = true` in `config.toml` to let `openpr-webhook` actively connect to a control plane.

```toml
[tunnel]
enabled = true
url = "wss://openpr.example.com/api/v1/agent-tunnel"   # ws:// also supported for LAN/dev
agent_id = "vano-qa"
auth_token = "opr_xxx"                                 # Authorization: Bearer <token>
reconnect_secs = 3
heartbeat_secs = 20
hmac_secret = "shared-hmac-secret"                     # optional, signs envelope body
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

- `OPENPR_WEBHOOK_SAFE_MODE=1` forces `tunnel/cli/callback` off at runtime.
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

### Webhook Headers

| Header | Description |
|--------|-------------|
| `X-Webhook-Signature` | HMAC-SHA256 signature (`sha256=...`) |
| `X-OpenPR-Signature` | Also accepted for backward compatibility |
| `X-OpenPR-Event` | Event type |

## Deployment

### Systemd

```ini
[Unit]
Description=OpenPR Webhook Receiver
After=network.target

[Service]
ExecStart=/usr/local/bin/openpr-webhook
WorkingDirectory=/etc/openpr-webhook
Restart=always

[Install]
WantedBy=multi-user.target
```

### Docker

```dockerfile
FROM rust:1.86 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/openpr-webhook /usr/local/bin/
COPY config.toml /etc/openpr-webhook/
CMD ["openpr-webhook"]
```

## Links

- [Documentation](https://docs.openprx.dev/en/openpr-webhook/) — Full documentation (10 languages)
- [Community](https://community.openprx.dev) — OpenPRX community forum

## Related

- [OpenPR](https://github.com/openprx/openpr) — Project management platform
- [PRX](https://github.com/openprx/prx) — AI assistant framework

## License

Apache-2.0
