# OAuth authorization server

Sylvode can act as an OAuth 2.1 authorization server for its own API and for its MCP server. A
person signed in to Sylvode connects an application to one workspace, chooses which projects and
which kinds of access it gets, and can disconnect it at any time. The application then calls the
API (and, if configured, the MCP server) as that person, within the chosen access.

- One client is pre-registered: `ramflux` (Ramflux desktop), a public client. There is no
  dynamic client registration and no token introspection endpoint.
- Only the authorization code grant with PKCE (`S256`) and the refresh token grant are supported.
- The feature is off until `[oauth] issuer` is configured. Without it the discovery documents and
  every OAuth endpoint answer `404`, and no delegated token is accepted.

Contents: [Enabling](#enabling) · [Discovery](#discovery) · [The registered client](#the-registered-client) ·
[Authorization flow](#authorization-flow) · [Endpoints](#endpoints) · [Scopes](#scopes) ·
[Tokens](#tokens) · [Calling the API with a delegated token](#calling-the-api-with-a-delegated-token) ·
[Event stream](#event-stream) · [Consent page](#consent-page) ·
[Managing connections](#managing-connections) · [MCP server](#mcp-server)

## Enabling

Add an `[oauth]` section to the API configuration. Only `issuer` is required:

```toml
[oauth]
issuer = "https://sylvode.example.com"   # https origin, no path
```

The issuer is the public origin that serves both the web UI (the consent page lives at
`<issuer>/oauth/authorize`) and the API. It appears byte for byte in the metadata and in every
`iss` parameter. For local development on loopback, `issuer = "http://127.0.0.1:<port>"` is
accepted together with `allow_insecure_loopback_issuer = true`; the flag is refused for any other
host.

To let connected applications use the MCP server as well, set `mcp_resource` and
`internal_forward_secret` in `[oauth]` (each requires the other) and the matching three keys in the
MCP server's `[mcp]` section; see [MCP server](#mcp-server).

Token lifetimes, grant limits, event stream limits and every other `[oauth]` key, with defaults and
ranges, are in [configuration.md](../configuration.md). Reverse proxy requirements (the discovery
locations, the consent page headers and the unbuffered event stream) and the retention jobs that
prune OAuth tables are in [operations.md](../operations.md).

## Discovery

| Document | Path | Content |
|---|---|---|
| Authorization server metadata (RFC 8414) | `/.well-known/oauth-authorization-server` | Issuer, endpoints, supported grants, `S256`, `token_endpoint_auth_methods_supported: ["none"]`, every scope, `authorization_response_iss_parameter_supported: true` |
| API protected resource metadata (RFC 9728) | `/.well-known/oauth-protected-resource/api` | The API resource, its authorization server and the scopes it supports |
| MCP protected resource metadata (RFC 9728) | Served by the MCP server; see [apps/mcp-server/README.md](../../apps/mcp-server/README.md#delegated-access-oauth) | The MCP resource |

The API document's path follows `[oauth] api_resource`, which defaults to `<issuer>/api`. The
documents need no authentication and may be cached for one hour (`Cache-Control: public,
max-age=3600`).

The metadata names these endpoints:

| Metadata field | Value |
|---|---|
| `authorization_endpoint` | `<issuer>/oauth/authorize` (the consent page) |
| `token_endpoint` | `<issuer>/api/v1/oauth/token` |
| `revocation_endpoint` | `<issuer>/api/v1/oauth/revoke` |

## The registered client

| Property | Value |
|---|---|
| `client_id` | `ramflux` |
| Type | Public; no client secret (`token_endpoint_auth_method: none`) |
| Redirect URIs | `http://127.0.0.1/oauth/sylvode/callback` and `http://[::1]/oauth/sylvode/callback` |
| Scopes it may request | `projects:read`, `work_items:read`, `notifications:read`, `tasks:read`, `offline_access` |
| Resources it may request | `api` and `mcp` |

Redirect URIs match byte for byte, except that a loopback redirect may add any port from 1024 to
65535 (RFC 8252). Nothing else is normalised: no case folding, no trailing-slash equivalence, no
`localhost`.

## Authorization flow

1. **Authorization request.** The client opens `<issuer>/oauth/authorize` in the browser with these
   query parameters:

   | Parameter | Required | Rule |
   |---|---|---|
   | `response_type` | yes | `code` |
   | `client_id` | yes | `ramflux` |
   | `redirect_uri` | yes | A registered redirect URI |
   | `code_challenge` | yes | PKCE challenge |
   | `code_challenge_method` | yes | `S256` |
   | `state` | yes | 16 to 512 visible ASCII characters |
   | `resource` | yes, one or more | The API resource (`api_resource`) and/or the MCP resource (`mcp_resource`) |
   | `scope` | yes | Space-separated scopes, a subset of the client's allowed scopes |
   | `device_name` | no | 1 to 64 characters, shown on the consent page and in the connection list |
   | `workspace_hint` | no | A workspace UUID to preselect |

   An unknown client or a redirect URI that does not match shows an error page and sends the browser
   nowhere. Any other invalid parameter sends the browser back to the redirect URI with a standard
   `error`, the `state` and `iss`.

2. **Consent.** The person signs in if needed, chooses the workspace, the projects and the access,
   and allows or cancels; see [Consent page](#consent-page). An authorization request can be decided
   for ten minutes.

3. **Redirect.** On approval the browser returns to the redirect URI with `code`, `state` and `iss`.
   The code starts with `svc_`, is single use and lives for `authorization_code_ttl_seconds`.

4. **Token request.** The client exchanges the code:

   ```sh
   curl -X POST https://sylvode.example.com/api/v1/oauth/token \
     -H 'Content-Type: application/x-www-form-urlencoded' \
     --data-urlencode grant_type=authorization_code \
     --data-urlencode client_id=ramflux \
     --data-urlencode code=svc_... \
     --data-urlencode redirect_uri=http://127.0.0.1:53682/oauth/sylvode/callback \
     --data-urlencode code_verifier=... \
     --data-urlencode resource=https://sylvode.example.com/api
   ```

   The response carries `access_token`, `token_type` (`Bearer`), `expires_in`, `scope`, `grant_id`,
   `workspace_id`, and `refresh_token` when the grant includes `offline_access`. An access token is
   bound to the one `resource` named in the request, and its `scope` is the grant's scopes narrowed
   to what that resource supports. A client that was granted both resources requests one token per
   resource.

5. **Refresh.** `grant_type=refresh_token` with `client_id`, `refresh_token`, `resource` and
   optionally a narrower `scope`. Every refresh rotates the refresh token. Presenting a refresh token
   that was already used revokes the whole grant (`refresh_reuse`), as does presenting a redeemed
   authorization code again (`code_replay`). A wrong verifier or redirect URI does not consume the
   code, but the third failed attempt invalidates it.

6. **Revocation.** `POST /api/v1/oauth/revoke` with `client_id` and `token` (RFC 7009). Revoking a
   refresh token ends its whole grant (`client_revoked`); revoking an access token ends only that
   token. An unknown, already revoked or foreign token answers `200` with an empty body.

The token and revocation endpoints take parameters in an `application/x-www-form-urlencoded` body
only; a query string is refused. Responses carry `Cache-Control: no-store`, and failures use the
standard OAuth error codes without telling apart an unknown, expired or foreign code.

## Endpoints

| Method | Path | Caller | Purpose |
|---|---|---|---|
| GET | `/.well-known/oauth-authorization-server` | anyone | Authorization server metadata |
| GET | `/.well-known/oauth-protected-resource/api` | anyone | API protected resource metadata |
| POST | `/api/v1/oauth/token` | client | Authorization code and refresh token grants |
| POST | `/api/v1/oauth/revoke` | client | Token revocation |
| POST | `/api/v1/oauth/authorization-requests` | consent page | Validate an authorization request and return what to show |
| POST | `/api/v1/oauth/authorization-requests/{request_id}/decision` | consent page, signed-in person | Record the decision; needs the single-use consent token |
| GET | `/api/v1/oauth/connection` | client, delegated token | The grant behind the token: workspace, account, scopes, projects, consent and expiry times |
| GET | `/api/v1/oauth/events/stream` | client, delegated token | [Event stream](#event-stream) |
| GET | `/api/v1/me/connected-apps` | signed-in person | List own connections |
| DELETE | `/api/v1/me/connected-apps/{grant_id}` | signed-in person | Disconnect one own connection |
| GET | `/api/v1/workspaces/{workspace_id}/connected-apps` | workspace owner or admin | List connections that reach the workspace |
| DELETE | `/api/v1/workspaces/{workspace_id}/connected-apps/{grant_id}` | workspace owner or admin | Disconnect one connection in the workspace |
| GET, PUT | `/api/v1/workspaces/{workspace_id}/connected-apps/settings` | workspace owner or admin | Read or set `{"allow_connected_apps": true\|false}` |
| GET | `/api/v1/admin/oauth-clients` | instance administrator | List registered clients |
| PATCH | `/api/v1/admin/oauth-clients/{client_id}` | instance administrator | Set `{"status": "active"\|"disabled"}` |

The management endpoints (`/me/...`, `/workspaces/.../connected-apps...`, `/admin/oauth-clients...`)
accept only Sylvode's own signed-in sessions: a bot token or a delegated token is refused with `403`
whatever its scopes. Requests that change state also refuse a cross-site origin. None of them can
create, extend or widen a grant. Without an `[oauth]` section every management endpoint answers
`error_code: "feature_disabled"` (`connected applications are not enabled on this server`).
`GET /api/v1/oauth/internal/token-context` exists for the MCP
server only and answers `404` to anyone without the internal forwarding credential.

## Scopes

| Scope | Grants | Optional on the consent page |
|---|---|---|
| `projects:read` | See projects and their details | no |
| `work_items:read` | See work items, comments, activities and labels | no |
| `work_items:write` | Create and update work items, add comments and labels (never delete); implies `work_items:read` | yes |
| `forms:read` | See forms and their records | no |
| `forms:write` | Create and update form records (never delete); implies `forms:read` | yes |
| `notifications:read` | Receive the person's notifications from the workspace | no |
| `notifications:write` | Mark notifications as read; does not imply `notifications:read` | yes |
| `tasks:read` | See agent tasks and their progress | no |
| `surfaces:embed` | Show Sylvode views inside the client. Part of the vocabulary; no API route requires it | yes |
| `offline_access` | Receive a refresh token, so the connection lasts until it is disconnected or expires | no |

The forms routes do not accept delegated tokens (see
[Calling the API with a delegated token](#calling-the-api-with-a-delegated-token)), so
`forms:read` and `forms:write` currently unlock nothing on the API.

"Optional" means the person may untick the scope on the consent page; the others are shown fixed
when the client asks for them. The scope list is closed: an unknown name is `invalid_scope`, and the
reserved names `flow:read`, `flow:write` and `tasks:dispatch` are refused as well. A scope string is
at most 1024 bytes, single-space separated.

What each resource supports:

| Resource | Scopes an access token can carry |
|---|---|
| API (`api_resource`) | Every scope except `offline_access` |
| MCP (`mcp_resource`) | `projects:read`, `work_items:read`, `work_items:write`, `forms:read`, `forms:write` |

`offline_access` belongs to the grant and never appears in an access token. A token request whose
grant has nothing in common with the requested resource is refused with `invalid_scope`.

## Tokens

| Prefix | Kind | Life |
|---|---|---|
| `svc_` | Authorization code | Single use; `authorization_code_ttl_seconds` |
| `sva_` | Access token, bound to one resource | `access_token_ttl_seconds` |
| `svr_` | Refresh token | Rotated on every use; expires when unused for `refresh_token_idle_days` |

Every secret is 32 random bytes rendered as 64 lowercase hex characters behind its prefix; the
server stores only its SHA-256 hash. A grant ends after `grant_max_lifetime_days` whatever happens,
and a person holds at most `max_active_grants_per_user_client` active connections per client.
Workspace bot tokens (`opr_`) and user session tokens are separate token kinds; an `sva_` token is
never accepted where they are, and the other way round.

## Calling the API with a delegated token

Send the access token in the `Authorization: Bearer` header; a token in a query string is refused.
Every request re-reads the token, the grant, the client, the user, the membership and the workspace
setting, so a disconnect, a removal or a disabled client takes effect on the next request.

A delegated token reaches only the routes below. Any other route is refused, whatever the person
could do themselves.

| Method | Path | Required scope |
|---|---|---|
| GET | `/api/v1/workspaces/{workspace_id}/projects` | `projects:read` |
| GET | `/api/v1/projects/{id}` | `projects:read` |
| GET | `/api/v1/projects/{project_id}/context` | `projects:read` |
| GET | `/api/v1/projects/{project_id}/agent-policy` | any scope |
| GET | `/api/v1/projects/{project_id}/issues` | `work_items:read` |
| POST | `/api/v1/projects/{project_id}/issues` | `work_items:write` |
| GET, PUT | `/api/v1/issues/{id}` | `work_items:read` (GET), `work_items:write` (PUT) |
| GET | `/api/v1/issues/by-identifier/{identifier}` | `work_items:read` |
| GET, POST | `/api/v1/issues/{issue_id}/comments` | `work_items:read` (GET), `work_items:write` (POST) |
| GET | `/api/v1/issues/{issue_id}/activities` | `work_items:read` |
| GET | `/api/v1/issues/{issue_id}/labels` | `work_items:read` |
| POST, DELETE | `/api/v1/issues/{issue_id}/labels/{label_id}` | `work_items:write` |
| GET | `/api/v1/notifications` | `notifications:read` |
| GET | `/api/v1/notifications/unread-count` | `notifications:read` |
| PATCH, PUT | `/api/v1/notifications/{id}/read` | `notifications:write` |
| PATCH, PUT | `/api/v1/notifications/read-all` | `notifications:write` |
| GET | `/api/v1/projects/{project_id}/ai/tasks` | `tasks:read` |
| GET | `/api/v1/oauth/connection` | any scope |
| GET | `/api/v1/oauth/events/stream` | `notifications:read` or `tasks:read` |

Rules that apply on top of the table:

- The object a request names (workspace, project, work item, notification) is resolved before the
  handler runs. An object outside the grant's workspace or project set is refused with the same
  answer as an object that does not exist.
- A workspace owner or admin acts as an ordinary member through a delegated token: it cannot
  administer the workspace.
- A request that would start or assign an agent (assigning a work item to a bot, mentioning a bot
  in a comment) is refused with `403`, because it needs the reserved `tasks:dispatch` scope.
- A missing or insufficient scope answers `403` with `WWW-Authenticate: Bearer
  error="insufficient_scope"`, naming the required scope; an invalid, expired or revoked token
  answers `401` with `error="invalid_token"`.
- Each grant is rate limited, and every delegated request is recorded in the OAuth operation log.

The rest of the API is described in [api.md](api.md).

## Event stream

`GET /api/v1/oauth/events/stream` is a server-sent-events stream that pushes the person's
notifications and agent task changes in the grant's workspace to the client. It needs an API
access token with `notifications:read` or `tasks:read`.

| Event type | Scope |
|---|---|
| `notification.created` | `notifications:read` |
| `notification.read` | `notifications:read` |
| `task.created` | `tasks:read` |
| `task.status_changed` | `tasks:read` |

- `?types=a,b` limits the stream to some event types; types the grant may not see are dropped, and
  a request left with none is refused.
- Events arrive as `event: sv.event` frames with an `id:` line and one JSON `data:` line (`spec`
  `sylvode.delegated_event.v1`, `id`, `type`, `occurred_at`, `workspace_id`, `project`, `subject`,
  `links.web`). Control messages arrive as `event: sv.control` frames without an id: `hello` (grant,
  workspace, cursor and server time, sent first), `resync_required`, `lagged`, `reauth` and
  `revoked`. A `: keep-alive` comment is sent after 20 seconds of silence.
- To resume, send the last event id in the `Last-Event-ID` header (or `?cursor=`). Delivery is in
  order and at least once across reconnects, exactly once on one connection. A cursor that is too
  far behind (more than `event_backfill_max_events` events or older than
  `event_backfill_max_age_hours`) gets `resync_required`: reload state through the API and start a
  new stream.
- Visibility is checked when an event is delivered, not when it happened: workspace, scope, user,
  the grant's projects and the person's current access to the project.
- A disconnect or revocation sends `revoked` and closes the stream within a few seconds. When the
  access token expires the stream sends `reauth` and closes; reconnect with a fresh token. A client
  that does not read its queue in time gets `lagged` and is disconnected.
- At most three streams per grant and twenty per person are open at once; another is refused with
  `Retry-After: 30`. `max_streams_per_process` caps streams per API process.

## Consent page

The consent page is part of the web UI at `/oauth/authorize`. It shows the application, the device
name and the host it returns to, the signed-in account (with a way to switch), a workspace picker,
the project choice ("All projects in this workspace, including new ones" or "Only these projects",
up to 500), and the requested access grouped as read, change and other, with optional scopes as
checkboxes. It states that the application will act as the person within that access and cannot
administer the workspace, delete anything, see secrets or start agents.

- Someone who is not signed in is asked to sign in first. Accounts are created by a Sylvode
  administrator; the page does not register accounts.
- Workspaces whose administrators switched connected applications off cannot be chosen.
- A person who already has the maximum number of connections for the client is asked to
  disconnect one first.
- The page cannot be shown inside another page (`frame-ancestors 'none'`), and the decision is
  bound to the request it was shown for: client, redirect URI, PKCE challenge and resources come
  from the stored request, never from the decision.

## Managing connections

| Who | Where in the web UI | What |
|---|---|---|
| Every person | Settings, "Connected applications" | List own connections (workspace, projects, access, last use, expiry) and disconnect one |
| Workspace owner or admin | Workspace settings, "Connected applications" | List connections that members made to the workspace, disconnect one, and switch "Allow members to connect applications such as Ramflux" off or on |
| Instance administrator | Admin settings, "Applications" | See the registered clients and their active connections; disable or enable a client |

The effect of each action:

- **Disconnect** revokes the grant at once: its tokens stop working and its event streams close.
- **Switching a workspace off** revokes every active connection to that workspace and blocks new
  ones until it is switched on again. A workspace without a setting allows connected applications.
- **Disabling a client** revokes every active grant of the client, refuses new consent and every
  token request. Enabling it again allows new consent and restores nothing.

A grant also ends by itself. The revocation reason recorded for each case:

| Reason | Cause |
|---|---|
| `user_revoked` | The person disconnected it |
| `client_revoked` | The client revoked its refresh token |
| `workspace_admin_revoked` | A workspace owner or admin disconnected it |
| `workspace_disabled` | Connected applications were switched off for the workspace |
| `password_changed` | The person's password was changed or reset |
| `account_disabled` | The person's account was disabled |
| `membership_removed` | The person's workspace membership ended |
| `refresh_reuse` | A used refresh token was presented again |
| `code_replay` | A redeemed authorization code was presented again |
| `client_disabled` | An instance administrator disabled the client |
| `expired` | The grant reached its maximum lifetime |

## MCP server

With `[oauth] mcp_resource` and `internal_forward_secret` set in the API configuration, and
`oauth_issuer`, `oauth_resource` and `oauth_forward_secret` set to the same values in the MCP
server's `[mcp]` section, a client can request a token for the MCP resource and call the MCP
server's `POST /mcp/rpc` with it. Delegated MCP callers get read tools only. Discovery, the tool
table, the forwarding credential and the error codes are described in
[apps/mcp-server/README.md, Delegated access](../../apps/mcp-server/README.md#delegated-access-oauth).

If the two forwarding secrets differ, every delegated MCP call fails with `401 invalid_token`. To
rotate the secret, write the new value to both configuration files and restart both processes.
Use at least 32 random bytes, and never reuse a bot token or the JWT secret.
