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

- Inbound webhooks are authenticated by an HMAC-SHA256 signature in `X-Sylvode-Signature`
  or `X-Webhook-Signature`. Requests whose signature headers disagree are refused. `allow_unsigned = true` disables verification and is meant for
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
  running as the same user. Separate trust domains need separate OS users or hosts.
- By default every task runs with all permission checks of the CLI off (`launch_command` and
  `headless_args` carry `--permission-mode bypassPermissions`), as the runner's OS user, with that
  user's environment and login. The runner adds no tool list, deny rule or settings-source limit,
  and project files (`.claude/settings.json`, `.mcp.json`, hooks, skills, `CLAUDE.md`) take effect:
  a work item that carries a prompt injection can make the CLI read or change any file that user
  can reach, run any command, read every bot token in the state directory, push code and use the
  operator's credentials. Put the flags you want in `launch_command` and `headless_args`; the
  start-up posture line, `doctor` and the connection's status in Sylvode warn while checks are off.
  `allowed_roots` is not a sandbox.
- A tmux session the operator created runs with whatever configuration it has; the runner does not
  inspect or limit it. It types only when the CLI is idle with an empty prompt and no client typed
  recently, only into the one pane that runs the CLI (checked by pane id and process id at every
  push), and never restarts, respawns or ends a session.
- The runner starts no CLI with a stripped environment: a CLI it starts, headless or in a session
  it creates, gets the runner's own environment minus `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT` and
  `TMUX`. The runner's own tmux calls get an allow-listed environment.
- `GET /health` is unauthenticated and returns only the overall status and the protocol state.
- A hook message carries no secret. It is admitted only from the runner's own user, from a pane a
  session worker recorded (the tmux server's socket path and the pane id), and from below the
  recorded pane process where the platform can name the sender (Linux: `/proc`; macOS: one `ps -A`
  snapshot per admitted message). The sender's identity is best-effort on macOS. Any process of the
  same user inside that pane can send hook messages for it.
