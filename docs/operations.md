# Operating Sylvode

This page describes what the Sylvode processes do at runtime and how to keep a deployment
healthy: health checks, logs, reverse proxies, storage, the database, backups, the Flow runtime
switches and the background jobs. Installation is in [install.md](install.md); every
configuration key is in [configuration.md](configuration.md).

## Processes

| Process | Role |
|---|---|
| `api` | Serves the REST API, the collaboration WebSocket and, when configured, the OAuth endpoints and the alert endpoint. At startup it applies the database migrations, logs a warning for each stored webhook whose endpoint fails the outbound address policy, and starts a sweep that fails Flow conversion jobs left `started` for more than 15 minutes (`flow.conversion_reap_interval_secs`). |
| `collab-isolated-apply-worker` | Started by the `api`, with an empty environment, for every Flow collaborative write and object diff. It decides whether the write is within the limits inside a 50 ms CPU window (2 s for a package import). Linux only. It needs the `timer_create`, `timer_settime` and `timer_delete` system calls; a sandbox that blocks them makes every Flow write fail. |
| `worker` | Runs the background jobs listed under [Background jobs](#background-jobs). Opens no listener. |
| `mcp-server` | Serves the MCP protocol on `stdio`, `http` or `sse`; see [apps/mcp-server/README.md](../apps/mcp-server/README.md). |
| Web UI (nginx) | Serves the static UI and forwards `/api` and `/uploads` to the API. |
| `sylvode-runner` | Optional. Receives deliveries and runs agents for bots; see [apps/runner/README.md](../apps/runner/README.md). |

The worker's jobs lock the rows they work on (`FOR UPDATE SKIP LOCKED` or advisory locks), so more
than one worker may run against the same database.

## Health and readiness

| Endpoint | Process | Answer |
|---|---|---|
| `GET /health` | `api` | `{"code":0,"message":"success","data":{"status":"ok","service":"<app_name>"}}` while the process serves requests. It does not touch the database. |
| `GET /ready` | `api` | `data.status` is `ready` when the database answers a ping. Otherwise the answer is HTTP 503 with `"code":503` and the message `database not ready`. |
| `GET /health` | `mcp-server` (`http`, `sse`) | Answers without authentication. |
| `GET /health` | web UI nginx | Returns `healthy` from nginx itself, without contacting the API. |
| `GET /health` | `sylvode-runner` | See the runner README. |

The Compose stack uses the `/health` endpoints as container health checks, and `mcp-server` and
`frontend` wait for a healthy `api`.

## Logging

All binaries write structured logs to stderr by default, one JSON object per line. Change the
format, stream and filter in `[logging]` ([configuration.md](configuration.md#logging)); the server
binaries do not read `RUST_LOG`.

Secrets from the configuration file (database URL, signing key, bot token, forwarding secrets, S3
credentials) are never logged. The reverse proxy is the other place where credentials can leak, because two kinds of URL carry one in
their query string: the one-time ticket of `GET /api/v2/flow/collab/ws?ticket=...` and the signature of
signed download URLs (`?expires=...&signature=...`). The shipped `frontend/nginx.conf` replaces
the whole query in its access log when it contains `ticket=` or `signature=`, and
`deploy/caddy/Caddyfile` deletes those two parameters from its log. Keep an equivalent rule in any
other proxy.

## Shutdown

`api` and `worker` stop on SIGTERM or Ctrl-C. The API first ends every open OAuth event stream
(clients reconnect with their last event id), stops the conversion sweep, refuses new Flow work
with `server_draining` and closes live editing sessions, then keeps serving for a short grace period
so those answers reach the clients before it exits. The worker stops at its next check for the
signal: while it delivers AI tasks or during the pause between rounds.

## Reverse proxy

Publish the web UI, not the API, behind a reverse proxy that terminates TLS. The web UI's nginx
already forwards `/api` and `/uploads` to the API, so browser, API and uploads share one origin.
Two references are shipped:

| File | What it is |
|---|---|
| `frontend/nginx.conf` | The nginx configuration of the web UI image and the frontend release archive. |
| `deploy/caddy/Caddyfile` | A Caddy site that proxies to the web UI on `127.0.0.1:3000`, with HSTS, the same Content Security Policy and log redaction. It is written for a site behind Cloudflare: its `trusted_proxies` list is Cloudflare's address ranges. Replace `sylvode.example.com` with your host. |

Whatever proxy you use:

- Forward WebSocket upgrades on `/api` (`Upgrade` and `Connection` headers) and allow idle
  connections to stay open; live editing holds a socket open. `nginx.conf` uses a one-hour read
  timeout.
- For the OAuth event stream `GET /api/v1/oauth/events/stream`, turn off response buffering,
  caching and compression. The stream sends a keep-alive comment every 20 seconds; `nginx.conf`
  closes it after 120 seconds of silence.
- Forward `/.well-known/oauth-authorization-server` and `/.well-known/oauth-protected-resource/api`
  to the API when the OAuth server is enabled. `/.well-known/oauth-protected-resource/mcp` belongs
  to the MCP server, not the API; see [integrations/oauth.md](integrations/oauth.md).
- Serve everything under `/oauth/` (the consent page) with `frame-ancestors 'none'`, no caching
  and no referrer, as `nginx.conf` does.
- Allow request bodies up to 200 MiB, the API's own limit (`client_max_body_size 200m` in
  `nginx.conf`).
- Keep `script-src 'self' 'wasm-unsafe-eval'` in the Content Security Policy. Flow's editor engine
  is WebAssembly, and the build moves SvelteKit's start script into its own file so that no inline
  script is needed.
- List the proxy's address in `server.trusted_proxies` if it is not covered by the default
  (loopback and `172.16.0.0/12`), so the API sees real client addresses in rate limits and
  allow-lists.
- List the public origin in `flow.collab_allowed_origins`, or live editing is refused.
- Terminate TLS. A web UI served over plain `http://` on an address other than `localhost` still
  works for sign-in (with `auth.allow_insecure_cookies`), live editing and copying, but it sends
  every credential and document in clear text, and the browser withholds the features that need a
  secure context. What changes is listed in
  [install.md](install.md#serving-the-web-ui-over-plain-http). On a private network without public
  DNS, see [install.md](install.md#https-on-a-private-network).

The MCP server is not proxied by `nginx.conf`. MCP clients reach its port directly or through a
tunnel, and authenticate with their own workspace bot tokens.

## Storage

Uploads and the files derived from them (thumbnails, previews, variants, signature images, import
files, attachment packages) are stored by the backend `[storage]` selects
([configuration.md](configuration.md#storage)). The `api` writes uploads and the `worker` writes
job results, so both must use the same backend and, for `local`, the same directory. In Compose
both mount `./uploads` at `/app/uploads`, which must be writable by uid 1000.

To use an S3-compatible service:

```toml
[storage]
backend = "s3"

[storage.s3]
endpoint = "https://s3.eu-central-1.amazonaws.com"
bucket = "sylvode-uploads"
region = "eu-central-1"
access_key_id = "replace_with_access_key_id"
secret_access_key = "replace_with_secret_access_key"
```

`[storage.s3]` is left unread while `backend = "local"`, so it can stay in the file and the
backend switches by editing one line. Switching does not copy existing files.

## Database and migrations

Sylvode runs on PostgreSQL 18 only. The migrations are compiled into the `api` binary and applied
at every start: each migration runs once under a lock, in a transaction that also records it in
the `schema_migrations` ledger, so several API replicas can start together. After the run the API
checks the ledger and stops if a migration failed or is missing.

Two switches in `[migrations]` change this, for inspecting a damaged database; turn them back off
afterwards:

- `replay = true` runs every migration again, reports failures without stopping, and never
  downgrades a recorded success.
- `continue_on_error = true` starts the API although a migration failed or the check found a gap.
  The failures are recorded and retried at the next start.

The Compose database is created by the `postgres` image with `POSTGRES_DB: openpr` and
`POSTGRES_USER: openpr`, and is reachable only inside the Compose network. Do not publish its port
without a firewall and access plan of your own.

## Backup and restore

### Compose deployments

`scripts/backup-db.sh` dumps the Compose database with `pg_dump` into
`backups/openpr_backup_<timestamp>.sql.gz` in the repository and keeps the seven most recent
dumps:

```bash
bash scripts/backup-db.sh
```

`scripts/restore-db.sh` drops the database, recreates it and loads a dump. It asks for
confirmation first. Run it without an argument to list the available dumps:

```bash
bash scripts/restore-db.sh backups/openpr_backup_<timestamp>.sql.gz
docker compose restart api worker mcp-server
```

Both scripts need the `postgres` service to be running. Back up `./uploads` (or the S3 bucket)
separately: the database holds references to the stored files, not the files.

### Other deployments

Use `pg_dump` and `psql` from PostgreSQL 18; `pg_dump` refuses to dump a server newer than itself.
Stop the API and the worker, or at least all writes, while the dump is taken, so the Flow documents
in it are consistent.

### Verifying a Flow restore

`scripts/verify-flow-backup-restore.sh` checks that a backup restores every Flow collaboration
document exactly. It dumps the source database as plain SQL, restores the dump in one transaction
into a temporary database on the target cluster, rebuilds every document on both sides, and fails
unless both sides hold the same non-empty set of documents with the same head sequence, head
frontier, semantic hash and projection sequence. It then prints a JSON result with the dump
checksum, the document count and the restore time, and drops the temporary database.

It runs from a source checkout and needs a Rust toolchain (it builds a helper binary), and the
PostgreSQL 18 `pg_dump` and `psql`, `jq` and `sha256sum`.

| Variable | Meaning |
|---|---|
| `OPENPR_BACKUP_SOURCE_DATABASE_URL` | Required. The database to back up. |
| `OPENPR_BACKUP_RESTORE_ADMIN_URL` | Required. A connection URL to the target cluster's `postgres` database, with the right to create and drop databases. |
| `OPENPR_BACKUP_RESTORE_DATABASE_NAME` | Name of the temporary database. Default `restore_drill_main`; must start with `restore_drill_`. |
| `SYLVODE_SCRATCH_ROOT` | Directory for the dump and the comparison files. |

```bash
export OPENPR_BACKUP_SOURCE_DATABASE_URL='postgres://backup@db.example.com:5432/sylvode'
export OPENPR_BACKUP_RESTORE_ADMIN_URL='postgres://admin@restore.example.com:5432/postgres'
bash scripts/verify-flow-backup-restore.sh /srv/sylvode/restore-check.json
```

The first argument is where the JSON result is written. Keep credentials in the environment or a
PostgreSQL password file. Stop writes on the source while the script runs, and after restoring a
production database keep writes stopped until the check passes.

## Flow runtime

### Flow per workspace

Flow is on for every workspace unless a workspace administrator switches it off in the
workspace's Flow settings or with `PUT /api/v1/workspaces/{workspace_id}/features/flow`. The switch
is stored in `flow_workspace_settings`, whose row is created with the workspace. The authorization
checks lock that row, so do not delete rows from `flow_workspace_settings`. How users work with
Flow is described in [guide/flow.md](guide/flow.md).

### Runtime capabilities

Installation-wide Flow features are switched in the single row of `flow_runtime_capabilities`.
Every capability is on by default. A capability only allows a feature; every request is still
authorised per user.

| Column | Default | What it allows |
|---|---|---|
| `minimum_reader_version` | `5` | The reader version every running component must register (see [Flow reader leases](#flow-reader-leases)). Rich Pages need at least 4; block transfers need exactly 5; portable packages need at least 5. Do not set it above 5, and do not lower it on a database that has run at 5. |
| `personal_provisioning` | on | Creating a user's personal Flow space on first visit. |
| `personal_writes` | on | Any write in a personal space. Requires `personal_provisioning`. |
| `external_mounts` | on | Mounting personal content into a workspace and changing a mount's level. Requires `personal_writes`. |
| `personal_forms_conversion` | on | Converting personal Flow content into a workspace's Forms. |
| `cross_workspace_forms_conversion` | on | Converting Flow content into another workspace's Forms. |
| `block_transfers_enabled` | on | Moving or copying blocks between Pages. |
| `portable_page_packages_enabled` | on | Exporting and importing single-Page packages. |
| `portable_graph_packages_enabled` | on | Exporting and importing multi-object packages. |
| `personal_package_imports` | on | Importing packages into a personal space. |
| `personal_scope_package_exports` | on | Exporting a whole personal space as a package. |
| `scope_native_legacy` | on | Scope-native editor initialisation of workspace Pages. |

Capabilities are switched with SQL and take effect on the next request, without a restart. The
table's constraints require `external_mounts` to be off before `personal_writes`, and
`personal_writes` before `personal_provisioning`, so turn them off in one statement:

```sql
-- Turn personal Flow off entirely.
UPDATE flow_runtime_capabilities
   SET external_mounts = false, personal_writes = false, personal_provisioning = false,
       updated_at = now()
 WHERE singleton;

-- Turn off one independent feature.
UPDATE flow_runtime_capabilities SET block_transfers_enabled = false, updated_at = now() WHERE singleton;
```

Turning `personal_writes` and `personal_provisioning` off stops new personal spaces and every
personal write; existing personal
content stays readable to its owner and the people it is shared with, and owners can still revoke
shares. Turning `external_mounts` off stops new mounts and level changes but keeps the access that
existing mounts grant; remove those mounts to withdraw it.

The defaults are applied to the row only once, and the fact is recorded in
`flow_runtime_capability_rollouts`. Do not delete rows from that table: a missing row lets the next
migration run switch the capabilities on again, over the values you set.

### Flow reader leases

The `api` registers an `api` lease, and the `worker` a `worker` and a `maintenance` lease, in
`flow_runtime_readers`. Each process renews its leases every 10 seconds for 30 seconds. Page
creation and editing (in workspaces and personal spaces, through the web UI, the REST API, MCP or
the command line), every write in a personal space, block transfers and package export and import
are refused with `feature_disabled` and `details.reason = "scope_runtime_not_ready"` unless all
three components hold a live lease at or above `minimum_reader_version` and no live lease is below
it. Reading, workspace comments and objects other than Pages are not affected.

The worker is therefore required for editing Pages. A stopped process keeps its lease for up to 30
seconds, so a restart does not interrupt writes; if the worker stays down longer, Page writes are
refused until it is back. To see the live leases:

```sql
SELECT component, reader_version, lease_expires_at FROM flow_runtime_readers
 WHERE lease_expires_at > now() ORDER BY component;
```

### Incremental write window

`flow.incremental_window` is experimental and off by default. With it off, every Flow write is
checked by the full computation inside the 50 ms CPU window of `collab-isolated-apply-worker`. On a
Page with more than roughly 3,000 flat blocks the full computation can exceed that window on a slow
host, and such writes are refused by the CPU limit.

With it on, the check works from facts about the stored Page that are computed before the window
opens, and evaluates only the parts an update touched; updates it cannot be certain about still
take the full computation. The limits are the same. It is off by default because a crafted update
can make the preparation of a Page so costly that its later writes are refused, which the full
computation does not allow.

- The preparation may use `flow.setup_cpu_budget_ms` of CPU time (default 2500, range 500 to 4000).
  The budget depends on the machine; raise it on a slow host whose largest honest Pages lose the
  incremental window. Preparation is always cut after 5 seconds of wall time.
- With the incremental window on, an update that rewrites the logical id of an existing tree node
  is refused (`logical_id_rewritten`), and an update that would make one text too costly to decode
  is refused with the limit `text_decode_cost`.
- The API checks the worker's result against its own full projection of the document. A
  disagreement is logged at error level as `incremental_check_divergence` and the write is decided
  again by the full computation; report such lines with the document and update ids. A Page whose
  incremental checks disagree repeatedly is checked by the full computation for the next ten
  minutes (`window_cooldown`).
- The API reads the setting at startup; restart it after a change.

With either setting, an update whose tree operations name a parent that does not exist, or move a
node below itself, is refused as `invalid_update` with reason `tree_parent_missing` or
`tree_parent_cycle`.

## Background jobs

The worker runs these jobs in rounds: one round, then a pause of 5 seconds, then the next round.

| Job | Schedule | What it does |
|---|---|---|
| Bot operation record retention | at startup, then daily | Deletes bot operation records older than `audit.operation_log_retention_days`. |
| AI task timeouts | every round | Takes back delivered tasks that passed `ai_tasks.processing_timeout_secs`: queued again while attempts remain, failed otherwise. |
| AI task delivery | every round | Delivers pending bot tasks. |
| Form jobs | every round | Runs form import, export and attachment packaging jobs. |
| Governance | every round | Advances proposals through their open and voting stages and settles them. |
| Flow event delivery | every round | Expands pending Flow events into deliveries and sends them, up to `flow.dispatch_max_attempts` attempts. Logs a warning when the oldest pending event is too old. |
| Flow maintenance | every round | Search index projection and rebuild jobs, quarantine and compaction queues, document compaction, projection rebuilds, integrity scans and retention of deleted objects. |
| OAuth event feed | continuously | With `[oauth]` configured, moves events into the delegated event feed. With or without it, deletes feed rows older than `oauth.event_feed_retention_days` every 10 minutes, and once a day deletes OAuth audit events older than 180 days, OAuth operation logs older than 90 days, and authorization requests, codes and tokens that expired more than a day (tokens: seven days) ago. |
| E-mail delivery | every 10 seconds, with `[email]` enabled | Sends the e-mails queued in the outbox (see [E-mail](#e-mail)). |
| Daily digests | every 5 minutes, with `[email]` enabled | Queues one digest e-mail for each user on the daily summary who has not had today's yet, once `email.digest_hour_utc` has passed. |
| Alert follow-up | every `inbound.followup_interval_secs` | Reports the outcome of alert tasks, starts queued alert tasks as capacity frees up, and closes finished alerts. Deletes stored inbound events older than `inbound.event_retention_days` once an hour. |

`worker --concurrency <n>` (default 4) scales the batch sizes of these jobs.

## E-mail

With `[email] enabled = true` ([configuration.md](configuration.md#email)) the API queues a row in
the `email_outbox` table in the same transaction that creates a notification, and the worker sends
it. A row is `pending`, `sending`, `sent` or `failed`. The worker takes up to 20 due rows every 10
seconds, with a lease of 2 minutes, so several workers can run. A failed attempt is tried again after
1 minute, 5 minutes, 30 minutes and 2 hours; after the fifth the row is `failed`. A host that the
[outbound address policy](#outbound-address-policy) refuses fails the row at once, with
`last_error` "refused by the outbound address policy". `last_error` holds at most 500 characters and
never the SMTP password. Rows stay after sending, as a record.

To see what happened, query the table:

```sql
SELECT kind, status, attempts, next_attempt_at, last_error FROM email_outbox
 WHERE status <> 'sent' ORDER BY created_at DESC LIMIT 20;
```

To check the setup, an instance administrator sends a test e-mail to their own address with
`POST /api/v1/admin/email/test`; it answers `{"queued": true}` and the row of kind `test` shows the
outcome a few seconds later. If `[email]` is not enabled the call answers 409 `feature_disabled`.

## Outbound address policy

Webhook deliveries, AI task deliveries, Flow event deliveries and the runner address of a bot
connection are refused when the target resolves to a private, loopback or other internal address,
and redirects are not followed. To reach internal services, list them in
`outbound.allowed_hosts` as `host` or `host:port`. The generated Compose file allows the services of
its own network: `runner:9090`, `api:8080`, `mcp-server:8090` and `frontend:80`. The SMTP host of
`[email]` is checked by the same rules; a relay on a private address has to be listed here. See
[configuration.md](configuration.md#outbound) and [integrations/webhooks.md](integrations/webhooks.md).
