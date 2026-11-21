# Repository Guidelines

## Project Overview

- Sylvode (formerly OpenPR) is an open-source project management platform with governance and AI integration.
- The MCP server exposes tools for managing projects, context, project types/templates, operation records, universal forms, WASM plugins, issues, sprints, labels, comments, proposals, check results, release next actions, files, scenario-specific governed work, and Sylvode Flow (v0.9).
- Transports: HTTP (`POST /mcp/rpc`), stdio (stdin/stdout), SSE (`GET /sse` + `POST /messages`).
- Tool mode: `search` by default (nine tools: `sylvode_overview`, `sylvode_search`, `sylvode_read`, `sylvode_write` and the five task tools; registry names are refused when called directly), `full` is the opt-in (`[mcp] tool_mode`, `serve --tools`, `?tools=` / `X-Sylvode-Tool-Mode`). Tests and scripts that call registry tools by name must opt into `full`. `[mcp] allow_write_call = false` disables every write tool except the five task lifecycle tools, in every mode and in the CLI.

## MCP Surface (Quick Reference)

- Transports:
  - `stdio` (default for Claude Desktop, Codex, local CLI)
  - `HTTP` (web integrations, OpenClaw plugins)
  - `SSE` (streaming clients; also available on HTTP port)
<!-- BEGIN GENERATED: tool-overview (sylvode-skillgen; do not edit) -->
- The tool catalogue has 153 tools in 19 categories:
  - `projects` (5 tools, 2 read, 3 write): Create, read, update and delete projects
  - `project_types` (2 tools, 2 read, 0 write): Read the project scenario types and their capability schemas
  - `scenario_templates` (3 tools, 2 read, 1 write): Browse and install ready-made multi-form setups
  - `context` (3 tools, 3 read, 0 write): Read project context, governance and the agent policy
  - `forms` (34 tools, 16 read, 18 write): Forms, records, attachments, permissions, imports and business events
  - `plugins` (5 tools, 3 read, 2 write): Install, inspect and invoke WASM plugins
  - `work_items` (11 tools, 5 read, 6 write): Issues of a project: list, read, create, update, label
  - `comments` (3 tools, 1 read, 2 write): Discuss work items
  - `files` (1 tool, 0 read, 1 write): Upload files and get their URLs
  - `labels` (5 tools, 2 read, 3 write): Workspace and project labels
  - `sprints` (4 tools, 1 read, 3 write): Plan iterations of a project
  - `proposals` (5 tools, 2 read, 3 write): Governance proposals and check results
  - `release` (1 tool, 1 read, 0 write): Release readiness gates and blockers
  - `code` (2 tools, 1 read, 1 write): Context and governed proposals for code tasks
  - `documents` (3 tools, 0 read, 3 write): Governed document summaries, risk reviews and approvals
  - `operations` (2 tools, 0 read, 2 write): Governed inspection reports and corrective actions
  - `workspace` (3 tools, 3 read, 0 write): Members, global search and bot operation records
  - `flow` (56 tools, 21 read, 35 write): Pages, collections, shares, comments, imports, exports and maintenance of Flow
  - `tasks` (5 tools, 1 read, 4 write): The protocol of tasks assigned to a bot: get, acknowledge, report progress, complete, fail
- Every tool, one line each, is in `skills/sylvode-mcp/reference/tools.md`; the hand-written skill is `skills/sylvode-mcp/SKILL.md`. Both are generated or checked by `sylvode-skillgen`.
<!-- END GENERATED: tool-overview -->
- Authentication:
  - `stdio` and the CLI subcommands: `mcp.bot_token` in the configuration file (prefix `opr_`). Not used by `http`/`sse`.
  - `http`/`sse`: no `mcp.bot_token`, no shared secret. Every request must carry its own caller account token as `Authorization: Bearer opr_...`; the server forwards it to the API as-is. Missing/malformed header = `401` (except `/health`). The token's workspace must match this server's `mcp.workspace_id`, or the API returns `403`.
  - Delegated (OAuth) callers: `Authorization: Bearer sva_...` on `POST /mcp/rpc` of the `http` transport, enabled by `mcp.oauth_issuer`, `mcp.oauth_resource` and `mcp.oauth_forward_secret` (all three or none). The server asks the API for the token context on every request (`401` invalid token, `503` no verdict), serves the caller in its grant's workspace (never `mcp.workspace_id`), filters `tools/list` and refuses `tools/call` by the table in `src/oauth_scopes.rs` (default deny; read tools only in the first release), and forwards the token to the API with the internal forwarding credential (`X-Sylvode-Forwarded-By`, `X-Sylvode-Forward-Auth`). This forwarding is a deliberate deviation from the MCP authorization specification (D2); keep it stated in release notes. `sva_` tokens are never forwarded without that credential and never accepted on `/sse`, `/messages`, the `sse` transport or `--flow-only`.
  - `mcp.auth_token` does not exist any more. If a configuration file still has it, the server refuses to start — remove the key, do not try to set a value for it.
- Skill package: `skills/sylvode-mcp/SKILL.md` (the hand-written source of everything an agent is told; generated files are checked with `sylvode-skillgen --check`)

## Project Structure & Module Organization

- `apps/api/` — Axum REST API server
- `apps/mcp-server/` — MCP server (tools, client, transport)
  - `src/tools/` — Tool implementations (one file per domain)
  - `src/client/` — HTTP client to API server (including file upload)
  - `src/server.rs` — MCP request dispatcher
  - `src/main.rs` — Transport setup (HTTP, stdio, SSE)
- `apps/worker/` — Background task worker
- `frontend/` — SvelteKit frontend
- `migrations/` — PostgreSQL migrations (SeaORM)
- `skills/` — MCP skill packages for AI agents

## Build, Test, and Development Commands

```bash
# Build
source ~/.cargo/env
cargo build --release --bin api
cargo build --release --bin mcp-server

# Frontend
cd frontend && bun run build

# Format and lint
cargo fmt
cargo clippy --all-targets -- -D warnings

# Deploy (local)
docker compose down && docker compose up -d --build

# Test MCP -- Authorization carries the caller's own MCP-type account token (opr_ prefix),
# scoped to the workspace configured in mcp.workspace_id. Omitting it gets 401.
curl -X POST http://localhost:8090/mcp/rpc \
  -H "Authorization: Bearer opr_your_own_account_token" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'   # search mode: nine tools

curl -X POST "http://localhost:8090/mcp/rpc?tools=full" \
  -H "Authorization: Bearer opr_your_own_account_token" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'   # full mode: the registry

curl -X POST http://localhost:8090/mcp/rpc \
  -H "Authorization: Bearer opr_your_own_account_token" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"project_id":"<project-uuid>"}}'
```

## Coding Style & Naming Conventions

- Rust 2021 idioms, 4-space indentation.
- File/module: `snake_case`. Types: `PascalCase`. Functions: `snake_case`. Constants: `SCREAMING_SNAKE_CASE`.
- MCP tool names: `domain.action` (e.g. `work_items.create`, `labels.update`).
- Tool implementations: one file per domain in `apps/mcp-server/src/tools/`.
- Frontend: SvelteKit conventions, components in `src/lib/components/`.
- Run `cargo fmt` before committing.

## Testing Guidelines

- MCP regression: test the whole registry across 3 transports (HTTP, stdio, SSE), plus project-aware `tools/list` when a project id is supplied.
- CLI business-flow coverage: use `sylvode tools call --name <tool> --args-json '{...}'` for universal forms, plugins, operation records, and scenario tools so CLI calls reuse the same MCP tool names and audit path.
- API: test via `curl` or MCP client against running instance.
- Frontend: `bun run build` must succeed.
- When adding a new MCP tool:
  1. Add tool definition in `src/tools/<domain>.rs`
  2. Register in `src/tools/mod.rs`
  3. Add dispatch in `src/server.rs`
  4. Add client method in `src/client/mod.rs` if needed
  5. Test via all three transports

## Commit & Pull Request Guidelines

- Conventional Commits:
  - `feat: add files.upload MCP tool`
  - `fix: PATCH→PUT method mismatch in work_items.update`
  - `docs: update MCP tool reference in README`
- PRs should include: problem/solution summary, test evidence, config changes.

## Security & Configuration

- Never commit bot tokens or API keys.
- The configuration file is the source of all secrets (`mcp.bot_token`, `mcp.workspace_id`); Sylvode reads no environment variables. Never commit the file itself.
- Bot tokens are workspace-scoped; each creates a bot user (`entity_type = bot`, `agent_type = mcp`) for audit integrity.
- `mcp.bot_token` is only required for `stdio` transport and CLI subcommands (no per-request caller to act on). `http`/`sse` transports ignore it: every request supplies its own caller bot token via `Authorization: Bearer opr_...`, forwarded to the API unchanged. There is no server-side fallback identity and no shared inbound secret for these transports, so `mcp.bind_addr` can be bound to any reachable address without extra configuration.
- Adding a tool: decide it for delegated callers in `src/oauth_scopes.rs` (`SCOPED_TOOLS` with its scope, or `WITHHELD_TOOLS`); `oauth_scopes::tests::every_registered_tool_has_a_decision` fails until you do. A tool that is not in `SCOPED_TOOLS` is invisible and uncallable for delegated callers. A tool goes into `SCOPED_TOOLS` only when the API route it calls is on `DELEGATED_ROUTES` (`apps/api/src/oauth/delegated.rs`) for a scope the tool's scope satisfies; `every_tool_available_to_delegated_callers_calls_a_route_the_api_allows_for_its_scope` (`tests/oauth_delegated_http.rs`) records the real calls and fails otherwise.
- Never log, echo or store a delegated token or `mcp.oauth_forward_secret`; delegated tokens come from the `Authorization` header only.
- `mcp.auth_token` was removed. A configuration file that still sets it fails to start; when asked to fix a broken MCP config, delete the key rather than filling in a value.
- File uploads: server-side type validation and size limits.
