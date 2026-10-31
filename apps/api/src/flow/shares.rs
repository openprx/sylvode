//! Read-only public links for a single Flow page.
//!
//! Creating, listing, and revoking require `full_access` on that page. The raw token is returned
//! once and only its SHA-256 hex is stored. A link never expands children, relations, or grants.
//! Anonymous reads are rate limited per client address inside this process.

use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use parking_lot::Mutex;
use rand::RngCore;
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use uuid::Uuid;

use super::collab::authz::PermissionLevel;
use super::repository;
use crate::error::ApiError;

const TOKEN_PREFIX: &str = "fsh_";
const TOKEN_RANDOM_BYTES: usize = 32;
const TOKEN_PREFIX_CHARS: usize = 12;
const SHARE_EXPIRY_DAYS: i64 = 366;
const READS_PER_WINDOW: u32 = 60;
const READ_WINDOW: Duration = Duration::from_mins(1);
const PUBLIC_TEXT_CAP: usize = 100_000;

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
    expires_at: Option<DateTime<Utc>>,
}

struct HitWindow {
    started: Instant,
    count: u32,
}

pub fn enforce_public_share_rate_limit(ip: IpAddr) -> Result<(), ApiError> {
    let mut windows = limiter().lock();
    if windows.len() > 10_000 {
        windows.retain(|_, window| window.started.elapsed() < READ_WINDOW);
    }
    let now = Instant::now();
    let entry = windows.entry(ip).or_insert(HitWindow { started: now, count: 0 });
    if now.saturating_duration_since(entry.started) >= READ_WINDOW {
        entry.started = now;
        entry.count = 0;
    }
    entry.count = entry.count.saturating_add(1);
    let count = entry.count;
    let retry = READ_WINDOW.saturating_sub(now.saturating_duration_since(entry.started));
    let retry_after_ms = u64::try_from(retry.as_millis()).unwrap_or(u64::MAX);
    drop(windows);
    if count > READS_PER_WINDOW {
        return Err(ApiError::limit_exceeded(
            "public share read rate limit exceeded",
            "flow_public_share_reads",
            Some(json!(READS_PER_WINDOW)),
            Some(json!(count)),
            Some(retry_after_ms),
        ));
    }
    Ok(())
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
        "SELECT fo.id AS object_id, p.title AS title, p.plain_text AS plain_text, l.expires_at \
         FROM flow_share_links l \
         JOIN flow_objects fo ON fo.id = l.object_id AND fo.workspace_id = l.workspace_id \
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
    Ok(PublicShareView {
        object_id: row.object_id,
        title: sanitize_share_text(row.title.as_deref().unwrap_or("")),
        plain_text: sanitize_share_text(row.plain_text.as_deref().unwrap_or("")),
        expires_at: row.expires_at.map(|stamp| stamp.to_rfc3339()),
    })
}

/// Tag-strips projection text before it is returned on an anonymous link.
#[must_use]
pub fn sanitize_share_text(input: &str) -> String {
    let decoded = decode_entities(input);
    let mut out = String::with_capacity(decoded.len());
    let mut in_tag = false;
    for ch in decoded.chars() {
        if in_tag {
            if ch == '>' {
                in_tag = false;
            }
            continue;
        }
        if ch == '<' {
            in_tag = true;
            continue;
        }
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        out.push(ch);
        if out.chars().count() >= PUBLIC_TEXT_CAP {
            break;
        }
    }
    out
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

fn decode_entities(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn limiter() -> &'static Mutex<HashMap<IpAddr, HitWindow>> {
    static LIMITER: OnceLock<Mutex<HashMap<IpAddr, HitWindow>>> = OnceLock::new();
    LIMITER.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::{enforce_public_share_rate_limit, sanitize_share_text};

    #[test]
    fn public_text_strips_tags_after_decoding_entities() {
        let raw = "Hello <script>alert(1)</script> &amp; &lt;b&gt;x&lt;/b&gt;";
        assert_eq!(sanitize_share_text(raw), "Hello alert(1) & x");
    }

    #[test]
    fn public_share_reads_stop_at_sixty_per_minute_for_one_address() {
        let ip = IpAddr::V4(Ipv4Addr::new(198, 51, 100, 23));
        let mut failures = 0_u32;
        for _ in 0..60 {
            if enforce_public_share_rate_limit(ip).is_err() {
                failures = failures.saturating_add(1);
            }
        }
        assert_eq!(failures, 0, "the first sixty reads in a window are allowed");
        assert!(
            enforce_public_share_rate_limit(ip).is_err(),
            "the sixty-first read is limited"
        );
    }
}
