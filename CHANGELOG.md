# Sylvode Webhook Changelog

Sylvode Webhook was called OpenPR Webhook up to 0.3.3, the last release under that name; it and
earlier releases are described on the
[GitHub releases page](https://github.com/openprx/openpr-webhook/releases). Versions 0.3.4 to 0.3.8
were never released: their changes are listed under 0.3.9. Versions 0.3.9 to 0.3.43 are released
together as 0.3.43.

## Upgrading from 0.3.3

### Breaking: configurations that started with 0.3.3 and are refused now

Each exits 1 at start-up with the error shown (logged as `failed to load config from <path>:
<error>` unless noted). The README's [Upgrade checks](README.md#upgrade-checks) has the same list.

1. `route.trigger_kinds` with a value Sylvode never sends, such as `assignment`:
   ``agent `X`: route.trigger_kinds contains `assignment`, which Sylvode never sends; valid values:
   ...``. Use `assigned`, `mentioned` and the other listed values. (0.3.11)
2. An `openclaw`, `openprx` or `custom` `command` with whitespace that names no file and cannot be
   split unambiguously (`args` also set, quotes or backslashes, or a first word that is no program):
   ``agent `X`: openclaw.command `...` contains whitespace and is not an executable, and cannot be
   split unambiguously (<reason>); ... command = "openclaw" and args = ["message", "send"]``. Write
   `command` and `args` separately. The plain one-string form is still accepted. (0.3.13, 0.3.34)
3. An enabled tunnel with `require_inbound_sig = true` and no `hmac_secret`:
   `tunnel.require_inbound_sig = true requires a non-empty tunnel.hmac_secret: ...`. (0.3.16)
4. A blank `security.webhook_secrets` entry while `allow_unsigned = false`:
   `security.webhook_secrets[N] is blank; ...`. (0.3.18, 0.3.43)
5. A blank `tunnel.hmac_secret` while the tunnel is enabled: `tunnel.hmac_secret is blank; ...`.
   (0.3.18, 0.3.43)
6. An example placeholder secret (`your-secret-here`, `shared-hmac-secret`,
   `replace_with_webhook_secret`, the placeholders of the current example, or any value containing
   `replace_with`, `replace-with`, `your_`, `your-`, `changeme` or `example`, ignoring case and
   surrounding whitespace), in a secret that is in use (`webhook_secrets` unless `allow_unsigned`,
   `hmac_secret` while the tunnel is enabled; an unused one is a warning) (0.3.42, 0.3.43): `security.webhook_secrets[N] is still the example placeholder; ...` or
   `tunnel.hmac_secret is still an example placeholder; ...`. (0.3.20)
7. `allow_unsigned = true` with a `listen` address other than loopback, as the old example had:
   `security.allow_unsigned = true with server.listen = "0.0.0.0:9090", which is not a loopback
   address: ...`. Verify signatures, listen on loopback, or set
   `security.allow_unsigned_non_loopback = true`. (0.3.20)
8. An enabled tunnel with a `ws://` URL to a host other than loopback:
   `tunnel.url = "ws://..." uses unencrypted ws:// to a host other than loopback, ...`. Use
   `wss://` or set `tunnel.allow_insecure_transport = true`. (0.3.33)
9. `SYLVODE_WEBHOOK_SAFE_MODE` and `OPENPR_WEBHOOK_SAFE_MODE` both set and disagreeing:
   `... disagree; set only SYLVODE_WEBHOOK_SAFE_MODE`. (0.3.9)
10. `RUST_LOG` directives for `openpr_webhook` and `sylvode_webhook` on the same target with
    different levels, on stderr: `error: RUST_LOG directive ... and the legacy directive ...
    disagree; ...`. (0.3.9)
11. A config path starting with `-`, including exactly `-h`, `--help`, `-V` or `--version`: taken
    as an option (on stderr: ``error: unknown option `-x`; ...``). Pass it as `./-x`. (0.3.9,
    0.3.31)

### Behaviour changes

- The tunnel connects; it never could before (0.3.32). An enabled tunnel starts running tasks.
- The one-string `command = "openclaw message send"` runs, with a deprecation warning; with 0.3.3
  every dispatch to it failed to spawn (0.3.34).
- `{message}` in `custom` `args` is substituted (0.3.12); default templates are rendered (0.3.10).
- Payload text that would start an argument with `-` stops a `custom` agent (0.3.12, 0.3.27) and a
  `cli` run (0.3.26).
- Webhook-triggered `cli` runs share the `cli_max_concurrency` limit with the tunnel (0.3.22).
- Signed deliveries are refused when replayed (`409`) or stale (`401`, `max_delivery_age_secs`,
  default 300) (0.3.23); signature headers that disagree are refused (0.3.9).
- The `/webhook` answer's `result` no longer contains command output or remote error bodies
  (0.3.29).
- A worker task's free-form `payload` no longer chooses the agent type, nor the callback's issue
  of a `work_item` task (0.3.24); a `comment` task (a comment mentioning the bot) still takes its
  issue from `payload.issue_id` (0.3.36). `trigger_kinds` ignores `task_type` and payload fields
  (0.3.25).
- Timeouts and shutdown stop the executor's whole process group; `SIGTERM` sends each running
  executor's terminal callback (`cancelled`) and waits up to 40 seconds by default (0.3.9, 0.3.21).
- Tunnel envelopes for another `agent_id`, and signed envelopes that are stale or repeated, are
  refused (0.3.33).
- The log target is `sylvode_webhook` (0.3.9); untrusted values are logged as escaped fields
  (0.3.28); the listening line names the bound address (0.3.19).

## 0.3.43

### Fixed

- **Blank and placeholder secrets refuse start-up only when they are in use.** The examples up to
  0.3.17 shipped an uncommented `hmac_secret = "shared-hmac-secret"` beside `[tunnel] enabled =
  false`, and since 0.3.20 such a configuration refused to start (in a container with
  `restart: unless-stopped`, a restart loop) although the tunnel, and so the secret, was not used.
  `tunnel.hmac_secret` is now checked only while the tunnel is enabled (`[features].tunnel_enabled`
  and `[tunnel].enabled`), and `security.webhook_secrets` only while signatures are verified
  (`allow_unsigned = false`). A blank or placeholder secret that is not in use is logged as a
  warning at start-up.

## 0.3.42

### Security

- **More placeholder secrets refuse start-up.** The compose example in the Sylvode repository
  (`config/sylvode-webhook.example.toml`) ships `webhook_secrets = ["replace_with_webhook_secret"]`,
  which was accepted, so a deployment started from it verified signatures with a publicly known
  secret. Besides the listed values (now including `replace_with_webhook_secret`), any secret that
  contains `replace_with`, `replace-with`, `your_`, `your-`, `changeme` or `example` is refused,
  ignoring case and surrounding whitespace.

## 0.3.41

### Fixed

- **Loopback detection parses the address and compares the host exactly.** `server.listen` (for
  `allow_unsigned`) and `tunnel.url` (for `ws://`) are parsed as URLs and count as loopback only
  when the host is `localhost` (any case), an address in `127.0.0.0/8` or `::1`, without user
  information. A `ws://` URL with user information, such as `ws://user:pw@127.0.0.1/t`, is now
  refused like a remote one (before, the user information was stripped and the URL accepted), and
  a `ws://` URL that does not parse is refused. Names that only look like loopback
  (`localhost.evil`, `localhost:x@evil.com`, `127.0.0.1.evil.com`), IPv4-mapped IPv6 addresses
  and other bracketed hosts were already refused; tests now prove it. `url` is a direct
  dependency now (reqwest already used it).

## 0.3.40

### Tests

- A webhook-triggered `cli` run still waiting for its `cli_max_concurrency` slot when `SIGTERM`
  arrives is proven to send its terminal `cancelled` callback (with `update_state_on_fail`), like
  the running one, without a start callback and without starting the executor. The test fails
  when a queued run is dropped without its callback.

## 0.3.39

### Fixed

- **The `/webhook` answer for a `cli` agent no longer waits for the start callback.** The start
  callback was sent before answering, and with a slow or unreachable callback target the answer
  took up to `http_timeout_secs` (15 seconds by default). Sylvode's worker times out after 10
  seconds, records the dispatch as failed and sends the task again with `attempts + 1` (a new
  replay key), so the task ran several times. The start callback is now sent in the background,
  after the run has its `cli_max_concurrency` slot and before the executor starts, so it no
  longer reports `in_progress` for a run that is still queued. Shutdown abandons a start callback
  still in flight, does not start the executor, and sends the terminal `cancelled` callback.

### Tests

- With a callback target that accepts connections and never answers, `/webhook` answers in under
  2 seconds (15 seconds before).

## 0.3.38

### Tests

- Signature verification is proven to come before the replay memory on both paths: a `/webhook`
  request without a signature or with a wrong one, and a tunnel envelope with a wrong `sig`, are
  refused without being remembered, so the same delivery id or envelope id, correctly signed, is
  still accepted once. Both tests fail when the replay check is moved before verification.

## 0.3.37

### Fixed

- **`scripts/install.sh` renders correct values under the bash 3.2 that macOS ships.** Its
  template substitution quoted the replacement (`${content//"{{KEY}}"/"$val"}`), and bash before
  4.3 keeps those quotes, so every value in the plist (and the unit) was wrapped in literal `"`:
  `<string>"/Users/me/.local/bin/sylvode-webhook"</string>`, a launchd agent that never starts.
  The replacement is now unquoted, which inserts the value byte for byte in bash 3.2 and 5.

### Tests

- The renderer, cut out of `install.sh`, renders both templates with a value containing a space,
  `&`, `%`, `$`, `\`, `"` and `<`, and the output is compared byte for byte with an independent
  rendering, under the system bash, under it with `BASH_COMPAT=3.2` and `4.2`, and under a real
  bash 3.2 (`docker.io/library/bash:3.2` through podman) when that image is present.

## 0.3.36

### Fixed

- **A task queued by a comment that mentions a bot reports on the commented issue again.** Since
  0.3.24 the issue of a worker AI task was read only from `reference_id` with
  `reference_type = "work_item"`, but Sylvode queues a mention as `reference_type = "comment"`,
  `reference_id` = the comment, with the issue in `payload.issue_id`. Such tasks lost their issue:
  the prompt said `Fix issue unknown`, and the start and final callbacks (with MCP,
  `work_items.update`) targeted the issue `unknown`. The issue is now chosen per reference type,
  from a table with one row per shape Sylvode sends: `work_item` takes `reference_id` (a
  `payload.issue_id` is still ignored), `comment` takes `payload.issue_id`, anything else has no
  issue. The README's [Agent selection](README.md#agent-selection) lists the shapes and the
  remaining trust in a task created by hand by a project admin.

## 0.3.35

### Fixed

- `scripts/install.sh` no longer uses associative arrays (`declare -A`), which made it fail on
  the bash 3.2 that macOS ships. (It still rendered wrong values there; fixed in 0.3.37.)
- The installer inserts paths literally. Template values were substituted with `sed`, so a `&`
  or `\` in the binary or config path corrupted the unit or plist. Values are now escaped for
  their format: the systemd unit quotes `ExecStart=` and `Environment=` values (escaping `\`, `"`,
  `%` and, in arguments, `$`), and the plist escapes XML characters. A path containing a space
  now works in `ExecStart=`.
- The installer no longer adds a fixed `~/.nvm/versions/node/v22.22.1/bin` (a developer machine's
  node version) to the service `PATH`; it adds the active nvm node (`$NVM_BIN`) when there is one.
- The generated systemd unit sets `TimeoutStopSec=90`, enough for running executors to stop and
  report on shutdown.

### Changed

- The container image's base images are pinned by digest.

### Tests

- The Linux installer and uninstaller are tested against a temporary `HOME` with a fake
  `systemctl`: a fresh install with a path containing a space, `&`, `%` and `$`, the in-place
  update of a legacy `openpr-webhook.service`, the refusal when both units exist, and the removal
  of both by the uninstaller.

## 0.3.34

### Changed

- **The one-string command form earlier documentation recommended works again, with a
  deprecation warning.** `command = "openclaw message send"` (and the same form for `openprx` and
  `custom`) refused start-up since 0.3.13. When it is unambiguous it is now split on whitespace
  once at start-up and runs as `command = "openclaw"` with `args = ["message", "send"]`, and
  start-up logs a warning that shows that replacement. Unambiguous means: the whole string names no
  file or program, `args` is empty, the string contains no quote or backslash, and its first word
  is a program on `PATH` or a file. Any other multi-word command is still refused, and the error
  now says which condition failed. No shell is involved and substituted text is never split.
  Note that in 0.3.3 and earlier such a command was looked up as one program name and every
  dispatch failed to spawn, so an agent configured that way starts sending messages after this
  upgrade.

## 0.3.33

### Security

- **`ws://` to a host other than loopback refuses start-up** for an enabled tunnel, unless
  `tunnel.allow_insecure_transport = true` acknowledges that the bearer token and every task
  travel unencrypted. `wss://` and `ws://` to `127.0.0.0/8`, `::1` or `localhost` are unchanged.
- **Signed tunnel envelopes are checked for freshness and replay.** With `hmac_secret` set, an
  envelope whose `ts` is more than 300 seconds from now is refused as `stale_envelope`, and one
  whose `id` repeats an envelope accepted on the same connection as `duplicate_envelope`. Before,
  a captured signed `task.dispatch` could be replayed to run the task again.
- An envelope addressed to another `agent_id` is refused as `wrong_agent`.

### Documentation

- `require_inbound_sig` is documented as what it is: with `hmac_secret` every envelope must carry
  a valid `sig` anyway, so the setting only changes the reported reason for a missing one.

## 0.3.32

### Fixed

- **The tunnel connects.** The WebSocket handshake request was built by hand without the
  handshake headers, so tungstenite refused it before sending anything (`tunnel connect failed:
  WebSocket protocol error: Missing, duplicated or incorrect header sec-websocket-key`) and the
  tunnel retried forever without ever connecting, in every version that had it. The request is now
  built from the URL by tungstenite, with the `Authorization: Bearer` header added.

### Tests

- The tunnel is tested end to end against an in-process WebSocket control plane: the bearer token,
  a wrongly signed envelope (`bad_signature`, nothing runs), a missing signature
  (`missing_signature` with `require_inbound_sig`, `bad_signature` without), `cli_enabled`, agent
  selection by id restricted to `cli` agents, `cli_max_concurrency`, the callback gate, signed
  outbound envelopes, `invalid_json`, and `SIGTERM` during a task (`cancelled` result and callback).

## 0.3.31

### Changed

- **An unknown option is a usage error.** An argument that starts with `-` and is not `-h`,
  `--help`, `-V` or `--version` now exits 1 with `error: unknown option ...` on stderr. Before, it
  was taken as the config path (`--bogus-flag` gave `failed to read config --bogus-flag`). A config
  file whose name starts with `-` is given as `./-name`.

## 0.3.30

### Fixed

- The deprecation notice for the legacy `X-OpenPR-Signature` header is written only for a request
  whose signature verified. Before, any unauthenticated request carrying the header used up the
  once-per-process notice, so the operator never saw it for the real sender.

## 0.3.29

### Security

- **The `/webhook` answer no longer echoes command output or remote error bodies.** The `result`
  field carried the stdout of a `custom` command, the stdout or stderr of an `openclaw` or
  `openprx` command and the body of a Signal API error, and the sender stores the answer in its
  delivery log, where it was visible to anyone who can read webhook deliveries. `result` is now a
  short status written by this service: `ok`, `error: the command exited with status N`,
  `error: the Signal API answered 500 ...`, `webhook: 200 OK`, the reason a dispatch was refused,
  or the `cli` run line. The full result is logged as `Dispatch result detail=...`. A `custom`
  agent whose consumer read the command's stdout from the answer must read it from the log
  instead.

## 0.3.28

### Security

- **Text from a delivery, a command or a remote server can no longer forge log lines.** The
  event name, bot key, command output, callback and Signal API errors and tunnel message ids were
  written into log lines verbatim, so a newline in them started a line of its own, for example a
  fake `ERROR` line. They are now logged as structured fields with control characters escaped.
  Log messages that contained these values read slightly differently, for example
  `Received webhook event event="issue.assigned"` instead of `Received webhook event: issue.assigned`.

## 0.3.27

### Fixed

- A `custom` agent whose `message_template` itself starts with `-` (for example `- {title}`) runs
  again. The leading-`-` refusal of 0.3.12 could not tell where the dash came from and refused it
  like payload text. Only a leading `-` that comes from the payload is refused now, also when it
  follows placeholders that rendered empty.

## 0.3.26

### Security

- **A `cli` prompt that starts with `-` is not run.** The prompt is passed as a positional
  argument and contains payload text, so with `prompt_template = "{title}"` an issue titled
  `--config=model_provider=evil --oss` became options of `codex exec`. Such a run now reports
  `failed` with the reason instead of starting the executor, for every executor. The default
  template begins with fixed text and is not affected.

## 0.3.25

### Fixed

- **`route.trigger_kinds` is matched only against the fields its allowed values describe**:
  `bot_context.trigger_reason` (or `bot_context.trigger_kind`) of a webhook delivery and the
  top-level `trigger_kind` of a worker AI task. The matcher also fell back to `task_type`,
  `payload.trigger` and `payload.trigger_kind`, whose values (`issue_assigned`, `issue.create`, or
  anything the task's creator wrote) either can never pass start-up validation, or let the
  free-form task payload satisfy a route. A task without those fields now fails a non-empty
  `trigger_kinds` instead. Every Sylvode version that has route matching sends `trigger_kind` on
  worker tasks.

## 0.3.24

### Security

- **A worker AI task's free-form `payload` can no longer redirect the callback or choose the agent
  type.** The callback of a worker task, sent with the bot's `callback_token`, now always targets
  the task's `reference_id` (with `reference_type = "work_item"`). Before, a `payload.issue_id`
  took precedence, so whoever could create an AI task with an arbitrary `payload` (a project admin)
  could make the callback set the state of, and comment on, any other issue the bot's token can
  write to. Likewise the agent type of a worker task is now only `ai_participant_agent_type`;
  `payload.agent_type` and `payload.bot_agent_type` no longer select, for example, a `cli` agent
  for a bot that has no agent type.

## 0.3.23

### Security

- **Replayed signed deliveries are refused.** A signed `/webhook` request whose body `timestamp`
  is more than `security.max_delivery_age_secs` (new, default 300; `0` turns it off) away from now
  is answered `401`, and one whose body `id` was already received (for a worker AI task: the same
  `task_id` and `attempts`) is answered `409`. Before, a captured signed delivery could be sent
  again any number of times and dispatched the agent each time. Bodies without these fields are not
  checked, and nothing is checked with `allow_unsigned = true`.

## 0.3.22

### Changed

- **`[runtime].cli_max_concurrency` limits webhook-triggered `cli` runs too.** It used to limit
  only tunnel tasks; any number of webhook deliveries each started an executor at once. Now at
  most `cli_max_concurrency` executors (default 1) run at a time, webhook and tunnel together; a
  delivery that finds no free slot is still answered at once and its run waits. The start
  callback is still sent before the answer. A run cancelled by shutdown while it waits sends its
  terminal callback (`cancelled`) without starting.

## 0.3.21

### Security

- **A timeout stops the executor's whole process tree, not only the executor.** On Linux and
  macOS every command (`cli` executors and the commands of `openclaw`, `openprx` and `custom`
  agents) now runs as the leader of its own process group. On timeout the group is sent `SIGTERM`
  and, if anything is still alive 5 seconds later, `SIGKILL`. Before, only the direct child was
  killed, so the processes an executor had started (the native binary behind an npm `codex`
  launcher, an agent's tool calls) kept running with bypassed permissions after the run had been
  reported as `timeout` and the issue moved back to `update_state_on_fail`. The timeout callback
  is now sent after the group is gone. Processes left behind by a command that exits by itself
  are stopped the same way. On other platforms only the direct child is killed, as before.
- Commands of `openclaw`, `openprx` and `custom` agents get `/dev/null` as stdin instead of the
  service's stdin (`cli` executors already did).

### Changed

- **Graceful shutdown stops running executors and reports them.** On `SIGTERM` or `SIGINT`
  every running `cli` executor, webhook-triggered or tunnel task, is stopped as on a timeout and
  sends its terminal callback with `status = "cancelled"`, which sets `update_state_on_fail`.
  The service waits for these callbacks up to `5 + 2 × http_timeout_secs + 5` seconds (40 by
  default) and then exits 0. Before (since 0.3.9, which added graceful shutdown), shutdown killed
  only the executor's direct child, sent no terminal callback, and left the issue at
  `update_state_on_start` (for example `in_progress`). The tunnel stops reading new tasks as soon
  as shutdown begins.
- A timed-out or cancelled run reports the output it wrote before it was stopped in
  `stdout_tail` and `stderr_tail`, instead of nothing.

## 0.3.20

### Security

- **`config.example.toml` is secure by default.** It listens on `127.0.0.1:9090` instead of
  `0.0.0.0:9090`, sets `allow_unsigned = false` instead of `true`, and its `openclaw` agent, which
  runs a command for every matching bot task without any feature gate, is commented out. Copying
  it unchanged no longer exposes an unauthenticated dispatcher on every interface.
- **An example placeholder secret refuses start-up.** The example ships with
  `webhook_secrets = ["replace-with-the-secret-of-your-sylvode-webhook"]`; that value, and the
  values earlier examples showed (`your-secret-here`, `shared-hmac-secret`), are refused in
  `security.webhook_secrets` and `tunnel.hmac_secret`.
- **`allow_unsigned = true` on an address other than loopback refuses start-up** unless
  `security.allow_unsigned_non_loopback = true` acknowledges it. On loopback it still starts.
- Start-up logs a warning when `allow_unsigned = true`, and when `allow_unsigned = false` with no
  `webhook_secrets`, in which case every `/webhook` request is answered `401`.

## 0.3.19

### Fixed

- The start-up line `sylvode-webhook listening on <address>` is written after the listener is
  bound and names the address actually bound. Before, it was written before binding, so it
  appeared even when binding then failed, and with `listen = "127.0.0.1:0"` it showed port `0`
  instead of the port the system chose.

### Tests

- A new end-to-end suite (`tests/e2e`) runs the real executable against generated configurations.
  It finds the server's port from the start-up line instead of reserving a port and releasing it
  first, which could collide when tests run in parallel. It also compares the stdout of a
  successful start under both executable names, and checks that a blank secret refuses start-up.

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

### Build

- `[lints]` denies every warning (rustc and clippy) and clippy's `disallowed_types` and
  `disallowed_methods` (which `clippy.toml` points at `std::sync::Mutex` and `RwLock`), so a plain
  `cargo check` or `cargo clippy` fails on them without `-D warnings`.

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

## 0.3.10

### Fixed

- Templates are rendered in a single pass. A placeholder that appears inside a value taken from the
  payload, such as an issue title containing `{actor}`, is no longer expanded.

- **Default templates are filled in.** An `openclaw`, `openprx` or `custom` agent without a
  `message_template`, and a `cli` agent without a `prompt_template` (on the webhook path and the
  tunnel path alike), received the literal text `[TPL_PROJECT] TPL_EVENT: ...` or
  `Fix issue ISSUE_ID: TITLE ...` instead of a message. The defaults are now
  `[{project}] {event}: {key} {title}` / `{actor} | Trigger: {reason}` and
  `Fix issue {issue_id}: {title}` / `Context: {reason}` (each on two lines), rendered like a
  configured template.

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

### Security

- rustls is updated from 0.23.37 to 0.23.45 for RUSTSEC-2026-0285: the older version accepted TLS
  1.3 handshake messages across encryption level boundaries.

### Unchanged

The configuration format and default path `config.toml`, the `/webhook` and `/health` routes, the
outbound `X-Webhook-Signature`, the `openprx` agent type and the repository URL. The tunnel
`agent_id` has no default: the unused `openpr-webhook` fallback was removed, and start-up already
required the value to be configured.
