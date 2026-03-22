# openpr-webhook

Webhook receiver for [OpenPR](https://github.com/openprx/openpr). Receives webhook events from OpenPR and dispatches notifications to AI agents, chat platforms, or external services.

Built with **Rust** (Axum).

## How It Works

```
OpenPR ──webhook POST──▶ openpr-webhook ──dispatch──▶ OpenClaw (Signal/Telegram)
                                         ──dispatch──▶ HTTP endpoint
                                         ──dispatch──▶ Custom command
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
- **CLI callback loop** — Send issue execution result back via MCP/API (comment write-back ready)
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

# Agent: CLI executor (Phase A)
[[agents]]
id = "vano-cli"
name = "Vano CLI"
agent_type = "cli"
message_template = "[{project}] {event}: {key} {title}"

[agents.cli]
executor = "codex" # codex | claude-code | opencode
workdir = "/opt/worker/code/openpr"
timeout_secs = 900
max_output_chars = 12000
prompt_template = "Fix issue {issue_id}: {title}\nContext: {reason}"
callback = "mcp" # mcp | api
callback_url = "http://127.0.0.1:8090/mcp/rpc"
callback_token = "opr_xxx"
update_state_on_start = "in_progress"
update_state_on_success = "done"
update_state_on_fail = "todo"
```

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
FROM rust:1.75 AS builder
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
