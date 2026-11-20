//! Revoking grants.
//!
//! Every path that ends a connection goes through [`revoke_grant`]: it marks the grant revoked,
//! revokes every token of it, writes the `grant_revoked` audit row and queues the
//! `sv_oauth_grant_changed` notification. All of it happens on the caller's transaction, so a
//! hook that revokes as part of its own write commits or rolls back together with it. `PostgreSQL`
//! delivers `pg_notify` when the transaction commits, which is what "after commit" means here: a
//! rolled-back revocation notifies nobody.
//!
//! Revocation is idempotent. The grant row is updated only while it is `active`, so a second call
//! changes nothing, writes no second audit row and sends no second notification.

use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, DbErr, FromQueryResult, Statement};
use uuid::Uuid;

use super::audit::{self, AuditEvent, AuditRecord, DetailValue};

/// The `LISTEN` channel the streams of every API process wake on.
pub const GRANT_CHANGED_CHANNEL: &str = "sv_oauth_grant_changed";

/// Why a grant ended. The names are the values `oauth_grants_reason_check` accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevokeReason {
    UserRevoked,
    ClientRevoked,
    WorkspaceAdminRevoked,
    WorkspaceDisabled,
    PasswordChanged,
    AccountDisabled,
    MembershipRemoved,
    RefreshReuse,
    CodeReplay,
    ClientDisabled,
    Expired,
}

impl RevokeReason {
    pub const ALL: [Self; 11] = [
        Self::UserRevoked,
        Self::ClientRevoked,
        Self::WorkspaceAdminRevoked,
        Self::WorkspaceDisabled,
        Self::PasswordChanged,
        Self::AccountDisabled,
        Self::MembershipRemoved,
        Self::RefreshReuse,
        Self::CodeReplay,
        Self::ClientDisabled,
        Self::Expired,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserRevoked => "user_revoked",
            Self::ClientRevoked => "client_revoked",
            Self::WorkspaceAdminRevoked => "workspace_admin_revoked",
            Self::WorkspaceDisabled => "workspace_disabled",
            Self::PasswordChanged => "password_changed",
            Self::AccountDisabled => "account_disabled",
            Self::MembershipRemoved => "membership_removed",
            Self::RefreshReuse => "refresh_reuse",
            Self::CodeReplay => "code_replay",
            Self::ClientDisabled => "client_disabled",
            Self::Expired => "expired",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|reason| reason.as_str() == name)
    }
}

/// Who caused the revocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Actor {
    /// The server itself: a failed live check, an expiry, a detected replay.
    System,
    User(Uuid),
}

impl Actor {
    const fn user_id(self) -> Option<Uuid> {
        match self {
            Self::System => None,
            Self::User(id) => Some(id),
        }
    }
}

#[derive(Debug, FromQueryResult)]
#[allow(clippy::struct_field_names, reason = "the fields are the columns of the row")]
struct RevokedGrant {
    client_id: String,
    user_id: Uuid,
    workspace_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct GrantId {
    id: Uuid,
}

/// Revokes one grant and everything issued under it. Returns whether this call ended it: `false`
/// means it was already revoked (or does not exist) and nothing was written.
pub async fn revoke_grant(
    tx: &DatabaseTransaction,
    grant_id: Uuid,
    reason: RevokeReason,
    actor: Actor,
) -> Result<bool, DbErr> {
    let updated = RevokedGrant::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE oauth_grants \
         SET status = 'revoked', revoked_at = now(), revoked_by = $2, revoke_reason = $3 \
         WHERE id = $1 AND status = 'active' \
         RETURNING client_id, user_id, workspace_id",
        vec![grant_id.into(), actor.user_id().into(), reason.as_str().into()],
    ))
    .one(tx)
    .await?;

    // Tokens are revoked even when the grant was already revoked, so a token that somehow
    // survived (a manual edit, an older build) is closed by the next revocation attempt.
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE oauth_tokens SET revoked_at = now() WHERE grant_id = $1 AND revoked_at IS NULL",
        vec![grant_id.into()],
    ))
    .await?;

    let Some(grant) = updated else {
        return Ok(false);
    };

    let mut record = AuditRecord::new(AuditEvent::GrantRevoked)
        .with_client(&grant.client_id)
        .with_grant(grant_id)
        .with_user(grant.user_id)
        .with_workspace(grant.workspace_id)
        .with_detail("reason", DetailValue::Code(reason.as_str()));
    if let Some(actor_id) = actor.user_id() {
        record = record.with_actor(actor_id);
    }
    audit::write(tx, &record).await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_notify($1, $2)",
        vec![GRANT_CHANGED_CHANNEL.into(), grant_id.to_string().into()],
    ))
    .await?;
    Ok(true)
}

/// Locks and revokes every active grant selected by `select_sql`, which takes one bind value and
/// returns grant ids. The statement text is chosen by the callers below, never built from input.
async fn revoke_selected(
    tx: &DatabaseTransaction,
    select_sql: &'static str,
    values: Vec<sea_orm::Value>,
    reason: RevokeReason,
    actor: Actor,
) -> Result<Vec<Uuid>, DbErr> {
    let ids = GrantId::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, select_sql, values))
        .all(tx)
        .await?;
    let mut revoked = Vec::with_capacity(ids.len());
    for GrantId { id } in ids {
        if revoke_grant(tx, id, reason, actor).await? {
            revoked.push(id);
        }
    }
    Ok(revoked)
}

/// Every active grant of a user, in every workspace (password change, account disabled).
pub async fn revoke_all_user_grants(
    tx: &DatabaseTransaction,
    user_id: Uuid,
    reason: RevokeReason,
    actor: Actor,
) -> Result<Vec<Uuid>, DbErr> {
    revoke_selected(
        tx,
        "SELECT id FROM oauth_grants WHERE user_id = $1 AND status = 'active' ORDER BY id FOR UPDATE",
        vec![user_id.into()],
        reason,
        actor,
    )
    .await
}

/// Every active grant of one user in one workspace (membership removed).
pub async fn revoke_user_workspace_grants(
    tx: &DatabaseTransaction,
    user_id: Uuid,
    workspace_id: Uuid,
    reason: RevokeReason,
    actor: Actor,
) -> Result<Vec<Uuid>, DbErr> {
    revoke_selected(
        tx,
        "SELECT id FROM oauth_grants WHERE user_id = $1 AND workspace_id = $2 AND status = 'active' \
         ORDER BY id FOR UPDATE",
        vec![user_id.into(), workspace_id.into()],
        reason,
        actor,
    )
    .await
}

/// Every active grant in a workspace (connected applications switched off).
pub async fn revoke_workspace_grants(
    tx: &DatabaseTransaction,
    workspace_id: Uuid,
    reason: RevokeReason,
    actor: Actor,
) -> Result<Vec<Uuid>, DbErr> {
    revoke_selected(
        tx,
        "SELECT id FROM oauth_grants WHERE workspace_id = $1 AND status = 'active' ORDER BY id FOR UPDATE",
        vec![workspace_id.into()],
        reason,
        actor,
    )
    .await
}

/// Every active grant of a client (client disabled).
pub async fn revoke_client_grants(
    tx: &DatabaseTransaction,
    client_id: &str,
    reason: RevokeReason,
    actor: Actor,
) -> Result<Vec<Uuid>, DbErr> {
    revoke_selected(
        tx,
        "SELECT id FROM oauth_grants WHERE client_id = $1 AND status = 'active' ORDER BY id FOR UPDATE",
        vec![client_id.into()],
        reason,
        actor,
    )
    .await
}

/// Stamps `last_used_at` on a token and its grant. Callers run it best effort and log a failure.
pub async fn record_use<C: ConnectionTrait>(conn: &C, token_id: Uuid, grant_id: Uuid) -> Result<(), DbErr> {
    conn.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "WITH t AS (UPDATE oauth_tokens SET last_used_at = now() WHERE id = $1) \
         UPDATE oauth_grants SET last_used_at = now() WHERE id = $2",
        vec![token_id.into(), grant_id.into()],
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn reason_names_round_trip_and_match_the_constraint() {
        let allowed = [
            "user_revoked",
            "client_revoked",
            "workspace_admin_revoked",
            "workspace_disabled",
            "password_changed",
            "account_disabled",
            "membership_removed",
            "refresh_reuse",
            "code_replay",
            "client_disabled",
            "expired",
        ];
        assert_eq!(RevokeReason::ALL.len(), allowed.len());
        for reason in RevokeReason::ALL {
            assert!(allowed.contains(&reason.as_str()));
            assert_eq!(RevokeReason::from_name(reason.as_str()), Some(reason));
        }
        assert_eq!(RevokeReason::from_name("nope"), None);
    }

    #[test]
    fn actor_maps_to_the_revoking_user() {
        let id = Uuid::new_v4();
        assert_eq!(Actor::System.user_id(), None);
        assert_eq!(Actor::User(id).user_id(), Some(id));
    }
}
