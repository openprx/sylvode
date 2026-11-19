//! Automation identities.
//!
//! A user is either a person (`entity_type = 'human'`) or a bot (`entity_type = 'bot'`). Every bot
//! is the same kind of identity whichever way it was created: by an administrator
//! (`POST /api/v1/admin/users`, no credential of its own) or through the workspace bots API
//! (`POST /api/v1/workspaces/{id}/bots`, backed by an `opr_` token and `agent_type = 'mcp'`).
//! Only a bot may be the assignee that receives bot tasks, the `bot_user_id` of a webhook, or the
//! AI participant of a project; whether it holds a token is recorded by `workspace_bots`, not by
//! its entity type. Being a bot grants nothing by itself: what a bot may do through its token is
//! decided by the token's permissions and workspace membership, and an AI task can only be
//! completed or failed by its own participant (or a system administrator).
//!
//! SQL that joins on users spells the same predicate as `u.entity_type = 'bot'`.

use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement};
use uuid::Uuid;

/// `users.entity_type` of every automation identity.
pub const BOT_ENTITY_TYPE: &str = "bot";

/// Whether `entity_type` names an automation identity.
pub fn is_bot_entity_type(entity_type: &str) -> bool {
    entity_type == BOT_ENTITY_TYPE
}

/// Whether `user_id` is an automation identity: one that may be assigned bot tasks, named as a
/// webhook's `bot_user_id` and registered as an AI participant.
pub async fn is_bot_user(db: &impl ConnectionTrait, user_id: Uuid) -> Result<bool, DbErr> {
    Ok(db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM users WHERE id = $1 AND entity_type = $2",
            vec![user_id.into(), BOT_ENTITY_TYPE.into()],
        ))
        .await?
        .is_some())
}

#[cfg(test)]
mod tests {
    use super::{BOT_ENTITY_TYPE, is_bot_entity_type};

    #[test]
    fn only_the_bot_entity_type_is_an_automation_identity() {
        assert!(is_bot_entity_type(BOT_ENTITY_TYPE));
        for other in ["human", "bot_mcp", "Bot", "", "ai"] {
            assert!(!is_bot_entity_type(other), "{other}");
        }
    }
}
