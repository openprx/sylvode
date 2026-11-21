# Sylvode MCP Server

Model Context Protocol (MCP) server for Sylvode project management system.

## Overview

The MCP Server provides AI models with tools to interact with Sylvode's project management features, including:
- Project management (CRUD operations)
- Project types and scenario templates
- Universal forms, form records, aggregate queries, and business events
- WASM plugin install/invoke and plugin invocation history
- Work item/issue tracking
- Comments and collaboration
- Global search across all entities
- Metadata-only bot operation records

## Install and check

`scripts/install.sh` checks the binary and its configuration, prints the exact commands that register the server in your agent CLI, and ends by running `mcp-server doctor`. It never registers anything and never installs tmux, a CLI or the runner; it only creates the configuration directory when it is missing and never rewrites an existing configuration.

```sh
apps/mcp-server/scripts/install.sh --binary-path ./mcp-server --config-dir ~/.config/sylvode-mcp
apps/mcp-server/scripts/install.sh --dry-run     # writes nothing
```

The configuration file (`<config-dir>/config.toml`) holds the bot token: keep it readable by you only (`chmod 600`). Registration commands, with paths filled in:

```sh
claude mcp add --scope user sylvode -- <abs mcp-server> serve --transport stdio --config <abs config.toml>
codex mcp add sylvode -- <abs mcp-server> serve --transport stdio --config <abs config.toml>
```

`mcp-server doctor [--json] [--config PATH] [--cli claude|codex|opencode ...] [--as-service]` prints the `sylvode.doctor.v1` report: configuration state and file mode, tmux, the agent CLIs, whether the server is registered in each CLI named by `--cli` (`connected`, `registered_failing`, `not_registered`, `binary_missing`; opencode is `unverified`), the runner and the API task protocol. Exit codes: 0 no `fail`, 2 at least one `fail`, 1 the checker could not run. It prints no token, no URL and no output of the programs it probes. Paste its output into bug reports.

`mcp-server --check` prints a one-row `sylvode.doctor.v1` report (version, `task_protocol`, `tool_mode`), reads no secrets and probes nothing. `sylvode-runner doctor` runs it to compare task protocols.

## Scope-native Flow profile

The existing binaries also provide a bounded Flow-only profile for a current global
user, without requiring a default workspace. The normal workspace server and commands
retain their existing startup requirements and bot/project policy.

```sh
sylvode flow tools
sylvode --api-url http://127.0.0.1:3000 --access-token-file /private/path/user-token flow call --name flow.home
sylvode --api-url http://127.0.0.1:3000 --access-token-file /private/path/user-token flow call --name objects.query --args-json '{"personal":true}'
mcp-server --api-url http://127.0.0.1:3000 serve --flow-only --transport stdio --access-token-file /private/path/user-token
mcp-server --api-url http://127.0.0.1:3000 serve --flow-only --transport http --bind-addr 127.0.0.1:8090
```

Use an existing API user access token in a private file; `--access-token` is also
accepted but may be visible in process arguments or shell history. The profile does
not obtain, refresh, or impersonate a user credential. Expiry is reported by the API.
It never interprets `mcp.bot_token` as a human credential. HTTP/SSE require each
caller's actual Bearer token and reject a shared `--access-token` configuration.
No token is needed for `flow tools`, which prints static schemas without API calls.

The profile now registers 36 names: 34 actual existing Flow registry tools plus
`flow.home` and the bounded `objects.command` alias. Existing names cover object
read/create/patch/move/link/unlink/diff, grants/inheritance/relations, Collections and
Records, comments/mentions, page shares/appearance, trash/purge, projection lag and
Forms references. Success values retain baseline data DTOs; typed business errors
remain structured.

`objects.command` additionally requires statically mapped baseline operation policies
on every affected project (and the outer alias policy). It cannot bypass a denied
`objects.patch`, `objects.link`, `objects.unlink` or `objects.move`. Atomic embedding
also requires `objects.create`. Named adapters keep their actual existing policy names,
without requiring the internal generic alias permission. Client payloads cannot select
a policy name.

Object `at_seq`/`render` options map to bounded API reads; Collection `at_seq` is only a
current `schema_seq` guard. A Collection create with `embed_page_id` derives scope and
project from that actual Page, not personal home. Optional context IDs are checked claims;
`personal` and `parent_id` cannot be combined with this variant. Omitted `after_block_id`
appends; only existing tree block anchors support relative placement. Editor prose CID
or alias relative placement is explicitly unsupported by the backend.

Personal moves reparent non-root Pages/Collections inside their existing scope, requiring source
management and target edit. Records retain their Collection association; navigator roots and
Collection/Record destinations are refused. Cross-scope ownership changes use no move fallback.
Navigator SQL hierarchy and CRDT ordering commit together; missing legacy placements are initialized
in creation order. Both trash and projection-lag accept cursor/limit (1..100), with requester/query-
bound cursors and an explicit published authorization scan-budget refusal. Lag aggregates cover the
authorized scope before pagination. Personal scope has no project filter; legacy adapters retain
the original workspace services and query contract.

Only registered tools can dispatch. Package promotion/export, historical restoration,
conversion jobs and workspace administrative/maintenance tools remain excluded from the
personal profile. The normal legacy server retains its own existing services and gates.
`objects.export` remains current-head rendered JSON, Markdown or CSV, never a raw typed
package. Explicit `initialize_editor_doc` requires a Page, an empty payload and captured
`expected_frontier`; nothing initializes during tools/list or ordinary metadata reads.

List/create/search selectors require exactly one of `personal:true`, `scope_id`, or
a genuine `workspace_id`; project claims must match actual resource context.
Object-targeted calls resolve the object's canonical scope. Personal home is resolved
only during an actual Flow call and is never cached between authenticated callers.
Backend authorization remains authoritative. OPR workspace-bot calls retain genuine
workspace adapters and Read/Write/project gates; they cannot select personal home.
A personal scope skips only the project policy that requires a project, not resource
authorization or the tool policy/audit path.

This profile has developer mock/schema/identity coverage. It is not a claim of live
MCP transport acceptance or of backend support for every broader Flow feature.

## Features

- **MCP tools** (counts in the tool table below): Project, governance, universal forms, WASM plugin, operation records, release next actions, template, scenario toolkit, and Sylvode Flow
- **Three Transport Modes**: stdio (for MCP clients), HTTP JSON-RPC, and SSE
- **JSON Schema Validation**: All tool parameters are strongly typed
- **Sylvode API Backend**: Calls the Sylvode API with workspace-scoped bot credentials
- **Async/Await**: Built on Tokio for high performance

## Quick Start

### Prerequisites

Sylvode reads no environment variables. Create a configuration file carrying the `[mcp]`
section — `config/sylvode.example.toml` is the full annotated reference:

```toml
[mcp]
api_url = "http://localhost:8081"
# Required for transport = "stdio" and for the CLI subcommands. Not used by http/sse.
bot_token = "opr_your_token_here"
workspace_id = "your-workspace-uuid"
```

Save it as `config/sylvode.toml` (the default path the binary looks for) or anywhere else and
pass `--config <path>`:

```bash
mcp-server serve --config config/sylvode.toml
```

New installations should invoke the `sylvode` CLI, which carries both the Flow
commands and the workspace commands (`projects`, `work-items`, `comments`,
`labels`, `sprints`, `search`, `files`, `operation-logs`, `tools`). Those
workspace commands under `mcp-server` are the same commands with the same output
and exit codes, but print a deprecation warning on stderr; `mcp-server serve`
does not warn. The legacy `config/openpr.toml` default also warns when it is
discovered. None of these is removed before Sylvode v2.0, and MCP tool names are
not renamed; see the
[compatibility matrix](../../docs/sylvode-v1.0-compatibility.md).

| Config key | Required | Purpose |
| --- | --- | --- |
| `mcp.api_url` | no, default `http://localhost:8081` | Base URL of the Sylvode API. |
| `mcp.bot_token` | conditional | Workspace bot token, used only by `stdio` and the CLI subcommands, which have no per-request caller to act on. Ignored by `http`/`sse`. |
| `mcp.workspace_id` | yes | Workspace UUID the calls are scoped to. Not used for delegated (OAuth) callers, whose workspace comes from their token. |
| `mcp.oauth_issuer` | with the two keys below | Issuer of the Sylvode authorization server. |
| `mcp.oauth_resource` | with the other two | Identifier of this server as a protected resource: the exact URL that receives JSON-RPC posts, for example `http://127.0.0.1:8090/mcp/rpc`, or `https://sv.example.com/mcp` behind a proxy that maps that path to `/mcp/rpc`. Must equal `[oauth] mcp_resource` of the API. |
| `mcp.oauth_forward_secret` | with the other two | Internal forwarding credential, at least 32 bytes, never logged. Must equal `[oauth] internal_forward_secret` of the API. |
| `mcp.tool_mode` | no, default `search` | Which tools `tools/list` offers; see [Tool modes](#tool-modes). |
| `mcp.allow_write_call` | no, default `true` | `false` disables every write tool except the five task lifecycle tools, in every tool mode and in the command line; see [Tool modes](#tool-modes). |

There is no `mcp.auth_token` key any more. If your configuration file still has one, delete
it — the server refuses to start rather than run with a key that no longer does anything.

### Tool modes

`tools/list` offers nine tools by default (**search mode**): `sylvode_overview` (categories and how
to call), `sylvode_search` (find a tool by words, category or exact name; the answer carries the
tool's input schema), `sylvode_read` and `sylvode_write` (call one registry tool by name,
`sylvode_read` only a tool that reads, `sylvode_write` only a tool that changes data), and the five
task tools `sylvode_task_get`, `sylvode_task_ack`, `sylvode_task_progress`,
`sylvode_task_complete` and `sylvode_task_fail`. The other tools are not listed, and calling one
of them by its name is refused with a pointer to `sylvode_read` or `sylvode_write`. The wrappers run
every check of a direct call (project agent policy, ownership, admin rules) under the target's name,
validate the arguments against the target's own schema, and the operation log shows the target.

**Full mode** is the explicit opt-in: every registry tool is listed and callable by name, and the
four entry tools are not offered.

| Transport | Chosen by |
| --- | --- |
| stdio | `[mcp] tool_mode`; `serve --tools search|full` overrides it |
| HTTP `/mcp/rpc` | per request: `?tools=search|full` or the `X-Sylvode-Tool-Mode` header (a conflict or an unknown value is a 400); otherwise `[mcp] tool_mode` |
| SSE | when the stream opens (`GET /sse?tools=...` or the header); the session keeps it |
| `serve --flow-only` | not applicable; `--tools` is refused |
| connected (OAuth) applications | ignored: they always get the scoped read tools and no entry or task tools |

`[mcp] allow_write_call = false` is a control, not a hint: every tool the catalogue marks as
writing is refused with `writes are disabled for this bot by the runner configuration` before any
API request, whichever way it is called: through `sylvode_write`, by its own name in full mode
(configured, `?tools=full`, the header, an SSE session), and from the command line (`tools call`
and the workspace commands). `sylvode_write` stays listed and answers with the refusal. The five
task lifecycle tools (`sylvode_task_get`, `_ack`, `_progress`, `_complete`, `_fail`) are the one
exception: a runner bot needs them, and the API only lets a bot act on its own tasks. A plain caller's `initialize` also carries `instructions` and
`capabilities.experimental["sylvode/task-protocol"] = {"version": 1}`.

### Inbound authentication (HTTP/SSE)

There is no shared secret and no server-side identity for the networked transports to fall
back to. Every request to `/mcp/rpc`, `/sse` and `/messages` must carry the *caller's own*
MCP-type account token:

```
Authorization: Bearer opr_...
```

The MCP server does not itself validate this token beyond reading it out of the header — it
forwards it to the Sylvode API unchanged, and the API authenticates it and enforces
authorization. A request with no `Authorization` header, or a malformed one, is rejected
with `401` and `WWW-Authenticate: Bearer` before it reaches any tool. Because every caller
supplies their own credential, `mcp.bind_addr` can be `0.0.0.0` or any other reachable
address without extra configuration — there is no "anonymous acting as the server" case to
guard against.

The caller's token must belong to the same workspace named by `mcp.workspace_id` in this
server's configuration; the API returns `403` if it belongs to a different workspace.

`/health` is exempt: it answers a constant `OK`, reads nothing, and discloses nothing a
caller who completed the TCP handshake does not already know, so orchestrator probes need
no credential.

**stdio is unaffected**: it is a pipe pair owned by the process that spawned the server, so
the caller is already established, and the process authenticates to the API with
`mcp.bot_token` from the configuration file.

```bash
# Authenticated HTTP call. Use the caller's own MCP-type account token, not a shared secret.
curl -H "Authorization: Bearer opr_your_own_account_token" \
     -H 'Content-Type: application/json' \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
     http://localhost:8090/mcp/rpc
```

### Delegated access (OAuth)

Sylvode can act as an OAuth 2.1 authorization server for its own API and for this MCP server;
Ramflux is the one pre-registered client. A delegated caller presents an access token that
starts with `sva_` and was issued for this server's resource. Setting the three `mcp.oauth_*`
keys together enables it on the **`http` transport**, on `POST /mcp/rpc` only. A partial set
refuses to start with a message naming the missing keys. Without them `sva_` tokens are
refused with `401` and nothing else changes. The `sse` transport and `serve --flow-only` never
serve delegated callers.

What the server does for a delegated caller:

- **Discovery.** `GET /.well-known/oauth-protected-resource` and the path-based form (for the
  resource `http://127.0.0.1:8090/mcp/rpc`: `/.well-known/oauth-protected-resource/mcp/rpc`)
  publish the RFC 9728 document. Both need no token. A request without a token gets `401` with
  `WWW-Authenticate: Bearer realm="sylvode", resource_metadata="...", scope="projects:read work_items:read"`.
- **Verification.** Before anything else, the server asks the API what the token allows
  (`GET /api/v1/oauth/internal/token-context`). The API's `401` becomes our `401`
  (`error="invalid_token"`; unknown, expired, revoked and wrong-audience tokens are not told
  apart). If the API cannot be asked or its answer cannot be understood the request is refused
  with `503`; it is never served with broader access.
- **Workspace.** The caller acts in the workspace of its grant, taken from the token context,
  never from `mcp.workspace_id` and never from a tool argument. A `workspace_id` argument that
  differs from the grant is refused, not replaced. For a grant limited to some projects the token
  context lists them (`project_ids`; an answer without the list is refused as malformed), and a
  `project_id` argument outside the list is refused before the API is called with the answer the
  API gives for any object outside a grant, so it does not reveal whether the project exists.
- **Tools.** `tools/list` shows only the tools the token's scopes allow; `tools/call` for a
  tool outside them answers `403` with `WWW-Authenticate: Bearer error="insufficient_scope",
  scope="...", resource_metadata="..."` and a JSON-RPC error `-32001`. The first release
  exposes **eight read tools only**: `projects:read` (`projects.list`, `projects.get`,
  `context.get_project`) and `work_items:read` (`work_items.list`, `.get`,
  `.get_by_identifier`, `.list_labels`, `comments.list`). A `:write` scope implies its `:read`
  scope but unlocks no tool that changes data. Every other tool is refused with JSON-RPC error
  `-32002` (`tool not available to connected applications`) whatever the scopes say: anything
  that writes, deletes, administers, imports or exports, or dispatches an agent, and the read
  tools whose API routes are not on the API's delegated allow-list, which would only fail
  closed (`work_items.search`, `labels.list_by_project`, and the forms and form-record reads).
  They stay available to bot and user tokens. A tool that is not in the table of
  `src/oauth_scopes.rs` is refused too. `resources/read` serves the static guides only. A test
  (`every_tool_available_to_delegated_callers_calls_a_route_the_api_allows_for_its_scope`)
  calls every delegated tool against a recording API and checks that the route it really calls
  is on the API's allow-list for the tool's scope.
- **Forwarding (deviation from the MCP authorization specification).** The specification says an
  MCP server must not pass through the token it received. This server forwards the caller's
  MCP-audience token to the Sylvode API together with an internal forwarding credential
  (`X-Sylvode-Forwarded-By: mcp-server`, `X-Sylvode-Forward-Auth: <mcp.oauth_forward_secret>`),
  and the API accepts an MCP-audience token only with that credential. The MCP server is a
  stateless facade over its own API, holds no signing key and no database, and the API is the
  issuer and the policy point for both audiences; a token for one audience is still refused at
  the other. It costs one shared secret in the API and MCP configurations. Release notes must
  state this deviation.
- **Protocol version.** The server keeps declaring `2024-11-05`; delegated clients use plain
  JSON-RPC over `POST /mcp/rpc`.
- **Tokens and secrets.** The token is read from the `Authorization` header only: never from a
  query string, a cookie, a tool argument or the environment. Neither it nor the forwarding
  credential is logged, echoed in an error or a tool result, or written anywhere.

`opr_` bot tokens and user tokens behave exactly as before: they are forwarded verbatim, the
configured `mcp.workspace_id` applies, and they never carry the forwarding headers.

### Build

```bash
cargo build -p mcp-server --release
```

### Run

#### stdio Mode (for MCP clients)
```bash
./target/release/mcp-server serve --transport stdio --config config/sylvode.toml
```

#### HTTP Mode (for testing/debugging)
```bash
# Loopback default (127.0.0.1:8090). Every request still needs its own caller bot token.
./target/release/mcp-server serve --transport http --config config/sylvode.toml

# Reachable from other hosts: no extra config needed, since there is no server-side
# credential to expose. Each request still authenticates with its own caller bot token.
./target/release/mcp-server serve --transport http --bind-addr 0.0.0.0:8090 --config config/sylvode.toml
```

#### SSE Mode (for streaming clients)
```bash
./target/release/mcp-server serve --transport sse --config config/sylvode.toml
```

## Available Tools

<!-- BEGIN GENERATED: tool-table (sylvode-skillgen; do not edit) -->
The MCP server registers 153 tools in 19 categories. By default it lists nine of them (search mode) and reaches the rest through `sylvode_search`, `sylvode_read` and `sylvode_write`. Every tool, with its category and access, is in [reference/tools.md](../../skills/sylvode-mcp/reference/tools.md).

| Category | Tools | Read | Write | Covers |
|---|---:|---:|---:|---|
| `projects`: Projects | 5 | 2 | 3 | Create, read, update and delete projects |
| `project_types`: Project types | 2 | 2 | 0 | Read the project scenario types and their capability schemas |
| `scenario_templates`: Scenario templates | 3 | 2 | 1 | Browse and install ready-made multi-form setups |
| `context`: Context | 3 | 3 | 0 | Read project context, governance and the agent policy |
| `forms`: Universal forms | 34 | 16 | 18 | Forms, records, attachments, permissions, imports and business events |
| `plugins`: Plugins | 5 | 3 | 2 | Install, inspect and invoke WASM plugins |
| `work_items`: Work items | 11 | 5 | 6 | Issues of a project: list, read, create, update, label |
| `comments`: Comments | 3 | 1 | 2 | Discuss work items |
| `files`: Files | 1 | 0 | 1 | Upload files and get their URLs |
| `labels`: Labels | 5 | 2 | 3 | Workspace and project labels |
| `sprints`: Sprints | 4 | 1 | 3 | Plan iterations of a project |
| `proposals`: Proposals | 5 | 2 | 3 | Governance proposals and check results |
| `release`: Release | 1 | 1 | 0 | Release readiness gates and blockers |
| `code`: Code | 2 | 1 | 1 | Context and governed proposals for code tasks |
| `documents`: Documents | 3 | 0 | 3 | Governed document summaries, risk reviews and approvals |
| `operations`: Operations | 2 | 0 | 2 | Governed inspection reports and corrective actions |
| `workspace`: Workspace | 3 | 3 | 0 | Members, global search and bot operation records |
| `flow`: Flow | 56 | 21 | 35 | Pages, collections, shares, comments, imports, exports and maintenance of Flow |
| `tasks`: Agent tasks | 5 | 1 | 4 | The protocol of tasks assigned to a bot: get, acknowledge, report progress, complete, fail |

Every tool, one line each, is in [skills/sylvode-mcp/reference/tools.md](../../skills/sylvode-mcp/reference/tools.md).
<!-- END GENERATED: tool-table -->

The Flow tools carry the Flow contracts (`objects.*`, `collections.*`, `records.create`, `collab.*`, `flow.*`, `legacy_pages.*`); `objects.command` is described under the scope-native Flow profile above. The legacy page tools answer `not_required_zero_inventory` on this deployment.

The registry exposes five static resources (the skill `sylvode://skills/sylvode-mcp`, its tool reference `sylvode://skills/sylvode-mcp/tools`, `sylvode://guides/agents`, `sylvode://guides/workflows`, `sylvode://scenario-templates`) and 21 templates under the canonical
`sylvode://` scheme, including all five frozen Flow resource templates. The matching
identity of every registry row is the `sylvode://` one; the retired `openpr://` spelling is not
served. The native `sylvode`
CLI (a second `[[bin]]` in this package) shares the same client and command contracts.

## Usage Examples

### List Tools
```bash
# Using stdin/stdout: nine tools (search mode); add --tools full for the whole registry
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}' | \
  ./target/release/mcp-server serve --transport stdio --config config/sylvode.toml

# Project-aware capability filtering
echo '{"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"project_id": "<project-uuid>"}}' | \
  ./target/release/mcp-server serve --transport stdio --config config/sylvode.toml
```

### Call a Tool
```bash
# List projects in a workspace. Registry tools are called by name in full mode; in the default
# search mode send {"name": "sylvode_read", "arguments": {"tool": "projects.list", "arguments": {...}}}.
echo '{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "projects.list",
    "arguments": {
      "workspace_id": "550e8400-e29b-41d4-a716-446655440000"
    }
  }
}' | ./target/release/mcp-server serve --transport stdio --tools full --config config/sylvode.toml
```

### HTTP Mode Example
```bash
# Start server
./target/release/mcp-server serve --transport http --config config/sylvode.toml

# Call tool via HTTP -- Authorization carries the caller's own MCP-type account token.
# `?tools=full` lists and accepts registry names directly; without it use sylvode_read/sylvode_write.
curl -X POST "http://localhost:8090/mcp/rpc?tools=full" \
  -H "Authorization: Bearer opr_your_own_account_token" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "tools/call",
    "params": {
      "name": "work_items.search",
      "arguments": {
        "workspace_id": "550e8400-e29b-41d4-a716-446655440000",
        "query": "bug"
      }
    }
  }'
```

## Docker

### Using Docker Compose

```bash
# Start all services (including MCP server)
docker compose up -d mcp-server

# MCP server will be available at localhost:8090
```

### Configuration in docker-compose.yml

```yaml
mcp-server:
  build:
    context: .
    dockerfile: Dockerfile.prebuilt
    args:
      APP_BIN: mcp-server
  command:
    - "/app/mcp-server"
    - "serve"
    - "--config"
    - "/app/config/sylvode.toml"
    - "--transport"
    - "http"
    - "--bind-addr"
    - "0.0.0.0:8090"
  volumes:
    # Carries [logging] and [mcp] only -- no database URL and no signing key reach this service.
    - ./config/sylvode.compose.mcp.toml:/app/config/sylvode.toml:ro
  ports:
    - "127.0.0.1:8090:8090"
  depends_on:
    api:
      condition: service_healthy
```

The default compose stack uses `Dockerfile.prebuilt`, so build the release
binary first or use `bash scripts/start.sh`, which performs that build before
starting compose. `scripts/start.sh` also generates `config/sylvode.compose.mcp.toml`;
`config/sylvode.example.toml` is the annotated reference for every key.

## Development

### Project Structure

```
apps/mcp-server/
├── src/
│   ├── main.rs           # Entry point and transport layer
│   ├── lib.rs            # Library exports
│   ├── protocol.rs       # MCP protocol types
│   ├── server.rs         # Core MCP server logic
│   ├── client/           # Sylvode API client helpers
│   ├── tools/            # Tool implementations
│   │   ├── forms.rs
│   │   ├── plugins.rs
│   │   ├── operation_logs.rs
│   │   ├── project_types.rs
│   │   ├── scenario_templates.rs
│   │   ├── projects.rs
│   │   ├── work_items.rs
│   │   ├── comments.rs
│   │   └── search.rs
│   └── bin/
│       └── list-tools.rs # Utility to list all tools
```

### List All Tools

```bash
cargo run --bin list-tools
```

This outputs the currently registered MCP tools with their complete JSON Schema definitions.

`cargo run --bin list-tools -- --json` prints one row per tool with its name, access class (`read` or `write`) and category, taken from the tool catalogue in `src/tools/catalog.rs`. The full `tools/list` carries `annotations.readOnlyHint` from the same catalogue.

### Adding a New Tool

1. Add or extend a Sylvode API client helper in `src/client/`.
2. Add tool definition and handler in `src/tools/<module>.rs`:
   ```rust
   pub fn my_tool_definition() -> ToolDefinition {
       ToolDefinition {
           name: "namespace.action".to_string(),
           description: "What this tool does".to_string(),
           input_schema: json!({
               "type": "object",
               "properties": { ... },
               "required": [ ... ]
           }),
       }
   }
   
   pub async fn my_tool(state: &AppState, args: Value) -> CallToolResult {
       // Implementation
   }
   ```
3. Register in `src/tools/mod.rs`
4. Add dispatch case in `src/server.rs`

## Testing

### Manual Testing

```bash
# Start the Sylvode API stack
bash scripts/start.sh

# Run MCP server
cargo run -p mcp-server -- serve --transport stdio --config config/sylvode.toml

# Test with example request
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}' | \
  cargo run -q -p mcp-server -- serve --transport stdio --config config/sylvode.toml | jq '.'

# Test project-aware tool discovery
echo '{"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"project_id": "<project-uuid>"}}' | \
  cargo run -q -p mcp-server -- serve --transport stdio --config config/sylvode.toml | jq '.'
```

### Integration with MCP Clients

The MCP server works with any MCP-compatible client:
- Claude Desktop
- MCP Inspector
- Custom MCP clients

Configure the client to use the server. The MCP server reads no environment variables, so
the client config points `--config` at a file carrying `[mcp]` (see Prerequisites above):
```json
{
  "mcpServers": {
    "sylvode": {
      "command": "/path/to/mcp-server",
      "args": ["serve", "--transport", "stdio", "--config", "/absolute/path/to/sylvode.toml"]
    }
  }
}
```

## Troubleshooting

### "mcp.auth_token has been removed"
The server refuses to start if the configuration file still has an `mcp.auth_token` key —
that key was a shared inbound secret in an earlier version and no longer does anything.
Delete the line. Every HTTP/SSE caller now presents its own workspace bot token instead;
see [Inbound authentication (HTTP/SSE)](#inbound-authentication-httpsse) above.

### "mcp.bot_token is required by the stdio transport and by the CLI subcommands..."
`stdio` and the CLI subcommands have no per-request caller to act on, so they need an
identity of their own. Create a workspace bot token and set it, along with the API URL and
workspace ID, in the `[mcp]` section of the configuration file:
```toml
[mcp]
api_url = "http://localhost:8081"
bot_token = "opr_your_token_here"
workspace_id = "your-workspace-uuid"
```
`http` and `sse` never need `mcp.bot_token`: every request already carries its own caller's
bot token.

### API Connection Refused
MCP talks to the Sylvode API, not directly to PostgreSQL. Confirm the API is running, then
point `mcp.api_url` at it in the configuration file:

```bash
curl http://localhost:8081/health
```

```toml
[mcp]
api_url = "http://localhost:8081"
```

### Tool Not Found
Use `cargo run --bin list-tools` to see all available tools and their exact names.

## Performance

- **API Requests**: Uses the Sylvode API client and relies on API-side indexes and authorization
- **Search Limits**: Results limited to prevent large responses
  - Work item search: 50 results
  - Global search: 20 results per category
- **Async I/O**: Non-blocking operations throughout

## Security

MCP is an API client. It does not accept arbitrary database credentials and it
does not bypass Sylvode authorization. It never verifies a bot token itself — it has no
signing key and no bot registry, so it forwards the token it holds and lets the API decide.

- `stdio` and the CLI subcommands act as the identity in `mcp.bot_token`, configured in the
  file.
- `http` and `sse` act as whoever called them: every request must carry its own caller's
  bot token in `Authorization: Bearer opr_...`, which this server forwards to the API
  unchanged. There is no shared secret and no server-side fallback identity for these
  transports — a request without a usable caller token gets `401` before it reaches any
  tool (`/health` is exempt).
- The API authenticates `opr_` bot tokens by SHA-256 token hash in
  `workspace_bots`.
- Disabled or expired bot tokens are rejected.
- Workspace access is enforced by the API; a bot token can only act inside its workspace,
  and must belong to the workspace named by this server's `mcp.workspace_id` — a token from
  another workspace gets `403`.
- JWT user tokens still use the normal API JWT validation path.
- Delegated (OAuth) callers present `sva_` tokens on `POST /mcp/rpc` of the `http` transport
  when `mcp.oauth_*` is configured; see [Delegated access (OAuth)](#delegated-access-oauth).
  They are checked against the API on every request, see only the read tools their scopes
  allow, act in their grant's workspace, and are forwarded to the API with an internal
  forwarding credential (deviation D2 of the authorization contract).
- A tool takes exactly the arguments its schema declares where that matters for authorization:
  an ownership bearing id (`work_item_id`, `form_id`, `record_id`, ...) the tool does not declare
  is refused before any request, because it could change which project's agent policy is
  evaluated, and a tool with a closed schema refuses any undeclared argument on every door.
- `sylvode_search` and `sylvode_overview` with a `project_id` read the project's agent policy with
  the caller's own token and filter by the tools it enables. The filtered answer therefore shows
  which tools that policy enables even when the policy disables `context.get_agent_policy`; the
  policy itself is not shown, and `tools/list` with a project discloses exactly the same. Without
  a `project_id` the static catalogue is shown, which is public information.
- The project resource tools (`project_resources.*`, `code.resources.list`, `code.directory.get`)
  and the resource template `sylvode://projects/{project_id}/resources` are parked: unreachable on
  every MCP door. This narrows the MCP surface only. The resources stay readable through
  `context.get_project` (and `sylvode://projects/{project_id}/context`) and through the API with
  the bot's token; the API's own authorization decides.
- For public HTTP exposure, place MCP behind your reverse proxy or tunnel and
  apply deployment-level rate limits.

## License

AGPL-3.0-or-later

## Resources

- [MCP Specification](https://modelcontextprotocol.io/)
- [Sylvode API Documentation](../../docs/)
- [Database Schema](../../migrations/)

### Flow multi-resource project policy

Flow-only mutations resolve actual source/target owners during each call, then check
every distinct legacy project before dispatch. Link/move use actual target objects;
unlink resolves an authorized outgoing relation and its target; record mutations and
declared relation-field values resolve their actual targets. Plain text/user JSON is
not recursively scanned for ownership hints. Caller project claims are not authority.
Archive/restore stay source-project scoped under the backend subtree invariant.

These are fresh admission/dispatch checks combined with backend resource authorization,
not a transactionally atomic policy guarantee across a concurrent project move. No
identity or backend permission model is changed. If unlink cannot resolve its target
(including an unavailable/removed target), it refuses without sending a mutation; this
can conservatively limit cleanup that another API surface permits. Relation lookup
is bounded to 100 pages, and target resolution to 1,000 distinct affected objects.

Forms reference creation checks the real destination Form/record project even for a
personal source, using request-local ownership resolution. Link cleanup through
`objects.unreference` follows source-Edit semantics and does not read or mutate the
Forms destination. This differs from Flow-to-Flow unlink, which retains both-endpoint
Edit authorization. No API permission is relaxed by these adapters.
