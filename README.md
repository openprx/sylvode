# Sylvode

Open-source project management platform with built-in governance, a universal
business-form engine, collaborative Flow pages, WASM plugins, and a first-class
MCP server for AI agents. Built with **Rust** (Axum + SeaORM), **SvelteKit**, and
**PostgreSQL 16**.

> **Formerly OpenPR.** Releases up to 0.2.21 were published as OpenPR. Existing
> OpenPR names (the `mcp-server` CLI commands, `config/openpr.toml`, `OPENPR_*`
> compose variables, `openpr://` resources, `openpr-*` release archives) keep
> working with a deprecation warning and are not removed before Sylvode v2.0;
> stable identifiers such as the REST API, MCP tool names, database names and the
> plugin ABI are not renamed at all. See the
> [v1.0 compatibility matrix](docs/sylvode-v1.0-compatibility.md) and
> [CHANGELOG.md](CHANGELOG.md).

## What It Provides

- **Project management** — workspaces, projects, issues, kanban board, sprints, labels, comments, activity feed, notifications, attachments.
- **Governance** — proposals, weighted voting, decision records, veto and escalation, trust scores, appeals, impact reviews, audit logs.
- **Universal forms** — project-defined business data types with grid/detail views, decimal-safe amounts, record links and child tables, formulas, per-role permissions, import/export, electronic signatures.
- **Sylvode Flow** — collaborative pages and objects with live multi-user editing (Loro CRDT over WebSocket), Collections and records, a workspace navigator, search, Forms conversion, and export/import packages. Off by default; enabled per workspace.
- **WASM plugins** — per-project sandboxed plugins for field validation, formulas, and event handlers.
- **Events** — transactional business-event ledger and HMAC-signed webhooks.
- **MCP server** — 140 tools, 4 static resources, 22 resource templates, 3 transports.
- **`sylvode` CLI** — fifteen command groups for Flow and workspace operations, with a stable JSON output contract.
- **Scenario templates** — 6 ready-to-start setups: `code_delivery_default`, `contract_review_default`, `equipment_maintenance_default`, `quality_corrective_action_default`, `customer_delivery_default`, `restaurant_ordering_default`.

## Architecture

| Component    | Path              | Role                                                                 |
| ------------ | ----------------- | -------------------------------------------------------------------- |
| `api`        | `apps/api`        | HTTP API, form engine, plugin runtime, event emission                 |
| `worker`     | `apps/worker`     | Background pipelines: AI tasks, form jobs, Flow dispatch, retention  |
| `mcp-server` | `apps/mcp-server` | MCP server (HTTP/stdio/SSE) over the API                             |
| `sylvode`    | `apps/mcp-server` | Command-line client over the API (second binary of the same package) |
| `collab-isolated-apply-worker` | `crates/collab-core` | Process the API spawns for every Flow collaborative write (isolated decode/apply); **must sit in the same directory as `api`** |
| `frontend`   | `frontend`        | SvelteKit 2 SPA (adapter-static)                                     |

`crates/platform` holds shared config, DB connection, auth, error, logging and
the deprecation texts. `crates/collab-core` is the Flow collaboration engine on
Loro. `migrations/` holds the ordered SQL schema history from `0000` through
`0070`; the API applies pending migrations at startup.

## Quick Start

```bash
git clone https://github.com/openprx/sylvode.git
cd sylvode
bash scripts/start.sh
```

`scripts/start.sh` generates the deployment's configuration on first run —
`config/sylvode.compose.toml` for the API and the worker,
`config/sylvode.compose.mcp.toml` for the MCP server, plus a compose-only `.env`
— filling in random non-production bootstrap secrets without echoing any of them
to the terminal. It then builds release binaries for `Dockerfile.prebuilt` and
runs `docker compose up -d --build`. The generated files hold real secrets:
replace them before production use, and never commit them. `.env` is written
mode `600`. The two compose configuration files are written mode `644` on
purpose: they are bind-mounted into containers that run as uid 1000, and under
a rootless runtime an owner-only file is unreadable inside the container. Any
local user who can traverse `config/` can therefore read them, so protect the
directory instead, for example `chmod 750 config` (or the deployment directory
above it) with the deploying user as owner.
Services publish on `${SYLVODE_BIND_HOST:-127.0.0.1}`: frontend `:3000`, API
`:8081`, MCP `:8090`. `scripts/start.sh` maps the documented legacy environment
aliases to these canonical compose inputs. `bash scripts/start.sh --check-config`
only generates and validates the configuration, without building or starting
anything.
The `api` image ships `collab-isolated-apply-worker` next to `/app/api`: the API spawns it for
every Flow collaborative write and looks for it in its own directory, so without it every edit
fails. `--no-build` therefore expects `api`, `collab-isolated-apply-worker`, `worker` and
`mcp-server` in `target/release`. Release archives carry it beside `api`; keep the two in the
same directory when installing them (or point `COLLAB_ISOLATED_APPLY_WORKER_PATH` at it).
The generated `[flow] collab_allowed_origins` lists the addresses the frontend
is published at (for the default bind, `http://localhost:3000` and
`http://127.0.0.1:3000`), so live editing works on first start; replace it with
the origin users actually open once the frontend is behind a domain or proxy.
An existing file is never rewritten, and `--check-config` warns when its list is
empty, which refuses every live-editing session.
For demo data once healthy, `scripts/bootstrap-restaurant-demo.sh` creates a
demo account, workspace, `restaurant_ordering_default` project with sample
records, and a workspace-scoped bot token; it refuses non-local API URLs unless
`SYLVODE_DEMO_ALLOW_REMOTE=1` (legacy `OPENPR_DEMO_ALLOW_REMOTE` still accepted).

### Local development

```bash
# Prerequisites: the Rust toolchain pinned in rust-toolchain.toml, Bun, PostgreSQL 16
scripts/dev-up.sh                                  # start only PostgreSQL from compose
cp config/sylvode.example.toml config/sylvode.toml
$EDITOR config/sylvode.toml                        # database.url, auth.jwt_secret, [mcp]

cargo build -p collab-core --bin collab-isolated-apply-worker   # Flow edits need it next to the api binary
cargo run --bin api -- --config config/sylvode.toml     # listens on server.bind_addr, default 0.0.0.0:8081
cargo run --bin worker -- --config config/sylvode.toml

cd frontend && bun install && bun run dev

cargo run --bin mcp-server -- serve --config config/sylvode.toml --transport http
```

> A host-side run reaches PostgreSQL through the published port, so
> `database.url` must name `localhost`, not the compose hostname `postgres`.
> `mcp.api_url` follows the same rule: `http://localhost:8081` from the host,
> the `api` service address from inside the compose network.
>
> `--config` is optional; every binary first looks for `config/sylvode.toml`.
> A legacy `config/openpr.toml` is discovered when the new path is absent. If both
> exist, startup fails until `--config` explicitly selects one.

## Configuration

**One TOML file, no environment variables.** `api`, `worker` and `mcp-server`
read every setting from a single configuration file and **no environment
variable at all**. The path comes from `--config <PATH>`, defaulting to
`config/sylvode.toml` relative to the process working directory. A missing file
is a startup error, never a silent fallback: the binaries never invent a
database URL or a signing key. `config/sylvode.example.toml` is the annotated
reference. The old `config/openpr.toml` default remains a compatibility fallback,
but never silently wins over the new path.

Unknown keys are rejected, so a misspelled setting fails startup instead of
being silently ignored.

| Section | Read by | Keys (defaults in parentheses) |
| --- | --- | --- |
| `[server]` | api, worker | `app_name`, `bind_addr`. Both optional; each binary keeps its own default when they are omitted (api listens on `0.0.0.0:8081`). |
| `[database]` | api, worker | `url` (**required**; full URL, password included, never logged), `max_connections` (`20`), `min_connections` (`2`), `connect_timeout_seconds` (`5`), `idle_timeout_seconds` (`30`), `acquire_timeout_seconds` (`5`) |
| `[auth]` | api, worker | `jwt_secret` (**required**; minimum 16 characters, 64 hex recommended — `openssl rand -hex 32`), `access_ttl_seconds` (`1296000`), `refresh_ttl_seconds` (`1728000`), `default_author_id` (optional, must be a real non-nil UUID), `allow_insecure_cookies` (`false`; drops `Secure` from auth cookies for local plain-HTTP development and is refused unless `[server] bind_addr` is loopback) |
| `[logging]` | all | `filter` — `tracing` directives, validated at startup (`<service>=info,tower_http=info`), `format` — `json` \| `text` (`json`), `output` — `stderr` \| `stdout` (`stderr`) |
| `[storage]` | api | `backend` — `local` \| `s3` (`local`), `dir` (`./uploads`); `[storage.s3]` with `endpoint`, `bucket`, `region` (`us-east-1`), `access_key_id`, `secret_access_key`, `session_token`, required only when `backend = "s3"` and left unread otherwise |
| `[audit]` | worker | `operation_log_retention_days` (`30`, range `1..=3650`) |
| `[migrations]` | api | `replay` (`false`), `continue_on_error` (`false`) — both are escape hatches; turn one on deliberately, then turn it back off |
| `[outbound]` | api, worker | `allowed_hosts` — a TOML **array of strings** (`[]`), `allow_private` (`false`) |
| `[flow]` | api, worker | `dispatch_max_attempts` (`10`), `collab_allowed_origins` — a TOML array of `scheme://host[:port]` origins allowed to open live collaboration sessions (`[]`; empty refuses every session, so list the web UI's origin to enable live editing) |
| `[mcp]` | mcp-server, sylvode | `api_url` (`http://localhost:8081`), `bot_token` (`opr_` prefix; **required** for `stdio` and the CLI subcommands, unused by `http`/`sse`), `workspace_id` (**required**, real non-nil UUID), `transport` — `stdio` \| `http` \| `sse` (`stdio`), `bind_addr` (`127.0.0.1:8090`) |

> **Eager shape, lazy presence.** Every value the file *does* contain is
> shape-checked at startup by whichever binary reads it, but whether a mandatory
> value is *present* is decided by the binary that needs it. A deployment that
> runs only the MCP server therefore needs no `[database]` and no `[auth]`
> section at all, and is never asked to invent two credentials it never uses.
> When api or worker is missing them, validation reports every missing or
> unusable value in **one** error instead of one restart per mistake.

A complete MCP-only configuration is three lines:

```toml
[mcp]
bot_token = "opr_..."
workspace_id = "..."
```

**Logging** replaces `RUST_LOG`: the level is part of the deployment's
configuration, not of whatever the surrounding shell happened to export. The MCP
server's stdio transport frames JSON-RPC on stdout, where one log line ends the
session, so it always logs to stderr and reports `output = "stdout"` as
overridden rather than honouring it.

**Outbound deliveries (api + worker).** Webhook endpoints are
validated when they are configured and again before every delivery: an endpoint
whose host resolves to a loopback, private, link-local, NAT64/6to4 or otherwise
internal address is refused, and redirects are not followed.
`outbound.allowed_hosts` lists the exemptions as `"host"` or `"host:port"`,
matched literally and case-insensitively — no wildcards, no URLs, no paths, all
three are rejected at startup because they would silently never match. Internal
receivers (compose services, in-cluster bots) must be listed there or their
deliveries are refused. `outbound.allow_private = true` disables the checks
entirely and is only for a closed network you control end to end.

```toml
[outbound]
allowed_hosts = ["webhook:9090", "api:8080", "mcp-server:8090", "frontend:80"]
allow_private = false
```

## WASM Plugins

Per-project WebAssembly modules executed by `wasmtime` 49. Each invocation runs under a
**fuel budget** (per manifest, at most 1e9), a **memory ceiling** (per manifest, at most
128 MiB), a **wall-clock deadline** (per manifest, at most 30000 ms) that covers compiling the
module as well as running it, a **module size limit** of 4 MiB, and store limits of one
instance, one memory and one table of at most 65,536 elements. **Zero host functions**: modules
are instantiated with an empty import list, so there is no WASI, no filesystem, no network and
no clock; communication happens only through linear memory. Every run is recorded in
`plugin_invocations` (see `docs/universal-forms-and-plugins.md`).

ABI (`docs/plugins/openpr-plugin-v1.wit`): export `memory`,
`openpr_alloc(len: i32) -> i32`, and `openpr_invoke(ptr: i32, len: i32) -> i64`
(packed pointer/length return); optionally `openpr_plugin_abi_version() -> i32`
returning `1`. Input and output are UTF-8 JSON, decimals stay strings.

| Hook              | Effect on the write path                                  |
| ----------------- | --------------------------------------------------------- |
| `field_validator` | Can reject the record write                                |
| `formula`         | Returns a value patch applied to the record               |
| `event_handler`   | Fire-and-forget; failure is recorded but does not block   |

> Plugins do **not** add entries to MCP `tools/list`. A manifest may declare
> `capabilities.tools`, but that list is only stored and used to authorize
> `plugins.invoke`; plugin logic is reached indirectly through the `hook_kind`
> argument of `plugins.invoke`.

## Events and Webhooks

**Business event ledger.** Business writes append to `business_events`. Consumers read the resulting stream with `events.tail`; there is no push-based delivery of these events.


**Legacy webhooks** are a separate path from the business event ledger, signed with HMAC-SHA256
in `X-Webhook-Signature: sha256=<hex>`. 31 event types can be emitted (issue,
comment, label, sprint, proposal, project, member, veto, escalation, appeal,
governance config, AI task), while webhook subscriptions are validated against a
narrower 14-entry allow-list in `apps/api/src/entities/webhook.rs`. **Legacy
webhook delivery has no retry** — `retry_count` is always written as `0`.

## Worker

`apps/worker` is a standalone process with a 5-second poll loop and a
`--concurrency` flag (default `4`). Concurrency is a **batch-size multiplier**,
not a parallelism level: each pipeline fetches `concurrency * N` rows per tick
and awaits them sequentially.

| Pipeline             | Source                                                                 | Behavior                                                      |
| -------------------- | ---------------------------------------------------------------------- | ------------------------------------------------------------- |
| AI task dispatch     | `ai_tasks`                                                             | POSTs to bot webhooks, HMAC-signed; retry delay `max(attempts, 1) * 30` s |
| Operation-log cleanup | `bot_operation_logs`                                                  | Deletes metadata records older than `[audit]` retention       |
| Form jobs            | `form_import_jobs`, `form_export_jobs`, `form_attachment_package_jobs` | Plus expiry cleanup of package artifacts and signature values |
| Proposal settlement  | `proposals`                                                            | Settles expired proposals (moved off the API read path)       |
| Flow                 | `event_dispatch`, Flow projections and documents                       | Event dispatch and delivery, search indexing, compaction, projection rebuilds, integrity scans |

Queue-backed pipelines pick rows with `SELECT ... FOR UPDATE SKIP LOCKED`, so
multiple worker instances can share one database safely.

## MCP Server

Only `--transport http` serves all three surfaces on one port.

| Transport | Command                   | Endpoints                                                |
| --------- | ------------------------- | -------------------------------------------------------- |
| **HTTP**  | `serve --transport http`  | `POST /mcp/rpc`, `GET /sse`, `POST /messages`, `/health` |
| **stdio** | `serve --transport stdio` | stdin/stdout JSON-RPC                                    |
| **SSE**   | `serve --transport sse`   | `GET /sse`, `POST /messages`, `/health` — **no** `/mcp/rpc` |

> **Security — an MCP HTTP/SSE request is made as its own caller.**
>
> `/mcp/rpc`, `/sse` and `/messages` require `Authorization: Bearer <opr_ bot
> token>` and reject everything else with 401; `/health` is exempt so
> healthchecks keep working. There is no shared inbound secret and no
> configuration that relaxes this: the server holds no identity of its own on
> these transports, forwards the token the request presented to the API
> unchanged, and lets the API authenticate it. Every call is therefore made by a
> named bot, and the audit trail records that bot rather than the server.
>
> Because an unauthenticated caller can reach nothing but `/health`, binding a
> reachable address publishes no anonymous surface — which is what lets the
> compose container bind `0.0.0.0:8090` with no secret in its configuration file
> at all. `mcp.auth_token` was the old shared secret and **has been removed**: a
> configuration file that still carries the key is refused at startup, so delete
> the line rather than leaving it for later.
>
> `mcp.bot_token` still names the identity for `stdio` and for the CLI
> subcommands, which have no per-request header to read one from. It is unused
> by `http` and `sse`.

### Bot tokens

MCP authenticates to the API with **bot tokens** (prefix `opr_`), managed under
**Workspace → Members → API Tokens**. A token has a display name shown in
activity feeds, is scoped to one workspace, creates a `bot_mcp` user entity for
audit-trail integrity, and carries `read`, `write` or `admin` permissions that the
API enforces: safe methods need `read`, every other method needs `write`, and
`admin` implies both.

Every token is also bound to **one transport surface** when it is created, and
the API refuses it with 401 on any other surface. Create one token per place you
connect from:

| Surface | Where the token works |
|---|---|
| `rest` | Direct REST calls with `Authorization: Bearer opr_...` |
| `mcp_http` | The MCP server's HTTP endpoint (`/mcp/rpc`), sent by the agent in its `Authorization` header |
| `mcp_sse` | The MCP server's SSE endpoint, sent the same way |
| `mcp_stdio` | `mcp-server serve --transport stdio`, as `mcp.bot_token` |
| `cli` | The `sylvode` and `mcp-server` commands other than `tools call`, as `mcp.bot_token` or `--bot-token` |
| `cli_tools_call` | `sylvode tools call` / `mcp-server tools call`, as `mcp.bot_token` or `--bot-token` |

The web form requires a choice. `POST /api/v1/workspaces/{id}/bots` takes it as
`transport_surface` and defaults to `rest` when the field is omitted; the
surface cannot be changed afterwards, so issue a new token instead.

### Client configuration — stdio (Claude Desktop / Cursor / Codex)

```json
{
  "mcpServers": {
    "sylvode": {
      "command": "/path/to/mcp-server",
      "args": ["serve", "--config", "/absolute/path/to/config/sylvode.toml"]
    }
  }
}
```

No `env` block: the binary reads no environment variables, so `mcp.api_url`,
`mcp.bot_token` and `mcp.workspace_id` come from the file the `--config` path
names. An absolute path is what makes this work — the default
`config/sylvode.toml` is relative to whatever working directory the MCP client
happens to launch the process in.

The legacy `mcp-server` CLI commands and `config/openpr.toml` remain supported
and print a deprecation warning; they are not removed before Sylvode v2.0. See
[the compatibility matrix](docs/sylvode-v1.0-compatibility.md) for conflict
handling, the warnings and the earliest possible removal versions.

> `--api-url`, `--bot-token`, `--workspace-id`, `--transport` and `--bind-addr`
> exist as command-line overrides and win over the file. Prefer the file for
> anything secret: a token in `argv` is readable by any local process through
> `/proc`.

### Client configuration — HTTP/SSE

One shared server, many callers: the token belongs to the client, not to the
server, so each client puts **its own** bot token in the header.

```json
{
  "mcpServers": {
    "sylvode": {
      "type": "http",
      "url": "http://localhost:8090/mcp/rpc",
      "headers": { "Authorization": "Bearer opr_your_own_workspace_bot_token" }
    }
  }
}
```

HTTP — plain JSON-RPC; passing `params.project_id` returns the
project-capability-filtered tool set. SSE — open the stream, POST to the session
endpoint it returns, and the response arrives back on the stream as
`event: message`.

```bash
# The Authorization header is mandatory on every one of these; only /health is exempt.
# This shell variable is a convenience for curl, not application configuration: the value
# is your own workspace bot token, created under Workspace → Members → API Tokens (surface `mcp_http`). The
# server forwards it to the API unchanged and the call is made as that bot.
export SYLVODE_MCP_BOT_TOKEN=opr_your_own_workspace_bot_token

curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" \
  -H "Authorization: Bearer $SYLVODE_MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
curl -X POST http://localhost:8090/mcp/rpc -H "Content-Type: application/json" \
  -H "Authorization: Bearer $SYLVODE_MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"project_id":"<project-uuid>"}}'

curl -N -H "Accept: text/event-stream" \
  -H "Authorization: Bearer $SYLVODE_MCP_BOT_TOKEN" http://localhost:8090/sse
# → event: endpoint / data: /messages?session_id=<uuid>
curl -X POST "http://localhost:8090/messages?session_id=<uuid>" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $SYLVODE_MCP_BOT_TOKEN" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"projects.list","arguments":{}}}'
```

### Tools (140)

Per-domain counts; the total and sorted-name hash are pinned in
`apps/mcp-server/tool-registry-baseline.json` and checked against the live registry.

| Domain                    | Count | Representative tools                                                      |
| ------------------------- | ----: | ------------------------------------------------------------------------- |
| Universal forms & events  |    34 | `forms.create`, `forms.update_schema`, `form_records.create`, `events.tail` |
| Work items                |    11 | `work_items.create`, `work_items.get_by_identifier`, `work_items.search`   |
| Scenario tools            |     9 | `code.change_proposal.create`, `documents.review_risk`, `approval.request` |
| Flow                      |    42 | `objects.create`, `objects.convert_commit`, `collab.status`, `legacy_pages.import_commit` |
| Project types & resources |     6 | `project_types.get`, `project_resources.create`                            |
| Projects                  |     5 | `projects.list`, `projects.create`                                         |
| Labels                    |     5 | `labels.create`, `labels.list_by_project`                                  |
| Plugins                   |     5 | `plugins.install`, `plugins.invoke`, `plugin_invocations.list`             |
| Proposals & check results |     5 | `proposals.create`, `check_results.create`                                 |
| Sprints                   |     4 | `sprints.create`, `sprints.update`                                         |
| Comments                  |     3 | `comments.create`, `comments.list`                                         |
| Context                   |     3 | `context.get_project`, `context.get_agent_policy`                          |
| Scenario templates        |     3 | `scenario_templates.list`, `scenario_templates.install`                    |
| Operation records         |     1 | `bot_operation_logs.list`                                                  |
| Single-tool domains       |   4×1 | `files.upload`, `members.list`, `search.all`, `release.readiness.get`      |

The full list with exact parameter schemas is generated from the code — do not
transcribe it. Get it with `cargo run --bin list-tools` (no running API needed)
or a `tools/list` JSON-RPC call.

### Resources

Four static resources — `sylvode://skills/openpr-mcp`, `sylvode://guides/agents`,
`sylvode://guides/workflows`, `sylvode://scenario-templates` — plus 22 resource
templates are returned by `resources/templates/list`. The templates include the existing
project, Forms, scenario and issue resources plus all five frozen Flow resources:
`sylvode://objects/{object_id}`, object history and schema, Collection records, and the
workspace/project navigator. The registry lists only `sylvode://` canonical identities;
`resources/read` continues to accept the corresponding `openpr://` alias for every one and
returns `_meta.canonical_uri` with the canonical identity, plus `_meta.deprecation` naming the
replacement and the earliest removal (Sylvode v2.0).

### Command-line client: `sylvode`

The `sylvode` binary (a second `[[bin]]` of the `mcp-server` package, shipped in
every release archive) is the command-line client. It reads the same
configuration file as the MCP server (`[mcp] api_url`, `bot_token`,
`workspace_id`) and has fifteen command groups; `sylvode --help` lists them and
`sylvode <group> --help` shows the options of one group.

| Kind | Groups | Conventions |
| --- | --- | --- |
| Flow | `features`, `objects`, `collections`, `records`, `collab`, `deliveries` | `sylvode.cli.v1` JSON envelope on stdout, typed exit codes, `--workspace` on the commands that take a workspace |
| Workspace | `projects`, `work-items`, `comments`, `labels`, `sprints`, `search`, `files`, `operation-logs`, `tools` | The tool's JSON on stdout, errors on stderr with exit code 1, global `--workspace-id` |

`--format json` (the default) is the stable machine contract; `--format table` is
a human display. `tools call` reaches any of the 140 tools by name, an escape
hatch for anything without a dedicated subcommand. It declares the transport
surface `cli_tools_call` rather than `cli`, so it needs a bot token issued for
that surface (see the surface table above).

```bash
sylvode projects list --format table
sylvode projects create --key WEB --name "Website"
sylvode work-items create --project <uuid> --title "Fix login" --priority high
sylvode files upload --file ./report.pdf
sylvode operation-logs list --outcome error --limit 50
sylvode tools call --name forms.list --args-json '{"project_id":"<uuid>"}'
sylvode features flow get --workspace <uuid>
sylvode objects get <object-uuid> --render markdown
```

`mcp-server` is the MCP server (`mcp-server serve`). It still accepts the nine
workspace groups as a deprecated alias: `mcp-server <group> ...` runs the same
command with byte-identical stdout and the same exit code, and prints one
deprecation warning on stderr. It is not removed before Sylvode v2.0; switch
scripts to `sylvode <group> ...`. The Flow groups exist only under `sylvode`, and
`serve` only under `mcp-server`.

## API

332 method+path endpoints (246 `.route()` calls), all registered in
`apps/api/src/main.rs`. Every route lives under `/api/v1/`, apart from the
unversioned `/health`, `/ready` and `/uploads/*` attachment paths.

Prefixes, all relative to `/api/v1`: auth and admin (`/auth/*`, `/admin/*`,
`/users/*`, `/my/*`); core PM (`/workspaces/*`, `/projects/*`, `/issues/*`,
`/comments/*`, `/sprints/*`, `/labels/*`, `/workflows/*`, `/workflow-states/*`);
Flow (`/flow/*`, `/collab/*`, `/workspaces/{id}/flow/*`); forms (`/forms/*`,
`/form-records/*`, `/form-views/*`, `/form-attachments/*`, `/form-*-jobs/*`,
`/form-import-mapping-templates/*`);
plugins and events (`/plugins/*`, `/check-results/*`);
governance (`/proposals/*`, `/proposal-comments/*`, `/decisions/*`,
`/decision-domains/*`, `/governance/*`, `/trust-scores/*`,
`/impact-reviews/*`, `/vetoers/*`); AI (`/ai/*`, `/ai-participants/*`,
`/ai-learning/*`); templates (`/scenario-templates/*`, `/project-types/*`,
`/proposal-templates/*`); files and misc (`/upload`, `/uploads/*`, `/search`,
`/export/*`, `/notifications/*`).

Search (`/api/v1/search`, MCP `search.all`) matches issues, comments, and
proposals with case-insensitive substring matching.

Responses are `{"code": 0, "message": "success", "data": {...}}` on success and
`{"code": 400, "message": "error description"}` on failure.

## Frontend

SvelteKit 2.50 on Svelte 5 with Tailwind 4, built with **Bun**. The adapter is
`@sveltejs/adapter-static` with `fallback: index.html` — the app ships as a pure
SPA served by nginx with same-origin API proxying in the production image. i18n
is a minimal in-repo store aliased to `svelte-i18n`
(`frontend/src/lib/i18n/svelte-i18n.ts`); `en.json` and `zh.json` each carry
2192 keys, and `bun run --cwd frontend test:i18n-parity` keeps the two key sets equal.

## Scripts

All under `scripts/`.

| Group       | Scripts                                                                   |
| ----------- | ------------------------------------------------------------------------- |
| Lifecycle   | `start.sh` (first-run `config/sylvode.compose.toml` + `config/sylvode.compose.mcp.toml` + compose `.env`, random bootstrap secrets, build release binaries, `compose up -d`; legacy OpenPR filenames are discovered), `dev-up.sh` (PostgreSQL only, for host-side Rust), `stop.sh`, `clean.sh` (**tears down volumes** — destroys database data, asks to confirm) |
| Database    | `init-db.sh` (apply migrations in order), `backup-db.sh` (gzipped dump into `backups/`), `restore-db.sh`                                                                                                                              |
| Verification | `e2e-test.sh` (one-shot end-to-end with automatic teardown), `test-api.sh`, `test-mcp.sh` (legacy v0.4 integration checks), `verify.sh` (component health check)                                                                            |
| Development | `dev-check.sh` (`cargo fmt --check`, `check`, `clippy -D warnings`, `test`), `ci-universal-forms-gates.sh` (reproduce the CI-only `Universal Forms Gates` bundle locally)                                                              |
| Demo data   | `bootstrap-restaurant-demo.sh`, `bun run --cwd frontend smoke:restaurant-ordering`                                                                                                                                                   |
| Other       | `benchmark.sh` (API latency/throughput), `bump-version.sh` (`major\|minor\|patch`, syncs `Cargo.toml` and `frontend/package.json`)                                                                                                     |

> The remaining ~80 `scripts/*universal-forms*` files are historical delivery
> acceptance and signoff scripts, kept for audit traceability. They are not part
> of the normal build or test flow.

## Testing

**Rust tests** — unit and integration tests across the workspace, run with
`cargo test --workspace --no-fail-fast` after
`cargo build -p collab-core --bin collab-isolated-apply-worker`. Database-backed
tests need `OPENPR_TEST_DATABASE_URL` pointing at a PostgreSQL maintenance
connection; without it they print `skipped:` and still count as passed. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the full contract and the CI checks.

**Playwright E2E — 10 specs** in `tests/e2e/web/`: eight cover universal forms
(field design save, human flow, import/export, interaction/IA, mobile + dark
mode, permissions, record CRUD, record detail edit/delete), and two cover the
bot token transport surface and the webhook event list. `playwright.config.ts`
defaults `baseURL` to `http://localhost:3000`; point it at another frontend with
`BASE_URL=http://<host>:<port> npx playwright test`.

**Frontend smoke scripts — 6 `.mjs` scripts** in `frontend/scripts/`, run via
`bun run smoke:*` (`smoke:connections`,
`smoke:phase1-project-types`, `smoke:project-template`,
`smoke:template-work-items`, `smoke:forms-ui`, `smoke:restaurant-ordering`).

**CI.** The `universal-forms` job is not a conventional test suite — it is in-repo
shell audits that grep and pattern-match the source tree to assert that claimed
delivery surfaces still have concrete entrypoints. Behavior is covered by
`cargo test` and the smoke/E2E scripts; treat the two as separate signals.

## Documentation

- `docs/universal-forms-and-plugins.md` — form data model and plugin behavior
- `docs/scenario-templates.md` — built-in scenario template catalog
- `docs/universal-forms-production.md` — production runbook
- `docs/universal-forms-implementation-map.md` — source module / verification command map
- `docs/plugins/openpr-plugin-v1.wit` — plugin ABI v1
- `docs/sylvode-v1.0-compatibility.md` — OpenPR compatibility names, deprecation warnings and upgrade sequence
- `CHANGELOG.md`, `CONTRIBUTING.md`, `SECURITY.md` — release notes, contributor guide, security policy
- `apps/mcp-server/AGENTS.md` — coding-agent workflow patterns and tool examples
- `skills/openpr-mcp/SKILL.md` — governed MCP skill package

## Tech Stack

- **Backend**: Rust edition 2024, axum 0.8, SeaORM 1 (`sqlx-postgres`, rustls), `rust_decimal`, wasmtime 49, Loro (Flow collaboration), PostgreSQL 16
- **Frontend**: SvelteKit 2.50, Svelte 5, Tailwind 4, `@sveltejs/adapter-static`, Bun
- **MCP**: JSON-RPC 2.0 over HTTP, stdio, and SSE
- **Auth**: JWT access + refresh, bot tokens (`opr_`)
- **Deployment**: Docker Compose / Podman, nginx

## Related Projects

| Repository                                                   | Description                                      |
| ------------------------------------------------------------ | ------------------------------------------------ |
| [Sylvode](https://github.com/openprx/sylvode)                | Core platform (this repo)                        |
| [Sylvode Webhook](https://github.com/openprx/openpr-webhook) | Webhook receiver for external integrations       |
| [prx](https://github.com/openprx/prx)                        | AI assistant framework with built-in Sylvode MCP |
| [prx-memory](https://github.com/openprx/prx-memory)          | Local-first MCP memory for coding agents         |
| [wacli](https://github.com/openprx/wacli)                    | WhatsApp CLI with JSON-RPC daemon                |

## Links

[Homepage](https://openprx.dev/sylvode) ·
[Documentation](https://docs.openprx.dev/en/sylvode/) ·
[Community](https://community.openprx.dev) · [OpenPRX](https://openprx.dev)

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) at your
option. `Cargo.toml` declares `license = "MIT OR Apache-2.0"`.
