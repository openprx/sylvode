# What the shared test environment needs before the suite can run

Values are for UI `http://127.0.0.1:18097` and API `http://127.0.0.1:18083`. Secrets are never
written down here: generate each with the command shown and put it in the files named. The sources
are contract section 17, `docs/runbooks/oauth-authorization-server.md`, and the code
(`crates/platform/src/config/{oauth.rs,mod.rs,raw.rs}`, `apps/api/src/main.rs`,
`apps/worker/src/oauth_feed.rs`, `apps/mcp-server/src/{main.rs,oauth.rs}`).

## Order

1. Build the binaries of this branch (api, worker, **mcp-server**) and the frontend.
2. Add the `[oauth]` section to the API and worker configuration; start the new API (it migrates)
   and the new worker.
3. Run the suite's setup (`node tests/e2e/oauth/setup.ts`) so that workspace W exists.
4. Write the MCP server's own configuration with W's id; start the MCP server.
5. Start the local web proxy for the UI origin.
6. Check readiness (below), then run the suite.

## 1. Binaries and frontend

`build-backend.sh` of the existing deployment builds `api`, `worker` and
`collab-isolated-apply-worker` from `/Users/nn/worker/llama/ops/sylvode-next`. For this suite:

- point its `src` at the worktree that holds this branch (`/Users/nn/worker/llama/ops/sylvode-p4`, or
  the branch once merged), because the OAuth code is only there;
- add the MCP server, which that script does not build:
  `cargo rustc --locked -p mcp-server --bin mcp-server -- $LINK` followed by
  `cp /target/debug/mcp-server /out/mcp-server` (same builder image, same `$LINK` flags);
- keep `OPENPR_BUILD_GIT_COMMIT`, `OPENPR_BUILD_GIT_DIRTY` and `OPENPR_BUILD_GIT_COMMITTER_DATE`
  exported as that script does. The API does not expose its build commit over HTTP; read it with
  `/release/bin/api --build-info` and pass it to the suite as `OAUTH_E2E_BUILD_COMMIT`.
- frontend: `cd frontend && bun install --frozen-lockfile && bun run build` on the same commit
  (it contains the consent page `/oauth/authorize` and the three "Connected applications" sections).

## 2. API and worker configuration (`sylvode.toml`, the file both mount)

Add (keys of `[oauth]`; only `issuer` is mandatory, the rest is what this environment needs):

| Key | Value for this environment |
|---|---|
| `issuer` | `"http://127.0.0.1:18097"` (the UI origin, no path) |
| `allow_insecure_loopback_issuer` | `true` (required for an `http` issuer and for an `http` `mcp_resource`; refused for any non-loopback host) |
| `api_resource` | omit (defaults to `http://127.0.0.1:18097/api`); this is the API audience |
| `mcp_resource` | `"http://127.0.0.1:<MCP port>/mcp/rpc"`, the URL that receives JSON-RPC posts, for example port 18098. Must equal `[mcp] oauth_resource` of the MCP server. Required together with the secret |
| `internal_forward_secret` | generate with `openssl rand -hex 32` (64 characters; at least 32 bytes are required, a placeholder is refused). **Identical** to `[mcp] oauth_forward_secret` in the MCP server's file. A mismatch shows as every MCP call answering 401 |
| `access_token_ttl_seconds` | omit (600); the suite refreshes as needed |
| `refresh_token_idle_days`, `grant_max_lifetime_days`, `authorization_code_ttl_seconds` | omit (30, 365, 60) |
| `max_active_grants_per_user_client` | omit (10); the suite keeps user A below it |
| `event_feed_retention_days` | omit (7) |
| `event_backfill_max_events`, `event_backfill_max_age_hours` | omit (1000, 72); A10 resumes one position behind |
| `max_streams_per_process` | omit (2000) |
| `embed_frame_ancestors` | omit (not used) |

Also needed, and normally already true for a UI served over `http` on loopback:
`[auth] allow_insecure_cookies = true` (the consent page and the settings pages sign in with a
cookie; the API refuses the flag unless `[server] bind_addr` is a loopback host as the API sees it, which
inside a container with a published port is a question for whoever wrote the existing file).

Other facts from the code:

- **The worker needs the same `[oauth]` section.** The single writer that moves events into the feed
  (`apps/worker/src/oauth_feed.rs`) only runs when `[oauth]` is configured; without it the worker only
  prunes. Both containers of the existing deployment already mount the same file, so one edit covers
  both, but both must be restarted (the configuration is read at start).
- The worker must be running for A08 (it picks up AI tasks) and for every stream event (A07 to A17).
  No new worker flag is needed (`--concurrency 1` as in `launch-p3-stack.sh`).
- The API container needs no new environment variable: `launch-p3-stack.sh`'s `docker run` line is
  unchanged except for the binaries. The binaries read no environment variables for any of this.
- A partial `[mcp]` set or a mismatch between `[mcp]` and `[oauth]` (issuer, resource) stops the
  process at start and names the key.

## 3. Migration

`migrations/0109_oauth_authorization_server.sql` is embedded in the API binary and applied by the
API itself at start, inside `run_migrations` (ledger table `schema_migrations`; a database that
already holds the objects is adopted, and re-running the file is harmless). Nothing is run by hand.
It creates twelve tables (`oauth_*` and `workspace_delegation_settings`), seeds the `ramflux` client
row and four triggers on `notifications` and `ai_tasks`. It takes a `SHARE ROW EXCLUSIVE` lock on
those two tables for a few milliseconds under `lock_timeout = 10s`; on a disposable database nothing
else is needed. To confirm after the first start:

```
SELECT name, status FROM schema_migrations WHERE name = '0109_oauth_authorization_server.sql';
SELECT client_id, status FROM oauth_clients;
```

The first row must read `applied` (or `adopted`), the second `ramflux | active`.

## 4. MCP server

Yes, a process must run: A02, A03, A04 and A06 call it, and A20 runs a script against it. It is a
separate binary with a separate configuration file that holds no database URL and no signing key
(the compose file mounts `config/sylvode.compose.mcp.toml` for it).

Configuration (`[logging]` is optional):

| Key | Value |
|---|---|
| `[mcp] transport` | `"http"` |
| `[mcp] bind_addr` | `"127.0.0.1:<MCP port>"` as a host process, or `"0.0.0.0:8090"` in a container with `-p 127.0.0.1:<MCP port>:8090` |
| `[mcp] api_url` | the API as the MCP process reaches it: `http://127.0.0.1:18083` for a host process, or `http://sv-p3-api:18083` on the container network of `launch-p3-stack.sh` |
| `[mcp] workspace_id` | the id of workspace W (mandatory to start; the setup prints it). Delegated callers use their grant's workspace instead; this value is what the bot-token script uses |
| `[mcp] oauth_issuer` | `"http://127.0.0.1:18097"`, equal to `[oauth] issuer` |
| `[mcp] oauth_resource` | equal to `[oauth] mcp_resource` |
| `[mcp] oauth_forward_secret` | the same secret as `[oauth] internal_forward_secret` |
| `[mcp] bot_token` | not used by the `http` transport |

Start: `mcp-server serve --config <that file> --transport http --bind-addr <address>` (binary
`mcp-server`; flags as in `docker-compose.yml`). Health: `GET /health` answers `ok`.

How the suite reaches it: `OAUTH_E2E_MCP_ORIGIN=http://127.0.0.1:<MCP port>`; the MCP resource it
posts to is `<origin>/mcp/rpc` unless `OAUTH_E2E_MCP_RESOURCE` says otherwise. The MCP origin is
reached **directly**, not through the UI proxy: the proxy forwards only the one well-known MCP
document and not the JSON-RPC posts.

For A20's bot-token script: create a workspace bot in W as the administrator
(`POST /api/v1/workspaces/<W>/bots` with `"transport_surface": "mcp_http"` returns its `opr_` token once; a token bound to the default REST transport is refused by the API with "bot credential is not valid for the presented transport" when it arrives through the MCP server) and export it as
`OAUTH_E2E_OPR_BOT_TOKEN`. The script also needs `python3` (3.11 or later) and `curl`.

## 5. The local web proxy (`scripts/lib/e2e-web-proxy.ts`, run with Bun)

```
E2E_PROXY_BUILD_DIR=<repo>/frontend/build \
E2E_PROXY_API_ORIGIN=http://127.0.0.1:18083 \
E2E_PROXY_PORT=18097 \
bun scripts/lib/e2e-web-proxy.ts
```

- `E2E_PROXY_BUILD_DIR`, `E2E_PROXY_API_ORIGIN` and `E2E_PROXY_PORT` are required; `E2E_PROXY_HOST`
  defaults to `127.0.0.1`.
- It must be the proxy of this branch: it forwards `/.well-known/oauth-authorization-server` and
  `/.well-known/oauth-protected-resource/api` to the API, serves `/oauth/*` as the single page app with
  `frame-ancestors 'none'` and no-store, and passes the event stream through with the idle timeout
  switched off (a plain proxy that buffers or cuts after 10 s breaks A07 to A17).
- `E2E_PROXY_MCP_ORIGIN` only enables `/.well-known/oauth-protected-resource/mcp`. This suite uses an
  MCP resource on the MCP origin, so leave it unset; set it only if the MCP resource is made to be
  `http://127.0.0.1:18097/mcp`, which the proxy cannot serve for JSON-RPC anyway.
- Everything the browser and the client send to the UI origin (consent page, `/api/*`) must reach
  the API unchanged: the decision endpoint refuses an `Origin` other than the issuer, which is why
  the issuer must be the UI origin.

## 6. Readiness: three commands and what each must return

```
curl -s -i http://127.0.0.1:18097/.well-known/oauth-authorization-server
curl -s -i http://127.0.0.1:18097/.well-known/oauth-protected-resource/api
curl -s -i http://127.0.0.1:18083/health
```

1. `HTTP 200`, JSON with `"issuer":"http://127.0.0.1:18097"` byte for byte,
   `"authorization_endpoint":"http://127.0.0.1:18097/oauth/authorize"`,
   `"token_endpoint":"http://127.0.0.1:18097/api/v1/oauth/token"`,
   `"revocation_endpoint":"http://127.0.0.1:18097/api/v1/oauth/revoke"`,
   `"code_challenge_methods_supported":["S256"]` and
   `"authorization_response_iss_parameter_supported":true`. A 404 means `[oauth] issuer` is not set in
   the running API, or the proxy does not route the path (an old proxy).
2. `HTTP 200`, JSON with `"resource":"http://127.0.0.1:18097/api"` and
   `"authorization_servers":["http://127.0.0.1:18097"]`. A 404 has the same two causes.
3. `HTTP 200`, `{"code":0,"message":"success","data":{"status":"ok","service":"..."}}`. This shows the
   API is up; it does not show the migration (see section 3) or the worker.

The MCP server has its own two checks: `curl -s http://127.0.0.1:<MCP port>/health` answers `ok`, and
`curl -s http://127.0.0.1:<MCP port>/.well-known/oauth-protected-resource` answers 200 with
`"resource"` equal to `[mcp] oauth_resource` (a 404 means the three `oauth_*` keys are not set). The
suite's first case asserts all of these again, so a wrong setting fails A01 with the reason.
