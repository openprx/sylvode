//! `ADR-0007`: the identity a consumed one-time WebSocket ticket binds a collaboration session to.
//!
//! Tickets are issued and consumed by `flow::scope_collab`; raw ticket bytes are never stored,
//! logged, or echoed back after issuance — only their SHA-256 hex digest lives in `collab_tickets`.

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

pub(super) async fn workspace_member_role<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<Option<String>, sea_orm::DbErr> {
    #[derive(FromQueryResult)]
    struct RoleRow {
        role: String,
    }
    Ok(RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace_id.into(), user_id.into()],
    ))
    .one(conn)
    .await?
    .map(|row| row.role))
}

pub struct ConsumedTicket {
    pub user_id: Uuid,
    pub workspace_id: Uuid,
    pub document_id: Uuid,
    /// The `client_id` this ticket was bound to at issuance (`ADR-0007`) and just matched
    /// exactly by the `UPDATE ... WHERE client_id = $2` above — carried through so the session
    /// loop can stamp it onto every `collab_updates.origin_client_id` this connection writes,
    /// instead of that column silently staying `NULL` for every direct WebSocket write.
    pub client_id: String,
}
