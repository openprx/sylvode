# Security Policy

## Supported versions

Security fixes are made on the latest Sylvode release, which carries the runner at the same
version; see the repository's [security policy](../../SECURITY.md).

## Reporting a vulnerability

Please do not open a public issue for a vulnerability. Report it privately through
[GitHub security advisories](https://github.com/openprx/sylvode/security/advisories/new), as the
repository's [security policy](../../SECURITY.md) describes.

Include the affected version, a description of the problem, steps to reproduce and the impact you
expect. We will acknowledge the report, keep you informed while we investigate, and credit you in
the advisory unless you prefer otherwise.

## Scope notes

- Inbound webhooks are authenticated by an HMAC-SHA256 signature in `X-Sylvode-Signature`,
  `X-Webhook-Signature` or the legacy `X-OpenPR-Signature`. Requests whose signature headers
  disagree are refused. `allow_unsigned = true` disables verification and is meant for
  development only.
- `allow_unsigned = true` is refused on an address other than loopback unless
  `allow_unsigned_non_loopback = true` acknowledges it. Blank secrets and the example placeholders
  refuse start-up. Signed requests are also checked for replay (`id`) and freshness (`timestamp`).
- Several agent types run local commands, and they are gated differently:
  - `cli` runs `codex`, `claude` or `opencode` with permission checks bypassed. It runs only with
    `features.cli_enabled = true`, never in safe mode (`SYLVODE_RUNNER_SAFE_MODE=1`), at most
    `runtime.cli_max_concurrency` at a time, and its whole process group is stopped on timeout or
    shutdown.
  - `openclaw`, `openprx` (with `command`) and `custom` run their configured command for every
    matching bot task. Neither `cli_enabled` nor safe mode stops them; the program and its fixed
    arguments come only from the configuration, and payload text is always passed as a single
    argument (for `custom`, it may not start an option). Configure only the agents you need.
- Commands run without a shell, with stdin closed, and inherit the service's environment; do not
  give the service environment variables you would not give the agents.
- Run the service as a dedicated unprivileged user (see the systemd example in the README).
- Never commit a `config.toml` containing secrets; `.gitignore` excludes it.

## The `runner` agent type

- One runner OS user is one trust domain: file modes do not protect one bot's token from a CLI
  running as the same user. Separate trust domains need separate OS users or hosts. The CLI's
  permission posture (allow list, deny rules for the state directory and the configuration file,
  `dontAsk`) is the only barrier inside one user, and `allowed_roots` is not a sandbox.
- The tmux channel is off by default (`[runner.tmux] enabled = false`). Enable it only after the
  hand checks in the README have been done on the pinned CLI version.
- Every child process the runner starts gets an allow-listed environment, never the service's.
- `GET /health` is unauthenticated and returns only the overall status and the protocol state.
- A hook message is admitted only with the launch's nonce, from the runner's own user, and from
  below the session's pane process where the platform can name the sender (Linux: `/proc`; macOS:
  one `ps -A` snapshot per admitted message). The sender's identity is best-effort on macOS.
