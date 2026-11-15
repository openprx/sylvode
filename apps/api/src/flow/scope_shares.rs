//! Public Page grants with canonical scope/lifecycle and current-token validation. Raw secrets
//! are returned once, never stored in command receipts, logs, event metadata or notifications.
use super::{appearance, collab::authz::PermissionLevel, public_page, scope, shares};
use crate::error::ApiError;
use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

async fn management_proof(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
) -> Result<scope::AuthorizationProof, ApiError> {
    let proof = scope::authorize(db, object, user, PermissionLevel::View).await?;
    if proof.level < PermissionLevel::FullAccess {
        return Err(ApiError::forbidden("full_access is required to manage public links"));
    }
    Ok(proof)
}
pub async fn list(db: &DatabaseConnection, object: Uuid, user: Uuid) -> Result<shares::ShareLinkList, ApiError> {
    let proof = management_proof(db, object, user).await?;
    let result = shares::list_links(db, object, proof.level).await?;
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(result)
}
pub async fn create(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    expires: Option<DateTime<Utc>>,
) -> Result<shares::CreatedShareLink, ApiError> {
    let proof = management_proof(db, object, user).await?;
    let identity = scope::identity(db, object).await?;
    if identity.object_type != "page" || !proof.resource_active {
        return Err(ApiError::policy_rejected("only an active page can be publicly shared"));
    }
    let tx = db.begin().await?;
    if identity.workspace_id.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::lock_change(&tx, &proof).await?;
    scope::authorize(&tx, object, user, PermissionLevel::FullAccess).await?;
    let result = shares::create_link_on(&tx, identity.workspace_id, identity.scope_id, object, user, expires).await?;
    scope::advance_revision(&tx, identity.scope_id).await?;
    super::scope_event::insert_for_object(
        &tx,
        &identity,
        user,
        "flow.share_link.created",
        "flow_share_link",
        result.link.id,
        &format!("share_link_create:{}", result.link.id),
        json!({"object_id":object,"share_link_id":result.link.id}),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
pub async fn revoke(
    db: &DatabaseConnection,
    object: Uuid,
    link: Uuid,
    user: Uuid,
) -> Result<shares::ShareLinkView, ApiError> {
    let proof = management_proof(db, object, user).await?;
    let identity = scope::identity(db, object).await?;
    let tx = db.begin().await?;
    scope::lock_change(&tx, &proof).await?;
    scope::authorize(&tx, object, user, PermissionLevel::FullAccess).await?;
    let was_active = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT revoked_at IS NULL AS active FROM flow_share_links WHERE id=$1 AND object_id=$2 AND scope_id=$3",
            vec![link.into(), object.into(), identity.scope_id.into()],
        ))
        .await?
        .and_then(|r| r.try_get::<bool>("", "active").ok())
        .ok_or_else(shares::missing)?;
    let result = shares::revoke_link(&tx, object, link, PermissionLevel::FullAccess).await?;
    if was_active {
        scope::advance_revision(&tx, identity.scope_id).await?;
        super::scope_event::insert_for_object(
            &tx,
            &identity,
            user,
            "flow.share_link.revoked",
            "flow_share_link",
            link,
            &format!("share_link_revoke:{link}"),
            json!({"object_id":object,"share_link_id":link}),
            json!({}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(result)
}
#[derive(FromQueryResult)]
struct PublicGrant {
    link_id: Uuid,
    object_id: Uuid,
    scope_id: Uuid,
    workspace_id: Option<Uuid>,
    appearance: Value,
    title: Option<String>,
    plain_text: Option<String>,
    document_id: Option<Uuid>,
    expires_at: Option<DateTime<Utc>>,
}
async fn grant(db: &DatabaseConnection, token: &str) -> Result<PublicGrant, ApiError> {
    if !shares::valid_token(token) {
        return Err(shares::missing());
    }
    let row=PublicGrant::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT l.id AS link_id,o.id AS object_id,o.scope_id,o.workspace_id,o.appearance,p.title,p.plain_text,d.id AS document_id,l.expires_at FROM flow_share_links l JOIN flow_objects o ON o.id=l.object_id AND o.scope_id=l.scope_id JOIN flow_scopes s ON s.id=o.scope_id LEFT JOIN flow_object_projections p ON p.object_id=o.id LEFT JOIN collab_documents d ON d.object_id=o.id WHERE l.token_hash=$1 AND l.revoked_at IS NULL AND (l.expires_at IS NULL OR l.expires_at>now()) AND o.object_type='page' AND o.lifecycle_status='active' AND s.lifecycle_status='active'",vec![shares::hash_token(token).into()])).one(db).await?.ok_or_else(shares::missing)?;
    if let Some(workspace) = row.workspace_id
        && !super::repository::fetch_flow_enabled(db, workspace).await?
    {
        return Err(shares::missing());
    }
    Ok(row)
}
pub async fn read_public(db: &DatabaseConnection, token: &str) -> Result<shares::PublicShareView, ApiError> {
    let first = grant(db, token).await?;
    let revision = scope::revision(db, first.scope_id).await?;
    let appearance = appearance::parse_stored(&first.appearance);
    let (blocks, text, title) = if let Some(document) = first.document_id {
        let engine = super::document::load_head_engine(db, document).await?;
        let semantic = engine
            .semantic_snapshot()
            .map_err(|e| super::command::map_collab_error(&e))?;
        let blocks = public_page::compose_public_document(&engine.root_state_json(), &semantic);
        let text = public_page::plain_text_of(&blocks);
        let title = engine.title().map_err(|e| super::command::map_collab_error(&e))?;
        (blocks, text, title)
    } else {
        (
            Vec::new(),
            public_page::sanitize_share_text(first.plain_text.as_deref().unwrap_or("")),
            first.title.clone().unwrap_or_default(),
        )
    };
    let current = grant(db, token).await?;
    if current.link_id != first.link_id || scope::revision(db, first.scope_id).await? != revision {
        return Err(shares::missing());
    }
    Ok(shares::PublicShareView {
        object_id: first.object_id,
        title: public_page::sanitize_share_text(&title),
        plain_text: text,
        blocks,
        appearance,
        expires_at: first.expires_at.map(|t| t.to_rfc3339()),
    })
}
pub async fn read_public_asset(
    db: &DatabaseConnection,
    token: &str,
    asset: Uuid,
) -> Result<appearance::AppearanceAssetBytes, ApiError> {
    let first = grant(db, token).await?;
    if !appearance::parse_stored(&first.appearance).references_asset(asset) {
        return Err(shares::missing());
    }
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT content_type,bytes FROM flow_appearance_assets WHERE id=$1 AND object_id=$2 AND scope_id=$3",
            vec![asset.into(), first.object_id.into(), first.scope_id.into()],
        ))
        .await?
        .ok_or_else(shares::missing)?;
    let mime = row.try_get::<String>("", "content_type")?;
    let mime = appearance::static_content_type(&mime).ok_or_else(ApiError::internal)?;
    let bytes = row.try_get::<Vec<u8>>("", "bytes")?;
    let current = grant(db, token).await?;
    if current.link_id != first.link_id || !appearance::parse_stored(&current.appearance).references_asset(asset) {
        return Err(shares::missing());
    }
    Ok(appearance::AppearanceAssetBytes {
        content_type: mime,
        bytes,
    })
}
