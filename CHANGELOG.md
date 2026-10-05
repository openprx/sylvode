# Sylvode Webhook Changelog

Sylvode Webhook was called OpenPR Webhook up to 0.3.3. Releases before 0.3.9 are described on the
[GitHub releases page](https://github.com/openprx/openpr-webhook/releases).

## 0.3.18

### Security

- **A blank inbound secret refuses start-up.** An empty or whitespace-only entry in
  `security.webhook_secrets`, or a present but blank `tunnel.hmac_secret`, is refused with an error
  naming the setting (and the list position). Before, an empty entry was used as an HMAC key that
  anyone can compute, so with `webhook_secrets = [""]` any request signed with the empty key was
  accepted. A blank `hmac_secret` is refused whether or not the tunnel is enabled and whatever
  `require_inbound_sig` says.
- Signature verification never accepts a signature under an empty or blank key, even if
  configuration validation were bypassed.

## 0.3.17

### Fixed

- The HTTP signature integration test left its `sylvode-webhook` server running when an assertion
  failed, because the process was only killed on the success path. The test server is now killed
  and reaped when it is dropped, on every exit path including a panic, before its temporary
  directory is removed.

## 0.3.16

### Security

- Start-up is refused when an enabled tunnel sets `require_inbound_sig = true` without a non-empty
  `hmac_secret`. Without a secret the inbound `sig` was never verified, so the setting only checked
  that the field existed and authenticated nothing. The error names both settings.

## 0.3.15

### Security

- Inbound signatures, on `/webhook` and on the tunnel, are compared in constant time with
  `Mac::verify_slice` instead of as text, so response timing no longer reveals how much of a
  forged signature matched. Both paths verify through one function.
- A signature must be exactly 64 lowercase hex digits after the optional `sha256=` prefix; anything
  else is refused before an HMAC is computed. This is what was accepted before: uppercase hex,
  odd-length or non-hex values and digests of the wrong length were already refused, and a
  prefix is still stripped as before.

## 0.3.14

### Changed

- Deprecation notices, `--help`, the installer and the documentation say a legacy name is "not
  removed before Sylvode v2.0" instead of "v2.0". The deprecation window follows the Sylvode
  product's releases, not this crate's version.

### Documentation

The README was checked against the code and the Sylvode API and corrected:

- It listed "proposal submitted" as an event. It now lists the 14 events a Sylvode webhook can
  subscribe to, and says a delivery carries `bot_context` only when the webhook has a bot user that
  is assigned to the issue or mentioned in the comment.
- It promised "multi-agent dispatch". A bot task goes to exactly one agent; the selection order and
  the route matching rules are now described, including the trigger kind values and which route
  fields current Sylvode tasks actually carry.
- It said a tunnel envelope without `sig` is accepted. With `hmac_secret` set, every inbound
  envelope must carry a valid `sig`; without it, `sig` is not verified and `require_inbound_sig`
  (now documented) only refuses envelopes that have none.
- The `webhook` agent example set a `message_template` and pointed at Slack. A `webhook` agent
  forwards the delivery JSON unchanged and renders no message.
- The placeholder table missed most placeholders and did not say which apply to prompts. It now
  lists all of them, the defaults, and how missing values render.
- Newly documented: what each agent type runs or sends, callback timing and formats, what
  `[runtime]` settings limit, tunnel task routing and requirements, and the `/webhook` response
  codes.
- The documentation link points at `https://docs.openprx.dev/en/sylvode-webhook/`.

## 0.3.13

### Added

- `args` for `openclaw` and `openprx` agents: fixed arguments placed between `command` and
  `--channel`. `openclaw message send` is now written `command = "openclaw"` with
  `args = ["message", "send"]`.

### Changed

- **A `command` containing whitespace that names no program refuses start-up** (`openclaw.command`,
  `openprx.command` and `custom.command`). Commands run without a shell, so a value such as
  `openclaw message send` was looked up as a single program of that name and every dispatch failed
  to spawn. The error shows the `command` and `args` to write instead. A path that contains
  whitespace and names an existing file is still accepted. The README and `config.example.toml`
  showed `command = "openclaw message send"`; they now use `command` and `args`.

## 0.3.12

### Fixed

- **`{message}` in a `custom` agent's `args` is replaced by the rendered message**, as the README
  example `args = ["{message}"]` always implied; before, the literal `{message}` was passed and the
  message was appended after it. Each argument stays exactly one argument whatever the message
  contains; there is no shell and nothing is split. Without the placeholder the message is still
  appended as the last argument.
- A `custom` agent is not run when the message would start an argument with `-` (so payload text
  could be read as an option of the command), unless an earlier argument is `--`. The dispatch
  result is a `custom_error:` line saying so. An argument written with a leading `-`, such as
  `--text={message}`, is not affected.

## 0.3.11

### Changed

- **An unknown `route.trigger_kinds` value refuses start-up.** The error names the agent and lists
  the values Sylvode sends: `assigned`, `created`, `status_changed`, `mentioned`, `completed` and
  `failed` on webhook deliveries (`bot_context.trigger_reason`), and `assigned`, `mention`,
  `proposal_vote` and `manual` on worker-dispatched AI tasks (`trigger_kind`). Before, such a value
  was accepted and the agent was never selected. The examples used `assignment`, which no Sylvode
  version has sent, and `mention` for a mention in a comment, which a webhook delivery reports as
  `mentioned`; they now use `assigned` and `mentioned`.

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
whole Sylvode 1.x line and is not removed before Sylvode v2.0; using one writes a single deprecation line to
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
