# OAuth authorization server

Sylvode can act as an OAuth 2.1 authorization server for its own API and MCP server. One client,
Ramflux, is registered by the migration; there is no registration endpoint. The feature is off
until `[oauth] issuer` is configured.

## Switching it on

Add an `[oauth]` section (see `config/sylvode.example.toml`). Only `issuer` is required:

```toml
[oauth]
issuer = "https://sylvode.example.com"   # https, no path
```

Local development on loopback may use `issuer = "http://127.0.0.1:<port>"` together with
`allow_insecure_loopback_issuer = true`. The flag is refused for any other host.

### Configuration keys

| Key | Default | Range | Meaning |
|---|---|---|---|
| `[oauth] issuer` | none (feature off) | https origin, no path | Public origin; appears byte for byte in metadata and in every `iss` |
| `api_resource` | `<issuer>/api` | URL | Identifier of the API as a protected resource (audience of API tokens) |
| `mcp_resource` | none | URL | Identifier of the MCP server; needed with `internal_forward_secret` for delegated MCP access |
| `internal_forward_secret` | none | at least 32 bytes | Credential the MCP server presents when it forwards a delegated token; never logged |
| `allow_insecure_loopback_issuer` | `false` | bool | Development only; loopback `http` issuer |
| `access_token_ttl_seconds` | 600 | 60 to 3600 | Access token lifetime |
| `refresh_token_idle_days` | 30 | 1 to 90 | A refresh token unused for this long expires |
| `grant_max_lifetime_days` | 365 | 1 to 730 | A connection ends after this, always |
| `authorization_code_ttl_seconds` | 60 | 10 to 600 | Authorization code lifetime |
| `max_active_grants_per_user_client` | 10 | 1 to 50 | Connections one person may hold for one client |
| `event_feed_retention_days` | 7 | 1 to 30 | How long delivered events stay available for resume |
| `event_backfill_max_events` | 1000 | 100 to 10000 | More matching events behind a cursor means `resync_required` |
| `event_backfill_max_age_hours` | 72 | 1 to 168 | A cursor older than this means `resync_required` |
| `max_streams_per_process` | 2000 | 10 to 20000 | Open event streams per API process |
| `embed_frame_ancestors` | none | origins | Reserved for cross-origin embedding (not built) |
| `[mcp] oauth_issuer`, `oauth_resource`, `oauth_forward_secret` | none | as above | The MCP server's side: equal to `[oauth] issuer`, `mcp_resource`, `internal_forward_secret` |

A partial `[mcp]` set refuses to start and names the missing keys.

### The forwarding secret

Delegated MCP access works because the MCP server forwards the caller's MCP-audience token to the
API together with `X-Sylvode-Forwarded-By: mcp-server` and `X-Sylvode-Forward-Auth: <secret>`. The
API accepts an MCP-audience token only with the right secret, and answers the MCP server's
token-context lookup only for it (everyone else gets 404). So `[oauth] internal_forward_secret` in
the API configuration and `[mcp] oauth_forward_secret` in the MCP server configuration **must be
the same value**. A mismatch shows as every delegated MCP call failing with 401 `invalid_token` at
the API (the MCP server logs a reason code, never the value). To rotate: write the new value to both
files and restart both processes; calls in between fail and are retried by the client. Use at least
32 random bytes; never reuse a bot token or the JWT secret.

Discovery documents (no authentication, cached for one hour):

- `GET /.well-known/oauth-authorization-server`
- `GET /.well-known/oauth-protected-resource/api` (the path follows `api_resource`)
- the MCP server publishes its own at `/.well-known/oauth-protected-resource` and the path form of
  `oauth_resource`

Without `issuer` all of them answer 404, and no `sva_` token is accepted.

### The reverse proxy

The event stream (`GET /api/v1/oauth/events/stream`) is a response that never ends by itself, with a
keep-alive comment every 20 seconds. `frontend/nginx.conf` serves it from its own exact location:
no buffering, no cache, no compression, `proxy_read_timeout 120s`, HTTP/1.1 keep-alive. Any other
proxy in front of the API needs the same: Caddy's `reverse_proxy` flushes `text/event-stream`
immediately; a proxy that buffers or has an idle timeout under 30 seconds will cut or delay the
stream. `scripts/test-nginx-event-stream-location.sh` checks the nginx file;
`scripts/lib/e2e-web-proxy-streaming-check.ts` checks the local test proxy.

## Operating it

### Disable the client

Instance administrator, first-party session (not a delegated or bot token):

```
PATCH /api/v1/admin/oauth-clients/ramflux   {"status": "disabled"}
```

This revokes every active grant of the client (reason `client_disabled`), refuses new consent and
every token request, and ends open streams within about 5 seconds. Disabling again sweeps again.
`{"status": "active"}` allows new consent and resurrects nothing. Without API access, the
equivalent is `UPDATE oauth_clients SET status = 'disabled' WHERE client_id = 'ramflux'`: every
validation reads the client's status, so tokens stop working at once; run the PATCH later to record
the revocation and the audit row.

### Switch a workspace off

Workspace owner or admin, first-party session:

```
PUT /api/v1/workspaces/{workspace_id}/connected-apps/settings   {"allow_connected_apps": false}
```

Revokes every active grant of the workspace (reason `workspace_disabled`) and blocks new consent for
it until switched on again. No settings row means allowed. A person lists and disconnects their own
connections at `GET /api/v1/me/connected-apps` and `DELETE /api/v1/me/connected-apps/{grant_id}`;
a workspace admin does the same for the workspace at
`/api/v1/workspaces/{workspace_id}/connected-apps`.

Every path that ends a connection is listed in the contract (section 6.5): user, client, workspace
admin, workspace switch-off, password change, admin reset, account disabled, removal from a
workspace, refresh-token reuse, code replay, client disabled, expiry. Each hook revokes in the
transaction of the change that causes it.

### What the retention sweeps delete

The worker (`apps/worker/src/oauth_feed.rs`) runs these for the life of the process, also when
`[oauth]` is not configured (so the outbox that the triggers keep filling stays bounded):

| What | When | Rule |
|---|---|---|
| `oauth_event_feed` and `oauth_event_outbox` rows | every few minutes | older than `event_feed_retention_days` |
| `oauth_audit_events` | daily | older than 180 days |
| `oauth_operation_logs` | daily | older than 90 days |
| `oauth_authorization_requests`, `oauth_authorization_codes` | daily | expired more than a day ago |
| `oauth_tokens` | daily | expired more than 7 days ago (a rotated refresh token stays long enough for a replay to be recognised) |

Deletes run in batches with a pause between them. A revoked grant's row stays (it is the record of
the revocation); deleting a user or a workspace removes their grants by cascade.

### Lock order

Revocation, consent and token issue race by design (a person disconnects while the client
refreshes). The database settles it with row locks taken in one global order. A transaction takes
the locks it needs in this order and none out of it:

| Step | Lock | Taken by |
|---|---|---|
| 1 | `oauth_authorization_requests` row, `FOR UPDATE` | consent decision |
| 2 | `oauth_clients` row | consent (`FOR SHARE`); disabling the client (`FOR NO KEY UPDATE`) |
| 3 | `users` row | consent (`FOR SHARE`); password change, admin reset, account disable (the update) |
| 4 | `workspaces` row | consent (`FOR KEY SHARE`); member removal (`FOR UPDATE`, the authorization epoch) |
| 5 | `workspace_members` row | consent (`FOR SHARE`); member removal (`FOR UPDATE`, `DELETE`) |
| 6 | `workspace_delegation_settings` row | consent (`FOR SHARE`, default row inserted if absent); workspace switch (the upsert) |
| 7 | advisory lock per user and client | consent (one decision at a time per user and client) |
| 8 | `oauth_grants` rows, by `id`, `FOR NO KEY UPDATE` | every revocation; the token endpoint (its one grant, first) |
| 9 | `oauth_authorization_codes` unredeemed, by `code_hash`, `FOR UPDATE` | every revocation; the token endpoint (the code it redeems) |
| 10 | `oauth_tokens` not yet revoked and not expired for over a day, by `id`, `FOR UPDATE` | every revocation; the token endpoint (the refresh token it rotates) |

Each transaction takes a subset: consent 1 to 9 (steps 8 and 9 only while it sweeps abandoned
grants), a hook one of 2 to 6 and then 8 to 10, the token endpoint 8 then 9 or 10, a user
disconnecting 8 to 10. Every arrow points from a lower step to a higher one, so no wait cycle
exists. Two properties carry the guarantees: tokens of a grant are only ever inserted by a
transaction that holds the grant row (step 8), so a revocation that holds it sees every token and no
new one appears while it works, and a change that ends a grant locks the state row it depends on
before it sweeps, so a consent that is in flight either commits first (and is then revoked by the
sweep) or waits and finds the state changed. Tested by `apps/api/src/oauth/manage/tests/races.rs`
(racing requests, many rounds, and deterministic "must wait" orderings).

Not part of the order: the worker's daily token prune takes token rows only (and touches children
through `parent_id ... ON DELETE SET NULL`); revocations therefore skip tokens that expired more than
a day ago, which the prune is allowed to delete, so the two never meet in opposite orders.

### Renumbering the migration

The migration is `migrations/0108_oauth_authorization_server.sql`. If another migration takes
`0108` first, rename the file to the next free number and change the three places that name it in
`apps/api/src/main.rs`: the embedded list (`include_str!` path and name), the schema probe table
(`SchemaProbe::Relation("oauth_grants")`), and the list in the replay-safety test
(`everything past the cutoff re-runs on an adopted database and must be idempotent`). Keep the
number order: the file must stay last or in its numeric place in all three lists. A database that
already recorded the old name has the objects but no ledger row under the new one: the probe finds
`oauth_grants`, the runner adopts it, and because the file is idempotent re-running it is harmless.
Do not edit the contents of an applied file for any other reason: the ledger stores a checksum.

## What the migration adds

Twelve tables (`oauth_*` and `workspace_delegation_settings`), one seeded client row (`ramflux`,
public, read scopes plus `offline_access`, resources `api` and `mcp`) and four triggers that append
to `oauth_event_outbox`:

| Trigger | Table | Fires on |
|---|---|---|
| `trg_oauth_outbox_notification_insert` | `notifications` | insert with a workspace |
| `trg_oauth_outbox_notification_read` | `notifications` | `is_read` false to true |
| `trg_oauth_outbox_ai_task_insert` | `ai_tasks` | insert |
| `trg_oauth_outbox_ai_task_status` | `ai_tasks` | status changes |

Locks: each `CREATE TRIGGER` takes `SHARE ROW EXCLUSIVE` on its table (writes wait, reads do not)
until the migration transaction commits. Nothing scans or rewrites either table, so the hold is a
few milliseconds. The statements are last in the file and run under `lock_timeout = 10s`: if a long
transaction holds the table, the migration fails without effect and the next start retries it.
Running the file again takes no table lock.

A trigger never fails the write that fired it. Its outbox insert runs in a nested block that traps
every error and raises a warning carrying only the SQLSTATE (`oauth event outbox insert skipped`).
Watch the database log for that line: it means an event was not recorded.

## Rolling back

Without touching the schema: remove `[oauth] issuer` (every endpoint answers 404 and every `sva_`
token is refused), or disable the client (revokes every grant).

Removing the schema, in this order:

```sql
DROP TRIGGER IF EXISTS trg_oauth_outbox_ai_task_status ON ai_tasks;
DROP TRIGGER IF EXISTS trg_oauth_outbox_ai_task_insert ON ai_tasks;
DROP TRIGGER IF EXISTS trg_oauth_outbox_notification_read ON notifications;
DROP TRIGGER IF EXISTS trg_oauth_outbox_notification_insert ON notifications;
DROP FUNCTION IF EXISTS oauth_outbox_ai_task();
DROP FUNCTION IF EXISTS oauth_outbox_notification();
DROP TABLE IF EXISTS oauth_event_feed, oauth_event_outbox, oauth_rate_hits, oauth_operation_logs,
  oauth_audit_events, oauth_tokens, oauth_authorization_codes, oauth_grant_projects, oauth_grants,
  oauth_authorization_requests, workspace_delegation_settings, oauth_clients;
```

Also delete the migration's row from `schema_migrations`, otherwise the runner reports the missing
objects at the next start.
