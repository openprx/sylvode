# Sylvode v1.0 compatibility matrix

This matrix supersedes the [v0.9 compatibility matrix](sylvode-v0.9-compatibility.md), which is
kept unchanged as the record of what v0.9 shipped. It implements ADR-0020 ("OpenPR compatibility
layer and brand completion").

Sylvode is the default product name. The transition does not rename stable protocols, historical
database objects, or user data. Compatibility aliases are implemented by the same code paths as
their canonical forms; they are not copied data or a second registry.

Every legacy entry point below keeps working, with the same behaviour as its canonical form,
throughout the 1.x series. Starting with v1.0 each one produces a user-visible deprecation
warning. **Nothing is removed before Sylvode v2.0**, and a removal additionally requires its own
accepted decision record; v1.0 removes nothing.

| Surface | v1.0 default | OpenPR compatibility | Conflict policy | Warning starts | Earliest removal |
| --- | --- | --- | --- | --- | --- |
| Product, Web UI, current docs | Sylvode | Historical documents may say “OpenPR” | Not applicable | v0.9 docs identify historical names | Not applicable |
| CLI | `sylvode`: the Flow commands (`features`, `objects`, `collections`, `records`, `collab`, `deliveries`) and the workspace commands (`projects`, `work-items`, `comments`, `labels`, `sprints`, `search`, `files`, `operation-logs`, `tools`) | `mcp-server` remains a shipped executable. Its workspace commands are the same commands `sylvode` runs — one definition and one handler — so stdout bytes and exit codes are identical under both names. The Flow commands exist only under `sylvode`; `serve` exists only under `mcp-server` | Explicit executable name selects the display surface | v1.0: `mcp-server <command>` other than `serve` writes one stderr line per process naming the replacement `sylvode <command>`; stdout and the exit code are unchanged. `mcp-server serve` does not warn on any transport | Not before Sylvode v2.0 and a separately accepted removal decision |
| Configuration | `config/sylvode.toml`; compose uses `sylvode.compose*.toml` | `config/openpr.toml` and `openpr.compose*.toml` are discovered only when the canonical file is absent | Both files present is an error; pass `--config` to select explicitly | v1.0: default discovery of `config/openpr.toml` logs one `warn` line per process in `api`, `worker`, `mcp-server` and `sylvode` (always on stderr for `mcp-server` and `sylvode`). An explicit `--config config/openpr.toml` does not warn. `scripts/start.sh` warns on stderr when it selects `openpr.compose*.toml` | Not before Sylvode v2.0 and a separately accepted removal decision |
| Compose environment | `SYLVODE_BIND_HOST`, `SYLVODE_API_PORT`, `SYLVODE_FRONTEND_PORT`, `SYLVODE_MCP_PORT`, `SYLVODE_RUNTIME_BASE`, `SYLVODE_FRONTEND_DOCKERFILE`, and `SYLVODE_WEBHOOK_*` | `OPENPR_BIND_HOST`, `OPENPR_API_PORT`, `OPENPR_FRONTEND_PORT`, `MCP_SERVER_PORT`, `OPENPR_RUNTIME_BASE`, `OPENPR_FRONTEND_DOCKERFILE`, and `OPENPR_WEBHOOK_*` remain readable | Equal values are accepted; different canonical and legacy values are an error | v1.0: `scripts/start.sh` (through `scripts/lib/sylvode_compat.sh`) writes one stderr line per legacy variable it reads | Not before Sylvode v2.0 and a separately accepted removal decision |
| REST API and schema IDs | `/api/v1` and the existing schema identifiers | Preserved byte-for-byte; no branded API prefix is introduced | Stable identifiers win; branding is display-only | None | No removal planned |
| MCP tools | Existing semantic tool names | Tool names and input/output schemas are unchanged | No alternate branded tool registry | None | No removal planned |
| MCP resources | `sylvode://` for every static resource and template | The corresponding `openpr://` URI reads the exact canonical body and returns `_meta.canonical_uri`; list endpoints emit canonical URIs only | Canonical URI is the identity | v1.0: an `openpr://` read also returns `_meta.deprecation = {"replaced_by": "<canonical sylvode:// URI>", "earliest_removal": "2.0"}` on every transport; a `sylvode://` read carries no `_meta.deprecation` | Not before Sylvode v2.0 and a separately accepted removal decision |
| MCP attribution headers | `X-Sylvode-MCP-Surface` and `X-Sylvode-MCP-Tool` | `mcp-server` also sends `X-OpenPR-MCP-Surface` and `X-OpenPR-MCP-Tool` with identical values, so a new `mcp-server` attributes correctly against an older API; the API accepts either set alone | The canonical and legacy header of a field, or repeated occurrences of either, carrying different values are refused (envelope code 401); the API never picks one, because the values are audit attribution | None (protocol headers, not a user entry point) | Not before Sylvode v2.0 and a separately accepted removal decision |
| Attachment count headers | `X-Sylvode-Attachment-Count` and `X-Sylvode-Attachment-File-Count` on attachment package downloads | `X-OpenPR-Attachment-Count` and `X-OpenPR-Attachment-File-Count` are returned alongside, with identical values, built by the same helper | Not applicable: both are always sent with the same value | None | Not before Sylvode v2.0 and a separately accepted removal decision |
| Webhook `User-Agent` | Outbound deliveries send `Sylvode-Webhook/1.0 (compatible; OpenPR-Webhook/1.0)`; the `request_headers` recorded with the delivery carry the same value | The legacy `OpenPR-Webhook/1.0` product token stays in the comment section, so receivers matching it as a **substring** are unaffected | A `User-Agent` cannot be sent twice. Receivers that match the old value **exactly** or as a **prefix** no longer match and must be updated; this is a behaviour change called out in the release notes | None | The legacy token in the comment section: not before Sylvode v2.0 and a separately accepted removal decision |
| Database and migrations | Existing physical names | Historical OpenPR-named databases, roles, tables, columns, and migrations are not renamed | Existing physical identity is authoritative | None | No removal planned |
| Release archives | `sylvode-<target>` | A matching `openpr-<target>` archive alias contains the same binaries | Canonical archive is shown first | v0.9 release notes | Not before Sylvode v2.0 and a separately accepted removal decision |
| Telemetry and containers | Sylvode user-facing descriptions | Stable service labels (`api`, `worker`, `mcp-server`, `frontend`) are preserved so existing dashboards continue across upgrade | Service identity remains stable; product display changes | None | No removal planned |

The compatibility contract is intentionally fail-closed where two operator inputs disagree. This
avoids a deployment changing ports or configuration merely because a new alias was added.
Explicit `--config` remains available when both files must be kept temporarily.

## Deprecation warnings

Every warning names the legacy name in use, its replacement and the earliest release that may
remove it. The texts are defined once in `crates/platform/src/deprecation.rs` (binaries) and
`scripts/lib/sylvode_compat.sh` (scripts). A warning never changes stdout or the exit code, and a
warning that cannot be written does not fail the command.

| Legacy use | Where | Text |
| --- | --- | --- |
| `mcp-server projects list` (any subcommand except `serve`) | stderr, once per process | ``warning: `mcp-server projects list` is deprecated; use `sylvode projects list` instead (the mcp-server CLI subcommands are not removed before Sylvode v2.0)`` |
| `config/openpr.toml` found by default discovery | one `warn` log line per process | `legacy configuration file config/openpr.toml was discovered by default; rename it to config/sylvode.toml or pass --config explicitly (default discovery of config/openpr.toml is not removed before Sylvode v2.0)` |
| `OPENPR_API_PORT` (any legacy compose variable) | stderr, once per variable per run | `warning: legacy compose variable OPENPR_API_PORT is deprecated; use SYLVODE_API_PORT instead (OPENPR_API_PORT is not removed before Sylvode v2.0)` |
| `./config/openpr.compose.toml` selected by `scripts/start.sh` | stderr, once per file per run | `warning: legacy configuration file ./config/openpr.compose.toml is deprecated; use ./config/sylvode.compose.toml instead (./config/openpr.compose.toml is not removed before Sylvode v2.0)` |
| `openpr://…` resource read | `_meta.deprecation` in the `resources/read` result | `{"replaced_by": "sylvode://…", "earliest_removal": "2.0"}` |

Only subcommand names appear in the CLI warning, never argument values, so a token passed on the
command line is not echoed. The CLI and resource warnings never reach stdout: for the workspace
commands stdout is a machine contract, and for `serve --transport stdio` it is the MCP protocol
channel.

## Conventions of the workspace commands under `sylvode`

The nine workspace commands keep the conventions they have always had under `mcp-server`, so a
script can switch executable names without any other change. They differ from the Flow commands'
`sylvode.cli.v1` conventions:

- `--format json` prints the tool's JSON text as returned by the API; the Flow commands print the
  `sylvode.cli.v1` envelope.
- A failed call prints its message on stderr and exits 1; the Flow commands print a failure
  envelope on stdout and exit with the `error-mapping-v1` code.
- They accept `--workspace-id` to override `mcp.workspace_id`; the Flow commands take the
  workspace as a per-command `--workspace` argument.

## Not renamed

These identifiers keep their OpenPR-era names, with no removal planned:

- REST `/api/v1` and its schema identifiers.
- MCP tool names and their input and output schemas.
- Database names, roles, tables, columns and historical migrations.
- The plugin ABI: `openpr_alloc`, `openpr_invoke`, `openpr_plugin_abi_version` and
  `openpr-plugin-v1.wit`.
- The bot token prefix `opr_`.
- Compose service labels (`api`, `worker`, `mcp-server`, `frontend`) and network names.
- `OPENPR_TEST_DATABASE_URL`, a development and CI variable rather than a user entry point.

## Upgrade sequence

1. Back up the database and the existing configuration.
2. Install the v1.0 binaries. Existing `mcp-server` invocations and old configuration continue
   to work; they now print the deprecation warnings above.
3. Replace `mcp-server <command>` with `sylvode <command>` in scripts; the output and exit codes
   are identical.
4. Copy the legacy configuration to the Sylvode filename, compare it, then remove or archive the
   old file before relying on default discovery.
5. Move compose variables to their `SYLVODE_*` names without defining conflicting values. No
   data-directory or database rename is required.
6. Update webhook receivers that match the `User-Agent` exactly or as a prefix.
7. Read MCP resources through `sylvode://` URIs.

Rollback to v0.9 retains every legacy name; the v1.0 additions (warnings, the new headers,
`_meta.deprecation`) are ignored by the older application.
