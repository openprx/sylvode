# OAuth authorization server: end-to-end acceptance (WP8)

A test client that acts as Ramflux, the one registered client of Sylvode's authorization server,
and runs the twenty cases A01 to A20 of the P4 contract and the alert chain case A21 against a live Sylvode. It needs a running
environment (see `DEPLOY-NOTES.md`), so it is **not** part of `.github/workflows/ci.yml` and nothing
registers it there.

What the client does, as Ramflux would: a loopback listener on `127.0.0.1:0` at
`/oauth/sylvode/callback`, PKCE S256, `state`, the consent step driven through the real Sylvode pages
in headless Chromium, the `iss` and `state` checks on the redirect, the token exchange for both
resources (the authorization code for the MCP resource, then a refresh for the API resource, as the
code is single use), MCP JSON-RPC calls, an event stream reader that stamps every message with a
monotonic clock, refresh with rotation and revocation.

## Commands

Run from the repository root. `npm ci` installs `@playwright/test` (no browser is downloaded; the
suite uses the local Chromium named by `CHROMIUM_BIN`, never Google Chrome).

| Purpose | Command |
|---|---|
| Type check | `frontend/node_modules/.bin/tsc -p tests/e2e/oauth/tsconfig.json` |
| Self-test (no server) | `node --test tests/e2e/oauth/selftest/*.selftest.ts` |
| Mocked dry run (no Sylvode; fake on ports 18456 and 18457) | `CHROMIUM_BIN=... OAUTH_E2E_OUT_DIR=<dir> node tests/e2e/oauth/dry-run.ts` |
| Dry run with a deliberate fault | add `--fault p2-event --expect-fail A09`, `--fault refresh-reuse --expect-fail A16` or `--fault no-revoked-control --expect-fail A11` |
| Create what the suite needs | `node tests/e2e/oauth/setup.ts` |
| Run the suite | `CHROMIUM_BIN=... npx playwright test -c tests/e2e/oauth/playwright.config.ts` |

A20 runs `scripts/test-mcp.sh`, which needs bash 4 or later; on macOS the suite uses `/opt/homebrew/bin/bash` when it exists (the system `/bin/bash` is 3.2).

The same commands exist as `npm run oauth:typecheck`, `oauth:selftest`, `oauth:dry-run`,
`oauth:setup` and `oauth:e2e` in the root `package.json`. Node 24 or later runs the TypeScript
directly (type stripping); `tsc` is the frontend's.

The suite file is `cases.e2e.ts`, not `*.spec.ts`, and has its own `playwright.config.ts`, so the
repository's `playwright.config.ts`, the Web E2E job and a bare `npx playwright test` never run it.

## Environment variables

Everything comes from the environment. Nothing is read from a file.

| Variable | Default | Meaning |
|---|---|---|
| `OAUTH_E2E_UI_ORIGIN` | `http://127.0.0.1:18097` | Web UI origin; also the issuer. Discovery, the consent pages, the token and revocation endpoints and every delegated API call (`<API resource>/v1/...`) go here |
| `OAUTH_E2E_API_ORIGIN` | `http://127.0.0.1:18083` | The API itself; used for sign-in, setup and the actions the cases attribute to B and to the administrator |
| `OAUTH_E2E_MCP_ORIGIN` | `http://127.0.0.1:8090` | The MCP server |
| `OAUTH_E2E_MCP_RESOURCE` | `<MCP origin>/mcp/rpc` | The MCP resource identifier; must equal `[oauth] mcp_resource` |
| `OAUTH_E2E_CLIENT_ID` | `ramflux` | Client id |
| `OAUTH_E2E_ADMIN_EMAIL`, `OAUTH_E2E_ADMIN_PASSWORD` | required | Instance administrator |
| `OAUTH_E2E_USER_A_EMAIL`, `OAUTH_E2E_USER_A_PASSWORD` | required | User A: the person who connects (workspace member) |
| `OAUTH_E2E_USER_B_EMAIL`, `OAUTH_E2E_USER_B_PASSWORD` | required | User B: acts on the workspace, and is its admin (the workspace-admin revocation and the AI task need it) |
| `OAUTH_E2E_WORKSPACE_NAME` | `qa-p4-acceptance` | Workspace W; must start with `qa-p4-`. `OAUTH_E2E_WORKSPACE_SLUG` defaults to the same |
| `OAUTH_E2E_P1_KEY`, `OAUTH_E2E_P1_NAME` | `QPA`, `qa-p4-alpha` | Project P1 (the one the grant covers) |
| `OAUTH_E2E_P2_KEY`, `OAUTH_E2E_P2_NAME` | `QPB`, `qa-p4-beta` | Project P2 (outside the grant) |
| `OAUTH_E2E_BOT_NAME` | `qa-p4-bot` | The bot made by the connection of P1 |
| `OAUTH_E2E_RUNNER_URL`, `OAUTH_E2E_WORKING_DIRECTORY` | `http://127.0.0.1:9`, `/tmp/qa-p4` | The runner address and working directory typed on the bot's connection. The address must be listed under `[outbound] allowed_hosts` in the API's `sylvode.toml` (see DEPLOY-NOTES.md) |
| `OAUTH_E2E_WORKSPACE_ID`, `OAUTH_E2E_P1_ID`, `OAUTH_E2E_P2_ID`, `OAUTH_E2E_BOT_ID` | none | Optional pins; otherwise found by name and key |
| `OAUTH_E2E_OUT_DIR` | required to run | Where the acceptance record and the case files are written (also Playwright's own artifacts) |
| `OAUTH_E2E_BUILD_COMMIT` | none | Build commit of the environment (`api --build-info` prints it). The API does not expose it over HTTP |
| `OAUTH_E2E_OPR_BOT_TOKEN` | none | A workspace bot token (`opr_`) of the workspace the MCP server is bound to, for A20's MCP script |
| `OAUTH_E2E_REPO_ROOT` | the repository containing this suite | Where the A20 scripts are found |
| `OAUTH_E2E_FLOWS_PER_MINUTE` | `7` | Consent flows started per minute at most (the server allows 10 decisions per minute per user) |
| `OAUTH_E2E_ALLOW_NON_LOOPBACK` | unset | `1` lets the suite and the setup run against a host that is not loopback. Without it any other host is refused |
| `OAUTH_E2E_A20_OVERRIDE` | none | Dry run only: an executable that stands in for both A20 scripts. The record shows it ran in `override` mode |
| `CHROMIUM_BIN` | `/usr/bin/chromium` | Chromium executable; a path containing "google chrome" is refused |

## Setup

`node tests/e2e/oauth/setup.ts` is idempotent. As the instance administrator it finds or creates:
users A and B (accounts that already exist are left alone; a wrong password for an existing
account is reported, not changed), workspace W (the administrator owns it), the members (A member,
B admin), projects P1 and P2, and one work item `qa-p4-seed-p1` / `qa-p4-seed-p2` in each. The bot
is made by connecting P1 (`POST /api/v1/projects/{P1}/bot-connection` with `bot.name`, which creates
its user, workspace seat, token and webhook) and joins P2 by id (`bot.id`), so it is an AI
participant of both projects. The configuration block's token is shown once and never printed;
the setup uses it for one `GET /api/v1/ai/protocol` call, as a starting runner would, because the
worker holds a new bot's tasks until its token has been used (at most an hour). It prints ids, never a password or a token.
It refuses a non-loopback host unless `OAUTH_E2E_ALLOW_NON_LOOPBACK=1`.

Every case that changes data does so inside W. The only things outside W the suite touches are user
A's account (A14 changes its password, A15 disables it) and the Ramflux client control; both are
restored (see below).

## Cases

How each case is driven and what it asserts, and where the as-built behaviour differs from the
contract's table.

| # | Driven by | Asserted | As built versus the contract |
|---|---|---|---|
| A01 | Discovery; the consent page (sign-in, workspace, projects "Only these" with P1, Allow); both token exchanges; `GET /api/v1/oauth/connection`; personal settings | `iss` and `state` of the redirect; token type, prefixes, `expires_in`, `no-store`; MCP scope `projects:read work_items:read`, API scope without `offline_access`; the connection lists P1 only; personal settings shows exactly one row for the grant | none |
| A02 | MCP `tools/list` with the MCP token | exactly the eight read tools | the contract's table lists more tools; as built the delegated set is the eight |
| A03 | MCP `work_items.get` and API `GET /issues/{id}` for a P1 and a P2 item | P1 succeeds; P2 refused and the item unchanged. API: HTTP 403, error `forbidden`. MCP: HTTP 403, or HTTP 200 with `isError`, no leak of the title | the MCP form of the refusal is accepted in either shape and recorded in the case file (the backend turns the API's 403 into a tool error) |
| A04 | MCP `work_items.update`; API `PUT /issues/{id}`; a consent request that also asks for `work_items:write` | MCP: HTTP 403, JSON-RPC code -32002, "tool not available to connected applications", no `WWW-Authenticate`. API: 403 `insufficient_scope`, `scope="work_items:read work_items:write"`. Consent: redirect with `invalid_scope`. The item is unchanged | the contract expects 403 `insufficient_scope` with `scope="work_items:write"` from MCP; as built, write scopes are refused for this client and the write tools are withheld |
| A05 | `GET /oauth/connection` with the MCP token | 401, body and challenge `invalid_token` | none |
| A06 | MCP `tools/list` with the API token | 401, challenge `invalid_token` with `resource_metadata` | none |
| A07 | Stream open; B creates an item in P1 and assigns it to A | `notification.created` for that item within 5 s of the end of the assignment response; envelope spec, workspace, project, id | none |
| A08 | Stream open; B `POST /projects/{P1}/ai/tasks` for the bot; the worker picks it up | `task.created` then `task.status_changed` (`pending` to `processing`) in order; created within 5 s of the creation response; the status change within 5 s of the first poll (every 100 ms, as B) that saw `processing` | "from its commit": the client cannot see a commit, so the poll is the observable commit; see Timing. The task is failed at the end (`retry: false`) to stop the worker's retry cycle |
| A09 | Stream (notification types) open; B assigns a P2 item to A; 10 s of silence; then a P1 item as a control | no event in 10 s; the control event arrives; the stream carried exactly that one event | the stream is restricted to notification types so that the task events of A08 are not part of the question; the control proves the stream was alive |
| A10 | Stream open, position kept, closed; B causes two notifications; reconnect with `Last-Event-ID` | exactly two events, the two items in order, no third, no `resync_required` | the position is the last event id, else the cursor of `hello`; types restricted to `notification.created` |
| A11 | Stream open; A disconnects in personal settings (confirmation dialog); B assigns another item | `revoked` with reason `user_revoked` within 5 s of the DELETE answer; the stream ends; no `sv.event` after it; reconnect 401; MCP 401; refresh `invalid_grant` | none |
| A12 | Fresh grant; stream; the refresh token sent to `/revoke` | `revoked` with `client_revoked` within 5 s, then the same dead-token checks | none |
| A13 | Fresh grant; stream; B disconnects the grant in the workspace settings | `workspace_admin_revoked`, same checks | none |
| A14 | Fresh grant; stream; A changes the password through the API | `password_changed`, same checks | restored in `finally` |
| A15 | Fresh grant; stream; the administrator disables A | `account_disabled`, same checks | restored in `finally` |
| A16 | Fresh grant; stream; the same refresh token used twice | second use 400 `invalid_grant`; `revoked` with `refresh_reuse`; same checks | none |
| A17 | Fresh grant; stream; the authorization code redeemed twice | second redemption 400 `invalid_grant`; `revoked` with `code_replay`; same checks | none |
| A18 | Deny after sign-in; "I don't have a Sylvode account" while signed out | the loopback receives `access_denied` with `consent_denied` / `account_required` and no code; A's connections and the number of users are unchanged | tokens cannot be counted through the API; the observable rows are connections and users |
| A19 | An authorization request with `redirect_uri` on `localhost` | the page is the error page (`consent-error-page`); nothing reaches the listener in 3 s | none |
| A20 | `scripts/test-mcp.sh` (needs `OAUTH_E2E_OPR_BOT_TOKEN`) and `frontend/scripts/ui-behaviour.mjs` as the administrator | both exit 0 | see below |

"The same checks" after a revocation: the stream ends; reopening it answers 401; an API call with the
last API token answers 401 `invalid_token`; an MCP call with the last MCP token answers 401; a refresh
answers 400 `invalid_grant`.

### A20

The contract names an "existing `opr_` MCP regression script" and a "JWT UI smoke test" without
paths. The suite runs `scripts/test-mcp.sh` (live `tools/list` and `projects.list` with a workspace
bot token, exit 69 means the environment is unavailable and counts as a failure) and
`frontend/scripts/ui-behaviour.mjs` (read-only; signs in with a JWT through the API and drives
`/admin/ui` in Chromium; run as the instance administrator). Neither is modified. "Unchanged
results" is judged as exit status 0: there is no recorded baseline to compare with. If either file
does not exist the case fails and names it.

## Timing

A07, A08 and A11 measure with `performance.now()` from the trigger to the moment the chunk carrying
the message arrived, and record the values in the result file; the limit is 5 s each (A09 waits 10 s).

- A07: from the end of the assignment response.
- A08, `task.created`: from the end of the creation response. `task.status_changed`: from the first
  poll that observed `processing`; if the event arrived before any poll saw it the interval is 0 and
  the case file says so; if no poll saw it, from the end of the creation response (an upper bound).
- A11: from the DELETE answer as Playwright reports it. The browser's answer reaches the test
  process a little after it reached the browser, so this figure is slightly smaller than the true one
  (by milliseconds on loopback).

## Output

Under `OAUTH_E2E_OUT_DIR`: `acceptance.md` (rendered from `RESULTS-TEMPLATE.md`), `acceptance.json`
(case, status, duration, timings, details, the three timing figures, the environment's build commit
and whether the Ramflux client was active at the end), and `cases/Axx.json` (a failed case also holds
the raw requests and responses with status, relevant headers and body, tokens redacted, and the
stream transcript).

No token, authorization code, PKCE verifier or password is written, printed or put in a trace.
Playwright traces, videos, screenshots and HTML reports are off. Connections are named by the grant
id (`GET /api/v1/oauth/connection` returns no token id). Redaction replaces values the run has
learned, anything shaped like a token or a JWT, and the values of sensitive keys. After the run the
reporter scans every file under the output directory for the token prefixes and the configured
passwords and fails the run if it finds one (`secret scan` in the record). Never run the suite with
`--trace`, `--video` or an HTML reporter on the command line: they would put files outside that
check.

## What the suite restores

- The Ramflux client is set to `active` at the start and at the end, and the record states the final
  value. No case disables it.
- User A is re-enabled and A's password is set back (checked by signing in, reset through the
  administrator when needed) in a cleanup that runs even when the case fails; a failed restore fails
  the case.
- A's remaining connections are disconnected at the start and at the end.

## Known limits

- It cannot prove anything about Ramflux itself (contract 15.7 is recorded separately).
- The A08 figure depends on the worker running with AI tasks enabled; a worker that is down shows as
  "no task.status_changed within 20 s".
- Playwright restarts its worker process after a failed test, so the shared grant is recreated and
  the setup of the environment is checked again; this costs a few seconds.
