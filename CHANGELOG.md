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

- **Flow idempotency-key body drift is `invalid_update` with `details.reason =
  "idempotency_key_body_drift"`, no longer a 409 `Conflict`.** Reusing an idempotency key with a
  different request body (object/record create, embeds, grants, lifecycle and governance
  commands, package export/import, delivery replay, maintenance operations) now answers envelope
  `code` 400 with that reason, so it can be told apart from `stale_frontier` (still 409) without
  reading `message`. Internal failures on the Flow surface answer `server_rejected` (500,
  non-retryable) instead of the untyped 500; an identical request that is still running answers
  `server_draining` with `reason = "contention"` and `retry_after_ms`.

- **Every existing bot token only works with the REST API after the upgrade.** Migration
  `0062_flow_forms_bridge.sql` adds a transport surface to each bot token,
  `ALTER TABLE workspace_bots ADD COLUMN IF NOT EXISTS transport_surface TEXT NOT NULL DEFAULT 'rest';`,
  so every token that exists when upgrading from 0.2.21 is bound to `rest`. The API accepts a
  bot token only on the surface it is bound to, and the 0.2.21 MCP server and every later client
  declare their surface (`X-Sylvode-MCP-Surface` / `X-OpenPR-MCP-Surface`), so **after the
  upgrade every MCP integration (`http`, `sse`, `stdio`) and every command-line integration using
  an existing token is refused with 401** (`bot credential is not valid for the presented
  transport`). Plain REST calls with those tokens keep working. For each integration, create a
  new bot token bound to the matching surface — `mcp_http`, `mcp_sse`, `mcp_stdio`, `cli`, or
  `cli_tools_call` for `tools call` (see **`tools call` declares its own transport surface**) —
  on the Members page or with `POST /api/v1/workspaces/{id}/bots` and `"transport_surface"`, then
  put the new token in that client's configuration (`mcp.bot_token`, the agent's bearer token or
  `--bot-token`). Existing tokens are not rebound automatically; see the
  [upgrade sequence](docs/sylvode-v1.0-compatibility.md#upgrade-sequence).
- **Outbound webhook `User-Agent` changed** from `OpenPR-Webhook/1.0` to
  `Sylvode-Webhook/1.0 (compatible; OpenPR-Webhook/1.0)`. A `User-Agent` cannot be sent twice,
  so receivers that match the old value **exactly** or as a **prefix** no longer match and must
  be updated. Receivers that match `OpenPR-Webhook/1.0` as a substring are unaffected. The
  `request_headers` recorded with each delivery carry the same value that was sent. The payload
  and its `X-Webhook-Signature` are unchanged. Flow event deliveries (the event dispatcher) and
  AI-task deliveries (the worker), which sent no `User-Agent` of their own, now send the same
  value.
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
  messages include the full error chain, so fuel exhaustion is named. A plugin run whose request
  is cancelled (the client disconnects, or the form write that triggered a hook is dropped) is no
  longer stopped with the request: it runs to its own deadline and is recorded like any other
  run, where before nothing was recorded at all. A formula hook whose output the host refuses as
  a patch is now recorded as `completed` (the plugin ran) before the write is refused.
- **Plugin modules are limited to 4 MiB, and the deadline covers compilation.** Installing a
  plugin whose decoded module exceeds 4,194,304 bytes is refused with 400 before the module is
  decoded or compiled, and an installed module over the limit fails at invocation (`failed`).
  `runtime.timeout_ms` now bounds the whole invocation from the moment the request starts:
  waiting for a blocking thread, compiling and instantiating the module, and running the guest.
  Previously compilation ran outside the deadline, so a large module held the request for as
  long as compiling took, on every invocation. When the deadline passes before the guest has
  started, the invocation returns `timeout` at once with no fuel recorded, and the guest never
  runs. Each module is compiled on the invocation's own thread instead of the shared compilation
  pool, so one large module no longer delays other plugins, and install-time validation runs off
  the request threads.
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
- **Flow commands report an unreachable API as `network_error`.** When the API gives no
  response at all (connection refused, DNS failure, TLS failure, timeout, a dropped connection, or
  a reply that is not the API's envelope, such as a proxy error page), the `sylvode` Flow commands
  (`features`, `objects`, `collections`, `records`, `collab`, `deliveries`) now report the CLI-local
  code `network_error` with `recoverable: true` and `details: {"reason": "unreachable"}`, as the
  error-mapping contract specifies. They previously reported `server_draining` with empty
  `details`, which broke that code's contract (its `details.reason` is required and is only `drain`
  or `contention`) and read like a server drain. The message now names the API URL and the cause
  and says the command can be retried, and table format prints
  `Error [network_error]: network failure: ...`. **Scripts that detect a network failure with
  `error.code == "server_draining"` must change to `network_error`; scripts that use exit code 9
  are unaffected.** A local problem that stops the request before anything is sent (no bot token
  configured, a package file that cannot be opened) is now `usage_error` with exit 2 instead of
  exit 9. The nine workspace groups keep the `mcp-server` behaviour (`Request failed: ...`,
  exit 1).
- **`tools call` declares its own transport surface.** `sylvode tools call` and
  `mcp-server tools call` now present the surface `cli_tools_call` to the API, as the MCP surface
  contract specifies; every other workspace and Flow command still presents `cli`. The API accepts
  a bot token only on the surface it was issued for, so a token issued for `cli` that was used
  with `tools call` is now refused with 401, and a `cli_tools_call` token does not run the native
  commands. Issue a separate `cli_tools_call` token (Members page, "CLI tools call") for
  `tools call`. Operation logs and Flow event origins record `cli_tools_call` for these calls.
- **A bot operation whose audit row cannot be written no longer reports success.** Every request
  made with a bot token writes a `bot_operation_logs` row, the record that attributes the call to
  its bot, surface and tool. The row was written in the background and a failed write only logged
  a warning, so the operation reported success with no record of who made it. The API now writes
  the row before it answers: when the write fails, a successful operation is answered with the
  internal-error response (`code` 500, `internal server error`) and the API logs the failure at
  `ERROR` with the database's reason; an operation that already failed keeps its own error. The
  operation itself has run by then, so retry it with the same idempotency key where the command
  takes one. Each bot request now also waits for that one insert before it is answered.
- **`work-items create` starts in the project workflow's initial state.** Under `mcp-server` and
  `sylvode` the command always sent `state=backlog`, so a project whose workflow has no `backlog`
  state refused the create (`state must be one of: ...`) and a project whose workflow starts at
  another state got its items in `backlog`. The command now sends a state only when `--state` is
  given, as the `work_items.create` tool does. A script that relied on new items landing in
  `backlog` in a workflow that starts elsewhere must pass `--state backlog`.
- **The webhook receiver example config is `config/sylvode-webhook.example.toml`.** It was
  `config/openpr-webhook.example.toml`. The compose `webhook` service and `scripts/start.sh` default
  to the new path; a `SYLVODE_WEBHOOK_CONFIG` (or legacy `OPENPR_WEBHOOK_CONFIG`) that names the old
  example file must be updated. Its secret placeholder is now `replace_with_webhook_secret`.
- **Demo account defaults.** `scripts/bootstrap-restaurant-demo.sh`, the deployed-environment smokes
  and the Playwright specs default to `demo@sylvode.local` / `SylvodeDemo123!` instead of
  `demo@openpr.local` / `OpenPRDemo123!`. A demo database bootstrapped earlier keeps its old
  account; set `SYLVODE_DEMO_EMAIL` and `SYLVODE_DEMO_PASSWORD` (or `TEST_EMAIL` /
  `TEST_PASSWORD` for the specs) to reuse it.

### Added

- Flow conversion jobs: a job the stale-`started` sweep failed carries
  `error_reason: "stale_started"` next to `error: "server_rejected"` in every job response
  (`GET /flow/conversions/{job_id}`, commit and retry), so a client can offer a retry for an
  interrupted run instead of treating it as a permanent refusal. Other failures, retry claims
  and completions have no `error_reason`. Migration `0071_flow_conversion_job_error_reason.sql`
  adds the column.
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
  - Web UI: a workspace Flow settings page under `/workspace/{workspaceId}/settings/flow` for
    workspace owners and admins, which shows the current flag, default member level and
    authorization epoch, and turns Flow on or off or changes the default member level after an
    explicit confirmation; the sidebar shows the Flow entry only while Flow is enabled.
  - Web UI: a workspace package export and import page under
    `/workspace/{workspaceId}/settings/flow/package` for workspace owners and admins. Export
    polls the job and downloads the `.sylvode-flow.zip`, hashing the downloaded bytes locally
    (SHA-256) and refusing the file unless the hash equals the job checksum; import is a
    five-step wizard (choose file, upload with progress and cancel, mapping and policies, server
    preview, exact-hash confirmation) that keeps checksum, format and policy errors on screen and
    out of confirm, and opens a report page that resumes polling after a reload.
  - Web UI: a workspace Flow operations panel under
    `/workspace/{workspaceId}/settings/flow/operations` for workspace owners and admins, usable
    while Flow is disabled. It shows health (auto-refreshed every 15 seconds, pausable, stale
    sample kept on a failed refresh, dead-letter counted apart from cancelled deliveries), lag
    with a projection-lag badge and per-object rows, and integrity per document; runs verify as a
    dry run; runs compact and rebuild-projection as a dry-run receipt followed by an execute that
    requires the exact target id typed by the user; and replays deliveries after a dry-run
    summary and an explicit acknowledgement, refusing windows older than 30 days locally.
  - Web UI: a conversion wizard under `/workspace/{workspaceId}/flow/{objectId}/convert`,
    opened from the object panel's "Convert to Forms…" action on a Page, that turns the Page into
    a Universal Forms record in four steps that cannot be skipped: source (frontier, document
    sequence and what the conversion does to ownership), target project, form and constant field
    values, server preview (schema version, object count, permission decision, countdown to
    expiry), and an acknowledged commit that sends the preview's own identifiers. A stale source
    is re-read and previewed again; an expired preview cannot be committed.
  - Web UI: a conversion job page under `/workspace/{workspaceId}/flow/conversions/{jobId}`,
    where the wizard lands after a commit. It polls the job until it is completed or failed and
    shows the status, the source Page (linked when the viewer can open it), source frontier,
    target schema version, lineage, warnings and error, and links each created record to its
    Forms record page (an id that is not a readable record is shown with a copy button). A failed
    job can be retried after an explicit confirmation, reusing one idempotency key per job; a
    permanent rejection withdraws the retry. Missing and inaccessible jobs look the same.
  - Web UI: a Share section in the object panel. It reads the object's grants and shows the
    caller's own level and the inheritance flag; with full access it also lists the grants
    inherited from parents apart from the grants on the object, and adds, changes and removes
    grants and turns inheritance off or on. Every change that can lower anyone's access
    (removing or lowering a grant, removing everyone, stopping inheritance) is first sent as a
    dry run and its `permission_changes` are shown; the real request is only sent on confirm,
    under the dry run's idempotency key. Losing one's own full access needs an explicit
    acknowledgement before `confirm_self_lockout` is sent. After a change the panel shows the
    server's response and reads the grants again. Without full access no grant list is shown and
    nothing can be changed.
  - Web UI: a Relations section in the object panel, loaded the first time it is opened and
    paged with the server's opaque cursor, with direction and relation-type filters. Each
    visible relation links to its target object; a relation whose target the viewer cannot see
    is shown as a placeholder with no link and no copy action. A relation that is not provably
    in the object's workspace, a malformed row, or a rejected read hides every relation and
    shows an integrity error instead.
  - Web UI: history entries in the object panel expand into their semantic diff (title change,
    added, removed, changed and moved nodes with their text and property changes; no node ids,
    peer ids or bytes), with errors worded by `error_code`. The Info section exports the object
    as JSON, Markdown, CSV (Collections only) or a package (optionally with history), polls the
    job and downloads the file after checking its locally computed SHA-256 against the job
    checksum.
  - Web UI: the object panel is split into Info, Share, Relations and History tabs
    (`role="tablist"`, arrow keys, Home and End); Share and Relations load when their tab is
    first opened.
  - Web UI adapters for the navigator: `flowApi.searchObjects` (`GET .../flow/search`, cancellable
    with an `AbortSignal`) behind `ObjectRepository.search`, a search controller (200ms debounce,
    the previous request aborted and its late answer dropped, stale hits marked, clamped keyboard
    index, `require_current` failures not retried as `allow_stale`), a projection-lag badge
    controller refreshed every 30 seconds, and a standalone Collection create session that keeps
    one idempotency key and one set of field and view ids per intent. `CommandService.createObject`
    accepts the caller's idempotency key and throws the typed `error_code`.
  - Web UI: the navigator searches Flow objects as you type (one request 200ms after the last
    keystroke, the previous one cancelled). Results are a keyboard listbox (`aria-activedescendant`,
    arrow keys, Home, End, Enter opens the object page, which reads the current object); a hit
    whose index lags the document is marked Stale, and a lagging index frontier is shown. An
    optional "up-to-date index only" switch sends `freshness=require_current` and shows the
    `stale_frontier` refusal instead of falling back. The navigator header shows the workspace
    projection lag (`max_lag`, `p95_lag`, refreshed every 30 seconds, `aria-live`), a New menu
    with New Page and New Collection, and 44px header buttons instead of the emoji glyphs. New
    Collection asks for a name, optional text fields (one default field) and a table or board
    view and creates it with one `POST .../flow/objects`; Collections are listed in the tree.
  - Web UI adapters for Collections: `flowApi.getObjectSchema`, `getCollection`,
    `listCollectionRecords` (filter, sort, group, cursor, field ids), `queryCollectionRecords` and
    `createCollectionRecord`, behind `ObjectRepository.getSchema` / `getCollection` /
    `listRecords` / `queryRecords` and `CommandService.createRecord`. `ObjectRepository.openRecord`
    / `closeRecord` reference-count the one read of a record's document; `open` accepts an object
    the caller already read. `CommandService.execute` now throws the envelope's typed
    `error_code` (with `details`) and falls back to the HTTP-status mapping only for a legacy
    envelope.
  - Web UI Collection view controller (`lib/flow/collection-view.ts`) and typed value codec
    (`lib/flow/collection-values.ts`): schema parsing (live fields and views by position, unknown
    types read-only, `config.restricted`, select options in `config.options`), editor/wire
    conversion for the seven field types (a number is sent as a JSON number, a date only as a
    real `YYYY-MM-DD`), placeholders for restricted, missing and malformed values that never
    fall back to another representation, one `record_patch` per edited cell with the record's
    frontier as `expected_frontier` (a `stale_frontier` keeps the draft and re-reads the
    records), drafts kept per record across view switches, a record document opened only for
    the record panel, record create with one key per intent, and every `field_*` / `view_*`
    command refused without a request unless schema mode is on.
  - Web UI: Collections open on the object page through the renderer registry
    (`collection:table`, `record:panel`, loaded on demand) instead of the unsupported fallback,
    without a collaboration ticket or bootstrap. The view switcher lists the Collection's saved
    views. The table renders only the visible rows, has resizable columns, is a `role="grid"`
    (arrow keys, Home, End, Enter edits, Escape cancels) with typed editors per field type
    (relations are read-only links), adds records from its footer, and turns into a card list
    below 768px. A record opens in a side panel with its properties and its body; unsaved edits
    survive switching views. **Design schema** (`?mode=schema`) is the only place to create,
    rename, retype, configure (select options), archive and reorder fields and to create, rename
    and reorder views; a refused change is shown by its `error_code`. Strings are under
    `flow.collection.*` and `flow.record.*`, which the i18n parity gate now requires.
  - Web UI: the Collection board (`collection:board`) groups records by a select field, one
    column per option plus "No value" (`role="list"`); dragging a card to another column, or its
    44px "move to previous / next column" buttons, writes that field with one `record_patch`, and
    each column adds a record with its option preset. Without a select field the board explains
    how to add one. A filter, sort and board grouping bar narrows the records with an unsaved
    draft query (sent as query parameters, never written to the document); in schema mode
    **Save view** stores the draft and the column widths in the view with `view_update`.
  - Web UI: the Page slash menu adds **Collection**, **Reference a Form**, **Embed a Form** and
    **Convert to Forms…**, each with one line on who owns the result. Collection asks for a title
    (default "Untitled collection") and sends one `create_collection_embed` command (never an
    object create followed by a block insert); while it runs a single provisional block is shown
    that is never written to the Page document, a failure removes it and offers Retry with the
    same idempotency key and body, and a permanent failure offers no retry. The new Collection is
    rendered in the Page as an embedded `collection:table` with an Open link, and the navigator
    lists it under its Page. Reference and Embed pick a project, a form and optionally a record
    and call `POST .../references` with `display.mode` `reference` or `embed`; the Page keeps only
    a placeholder block pointing at the `reference_id`. Reference cards show the title, type, a
    permission badge and a link to Forms; Embed cards also show the target's content when the
    request-time permission allows viewing and otherwise a placeholder without requesting any
    Forms data; a reference the bridge no longer returns is shown as unavailable. Removing a
    reference asks for confirmation and only calls `DELETE .../references/{id}`. Convert opens
    the conversion wizard. Strings are under `flow.canvas.slashMenu.*`, `flow.canvas.embed.*`
    and `flow.bridge.card.*`.
  - Web UI: collaboration on a Page. Other people's carets and selections are shown with their
    name and colour and a marker on the block they are in (`presence` frames, at most four per
    second, payload checked against the 8 KiB ceiling, expired locally after the 30 second TTL;
    presence is never written to the document and never locks a block). A peer's committed edit is
    applied in place from its `update`/`accepted` pair instead of reconnecting. Edits made while
    offline go to an IndexedDB outbox (update bytes, base frontier, idempotency key, intent); the
    sync indicator shows **Offline** with the pending count. On reconnect the Page fetches the
    accepted bootstrap, merges it, re-checks the user's permission and replays the queued edits in
    order under their original keys; an edit the accepted snapshot already contains is not sent
    again. An edit the server refuses (`policy_rejected`, `invalid_update`, `server_rejected`, a
    size limit) and every later edit that depends on it go to a **recovery draft** (download as
    JSON or copy as text, each entry with its reason), and the Page is rebuilt from the accepted
    state. A user whose level is `view` or `comment` gets a read-only canvas (no typing, slash menu
    or block handle); a revocation on a live connection (close 4403) or a refused write whose
    re-check finds no write access switches to read-only and keeps pending edits in the recovery
    draft. Three consecutive `server_rejected` answers close the connection with 4500 and stop
    reconnecting. `invalid_update` reasons are acted on: `subtree_spans_multiple_projects` points
    to the workspace operations page, `child_project_must_match_parent` asks for the parent's
    project, and `idempotency_key_body_drift` is resent once under a new key. The recovery draft
    download is a JSON file (`sylvode.flow.recovery-draft.v1`); it used to be the raw engine
    snapshot. Strings are under `flow.presence.*`, `flow.sync.pending`, `flow.recovery.*` and
    `flow.error.invalid_update_reason.*`.
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
- **Brand residue gate** `scripts/verify-sylvode-brand-residue.sh`: every `OpenPR` / `openpr` in
  the tracked files of this repository and of the webhook, documentation, website and organisation
  repositories must be covered by a reasoned entry of `scripts/contracts/sylvode-brand-allowlist.json`.
  CI runs it for this repository; the five-repository form is a local release gate.
- **Project policies in the repository**: `SECURITY.md` (how to report a vulnerability and what is
  in scope), this `CHANGELOG.md`, and a rewritten `CONTRIBUTING.md`.
- **Workspace home page** at `/workspace/{workspaceId}`, which used to render a 404. It shows the
  workspace name, slug and the viewer's role, the project total with the five most recently
  updated projects, a Sylvode Flow card (a link into Flow when it is enabled, a link to the Flow
  settings for admins when it is not, nothing for other members) and, for owners and admins,
  links to Members, Webhooks, Operation Records and Settings. Clicking a workspace on
  `/workspace` now opens this page instead of the project list. The page title is
  `Sylvode - <workspace name>`, and the Operation Records, Flow, Workflows and Forms record pages
  now carry the same `Sylvode - ` title prefix as every other page.
- **Browser E2E and image contents in CI.** `scripts/e2e-web-stack.sh up|down` starts the stack
  the browser specs run against (release API on a scratch database, a generated configuration
  with a random `jwt_secret` and `[flow] collab_allowed_origins` set to the UI origin, the built
  frontend behind a same-origin proxy that also forwards WebSockets, and an instance admin); `up`
  refuses to start when `collab-isolated-apply-worker` is not next to the API binary. The new
  **Web E2E** CI job uses it to run the `flow-settings`, `flow-package-roundtrip`,
  `flow-operations`, `flow-convert` and `workspace-home` specs with `E2E_REQUIRE_DATABASE=1` and
  fails on any skipped test. The new **Image contents** job builds the `api` and `worker` images
  from `Dockerfile.prebuilt` with `scripts/verify-prebuilt-images.sh` and checks that only the
  `api` image ships `collab-isolated-apply-worker` and that `/app/api --build-info` runs in it.
- **Form record detail shows its Flow lineage.** `GET /api/v1/form-records/{record_id}` (and the
  MCP tool `form_records.get`, which passes the response through) carries a read-only
  `flow_lineage` object: `references` lists the live Flow references that point at the record and
  `conversions` lists the conversion jobs that created it. Each entry is filtered by the caller's
  Flow read access to its source object; an unreadable source is reported only as
  `reference_id`/`job_id` with `source_available: false`. A readable conversion entry carries
  `job_available`, true when the caller may open the job (its actor or a workspace
  owner/admin). Both lists are empty when Flow or the
  Forms bridge is disabled for the workspace.
  The web record detail page shows this as a read-only **Flow** section: each reference with its
  source Page (linked), display mode and time, each conversion with its source Page, job status
  and a link to the job page when `job_available` is true. An unreadable source shows only a
  "no access to the source Page" placeholder with no link. The section is hidden when both
  lists are empty or the API does not send the field; references are still removed from the
  Page card in Flow, never from the record.

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
  `command: ["/app/openpr-webhook", "/etc/sylvode-webhook/config.toml"]`. The
  `ghcr.io/openprx/sylvode-webhook` image name exists only from the first Sylvode Webhook release
  after 0.3.3 onward (every earlier image is published only as `ghcr.io/openprx/openpr-webhook`),
  so until that release is published, starting the `connectors` profile with the defaults fails
  to pull; set `SYLVODE_WEBHOOK_IMAGE=ghcr.io/openprx/openpr-webhook:<tag>` together with the
  `command` above.
- The repository root carries exactly `LICENSE-MIT` and `LICENSE-APACHE`. `LICENSE-APACHE` is now
  the complete standard Apache License 2.0 text instead of a shortened rendering, the duplicate
  `LICENSE` file is gone, and `LICENSE-MIT` names OpenPRX Contributors as the copyright holder.
  `LICENSE-APACHE` is byte-identical to the text published at apache.org, and `LICENSE-MIT` gives
  2026, the year of the first commit.
  The licence itself (`MIT OR Apache-2.0`) is unchanged.
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
| Compose variables `OPENPR_BIND_HOST`, `OPENPR_API_PORT`, `OPENPR_FRONTEND_PORT`, `MCP_SERVER_PORT`, `OPENPR_RUNTIME_BASE`, `OPENPR_FRONTEND_DOCKERFILE` and `OPENPR_WEBHOOK_*` | `SYLVODE_BIND_HOST`, `SYLVODE_API_PORT`, `SYLVODE_FRONTEND_PORT`, `SYLVODE_MCP_PORT`, `SYLVODE_RUNTIME_BASE`, `SYLVODE_FRONTEND_DOCKERFILE` and `SYLVODE_WEBHOOK_*` | One line on stderr per variable from `scripts/start.sh`. Only `scripts/start.sh` reads the legacy names: `docker-compose.yml` interpolates `SYLVODE_*` only, so running `docker compose` directly with an old `.env` silently uses the defaults. |
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
- **AI task references are checked against the project.** `POST /api/v1/projects/{id}/ai/tasks`
  accepted any `reference_id`, which is handed to the project's AI participant and recorded as the
  causation of the task's events, so a project admin could point the agent at an entity of another
  project or workspace. The reference must now be a work item of the project, a comment on one, or
  a proposal of the project's workspace; anything else, and a `reference_id` without a
  `reference_type`, is refused with 400. Present in 0.2.21.
- **Six cross-tenant reads and writes closed.** Draft, open, voting and rejected proposals were
  visible to any authenticated user of the instance, the project context's recent decisions had
  no tenant filter, and issue links, bot label batches, sprint assignment and bot mentions did not
  check the tenant of the id being attached. Empty scopes and missing tenant columns now fail
  closed, and a resource of another tenant is indistinguishable from an absent one. These were
  present in 0.2.21.
- Request tracing spans no longer contain query strings, and the reference Caddy and nginx
  configurations no longer log collaboration tickets or signed download signatures.
- Auth cookies carry `Secure` (see **Breaking and behaviour changes**).
- Details of real deployments are removed from the tree: the legacy-pages inventory spikes
  (`spikes/legacy-pages-inventory/`) named a deployment host's ssh alias, public domain, admin
  e-mail and install path, now placeholders, and `playwright.config.ts` and the eleven
  `scripts/smoke-universal-forms-*.mjs` defaulted to a maintainer's private VPN address; they
  now default to `http://localhost:3000` (and `http://localhost:8090/mcp/rpc`), still
  overridable with `BASE_URL` / `OPENPR_FRONTEND_URL` / `OPENPR_MCP_URL`.
  `scripts/test-no-instance-literals.sh` (run in CI) fails on private IPv4 addresses and bot
  token literals in any tracked text file, and `scripts/test-no-machine-paths.sh` now also
  scans `apps/`, `crates/` and `docs/schemas/`.
- `skills/openpr-mcp/scripts/mcp-regression.py` contained a bot token and the workspace and
  project ids of a test instance since 0.1.x. They are removed; the script reads them from
  `SYLVODE_MCP_REGRESSION_TOKEN`, `SYLVODE_MCP_REGRESSION_WORKSPACE_ID` and
  `SYLVODE_MCP_REGRESSION_PROJECT_ID` and exits when one is missing. The token remains in the
  public git history and must be treated as compromised: revoke it on the instance that issued
  it. Its output is now in English. The production readiness audit now checks the script's
  default API URL in its new form.
- **A plugin could allocate unbounded host memory through table growth.** The plugin store
  limited linear memory to `runtime.memory_bytes` but left the table element count at
  wasmtime's default, which is unlimited, so a guest could `table.grow` hundreds of millions of
  elements (about 8 bytes of host memory each) within one invocation's fuel budget. A plugin
  table now holds at most 65,536 elements, its initial size included: a larger initial table
  fails to instantiate and a larger `table.grow` returns `-1` to the guest. Present in every
  release since 0.2.2.

### Removed

- The `openpr-mcp-server-<target>` release archives (see **Breaking and behaviour changes**).

### Fixed

- Flow collaboration: an `update` whose causal dependencies are not in the document (the engine
  could only buffer it as pending) is refused with `invalid_update` and
  `details.reason = "causal_dependency_missing"`; it gets no seq and nothing is stored. It used to
  be accepted with a seq while its content stayed invisible. Replaying already-stored history is
  unchanged.
- Flow collaboration: a `rejected` frame answering an `update` carries that update's
  `update_id` when the update is refused by the `update_rate` or `frame_rate` limiter or is not
  a well-formed frame. Only the `type` and `update_id` members of a frame refused before
  decoding are read; a frame over `websocket_frame_bytes` is still refused on its length alone.
- Flow collaboration: re-sending an `update` whose `update_id` is already committed is answered
  with the original `accepted` receipt (same `head_seq`, `head_frontier` and `event_id`), on the
  connection that committed it and after a reconnect. The receipt used to be dropped as a
  duplicate seq, so a client retrying an update in `write_state: "unknown"` with the same
  `update_id` never got an answer.
- Flow collaboration: an open WebSocket connection re-evaluates its authorization when the
  workspace `authz_epoch` has moved since it opened. A write refused by the commit-time epoch
  fence is re-sourced in place: if the actor still has `edit` on the document the connection
  takes the new epoch and the update is retried (and accepted); only an actor whose level is now
  below `edit` gets `policy_rejected`. A connection opened read-only that has since been granted
  `edit` is re-sourced the same way. Before, any authorization change anywhere in the workspace
  made the next write on every existing connection `policy_rejected`, including the owner's.
- Flow: a request refused because it crosses a workspace boundary through a relation (a
  cross-workspace `parent_object_id` or move target, or a stored cross-workspace
  `flow_relations` row met while listing relations) answers `invalid_update` with
  `details.reason = "cross_workspace_relation"`, so a client can show it as an integrity
  failure without reading `message`. The integrity record is written as before.
- Flow: `GET /flow/objects/{id}/bootstrap` needs `view`, the level the WebSocket `open` and
  `snapshot` (which use the same loader) already need; it used to require `edit`, so a read-only
  member could watch a document live but not download its bootstrap.
- Flow Collections: `GET /flow/collections/{id}/records` honours `view_id`. Records are queried
  with the view's saved `config.filter`, `config.sort` and `config.group_field_id`; an explicit
  `filter`, `sort` or `group` parameter overrides the saved value for that part only, and a
  `view_id` that is not a view of the Collection is `invalid_update`. The parameter used to be
  ignored.
- Flow Collections: `GET /flow/objects/{record_id}/schema` returns the owning Collection's
  schema (the same body as the Collection's own schema) to a caller who can read the Record and
  its Collection; it used to answer `not_found` for every Record. `GET /flow/collections/{id}`
  still accepts only Collections.
- Flow Pages: `create_collection_embed` accepts `after_block_id` and inserts the embed block
  right after that block (under the same parent); without it the block is still appended. An
  `after_block_id` that names no live block of the Page is refused with `invalid_update` and
  nothing is created. `after_block_id` is part of the idempotent request body, so replaying the
  key with another position is body drift.
- Flow Collections: a record value that has the right JSON type but is not a valid value of its
  field is refused as `invalid_update` with `details.reason = "invalid_field_value"` and
  `details.field_id`, on records create and `record_patch`: an unparseable date (it used to fail
  in the typed index cast and answer an untyped HTTP 500), a `select`/`multi_select` value that
  is not in the field's `config.options`, and a relation element that is not a UUID or names an
  object outside the Collection's workspace. A query filter with an unparseable date is a typed
  `invalid_update` instead of a 500. A `select`/`multi_select` field's `config.options` must be
  an array of distinct, non-empty strings on `field_create`, `field_update` and an initial
  schema; fields without `options` stay unconstrained.
- Flow Collections (security): under `field_secrecy_enabled`, every representation of a Record
  now applies the records query's redaction (`semantic_content` is `{}`): the single-object read
  `GET /flow/objects/{record_id}` in both renders, the object list, the object diff, and Record
  command responses (`object.semantic_content` and `semantic_diff`). They used to return every
  `record_property` value, including restricted ones, and the Record body.
- Browser E2E stack: `scripts/e2e-web-stack.sh up` also starts the background worker on the
  same configuration (it is the only writer of the Flow search index), stops it on `down`, and
  refuses to start when `worker` is missing from `E2E_BIN_DIR`. The CI Web E2E job runs every
  `tests/e2e/web/flow-*.spec.ts` plus the workspace home spec.
- Web UI, Flow object page: an object whose type has no registered renderer and no collaboration
  session (a Collection, for which the server refuses a collab ticket) shows its title, the
  "view isn't available yet" fallback and the object panel, instead of the "Flow is not enabled"
  / permission error the failed session open produced.
- Web UI, Flow operations panel: maintenance receipts show the `OperationReceipt` contract
  fields — `mode`, `scope`, observed head, start and finish times, `audit_event_id`, the
  `changes[]` before/after of a compaction or projection rebuild, and the two warning codes. The
  integrity table shows the workspace-wide `counts.total` and lists failed documents with their
  `failure` (`storage_inconsistent`, `projection_missing`, `limit_exceeded`,
  `document_missing`, `replay_failed`) and error code instead of only counting them, and the lag
  section shows the admin lag endpoint's per-document rows.
- Web UI, Flow conversion job page: shows the job's `target_type`, words the stored `error`
  through `flow.error.<code>`, shows each warning by its code, and words a refused retry by
  cause (`stale_frontier`, `policy_rejected` with `target_schema_changed` or
  `permission_unconfigured`) before reading the job again. The retry confirmation no longer
  says the retry depends on the original preview. The browser spec now builds failed jobs
  through the production path (permission revoked between preview and commit) and proves a
  retry succeeds after the preview TTL has passed.
- Web UI, Flow convert wizard: `policy_rejected` is worded by its `details.reason`. A commit
  refused with `preview_expired` or `target_schema_changed` returns to the preview step and
  previews again under a new key (after reading the form again for a schema change);
  `permission_unconfigured` asks for an admin to configure the form's policy. A commit that
  fails after its job exists (`details.job_id`) opens the conversion job page, where it can be
  retried. Preview warnings are shown one per line, worded by their `<code>:<field_key>` code.
- Web UI, Flow package import: the wizard now follows the preview's `conflict_details`. A
  `project_unmapped` conflict returns to the mapping step with one row per unmapped source
  project (by title) that needs an explicit target or Unprojected and a new preview; a
  `lineage_exists` conflict under "Reject the import" lists the already-imported objects, keeps
  Confirm closed and offers a re-preview with "Reuse the earlier import". The preview shows the
  package's `manifest_summary`, and a commit refused with `invalid_update` /
  `reason = "project_unmapped"` or `policy_rejected` has its own wording. A workspace export
  whose POST answers `status: "failed"` is shown as failed with its `error` code's text.
- Flow conversions: a job left `started` for more than 15 minutes (its run crashed after the
  job was created, or recording its failure failed) is now reaped to `failed` with
  `error = "server_rejected"` and one business `flow.conversion.failed` event whose payload
  carries `reason = "stale_started"`, so it can be retried instead of being stuck. The API
  reaps such jobs once at startup, and a retry that finds a job `started` past the threshold
  reaps it in place and proceeds; a retry of a job that is `started` more recently is still
  refused. The 15-minute threshold is an implementation constant, not a contract limit.
- Flow conversions: when a commit or retry fails after its job exists, the typed error's
  `details` now also carry `job_id` and `job_status` (normally `"failed"`), so a client can open
  the job and retry it without a second request. `error_code`, `message` and the existing
  `details` (`reason`, `permission_state`, `retry_after_ms`) are unchanged.
- Flow conversions: `warnings` on the preview and on a completed record-conversion job are no
  longer always empty. Each entry is a stable `<code>:<field_key>` string:
  `target_value_missing` for a target field the mapping leaves empty (Forms fields have no
  default), `required_target_value_missing` when that field is required (the commit will be
  refused by Forms validation), and `unknown_target_field` for a mapped value that names no field
  of the target schema (it is dropped, as before, not rejected). `autonumber` and `child_table`
  fields are not reported. Form-creating conversions keep `warnings: []`.
- Flow conversions: retrying a `failed` job no longer depends on the preview's 15-minute TTL,
  so a job can be retried after its preview expired (previously that retry always answered
  `policy_rejected`). The retry re-authorizes every side and re-runs against the
  `source_frontier` and `target_schema_version` recorded on the job: a source that has moved on
  answers `stale_frontier` and a changed target schema `policy_rejected` with
  `details.reason = "target_schema_changed"`, and either one leaves the job `failed` with that
  code. Retry now also checks that the caller can read the source before answering, and a retry
  key already used by another job answers `invalid_update` with
  `details.reason = "idempotency_key_body_drift"`.
- Flow conversions: `policy_rejected` now says why with `details.reason` — `preview_expired`
  when the commit's preview is past its 15-minute TTL, `target_schema_changed` when the target
  form's schema version differs from the preview or job (at commit and at retry), and
  `permission_unconfigured` when the target form has no policy row and is therefore read-only
  through the bridge (ADR-0019 BR-4), at preview and at commit. The existing
  `details.permission_state` is kept where it was returned before.
- Flow conversions can now actually end `failed` in production. A commit first persists the job
  as `started` (with `flow.conversion.started`), then runs it; any failure from then on — a
  revoked Forms or Flow permission, an archived target form, a frontier or schema change found
  under the lock, a Forms validation error, a database error — marks the job `failed`, records
  `flow.conversion.failed`, and answers the caller with the typed error. Previously every such
  failure rolled the job row back, so `failed` (and therefore retry) was reachable only through
  a test-only fault hook. The job's `error` is now always one of the 14 stable error codes
  (`policy_rejected`, `stale_frontier`, `invalid_update`, `server_rejected`, ...) instead of an
  internal name such as `injected_before_target_create`; the internal cause is logged
  server-side only. A commit refused before the job exists (expired preview, a frontier or
  schema version that already differs, a caller who cannot see the source) still creates no job.
- Flow conversions: committing a preview that already has a job, under a different idempotency
  key, now returns that same job instead of a 500 from the one-job-per-preview constraint; two
  concurrent commits of one preview both get the single job. A replay of a commit (same key or
  same preview) returns its job even after the preview's 15-minute TTL. A key already used for
  another preview answers `invalid_update` with `details.reason = "idempotency_key_body_drift"`.
- Flow conversions: the job returned by commit, `GET /api/v1/flow/conversions/{job_id}` and
  retry now carries `target_type` (`form` or `form_record`), as the contract's job shape requires.
- Flow conversions: a malformed id in `GET /api/v1/flow/conversions/{job_id}`,
  `GET /api/v1/flow/conversions/{id}/owner` and `POST /api/v1/flow/conversions/{job_id}/retry`
  now answers the typed `not_found` envelope, like an unknown id, instead of a plain-text 400
  without `error_code`.
- Flow admin integrity: `GET /api/v1/admin/workspaces/{id}/flow/integrity` accepts `cursor`
  (previously any cursor answered 400) and pages documents in id order with `limit` 1..=100
  (default 50); `next_cursor` is present only when another page exists. With
  `scope=documents`, `documents[]` now lists every checked document: healthy rows carry
  `status: "ok"` and `checked_at` next to their fingerprint, and failing rows (previously only
  counted) carry `status: "failed"`, `checked_at`, a stable `failure` class
  (`storage_inconsistent`, `projection_missing`, `limit_exceeded`, `document_missing`,
  `replay_failed`) and `error_code`. `counts` (`total`, `checked`, `healthy`, `failed`) always
  cover every document in the workspace, whatever the `scope`, `cursor` or `limit`. A database
  error while checking now fails the request instead of being counted as a failed document.
- Flow admin lag: `GET /api/v1/admin/workspaces/{id}/flow/lag` now honours `cursor` and `limit`
  (1..=100, default 50) and lists one row per document in `projection.items`, `search.items` and
  `fanout.items` (`object_id`, `document_id`, `head_seq`, the consumer's seq, `lag`), paged in
  document-id order. `next_cursor` is present only when another page exists. The workspace-wide
  `max`/`p95` aggregates are unchanged. An invalid cursor or limit answers `invalid_update`.
- Flow maintenance operations: the `OperationReceipt` returned by document compact, projection
  rebuild and document verify now carries the contract's full shape — `mode`
  (`dry_run`/`execute`), `scope`, `observed_head_seq`, `changes[]` (before/after boundary and
  checksum for compaction, before/after projection hash for rebuild), `warnings[]`,
  `started_at`, `finished_at` and `audit_event_id` — alongside the existing fields. An executed
  compaction or rebuild now writes its audit `business_events` row (`flow.compaction.completed` /
  `flow.repair.completed`) in the same transaction as the change, and `audit_event_id` is that
  row's id (also on idempotent replay). A dry-run (compact, rebuild, verify) writes one
  `delivery_class=audit_only` event (`flow.compaction.previewed` / `flow.repair.previewed`, no
  dispatch row, no webhook) and its `audit_event_id` is that row's id.
- Flow export: an accepted, authorized export that fails while rendering (a canonical
  snapshot/update integrity check, or a server-side refusal) is now recorded as a `failed` job
  whose `error` carries the stable code (for example `checksum_mismatch`). `POST` returns that
  job receipt with `status: "failed"` instead of the bare error, an idempotent replay returns the
  same job, and `GET /api/v1/flow/exports/{job_id}` reports `error` and omits `download_url`
  (`checksum` is empty and `size` is 0, since there is no artifact). Request, permission and
  precondition errors still answer the request directly.
- Flow package import: previewing a package whose objects already have import lineage in the
  target workspace under `conflict_policy = "reject_existing"` succeeds and lists the hits as
  `lineage_exists` conflicts instead of failing; committing that preview (or any
  `reject_existing` commit whose objects gained lineage after the preview) is refused with
  `policy_rejected` and writes nothing.
- Flow package import: a preview whose `project_mapping` leaves a package source project
  unmapped (including an empty mapping) now succeeds and lists each such project in `conflicts`
  and in the new `conflict_details[]` as `{kind:"project_unmapped",source_project_id,title}`;
  it used to fail with a bare 400 that did not say which projects exist. Committing such a
  preview answers `invalid_update` with `details.reason = "project_unmapped"`. A mapping value
  may be the contract literal `"unprojected"` (same as `null`). `conflict_details[]` also
  describes import-lineage hits as `{kind:"lineage_exists",source_object_id,existing_target_id}`;
  `conflicts` keeps its string-array shape.
- Flow package import: `POST .../flow/imports/preview` returns the contract's `manifest_summary`
  (`projects:[{source_project_id,title}]`, `object_count`, `document_count`, `package_schema`,
  `engine`), so a caller can see which source projects to map before committing. Workspace
  package exports carry the referenced project titles in a new optional `projects` manifest
  field (omitted when empty, so packages without projects are byte-identical to before); for
  packages without it the title is `null`.
- Tests: two measurement assertions that only mean something under controlled conditions no
  longer fail on shared CI runners. The depth-32 inheritance-evaluation regression ceilings are
  enforced only with `OPENPR_FLOW_QUIET_PG_QUALIFIED=1` and otherwise recorded in the evidence
  line as `diagnostic_only_unqualified_environment`; the isolated-apply memory-ceiling test pins
  the exact-byte boundary on the counting allocator directly and accepts either resource ceiling
  for the end-to-end over-ceiling refusal, whose abort can lose the race to the 100 ms wall
  watchdog on a loaded host.
- `GET /api/v1/workspaces/{workspace_id}/projects` honours `page` and `per_page` (default 20,
  at most 100) and reports the real `total` / `total_pages`; it used to ignore both and always
  return every project. A request with neither parameter still returns the whole list.
- Flow: every error the Flow REST surface constructs (`/api/v1/flow/**`, the workspace Flow
  feature, projection-lag, conversion, package import/export and operations endpoints, and the
  collab ticket/WebSocket routes) is now a typed envelope carrying `error_code` (and `details`
  where the contract defines them) instead of the legacy `{code,message}` shape. A
  `flow_typed_error_gate` test lists any new untyped construction point on that surface.
  "Flow is not enabled for this workspace" is `feature_disabled` everywhere (it was a bare 403
  on `projection-lag` and every other route behind the feature check), workspace-admin and
  tool-policy refusals are `forbidden`, a missing `confirm` on dangerous operations is
  `policy_rejected`, and the not-found answers keep collapsing "no access" into `not_found`.
- Packaging: the `api` container image and every release archive now ship
  `collab-isolated-apply-worker` beside `api`. The API spawns that process for every Flow
  collaborative write and looks for it next to its own executable; the image built by
  `scripts/start.sh` (`Dockerfile.prebuilt`), the source `Dockerfile` and the release archives
  shipped `api` alone, so the first edit of a Page showed "Sync error", the text was lost on
  reload and `POST /flow/objects/{id}/commands` returned 500 (api log: `worker binary
  'collab-isolated-apply-worker' not found next to '/app/api'`). `scripts/start.sh --no-build`
  now also requires it in `target/release`, and the release job refuses an archive without it.
- Release: the macOS and Windows targets build again. `api` and `collab-isolated-apply-worker`
  used the Linux-only isolated-apply module (ADR-0014) unconditionally, so every non-Linux release
  target failed to compile. The module's types, wire codec and limits are now platform-neutral;
  off Linux the isolated-apply entry points refuse every Flow collaborative write and object diff
  with `server_rejected` (`details.reason = "isolated_apply_unsupported_platform"`,
  `write_state: not_applied`) instead of applying anything outside the boundary, `api` logs one
  startup `warn`, and `collab-isolated-apply-worker` is a stub that exits with status 2. See the
  [platform matrix](docs/sylvode-v1.0-compatibility.md#platform-matrix).
- Flow collaboration: publishing the cross-instance fanout notice after a committed update no
  longer reports a failure on every write. The statement filtered on `pg_notify(..) IS NULL`,
  which is never true (`pg_notify` returns `void`), so it returned no row and the API logged
  `committed update fanout publication failed ... internal server error` for each accepted
  update even though the notice row and the notification were written. The notification is now
  emitted from a LATERAL subquery in the same single round trip.
- Web UI: on screens narrower than 1024px the Flow navigator is a drawer, closed by default and
  opened from a 44px "Show navigator" button (`aria-expanded`, Escape closes it), so the object,
  convert and conversion job pages use the full width. At 390px the navigator used to sit beside
  the content and leave it about 70px wide. The object page stacks its context panel under the
  canvas there, and the convert and job pages' buttons are at least 44x44px.
- Web UI: the Flow operations panel is usable while Flow is disabled. It showed the "Workspace
  admins only" state because the per-object `GET .../flow/projection-lag`, which is refused while
  Flow is off, was allowed to decide the whole page. Only the admin check and the three
  `/admin/.../flow/{health,lag,integrity}` endpoints decide it now; the lag section shows its own
  notice that there is no per-object lag while Flow is off. Leaving the panel before its first
  load finished no longer starts the 15-second health poll after the page is gone.
- Web UI: `server_rejected` is now one of the client's registered stable Flow error codes, so
  the zh/en parity gate covers its keys, and a `rejected` frame carrying it on a live editing
  session fails the pending write as a permanent error instead of being ignored and leaving the
  write pending. The operations panel and the conversion job page classify it through the same
  registry; it is still shown as a permanent failure with no retry.
- Web UI: the sidebar's workspace-admin links (Members, Webhook, Operation Records, Workspace
  Settings, Flow settings) follow the viewer's member role in the workspace on screen. The role
  was read once when the app first loaded and an instance admin counted as a workspace admin, so
  a member could see another workspace's admin links and an owner who opened a workspace from the
  workspace list saw none until a reload. The role is now re-read on every workspace change, the
  links stay hidden until it arrives, and only `owner`/`admin` members see them, as on the server.
- `mcp-server serve` had no SIGTERM or SIGINT handler: it died of the signal (exit 143) and
  dropped the call it was serving, and as PID 1 of the compose `mcp-server` container it ignored
  the signal and was killed by the runtime after 10 s. On SIGTERM or SIGINT it now stops accepting
  connections (`http`, `sse`) or stops reading stdin (`stdio`), finishes the requests in flight,
  delivers an SSE result before it ends that stream, flushes stdout and exits 0. Requests still
  running 8 s after the signal are abandoned and logged, which keeps the stop inside the 10 s a
  container runtime waits before SIGKILL.
- `tools call` with a `cli_tools_call` token left no operation log: `bot_operation_logs` only
  accepted the surfaces `mcp_http`, `mcp_sse`, `mcp_stdio`, `cli` and `rest`, so every audit row of
  such a call was rejected by the table and only a warning was logged. Migration
  `0070_bot_operation_logs_surface_check.sql` adds `cli_tools_call` (expand-only; existing rows are
  untouched), the API takes its accepted bot surfaces from one list, and a test writes a token and
  an operation log row for every surface in that list against a real database.
- Documentation facts: the README's WASM plugin section had lost its heading and introduction and
  now states the current limits; it gave 2155 translation keys (2192) and 245 `.route()` calls
  (246), still mentioned the removed outbox, and described `playwright.config.ts` as targeting an
  internal address. `frontend/README.md`, `frontend/QUICKSTART.md`, `docs/API_ENDPOINTS_PHASE3.md`
  and `docs/frontend-requirements.md` are now in English. The Universal Forms acceptance guide and
  production runbook say that the `.flow-gate/universal-forms/` files they name are not in a fresh
  checkout and where each comes from. The v1.0 stable runbook and CONTRIBUTING list every input
  the v1.0 gate reads from outside the repository, and the bash and GNU tool requirement.
- The v0.9 and v1.0 reports and the v0.9 verifier read the predecessor receipt from a fixed path
  under the checkout even when `--evidence-root` named another directory; the default is now
  `<evidence root>/../<previous release>/gate-result.json`, as for v0.6-v0.8.
- `scripts/test-no-instance-literals.sh` no longer reports a private range written in CIDR
  notation (`10.0.0.0/8`), which names a range rather than an address.
- The root `Dockerfile` did not copy `Cargo.lock` or the spike crates that are workspace members,
  so it could not load the workspace; it now copies both and builds with `--locked`. A root
  `.dockerignore` keeps the build context to what the two Dockerfiles copy (no `target/` apart from
  the four release binaries, no `.flow-gate/`, no generated configuration or `.env`), and
  `backups/`, where `scripts/backup-db.sh` writes database dumps, is ignored by git.
- `SECURITY.md` named a private reporting channel that is not enabled until the release is
  published and called the release line "pre-1.0" next to a 1.x compatibility commitment. It now
  says private vulnerability reporting is enabled with the release, to use the e-mail contact
  while the **Report a vulnerability** button is absent, and that only the latest release is
  supported.
- `scripts/start.sh` copied a quoted `.env` value (`SYLVODE_FRONTEND_PORT="3999"`) with its quotes
  into the generated TOML, which then failed to load; values are unquoted as docker-compose does,
  and a port that is not a number is refused before any file is written. A legacy variable set
  only in the environment conflicted with the canonical values `.env.example` preset; those lines
  are now commented out (the defaults are unchanged). An unknown argument prints a usage text and
  exits 2 instead of building and starting the stack.
- The compose comment, the production runbook and `.env.example` presented
  `ghcr.io/openprx/sylvode-webhook` as published. They now say it exists only from the first
  Sylvode Webhook release after 0.3.3, how to pin a published image until then, and to pin a
  version tag rather than `latest`.
- The release workflow published a release, with all six platforms listed, even when a build or
  package job had failed (the arm64 leg was allowed to fail), and never checked the tag against
  the version. It now fails before building unless the tag is `v<version>` equal to the
  `Cargo.toml` workspace version and `frontend/package.json`, publishes only when every build and
  package job succeeded and every archive and checksum is present, checks out the tag for manual
  runs, writes `sha256sum`-compatible checksums on Windows and for the frontend archive, pins
  `cross` and installs frontend dependencies from the lockfile. All workflow actions are pinned by
  commit, CI has read-only permissions, and only the release job can write. CI also runs
  `cargo check --all-features` and the frontend unit and contract suites.
- The brand residue gate (`scripts/verify-sylvode-brand-residue.sh`) saw only `OpenPR` and
  lowercase `openpr` in UTF-8 file content, so `OPENPR_*`, `OpenPr*`, file names, symbolic link
  targets and UTF-16 text were never checked. It now matches `openpr` in any letter case in file
  content (UTF-8 and UTF-16), tracked paths and link targets, adds the reason
  `internal_identifier` (identifier tokens in source files only), and under `--release` fails on a
  checkout with uncommitted changes. The allow-list classifies the newly visible names: legacy
  compose variables, `OPENPR_TEST_DATABASE_URL`, the scripts' and tests' `OPENPR_*` inputs and the
  Rust client types; four entries that were wider than their reason were narrowed. Every kept
  identifier is listed under "Not renamed" in the v1.0 compatibility matrix.
- The bun tests for the bot token form and the webhook event form only searched the page source,
  so a form that always sent `rest`, or a checkbox that did nothing, passed them. The request the
  members page sends is built by `createBotRequest` and the event selection by
  `toggleWebhookEvent`, both tested for their behaviour, and the page checks ignore comments.
- `auth.allow_insecure_cookies` treated any `server.bind_addr` host starting with `127.` as
  loopback, including host names such as `127.example.org`. Only loopback IP addresses and
  `localhost` count now.
- `sylvode --help <workspace group>` printed internal source documentation as the program
  description and listed only the nine workspace groups. A help or version flag before the first
  group now always shows the top-level help of all fifteen groups, and no parser prints source
  documentation.
- `--version` / `-V` were refused (exit 2) by `mcp-server` and `sylvode`; they now print the
  program name and version and exit 0.
- The legacy configuration notice's "once per process" guarantee had no test of its own; a test
  now asks for it twice (and again after a second load) in a fresh process.
- Nothing pinned the plugin ABI export names against a rename applied to the runtime and its
  tests together. A compiled fixture, `apps/api/tests/fixtures/plugin-abi-v1.wasm` (source
  `plugin-abi-v1.wat` beside it, hash checked by the test), exports exactly `memory`,
  `openpr_alloc`, `openpr_invoke` and `openpr_plugin_abi_version` and must load and round-trip
  its input.
- `projects create` under `mcp-server` and `sylvode` failed with ``missing field `key` ``
  because it offered no way to pass the required project key; it now takes `--key`.
- `mcp-server` and `sylvode` no longer panic when stdout or stderr cannot be written. This
  includes log lines: any log event (the legacy configuration notice, a `warn` from `tools call`,
  the stdio server's startup lines) written to an unwritable stderr (`2>/dev/full`, a full disk
  behind a redirected log) made the logger report the failure on that same stderr and panic with
  exit 101. Log lines that cannot be written are now dropped, in `api`, `worker`, `mcp-server` and
  `sylvode`, and the command exits as it would with a writable stderr.
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
- Scripts under `scripts/` defaulted to paths on one maintainer's machine: the Universal Forms
  report generators, render smokes and screenshot collector wrote to a report directory outside
  the repository and overwrote what was there, the Flow gate scripts read their contracts from and
  wrote evidence into a fixed contracts checkout, and scratch trees, cargo targets and the Bun
  binary were looked up at fixed absolute paths. Generated output now defaults to the ignored
  `.flow-gate/` directory of the checkout, outside inputs are named through `--contracts-root`,
  `SYLVODE_CONTRACTS_ROOT` and similar variables (see CONTRIBUTING, "Where scripts read and
  write") and a missing one is an error, the render smokes and the screenshot collector refuse a
  non-empty target directory without `--overwrite`, and `scripts/test-no-machine-paths.sh` runs
  in CI.
- The README said the files `scripts/start.sh` generates are `chmod 600`; only `.env` is. The two
  compose configuration files are `644` so the container user can read the bind mount, and the
  README now says so and recommends restricting the `config/` directory instead.
  `docs/universal-forms-production.md` no longer describes connector receivers, connector
  receipts, the event outbox or the `connectors.*` tools, all removed in 0.2.21, and its audit
  stopped requiring the outbox sentence. The bundled MCP skill no longer lists a fixed work-item
  state enum or the `none` priority: states come from the project's workflow.

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
