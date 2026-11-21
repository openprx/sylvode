# Documentation Index

## 1. Product and Setup

- `../README.md` — Project overview, MCP configuration, tool reference, quick start.
- `./universal-forms-and-plugins.md` — Universal forms, WASM plugins, scenario templates, business events and webhooks, and restaurant delivery reference.
- `./universal-forms-implementation-map.md` — Source-module, public-surface, verification-command, and status-marker map for the universal business platform.
- `./scenario-templates.md` — Catalog of built-in scenario templates, generated forms, integrations, and usage paths.
- `./universal-forms-acceptance.md` — Acceptance guide for the universal forms and restaurant reference workflow.
- `./universal-forms-production.md` — Production runbook for operating Sylvode as a generic business platform.
- `./schemas/openpr-universal-forms-readiness.schema.json` — JSON Schema for the machine-readable universal forms readiness report.
- `./schemas/openpr-universal-forms-signoff-status.schema.json` — JSON Schema for the machine-readable manual signoff progress report.
- `./schemas/openpr-universal-forms-completion-audit.schema.json` — JSON Schema for the machine-readable completion audit gate report.
- `./schemas/openpr-universal-forms-development-status.schema.json` — JSON Schema for the machine-readable development matrix status report.
- `./schemas/openpr-universal-forms-scenario-catalog.schema.json` — JSON Schema for the machine-readable scenario template catalog.
- `./schemas/openpr-universal-forms-implementation-map.schema.json` — JSON Schema for the machine-readable implementation map.
- `./schemas/openpr-universal-forms-delivery-status.schema.json` — JSON Schema for the compact one-command delivery status summary.
- `./schemas/openpr-universal-forms-delivery-manifest.schema.json` — JSON Schema for the machine-readable delivery manifest mirror.
- `./schemas/openpr-universal-forms-release-gate.schema.json` — JSON Schema for the machine-readable release gate decision.
- `./schemas/openpr-project-release-readiness.schema.json` — JSON Schema for project release readiness API/MCP responses.
- `../apps/mcp-server/AGENTS.md` — Coding agent guidelines: build, test, commit conventions.

## 2. MCP Skill Package

<!-- BEGIN GENERATED: tool-count (sylvode-skillgen; do not edit) -->
The MCP server registers 153 tools in 19 categories. By default it lists nine of them (search mode) and reaches the rest through `sylvode_search`, `sylvode_read` and `sylvode_write`. Every tool, with its category and access, is in [reference/tools.md](../skills/sylvode-mcp/reference/tools.md).
<!-- END GENERATED: tool-count -->

- `../skills/sylvode-mcp/SKILL.md` — The MCP skill: task procedure, finding and calling tools, conventions, recipes.
- `../skills/sylvode-mcp/reference/tools.md` — Every tool, one line each (generated).
- `../skills/sylvode-mcp/scripts/mcp-regression.py` — Tool registry and core-surface regression test across HTTP, stdio, and SSE.
- `../skills/sylvode-mcp/scripts/validate-mcp.sh` — Quick smoke test.

## 3. Source Code

- `../apps/api/` — REST API server (Axum + SeaORM).
- `../apps/mcp-server/` — MCP server (HTTP/stdio/SSE).
- `../apps/worker/` — Background task worker.
- `../frontend/` — SvelteKit frontend.
- `../migrations/` — PostgreSQL schema migrations.

## 4. Deployment

- `../docker-compose.yml` — Full stack: API, frontend, MCP, worker, webhook, PostgreSQL.
- `../Dockerfile.prebuilt` — Production image using pre-built binaries.
- `./runbooks/flow-runtime-capabilities.md` — Flow runtime capabilities: what is on by default, reader leases, and how to turn a capability off.
