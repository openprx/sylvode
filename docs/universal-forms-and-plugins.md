# Universal Forms, Plugins, and Scenario Templates

Sylvode supports project-defined business applications through universal forms, business events, webhooks, MCP tools, and sandboxed WASM plugins.

## Runtime Model

Universal forms are project-scoped data types. Each form defines a schema, title template, optional detail layout, and one or more views. Records store normalized values and write field indexes for query and aggregate paths.

Core tables:

- `project_forms` defines business types such as `order`, `order_line`, `print_job`, or `business_report`.
- `form_views` stores grid/detail view configuration.
- `form_records` stores record values.
- `form_record_links` stores parent-child and reference relationships.
- `form_record_field_index` stores typed projections, including decimal amount values.
- `business_events` and `event_inbox` record business events and idempotent receipts. (The write-only `event_outbox` was dropped in 0.2.21, migration `0053`.)
- `plugins` and `plugin_invocations` store WASM plugin packages and execution logs.

Amount fields use decimal strings at API boundaries. JSON numbers are rejected for amount input so money and business totals do not pass through floating-point math.

## API, MCP, and Integrations

The REST API is the source of truth for forms, records, links, events, aggregates, and plugins.

MCP exposes the same business surface to agents through forms tools, plugin tools, events resources, and aggregate reads. This lets human users work in the frontend while AI agents use the same project data and event ledger.

External systems integrate in three ways: they read business events through the form and record event endpoints or the MCP `events.tail` tool, they receive workspace webhooks (configured on the Webhooks page, with the optional webhook receiver service in `docker-compose.yml`), or they act through the REST API, MCP or the CLI with a workspace bot token bound to that surface. A webhook does not have to be an agent.

Connectors and agent invocations were removed in 0.2.21 (migration `0052`); there are no connector records, connector kinds or connector receipts any more.

## WASM Plugins

Plugins use `openpr.plugin.v1` manifests and a small core WASM ABI:

- `field_validator` can block invalid record writes.
- `formula` can return a patch before the final schema and decimal validation pass.
- `event_handler` can react to business events after writes commit.
- Plugin-provided tools appear through MCP when declared in the manifest.

WASM modules run under wasmtime with fuel, timeout, and memory limits. Plugins have no host imports or WASI access in the current runtime.

A plugin module may be at most 4 MiB (4,194,304 bytes). The limit is checked at install, before the module is decoded or compiled, and again at every invocation. Compilation cannot be interrupted, so the module size is what bounds how long one invocation can keep a thread compiling; a plugin implements the three-export ABI with no imports, and an optimised Rust plugin that reads and writes JSON is a few hundred KiB.

`runtime.timeout_ms` bounds the whole invocation from the moment the request starts: waiting for a thread, compiling and instantiating the module, and running the guest. If the deadline passes before the guest starts, the invocation is a `timeout` with no fuel recorded and the guest never runs; a running guest is interrupted.

Each invocation gets its own store with fixed resource limits: one instance, one linear memory of at most `runtime.memory_bytes`, and one table of at most 65,536 elements (initial size included). A growth beyond a limit returns `-1` to the guest; a module whose initial memory or table already exceeds it, or that declares a second memory or table, fails to instantiate.

Every run is recorded in `plugin_invocations` with `status` set to `completed`, `failed`, or `timeout`. `timeout` means the wall-clock deadline (`runtime.timeout_ms`) expired, and its `error_message` reads `wasm execution timeout after {N}ms`; every other run that produced no output, including fuel exhaustion and guest traps, is `failed`. `duration_ms` is the elapsed wall time of the run for every status, and `fuel_consumed` is recorded whenever the runtime got far enough to know it (including fuel exhaustion, traps, and timeouts); it is null only when it is unknown, for example when the module does not compile.

## Scenario Templates

Creating a project with a scenario template initializes the project as a ready-to-use business workspace.

Current built-in templates:

- `code_delivery_default`
- `contract_review_default`
- `equipment_maintenance_default`
- `quality_corrective_action_default`
- `customer_delivery_default`
- `restaurant_ordering_default`

See `docs/scenario-templates.md` for the full scenario catalog: business fit,
generated forms, integration notes, MCP usage, frontend usage, and extension
rules for adding new scenarios.

Each template creates default forms and grid/detail views. The restaurant template additionally auto-installs and activates the `restaurant_calc` WASM plugin.

## Restaurant Reference Flow

The restaurant scenario is the delivery reference for universal business usage:

1. Create a restaurant project from `restaurant_ordering_default`.
2. Create menu category, SKU, and table records.
3. Create an order and order line.
4. The `restaurant_calc` formula plugin calculates `line_total`.
5. Link order line to order through `parent_child`.
6. Change table and emit `order.table_changed`.
7. Create kitchen and receipt `print_job` records.
8. Read the `print_job.created` business events from the events API or MCP `events.tail` and hand them to the printer integration.
9. Create `business_report` and query revenue through MCP aggregate.

For a local stack started with `bash scripts/start.sh`, this can be seeded
through the public API with:

```bash
scripts/bootstrap-restaurant-demo.sh
```

The demo helper creates or reuses a local user, workspace, restaurant scenario
project, sample menu/table/order/order-line/report records, a `parent_child`
link, and a workspace-scoped MCP bot token. When the MCP configuration file
exists it writes `mcp.bot_token` and `mcp.workspace_id` into it, then recreates
a running compose
`mcp-server` so MCP clients use the same demo workspace. If the MCP HTTP
endpoint is reachable, it verifies `/mcp/rpc` with `projects.list` and confirms
the demo project appears through MCP. It refuses non-local API URLs by default
so the built-in demo credentials are not accidentally used against production.
For a disposable verification of that full API -> bot token -> MCP HTTP path,
run `scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh`; it uses a temporary
database and a temporary configuration file.

## Verification

Backend and integration:

```bash
cargo fmt --all -- --check
cargo clippy -p api -p worker -p mcp-server --all-targets --all-features -- -D warnings
cargo test -p api routes::project::tests::
scripts/smoke-restaurant-demo-bootstrap-mcp-http.sh
cargo test -p api forms::
scripts/audit-universal-forms-docs.sh
scripts/smoke-forms-mcp.sh
scripts/smoke-scenario-template-forms.sh
bun run --cwd frontend smoke:restaurant-ordering
```

Frontend:

```bash
cd frontend
bun run check
bun run build
bun run smoke:forms-ui
bun run smoke:restaurant-ordering
```

Delivery requires the relevant checklist in `.flow-gate/universal-forms/docs/openpr-universal-form-development-execution-tracker-2026-05-31.md` to be marked `已测试` or `已验收` with command evidence.
