-- Token bots are bot users.
--
-- `POST /api/v1/workspaces/{id}/bots` used to create the user behind an `opr_` token with
-- `entity_type = 'bot_mcp'`, while every place that treats a user as an automation identity
-- (assignment to a bot, `webhooks.bot_user_id`, AI participants, bot mentions, bot context on
-- webhook deliveries, AI authorship of proposals, votes, vetoes and appeals) matches
-- `entity_type = 'bot'` only. A token user could therefore not be assigned work or act as the AI
-- participant of a task without an administrator flipping the column by hand, and its proposal
-- activity was recorded as human.
--
-- There is one automation identity type, `bot`. Whether a bot authenticates with a token is
-- recorded by its `workspace_bots` row (and `agent_type = 'mcp'`), not by a second entity type.
-- Existing token users are moved to `bot`; their `agent_type`, workspace membership, token rows
-- and audit history are untouched, so every issued token keeps working.
-- Re-running this file is a no-op: nothing matches once the old value is gone.

UPDATE users
SET entity_type = 'bot',
    updated_at = NOW()
WHERE entity_type = 'bot_mcp';
