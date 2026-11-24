# Configuration reference

This page lists every key of the Sylvode configuration file, the environment variables the
server binaries read, and the variables of the Docker Compose stack. The runner has its own
file and its own reference in [apps/runner/README.md](../apps/runner/README.md).

For installation steps see [install.md](install.md); for running and maintaining a deployment see
[operations.md](operations.md). The annotated example
[config/sylvode.example.toml](../config/sylvode.example.toml) carries the same keys with short
comments.

## The configuration file

The `api`, `worker` and `mcp-server` binaries, and the `sylvode` command line, read one TOML file.

| Topic | Behaviour |
|---|---|
| Location | The path given with `--config <path>`. Without the flag, `config/sylvode.toml` relative to the process working directory. |
| Missing file | Startup error. No binary falls back to built-in defaults for a missing file. |
| Unknown keys | Every section refuses keys it does not know, so a misspelled key stops the process at startup. |
| Placeholders | A value that is empty, contains `replace_with`, `change-me-in-production` or an unexpanded `${...}` template is refused wherever the key expects a concrete value (URLs, secrets, names, UUIDs). |
| Error report | Validation reports every unusable value of the file in one error, so one round of edits fixes the whole file. |
| Shape and presence | Every value that is present is checked for shape whichever binary loads the file. Whether a required value is present is checked only by the binary that needs it: the `mcp-server` needs no `[database]` and no `[auth]`. |
| One file, three binaries | Each binary reads the sections it needs and ignores the rest. The Compose stack uses two files instead, one for `api` and `worker` and one for `mcp-server`, so the MCP server never holds the database URL or the signing key (see [install.md](install.md#docker-compose)). |
| Offline check | `bash scripts/start.sh --check-config` validates the two Compose files and `.env` without starting anything. |

### Which binary reads which section

| Section | Read by |
|---|---|
| `[server]` | `api`. The `worker` uses `app_name` in its log lines and `bind_addr` only for the `auth.allow_insecure_cookies` check; it opens no listener. |
| `[database]` | `api`, `worker` (both require `url`) |
| `[auth]` | `api`, `worker` (both require `jwt_secret`) |
| `[logging]` | `api`, `worker`, `mcp-server`, `sylvode` |
| `[storage]`, `[storage.s3]` | `api`, `worker` |
| `[audit]` | `worker` |
| `[ai_tasks]` | `api`, `worker` |
| `[migrations]` | `api` |
| `[outbound]` | `api`, `worker` |
| `[mcp]` | `mcp-server`, `sylvode` |
| `[flow]` | `api`, `worker` |
| `[oauth]` | `api`, `worker`; when both `[oauth]` and `[mcp]` are in one file, their shared values must agree |
| `[inbound]` | `api`, `worker` |
| `[email]` | `api` (whether e-mail is available, the links), `worker` (sends the e-mails) |

## `[server]`

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `app_name` | string | the binary's own name | Service name in log lines and in the `/health` response. Must be concrete. Leave it out of a file shared by `api` and `worker`, so each keeps its own name. |
| `bind_addr` | string | `0.0.0.0:8081` | `host:port` the `api` listens on. IPv6 hosts in brackets. Port 1 to 65535. |
| `public_url` | string | none | Absolute `http` or `https` origin without a path, for example `https://sylvode.example.com`, that people reach the server at. Links in e-mails start with it. Required when `[email] enabled = true`. |
| `trusted_proxies` | array of strings | `["127.0.0.0/8", "::1/128", "172.16.0.0/12"]` | IP addresses or CIDR blocks of reverse proxies whose `X-Forwarded-For` and `X-Real-IP` headers the `api` believes when it needs the client address (per-address rate limits and allow-lists). Headers from any other peer are ignored. Setting the key replaces the default list. The default covers loopback and the address range Docker gives the Compose bridge network; Podman and custom networks use other ranges. |

## `[database]`

PostgreSQL 18 connection and pool settings.

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `url` | string | none | Required by `api` and `worker`. Full connection URL, password included, for example `postgres://sylvode:secret@localhost:5432/sylvode`. Never logged. |
| `max_connections` | integer | `20` | At least 1. |
| `min_connections` | integer | `2` | Must not exceed `max_connections`. |
| `connect_timeout_seconds` | integer | `5` | Seconds, at least 1. |
| `idle_timeout_seconds` | integer | `30` | Seconds, at least 1. |
| `acquire_timeout_seconds` | integer | `5` | Seconds, at least 1. |

## `[auth]`

Token signing and issuance.

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `jwt_secret` | string | none | Required by `api` and `worker`. Signs every access and refresh token. At least 16 characters; generate one with `openssl rand -hex 32`. Changing it invalidates every issued token. Never logged. |
| `access_ttl_seconds` | integer | `1296000` (15 days) | Greater than 0. |
| `refresh_ttl_seconds` | integer | `1728000` (20 days) | Greater than 0. |
| `allow_insecure_cookies` | boolean | `false` | `true` drops the `Secure` attribute from the authentication cookies, for local development over plain `http://`. `api` and `worker` refuse to start with `true` unless `server.bind_addr` (or the binary's default when it is unset) is a loopback host: `127.0.0.1`, `::1` or `localhost`. |

## `[logging]`

The server binaries do not read `RUST_LOG`; logging is configured here.

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `filter` | string | `<service>=info,tower_http=info` | `tracing-subscriber` filter directives, for example `api=info,tower_http=info,sea_orm=warn`. A filter that does not parse stops the process at startup. |
| `format` | string | `json` | `json` (one JSON object per line) or `text` (`pretty` and `plain` are accepted as `text`). |
| `output` | string | `stderr` | `stderr` or `stdout`. Honoured by `api` and `worker`. `mcp-server` and `sylvode` always log to stderr, because stdout carries the `stdio` protocol and command output. |

## `[storage]`

Where uploaded files and the files generated from them (thumbnails, previews, import files,
attachment packages) are stored. `api` and `worker` must see the same storage.

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `backend` | string | `local` | `local` (also `filesystem`, `fs`) or `s3` (also `s3-compatible`, `s3_compatible`). |
| `dir` | string | `./uploads` | Root directory of the local backend, relative to the working directory unless absolute. Must not be empty. Ignored when `backend` is `s3`. |
| `s3` | table | none | The `[storage.s3]` section. Required when `backend` is `s3`; left unread when `backend` is `local`. |

### `[storage.s3]`

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `endpoint` | string | none | Required. `http://` or `https://` URL of the S3-compatible service. |
| `bucket` | string | none | Required. 3 to 63 characters: lowercase letters, digits, dots and dashes, starting and ending with a letter or digit. |
| `region` | string | `us-east-1` | Must be concrete when set. |
| `access_key_id` | string | none | Required. Never logged. |
| `secret_access_key` | string | none | Required. Never logged. |
| `session_token` | string | none | Optional, for temporary credentials. Never logged. |

## `[audit]`

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `operation_log_retention_days` | integer | `30` | 1 to 3650. The `worker` deletes bot operation records older than this, once at startup and then once a day. |

## `[ai_tasks]`

How long work handed to a bot may run. The task lifecycle is described in
[guide/agents.md](guide/agents.md).

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `processing_timeout_secs` | integer | `3600` | 60 to 604800. A delivered task that is neither completed nor failed within this time is taken back by the `worker`: queued again while attempts remain, failed otherwise. |
| `lease_extension_secs` | integer | `1800` | 60 to 86400. Each acknowledge, progress report or runner heartbeat moves an active task's deadline to this many seconds from now. |
| `max_processing_secs` | integer | the larger of `86400` and `processing_timeout_secs` | From `processing_timeout_secs` to 604800. No extension moves a deadline past this many seconds after the task started. |
| `max_chain_depth` | integer | `3` | 1 to 10. Deepest hop of a bot-to-bot chain. A task started by a person or an inbound alert has depth 0; a task a bot causes while it runs a task is one deeper. A hop beyond this depth creates no task. |

## `[migrations]`

The `api` applies the database migrations at startup. Both switches default to `false`, which is
the setting for normal operation; see [operations.md](operations.md#database-and-migrations).

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `replay` | boolean | `false` | Re-execute every migration once, reporting failures without stopping. A ledger row that records a success is never downgraded. |
| `continue_on_error` | boolean | `false` | Start even though a migration failed or the schema check found a gap. The failures are recorded and retried at the next start. |

## `[outbound]`

What the outgoing HTTP paths may call: webhook deliveries, AI task deliveries, Flow event
deliveries and the runner address of a bot connection. Requests to private, loopback and other
internal addresses are refused unless allowed here.

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `allowed_hosts` | array of strings | `[]` | Hosts exempt from the private address check, as `host` or `host:port`. Matched literally and case-insensitively: no URLs, no paths, no wildcards. |
| `allow_private` | boolean | `false` | Turns the private address check off entirely. Only for a closed network you control end to end. |

## `[mcp]`

Settings of the MCP server and the `sylvode` command line. How the transports authenticate and what
the tool modes list is described in [apps/mcp-server/README.md](../apps/mcp-server/README.md); the
command line in [integrations/cli.md](integrations/cli.md).

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `api_url` | string | `http://localhost:8081` | `http://` or `https://` URL of the Sylvode API. |
| `bot_token` | string | none | Workspace bot token with the `opr_` prefix. Required by the `stdio` transport and by the `sylvode` workspace commands. Not used by the `http` and `sse` transports, where every request carries its caller's own bot token. Never logged. |
| `workspace_id` | string (UUID) | none | Non-nil UUID of the workspace the calls are scoped to. Required by the `stdio` transport and by the `sylvode` workspace commands. Not used by the `http` and `sse` transports, where each request acts in the workspace its caller's own bot token belongs to. |
| `transport` | string | `stdio` | `stdio`, `http` or `sse`. The `--transport` flag of `mcp-server serve` takes precedence. |
| `bind_addr` | string | `127.0.0.1:8090` | `host:port` for the `http` and `sse` transports. The `--bind-addr` flag takes precedence. |
| `oauth_issuer` | string | none | Issuer of delegated tokens; must equal `oauth.issuer` when both are in one file. `https`, or `http` on a loopback IP literal. |
| `oauth_resource` | string | none | The URL that receives JSON-RPC posts, as clients reach it; must equal `oauth.mcp_resource` when both are in one file. |
| `oauth_forward_secret` | string | none | Credential presented to the API when the server forwards a delegated token; equal to `oauth.internal_forward_secret`. At least 32 bytes. Never logged. |
| `tool_mode` | string | `search` | `search` or `full`. Which tools the server lists; see the MCP server README. |
| `allow_write_call` | boolean | `true` | `false` disables every write tool except the task lifecycle tools. Reads are not affected. |

`oauth_issuer`, `oauth_resource` and `oauth_forward_secret` are set together or not at all; a
partial set stops the server and names the missing keys. Without them the server accepts
workspace bot tokens only.

## `[flow]`

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `dispatch_max_attempts` | integer | `10` | Greater than 0. Attempts to deliver one Flow event before it is given up. |
| `collab_allowed_origins` | array of strings | `[]` | Browser origins allowed to open live editing sessions, each `http://` or `https://` `host[:port]` with no path and no wildcard. A collaboration ticket request and the WebSocket upgrade are refused unless their `Origin` header is listed. Empty refuses every live editing session: list the origin users reach the web UI at, for example `https://sylvode.example.com`. |
| `conversion_reap_interval_secs` | integer | `60` | 1 to 86400. Seconds between two sweeps that fail conversion jobs left `started` for more than 15 minutes, so they can be retried. The `api` also sweeps once at startup. |
| `body_asset_max_bytes` | integer | `52428800` (50 MiB) | 1 to 52428800. Largest file a Page body asset may have. |
| `incremental_window` | boolean | `false` | Experimental. Lets the Flow write check work from facts about the stored Page and the parts an update touched, instead of the full computation for every write. See [operations.md](operations.md#incremental-write-window). Read at startup. |
| `setup_cpu_budget_ms` | integer | `2500` | 500 to 4000. CPU milliseconds the write check's preparation may use before the write falls back to the full computation. Only matters with `incremental_window = true`. |

## `[oauth]`

The OAuth authorization server for connected applications. Without `issuer` the feature is off:
the discovery documents and every OAuth endpoint answer 404 and no delegated token is accepted.
How to enable and use it is described in [integrations/oauth.md](integrations/oauth.md).

URLs in this section must use `https`. Plain `http` is accepted only for a loopback IP literal
(`127.0.0.1` or `[::1]`) and only with `allow_insecure_loopback_issuer = true`.

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `issuer` | string | none | Turns the feature on. The public origin of this Sylvode as clients reach the web UI: scheme, host and optional port, no path, query or fragment. A loopback `http` issuer must name a port. Required when any other key of this table except the limits is set. |
| `api_resource` | string | `<issuer>/api` | Identifier of the API as a protected resource. |
| `mcp_resource` | string | none | Identifier of the MCP server as a protected resource: the URL that receives JSON-RPC posts. Set together with `internal_forward_secret`. |
| `internal_forward_secret` | string | none | Credential the MCP server presents when it forwards a delegated token. At least 32 bytes. Set together with `mcp_resource`. Never logged. |
| `allow_insecure_loopback_issuer` | boolean | `false` | Local development only: allows `http` URLs on `127.0.0.1` or `[::1]`. Refused with any other issuer host. |
| `access_token_ttl_seconds` | integer | `600` | 60 to 3600. |
| `refresh_token_idle_days` | integer | `30` | 1 to 90. A refresh token unused for this long expires. |
| `grant_max_lifetime_days` | integer | `365` | 1 to 730. A connection ends after this time whatever its use. |
| `authorization_code_ttl_seconds` | integer | `60` | 10 to 600. |
| `max_active_grants_per_user_client` | integer | `10` | 1 to 50. |
| `event_feed_retention_days` | integer | `7` | 1 to 30. Days the delegated event feed is kept. |
| `event_backfill_max_events` | integer | `1000` | 100 to 10000. How many events a resuming event stream may be behind. |
| `event_backfill_max_age_hours` | integer | `72` | 1 to 168. How many hours a resuming event stream may be behind. |
| `max_streams_per_process` | integer | `2000` | 10 to 20000. Concurrent event streams per `api` process. |

## `[inbound]`

The inbound endpoint of the alert chain, `POST /api/v1/inbound/events`. With `enabled = false` it
answers 503 to every caller. Sources, credentials and rules are described in
[integrations/alerts.md](integrations/alerts.md). Every limit has an accepted range, so a limit can
be tightened but not removed.

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `enabled` | boolean | `false` | Whether the endpoint answers at all. |
| `max_body_bytes` | integer | `65536` | 4096 to 262144. Largest request body the endpoint reads. |
| `signature_tolerance_secs` | integer | `300` | 30 to 900. Allowed difference between a signed request's timestamp and the server clock. |
| `auth_failures_per_minute_per_ip` | integer | `30` | 1 to 1000. Failed authentications one client address may make per minute. |
| `max_active_alert_tasks_per_workspace` | integer | `5` | 1 to 100. Bot tasks started by alerts that may be active in one workspace at once. |
| `max_alerts_per_rule_per_hour` | integer | `60` | 1 to 10000. Work items one rule may open per hour. |
| `event_retention_days` | integer | `30` | 1 to 365. Days stored inbound events are kept. |
| `followup_interval_secs` | integer | `2` | 1 to 60. How often the `worker` checks alert tasks for outcomes and starts queued ones. |

## `[email]`

Notification e-mails over SMTP, plain text only. The section is optional and off by default. With
`enabled = false` nothing is sent, the notification settings tell people that e-mail is not set up on
this server, and the test e-mail endpoint answers 409 `feature_disabled`. With `enabled = true` the
`api` and the `worker` refuse to start unless `smtp_host`, `from` and `[server] public_url` are
present and well formed; messages name the key and never repeat its value.

| Key | Type | Default | Accepted values and notes |
|---|---|---|---|
| `enabled` | boolean | `false` | Turns delivery on. |
| `smtp_host` | string | none | Host name or IP address of the SMTP server, without scheme, port or path. Required when enabled. |
| `smtp_port` | integer | 587 for `starttls`, 465 for `implicit`, 25 for `none` | 1 to 65535. |
| `tls` | string | `starttls` | `starttls` (the upgrade is required, the connection fails without it), `implicit` (TLS from the first byte) or `none`. `none` is accepted only when `smtp_host` is `localhost` or a loopback address. |
| `username` | string | none | With `password`, logs in to the server. |
| `password` | string | none | Needs `username`. Never logged, never shown in an error, never stored in the outbox. |
| `from` | string | none | RFC 5322 mailbox, for example `Sylvode <sylvode@example.com>`. Required when enabled. |
| `ca_file` | string | none | PEM file with certificates added to the trust roots, for a server with a private CA. The `worker` fails to start if it cannot read it. |
| `digest_hour_utc` | integer | `8` | 0 to 23. Hour (UTC) after which each day's digest is sent. |

The `worker` resolves `smtp_host` through the [`[outbound]`](#outbound) address rules and connects
only to addresses that pass them, checking the certificate against `smtp_host`. A relay on a private
or loopback address must be listed in `outbound.allowed_hosts` (as `host` or `host:port`); a message
for a host the rules refuse is marked `failed` at once. See [operations.md](operations.md#e-mail)
for the outbox.

## Environment variables read by the server binaries

Apart from the configuration file, the `api` reads these variables. The `worker` and
`mcp-server` read none.

| Variable | Read by | Effect |
|---|---|---|
| `SV_FORM_PLUGIN_PACKAGES_ENABLED` | `api` | `1` enables form plugin packages and personal plugin libraries. Any other value, or unset, leaves them disabled. |
| `SV_FORM_PLUGIN_EXTENSIONS_ENABLED` | `api` | `1` enables protocol 3 form plugin packages and the extension endpoints. Protocol 3 packages need both variables set to `1`. See [plugins/README.md](plugins/README.md). |
| `COLLAB_ISOLATED_APPLY_WORKER_PATH` | `api` | Path of the `collab-isolated-apply-worker` executable. Only needed when it is not in the same directory as `api`. |

The runner reads its own variables; see [apps/runner/README.md](../apps/runner/README.md).

## Docker Compose variables

`docker-compose.yml` takes these values from the environment or from `.env` in the repository
root. `scripts/start.sh` creates `.env` from `.env.example` when it is missing.

| Variable | Default | Effect |
|---|---|---|
| `POSTGRES_PASSWORD` | none (required) | Password the `postgres` image initialises the database with, on the first start of the data volume only. Must match the password inside `database.url`. `scripts/start.sh` generates one when it is unset. |
| `SYLVODE_BIND_HOST` | `127.0.0.1` | Host interface every published port binds to. |
| `SYLVODE_API_PORT` | `8081` | Host port of the API (container port 8080). |
| `SYLVODE_FRONTEND_PORT` | `3000` | Host port of the web UI (container port 80). |
| `SYLVODE_MCP_PORT` | `8090` | Host port of the MCP server. |
| `SYLVODE_RUNNER_PORT` | `9090` | Host port of the optional runner. |
| `SYLVODE_RUNNER_IMAGE` | `ghcr.io/openprx/sylvode-runner:latest` | Runner image. Pin a release version in production. |
| `SYLVODE_RUNNER_CONFIG` | `./config/sylvode-runner.example.toml` | Runner configuration file, mounted at `/etc/sylvode-runner/config.toml`. |
| `SYLVODE_RUNTIME_BASE` | `debian:bookworm-slim` | Base image of the `api`, `worker` and `mcp-server` images, which copy binaries built on the host; it must ship a glibc at least as new as the host's. `scripts/start.sh` sets it to the host release on Debian hosts. |
| `SYLVODE_FRONTEND_DOCKERFILE` | `Dockerfile` | `Dockerfile` builds the web UI inside the image; `Dockerfile.prebuilt` serves an existing `frontend/build`. |
| `SYLVODE_APP_CONFIG_PATH` | `./config/sylvode.compose.toml` | Configuration file mounted into `api` and `worker`. `scripts/start.sh` always sets this value. |
| `SYLVODE_MCP_CONFIG_PATH` | `./config/sylvode.compose.mcp.toml` | Configuration file mounted into `mcp-server`. `scripts/start.sh` always sets this value. |
| `SV_FORM_PLUGIN_PACKAGES_ENABLED` | `0` | Passed to the `api` container. Enables personal plugin libraries and protocol 3 form plugin packages. |
| `SV_FORM_PLUGIN_EXTENSIONS_ENABLED` | `0` | Passed to the `api` container. Enables the protocol 3 package extensions; the Plugin Creator needs this and the packages switch. |

`scripts/start.sh` also reads `SYLVODE_COMPOSE_STAGED`: with `1` it builds the images first and then
starts `postgres`, `api`, `worker`, `mcp-server` and `frontend` one by one, for Compose providers
that lose a dependency when they build and start in one step.

The web UI served by Compose reaches the API from its own origin. To build or run the web UI
yourself against another API address, set `VITE_API_BASE_URL` in `frontend/.env` (see
[frontend/README.md](../frontend/README.md)); the root `.env` does not read it.
