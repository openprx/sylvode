//! Workspace bots: the bot user, its workspace seat and its `opr_` token.
//!
//! Every function takes the connection or transaction to run on, so the Bots page's routes and the
//! project bot connection service run the same code inside whichever transaction they own.
//!
//! A bot is *managed* when a `bot_runners` row names it, that is, when a project bot connection
//! created it. Nothing marks the row itself: [`managed_by`] derives the answer from
//! `bot_runners` joined to `project_bot_connections`, so removing the last connection un-marks the
//! bot in the same transaction.

use platform::timestamp::Timestamp;
use rand::Rng;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::ApiError;
use crate::flow::event_origin::EventSurface;
use crate::middleware::bot_auth::{BotPermission, bot_permissions_allow};
use crate::services::bot_identity::BOT_ENTITY_TYPE;

/// `users.agent_type` of a bot created through this API.
///
/// Such a bot acts through its `opr_` token, most often from an MCP client. Its entity type is the
/// ordinary [`BOT_ENTITY_TYPE`], so it can be assigned work, named as a webhook's bot user and
/// registered as an AI participant like any other bot.
pub const TOKEN_BOT_AGENT_TYPE: &str = "mcp";

/// What the managed-row 409 says, on every endpoint that refuses an edit of a managed row.
pub const MANAGED_REFUSAL: &str = "managed by a project bot connection; change it in the project's Execution settings";

/// What the `bot_user_id` 400 says when a webhook names a managed bot.
pub const MANAGED_BOT_USER_REFUSAL: &str = "bot_user_id: this bot's deliveries are managed by a project bot connection";

/// A project a managed row serves.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManagedProject {
    pub id: Uuid,
    pub name: String,
}

/// Why a bot or webhook cannot be edited on the Bots and Webhooks endpoints.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManagedBy {
    pub kind: &'static str,
    pub projects: Vec<ManagedProject>,
}

impl ManagedBy {
    const KIND: &'static str = "project_connection";
}

/// Whether an operation may touch a managed row. The Bots and Webhooks routes refuse; the project
/// bot connection service is the one caller that allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedPolicy {
    Refuse,
    Allow,
}

/// The projects served by the managed bot `bot_id`, or `None` when `bot_id` is not managed.
pub async fn managed_by(db: &impl ConnectionTrait, bot_id: Uuid) -> Result<Option<ManagedBy>, ApiError> {
    let exists = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM bot_runners WHERE bot_id = $1",
            vec![bot_id.into()],
        ))
        .await?
        .is_some();
    if !exists {
        return Ok(None);
    }
    Ok(Some(managed_projects(db, bot_id).await?))
}

/// The projects a managed bot is connected to, for a bot already known to be managed.
pub async fn managed_projects(db: &impl ConnectionTrait, bot_id: Uuid) -> Result<ManagedBy, ApiError> {
    #[derive(FromQueryResult)]
    struct ProjectRow {
        id: Uuid,
        name: String,
    }
    let rows = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"SELECT p.id, p.name
          FROM project_bot_connections c
          JOIN projects p ON p.id = c.project_id
          WHERE c.bot_id = $1
          ORDER BY p.name, p.id",
        vec![bot_id.into()],
    ))
    .all(db)
    .await?;
    Ok(ManagedBy {
        kind: ManagedBy::KIND,
        projects: rows
            .into_iter()
            .map(|row| ManagedProject {
                id: row.id,
                name: row.name,
            })
            .collect(),
    })
}

/// 409 unless `bot_id` is unmanaged or the policy allows managed rows.
pub async fn ensure_editable(db: &impl ConnectionTrait, bot_id: Uuid, policy: ManagedPolicy) -> Result<(), ApiError> {
    if policy == ManagedPolicy::Refuse && managed_by(db, bot_id).await?.is_some() {
        return Err(ApiError::Conflict(MANAGED_REFUSAL.to_string()));
    }
    Ok(())
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

fn generate_token() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen::<u8>()).collect();
    format!("opr_{}", hex::encode(&bytes))
}

/// First 8 characters of a raw token, e.g. `opr_a1b2`: the part safe to show and log.
fn token_prefix_of(raw_token: &str) -> String {
    raw_token.chars().take(8).collect()
}

/// The role a bot is seated with: only the `admin` permission makes it a workspace admin.
pub fn workspace_role_from_permissions(perms: &[String]) -> &'static str {
    if bot_permissions_allow(perms, BotPermission::Admin) {
        "admin"
    } else {
        "member"
    }
}

/// Validate and canonicalize the permission list a bot token will carry.
///
/// The stored array is what every request is judged against, so it has to be exactly the
/// vocabulary the enforcement side understands: unknown names grant nothing there and would
/// silently produce an inert token here. Duplicates are collapsed so the stored value reads the
/// same as it behaves.
pub fn normalize_bot_permissions(permissions: Vec<String>) -> Result<Vec<String>, ApiError> {
    const VALID_PERMISSIONS: [&str; 3] = ["read", "write", "admin"];
    let mut normalized: Vec<String> = Vec::with_capacity(permissions.len());
    for permission in permissions {
        let permission = permission.trim();
        if !VALID_PERMISSIONS.contains(&permission) {
            return Err(ApiError::BadRequest(format!(
                "invalid permission '{permission}', must be one of: read, write, admin"
            )));
        }
        if !normalized.iter().any(|kept| kept == permission) {
            normalized.push(permission.to_string());
        }
    }
    if normalized.is_empty() {
        return Err(ApiError::BadRequest(
            "permissions must contain at least one of: read, write, admin".to_string(),
        ));
    }
    Ok(normalized)
}

/// The transport surface a token is bound to; `rest` when none is given.
pub fn normalize_transport_surface(value: Option<String>) -> Result<String, ApiError> {
    let value = value.unwrap_or_else(|| "rest".to_string());
    let value = value.trim();
    EventSurface::from_bot_credential_label(value)
        .map(|surface| surface.as_wire().to_string())
        .ok_or_else(|| ApiError::BadRequest(format!("transport_surface must be {}", accepted_transport_surfaces())))
}

/// `rest, mcp_http, ..., or cli_tools_call`, spelled from the one accepted-surface list.
fn accepted_transport_surfaces() -> String {
    let mut text = String::new();
    let last = EventSurface::BOT_CREDENTIAL_SURFACES.len().saturating_sub(1);
    for (index, surface) in EventSurface::BOT_CREDENTIAL_SURFACES.iter().enumerate() {
        if index > 0 {
            text.push_str(if index == last { ", or " } else { ", " });
        }
        text.push_str(surface.as_wire());
    }
    text
}

/// What it takes to create a bot. Permissions and surface are already validated.
#[derive(Debug)]
pub struct NewBot<'a> {
    pub workspace_id: Uuid,
    pub created_by: Uuid,
    pub name: &'a str,
    pub permissions: Vec<String>,
    pub transport_surface: String,
    pub expires_at: Option<Timestamp>,
}

/// A created bot, with its raw token: the only place the token ever exists in clear.
pub struct CreatedBot {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: String,
    pub token: String,
    pub token_prefix: String,
    pub permissions: Vec<String>,
    pub transport_surface: String,
    pub expires_at: Option<Timestamp>,
    pub created_at: Timestamp,
    /// The Flow authorization epoch advanced: the caller invalidates the workspace's permission
    /// cache after the surrounding transaction commits.
    pub epoch_advanced: bool,
}

impl std::fmt::Debug for CreatedBot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedBot")
            .field("id", &self.id)
            .field("workspace_id", &self.workspace_id)
            .field("name", &self.name)
            .field("token_prefix", &self.token_prefix)
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Creates the bot user, its workspace seat and its `workspace_bots` row on `tx`. The caller owns
/// the transaction and the commit.
pub async fn create(tx: &impl ConnectionTrait, input: NewBot<'_>) -> Result<CreatedBot, ApiError> {
    let raw_token = generate_token();
    let token_hash = sha256_hex(&raw_token);
    let token_prefix = token_prefix_of(&raw_token);

    let bot_id = Uuid::new_v4();
    let now = Timestamp::now();
    let perms_json = serde_json::to_value(&input.permissions).unwrap_or_else(|_| serde_json::json!(["read"]));
    let workspace_role = workspace_role_from_permissions(&input.permissions);
    let bot_email = format!("{bot_id}@bot.sylvode.local");

    let advanced_epoch = crate::flow::collab::authz::advance_epoch_if_present(tx, input.workspace_id).await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO users
           (id, email, password_hash, name, role, is_active, entity_type, agent_type, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, true, $6, $7, $8, $8)",
        vec![
            bot_id.into(),
            bot_email.into(),
            "!".into(),
            input.name.to_string().into(),
            "user".into(),
            BOT_ENTITY_TYPE.into(),
            TOKEN_BOT_AGENT_TYPE.into(),
            now.into(),
        ],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
           VALUES ($1, $2, $3, $4)",
        vec![
            input.workspace_id.into(),
            bot_id.into(),
            workspace_role.into(),
            now.into(),
        ],
    ))
    .await?;

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO workspace_bots
           (id, workspace_id, name, token_hash, token_prefix, permissions, transport_surface,
            created_by, expires_at, is_active, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, true, $10, $10)",
        vec![
            bot_id.into(),
            input.workspace_id.into(),
            input.name.to_string().into(),
            token_hash.into(),
            token_prefix.clone().into(),
            perms_json.into(),
            input.transport_surface.clone().into(),
            input.created_by.into(),
            input.expires_at.into(),
            now.into(),
        ],
    ))
    .await?;

    Ok(CreatedBot {
        id: bot_id,
        workspace_id: input.workspace_id,
        name: input.name.to_string(),
        token: raw_token,
        token_prefix,
        permissions: input.permissions,
        transport_surface: input.transport_surface,
        expires_at: input.expires_at,
        created_at: now,
        epoch_advanced: advanced_epoch.is_some(),
    })
}

/// Revokes the token of a bot of `workspace_id` (`is_active = false`). 404 when the workspace has
/// no such bot; 409 for a managed bot unless `policy` allows it.
pub async fn revoke(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    bot_id: Uuid,
    policy: ManagedPolicy,
) -> Result<(), ApiError> {
    let exists = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND workspace_id = $2",
            vec![bot_id.into(), workspace_id.into()],
        ))
        .await?;
    if exists.is_none() {
        return Err(ApiError::NotFound("bot not found".to_string()));
    }
    ensure_editable(db, bot_id, policy).await?;
    set_token_active(db, bot_id, false).await
}

/// Switches a bot's token on or off without touching anything else.
pub async fn set_token_active(db: &impl ConnectionTrait, bot_id: Uuid, active: bool) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE workspace_bots SET is_active = $1, updated_at = $2 WHERE id = $3",
        vec![active.into(), Timestamp::now().into(), bot_id.into()],
    ))
    .await?;
    Ok(())
}

/// A token issued by [`rotate_token`].
pub struct RotatedToken {
    pub token: String,
    pub token_prefix: String,
    /// Prefix of the token that stopped working, for the audit event.
    pub previous_prefix: String,
}

impl std::fmt::Debug for RotatedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RotatedToken")
            .field("token_prefix", &self.token_prefix)
            .field("token", &"<redacted>")
            .finish_non_exhaustive()
    }
}

/// Replaces a bot's token. The old token stops working with the surrounding commit. 404 when the
/// bot has no `workspace_bots` row.
pub async fn rotate_token(db: &impl ConnectionTrait, bot_id: Uuid) -> Result<RotatedToken, ApiError> {
    #[derive(FromQueryResult)]
    struct PrefixRow {
        token_prefix: String,
    }
    let previous = PrefixRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT token_prefix FROM workspace_bots WHERE id = $1 FOR UPDATE",
        vec![bot_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("bot not found".to_string()))?;

    let raw_token = generate_token();
    let token_prefix = token_prefix_of(&raw_token);
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE workspace_bots SET token_hash = $1, token_prefix = $2, updated_at = $3 WHERE id = $4",
        vec![
            sha256_hex(&raw_token).into(),
            token_prefix.clone().into(),
            Timestamp::now().into(),
            bot_id.into(),
        ],
    ))
    .await?;
    Ok(RotatedToken {
        token: raw_token,
        token_prefix,
        previous_prefix: previous.token_prefix,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        generate_token, normalize_bot_permissions, normalize_transport_surface, sha256_hex, token_prefix_of,
        workspace_role_from_permissions,
    };
    use crate::{error::ApiError, flow::event_origin::EventSurface};

    #[test]
    fn transport_surface_accepts_exactly_the_bot_credential_surfaces() {
        assert_eq!(normalize_transport_surface(None).expect("default"), "rest");
        for surface in EventSurface::BOT_CREDENTIAL_SURFACES {
            assert_eq!(
                normalize_transport_surface(Some(format!(" {} ", surface.as_wire()))).expect("accepted surface"),
                surface.as_wire()
            );
        }
        for refused in ["web", "worker", "system", "MCP_HTTP", ""] {
            match normalize_transport_surface(Some(refused.to_string())) {
                Err(ApiError::BadRequest(message)) => assert_eq!(
                    message,
                    "transport_surface must be rest, mcp_http, mcp_sse, mcp_stdio, cli, or cli_tools_call"
                ),
                other => panic!("`{refused}` must be refused, got {other:?}"),
            }
        }
    }

    #[test]
    fn permissions_are_canonicalized_and_deduplicated() {
        let normalized = normalize_bot_permissions(vec![" read ".to_string(), "write".to_string(), "read".to_string()])
            .expect("known permissions are accepted");

        assert_eq!(normalized, vec!["read".to_string(), "write".to_string()]);
    }

    #[test]
    fn a_token_that_could_do_nothing_is_not_issued() {
        assert!(normalize_bot_permissions(Vec::new()).is_err());
        assert!(normalize_bot_permissions(vec!["Read".to_string()]).is_err());
        assert!(normalize_bot_permissions(vec!["owner".to_string()]).is_err());
    }

    #[test]
    fn only_the_admin_permission_seats_the_bot_as_a_workspace_admin() {
        assert_eq!(workspace_role_from_permissions(&["admin".to_string()]), "admin");
        assert_eq!(
            workspace_role_from_permissions(&["read".to_string(), "write".to_string()]),
            "member"
        );
    }

    #[test]
    fn tokens_are_prefixed_hashed_and_distinct() {
        let token = generate_token();
        assert!(token.starts_with("opr_"));
        assert_eq!(token.len(), 4 + 64);
        assert_eq!(token_prefix_of(&token).len(), 8);
        assert_eq!(sha256_hex(&token).len(), 64);
        assert_ne!(token, generate_token());
    }
}
