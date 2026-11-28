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

## Inbound requests

- Deliveries are authenticated by an HMAC-SHA256 signature, `v1=<hex>` over `<timestamp>.<body>`,
  in `X-Sylvode-Signature` or `X-Webhook-Signature`, with the timestamp in `X-Sylvode-Timestamp` or
  `X-Webhook-Timestamp`. A timestamp further than `max_delivery_age_secs` from the clock is
  refused. Requests whose signature or timestamp headers disagree are refused, even if one of
  them is valid. A delivery for a bot that has its own `webhook_secret` is verified with that
  secret only.
- `allow_unsigned = true` disables verification and is meant for local development only. It is
  refused on an address other than loopback unless `allow_unsigned_non_loopback = true`
  acknowledges it. Blank secrets and example placeholders refuse start-up.
- Signed requests are checked for replay (`id`, or `task_id` and `attempts`) and freshness
  (`timestamp`). The replay memory is held in the process and cleared on restart; worker AI tasks
  carry no timestamp. See "Replayed requests" in the [README](README.md).
- `GET /health` is unauthenticated and returns only the overall status and the protocol state.

## Agents that run commands

- `openclaw`, `openprx` (with `command`) and `custom` agents run their configured command for
  every matching bot task. Safe mode (`SYLVODE_RUNNER_SAFE_MODE=1`) does not stop them. The
  program and its fixed arguments come only from the configuration; text from Sylvode is always
  passed as a single argument, and for `custom` it may not start an option. Configure only the
  agents you need.
- `runner` agents run a CLI agent ([below](#runner-agents)). Their headless runs are limited to
  `runtime.cli_max_concurrency` at a time; safe mode makes every `runner` agent refuse its tasks.
- Every command and headless run is started as its own process group, and the whole group is
  stopped on time-out or shutdown.
- Commands run without a shell, with stdin closed, and inherit the service's environment; do not
  give the service environment variables you would not give the agents.
- Run the service as a dedicated unprivileged user (see the systemd example in the README).
- Never commit a `config.toml` containing secrets; `.gitignore` excludes it.

## `runner` agents

- One runner OS user is one trust domain: file modes do not protect one bot's token from a CLI
  running as the same user. Separate trust domains (workspaces, or bots whose assigners you do
  not trust equally) need separate OS users or hosts.
- Anyone who can assign an issue to the bot, mention it or request its vote can make the CLI act,
  and anyone who can edit an issue or comment can put text in front of it.
- By default every task runs with all permission checks of the CLI off (`launch_command` and
  `headless_args` carry `--permission-mode bypassPermissions`), as the runner's OS user, with that
  user's environment and login. The runner adds no tool list, deny rule or settings-source limit,
  and project files (`.claude/settings.json`, `.mcp.json`, hooks, skills, `CLAUDE.md`) take effect:
  a work item that carries a prompt injection can make the CLI read or change any file that user
  can reach, run any command, read every bot token in the state directory, push code and use the
  operator's credentials. Put the flags you want in `launch_command` and `headless_args` (a
  permission mode that asks, a tool list, `--strict-mcp-config`, `--setting-sources`); the
  start-up posture line, `doctor` and the connection's status in Sylvode warn while checks are off.
- codex in a tmux session starts by default with `--dangerously-bypass-approvals-and-sandbox`: no
  approval prompts and no sandbox, so every task runs with the full rights of the runner's OS user,
  as with claude above. This is the operator's responsibility: run the runner on a host that is
  sandboxed from outside (a dedicated user, container or virtual machine), or change
  `[runner.cli.codex] launch_command`; the default is needed because the approval prompts of
  codex refuse every MCP tool call while approvals are off. The start-up posture line, `doctor`
  and the connection's status in Sylvode show `bypass` for it. Headless codex runs take the
  operator's `headless_args` and are not changed.
- The runner gives a codex session it creates the Sylvode MCP server and the turn hook as `-c`
  overrides on the command line: they name the bot's MCP file (mode 0600) and the hook socket,
  never a token. The operator's `~/.codex/config.toml` is not changed, but the session's `notify`
  replaces the operator's own. The notify program receives the turn summary as its last argument,
  visible in the process table to the same user for the life of that short process; the runner
  reads only its `type` and `client` and never logs or stores the rest. codex also starts a
  second MCP server process of the same bot for subagents.
- codex reads project files (`AGENTS.md`, `.codex/`, and may import `CLAUDE.md` and `.claude`
  settings); the runner's managed `AGENTS.md` section is written into the project (no git exclude).
- The runner never answers a codex dialog (folder trust, login, hook review, command or tool
  approval) and never sends Escape or Ctrl-C to a session; the operator answers them in the
  attached session.
- A hook message is applied only to the session of its own CLI.
- `--no-daemon` is part of the codex default: a shared daemon would run work outside the pane the
  runner checks.
- opencode's permissions are not controlled by the runner; the start-up log warns when it is
  enabled.
- `allowed_roots` keep a project from naming a directory outside them, and the state directory
  and the configuration file must lie outside every root; they are not a sandbox. A directory
  replaced by a symbolic link between the check and the use, by a process of the same OS user, is
  out of scope.
- A tmux session the operator created runs with whatever configuration it has; the runner does not
  inspect or limit it. The runner types only when the CLI is idle with an empty prompt and no
  client typed recently, only into the one pane that runs the CLI (checked by pane id and process
  id at every push), and never restarts, respawns or ends a session.
- Everything on one tmux server runs as one OS user, and tmux sets `TMUX` in each pane: a CLI that
  may run shell commands can drive every other session on the same server.
- A CLI the runner starts, headless or in a session it creates, gets the runner's own environment
  minus `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT` and `TMUX`. The runner's own tmux calls get an
  allow-listed environment.
- Bot tokens are kept in files of mode 0600 in the state directory (mode 0700), are never logged,
  never part of an error or of a command line, and are never sent after a redirect. The Sylvode API
  URL must be `https://` unless its host is loopback.
- A hook message carries no secret. It is admitted only from the runner's own user, from a pane a
  session recorded (the tmux server's socket path and the pane id), and from below the recorded
  pane process where the platform can name the sender (Linux: `/proc`; macOS: one `ps -A`
  snapshot per admitted message). The sender's identity is best-effort on macOS. Any process of the
  same user inside that pane can send hook messages for it.
- The token of a bot made by a project connection acts only in the projects where that bot has an
  enabled connection, and only on the routes a runner needs; see
  [Bots and agent work](../../docs/guide/agents.md).
