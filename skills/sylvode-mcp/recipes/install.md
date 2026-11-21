# Install and troubleshoot Sylvode task delivery

## Components
- `mcp-server`: the Sylvode MCP server. Needed by every agent.
- `sylvode-runner`: receives Sylvode tasks and drives the CLI. Needed for task delivery.
- `tmux` 3.2 or newer: needed for the interactive (tmux) channel.
- One CLI: `claude`, `codex` or `opencode`, logged in once interactively.

Installers never install another component for you. They print the command.

## Install everything on one machine
1. Install tmux: `brew install tmux`, `sudo apt install tmux`, `sudo dnf install tmux` or `sudo pacman -S tmux`.
2. Install and log in to your CLI.
3. Run the MCP server installer: `apps/mcp-server/scripts/install.sh`.
4. Run the runner installer: `apps/runner/scripts/install.sh`. Add `--headless-only` if you will not use tmux.
5. Set `[runner] allowed_roots` in the runner config to the directories projects may use.
6. In Sylvode, a workspace admin opens the project, Execution, and sets the working directory and CLI.
7. Check: `sylvode-runner doctor` and `mcp-server doctor`. Both must show no `fail`.

## Watch a session
`tmux -L sylvode attach -t =<PROJECTKEY>`. Detach with `Ctrl-b d`. Typing there is fine. The runner waits until you are idle before it sends the next task.

## Tool mode
The MCP server lists nine tools by default (search mode). Set `[mcp] tool_mode = "full"` or start it with `serve --tools full` to list every tool by name instead. `[mcp] allow_write_call = false` makes `sylvode_write` answer that writes are disabled.

## Common failures
| Task error contains | Fix |
|---|---|
| `path_outside_allowlist` | Add the directory's parent to `[runner] allowed_roots`, or change the project's working directory |
| `path_missing` | Create the directory on the runner machine, or fix the setting |
| `tmux_unavailable` | Install tmux 3.2+ where the service can find it, then re-run the installer |
| `cli_unavailable` | Install the CLI, or set `[runner.cli.<name>] binary` to its absolute path |
| `workspace_untrusted` | Run `sylvode-runner trust <PROJECTKEY>` and accept the CLI's trust prompt once |
| `protocol_mismatch` | Upgrade the component with the lower version named in the error |
| `did not acknowledge` / `ended its turn without completing` | Attach to the session, read what the agent did, then reassign the issue to retry |
| `waiting for a permission decision` | Attach and answer, or widen the runner's allowed tools for this CLI |
