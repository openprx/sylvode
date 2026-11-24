//! Next-batch SQL preparation only. Not registered in the P3A API module/routes.
//!
//! Callers supply current Page authorization first; these facts never become recipient DTOs.
//! All queries deliberately exclude document snapshots, comment bodies and asset bytes.
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

#[derive(Debug, FromQueryResult)]
pub struct PageMetadata {
    pub object_id: Uuid,
    pub scope_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub object_type: String,
    pub lifecycle_status: String,
    pub document_id: Uuid,
    pub head_seq: i64,
    pub head_frontier: Vec<u8>,
    pub registry_head: Option<i64>,
}
#[derive(Debug, FromQueryResult)]
pub struct PlacementMetadata {
    pub logical_id: Uuid,
    pub physical_cid: String,
    pub generation: i64,
}
#[derive(Debug, FromQueryResult)]
pub struct AssetMetadata {
    pub id: Uuid,
    pub object_id: Uuid,
    pub kind: String,
    pub content_type: String,
    pub byte_size: i32,
    pub content_sha256: String,
    pub state: String,
    pub current_reference: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, FromQueryResult, serde::Serialize, serde::Deserialize)]
pub struct ShareMemberMetadata {
    pub share_id: Uuid,
    pub root_logical_id: Uuid,
    pub logical_id: Uuid,
    pub state: String,
    pub auth_revision: i64,
}
#[derive(Debug)]
pub enum Error {
    Database(sea_orm::DbErr),
    InvalidSelection,
    MissingPage,
    RegistryNotCurrent,
}
impl From<sea_orm::DbErr> for Error {
    fn from(error: sea_orm::DbErr) -> Self {
        Self::Database(error)
    }
}

pub async fn pages<C: ConnectionTrait>(db: &C, source: Uuid, destination: Uuid) -> Result<Vec<PageMetadata>, Error> {
    if source == destination {
        return Err(Error::InvalidSelection);
    }
    let rows=PageMetadata::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT o.id AS object_id,o.scope_id,o.workspace_id,o.object_type,o.lifecycle_status,d.id AS document_id,d.head_seq,d.head_frontier,c.accepted_head AS registry_head FROM flow_objects o JOIN collab_documents d ON d.object_id=o.id LEFT JOIN flow_block_checkpoints c ON c.page_id=o.id AND c.scope_id=o.scope_id WHERE o.id=ANY($1::uuid[]) ORDER BY o.id",
        vec![vec![source,destination].into()])).all(db).await?;
    if rows.len() != 2 || rows.iter().any(|row| row.object_type != "page") {
        return Err(Error::MissingPage);
    }
    if rows.iter().any(|row| row.registry_head != Some(row.head_seq)) {
        return Err(Error::RegistryNotCurrent);
    }
    Ok(rows)
}
pub async fn active_placements<C: ConnectionTrait>(
    db: &C,
    page: Uuid,
    cids: &[String],
) -> Result<Vec<PlacementMetadata>, Error> {
    if cids.is_empty() || cids.len() > 256 || cids.iter().any(|cid| cid.is_empty() || cid.len() > 256) {
        return Err(Error::InvalidSelection);
    }
    let rows=PlacementMetadata::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT logical_id,physical_cid,generation FROM flow_block_placements WHERE page_id=$1 AND physical_cid=ANY($2::text[]) AND state='active' ORDER BY logical_id",
        vec![page.into(),cids.to_vec().into()])).all(db).await?;
    if rows.len() != cids.len() {
        return Err(Error::InvalidSelection);
    }
    Ok(rows)
}
pub async fn assets<C: ConnectionTrait>(db: &C, page: Uuid, ids: &[Uuid]) -> Result<Vec<AssetMetadata>, Error> {
    if ids.len() > 256 {
        return Err(Error::InvalidSelection);
    }
    // No filename/provenance is copied implicitly; selected typed labels determine safe names.
    Ok(AssetMetadata::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT a.id,a.object_id,a.kind,a.content_type,a.byte_size,a.content_sha256,a.state,EXISTS(SELECT 1 FROM flow_page_asset_refs r JOIN collab_documents d ON d.object_id=r.object_id WHERE r.asset_id=a.id AND r.object_id=$1 AND r.active AND r.last_seen_seq=d.head_seq) AS current_reference FROM flow_page_assets a WHERE a.object_id=$1 AND a.id=ANY($2::uuid[]) ORDER BY a.id",
        vec![page.into(),ids.to_vec().into()])).all(db).await?)
}
pub async fn affected_shares<C: ConnectionTrait>(
    db: &C,
    logical_ids: &[Uuid],
) -> Result<Vec<ShareMemberMetadata>, Error> {
    if logical_ids.len() > 256 {
        return Err(Error::InvalidSelection);
    }
    Ok(ShareMemberMetadata::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT s.id AS share_id,s.root_logical_id,m.logical_id,s.state,s.auth_revision FROM flow_block_shares s JOIN flow_block_share_members m ON m.share_id=s.id WHERE s.state<>'revoked' AND EXISTS(SELECT 1 FROM flow_block_share_members selected WHERE selected.share_id=s.id AND selected.logical_id=ANY($1::uuid[])) ORDER BY s.id,m.logical_id",
        vec![logical_ids.to_vec().into()])).all(db).await?)
}
