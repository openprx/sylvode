//! Optional object metadata extension for package v1. Assets stay inside object.json so older
//! readers retain the original member layout; new readers preserve and remap page appearance.

use std::collections::BTreeMap;

use base64::{Engine, engine::general_purpose::STANDARD};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

use super::appearance::{self, PageAppearance, PageCover, PageIcon};
use crate::error::ApiError;

pub type PackedAssets = BTreeMap<Uuid, String>;

fn asset_ids(appearance: &PageAppearance) -> Vec<Uuid> {
    let mut ids = Vec::new();
    if let Some(PageIcon::Image { asset_id }) = appearance.icon {
        ids.push(asset_id);
    }
    if let Some(PageCover::Upload { asset_id, .. }) = appearance.cover
        && !ids.contains(&asset_id)
    {
        ids.push(asset_id);
    }
    ids
}

pub async fn export<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    object_id: Uuid,
    appearance: &PageAppearance,
) -> Result<PackedAssets, ApiError> {
    appearance::validate_appearance(appearance)?;
    let mut packed = PackedAssets::new();
    for id in asset_ids(appearance) {
        #[derive(FromQueryResult)]
        struct Asset {
            bytes: Vec<u8>,
        }
        let row = Asset::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT bytes FROM flow_appearance_assets WHERE id=$1 AND workspace_id=$2 AND object_id=$3",
            vec![id.into(), workspace_id.into(), object_id.into()],
        ))
        .one(conn)
        .await?
        .ok_or_else(|| ApiError::checksum_mismatch("page appearance references a missing asset"))?;
        packed.insert(id, STANDARD.encode(row.bytes));
    }
    validate(appearance, &packed)?;
    Ok(packed)
}

/// Reject missing, unreferenced, oversized or non-image data before preview/promotion.
/// An object can reference at most two unique assets, each subject to the normal upload limit.
pub fn validate(appearance: &PageAppearance, assets: &PackedAssets) -> Result<(), ApiError> {
    appearance::validate_appearance(appearance)?;
    let ids = asset_ids(appearance);
    if assets.len() != ids.len() || ids.iter().any(|id| !assets.contains_key(id)) {
        return Err(ApiError::unsupported_format(
            "appearance assets must exactly match page references",
        ));
    }
    for encoded in assets.values() {
        appearance::decode_image(encoded)?;
    }
    Ok(())
}

pub fn remap(appearance: &PageAppearance, target_object: Uuid) -> PageAppearance {
    let mut mapped = appearance.clone();
    if let Some(PageIcon::Image { asset_id }) = &mut mapped.icon {
        *asset_id = target_asset_id(target_object, *asset_id);
    }
    if let Some(PageCover::Upload { asset_id, .. }) = &mut mapped.cover {
        *asset_id = target_asset_id(target_object, *asset_id);
    }
    mapped
}

fn target_asset_id(target_object: Uuid, source_asset: Uuid) -> Uuid {
    Uuid::new_v5(&target_object, source_asset.as_bytes())
}

pub async fn insert_assets<C: ConnectionTrait>(
    conn: &C,
    workspace_id: Uuid,
    target_object: Uuid,
    actor_id: Option<Uuid>,
    assets: &PackedAssets,
) -> Result<(), ApiError> {
    for (source_id, encoded) in assets {
        let (content_type, bytes) = appearance::decode_image(encoded)?;
        let byte_size = i32::try_from(bytes.len()).map_err(|_| ApiError::internal())?;
        conn.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_appearance_assets (id,workspace_id,object_id,content_type,byte_size,bytes,created_by) \
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
            vec![
                target_asset_id(target_object, *source_id).into(),
                workspace_id.into(),
                target_object.into(),
                content_type.into(),
                byte_size.into(),
                bytes.into(),
                actor_id.into(),
            ],
        ))
        .await?;
    }
    Ok(())
}

pub fn to_value(appearance: &PageAppearance, target_object: Uuid) -> Result<Value, ApiError> {
    serde_json::to_value(remap(appearance, target_object)).map_err(|_| ApiError::internal())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn appearance_references_are_remapped_consistently_without_cross_object_aliases() {
        let source = Uuid::new_v4();
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let appearance = PageAppearance {
            icon: Some(PageIcon::Image { asset_id: source }),
            cover: Some(PageCover::Upload {
                asset_id: source,
                position: 0.4,
            }),
        };
        let mapped = remap(&appearance, first);
        assert!(mapped.references_asset(target_asset_id(first, source)));
        assert!(!mapped.references_asset(source));
        assert_ne!(remap(&appearance, second), mapped);
        assert_eq!(remap(&appearance, first), mapped);
    }

    #[test]
    fn package_assets_reject_missing_extra_and_invalid_image_data() {
        let id = Uuid::new_v4();
        let appearance = PageAppearance {
            icon: Some(PageIcon::Image { asset_id: id }),
            cover: None,
        };
        assert!(validate(&appearance, &PackedAssets::new()).is_err());
        assert!(
            validate(
                &PageAppearance::default(),
                &BTreeMap::from([(id, STANDARD.encode(b"not image"))])
            )
            .is_err()
        );
        assert!(validate(&appearance, &BTreeMap::from([(id, STANDARD.encode(b"not image"))])).is_err());
        assert!(validate(&PageAppearance::default(), &PackedAssets::new()).is_ok());
    }
    #[test]
    fn package_assets_apply_the_upload_size_limit_before_decoding() {
        let id = Uuid::new_v4();
        let appearance = PageAppearance {
            icon: Some(PageIcon::Image { asset_id: id }),
            cover: None,
        };
        let oversized = "A".repeat(2_097_152 * 2 + 1);
        let error = validate(&appearance, &BTreeMap::from([(id, oversized)])).unwrap_err();
        assert_eq!(error.kind(), crate::error::ApiErrorKind::LimitExceeded);
    }

    #[tokio::test]
    async fn appearance_package_export_never_reads_another_objects_asset() {
        use crate::routes::context::tenant_fixture::{exec, scratch, seed_tenant};
        let Some(scratch) = scratch("appearance_package_asset_scope").await else {
            return;
        };
        let tenant = seed_tenant(&scratch.db, "asset_scope").await;
        let first = Uuid::new_v4();
        let second = Uuid::new_v4();
        let root = crate::flow::repository::ensure_navigator_root(&scratch.db, tenant.workspace_id, None)
            .await
            .unwrap();
        for id in [first, second] {
            exec(
                &scratch.db,
                "INSERT INTO flow_objects (id,workspace_id,object_type,parent_id) VALUES ($1,$2,'page',$3)",
                vec![id.into(), tenant.workspace_id.into(), root.into()],
            )
            .await;
        }
        let asset = appearance::upload_asset(
            &scratch.db,
            tenant.workspace_id,
            first,
            tenant.member_id,
            false,
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aEmUAAAAASUVORK5CYII=",
        )
        .await
        .unwrap();
        let appearance = PageAppearance {
            icon: Some(PageIcon::Image {
                asset_id: asset.asset_id,
            }),
            cover: None,
        };
        assert!(
            export(&scratch.db, tenant.workspace_id, first, &appearance)
                .await
                .is_ok()
        );
        assert!(
            export(&scratch.db, tenant.workspace_id, second, &appearance)
                .await
                .is_err()
        );
        assert!(export(&scratch.db, Uuid::new_v4(), first, &appearance).await.is_err());
        scratch.drop_self().await;
    }
}
