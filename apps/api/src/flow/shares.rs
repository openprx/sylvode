//! Read-only public links for a single Flow page.
//!
//! Creating, listing, and revoking require `full_access` on that page. The raw token is returned
//! once and only its SHA-256 hex is stored. A link never expands children, relations, or grants.
//! Anonymous reads share one Postgres counter across API processes.

use std::net::IpAddr;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::RngCore;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::appearance::{self, PageAppearance};
use super::collab::authz::PermissionLevel;
use super::document;
use super::public_page::{self, PublicBlock, sanitize_share_text};
use super::repository;
use crate::error::ApiError;

const TOKEN_PREFIX: &str = "fsh_";
const TOKEN_RANDOM_BYTES: usize = 32;
const TOKEN_PREFIX_CHARS: usize = 12;
const SHARE_EXPIRY_DAYS: i64 = 366;
const READS_PER_WINDOW: i32 = 60;
const READ_WINDOW_SECS: i64 = 60;

#[derive(Debug, Serialize)]
pub struct ShareLinkView {
    pub id: Uuid,
    pub token_prefix: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
    pub created_by: Option<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct CreatedShareLink {
    pub link: ShareLinkView,
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct ShareLinkList {
    pub items: Vec<ShareLinkView>,
}

#[derive(Debug, Serialize)]
pub struct PublicShareView {
    pub object_id: Uuid,
    pub title: String,
    pub plain_text: String,
    pub blocks: Vec<PublicBlock>,
    #[serde(skip_serializing_if = "PageAppearance::is_empty")]
    pub appearance: PageAppearance,
    pub expires_at: Option<String>,
}

#[derive(Debug, FromQueryResult)]
struct LinkRow {
    id: Uuid,
    token_prefix: String,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    created_by: Option<Uuid>,
}

#[derive(Debug, FromQueryResult)]
struct PublicRow {
    object_id: Uuid,
    title: Option<String>,
    plain_text: Option<String>,
    appearance: Value,
    document_id: Option<Uuid>,
    expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromQueryResult)]
struct HitRow {
    hit_count: i32,
    window_started: DateTime<Utc>,
}

#[derive(Debug, FromQueryResult)]
struct AssetPage {
    object_id: Uuid,
    workspace_id: Uuid,
    appearance: Value,
}

/// The next hit in a fixed window. `allowed` is true while `count` is still within `limit`.
#[must_use]
pub fn advance_rate_window(previous_count: Option<u32>, window_open: bool, limit: u32) -> (u32, bool) {
    let count = if window_open {
        previous_count.unwrap_or(0).saturating_add(1)
    } else {
        1
    };
    (count, count <= limit)
}

/// Shared across processes: one row per hashed client address, updated in a single statement.
pub async fn enforce_public_share_rate_limit(db: &DatabaseConnection, ip: IpAddr) -> Result<(), ApiError> {
    let client_hash = hash_token(&ip.to_string());
    let sql = format!(
        "INSERT INTO flow_public_share_hits AS hit (client_hash, window_started, hit_count) \
         VALUES ($1, now(), 1) \
         ON CONFLICT (client_hash) DO UPDATE \
         SET window_started = CASE \
               WHEN hit.window_started <= now() - interval '{READ_WINDOW_SECS} seconds' THEN now() \
               ELSE hit.window_started \
             END, \
             hit_count = CASE \
               WHEN hit.window_started <= now() - interval '{READ_WINDOW_SECS} seconds' THEN 1 \
               ELSE hit.hit_count + 1 \
             END \
         RETURNING hit_count, window_started"
    );
    let row = HitRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        &sql,
        vec![client_hash.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)?;
    if row.hit_count == 1
        && let Err(error) = db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM flow_public_share_hits WHERE window_started < now() - interval '1 day'",
                vec![],
            ))
            .await
    {
        tracing::warn!(error = %error, "flow public share rate prune failed");
    }
    let observed = u32::try_from(row.hit_count).unwrap_or(u32::MAX);
    let limit = u32::try_from(READS_PER_WINDOW).unwrap_or(u32::MAX);
    if observed <= limit {
        return Ok(());
    }
    let elapsed = Utc::now().signed_duration_since(row.window_started);
    let remain = ChronoDuration::seconds(READ_WINDOW_SECS) - elapsed;
    let retry_after_ms = u64::try_from(remain.num_milliseconds().max(0)).unwrap_or(0);
    Err(ApiError::limit_exceeded(
        "public share read rate limit exceeded",
        "flow_public_share_reads",
        Some(serde_json::json!(limit)),
        Some(serde_json::json!(observed)),
        Some(retry_after_ms),
    ))
}

pub async fn list_links(
    db: &DatabaseConnection,
    object_id: Uuid,
    level: PermissionLevel,
) -> Result<ShareLinkList, ApiError> {
    require_manage(level)?;
    let rows = LinkRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, token_prefix, expires_at, revoked_at, created_at, created_by \
         FROM flow_share_links WHERE object_id = $1 ORDER BY created_at DESC, id DESC",
        vec![object_id.into()],
    ))
    .all(db)
    .await?;
    Ok(ShareLinkList {
        items: rows.into_iter().map(view_of).collect(),
    })
}

pub async fn create_link(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    object_id: Uuid,
    actor_id: Uuid,
    level: PermissionLevel,
    expires_at: Option<DateTime<Utc>>,
) -> Result<CreatedShareLink, ApiError> {
    require_manage(level)?;
    ensure_shareable_page(db, workspace_id, object_id).await?;
    let expires_at = validate_expiry(expires_at)?;
    let token = mint_token();
    let token_hash = hash_token(&token);
    let token_prefix = prefix_of(&token);
    let id = Uuid::new_v4();
    let now = Utc::now();
    let row = LinkRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO flow_share_links ( \
            id, workspace_id, object_id, token_hash, token_prefix, created_by, expires_at, created_at \
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
         RETURNING id, token_prefix, expires_at, revoked_at, created_at, created_by",
        vec![
            id.into(),
            workspace_id.into(),
            object_id.into(),
            token_hash.into(),
            token_prefix.into(),
            actor_id.into(),
            expires_at.into(),
            now.into(),
        ],
    ))
    .one(db)
    .await?
    .ok_or_else(ApiError::internal)?;
    Ok(CreatedShareLink {
        link: view_of(row),
        token,
    })
}

pub async fn revoke_link(
    db: &DatabaseConnection,
    object_id: Uuid,
    link_id: Uuid,
    level: PermissionLevel,
) -> Result<ShareLinkView, ApiError> {
    require_manage(level)?;
    let now = Utc::now();
    let row = LinkRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE flow_share_links SET revoked_at = COALESCE(revoked_at, $3) \
         WHERE id = $1 AND object_id = $2 \
         RETURNING id, token_prefix, expires_at, revoked_at, created_at, created_by",
        vec![link_id.into(), object_id.into(), now.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::not_found("flow share link not found".to_string()))?;
    Ok(view_of(row))
}

pub async fn read_public(db: &DatabaseConnection, token: &str) -> Result<PublicShareView, ApiError> {
    if !valid_token(token) {
        return Err(missing());
    }
    let token_hash = hash_token(token);
    let row = PublicRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.id AS object_id, p.title AS title, p.plain_text AS plain_text, \
                fo.appearance AS appearance, cd.id AS document_id, l.expires_at \
         FROM flow_share_links l \
         JOIN flow_objects fo ON fo.id = l.object_id AND fo.workspace_id = l.workspace_id \
         LEFT JOIN collab_documents cd ON cd.object_id = fo.id \
         LEFT JOIN flow_object_projections p ON p.object_id = fo.id \
         WHERE l.token_hash = $1 \
           AND l.revoked_at IS NULL \
           AND (l.expires_at IS NULL OR l.expires_at > now()) \
           AND fo.object_type = 'page' \
           AND fo.lifecycle_status = 'active'",
        vec![token_hash.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(missing)?;
    let workspace_id = workspace_of_object(db, row.object_id).await?;
    if !repository::fetch_flow_enabled(db, workspace_id).await? {
        return Err(missing());
    }
    let appearance = appearance::parse_stored(&row.appearance);
    // A page can be shared before anyone opens it in the editor. The projection is the text then.
    let (blocks, composed) = if let Some(document_id) = row.document_id {
        let engine = document::load_head_engine(db, document_id).await?;
        let semantic = engine.semantic_snapshot().map_err(|_| ApiError::internal())?;
        let blocks = public_page::compose_public_document(&engine.root_state_json(), &semantic);
        let composed = public_page::plain_text_of(&blocks);
        (blocks, composed)
    } else {
        (Vec::new(), String::new())
    };
    let plain_text = if composed.is_empty() {
        sanitize_share_text(row.plain_text.as_deref().unwrap_or(""))
    } else {
        composed
    };
    Ok(PublicShareView {
        object_id: row.object_id,
        title: sanitize_share_text(row.title.as_deref().unwrap_or("")),
        plain_text,
        blocks,
        appearance,
        expires_at: row.expires_at.map(|stamp| stamp.to_rfc3339()),
    })
}

pub async fn read_public_asset(
    db: &DatabaseConnection,
    token: &str,
    asset_id: Uuid,
) -> Result<appearance::AppearanceAssetBytes, ApiError> {
    if !valid_token(token) {
        return Err(missing());
    }
    let token_hash = hash_token(token);
    let page = AssetPage::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT fo.id AS object_id, fo.workspace_id, fo.appearance \
         FROM flow_share_links l \
         JOIN flow_objects fo ON fo.id = l.object_id AND fo.workspace_id = l.workspace_id \
         WHERE l.token_hash = $1 \
           AND l.revoked_at IS NULL \
           AND (l.expires_at IS NULL OR l.expires_at > now()) \
           AND fo.object_type = 'page' \
           AND fo.lifecycle_status = 'active'",
        vec![token_hash.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(missing)?;
    if !repository::fetch_flow_enabled(db, page.workspace_id).await? {
        return Err(missing());
    }
    let appearance = appearance::parse_stored(&page.appearance);
    if !appearance.references_asset(asset_id) {
        return Err(missing());
    }
    appearance::read_asset(db, page.workspace_id, page.object_id, asset_id).await
}

fn require_manage(level: PermissionLevel) -> Result<(), ApiError> {
    if level >= PermissionLevel::FullAccess {
        Ok(())
    } else {
        Err(ApiError::forbidden(
            "full access is required to manage share links".to_string(),
        ))
    }
}

async fn ensure_shareable_page(db: &DatabaseConnection, workspace_id: Uuid, object_id: Uuid) -> Result<(), ApiError> {
    #[derive(FromQueryResult)]
    struct PageRow {
        object_type: String,
        lifecycle_status: String,
    }
    let row = PageRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT object_type, lifecycle_status FROM flow_objects WHERE id = $1 AND workspace_id = $2",
        vec![object_id.into(), workspace_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(missing)?;
    if row.object_type != "page" || row.lifecycle_status != "active" {
        return Err(ApiError::invalid_update(
            "only an active page can be shared by link".to_string(),
        ));
    }
    Ok(())
}

async fn workspace_of_object(db: &DatabaseConnection, object_id: Uuid) -> Result<Uuid, ApiError> {
    #[derive(FromQueryResult)]
    struct WorkspaceRow {
        workspace_id: Uuid,
    }
    WorkspaceRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM flow_objects WHERE id = $1",
        vec![object_id.into()],
    ))
    .one(db)
    .await?
    .map(|row| row.workspace_id)
    .ok_or_else(missing)
}

fn validate_expiry(expires_at: Option<DateTime<Utc>>) -> Result<Option<DateTime<Utc>>, ApiError> {
    let Some(expires_at) = expires_at else {
        return Ok(None);
    };
    let now = Utc::now();
    if expires_at <= now {
        return Err(ApiError::invalid_update(
            "share link expiry must be in the future".to_string(),
        ));
    }
    let latest = now + ChronoDuration::days(SHARE_EXPIRY_DAYS);
    if expires_at > latest {
        return Err(ApiError::invalid_update(
            "share link expiry must be within 366 days".to_string(),
        ));
    }
    Ok(Some(expires_at))
}

fn mint_token() -> String {
    let mut bytes = [0_u8; TOKEN_RANDOM_BYTES];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", hex::encode(bytes))
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn prefix_of(token: &str) -> String {
    token.chars().take(TOKEN_PREFIX_CHARS).collect()
}

fn valid_token(token: &str) -> bool {
    let mut chars = token.chars();
    let prefix: String = chars.by_ref().take(TOKEN_PREFIX.len()).collect();
    if prefix != TOKEN_PREFIX {
        return false;
    }
    let rest: String = chars.collect();
    rest.len() == TOKEN_RANDOM_BYTES * 2 && rest.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn missing() -> ApiError {
    ApiError::not_found("flow share link not found".to_string())
}

fn view_of(row: LinkRow) -> ShareLinkView {
    ShareLinkView {
        id: row.id,
        token_prefix: row.token_prefix,
        expires_at: row.expires_at.map(|stamp| stamp.to_rfc3339()),
        revoked_at: row.revoked_at.map(|stamp| stamp.to_rfc3339()),
        created_at: row.created_at.to_rfc3339(),
        created_by: row.created_by,
    }
}

#[cfg(test)]
mod tests {
    use super::{advance_rate_window, sanitize_share_text};

    #[test]
    fn public_text_strips_tags_after_decoding_entities() {
        let raw = "Hello <script>alert(1)</script> &amp; &lt;b&gt;x&lt;/b&gt;";
        assert_eq!(sanitize_share_text(raw), "Hello alert(1) & x");
    }

    #[test]
    fn public_share_reads_stop_at_sixty_per_minute_for_one_address() {
        let mut count = None;
        for step in 1..=60 {
            let (next, allowed) = advance_rate_window(count, true, 60);
            assert!(allowed, "read {step} is inside the window");
            assert_eq!(next, step);
            count = Some(next);
        }
        let (next, allowed) = advance_rate_window(count, true, 60);
        assert!(!allowed, "the sixty-first read is limited");
        assert_eq!(next, 61);
        let (reset, allowed) = advance_rate_window(Some(61), false, 60);
        assert!(allowed, "a new window starts at one");
        assert_eq!(reset, 1);
    }

    #[test]
    fn rate_limit_sql_uses_the_shared_window() {
        let sql = include_str!("shares.rs");
        assert!(sql.contains("flow_public_share_hits"));
        assert!(sql.contains("READ_WINDOW_SECS"));
        assert_eq!(super::READ_WINDOW_SECS, 60);
        assert_eq!(super::READS_PER_WINDOW, 60);
    }
}
