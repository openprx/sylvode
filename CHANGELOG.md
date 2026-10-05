# Sylvode Webhook Changelog

Sylvode Webhook was called OpenPR Webhook up to 0.3.3. Releases before 0.3.9 are described on the
[GitHub releases page](https://github.com/openprx/openpr-webhook/releases).

## 0.3.10

### Fixed

- **Default templates are filled in.** An `openclaw`, `openprx` or `custom` agent without a
  `message_template`, and a `cli` agent without a `prompt_template` (on the webhook path and the
  tunnel path alike), received the literal text `[TPL_PROJECT] TPL_EVENT: ...` or
  `Fix issue ISSUE_ID: TITLE ...` instead of a message. The defaults are now
  `[{project}] {event}: {key} {title}` / `{actor} | Trigger: {reason}` and
  `Fix issue {issue_id}: {title}` / `Context: {reason}` (each on two lines), rendered like a
  configured template.
- Templates are rendered in a single pass. A placeholder that appears inside a value taken from the
  payload, such as an issue title containing `{actor}`, is no longer expanded.

## 0.3.9

The project is renamed **Sylvode Webhook**, following the main product
([openprx/sylvode](https://github.com/openprx/sylvode)). Every legacy name keeps working for the
whole 1.x line and is not removed before v2.0; using one writes a single deprecation line to
stderr per process that names the replacement. See the migration table in the
[README](README.md#migrating-from-openpr-webhook).

### Added

- The canonical executable `sylvode-webhook`. The legacy `openpr-webhook` executable is still
  built from the same code and behaves identically on stdout and in its exit codes.
- `--help` and `--version`. Their output names `sylvode-webhook` under both executable names.
- The inbound signature header `X-Sylvode-Signature`, next to `X-Webhook-Signature`.
  `X-OpenPR-Signature` is still accepted, with a deprecation notice.
- The outbound attribution header `X-Sylvode-MCP-Surface` on plain API callbacks, sent together
  with the legacy `X-OpenPR-MCP-Surface` carrying the same value.
- The environment variable `SYLVODE_WEBHOOK_SAFE_MODE`. `OPENPR_WEBHOOK_SAFE_MODE` is still
  honoured, with a deprecation notice; when both are set and disagree, start-up is refused.
- A container image, `ghcr.io/openprx/sylvode-webhook`, also published as
  `ghcr.io/openprx/openpr-webhook` with the same digest. It contains both executables and runs as
  a non-root user.
- Release archives `sylvode-webhook-<platform>.tar.gz` containing both executables, with
  `openpr-webhook-<platform>.tar.gz` aliases of the same bytes; each has its own `.sha256`.
- Graceful shutdown on SIGTERM and SIGINT.
- `LICENSE-MIT` next to `LICENSE-APACHE` (the license was already `MIT OR Apache-2.0` in
  `Cargo.toml`), `CONTRIBUTING.md`, `SECURITY.md` and this changelog.

### Changed

- **Conflicting signature headers are refused.** All signature headers present are now read. When
  they carry different signatures, or one header is repeated with different values, the request is
  answered `401` even if one of them is valid. Previously the first header present decided, so an
  invalid signature could ride along a valid one. A non-ASCII signature value is now refused
  instead of skipped.
- The crate is renamed `sylvode-webhook`, so this service's log target is `sylvode_webhook`.
  `RUST_LOG` directives for `openpr_webhook` are rewritten to the new target with a deprecation
  notice; a legacy directive that names the same target as a canonical one with a different level
  refuses start-up. Log lines show the new target, and the start-up line reads
  `sylvode-webhook listening on ...`.
- The installer creates `sylvode-webhook.service` (systemd) or `dev.sylvode.webhook` (launchd). An
  existing `openpr-webhook.service` or `dev.openpr.webhook` is updated in place under its old name;
  having both is refused. The uninstaller removes both.
- A SIGTERM now ends the process with exit code 0 instead of termination by the signal.
- A config path that is not valid UTF-8 is reported as an error instead of panicking.
- Prompt text given to CLI agents and the examples now name Sylvode. The examples no longer suggest
  `OPENPR_*` environment variables for the MCP server, which reads its own configuration file.

### Unchanged

The configuration format and default path `config.toml`, the `/webhook` and `/health` routes, the
outbound `X-Webhook-Signature`, the `openprx` agent type and the repository URL. The tunnel
`agent_id` has no default: the unused `openpr-webhook` fallback was removed, and start-up already
required the value to be configured.
