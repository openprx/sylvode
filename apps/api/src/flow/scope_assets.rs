//! Object-owned appearance/assets. Publishing an appearance reference is distinct from uploading
//! a private image; public reads may access only the currently referenced image on that object.
use super::{
    appearance::{self, AppearanceAssetBytes, AppearanceAssetCreated, PageAppearance},
    collab::authz::PermissionLevel,
    scope,
};
use crate::error::ApiError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;
fn operation(provided: Option<&str>, kind: &str) -> String {
    provided.map_or_else(|| format!("{kind}:{}", Uuid::new_v4()), str::to_string)
}
async fn editable(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
) -> Result<(scope::ObjectIdentity, scope::AuthorizationProof), ApiError> {
    let proof = scope::authorize(db, object, user, PermissionLevel::Edit).await?;
    let identity = scope::identity(db, object).await?;
    if identity.object_type != "page" || !proof.resource_active {
        return Err(ApiError::policy_rejected("only an active page can change appearance"));
    }
    Ok((identity, proof))
}
pub async fn set(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    appearance: PageAppearance,
    intent: Option<&str>,
) -> Result<PageAppearance, ApiError> {
    let (identity, proof) = editable(db, object, user).await?;
    appearance::validate_appearance(&appearance)?;
    let key = operation(intent, "appearance");
    let body = json!({"operation":"appearance","object_id":object,"appearance":appearance});
    let tx = db.begin().await?;
    if identity.workspace_id.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence(&tx, &proof).await?;
    if let Some(result) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &key, &body).await? {
        tx.commit().await?;
        return serde_json::from_value(result).map_err(|_| ApiError::internal());
    }
    for asset in appearance::referenced_assets(&appearance) {
        if tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM flow_appearance_assets WHERE id=$1 AND object_id=$2 AND scope_id=$3",
                vec![asset.into(), object.into(), identity.scope_id.into()],
            ))
            .await?
            .is_none()
        {
            return Err(ApiError::invalid_update(
                "appearance image was not uploaded for this page",
            ));
        }
    }
    let value = serde_json::to_value(&appearance).map_err(|_| ApiError::internal())?;
    let changed=tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE flow_objects SET appearance=$3,updated_at=now(),updated_by=$4 WHERE id=$1 AND scope_id=$2 AND lifecycle_status='active'",vec![object.into(),identity.scope_id.into(),value.clone().into(),user.into()])).await?;
    if changed.rows_affected() != 1 {
        return Err(ApiError::policy_rejected("page lifecycle changed"));
    }
    let event = super::scope_event::insert_for_object(
        &tx,
        &identity,
        user,
        "flow.appearance.changed",
        "flow_object",
        object,
        &key,
        json!({"object_id":object}),
        json!({}),
    )
    .await?;
    super::scope_commands::record(&tx, identity.scope_id, user, &key, body, object, event, value).await?;
    tx.commit().await?;
    Ok(appearance)
}
pub async fn upload(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    encoded: &str,
    intent: Option<&str>,
) -> Result<AppearanceAssetCreated, ApiError> {
    let (identity, proof) = editable(db, object, user).await?;
    let (content_type, bytes) = appearance::decode_image(encoded)?;
    let hash = super::collab::bootstrap::content_hash(&bytes);
    let key = operation(intent, "appearance_asset");
    let body = json!({"operation":"appearance_asset","object_id":object,"content_hash":hash});
    let size = bytes.len();
    let tx = db.begin().await?;
    if identity.workspace_id.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence(&tx, &proof).await?;
    if let Some(result) = super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &key, &body).await? {
        tx.commit().await?;
        return serde_json::from_value(result).map_err(|_| ApiError::internal());
    }
    let asset = Uuid::new_v4();
    tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_appearance_assets(id,scope_id,workspace_id,object_id,content_type,byte_size,bytes,created_by) VALUES($1,$2,$3,$4,$5,$6,$7,$8)",vec![asset.into(),identity.scope_id.into(),identity.workspace_id.into(),object.into(),content_type.into(),i32::try_from(size).map_err(|_|ApiError::internal())?.into(),bytes.into(),user.into()])).await?;
    let result = AppearanceAssetCreated {
        asset_id: asset,
        content_type: content_type.into(),
        byte_size: size,
    };
    let event = super::scope_event::insert_object_audit(
        &tx,
        &identity,
        user,
        "flow.appearance.asset_uploaded",
        &key,
        json!({"object_id":object,"asset_id":asset,"content_type":content_type,"byte_size":size}),
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &key,
        body,
        object,
        event,
        serde_json::to_value(&result).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(result)
}
pub async fn read(
    db: &DatabaseConnection,
    object: Uuid,
    asset: Uuid,
    user: Uuid,
) -> Result<AppearanceAssetBytes, ApiError> {
    let proof = scope::authorize(db, object, user, PermissionLevel::View).await?;
    let row=db.query_one(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT a.content_type,a.bytes,a.created_by,o.appearance FROM flow_appearance_assets a JOIN flow_objects o ON o.id=a.object_id AND o.scope_id=a.scope_id WHERE a.id=$1 AND a.object_id=$2 AND a.scope_id=$3",vec![asset.into(),object.into(),proof.scope_id.into()])).await?.ok_or_else(||ApiError::not_found("flow appearance asset not found"))?;
    let appearance = appearance::parse_stored(&row.try_get::<Value>("", "appearance")?);
    let creator = row.try_get::<Option<Uuid>>("", "created_by")?;
    if !appearance.references_asset(asset) && creator != Some(user) && proof.level < PermissionLevel::FullAccess {
        return Err(ApiError::not_found("flow appearance asset not found"));
    }
    let content_type = row.try_get::<String>("", "content_type")?;
    let content_type = appearance::static_content_type(&content_type).ok_or_else(ApiError::internal)?;
    let bytes = row.try_get::<Vec<u8>>("", "bytes")?;
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(AppearanceAssetBytes { content_type, bytes })
}
