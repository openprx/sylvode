-- A bot may hold a tmux connection in several projects, and a project may hold tmux connections
-- of several bots for the same CLI. The unique index of 0112/0114 allowed one per (project, cli).
-- Idempotent.
DROP INDEX IF EXISTS pbc_one_tmux_per_cli;
