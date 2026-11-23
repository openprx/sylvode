# The `sylvode` command line

`sylvode` runs Sylvode operations from a shell or a script. It talks to the Sylvode API over HTTP
and never starts a server. It ships in the release archives next to `mcp-server` (see
[install.md](../install.md)) and is built from the `mcp-server` package.

It carries two command models, chosen by the first command name on the line:

| Model | Groups | Output | Exit codes |
|---|---|---|---|
| Workspace commands | `projects`, `work-items`, `comments`, `labels`, `sprints`, `search`, `files`, `operation-logs`, `tools` | The tool result as JSON (or a table) | [Workspace exit codes](#workspace-exit-codes) |
| Flow commands | `flow`, `features`, `objects`, `collections`, `records`, `collab`, `deliveries` | The `sylvode.cli.v1` envelope (or a table) | [Flow exit codes](#flow-exit-codes) |

`sylvode --help` lists every group; `sylvode <group> --help` and `sylvode help <group>` show the
options of one group.

Every workspace command and most Flow commands run one MCP tool through the same code the MCP
server uses, so tool permissions, the project agent policy and the operation log apply exactly as
they do over MCP. The MCP server and its tools are described in
[apps/mcp-server/README.md](../../apps/mcp-server/README.md).

## Configuration

`sylvode` reads the same configuration file as `mcp-server`: `--config PATH`, or
`config/sylvode.toml` relative to the working directory. It uses the `[mcp]` section; every key is
in [configuration.md](../configuration.md).

```toml
[mcp]
api_url = "https://sylvode.example.com"
bot_token = "opr_your_token_here"
workspace_id = "your-workspace-uuid"
allow_write_call = true
```

`sylvode` is a local process with no caller of its own, so the workspace commands and the Flow
commands (except `flow`) always act as the bot in `bot_token` and need one, together with
`workspace_id`. `allow_write_call = false` refuses every write tool here as it does on the MCP
server.

Global options, accepted anywhere on the line:

| Option | Workspace commands | Flow commands | Meaning |
|---|---|---|---|
| `--config PATH` | yes | yes | Configuration file |
| `--format json\|table` | yes | yes | Output format; `json` is the default |
| `--api-url URL` | yes | yes | Overrides `[mcp] api_url` |
| `--bot-token TOKEN` | yes | yes, not with `flow call` | Overrides `[mcp] bot_token`; must start with `opr_` |
| `--workspace-id UUID` | yes | no | Overrides `[mcp] workspace_id` |
| `--access-token TOKEN`, `--access-token-file PATH` | no | `flow call` only | The user credential of the [Flow-only profile](#flow-only-profile) |

A bot token on the command line can be visible in process listings and shell history; prefer the
configuration file, readable by you only.

## Workspace commands

| Command | Tool it calls |
|---|---|
| `projects list` | `projects.list` |
| `projects get <ID>` | `projects.get` |
| `projects create --key KEY --name NAME [--description TEXT]` | `projects.create` |
| `work-items list --project ID [--state KEY]` | `work_items.list` |
| `work-items get <ID or IDENTIFIER>` | `work_items.get` for a UUID, `work_items.get_by_identifier` for an identifier such as `PRJ-42` |
| `work-items create --project ID --title TEXT [--state KEY] [--priority P] [--description TEXT]` | `work_items.create`; priority defaults to `medium`, state to the workflow's initial state |
| `work-items search --query TEXT` | `work_items.search` |
| `work-items update <ID> [--state KEY] [--priority P] [--title TEXT]` | `work_items.update` |
| `comments list --work-item ID` | `comments.list` |
| `comments create --work-item ID --content TEXT` | `comments.create` |
| `labels list [--project ID]` | `labels.list`, or `labels.list_by_project` with `--project` |
| `sprints list --project ID` | `sprints.list` |
| `search <QUERY>` | `search.all` |
| `files upload --file PATH` | `files.upload` (reads the file and sends it base64-encoded) |
| `operation-logs list [--bot-id ID] [--tool-name NAME] [--outcome O] [--cursor C] [--limit N]` | `bot_operation_logs.list`; `--limit` defaults to 50 |
| `tools call --name NAME [--args-json JSON]` | Any tool, see below |

```sh
sylvode --config /srv/sylvode/mcp/config.toml projects list
sylvode work-items create --project <project-uuid> --title "Fix login redirect" --priority high
sylvode --format table work-items list --project <project-uuid> --state todo
```

### `tools call`

`tools call` runs any tool of the MCP registry by its name, with `--args-json` as its arguments (a
JSON object, `{}` by default). The registry is used in full mode, so every tool is reachable by its
own name; the entry tools of search mode are not needed here. Calls go through the same checks and
the same operation log as an MCP call by the same bot, and `allow_write_call = false` refuses write
tools.

```sh
sylvode tools call --name forms.list --args-json '{"project_id":"<project-uuid>"}'
sylvode tools call --name sylvode_task_get --args-json '{"task_id":"<task-uuid>","attempt":1}'
```

The tool names and their arguments are in
[skills/sylvode-mcp/reference/tools.md](../../skills/sylvode-mcp/reference/tools.md).

### Workspace exit codes

| Code | Meaning |
|---|---|
| `0` | The tool succeeded; its result is on stdout |
| `1` | The tool failed (its message is on stderr), the configuration could not be used, or the result could not be written to stdout |
| `2` | The command line could not be parsed |

## Flow commands

Flow commands take the workspace, project or object they act on as arguments (`--workspace`,
`--project`, object ids). Commands that change something require `--idempotency-key`, so a retry
with the same key does not apply the change twice. Commands that take a file (`--patch-file`,
`--schema-file`, `--values-file`, `--query-file`, `--mapping`, `--package`, `--display-file`) read
JSON or a package from disk.

| Group | Subcommands |
|---|---|
| `features flow` | `get --workspace ID`; `set --workspace ID [--enabled BOOL] [--default-member-level LEVEL] --idempotency-key K` (workspace admin) |
| `objects` | `get`, `query`, `search`, `history`, `diff`, `relations`, `create`, `patch`, `move`, `link`, `unlink`, `reference`, `unreference`, `export`, `grants get`, `grants set`, `inheritance set`, `convert-preview`, `convert-commit`, `convert-status`, `convert-retry` |
| `collections` | `describe <ID>`, `query <ID> --query-file PATH [--cursor C]` |
| `records` | `create --collection ID --values-file PATH`, `patch <ID> --values-file PATH` |
| `collab` | `status`, `inspect`, `verify`, `projection-lag`, `compact`, `rebuild-projection`, `export`, `export-workspace`, `import-preview`, `import-commit`, `import-status` |
| `deliveries` | `replay --workspace ID --mode rebuild\|requeue-failed --from TIME --to TIME` |
| `flow` | `tools`, `call`; see [Flow-only profile](#flow-only-profile) |

Notes on some subcommands:

- `objects get <ID> [--at-seq N] [--render semantic-json|markdown]` reads one object;
  `objects query --workspace ID [--project ID | --unprojected]` lists objects;
  `objects search --workspace ID --query TEXT [--freshness allow-stale|require-current]` searches
  accepted Flow projections.
- `objects create --workspace ID --type page|navigator|collection --title TEXT` creates an object;
  `--embed-page ID [--after-block ID]` embeds a new Collection in a page.
- `objects export <ID> --render json|markdown|csv|package [--wait]` creates an export job.
- `objects move`, `objects grants set` and `objects inheritance set` accept
  `--confirm-self-lockout` to confirm a change that removes the caller's own access; `grants set`
  and `inheritance set` also accept `--dry-run`. `grants set` takes one `--grant KIND:ID=LEVEL` per
  grant.
- `objects convert-preview <SOURCE> --frontier F --to form|form-record --mapping PATH` previews a
  Flow-to-Forms conversion; `convert-commit --preview ID ... --confirm` commits it.
- `collab verify <ID> [--deep] [--expected-head N]` checks collaboration integrity of one object;
  it exits `10` when it finds an inconsistency.
- `collab compact <ID>` and `collab rebuild-projection <ID>` take exactly one of `--dry-run` or
  `--execute`, plus `--expected-head N`; `--execute` also needs `--confirm <ID>` naming the same
  object.
- `deliveries replay` takes `--dry-run` or `--execute` and always needs `--confirm`;
  `objects convert-commit`, `objects convert-retry` and `collab import-commit` need `--confirm` as
  well.
- `collab import-commit --conflict-policy reject-existing|reuse-import-lineage --confirm` commits an
  import previewed with `collab import-preview`.

Run `sylvode <group> <subcommand> --help` for every option.

### Output

With `--format json` (the default) every Flow command prints one JSON envelope on stdout, on
success and on failure:

```json
{"schema_version":"sylvode.cli.v1","ok":true,"command":"objects.get","data":{},"request_id":"<uuid>"}
```

```json
{"schema_version":"sylvode.cli.v1","ok":false,"command":"objects.get","error":{"code":"not_found","message":"...","recoverable":false,"details":null},"request_id":"<uuid>"}
```

| Field | Meaning |
|---|---|
| `schema_version` | Always `sylvode.cli.v1` |
| `ok` | `true` on success |
| `command` | The command as dotted words, for example `objects.grants.set` or `flow.call.objects.query` |
| `data` | The result, on success |
| `error.code` | A stable error code such as `usage_error`, `network_error`, `unauthenticated`, `forbidden`, `not_found`, `stale_frontier`, `invalid_update`, `limit_exceeded`, `server_draining`, `server_rejected`, `integrity_mismatch` |
| `error.recoverable` | Whether retrying the same request can succeed |
| `request_id` | A UUID generated for this invocation |

`--format table` prints `data` as a table on stdout, or one `Error [code]: message` line on stderr.
The table layout is for people and may change; scripts read the JSON.

### Flow exit codes

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | An unclassified failure, or the result could not be written to stdout |
| `2` | A local problem with arguments, JSON, a file or the configuration; nothing was sent |
| `3` | Not authenticated |
| `4` | Forbidden, the feature is disabled, or the policy refused |
| `5` | Not found |
| `6` | Conflict: a stale frontier or a resync is required |
| `7` | The request is invalid (invalid update, unsupported protocol or format, checksum mismatch) |
| `8` | A limit was exceeded |
| `9` | Temporary: the server is draining or busy, authorization changed during the request, or the network failed; retrying is safe |
| `10` | `collab verify` completed and found an inconsistency |
| `11` | The server refused permanently; retrying will not help |

## Flow-only profile

`sylvode flow tools` prints the static schemas of the Flow-only tools without a token or a request.
`sylvode flow call --name NAME [--args-json JSON]` calls one of them as a global user, with that
user's API access token from `--access-token-file` (preferred) or `--access-token`, never with a
bot token:

```sh
sylvode flow tools
sylvode --api-url https://sylvode.example.com --access-token-file /srv/sylvode/user-token flow call --name flow.home
sylvode --api-url https://sylvode.example.com --access-token-file /srv/sylvode/user-token \
  flow call --name objects.query --args-json '{"personal":true}'
```

The configuration file is optional for `flow call`; without `--config` and `--api-url` it uses
`http://127.0.0.1:3000`. The tools, scoping rules and the matching MCP server mode
(`mcp-server serve --flow-only`) are described in
[apps/mcp-server/README.md, Flow-only profile](../../apps/mcp-server/README.md#flow-only-profile).
