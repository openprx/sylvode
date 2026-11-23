# Plugins

Sylvode runs two kinds of project-scoped extensions:

| Kind | Manifest `schema_version` | What it does | Managed through |
|---|---|---|---|
| WASM plugin | `openpr.plugin.v1` | Server-side hooks on universal form records: field validators, formulas and event handlers | `/api/v1/projects/{project_id}/plugins`, `/api/v1/plugins/{plugin_id}`, MCP `plugins.*` tools |
| Form plugin package | `openpr.plugin.v3` | Sandboxed HTML panels, actions and project views in the web app, optionally with a WASM order amount calculation | Project settings "Plugins", "My plugin library", "Plugin Creator", `/api/v1/projects/{project_id}/plugin-packages` and the endpoints below |

Both kinds run WebAssembly through the same runtime and the same core ABI, described in
[`openpr-plugin-v1.wit`](openpr-plugin-v1.wit) and in [The WASM ABI](#the-wasm-abi) below.

For forms, fields and records themselves see [the forms guide](../guide/forms.md). For request
and response conventions of the REST API see [the API reference](../integrations/api.md).

## The WASM ABI

A plugin is a core WebAssembly module (not a component). The runtime looks for these exports:

| Export | Signature | Required |
|---|---|---|
| `memory` | linear memory | yes |
| `openpr_alloc` | `(len: i32) -> i32` | yes |
| `openpr_invoke` | `(ptr: i32, len: i32) -> i64` | yes |
| `openpr_plugin_abi_version` | `() -> i32`, must return `1` | no; when present, any other value fails the run |

One invocation works like this:

1. The host serialises the input as UTF-8 JSON and calls `openpr_alloc(len)`.
2. The host writes the input bytes into `memory` at the returned pointer.
3. The host calls `openpr_invoke(ptr, len)`. The guest returns a packed `i64`: the high 32 bits are
   the output pointer, the low 32 bits the output length.
4. The host reads that range from `memory` and parses it as JSON. The output may be at most 1 MiB.

The module is instantiated with no imports: there is no WASI, no host function and no network or
file access. A module that declares an import fails to instantiate. Every invocation compiles and
instantiates the module afresh in its own store, so no state survives between invocations.

Amounts and other decimal values stay JSON strings in both directions.

The input of a WASM plugin hook has this envelope (the WIT record `invocation-input` describes the
same three parts):

```json
{
  "hook_kind": "formula",
  "plugin_key": "order_calc",
  "payload": { "...": "hook-specific fields" }
}
```

Form plugin packages call their WASM guest with a different, fixed input; see
[The `wasm` profile](#the-wasm-profile).

## WASM plugins

### Manifest

```json
{
  "schema_version": "openpr.plugin.v1",
  "key": "order_calc",
  "name": "Order calculation",
  "version": "0.1.0",
  "description": "Computes line totals.",
  "capabilities": {
    "hooks": [
      { "kind": "formula", "form_key": "order_line", "field_key": "line_total" },
      { "kind": "field_validator", "form_key": "order", "field_key": "total_amount" },
      { "kind": "event_handler", "form_key": "order", "event_type": "form.record.created" }
    ],
    "tools": [
      { "name": "order_calc.today_revenue", "description": "Revenue for today", "input_schema": { "type": "object" } }
    ],
    "runtime": { "timeout_ms": 500, "fuel": 100000, "memory_bytes": 1048576 }
  }
}
```

| Field | Rules |
|---|---|
| `schema_version` | `openpr.plugin.v1`; it is the default when omitted, and any other value is refused |
| `key` | normalised to a form-style key (lowercase letters, digits, underscores) |
| `name`, `version` | required, non-empty after trimming |
| `description` | optional |
| `capabilities.hooks[].kind` | `field_validator`, `formula` or `event_handler` |
| `capabilities.hooks[].form_key`, `field_key` | optional filters; a hook without them matches every form or field |
| `capabilities.hooks[].event_type` | required for `event_handler` |
| `capabilities.tools[].name` | must start with the plugin key and contain a dot (`<plugin_key>.<tool>`), using lowercase letters, digits, underscores and dots only |
| `capabilities.tools[].input_schema` | replaced by `{"type": "object"}` when it is not an object |
| `capabilities.runtime` | resource limits, see [Limits](#limits) |

A tool declared in the manifest is not exposed as an MCP tool of its own. It can be run by name
through `POST /api/v1/plugins/{plugin_id}/invoke` or the MCP tool `plugins.invoke`, which accept
any hook kind or tool name that the manifest declares.

### Hooks

Hooks run automatically on universal form record writes, for every plugin of the project whose
status is `active` and that has a module, in plugin key order.

| Hook | When it runs | `payload` fields | Output the host reads | Effect |
|---|---|---|---|---|
| `formula` | On record create and update, before form calculations and schema validation | `workspace_id`, `project_id`, `form_id`, `form_key`, `field_key`, `values` (the submitted values) | `{"patch": {...}}` is merged into the values; otherwise, when the hook has a `field_key`, `{"value": ...}` sets that field | The patched values then go through calculation and validation. A failed or timed-out run rejects the write. |
| `field_validator` | On record create and update, after validation, before the record is stored; once per field present in the record values that matches the hook | `workspace_id`, `project_id`, `form_id`, `form_key`, `field_key`, `field_type`, `value`, `values` | `{"ok": false, "error": "..."}` (or `"message"`) rejects the value | The write fails with `field validator rejected value: ...`. A failed or timed-out run also rejects the write. |
| `event_handler` | After the write commits, for `form.record.created`, `form.record.updated` and `form.record.recalculated` events whose type equals the hook's `event_type` | `workspace_id`, `project_id`, `form_id`, `form_key`, `record_id`, `event_type`, `event_payload` | ignored | The run is recorded; its outcome does not change the write. |

### Limits

| Limit | Value |
|---|---|
| Module size | at most 4 MiB (4,194,304 bytes), checked at install before the module is decoded or compiled, and again at every invocation |
| `runtime.timeout_ms` | 1 to 30,000; default 1,000 |
| `runtime.fuel` | 1 to 1,000,000,000; default 5,000,000 |
| `runtime.memory_bytes` | 64 KiB to 128 MiB; default 16 MiB |
| Instances, memories, tables per store | one each |
| Table elements | at most 65,536, initial size included |
| Output | at most 1 MiB |

`runtime.timeout_ms` bounds the whole invocation from the moment the request starts: waiting for a
thread, compiling and instantiating the module, and running the guest. If the deadline passes
before the guest starts, the guest never runs; a running guest is interrupted. Compilation itself
cannot be interrupted, which is why the module size is capped.

Fuel is the deterministic CPU budget; running out of fuel traps the guest. A memory or table growth
beyond a limit returns `-1` to the guest. A module whose initial memory or table already exceeds the
limit, or that declares a second memory or table, fails to instantiate.

### Invocation log

Every run, automatic or through the invoke endpoint, writes a row to `plugin_invocations` and a
`plugin.invoked` business event. A run is recorded even when the request that started it is
cancelled: the run continues to its own deadline instead of stopping with the request.

| Field | Meaning |
|---|---|
| `hook_kind` | the hook kind or tool name that ran |
| `status` | `completed`, `failed` or `timeout` |
| `input`, `output` | the input envelope and the parsed output (`{}` when there is none) |
| `error_message` | why the run failed; a timeout reads `wasm execution timeout after {N}ms` |
| `duration_ms` | elapsed wall time of the run, for every status |
| `fuel_consumed` | fuel burnt, whenever the runtime got far enough to know it (including fuel exhaustion, traps and timeouts); null when unknown, for example when the module does not compile |

`timeout` means the `runtime.timeout_ms` deadline expired. Every other run that produced no output,
including fuel exhaustion and guest traps, is `failed`. A field validator that returns
`"ok": false` is recorded as `failed` with its rejection message. Invoking a plugin that has no
module records a `failed` run with `plugin has no wasm module`.

### Endpoints and MCP tools

The endpoints accept a user session or a workspace bot token with access to the project's
workspace.

| Method and path | Purpose |
|---|---|
| `GET /api/v1/projects/{project_id}/plugins` | List the project's plugins (`page`, `per_page`; default 50, at most 200) |
| `POST /api/v1/projects/{project_id}/plugins` | Install: `manifest`, optional `wasm_base64`, optional `status` |
| `GET /api/v1/plugins/{plugin_id}` | One plugin with its manifest and `wasm_sha256` |
| `PATCH /api/v1/plugins/{plugin_id}` | Change `status` |
| `POST /api/v1/plugins/{plugin_id}/invoke` | Run a declared hook kind or tool: `hook_kind`, `input` |
| `GET /api/v1/plugins/{plugin_id}/invocations` | Invocation log, newest first (`page`, `per_page`) |

`status` is `active` (the default), `disabled` or `failed`; only `active` plugins run. A project can
hold one plugin per key and version. Installing and status changes write `plugin.installed` and
`plugin.updated` business events. A plugin installed without `wasm_base64` is stored with its
manifest only; hooks skip it.

The MCP server offers the same operations as `plugins.list`, `plugins.get`, `plugins.install`,
`plugins.invoke` and `plugin_invocations.list`; see the [MCP server README](../../apps/mcp-server/README.md).

The `restaurant_ordering_default` scenario template installs an `openpr.plugin.v1` plugin named
`restaurant_calc` whose module returns fixed sample output; see
[scenario templates](../guide/scenario-templates.md).

### Building a plugin

Any toolchain that produces a core WebAssembly module with the exports above works. With Rust, build
a `no_std` `cdylib` for `wasm32-wasip1` with no entry point, as the example guest in
[`extension-example/guest.rs`](extension-example/guest.rs) does (the command is in
[`extension-example/build.py`](extension-example/build.py)):

```bash
rustc guest.rs --edition=2024 --target=wasm32-wasip1 --crate-type=cdylib -O \
  -C panic=abort -C strip=symbols -C link-arg=--no-entry -o plugin.wasm
```

Then install it with the module base64-encoded:

```bash
curl -sS -X POST "https://sylvode.example.com/api/v1/projects/$PROJECT_ID/plugins" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d "{\"manifest\": $(cat manifest.json), \"wasm_base64\": \"$(base64 < plugin.wasm | tr -d '\n')\"}"
```

## Form plugin packages

A form plugin package adds user interface to a project: panels on a record, actions on a record or
a form list, and full-page project views. Its HTML runs in a sandboxed frame and talks to the web
app only through a message bridge. A package in the `wasm` profile also carries a WASM guest that
calculates an order amount.

### Required switches

Form plugin packages are off by default. Two environment variables of the `api` process control
them, and both must be set to exactly `1` for packages to work:

| Variable | Read in | Gates |
|---|---|---|
| `SV_FORM_PLUGIN_PACKAGES_ENABLED` | `apps/api/src/routes/plugin_package.rs` (`enabled`) | Every package endpoint: the project catalog, install, binding, invocation log, the personal library, contributions, data reads and invocations. When it is not `1`, these endpoints answer that form plugin packages are disabled. |
| `SV_FORM_PLUGIN_EXTENSIONS_ENABLED` | `apps/api/src/routes/plugin_package.rs` (`extensions_enabled`, `require_extensions`) | Installing or saving an `openpr.plugin.v3` package, binding a package, attaching a library release to a project, approving a request, contribution discovery and entry, data reads, amount invocations and configuration reset. When it is not `1`, these fail with reason `extensions_disabled`; the catalog reports `"extension_supported": false`, and disabling a binding still works. |

Both are read from the process environment on each request; restart the `api` process after
changing them. The Plugin Creator also needs both.

With Docker Compose, `docker-compose.yml` passes only `SV_FORM_PLUGIN_PACKAGES_ENABLED` to the `api`
service (from `.env`, default `0`). It does not pass `SV_FORM_PLUGIN_EXTENSIONS_ENABLED`, so to use
form plugin packages under Compose, add that variable to the `api` service's `environment` block
as well:

```yaml
    environment:
      SV_FORM_PLUGIN_PACKAGES_ENABLED: "${SV_FORM_PLUGIN_PACKAGES_ENABLED:-0}"
      SV_FORM_PLUGIN_EXTENSIONS_ENABLED: "${SV_FORM_PLUGIN_EXTENSIONS_ENABLED:-0}"
```

and set both to `1` in `.env`. Outside Compose, set both in the environment of the `api` binary.
See [configuration](../configuration.md) for every environment variable Sylvode reads.

### Package format

An install request is one JSON object, which is also the `.json` file that the project settings
"Install package" field accepts:

| Field | Content |
|---|---|
| `manifest` | the `openpr.plugin.v3` manifest |
| `web_base64` | base64 of the panel HTML (`panel.html`) |
| `wasm_base64` | base64 of the WASM guest (`plugin.wasm`); present only for the `wasm` profile |

The manifest:

```json
{
  "schema_version": "openpr.plugin.v3",
  "key": "order_board",
  "name": "Order board",
  "version": "1.0.0",
  "publisher": "Example publisher",
  "host_protocol": { "min": 3, "max": 3 },
  "profile": "ui",
  "bridge_version": 2,
  "entry": { "path": "panel.html", "size": 5120, "sha256": "<64 lowercase hex>" },
  "wasm": null,
  "contributions": [
    { "id": "board", "kind": "view", "slot": "project.view", "label": "Order board",
      "view": "board", "route": "board", "form_key": null },
    { "id": "record_summary", "kind": "panel", "slot": "form.record.detail", "label": "Order summary",
      "view": "summary", "route": null, "form_key": "order_line" },
    { "id": "list_tool", "kind": "action", "slot": "form.list.toolbar", "label": "Order totals",
      "view": "totals", "route": null, "form_key": "order_line" }
  ],
  "reads": [ { "id": "orders", "form_key": "order_line", "fields": ["quantity", "unit_price", "amount"] } ],
  "capabilities": []
}
```

Unknown fields are refused everywhere in the manifest.

| Field | Rules |
|---|---|
| `schema_version` | `openpr.plugin.v3`; any other value fails with reason `manifest_protocol_unsupported` |
| `host_protocol`, `bridge_version` | exactly `{"min": 3, "max": 3}` and `2` |
| `key` | lowercase letters, digits and underscores, at most 64 characters; `library` is reserved |
| `name`, `publisher` | non-empty display labels of at most 120 characters, no surrounding spaces or control characters |
| `version` | a SemVer version, at most 64 characters |
| `profile` | `ui` or `wasm` |
| `entry` | `path` is `panel.html`; `size` 1 byte to 64 KiB; `sha256` is lowercase hex of the HTML |
| `wasm` | `null` for `ui`; for `wasm` see [The `wasm` profile](#the-wasm-profile) |
| `contributions` | 1 to 8 entries, see below |
| `reads` | at most 4 entries: `id` (key, at most 32 characters), `form_key`, `fields` (1 to 32 unique field keys) |
| `capabilities` | empty for `ui`; the two amount capabilities for `wasm` |

Each contribution has a unique `id`, a `label`, a `view` name (passed to the panel), and one of
these slot and kind pairs:

| `slot` | `kind` | Target | Appears |
|---|---|---|---|
| `form.record.detail` | `panel` | `form_key`, no `route` | on the record page of that form |
| `form.record.actions` | `action` | `form_key`, no `route` | among the record actions of that form |
| `form.list.toolbar` | `action` | `form_key`, no `route` | in the record list toolbar of that form |
| `project.view` | `view` | `route` (key, at most 32 characters), no `form_key` | as a project app page at `/workspace/{workspaceId}/projects/{projectId}/apps/{pluginKey}/{route}` |

A package declares at most two project views, and each slot and target pair at most once.

The install checks the uploaded assets against the manifest: a size or digest mismatch fails with
`asset_digest_mismatch`, and assets that do not fit the profile fail with `profile_mismatch`. Error
responses carry a stable reason string in `details.reason`.

### The `wasm` profile

A `wasm` package declares:

```json
"wasm": { "abi": "openpr.plugin.v1", "path": "plugin.wasm", "size": 4492,
          "sha256": "<64 lowercase hex>", "form_key": "order_line" }
```

- `abi` is `openpr.plugin.v1`: the guest implements [the WASM ABI](#the-wasm-abi).
- `size` is 1 byte to 256 KiB.
- At least one `form.record.detail` or `form.record.actions` contribution must target `form_key`
  (otherwise `capability_target_missing`).
- `capabilities` holds exactly `amount.preview` (permission `record.read`) and `amount.apply`
  (permission `record.update`), each with input schema
  `{"type": "object", "additionalProperties": false}` and an output schema of one integer
  `amount_cents` from 0 to 1,000,000,000,000. [`extension-example/build.py`](extension-example/build.py)
  writes these declarations.

The guest receives `{"capability": "amount.preview", "quantity": 2, "unit_price_cents": 1250}` and
must return exactly `{"amount_cents": <integer>}`. Every install runs both capabilities once against
that sample input; a guest that fails fails the install with `wasm_probe_failed`.

The amount capabilities work on forms with `number` fields `quantity`, `unit_price` and `amount`.
The host reads `quantity` (an integer from 1 to 1,000,000) and `unit_price` (0 to 10,000,000 with
at most two decimal places) from the record, and `amount.apply` writes the result to `amount` as a
decimal string. `amount.apply` requires a preceding `amount.preview`.

Package runs use fixed limits instead of a manifest policy: 250 ms, 500,000 fuel and 2 MiB of
memory. At most four package runs (install probes and invocations) execute at once in one `api`
process; a run beyond that is refused at once with "plugin execution capacity reached; retry
shortly".

### Lifecycle: install, bind, grant

Only workspace owners and admins manage a project's packages.

1. **Install.** Upload a package in the project settings "Plugins" page, or
   `POST /api/v1/projects/{project_id}/plugin-packages`. Installing the same key and version again
   with identical content returns the existing package; different content under the same key and
   version is a conflict. A project holds at most 32 packages.
2. **Bind.** Each key has one binding that selects the active package version, with a
   `generation` counter. `POST /api/v1/projects/{project_id}/plugin-packages/{key}/binding` takes
   `package_id` (or `null` to disable), `expected_generation`, a fresh `request_id` and `grants`,
   and optional `placements`. A stale `expected_generation` fails with
   `form_plugin_binding_changed`; repeating a `request_id` with the same body returns the stored
   response.
3. **Grant.** `grants` are what the package may do in this project:
   - `ui.display` lets its contributions appear; any non-empty grant list must include it.
   - `read:<id>` allows the declared read with that `id`.
   - `amount.preview` and `amount.apply` allow the amount capabilities; `amount.apply` requires
     `amount.preview`.
   A disable request carries no grants and no placements.
4. **Place.** `placements` lists every declared contribution exactly once with `enabled` and a
   `position` from 0 to 99. Contributions without a stored placement are enabled at position 50.

`GET /api/v1/projects/{project_id}/plugin-packages` returns the installed packages, the bindings,
the latest binding history and `extension_supported`.
`GET /api/v1/projects/{project_id}/plugin-packages/invocations` lists the latest amount invocations
with their status, duration and fuel.

### Personal library and project requests

Any signed-in user can keep private package versions in "My plugin library" (`/plugins`):

| Method and path | Purpose |
|---|---|
| `GET /api/v1/me/form-plugin-packages` | The caller's saved versions and the projects they can attach them to |
| `POST /api/v1/me/form-plugin-packages` | Save a version (same body as a project install); at most 128 versions per user |
| `DELETE /api/v1/me/form-plugin-packages/{package_id}` | Archive a version; its pending project requests are revoked |
| `GET /api/v1/me/form-plugin-packages/{package_id}/panel` | The version's panel, for preview |
| `POST /api/v1/me/form-plugin-packages/{package_id}/preview` | Run the amount calculation of a `wasm` version: `{"quantity": 2, "unit_price_cents": 1250}` |
| `POST /api/v1/projects/{project_id}/plugin-packages/library/{package_id}` | Attach the version to a project |
| `DELETE /api/v1/projects/{project_id}/plugin-packages/library/{package_id}` | Revoke the link (release owner or project admin) |
| `GET /api/v1/projects/{project_id}/plugin-packages/library-requests` | Pending requests (project admins) |
| `POST /api/v1/projects/{project_id}/plugin-packages/library-requests/{package_id}` | Approve or reject: `{"approve": true}` |

When a workspace owner or admin attaches a version, it is installed into the project at once. When
another member attaches it, the request is `pending` until a project admin approves it. A saved key
and version cannot be reused with different content; publish a new version instead.

### Plugin Creator

The Plugin Creator (`/plugins/creator`, or from a project's plugin settings) builds a package from
a built-in template: an order amount template in the `wasm` profile, and a `ui` view template. You
edit labels, texts, the key, version and form key, preview the result in the sandbox with a sample
order and review the requested permissions. You can export the package file; the personal Creator
saves the version to your library, and the Creator opened from a project installs it into that
project. The target form needs `number` fields `quantity`, `unit_price` and `amount`.

### Panels and the message bridge

The web app loads each contribution in an `<iframe sandbox="allow-scripts">` with an opaque origin
and hands the panel a `MessagePort`. The panel cannot call the API directly; everything goes
through the bridge, and the host checks every request against the binding's grants.

1. The frame receives a `window` message `{"type": "sylvode-plugin-connect", "protocol": 2, ...}`
   from its parent with one port, the contribution `context` and the `grants`.
2. The panel replies on the port with `{"protocol": 2, "type": "ready"}`.
3. Requests are `{"protocol": 2, "type": "request", "request_id": "<uuid>", "method": "...", "params": {...}}`;
   the host answers `{"protocol": 2, "type": "result", "request_id": "...", "ok": true, "result": ...}`
   or `"ok": false` with an `error`.
4. The panel may send `{"type": "resize", "height": <px>}` and `{"type": "close"}`.
5. The host sends events `{"type": "event", "name": "context", "data": ...}` when the context changes,
   and `{"type": "event", "name": "suspended"}` when access is withdrawn.

| Method | Purpose | Grant |
|---|---|---|
| `host.context` | the current contribution context | `ui.display` |
| `data.describe` | the fields of a declared read | `read:<id>` |
| `data.query` | a page of records of a declared read (`limit` up to 50, `cursor`) | `read:<id>` |
| `data.record` | one record of a declared read | `read:<id>` |
| `host.navigate` | navigate to a project view, a record or the project | `ui.display` |
| `amount.preview`, `amount.apply` | the amount capabilities on the current record | `amount.preview`, `amount.apply` |

Bridge error codes are `method_unavailable`, `invalid_params`, `busy`, `denied`, `stale`,
`not_found`, `rate_limited`, `timeout`, `canceled`, `preview_required` and `failed`.

Data reads return only the declared fields that exist, that the caller's field permissions allow and
whose type is readable; media, people, relation and generated summary fields are never returned.
Record titles are not returned. Data reads are rate limited per user, project and package key.

Users can hide, collapse or reorder contributions for themselves; these preferences
(`/api/v1/projects/{project_id}/plugin-preferences/{key}/{contribution_id}`) affect only that user and are not a
security boundary.

## The example in `extension-example/`

[`extension-example/`](extension-example/) holds a complete pair of sample packages and a backend
acceptance script.

| File | Purpose |
|---|---|
| [`panel-view.html`](extension-example/panel-view.html) | The panel used by both samples. It connects to the bridge, and when the `read:orders` grant is present, shows up to 20 order lines through `data.describe` and `data.query`. In the project view it adds a Refresh button. |
| [`guest.rs`](extension-example/guest.rs) | A `no_std` Rust WASM guest for the amount capabilities: it reads `quantity` and `unit_price_cents` and returns `amount_cents`. |
| [`build.py`](extension-example/build.py) | Writes two install request files into an output directory: `extension-ui-1.0.0.json` (key `order_board`, `ui` profile) and `extension-wasm-1.0.0.json` (key `order_board_amount`, `wasm` profile, which compiles `guest.rs`). |
| [`verify_extension.py`](extension-example/verify_extension.py) | Drives the package API end to end against a disposable API and database. |

Build the samples (the `wasm` sample needs `rustc` with the `wasm32-wasip1` target):

```bash
python3 docs/plugins/extension-example/build.py /srv/sylvode-samples --form-key order_line
```

Install one of them from a project's settings "Plugins" page ("Install package"), or with the API:

```bash
curl -sS -X POST "https://sylvode.example.com/api/v1/projects/$PROJECT_ID/plugin-packages" \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  --data @/srv/sylvode-samples/extension-ui-1.0.0.json
```

The project needs a form with key `order_line` (or the key given to `--form-key`); for the amount
sample it needs `number` fields `quantity`, `unit_price` and `amount`. Then review access for the
package, grant `ui.display` and `read:orders` (and the amount capabilities for the `wasm` sample),
and open a record of that form or the "Order board" project view.

`verify_extension.py` registers its own users, creates workspaces, projects and records, and
exercises install, binding, grants, data reads and invocations. Run it only against a throwaway API
with an empty database, started with both switches set to `1`:

```bash
python3 docs/plugins/extension-example/verify_extension.py \
  --base http://127.0.0.1:8081 --context /srv/sylvode-verify/context.json
```

After it passes, restart that API with `SV_FORM_PLUGIN_EXTENSIONS_ENABLED=0` and run
`--phase gate-off` with the same `--context` file to check that the switch refuses installs,
discovery, entry and binding while still allowing a binding to be disabled.
