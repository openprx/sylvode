-- The tmux channel takes claude and codex. The check of 0112/0114 allowed claude only; the unique
-- index pbc_one_tmux_per_cli (project_id, cli) WHERE channel = 'tmux' already keeps one tmux
-- connection per CLI and stays as it is. Idempotent.
ALTER TABLE project_bot_connections DROP CONSTRAINT IF EXISTS pbc_tmux_cli_check;
ALTER TABLE project_bot_connections ADD CONSTRAINT pbc_tmux_cli_check
  CHECK (channel <> 'tmux' OR cli IN ('claude', 'codex'));
