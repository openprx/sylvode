# Sylvode MCP Server

Model Context Protocol (MCP) server for Sylvode project management system.

## Overview

The MCP Server provides AI models with tools to interact with Sylvode's project management features, including:
- Project management (CRUD operations)
- Project types, scenario templates, and project resources
- Universal forms, form records, aggregate queries, and business events
- WASM plugin install/invoke and plugin invocation history
- Work item/issue tracking
- Comments and collaboration
- Global search across all entities
- Metadata-only bot operation records

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
remain structured. The normal 154-tool registry is unchanged.

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

- **154 MCP Tools**: Project, governance, universal forms, WASM plugin, operation records, release next actions, template, scenario toolkit, and Sylvode Flow
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

There is no `mcp.auth_token` key any more. If your configuration file still has one, delete
it — the server refuses to start rather than run with a key that no longer does anything.

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

### Projects
- `projects.list` - List all projects in a workspace
- `projects.get` - Get project details by ID
- `projects.create` - Create a new project
- `projects.update` - Update project details

### Project Types, Templates, and Resources
- `project_types.list`, `project_types.get`
- `scenario_templates.list`, `scenario_templates.get`, `scenario_templates.install`
- `project_resources.list`, `project_resources.create`, `project_resources.update`, `project_resources.delete`

### Universal Forms and Events
- `forms.list`, `forms.get`, `forms.create`, `forms.create_from_template`, `forms.update_schema`, `forms.duplicate`
- `forms.schema_summary`, `forms.field_usage`, `forms.field_dependencies`
- `form_schema_versions.list`, `form_schema_versions.get`
- `form_views.list`
- `form_permissions.get`, `form_permissions.update`
- `form_attachments.list`, `form_attachments.create`, `form_attachments.archive`, `form_attachments.restore`
- `form_records.list`, `form_records.export`, `form_records.import_preview`, `form_records.import_commit`
- `form_records.get`, `form_records.create`, `form_records.update`
- `form_records.link`, `form_records.relation_targets`, `form_records.children`
- `form_records.child_create`, `form_records.child_update`, `form_records.child_archive`, `form_records.child_restore`, `form_records.aggregate`
- `events.tail`

### WASM Plugins
- `plugins.list`, `plugins.get`, `plugins.install`, `plugins.invoke`
- `plugin_invocations.list`

### Work Items
- `work_items.list` - List work items in a project
- `work_items.get` - Get work item details by ID
- `work_items.create` - Create a new work item
- `work_items.update` - Update work item details
- `work_items.search` - Search work items by text

### Comments
- `comments.list` - List comments on a work item
- `comments.create` - Add a comment to a work item

### Search
- `search.all` - Global search across projects, work items, and comments

### Scenario Tools
- `code.resources.list`, `code.directory.get`, `code.task_context.get`, `code.change_proposal.create`
- `documents.extract_summary`, `documents.review_risk`, `approval.request`
- `inspection.report`, `corrective_action.propose`

### Sylvode Flow (v0.5)
- `flow.feature_get`, `flow.feature_set` — the Flow workspace rollout flag
- `objects.get`, `objects.query`, `objects.history` — Flow object read tools
- `objects.create`, `objects.patch`, `objects.move`, `objects.link`, `objects.unlink` — idempotent Flow writes
- `objects.diff`, `objects.relations`, `objects.search`, `collab.projection_lag` — policy-filtered derived reads
- `objects.grants_get`, `objects.grants_set`, `objects.inheritance_set` — object authorization tools with dry-run support
- `legacy_pages.inventory`, `legacy_pages.import_preview`, `legacy_pages.import_commit`, `legacy_pages.import_status` — ADR-0003 conditional migration tools; this deployment's inventory is zero, so the import tools always answer `not_required_zero_inventory`

The v0.9 registry exposes four static resources and 22 templates under the canonical
`sylvode://` scheme, including all five frozen Flow resource templates. The matching
`openpr://` identity remains a read alias for every registry row and resolves to the same
payload and policy subject; aliases are not duplicated in list results. The native `sylvode`
CLI (a second `[[bin]]` in this package) shares the same client and command contracts.

## Usage Examples

### List Tools
```bash
# Using stdin/stdout
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}' | \
  ./target/release/mcp-server serve --transport stdio --config config/sylvode.toml

# Project-aware capability filtering
echo '{"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"project_id": "<project-uuid>"}}' | \
  ./target/release/mcp-server serve --transport stdio --config config/sylvode.toml
```

### Call a Tool
```bash
# List projects in a workspace
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
}' | ./target/release/mcp-server serve --transport stdio --config config/sylvode.toml
```

### HTTP Mode Example
```bash
# Start server
./target/release/mcp-server serve --transport http --config config/sylvode.toml

# Call tool via HTTP -- Authorization carries the caller's own MCP-type account token.
curl -X POST http://localhost:8090/mcp/rpc \
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
