# Changelog

All notable changes to Sylvode (formerly OpenPR) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html). Every commit bumps the
patch version, so the entries below are grouped by published release tags rather than by
individual patch versions. Releases up to and including 0.2.21 were published under the OpenPR
name.

## [Unreleased]

This release renames the product from OpenPR to Sylvode and adds Sylvode Flow. Read
**Breaking and behaviour changes** before upgrading, then follow the upgrade sequence in the
[v1.0 compatibility matrix](docs/sylvode-v1.0-compatibility.md#upgrade-sequence). Every legacy
OpenPR name keeps working; see **Deprecated**.

### Breaking and behaviour changes

- **Outbound webhook `User-Agent` changed** from `OpenPR-Webhook/1.0` to
  `Sylvode-Webhook/1.0 (compatible; OpenPR-Webhook/1.0)`. A `User-Agent` cannot be sent twice,
  so receivers that match the old value **exactly** or as a **prefix** no longer match and must
  be updated. Receivers that match `OpenPR-Webhook/1.0` as a substring are unaffected. The
  `request_headers` recorded with each delivery carry the same value that was sent. The payload
  and its `X-Webhook-Signature` are unchanged.
- **Plugin fuel accounting changed with wasmtime 49.** Wasmtime 49 charges fuel per byte or
  element for bulk memory and table operations (`memory.copy`, `memory.fill`, `memory.init`,
  `table.copy` and similar) and per page for `memory.grow`, which wasmtime 47 charged as single
  instructions. A plugin that moves large buffers or grows its memory
  with a tight `runtime.fuel` budget may now run out of fuel; raise the budget in its manifest
  (the maximum is 1,000,000,000). The wide-arithmetic proposal, which wasmtime 49 enables by
  default, is explicitly disabled, so the set of accepted modules is unchanged.
- **Plugin deadlines now stop the guest.** A plugin invocation that exceeds `runtime.timeout_ms`
  is interrupted at its deadline; previously the caller received the timeout error while the
  guest kept running on a blocking thread until its fuel ran out. Cancelling the request or
  shutting the API down now also stops the guest. Such invocations are recorded with status
  `timeout` instead of `failed`. Every failed invocation, whether timed out, out of fuel or
  trapped, now records its real `duration_ms` and `fuel_consumed` (previously `0` and `NULL`)
  in `plugin_invocations`, in the API response and in the `plugin.invoked` event. Guest trap
  messages include the full error chain, so fuel exhaustion is named.
- **Conflicting MCP attribution headers are rejected.** The API accepts
  `X-Sylvode-MCP-Surface` / `X-Sylvode-MCP-Tool` and the legacy `X-OpenPR-MCP-*` headers. If the
  canonical and legacy header of a field, or repeated occurrences of either, carry different
  values, the request is refused with envelope code 401 instead of one value being picked,
  because these values are recorded as audit attribution. A client that sends only one set, or
  both with equal values, is unaffected.
- **Two configuration files are an error.** The default configuration file is now
  `config/sylvode.toml`. `config/openpr.toml` is still discovered when the new file is absent,
  but if both exist the binaries refuse to start until `--config` selects one. `scripts/start.sh`
  applies the same rule to `config/sylvode.compose*.toml` and `config/openpr.compose*.toml`, and
  to a canonical and a legacy compose variable that are both set with different values.
- **Auth cookies always carry `Secure`.** Browsers do not send `Secure` cookies over plain HTTP
  except to `localhost`, so a deployment reached over plain HTTP at any other address needs
  HTTPS. `auth.allow_insecure_cookies = true` drops the attribute for local development and is
  refused at startup unless `server.bind_addr` is a loopback address.
- **Standalone MCP server archives are no longer published.** The `openpr-mcp-server-<target>`
  release archives are gone. The `mcp-server` binary ships in every `sylvode-<target>` archive
  (and in its `openpr-<target>` alias) together with `api`, `worker` and `sylvode`.
- **CLI exit codes on output errors.** `mcp-server` and `sylvode` no longer panic (exit 101) when
  stdout or stderr cannot be written: a failed command still exits 1, a successful command whose
  result cannot be written exits 1 with a line on stderr, and the Flow commands keep their own
  exit code. `list-tools` exits 1 when its listing cannot be written for any reason other than a
  closed pipe.
- **Messages name the Sylvode API.** MCP client refusals (rejected or missing credential) and
  structured Flow tool errors say "Sylvode API" instead of "OpenPR API". Help, log and error texts
  are not a compatibility contract; match on codes, not on message text.
- **Deprecation warnings on stderr and in logs.** Legacy entry points now announce themselves
  (see **Deprecated**). The warnings never change stdout or the exit code, but a wrapper that
  treats any stderr output as failure will notice them.

### Added

- **Sylvode Flow**: collaborative pages, objects, Collections and records inside a workspace.
  Flow is off by default and is enabled per workspace through
  `PUT /api/v1/workspaces/{id}/features/flow`, the MCP tool `flow.feature_set` or
  `sylvode features flow set`.
  - Objects with history, diff, schema, relations, grants and inheritance, cross-project moves
    of whole subtrees, a workspace and project navigator, permission-filtered search and
    projection lag reporting.
  - Collections and Collection records with typed queries and field-level secrecy.
  - Live collaborative editing over WebSocket on the Loro CRDT engine (`crates/collab-core`):
    sessions are opened with short-lived tickets bound to an allowed origin, resume from the
    last acknowledged sequence, open read-only for readers and close when access is revoked.
  - Conversion between Flow objects and universal-form records (preview, commit, status, retry).
  - Export and import packages for one object or a whole workspace; an import is previewed
    first and then committed as a whole.
  - Import of legacy pages through the `legacy_pages.*` MCP tools.
  - Administrative maintenance: document compaction and verification, projection rebuilds,
    quarantine repair, delivery replay, and workspace health, integrity and lag reports.
  - Web UI: a Flow navigator and an object canvas under `/workspace/{workspaceId}/flow`.
  - Migrations `0054` to `0069`.
- **`sylvode` CLI**, a new binary in every release archive with fifteen command groups: the Flow
  commands `features`, `objects`, `collections`, `records`, `collab` and `deliveries`, which
  follow the `sylvode.cli.v1` JSON contract, and the workspace commands `projects`, `work-items`,
  `comments`, `labels`, `sprints`, `search`, `files`, `operation-logs` and `tools`, which are the
  same commands `mcp-server` runs, with byte-identical stdout and the same exit codes.
- **MCP**: 42 new tools for Flow (140 in total; the names and schemas of the existing tools are
  unchanged), `sylvode://` URIs for every resource, and Flow resource templates for objects,
  object history and schema, Collection records and the workspace navigator.
- **Protocol headers**: the MCP client sends `X-Sylvode-MCP-Surface` and `X-Sylvode-MCP-Tool`
  alongside the legacy headers, and attachment package downloads return
  `X-Sylvode-Attachment-Count` and `X-Sylvode-Attachment-File-Count` alongside the legacy
  headers, with identical values.
- **Configuration**: a `[flow]` section with `dispatch_max_attempts` (default `10`) and
  `collab_allowed_origins` (default empty, which refuses every collaboration ticket; list the
  web UI's origin, for example `"https://sylvode.example.com"`, to enable live editing), and
  `auth.allow_insecure_cookies` (see above). `config/sylvode.example.toml` is the annotated
  reference.
- **Release archives** named `sylvode-<target>` and `sylvode-frontend.tar.gz`, with
  `openpr-<target>` aliases that contain the same binaries.

### Changed

- The product is now called **Sylvode**. The web UI, documentation, MCP server identity, help
  texts and release names use the new name. The canonical names are `config/sylvode.toml`,
  `config/sylvode.compose.toml`, `config/sylvode.compose.mcp.toml`, the `SYLVODE_*` compose
  variables, `sylvode://` resource URIs and `sylvode-<target>` archives. The GitHub repository
  moves to `openprx/sylvode`. These identifiers are deliberately **not** renamed and have no
  removal planned: REST `/api/v1` and its schema identifiers, MCP tool names, database names,
  roles, tables, columns and migrations, the plugin ABI (`openpr_alloc`, `openpr_invoke`,
  `openpr_plugin_abi_version`, `openpr-plugin-v1.wit`), the bot token prefix `opr_`, the compose
  service labels and network names, and `OPENPR_TEST_DATABASE_URL`.
- `--config` help shows `[default: config/sylvode.toml; legacy config/openpr.toml fallback]`
  without Markdown escapes.
- When `logging.output = "stdout"` is overridden because stdout carries the command's result or
  the MCP stdio protocol, `mcp-server` and `sylvode` log the notice under their own target, so
  the default filter shows it.
- The optional `webhook` compose service now defaults to the canonical Sylvode Webhook names:
  image `ghcr.io/openprx/sylvode-webhook:latest`, executable `/app/sylvode-webhook`, and the
  configuration mounted at `/etc/sylvode-webhook/config.toml`. `SYLVODE_WEBHOOK_IMAGE` and
  `SYLVODE_WEBHOOK_CONFIG` still override them, and `scripts/start.sh` still maps
  `OPENPR_WEBHOOK_IMAGE` / `OPENPR_WEBHOOK_CONFIG` onto them. The canonical executable ships in
  Sylvode Webhook images after 0.3.3; an image pinned to 0.3.3 or earlier needs its own
  `command: ["/app/openpr-webhook", "/etc/sylvode-webhook/config.toml"]`.
- `scripts/bootstrap-restaurant-demo.sh` reads its inputs as `SYLVODE_API_URL`,
  `SYLVODE_DEMO_*` and `SYLVODE_MCP_BOT_TOKEN`. The `OPENPR_*` names still work with one
  deprecation notice each; setting both names of one input to different values is an error. The
  default demo user name is now `Sylvode Demo`.
- The Universal Forms delivery reports, dashboards and their checkers say Sylvode instead of
  OpenPR in their headings.

### Deprecated

Each legacy entry point below keeps working, with the same behaviour as its replacement,
throughout the 1.x series. **None of them is removed before Sylvode v2.0**, and a removal
additionally requires its own accepted decision record. Every warning names the legacy name, its
replacement and the earliest removal.

| Legacy | Replacement | Where the warning appears |
| --- | --- | --- |
| `mcp-server <command>` for any command except `serve` | `sylvode <command>` | One line on stderr per process. `mcp-server serve` does not warn on any transport. |
| Default discovery of `config/openpr.toml` | `config/sylvode.toml` | One `warn` log line per process from `api`, `worker`, `mcp-server` and `sylvode` (on stderr for `mcp-server` and `sylvode`). An explicit `--config config/openpr.toml` does not warn. |
| `config/openpr.compose.toml` and `config/openpr.compose.mcp.toml` | `config/sylvode.compose.toml` and `config/sylvode.compose.mcp.toml` | One line on stderr per file from `scripts/start.sh` when it selects the legacy file. |
| Compose variables `OPENPR_BIND_HOST`, `OPENPR_API_PORT`, `OPENPR_FRONTEND_PORT`, `MCP_SERVER_PORT`, `OPENPR_RUNTIME_BASE`, `OPENPR_FRONTEND_DOCKERFILE` and `OPENPR_WEBHOOK_*` | `SYLVODE_BIND_HOST`, `SYLVODE_API_PORT`, `SYLVODE_FRONTEND_PORT`, `SYLVODE_MCP_PORT`, `SYLVODE_RUNTIME_BASE`, `SYLVODE_FRONTEND_DOCKERFILE` and `SYLVODE_WEBHOOK_*` | One line on stderr per variable from `scripts/start.sh`. |
| `openpr://` MCP resource URIs | The corresponding `sylvode://` URI | `_meta.deprecation = {"replaced_by": "<sylvode:// URI>", "earliest_removal": "2.0"}` in the `resources/read` result, next to `_meta.canonical_uri`. The resource body is unchanged and list endpoints return only `sylvode://` URIs. |
| `openpr-<target>` release archive aliases | `sylvode-<target>` | Release notes only. |
| `X-OpenPR-MCP-Surface` and `X-OpenPR-MCP-Tool` request headers | `X-Sylvode-MCP-Surface` and `X-Sylvode-MCP-Tool` | None (protocol headers). The MCP client sends both sets so that a new `mcp-server` attributes correctly against an older API. |
| `X-OpenPR-Attachment-Count` and `X-OpenPR-Attachment-File-Count` response headers | `X-Sylvode-Attachment-Count` and `X-Sylvode-Attachment-File-Count` | None (protocol headers). Both sets are always returned. |
| The `OpenPR-Webhook/1.0` token in the webhook `User-Agent` comment section | `Sylvode-Webhook/1.0` | None. |

### Security

- **Wasmtime upgraded from 47.0.4 to 49.0.2** for RUSTSEC-2026-0315, RUSTSEC-2026-0316,
  RUSTSEC-2026-0325, RUSTSEC-2026-0326 and RUSTSEC-2026-0327. Exposure of earlier builds:
  - Every release from 0.2.2 through 0.2.21, and source builds from commit `3159995` (the
    upgrade to wasmtime 47) up to 0.2.334, were exposed to **RUSTSEC-2026-0315** (`call_ref` and
    exception `catch` can drop fuel accounting, so a plugin can consume far more CPU than its
    fuel budget) and **RUSTSEC-2026-0326** (missing GC rooting across `try_call`, which can
    corrupt the GC heap). Both are reachable by anyone allowed to install a plugin, which is any
    workspace member and any bot token with write permission.
  - RUSTSEC-2026-0316 and RUSTSEC-2026-0327 affect only the WebAssembly component model, which
    Sylvode does not use: it loads core modules only.
  - RUSTSEC-2026-0325 requires a module that imports a tag; the plugin runtime instantiates
    modules with no imports, so such a module is refused.
  - Releases before 0.2.2 did not contain the plugin runtime.
- **rustls upgraded from 0.23.40 to 0.23.45** for RUSTSEC-2026-0285 (TLS 1.3 handshake messages
  accepted across encryption level boundaries). Every release up to and including 0.2.21, and
  builds up to 0.2.324, used an affected version for their TLS client connections (outbound HTTP, and PostgreSQL when TLS is used).
- **Six cross-tenant reads and writes closed.** Draft, open, voting and rejected proposals were
  visible to any authenticated user of the instance, the project context's recent decisions had
  no tenant filter, and issue links, bot label batches, sprint assignment and bot mentions did not
  check the tenant of the id being attached. Empty scopes and missing tenant columns now fail
  closed, and a resource of another tenant is indistinguishable from an absent one. These were
  present in 0.2.21.
- Request tracing spans no longer contain query strings, and the reference Caddy and nginx
  configurations no longer log collaboration tickets or signed download signatures.
- Auth cookies carry `Secure` (see **Breaking and behaviour changes**).

### Removed

- The `openpr-mcp-server-<target>` release archives (see **Breaking and behaviour changes**).

### Fixed

- `projects create` under `mcp-server` and `sylvode` failed with ``missing field `key` ``
  because it offered no way to pass the required project key; it now takes `--key`.
- `mcp-server` and `sylvode` no longer panic when stdout or stderr cannot be written.
- `--config`, `--transport` and `--bind-addr` help printed backslash-escaped brackets.
- The notice that `logging.output = "stdout"` was overridden was logged under a target the
  default filter drops, so it was never shown.
- Failed plugin invocations recorded `duration_ms = 0` and no fuel, and a deadline expiry was
  recorded as `failed`.
- Bot tokens created on the Members page could only be used with the REST API: the form sent
  no transport surface, so the API bound every token to `rest` and refused it with 401 through
  MCP (HTTP, SSE, stdio) and the command-line tools. The form now requires choosing where the
  token will be used, the token list shows each token's surface, and the token-reveal dialog
  says where the token works.
- The Webhooks page offered only `issue.created`, `issue.updated` and `comment.created`,
  although the API accepts 14 events. The form now offers all of them, grouped by work items,
  comments, labels, sprints and AI tasks, with translated labels.
- `scripts/start.sh --check-config` (and every `start.sh` run) refused valid configuration
  files that use `[flow]`, `[audit]` or `auth.allow_insecure_cookies`. Its key table now lives in
  `scripts/lib/sylvode_config_schema.json`, which a test holds equal to the keys the binaries
  accept.
- A compose deployment generated by `scripts/start.sh` could not edit collaboratively: the
  generated file set no `[flow] collab_allowed_origins`, and the empty default refuses every live
  editing session. Newly generated files list the origins the frontend is published at, derived
  from `SYLVODE_BIND_HOST` and `SYLVODE_FRONTEND_PORT`, with a comment to replace them with the
  public origin. Existing files are not rewritten; `--check-config` now warns when the list is
  empty and rejects entries that are not `scheme://host[:port]`.
- `sylvode` Flow commands reported an unreachable API as `Request failed: error sending request
  for url (...)` under the code `server_draining`, which reads like a server drain and hides the
  cause. The message now says it is a network failure, names the API URL and the cause
  (connection refused, DNS, timeout) and that the command can be retried. The envelope is
  unchanged: exit 9, `recoverable: true`, code `server_draining` with empty `details`, because
  the CLI contract defines no code or reason of its own for a network failure.
- `work-items create` (under `mcp-server` and `sylvode`) always sent `state=backlog`. For a
  project whose workflow has no `backlog` state the create failed with `state must be one of:
  ...`, and for one whose workflow starts at another state the item landed in `backlog`. The
  command now sends no state unless `--state` is given, so the workflow's initial state applies,
  as with the `work_items.create` tool.
- `work-items create/update --priority` offered `none`, which the API rejects; the accepted values
  are now exactly the tool's (`low`, `medium`, `high`, `urgent`) and anything else is refused
  before a request is sent.
- `sylvode <group> --help` described `--config` and `--bot-token` in terms of a `sylvode serve`
  command that does not exist.
- `scripts/bootstrap-restaurant-demo.sh` refused `http://[::1]:<port>` as a non-local API URL.
- The documentation index listed four `docs/prd/` files that are not in the repository, and
  `docs/universal-forms-and-plugins.md` still described connectors, connector suggestions and the
  event outbox, all removed in 0.2.21. The documented `bun --cwd frontend run ...` commands only
  printed bun's usage; they now read `bun run --cwd frontend ...`.
- The production readiness audit checked for a `localhost:3000` API default in
  `apps/mcp-server/src/main.rs`, which no longer holds that code, so the check could not fail. It
  now checks `DEFAULT_MCP_API_URL` and `apps/mcp-server/src/cli.rs`, and every audit's
  `not_contains` fails when the file it inspects does not exist.

## [0.2.21] - 2026-08-19

### Added

- Bot operation records: a metadata-only record of every bot call (migration `0051`) and the MCP
  tool `bot_operation_logs.list`, with a retention period set by
  `[audit] operation_log_retention_days`.

### Removed

- Connectors and agent invocations, including the eight `connectors.*` MCP tools and the
  `[connectors.secrets]` configuration section; their tables are dropped by migration `0052`.
- The write-only event outbox (migration `0053`).

### Fixed

- The frontend API proxy passes WebSocket upgrades.
- The system default workflow's states open read-only.
- AI task dispatch requests are signed.
- Migrations `0048` and `0049` are guarded against the relations that `0052` and `0053` drop.

## [0.2.8] - 2026-08-16

### Changed

- Release build fix only: the release target is installed into the pinned toolchain. No
  user-visible change.

## [0.2.7] - 2026-08-16

### Changed

- The Rust toolchain is pinned in `rust-toolchain.toml`, the workspace builds with no clippy
  warnings, and numeric casts were reviewed at each site. No user-visible change.

## [0.2.2] - 2026-08-15

Version 0.2.1 was not tagged; its changes first shipped in 0.2.2. 0.2.1 is a breaking release:
an existing deployment will not start without configuration changes, and several operations that
previously succeeded now return `403`. Follow [docs/upgrade-0.2.1.md](docs/upgrade-0.2.1.md)
before upgrading from 0.1.x.

### Breaking changes

- **All configuration moved from environment variables to a TOML file.** `api`, `worker` and
  `mcp-server` read no environment variables at all. The file is named with `--config <PATH>`,
  defaulting to `config/openpr.toml` relative to the working directory (`config/sylvode.toml`
  since the Sylvode rename); sections are `[server]`, `[database]`, `[auth]`, `[logging]`,
  `[storage]`, `[migrations]`, `[outbound]`, `[mcp]` and `[connectors.secrets]` (the last was
  removed in 0.2.21). `DATABASE_URL`, `JWT_SECRET`, `RUST_LOG`, `BIND_ADDR`, `APP_NAME`,
  `OPENPR_OBJECT_STORAGE_*`, `OPENPR_MIGRATIONS_*`, `OPENPR_OUTBOUND_*`, `OPENPR_API_URL`,
  `OPENPR_BOT_TOKEN`, `OPENPR_WORKSPACE_ID`, `OPENPR_MCP_TRANSPORT` and `OPENPR_INVOCATION_ID` are
  inert. Unknown keys, placeholders and unexpanded `${...}` templates are startup errors, and
  validation reports every unusable value at once. `[database]` and `[auth]` are validated
  lazily, so an MCP-only deployment can omit both sections. Compose mounts two generated files,
  one for api and worker and one for mcp-server; `scripts/start.sh` generates and validates both.
- **The MCP shared inbound secret `mcp.auth_token` was removed.** A configuration file still
  carrying the key is refused at startup with an explanation. Under the `http` and `sse`
  transports each caller presents its own MCP-type account token in
  `Authorization: Bearer opr_...`, which the server forwards to the API unchanged and never
  verifies itself; a request without one is answered `401`, and only `/health` is exempt.
  `stdio` and the CLI subcommands act as `mcp.bot_token`. A calling bot outside the workspace
  named by `mcp.workspace_id` is refused `403`.
- **Bot token permissions are enforced.** `workspace_bots.permissions` was previously stored and
  ignored; safe methods now require `read`, everything else requires `write`, and `admin` implies
  both. Because the column defaults to `["read"]`, every pre-existing bot becomes read-only on
  upgrade and has to be granted write explicitly. Issuance rejects an empty permission array and
  unknown or wrongly cased names.
- **Connector credentials are referenced by bare name.** The retired
  `env:OPENPR_CONNECTOR_SECRET_W_<uuid>_<NAME>` form is refused with an explanation. Values are
  filed per workspace under `[connectors.secrets."<workspace-uuid>"]`, and a lookup is confined
  to the owning workspace, so no reference can reach another tenant's credentials. Stored
  `auth_policy.secret_ref` values must be rewritten by hand.
- **Outbound requests are SSRF-filtered.** Connector and webhook targets resolving to loopback,
  private, link-local or CGNAT addresses are rejected on create and update (`400`) and again at
  delivery time unless the host is listed in `[outbound] allowed_hosts`. Compose service names
  must be listed. Creating a webhook now also requires the workspace `owner` or `admin` role,
  matching update and delete.
- **Migrations are tracked in a ledger and a failure aborts startup.** The new
  `schema_migrations` table records each migration as `applied`, `adopted` or `failed`, and an
  applied migration is no longer replayed on every start. A failure used to be downgraded to a
  warning; it now stops the service, with `[migrations] continue_on_error` and
  `[migrations] replay` as recovery escape hatches. An existing database has its historical
  migrations marked `adopted` on first start.
- **Proposal endpoints are authenticated and tenant-scoped.** `GET /api/v1/proposals` and its
  sub-resources had no authentication and returned every proposal in the instance. Migration
  `0050` adds `proposals.workspace_id` and reads are filtered by it; rows whose owning workspace
  cannot be derived are left `NULL` and are visible to instance administrators only. Creating a
  proposal requires an explicit `workspace_id` when the author belongs to more than one
  workspace. Settlement of expired proposals moved from the API read path into the worker, so a
  deployment that runs the API without the worker no longer settles proposals.
- **`PUT /api/v1/labels/:id` requires the `owner` or `admin` role**, and the `admin` permission
  for a bot token. Labels are workspace-level and one edit changes them across every project.
  Delete is held to the same bar.
- **Attachment downloads require authorization.** `/uploads/*` and `/api/v1/uploads/*`,
  including the `thumbnails/`, `previews/`, `variants/` and `signatures/` sub-paths, were public.
  They now admit either a session (JWT or bot token) or a valid, unexpired signed download URL,
  and the object is then scoped to the workspace that owns it.

### Added

- Universal forms: project-defined business data types with grid and detail views, record links
  and child tables, formulas, per-role permissions, import and export, attachments and
  electronic signatures; WASM plugins for field validation, formulas and event handlers;
  configurable workflows; scenario templates. The MCP server grew from 34 to 105 tools.
- `scripts/start.sh --no-build` starts a deployment from prebuilt binaries in `target/release`,
  for hosts with no Rust toolchain, and refuses to proceed naming any binary that is missing.
- `frontend/Dockerfile.prebuilt` serves an existing `frontend/build/` for hosts with no Bun
  toolchain.
- `Dockerfile.prebuilt` takes its runtime base image from a compose variable, which must ship a
  glibc at least as new as the build host; `scripts/start.sh` derives it from the host and
  persists it to `.env`.
- `scripts/start.sh --check-config` validates both generated configuration files and `.env`
  without starting anything.
- MCP list results carry a `pagination_hint` (`total`, `total_pages`, `has_more`, `next_page`),
  `form_records.list` forwards paging, view, sort and filter parameters, and `serverInfo`
  reports the real version.
- An optional `project_id` filter on `/api/v1/search`, validated against the caller's scope
  before any result query runs.
- Test coverage for the MCP server over real SSE sockets and the CLI, caller identity and HTTP
  auth, policy bypass attempts, configuration validation, the migration ledger against a live
  PostgreSQL, delivery pickup, and a frontend round trip over all 27 form field types.

### Fixed

- **Universal forms, cross-tenant reads.** Record link targets are validated against the source
  record's workspace, project and `form.view` permission; child aggregate, child listing and
  parent recalculation queries are scoped to the parent record's workspace and project; and
  `record_scope=owned` is honoured on every record endpoint, not only on list.
- **Universal forms, async job artifacts.** Export, import and attachment package job results are
  restricted to the job's creator, and bulk result bodies are no longer returned in job listings.
- **Universal forms, events bypassing field-level permissions.** Event payload redaction is
  declared per event type instead of by a key-name denylist, and an event type with no
  declaration withholds its payload.
- Attachment claims and record writes that point at an upload owned by another workspace are
  rejected.
- Stored values of hidden or unreadable fields are preserved on record update, and formulas are
  evaluated against the stored record merged with the payload.
- Required `child_table` fields are enforced where the row count changes instead of rejecting
  every update after creation.
- **MCP errors are no longer reported as success.** The API returns HTTP 200 for every error and
  carries the status in the `{code, message, data}` envelope; the client now validates the
  envelope on every verb, and two tools that returned a hardcoded `{"success": true}` return the
  backend payload.
- `CallToolResult.is_error` is serialized as `isError`, the name MCP clients read, and tool
  results are no longer silently truncated.
- **The MCP project policy gate covers all 105 registered tools**, previously only the form tools.
  Project identifiers are validated as UUIDs and percent-encoded, every tool is classified
  against the live registry, the owning project is resolved from the object rather than taken
  from call arguments, and `resources/read` runs through the same policy.
- An SSE `POST /messages` checks that its stream still exists before running the tool.
- **Delivery reliability.** Expired leases are reclaimed, completion writes must present the
  lease token, exhausted rows are dead-lettered, failures retry with capped backoff, and a
  delivered payload is not re-sent by crash recovery.
- **Delivery signing is enforced.** A signed delivery without a resolvable credential is refused
  instead of being sent unsigned; signatures use `X-Webhook-Signature: sha256=<hex>` over the raw
  body. The legacy webhook sender no longer follows redirects, re-validates the target before each
  request and caps what it reads and stores.
- The worker installs the runtime configuration the API library reads, so form jobs and retention
  use the configured object store.
- Logging defaults to stderr, so it can no longer corrupt the MCP stdio protocol.
- `/api/v1/search` is scoped to the caller's projects, and bot tokens to their own workspace.
- Frontend record updates no longer drop fields the editor did not round-trip, and `multi_select`,
  `number`, `rating`, `progress` and `location` values are submitted correctly.
- Attachment URLs with unsupported schemes are not rendered, and `frontend/nginx.conf` no longer
  publishes the MCP server on the web origin.
- Migration `0048` adds partial indexes for lease recovery and delivery pickup.

### Security

- Dependency security updates; `.cargo/audit.toml` and `deny.toml` document each ignored advisory
  that had no upstream fix, with the reason.
- Credentials are held in a `Secret` wrapper that redacts them in every `Debug` and error output.

## [0.1.4] - 2026-03-18

### Added

- Configurable workflows with an effective-workflow resolver; the board and issue status pickers
  are driven by the workflow API.
- MCP server CLI subcommands for direct tool invocation, including `files upload`.
- A release workflow for all platforms that ships `api`, `worker`, `mcp-server` and the frontend.

### Changed

- Licensed under MIT OR Apache-2.0.
- Outbound HTTP uses rustls, which makes the ARM64 builds work.

## [0.1.3] - 2026-03-01

### Added

- Pagination for the `work_items.list` MCP tool.

### Fixed

- The workspace admin sidebar guard, and image and video paste upload in issues.

## [0.1.2] - 2026-02-28

Same source as 0.1.1, republished.

## [0.1.1] - 2026-02-28

First public release of the MCP server binaries.

### Added

- Project management API and web UI: workspaces, projects, issues, comments, labels, sprints,
  board, activity feed, notifications, governance and AI task callbacks.
- MCP server as an HTTP client of the API, with 34 tools, resources and resource templates over
  HTTP, stdio (including Content-Length framing) and SSE.
- Workspace bot tokens (`opr_` prefix) and their management endpoints.

[Unreleased]: https://github.com/openprx/sylvode/compare/v0.2.21...HEAD
[0.2.21]: https://github.com/openprx/sylvode/compare/v0.2.8...v0.2.21
[0.2.8]: https://github.com/openprx/sylvode/compare/v0.2.7...v0.2.8
[0.2.7]: https://github.com/openprx/sylvode/compare/v0.2.2...v0.2.7
[0.2.2]: https://github.com/openprx/sylvode/compare/v0.1.4...v0.2.2
[0.1.4]: https://github.com/openprx/sylvode/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/openprx/sylvode/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/openprx/sylvode/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/openprx/sylvode/releases/tag/v0.1.1
