# Sylvode MCP server

`mcp-server` is the Model Context Protocol (MCP) server of Sylvode. It gives AI agents and MCP
clients tools for projects, project types and scenario templates, work items, comments, labels,
sprints, universal forms and their records, WASM plugins, governance proposals and check results,
release readiness, governed code, document and operations work, files, workspace members and
search, bot operation records, Sylvode Flow, and the agent task protocol. The generated table in
[Tools](#tools) lists every category.

The server is a client of the Sylvode API. It has no database connection and no signing key: every
tool call becomes an API request made with the caller's credential, and the API decides what the
caller may do.

The same package builds the `sylvode` command line, which runs the same tools from a shell; see
[docs/integrations/cli.md](../../docs/integrations/cli.md).

- [Install and register](#install-and-register)
- [Configuration](#configuration)
- [Commands](#commands)
- [Transports and endpoints](#transports-and-endpoints)
- [Authentication](#authentication)
- [Tool modes](#tool-modes)
- [Write control](#write-control)
- [Delegated access (OAuth)](#delegated-access-oauth)
- [Flow-only profile](#flow-only-profile)
- [Resources](#resources)
- [Tools](#tools)
- [Examples](#examples)
- [Security notes](#security-notes)
- [Troubleshooting](#troubleshooting)
- [License](#license)

## Install and register

The release archives `sylvode-<platform>.tar.gz` (and the Windows `.zip`) contain `mcp-server`
and `sylvode` next to the other Sylvode binaries; [docs/install.md](../../docs/install.md) covers
the archives, the compose stack and building from source.

`apps/mcp-server/scripts/install.sh` (in a checkout of the repository) checks the binary and its
configuration, prints the exact commands that register the server in your agent CLI, and ends by
running `mcp-server doctor`. It never registers anything, never installs tmux, an agent CLI or the
runner, and never rewrites an existing configuration. The only thing it writes is the
configuration directory, when it does not exist yet.

```sh
apps/mcp-server/scripts/install.sh --binary-path ./mcp-server --config-dir ~/.config/sylvode-mcp
apps/mcp-server/scripts/install.sh --dry-run     # writes nothing
```

| Option | Default | Meaning |
|---|---|---|
| `--binary-path PATH` | `./mcp-server`, then `./target/release/mcp-server` | The `mcp-server` binary to check |
| `--config-dir PATH` | `~/.config/sylvode-mcp` | Directory holding `config.toml` |
| `--dry-run` | off | Write nothing; report what would be created, then run doctor |

`config.toml` in that directory holds the bot token: keep it readable by you only (`chmod 600`).
The registration commands it prints look like this, with absolute paths filled in:

```sh
claude mcp add --scope user sylvode -- /srv/sylvode/bin/mcp-server serve --transport stdio --config /srv/sylvode/mcp/config.toml
codex mcp add sylvode -- /srv/sylvode/bin/mcp-server serve --transport stdio --config /srv/sylvode/mcp/config.toml
```

For opencode it prints the `mcp` entry to add to `opencode.json`. Any other MCP client that starts
a stdio server takes the same command and arguments:

```json
{
  "mcpServers": {
    "sylvode": {
      "command": "/srv/sylvode/bin/mcp-server",
      "args": ["serve", "--transport", "stdio", "--config", "/srv/sylvode/mcp/config.toml"]
    }
  }
}
```

### In the compose stack

`docker-compose.yml` runs the `mcp-server` service with `serve --transport http --bind-addr
0.0.0.0:8090` inside the container, publishes it on `${SYLVODE_BIND_HOST}:${SYLVODE_MCP_PORT}`
(default `127.0.0.1:8090`), and mounts the file named by `SYLVODE_MCP_CONFIG_PATH` (default
`config/sylvode.compose.mcp.toml`, written by `scripts/start.sh`) as its configuration. That file
carries only `[logging]` and `[mcp]`. See [docs/install.md](../../docs/install.md).

## Configuration

The server reads one TOML file: `--config PATH`, or `config/sylvode.toml` relative to the working
directory when the option is absent. It uses the `[mcp]` and `[logging]` sections and reads no
environment variables. Every key, its default and its validation are in
[docs/configuration.md](../../docs/configuration.md); `config/sylvode.example.toml` is the annotated
example.

```toml
[mcp]
api_url = "http://localhost:8081"
bot_token = "opr_your_token_here"     # stdio transport and command line only
workspace_id = "your-workspace-uuid"  # stdio transport and command line only
```

| Key | Default | Used for |
|---|---|---|
| `api_url` | `http://localhost:8081` | Base URL of the Sylvode API |
| `bot_token` | none | The identity of the `stdio` transport and of the command line. Required for them, ignored by `http` and `sse` |
| `workspace_id` | none | The workspace that workspace-scoped tools act in on `stdio` and in the command line, where it is required. Not used by `http` and `sse`, where each caller acts in the workspace its own bot token belongs to, nor for delegated callers |
| `transport` | `stdio` | `stdio`, `http` or `sse` |
| `bind_addr` | `127.0.0.1:8090` | Listener of `http` and `sse` |
| `tool_mode` | `search` | What `tools/list` offers; see [Tool modes](#tool-modes) |
| `allow_write_call` | `true` | `false` refuses every write tool; see [Write control](#write-control) |
| `oauth_issuer`, `oauth_resource`, `oauth_forward_secret` | none | Delegated access; all three or none; see [Delegated access](#delegated-access-oauth) |

The global options `--api-url`, `--bot-token` and `--workspace-id`, and `serve --transport`,
`--bind-addr` and `--tools`, override the matching keys. Inbound HTTP and SSE requests authenticate
with the caller's own token, as described under [Authentication](#authentication); there is no
shared inbound secret key.

## Commands

| Command | What it does |
|---|---|
| `mcp-server serve [--transport stdio\|http\|sse] [--bind-addr HOST:PORT] [--tools search\|full]` | Runs the server |
| `mcp-server serve --flow-only ...` | Runs the [Flow-only profile](#flow-only-profile) |
| `mcp-server doctor [--json] [--cli claude\|codex\|opencode ...] [--as-service]` | Checks the prerequisites and prints the `sylvode.doctor.v1` report |
| `mcp-server --check` | Prints a one-row `sylvode.doctor.v1` report (version, task protocol, tool mode); reads no secrets and probes nothing |

`--config`, `--api-url`, `--bot-token`, `--workspace-id` and `--format json|table` are global and
may appear anywhere on the line.

`doctor` reports the configuration state and file mode, tmux, the agent CLIs, whether the server is
registered in each CLI named by `--cli` (`connected`, `registered_failing`, `not_registered`,
`binary_missing`; opencode is always `unverified`), the runner and the API task protocol.
`--as-service` evaluates programs on the `PATH` a service unit would have instead of the shell's.
Exit codes: `0` no row failed, `2` at least one row failed, `1` the checker could not run. The
report contains no token, no URL and no output of the programs it probes, so it can be pasted into
a bug report. `sylvode-runner doctor` runs `mcp-server --check` to compare task protocols.

The workspace commands (`projects`, `work-items`, `tools call` and the others) are part of the
`sylvode` command line, not of `mcp-server`; see
[docs/integrations/cli.md](../../docs/integrations/cli.md).

## Transports and endpoints

| Transport | Endpoints | Typical client |
|---|---|---|
| `stdio` | JSON-RPC on standard input and output | Agent CLIs and desktop MCP clients that start the server themselves |
| `http` | `POST /mcp/rpc` (JSON-RPC), `GET /sse` and `POST /messages` (SSE), `GET /health` | Web integrations, scripts, delegated (OAuth) clients |
| `sse` | `GET /sse`, `POST /messages`, `GET /health` | Streaming clients |

`GET /sse` opens the stream and sends the session's `POST /messages?session_id=...` endpoint as its
first event. With delegated access configured, the `http` transport also serves
`GET /.well-known/oauth-protected-resource` and its path form; see
[Delegated access](#delegated-access-oauth).

### Protocol versions

The server speaks two protocol eras on the same endpoints (`POST /mcp/rpc` and stdio).

* **Legacy.** `initialize` echoes the version the client asks for when it is `2025-11-25`,
  `2025-06-18`, `2025-03-26` or `2024-11-05`, and answers `2025-11-25` otherwise. `ping` and
  `notifications/initialized` work as before. The legacy HTTP+SSE transport (`GET /sse`,
  `POST /messages`) is legacy only.
* **Modern (`2026-07-28`).** There is no handshake. A client may probe with `server/discover`
  (no API call; the result carries `supportedVersions`, `capabilities`, `instructions`, `ttlMs`,
  `cacheScope` and the server identity in `_meta`). A request is modern exactly when
  `params._meta["io.modelcontextprotocol/protocolVersion"]` is present; the HTTP header
  `MCP-Protocol-Version` alone never selects the era. Modern results carry `resultType`,
  `_meta` server info and, for lists and reads, `ttlMs` and `cacheScope` (board data has
  `ttlMs: 0`). A modern `tools/call` also sets `structuredContent` when the first text block is
  a JSON object or array. A modern request naming another version is answered with `-32022`
  (`data.supported`, `data.requested`); `ping` and `initialize` do not exist in this era.

#### Modern requests over HTTP

* A modern request carries `MCP-Protocol-Version` (the `_meta` version), `Mcp-Method` (the method)
  and, for `tools/call`, `resources/read` and `prompts/get`, `Mcp-Name` (the tool name or the
  URI). A value may be written `=?base64?<standard base64>?=`. A missing or different header is
  answered `-32020` with `data.header`, status `400`. The headers are checked after
  authentication; legacy requests are never checked.
* Statuses of a modern request: `-32020`, `-32022`, `-32700`, `-32600` answer `400`; `-32601`
  answers `404`; `-32000` (too many listen streams) answers `429` with `Retry-After: 30`;
  everything else `200`; a notification `202`. Legacy requests keep answering `200`.
* No response carries `Mcp-Session-Id`; `GET /mcp/rpc` answers `405`.
* `Origin`: a request without an `Origin` header is served. A request whose `Origin` is present
  and not listed in `[mcp] allowed_origins` (exact `scheme://host[:port]`, empty by default) is
  answered `403` on every route except `/health` and the well-known metadata, before
  authentication. A request with more than one `Origin` header is refused too.
* A connection that does not send its request head within 10 seconds is closed.

#### Listen streams

`subscriptions/listen` answers on `POST /mcp/rpc` with `text/event-stream` (`Cache-Control:
no-cache`, `X-Accel-Buffering: no`), one `data:` event per JSON-RPC message. The first message is
the acknowledgment (`notifications/subscriptions/acknowledged`, whose `_meta` subscription id is
the request id). This release delivers nothing and the server declares no push capability, so
the acknowledgment carries the empty subset and the final result (`resultType: complete`) follows
at once; the response then ends and holds no stream slot. The request needs `jsonrpc` `"2.0"` and
a string or number id (else `-32600`). A stream that is held open will come with push; its
limits are already in place: 1,000 open streams per credential (the key is a digest of the
bearer token), `[mcp] max_listen_streams` (default 10,000) per process, beyond either the request
is answered `429`; every open stream ends with its final result on SIGTERM or SIGINT. A comment
line every 15 seconds keeps an idle stream alive. `[mcp] listen = false` removes the method
(`-32601`).

Over stdio the acknowledgment is written before anything else is answered; a listen request
runs next to ordinary requests on the same pipe, a second listen
with an id that is still open is answered `-32600`, and `notifications/cancelled` with the
listen's `requestId` ends it without a final result.

## Authentication

| Door | Who the server acts as |
|---|---|
| `stdio` | The bot in `[mcp] bot_token` (or `--bot-token`). The process that started the server is the caller |
| `http`, `sse` | Whoever sent the request. Every request to `/mcp/rpc`, `/sse` and `/messages` carries the caller's own token in `Authorization: Bearer ...` |
| `/health` | Nobody. It answers a constant `OK`, reads nothing and needs no token |

On the networked transports there is no shared secret and no server-side identity to fall back to.
A request without an `Authorization` header, or with a malformed one, is refused with `401` and
`WWW-Authenticate: Bearer` before it reaches a tool. A workspace bot token (`opr_...`) or a user
token is forwarded to the API unchanged, and the API authenticates it and enforces permissions. The
API authenticates `opr_` bot tokens by their SHA-256 token hash, rejects disabled and expired ones,
and lets a bot token act only inside its own workspace; the server acts in that workspace, as
described under [One server, any workspace](#one-server-any-workspace). Because every caller brings
its own credential,
`bind_addr` may be any reachable address; put the server behind a reverse proxy or tunnel with
rate limits when it is exposed publicly.

Tokens that start with `sva_` are delegated OAuth access tokens; see
[Delegated access](#delegated-access-oauth).

```sh
curl -H "Authorization: Bearer opr_your_own_bot_token" \
     -H 'Content-Type: application/json' \
     -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
     http://localhost:8090/mcp/rpc
```

[docs/guide/agents.md](../../docs/guide/agents.md) describes workspace bots and how their tokens
are created.

## One server, any workspace

One `http` or `sse` server serves callers of any number of workspaces. A request acts in the
workspace its own bot token belongs to, which the server reads from `GET /api/v1/auth/me`
(`data.bot.workspace_id`) with the caller's token the first time a tool needs a workspace:
`projects.list`, `projects.create`, `project_types.list`, `members.list`, `bot_operation_logs.list`,
the workspace label tools and the milestone list and create tools. Tools that name an object of their
own (a project, a work item) need no lookup.

- The answer is kept for 60 seconds per token, under the SHA-256 of the token, for at most 1,000
  tokens at once; the oldest entry makes room when the table is full. A failed lookup is not kept.
  The token itself is never stored in the table, logged or echoed in an error.
- A token that belongs to no bot (a user token) has no workspace to take: a call that needs one is
  refused with `401`, and no workspace route is called.
- `[mcp] workspace_id` plays no part on `http` and `sse`, so a deployment leaves it out. It is still
  required by the `stdio` transport and by the command line, which have no per-request token to
  take the workspace from. A delegated caller's workspace is its grant's.
- A token can be limited per project (Members, API tokens, Project access). The API then refuses a
  call the token's level for that project does not allow with code `403` and
  `this bot may not do this in this project`; the tool answers with that text as an error, and a
  retry or another tool does not change it. The milestone tools are refused for such a token, and
  creating, changing or deleting a milestone also needs a workspace owner or admin.

## Tool modes

In **search mode** (the default) `tools/list` offers nine tools:

| Tool | Purpose |
|---|---|
| `sylvode_overview` | The categories and how to call their tools |
| `sylvode_search` | Find a tool by words, category or exact name; the answer carries the tool's input schema |
| `sylvode_read` | Call one registry tool that reads, by name |
| `sylvode_write` | Call one registry tool that changes data, by name |
| `sylvode_task_get`, `sylvode_task_ack`, `sylvode_task_progress`, `sylvode_task_complete`, `sylvode_task_fail` | The agent task protocol, called directly |

The other registry tools are not listed, and calling one by its own name is refused with a pointer
to `sylvode_read` or `sylvode_write`. The wrappers run every check of a direct call (project agent
policy, ownership, admin rules) under the target's name, validate the arguments against the
target's own schema, and the operation log records the target.

In **full mode** every registry tool is listed and callable by its own name, and the four entry
tools are not offered.

| Door | How the mode is chosen |
|---|---|
| `stdio` | `[mcp] tool_mode`; `serve --tools search\|full` overrides it |
| `http` `POST /mcp/rpc` | Per request: `?tools=search\|full` or the `X-Sylvode-Tool-Mode` header; otherwise `[mcp] tool_mode`. An unknown value, or two values that disagree, is a `400` |
| `GET /sse` | When the stream opens (`?tools=...` or the header); the session keeps it |
| `serve --flow-only` | Not applicable; `--tools` is refused |
| Delegated callers | Ignored: they get only the read tools their scopes allow, and no entry or task tools |

`tools/list` with `"params": {"project_id": "..."}`, and `sylvode_search` or `sylvode_overview`
with a `project_id`, filter by the tools the project's agent policy enables. The filter reads the
policy with the caller's own token. The answer shows which tools are enabled but not the policy
itself; without a `project_id` the static catalogue is shown.

A plain caller's `initialize` result carries `instructions` and
`capabilities.experimental["sylvode/task-protocol"] = {"version": 1}`.

## Write control

`[mcp] allow_write_call = false` refuses every tool the catalogue marks as writing with
`writes are disabled for this bot by the runner configuration`, before any API request, whichever
way the tool is reached: through `sylvode_write`, by its own name in full mode, and from the
`sylvode` command line. `sylvode_write` stays listed and answers with the refusal. The five task
tools are the exception: a runner bot needs them, and the API lets a bot act only on its own tasks.
The runner writes this key into the per-bot MCP configuration it generates, from its own
`[runner.mcp] allow_write_call` (default `false`); see [apps/runner/README.md](../runner/README.md).

A tool takes exactly the arguments its schema declares where that matters for authorization: an
ownership-bearing id (`work_item_id`, `form_id`, `record_id`, ...) that the tool does not declare is
refused before any request, because it could change which project's agent policy is evaluated, and
a tool with a closed schema refuses any undeclared argument.

## Delegated access (OAuth)

Sylvode can act as an OAuth 2.1 authorization server for its API and for this MCP server; the
authorization server, its scopes, tokens, consent page and connected-apps management are described
in [docs/integrations/oauth.md](../../docs/integrations/oauth.md). This section covers the MCP side.

A delegated caller presents an access token that starts with `sva_` and was issued for this
server's resource. Delegated access is served on the **`http` transport, on `POST /mcp/rpc`
only**, and is switched on by three `[mcp]` keys set together:

| `[mcp]` key | Value | Must equal, in the API's configuration |
|---|---|---|
| `oauth_issuer` | The authorization server's issuer, for example `https://sylvode.example.com` | `[oauth] issuer` |
| `oauth_resource` | This server as a protected resource: the exact URL that receives the JSON-RPC posts, for example `https://sylvode.example.com/mcp` behind a proxy that maps that path to `/mcp/rpc` | `[oauth] mcp_resource` |
| `oauth_forward_secret` | Internal forwarding credential, at least 32 bytes, never logged | `[oauth] internal_forward_secret` |

A partial set refuses to start and names the missing keys. Without the keys, `sva_` tokens are
refused with `401` and nothing else changes. An `sva_` token on `/sse` or `/messages` is refused
with `403`; the `sse` transport and the Flow-only profile never serve delegated callers.

What the server does for a delegated caller:

- **Discovery.** `GET /.well-known/oauth-protected-resource` and the path form for the configured
  resource (for `https://sylvode.example.com/mcp`: `/.well-known/oauth-protected-resource/mcp`)
  publish the RFC 9728 document without a token. A request without a token gets `401` with
  `WWW-Authenticate: Bearer realm="sylvode", resource_metadata="...", scope="projects:read work_items:read"`.
- **Verification.** Before anything else the server asks the API what the token allows
  (`GET /api/v1/oauth/internal/token-context`, with the forwarding credential). An API `401`
  becomes a `401` with `error="invalid_token"`; unknown, expired, revoked and wrong-audience tokens
  are not told apart. If the API cannot be asked within five seconds or its answer cannot be
  understood, the request is refused with `503`, never served with broader access.
- **Workspace.** The caller acts in the workspace of its grant, taken from the token context; never
  from `[mcp] workspace_id` and never from a tool argument. A `workspace_id` argument that differs
  from the grant is refused. For a grant limited to some projects, a `project_id` argument outside
  the grant is refused before the API is called, with the same answer the API gives for any object
  outside a grant, so the refusal does not reveal whether the project exists.
- **Tools.** `tools/list` shows only the tools the token's scopes allow. A `tools/call` for a tool
  outside them answers `403` with `WWW-Authenticate: Bearer error="insufficient_scope", scope="...",
  resource_metadata="..."` and JSON-RPC error `-32001`. Delegated callers get read tools only:

  | Scope | Tools |
  |---|---|
  | `projects:read` | `projects.list`, `projects.get`, `context.get_project` |
  | `work_items:read` | `work_items.list`, `work_items.get`, `work_items.get_by_identifier`, `work_items.list_labels`, `comments.list` |

  A `:write` scope implies its `:read` scope but unlocks no tool that changes data. Every other
  tool is refused with JSON-RPC error `-32002` (`tool not available to connected applications`)
  whatever the scopes say: anything that writes, deletes, administers, imports or exports, or
  dispatches an agent, and the read tools whose API routes do not accept delegated tokens
  (`work_items.search`, `labels.list_by_project`, the forms and form-record reads). Those stay
  available to bot and user tokens. `resources/read` serves only the static guides.
- **Forwarding.** The server forwards the caller's token to the API together with
  `X-Sylvode-Forwarded-By: mcp-server` and `X-Sylvode-Forward-Auth: <oauth_forward_secret>`. The API
  accepts an MCP-audience token only with that credential, and a token for one audience is still
  refused at the other. This differs from the MCP authorization specification, which says a server
  must not pass through the token it received: the MCP server is a stateless facade over the API,
  holds no signing key and no database, and the API is the issuer and the policy point for both
  audiences. The cost is one shared secret in the API and MCP configurations.
- **Tokens and secrets.** The token is read from the `Authorization` header only, never from a
  query string, a cookie, a tool argument or the environment. Neither the token nor the forwarding
  credential is logged, echoed in an error or a tool result, or written anywhere.

Delegated clients use plain JSON-RPC over `POST /mcp/rpc`. `opr_` bot tokens and user tokens on the
same server are forwarded verbatim, act in the workspace of the bot token (see
[One server, any workspace](#one-server-any-workspace)), and never carry the forwarding headers.

## Flow-only profile

`serve --flow-only` and `sylvode flow` expose a bounded set of Flow tools for one global user,
without a default workspace, a bot token or project-bound bot policy. They act with an existing API
user access token:

```sh
sylvode flow tools
sylvode --api-url https://sylvode.example.com --access-token-file /srv/sylvode/user-token flow call --name flow.home
sylvode --api-url https://sylvode.example.com --access-token-file /srv/sylvode/user-token flow call --name objects.query --args-json '{"personal":true}'
mcp-server --api-url https://sylvode.example.com serve --flow-only --transport stdio --access-token-file /srv/sylvode/user-token
mcp-server --api-url https://sylvode.example.com serve --flow-only --transport http --bind-addr 127.0.0.1:8090
```

| Rule | Behaviour |
|---|---|
| Credential on `stdio` and `sylvode flow call` | Exactly one of `--access-token-file` (preferred) or `--access-token`; at most 8192 ASCII bytes. An `opr_` token is refused, and `[mcp] bot_token` and `--bot-token` are never used as a user credential. The profile does not obtain, refresh or impersonate a credential; expiry is reported by the API |
| Credential on `http` and `sse` | Each request's own `Authorization: Bearer` token. Configuring `--access-token` for a networked transport is refused |
| Configuration file | Optional. Without `--config`, the API URL is `--api-url` or `http://localhost:8081` and the listener is `--bind-addr` or `127.0.0.1:8090`; with one, `[mcp] api_url`, `transport`, `bind_addr` and `allow_write_call` apply |
| `--tools` | Refused; the profile lists its own tools |
| `sylvode flow tools` | Prints the static tool schemas; needs no token and sends no request |

The profile lists `flow.home` (the caller's personal home), the bounded `objects.command` (a set of
named editing, linking, move, archive, field, view and record commands applied to one object), and
Flow registry tools for object read, create, query, search, history and export, Collections and
Records, grants, inheritance and relations, comments and mentions, page shares and appearance,
trash and purge, projection lag, and Forms references. `flow.home` and `objects.command` exist only
in this profile. Package promotion and export, historical restoration, conversion jobs and workspace
administration and maintenance tools are not available here; `objects.export` returns the current
head rendered as JSON, Markdown or CSV, never a typed package.

How calls are scoped:

- List, create and search calls name exactly one of `personal: true`, `scope_id` or `workspace_id`.
  Calls on an object resolve the object's own scope. The personal home is resolved during each call
  and never cached between callers.
- Project claims in arguments must match the real resource; they are never authority. Before a
  mutation the server resolves the real source and target objects (link and move targets, the target
  of the relation being removed, record and relation-field targets) and checks the agent policy of
  every distinct project involved. `objects.command` also requires the policy of each named
  operation it maps to, so it cannot bypass a denied `objects.patch`, `objects.link`,
  `objects.unlink` or `objects.move`, and an atomic embed also requires `objects.create`. A
  client payload cannot choose which policy is checked.
- If an unlink cannot resolve its target, it is refused without sending anything. Relation lookup is
  bounded to 100 pages and target resolution to 1,000 distinct objects; trash and projection-lag
  pages take a `limit` of 1 to 100.
- A Forms reference checks the real destination form or record project. Removing a reference with
  `objects.unreference` needs edit permission on the source only and does not read or change the
  Forms destination, whereas a Flow-to-Flow unlink needs edit permission on both ends.
- The API's own authorization always applies. A personal scope skips only the project policy that
  needs a project, not resource authorization, the tool policy or the operation log.

## Resources

`resources/list` returns five static resources:

| URI | Content |
|---|---|
| `sylvode://skills/sylvode-mcp` | The agent skill: the task procedure, finding and calling tools, conventions and recipes |
| `sylvode://skills/sylvode-mcp/tools` | Every tool with its category, whether it reads or writes, and a one-line summary |
| `sylvode://guides/workflows` | Common workflow recipes: fix an assigned issue, answer a mention, vote on a proposal, find and change data |
| `sylvode://scenario-templates` | Project creation templates with workflow, fields, resources, AI roles and governance |

`resources/templates/list` returns URI templates under the `sylvode://` scheme for project issues,
forms, context, governance, agent policy, release readiness, type, recent decisions and sprints;
forms, their records and events; form records and their events; scenario templates by key; issues
by identifier; and Flow objects, their history and schema, Collection records and the workspace
navigator. `resources/read` reads one with the caller's credential.

A failed read is answered with a JSON-RPC error; `data.uri` is the requested text, unchanged:

| Failure | Modern era | Legacy era |
|---|---|---|
| Unknown URI, or an object the API does not know | `-32602` "Resource not found" | `-32002` "Resource not found" |
| Refused by the project agent policy, by the API (`403`), or for a connected application | `-32602` "Resource access refused", `data.reason` `policy`, `forbidden` or `not_available_to_connected_applications` | the same |
| An id inside the URI is not valid | `-32602` with the validation text | the same |
| The API rejected the credential, or failed | `-32603` | the same |

## Tools

<!-- BEGIN GENERATED: tool-table (sylvode-skillgen; do not edit) -->
The MCP server registers 162 tools in 21 categories. By default it lists nine of them (search mode) and reaches the rest through `sylvode_search`, `sylvode_read` and `sylvode_write`. Every tool, with its category and access, is in [reference/tools.md](../../skills/sylvode-mcp/reference/tools.md).

| Category | Tools | Read | Write | Covers |
|---|---:|---:|---:|---|
| `projects`: Projects | 5 | 2 | 3 | Create, read, update and delete projects |
| `project_types`: Project types | 2 | 2 | 0 | Read the project scenario types and their capability schemas |
| `scenario_templates`: Scenario templates | 3 | 2 | 1 | Browse and install ready-made multi-form setups |
| `context`: Context | 3 | 3 | 0 | Read project context, governance and the agent policy |
| `forms`: Universal forms | 36 | 17 | 19 | Forms, records, attachments, permissions, imports and business events |
| `layouts`: Layouts | 2 | 1 | 1 | Check layouts and propose them as drafts for a person to approve |
| `plugins`: Plugins | 5 | 3 | 2 | Install, inspect and invoke WASM plugins |
| `work_items`: Work items | 14 | 6 | 8 | Issues of a project: list, read, create, update, label |
| `comments`: Comments | 3 | 1 | 2 | Discuss work items |
| `files`: Files | 1 | 0 | 1 | Upload files and get their URLs |
| `labels`: Labels | 5 | 2 | 3 | Workspace and project labels |
| `sprints`: Sprints | 4 | 1 | 3 | Plan iterations of a project |
| `milestones`: Milestones | 5 | 2 | 3 | Workspace milestones that span projects |
| `proposals`: Proposals | 5 | 2 | 3 | Governance proposals and check results |
| `release`: Release | 1 | 1 | 0 | Release readiness gates and blockers |
| `code`: Code | 2 | 1 | 1 | Context and governed proposals for code tasks |
| `documents`: Documents | 3 | 0 | 3 | Governed document summaries, risk reviews and approvals |
| `operations`: Operations | 2 | 0 | 2 | Governed inspection reports and corrective actions |
| `workspace`: Workspace | 3 | 3 | 0 | Members, global search and bot operation records |
| `flow`: Flow | 53 | 20 | 33 | Pages, collections, shares, comments, imports, exports and maintenance of Flow |
| `tasks`: Agent tasks | 5 | 1 | 4 | The protocol of tasks assigned to a bot: get, acknowledge, report progress, complete, fail |

Every tool, one line each, is in [skills/sylvode-mcp/reference/tools.md](../../skills/sylvode-mcp/reference/tools.md).
<!-- END GENERATED: tool-table -->

The Flow tools carry the Flow contracts (`objects.*`, `collections.*`, `records.create`, `collab.*`,
`flow.*`).

To find the exact name and schema of a tool, call `sylvode_search`, read
`sylvode://skills/sylvode-mcp/tools`, or open
[skills/sylvode-mcp/reference/tools.md](../../skills/sylvode-mcp/reference/tools.md).

## Examples

List tools over stdio (nine tools in search mode; add `--tools full` for the whole registry):

```sh
echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | \
  mcp-server serve --transport stdio --config config/sylvode.toml
```

List tools filtered by a project's agent policy:

```sh
echo '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"project_id":"<project-uuid>"}}' | \
  mcp-server serve --transport stdio --config config/sylvode.toml
```

Call a registry tool in search mode through `sylvode_read`:

```sh
echo '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"sylvode_read","arguments":{"tool":"projects.list","arguments":{}}}}' | \
  mcp-server serve --transport stdio --config config/sylvode.toml
```

Call a registry tool by name over HTTP in full mode:

```sh
curl -X POST "http://localhost:8090/mcp/rpc?tools=full" \
  -H "Authorization: Bearer opr_your_own_bot_token" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"work_items.search","arguments":{"query":"bug"}}}'
```

## Security notes

- The server never verifies a token itself and never bypasses Sylvode authorization; it forwards
  the caller's token and the API decides. It accepts no database credentials.
- `stdio` and the `sylvode` command line act as the configured bot, in the configured workspace;
  `http` and `sse` act as the caller of each request, in the workspace of the caller's own bot token.
- JWT user tokens go through the API's normal JWT validation.
- Delegated callers are checked against the API on every request, see only the read tools their
  scopes allow, act in their grant's workspace, and are forwarded with the internal forwarding
  credential.
- `/health` is the only route that answers without a token (with delegated access on, the
  protected resource metadata document is public too).

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| The server refuses to start with `unknown field auth_token` | The `[mcp]` section has an `auth_token` key; the loader accepts no such key. Delete the line; HTTP and SSE callers present their own tokens |
| `mcp.bot_token is required by the stdio transport and by the CLI subcommands...` | `stdio` and the command line have no per-request caller, so they act as a configured bot. Set `bot_token`, `api_url` and `workspace_id` in `[mcp]`. `http` and `sse` never need `bot_token` or `workspace_id` |
| A tool answers `401` and says the credential belongs to no workspace bot | The caller presented a user token on `http` or `sse`; the workspace is taken from a workspace bot token. Present a bot token |
| A tool answers `403 this bot may not do this in this project` | The bot token is limited per project and its level for that project is too low. A workspace owner or admin changes it under Members, API tokens, Project access |
| `mcp.oauth_issuer, mcp.oauth_resource and mcp.oauth_forward_secret are set together or not at all` | Only some of the three keys are set. Set all three or remove them |
| Every request answers `401` | The request has no `Authorization: Bearer` header, or the API rejected the token |
| Every delegated call answers `401 invalid_token` | `oauth_forward_secret` differs from the API's `[oauth] internal_forward_secret`, or `oauth_resource` differs from `[oauth] mcp_resource` |
| Delegated calls answer `503` | The MCP server could not reach the API's token-context endpoint |
| Connection refused when calling a tool | The server talks to the Sylvode API, not to PostgreSQL. Check that the API answers `curl http://localhost:8081/health` and that `[mcp] api_url` points at it |
| A tool name is refused in search mode | Call it through `sylvode_read` or `sylvode_write`, or switch to full mode |
| `writes are disabled for this bot by the runner configuration` | `[mcp] allow_write_call = false`; see [Write control](#write-control) |

## License

MIT OR Apache-2.0, as the rest of the repository
([LICENSE-MIT](../../LICENSE-MIT), [LICENSE-APACHE](../../LICENSE-APACHE)).
