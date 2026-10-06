-- Widen bot_operation_logs.surface to every surface a bot credential can carry.
-- 0051 allowed mcp_http, mcp_sse, mcp_stdio, cli and rest; 0062 added cli_tools_call to
-- workspace_bots.transport_surface but not here, so every audit row of a `tools call` made with a
-- cli_tools_call credential was rejected. The value list is EventSurface::BOT_CREDENTIAL_SURFACES
-- (apps/api/src/flow/event_origin.rs); a real-database test walks that list through this table.
-- Expand-only: the new list is a superset of the old one, so every existing row still satisfies it.
-- Idempotent: dropping and re-adding the named constraint gives the same end state on every run.

ALTER TABLE bot_operation_logs
  DROP CONSTRAINT IF EXISTS bot_operation_logs_surface_check;
ALTER TABLE bot_operation_logs
  ADD CONSTRAINT bot_operation_logs_surface_check CHECK (
    surface IN ('rest', 'mcp_http', 'mcp_sse', 'mcp_stdio', 'cli', 'cli_tools_call')
  );
