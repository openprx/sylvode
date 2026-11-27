# Sylvode

Sylvode is an open-source, self-hosted platform for project work, governance and structured
business data, built for teams of people and AI agents working side by side. It is written in
Rust (axum, SeaORM) with a SvelteKit web interface and stores everything in PostgreSQL 18.

## Features

- **Projects and work items**: workspaces, projects, work items, boards, cycles, labels,
  workflows, comments, notifications, search and project export/import.
  See [workspaces and projects](docs/guide/workspaces-and-projects.md).
- **Governance**: proposals with weighted voting, decision records, veto and escalation, trust
  scores and appeals, impact reviews and audit logs. See [governance](docs/guide/governance.md).
- **Universal forms**: project-defined business record types with views, record links and child
  tables, formulas, per-role permissions, import/export, attachments and signatures.
  See [forms](docs/guide/forms.md).
- **Template packages**: ready-made, signed project setups for common business scenarios.
  See [template packages](docs/guide/template-packages.md).
- **Flow**: pages, collections and records with live multi-user editing, comments, shares and
  public links, conversion to forms, and export/import packages. Flow is on for every workspace
  unless an administrator turns it off. See [Flow](docs/guide/flow.md).
- **Bots and agent tasks**: bot users and tokens; assigning or mentioning a bot gives it a task,
  and bots are connected to projects to do the work. See [agents](docs/guide/agents.md).
- **Runner**: `sylvode-runner` receives task deliveries and runs agents, either coding-agent CLIs
  in tmux sessions or headless, or command and HTTP agents.
  See [the runner](apps/runner/README.md).
- **MCP server**: tools for AI agents over HTTP, stdio and SSE; every HTTP request acts as the
  caller's own bot token. See [the MCP server](apps/mcp-server/README.md).
- **`sylvode` command-line client**: Flow and workspace operations with JSON output for scripts.
  See [the CLI](docs/integrations/cli.md).
- **REST API**: the HTTP API the web interface itself uses, with bot-token access for scripts and
  integrations. See [the API reference](docs/integrations/api.md).
- **Webhooks**: HMAC-signed outbound deliveries of project events.
  See [webhooks](docs/integrations/webhooks.md).
- **Alert intake**: security and monitoring products post events to an inbound endpoint, and rules
  turn them into alert work items and bot tasks. Off unless enabled.
  See [alerts](docs/integrations/alerts.md).
- **OAuth authorization server**: external applications get user-approved, scoped access; users,
  workspace administrators and instance administrators manage connected apps. Off unless
  configured. See [OAuth](docs/integrations/oauth.md).
- **WASM plugins**: sandboxed per-project plugins for field validation, formulas and event
  handlers, and form plugin packages. See [plugins](docs/plugins/README.md).

<!-- BEGIN GENERATED: tool-count (sylvode-skillgen; do not edit) -->
The MCP server registers 159 tools in 20 categories. By default it lists nine of them (search mode) and reaches the rest through `sylvode_search`, `sylvode_read` and `sylvode_write`. Every tool, with its category and access, is in [reference/tools.md](skills/sylvode-mcp/reference/tools.md).
<!-- END GENERATED: tool-count -->

## Quick start

On a Linux host with Docker (with the Compose plugin) and the Rust toolchain named in
`rust-toolchain.toml`:

```bash
git clone https://github.com/openprx/sylvode.git
cd sylvode
bash scripts/start.sh
```

On first run `scripts/start.sh` writes `.env`, `config/sylvode.compose.toml` and
`config/sylvode.compose.mcp.toml` with random bootstrap secrets (it never rewrites a file that
already exists), builds the release binaries and starts the compose stack. By default the services
listen on `127.0.0.1`: the web interface on port 3000, the API on 8081 and the MCP server on 8090.
The generated files hold real secrets: keep them out of version control and replace them before
production use.

The web interface has a sign-in page but no sign-up page. Create the first account through the
API; the first account registered on an empty instance becomes the instance administrator, and
every later registration needs an administrator:

```bash
curl -X POST http://localhost:8081/api/v1/auth/register \
  -H 'Content-Type: application/json' \
  -d '{"email": "admin@example.com", "password": "choose-a-long-password", "name": "Admin"}'
```

Then sign in at `http://localhost:3000`.

## Install options

Sylvode runs from the compose stack above, from release archives (Linux, macOS and Windows server
binaries, a static web interface archive and runner archives), or from a source build. Flow
collaborative editing needs the API on Linux. See the [installation guide](docs/install.md) for
every option, the first administrator account, demo data and upgrades.

## Documentation

| Topic | Document |
| --- | --- |
| Documentation index | [docs/README.md](docs/README.md) |
| Install and upgrade | [docs/install.md](docs/install.md) |
| Configuration reference | [docs/configuration.md](docs/configuration.md) |
| Operations (processes, proxies, storage, backups) | [docs/operations.md](docs/operations.md) |
| User guides | [docs/guide/](docs/README.md#users) |
| Integrations (API, webhooks, alerts, OAuth, CLI) | [docs/README.md](docs/README.md#integrators) |
| MCP server | [apps/mcp-server/README.md](apps/mcp-server/README.md) |
| Runner | [apps/runner/README.md](apps/runner/README.md) |
| Plugins | [docs/plugins/README.md](docs/plugins/README.md) |
| Security policy | [SECURITY.md](SECURITY.md) |
| Release notes | [CHANGELOG.md](CHANGELOG.md) |

## Components

| Component | Path | Role |
| --- | --- | --- |
| `api` | `apps/api` | HTTP API, form engine and plugin runtime; applies pending database migrations at startup |
| `collab-isolated-apply-worker` | `crates/collab-core` | Process the API starts for every Flow collaborative write; must sit in the same directory as `api` |
| `worker` | `apps/worker` | Background jobs: agent task delivery, form jobs, proposal settlement, Flow jobs, alert follow-up, retention |
| `mcp-server` | `apps/mcp-server` | MCP server over the API (HTTP, stdio, SSE) |
| `sylvode` | `apps/mcp-server` | Command-line client over the API |
| `sylvode-runner` | `apps/runner` | Receives task deliveries and runs agents (Linux and macOS) |
| `frontend` | `frontend` | Web interface, a static single-page app served by nginx |

Shared code lives in `crates/platform` (configuration, database, authentication, logging),
`crates/collab-core` (the Flow collaboration engine, on Loro) and `crates/sylvode-prereq`
(prerequisite checks used by the runner and the MCP server). `migrations/` holds the SQL schema.

## Licence

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
