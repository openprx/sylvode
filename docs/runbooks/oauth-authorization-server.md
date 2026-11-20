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

Delegated MCP access additionally needs `[oauth] mcp_resource` and `internal_forward_secret`, and
the same three values in `[mcp]` (`oauth_issuer`, `oauth_resource`, `oauth_forward_secret`). The
secret is at least 32 bytes and is never logged. A partial `[mcp]` set refuses to start and names
the missing keys.

Local development on loopback may use `issuer = "http://127.0.0.1:<port>"` together with
`allow_insecure_loopback_issuer = true`. The flag is refused for any other host.

Discovery documents (no authentication, cached for one hour):

- `GET /.well-known/oauth-authorization-server`
- `GET /.well-known/oauth-protected-resource/api` (the path follows `api_resource`)

Without `issuer` both answer 404, and no `sva_` token is accepted.

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
