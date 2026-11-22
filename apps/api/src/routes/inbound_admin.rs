//! Management of inbound sources and alert rules (`/api/v1/.../inbound-sources`, `.../alert-rules`).
//!
//! Only workspace owners and admins signed in as themselves may call these routes. A bot token is
//! refused whatever its permissions (a bot that could edit rules could make its own output trigger
//! agents), and a connected application's token is refused because none of these routes is on the
//! delegated allow-list. Secrets are returned only by the response that creates or rotates them.
//! Every change writes a business event with the old and new values, never a secret.

use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    inbound::{
        allowlist, audit, credentials,
        dry_run::{self, EventFacts, Limits, SourceFacts},
        envelope::{EnvelopeError, parse_envelope},
        notify,
        rule_input::{RuleInput, ValidatedRule},
        rules::{RULE_COLUMNS, RuleRow},
    },
    middleware::bot_auth::{BotAuthContext, require_workspace_access_from_auth},
    response::ApiResponse,
    services::workflow_service::resolve_effective_workflow_for_project,
};

const NAME_MAX: usize = 100;
const EVENTS_DEFAULT_LIMIT: i64 = 50;
const EVENTS_MAX_LIMIT: i64 = 100;
/// The only payload format: the native envelope. Sources of every product send it.
const FORMAT_NATIVE: &str = "native_v1";

// ---------------------------------------------------------------------------------------------
// Authorization
// ---------------------------------------------------------------------------------------------

/// The signed-in user behind the request, once it is known to be an owner or admin of
/// `workspace_id`. A bot (any permissions), a delegated caller and a plain member are refused.
async fn require_workspace_admin(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    workspace_id: Uuid,
) -> Result<Uuid, ApiError> {
    if bot.is_some() {
        return Err(ApiError::Forbidden(
            "inbound sources and alert rules are managed by workspace owners and admins, not by bots".to_owned(),
        ));
    }
    let (user_id, role, _) = require_workspace_access_from_auth(state, claims, None, workspace_id).await?;
    if role == "owner" || role == "admin" {
        Ok(user_id)
    } else {
        Err(ApiError::Forbidden(
            "only workspace owners and admins manage inbound sources and alert rules".to_owned(),
        ))
    }
}

#[derive(FromQueryResult)]
struct WorkspaceOf {
    workspace_id: Uuid,
}

async fn project_workspace(state: &AppState, project_id: Uuid) -> Result<Uuid, ApiError> {
    WorkspaceOf::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .map(|row| row.workspace_id)
    .ok_or_else(|| ApiError::NotFound("project not found".to_owned()))
}

fn actor_source(user_id: Uuid) -> Value {
    json!({ "type": "user", "actor_id": user_id })
}

// ---------------------------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------------------------

const SOURCE_COLUMNS: &str = "id, workspace_id, project_id, name, format, token_prefix, \
     require_signature, (signing_secret IS NOT NULL) AS has_signing_secret, ip_allowlist::text[] AS ip_allowlist, \
     acting_user_id, enabled, rate_per_minute, daily_cap, last_used_at, host(last_used_ip) AS last_used_ip, \
     created_by, created_at, updated_at";

/// A source as the management API shows it: no token hash, no signing secret.
#[derive(Debug, Clone, FromQueryResult, Serialize)]
pub struct SourceView {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub format: String,
    pub token_prefix: String,
    pub require_signature: bool,
    pub has_signing_secret: bool,
    /// `address/prefix` blocks the source may post from; empty admits every address.
    pub ip_allowlist: Vec<String>,
    pub acting_user_id: Option<Uuid>,
    pub enabled: bool,
    pub rate_per_minute: i32,
    pub daily_cap: i32,
    pub last_used_at: Option<Timestamp>,
    pub last_used_ip: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl SourceView {
    /// The settings an audit event records: everything an administrator can change.
    fn audit_settings(&self) -> Value {
        json!({
            "name": self.name,
            "enabled": self.enabled,
            "require_signature": self.require_signature,
            "rate_per_minute": self.rate_per_minute,
            "daily_cap": self.daily_cap,
            "format": self.format,
            "ip_allowlist": self.ip_allowlist,
            "acting_user_id": self.acting_user_id,
            "token_prefix": self.token_prefix,
        })
    }
}

async fn load_source<C: ConnectionTrait>(db: &C, id: Uuid) -> Result<SourceView, ApiError> {
    SourceView::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {SOURCE_COLUMNS} FROM inbound_sources WHERE id = $1"),
        vec![id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("inbound source not found".to_owned()))
}

/// Loads a source and checks that the caller administers its workspace.
async fn admin_source(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    id: Uuid,
) -> Result<(SourceView, Uuid), ApiError> {
    let source = load_source(&state.db, id).await?;
    let user = require_workspace_admin(state, claims, bot, source.workspace_id).await?;
    Ok((source, user))
}

fn valid_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(ApiError::BadRequest("name must be 1-100 characters".to_owned()));
    }
    Ok(name.to_owned())
}

fn valid_rates(rate_per_minute: Option<i32>, daily_cap: Option<i32>) -> Result<(), ApiError> {
    if rate_per_minute.is_some_and(|rate| !(1..=6_000).contains(&rate)) {
        return Err(ApiError::BadRequest(
            "rate_per_minute must be between 1 and 6000".to_owned(),
        ));
    }
    if daily_cap.is_some_and(|cap| !(1..=1_000_000).contains(&cap)) {
        return Err(ApiError::BadRequest(
            "daily_cap must be between 1 and 1000000".to_owned(),
        ));
    }
    Ok(())
}

/// The allow-list as it is stored, or the 400 that names the entry that is wrong.
fn valid_allowlist(entries: &[String]) -> Result<Vec<String>, ApiError> {
    allowlist::normalise_entries(entries).map_err(ApiError::BadRequest)
}

fn mint_failure() -> ApiError {
    tracing::error!("the operating system random source is unavailable");
    ApiError::Internal
}

pub async fn list_sources(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = project_workspace(&state, project_id).await?;
    require_workspace_admin(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    let sources = SourceView::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {SOURCE_COLUMNS} FROM inbound_sources WHERE project_id = $1 ORDER BY created_at, id"),
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(sources))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateSourceRequest {
    pub name: String,
    /// Only `native_v1` exists.
    pub format: Option<String>,
    pub require_signature: Option<bool>,
    pub rate_per_minute: Option<i32>,
    pub daily_cap: Option<i32>,
    pub ip_allowlist: Option<Vec<String>>,
}

#[derive(Serialize)]
struct SecretsOnce {
    source: SourceView,
    /// Shown once: only its hash is stored.
    token: String,
    /// Shown once: the key to sign requests with.
    signing_secret: String,
}

pub async fn create_source(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(request): Json<CreateSourceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = project_workspace(&state, project_id).await?;
    let user = require_workspace_admin(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    let name = valid_name(&request.name)?;
    if request.format.as_deref().unwrap_or(FORMAT_NATIVE) != FORMAT_NATIVE {
        return Err(ApiError::BadRequest("format must be native_v1".to_owned()));
    }
    valid_rates(request.rate_per_minute, request.daily_cap)?;
    let allowlist = valid_allowlist(&request.ip_allowlist.unwrap_or_default())?;

    let minted = credentials::mint_token().map_err(|_| mint_failure())?;
    let signing_secret = credentials::mint_signing_secret().map_err(|_| mint_failure())?;
    let id = Uuid::new_v4();
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO inbound_sources (id, workspace_id, project_id, name, format, token_hash, \
         token_prefix, signing_secret, require_signature, acting_user_id, rate_per_minute, daily_cap, \
         ip_allowlist, created_by) \
         VALUES ($1, $2, $3, $4, 'native_v1', $5, $6, $7, $8, $9, COALESCE($10, 120), COALESCE($11, 20000), \
                 $12::text[]::cidr[], $9)",
        vec![
            id.into(),
            workspace_id.into(),
            project_id.into(),
            name.into(),
            minted.hash.into(),
            minted.display_prefix.into(),
            signing_secret.clone().into(),
            request.require_signature.unwrap_or(true).into(),
            user.into(),
            request.rate_per_minute.into(),
            request.daily_cap.into(),
            allowlist.into(),
        ],
    ))
    .await?;
    let source = load_source(&tx, id).await?;
    audit::record(
        &tx,
        workspace_id,
        project_id,
        "inbound_source.created",
        "inbound_source",
        id,
        Some(user),
        actor_source(user),
        json!({ "source_id": id, "new": source.audit_settings() }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(SecretsOnce {
        source,
        token: minted.raw,
        signing_secret,
    }))
}

pub async fn get_source(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let (source, _) = admin_source(&state, &claims, bot.as_ref().map(|b| &b.0), id).await?;
    Ok(ApiResponse::success(source))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchSourceRequest {
    pub name: Option<String>,
    pub enabled: Option<bool>,
    pub require_signature: Option<bool>,
    pub rate_per_minute: Option<i32>,
    pub daily_cap: Option<i32>,
    /// Replaces the allow-list; an empty list admits every address.
    pub ip_allowlist: Option<Vec<String>>,
    /// Makes the caller the person the source acts for (D7).
    pub take_over: Option<bool>,
}

pub async fn patch_source(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    Json(request): Json<PatchSourceRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let (before, user) = admin_source(&state, &claims, bot.as_ref().map(|b| &b.0), id).await?;
    let name = request.name.as_deref().map(valid_name).transpose()?;
    valid_rates(request.rate_per_minute, request.daily_cap)?;
    let allowlist = request.ip_allowlist.as_deref().map(valid_allowlist).transpose()?;
    let take_over = request.take_over.unwrap_or(false);
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_sources SET name = COALESCE($2, name), enabled = COALESCE($3, enabled), \
                require_signature = COALESCE($4, require_signature), rate_per_minute = COALESCE($5, rate_per_minute), \
                daily_cap = COALESCE($6, daily_cap), ip_allowlist = COALESCE($7::text[]::cidr[], ip_allowlist), \
                acting_user_id = CASE WHEN $8 THEN $9 ELSE acting_user_id END, updated_at = now() \
          WHERE id = $1",
        vec![
            id.into(),
            name.into(),
            request.enabled.into(),
            request.require_signature.into(),
            request.rate_per_minute.into(),
            request.daily_cap.into(),
            allowlist.into(),
            take_over.into(),
            user.into(),
        ],
    ))
    .await?;
    let after = load_source(&tx, id).await?;
    audit::record(
        &tx,
        after.workspace_id,
        after.project_id,
        "inbound_source.updated",
        "inbound_source",
        id,
        Some(user),
        actor_source(user),
        json!({ "source_id": id, "old": before.audit_settings(), "new": after.audit_settings(), "take_over": take_over }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(after))
}

pub async fn rotate_source(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let (before, user) = admin_source(&state, &claims, bot.as_ref().map(|b| &b.0), id).await?;
    let minted = credentials::mint_token().map_err(|_| mint_failure())?;
    let signing_secret = credentials::mint_signing_secret().map_err(|_| mint_failure())?;
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE inbound_sources SET token_hash = $2, token_prefix = $3, signing_secret = $4, updated_at = now() \
          WHERE id = $1",
        vec![
            id.into(),
            minted.hash.into(),
            minted.display_prefix.clone().into(),
            signing_secret.clone().into(),
        ],
    ))
    .await?;
    let source = load_source(&tx, id).await?;
    audit::record(
        &tx,
        source.workspace_id,
        source.project_id,
        "inbound_source.rotated",
        "inbound_source",
        id,
        Some(user),
        actor_source(user),
        json!({ "source_id": id, "old_token_prefix": before.token_prefix, "new_token_prefix": minted.display_prefix }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(SecretsOnce {
        source,
        token: minted.raw,
        signing_secret,
    }))
}

pub async fn delete_source(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let (source, user) = admin_source(&state, &claims, bot.as_ref().map(|b| &b.0), id).await?;
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM inbound_sources WHERE id = $1",
        vec![id.into()],
    ))
    .await?;
    audit::record(
        &tx,
        source.workspace_id,
        source.project_id,
        "inbound_source.deleted",
        "inbound_source",
        id,
        Some(user),
        actor_source(user),
        json!({ "source_id": id, "old": source.audit_settings() }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(json!({ "deleted": true })))
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    pub before: Option<String>,
    pub limit: Option<i64>,
}

/// A stored event as an administrator sees it, payload included.
#[derive(Debug, FromQueryResult, Serialize)]
struct EventView {
    id: Uuid,
    event_id: String,
    kind: String,
    severity_rank: i16,
    dedup_key: String,
    occurred_at: Timestamp,
    received_at: Timestamp,
    title: String,
    summary: String,
    subject: Value,
    payload: Value,
    signed: bool,
    client_ip: Option<String>,
    outcome: String,
    rule_id: Option<Uuid>,
    alert_id: Option<Uuid>,
    task_id: Option<Uuid>,
}

pub async fn list_source_events(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    Query(query): Query<EventsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    admin_source(&state, &claims, bot.as_ref().map(|b| &b.0), id).await?;
    let before = query
        .before
        .as_deref()
        .map(Timestamp::parse_rfc3339)
        .transpose()
        .map_err(|_| ApiError::BadRequest("before must be an RFC 3339 timestamp".to_owned()))?;
    let limit = query.limit.unwrap_or(EVENTS_DEFAULT_LIMIT).clamp(1, EVENTS_MAX_LIMIT);
    let events = EventView::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, event_id, kind, severity_rank, dedup_key, occurred_at, received_at, title, summary, subject, \
                payload, signed, host(client_ip) AS client_ip, outcome, rule_id, alert_id, task_id \
           FROM inbound_events WHERE source_id = $1 AND ($2::timestamptz IS NULL OR received_at < $2) \
          ORDER BY received_at DESC, id DESC LIMIT $3",
        vec![id.into(), before.into(), limit.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(events))
}

// ---------------------------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------------------------

#[derive(FromQueryResult)]
struct Found {
    id: Uuid,
}

async fn load_rule<C: ConnectionTrait>(db: &C, id: Uuid) -> Result<RuleRow, ApiError> {
    RuleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {RULE_COLUMNS} FROM alert_rules WHERE id = $1"),
        vec![id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("alert rule not found".to_owned()))
}

async fn admin_rule(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<&BotAuthContext>,
    id: Uuid,
) -> Result<(RuleRow, Uuid), ApiError> {
    let rule = load_rule(&state.db, id).await?;
    let user = require_workspace_admin(state, claims, bot, rule.workspace_id).await?;
    Ok((rule, user))
}

/// Checks what a rule refers to against the database and returns the warnings to show the author.
async fn check_rule_references<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    project_id: Uuid,
    rule: &ValidatedRule,
) -> Result<Vec<&'static str>, ApiError> {
    if let Some(source_id) = rule.source_id {
        let bound = Found::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM inbound_sources WHERE id = $1 AND project_id = $2 AND workspace_id = $3",
            vec![source_id.into(), project_id.into(), workspace_id.into()],
        ))
        .one(db)
        .await?;
        if bound.is_none() {
            return Err(ApiError::BadRequest(
                "source_id must be a source of this project".to_owned(),
            ));
        }
    }
    let mut warnings = Vec::new();
    if let Some(bot) = rule.bot_user_id {
        let valid = Found::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT u.id FROM users u JOIN workspace_members m ON m.user_id = u.id AND m.workspace_id = $2 \
              WHERE u.id = $1 AND u.entity_type = 'bot' AND u.is_active",
            vec![bot.into(), workspace_id.into()],
        ))
        .one(db)
        .await?;
        if valid.is_none() {
            return Err(ApiError::BadRequest(
                "bot_user_id must be a bot that is a member of this workspace".to_owned(),
            ));
        }
        let connected = Found::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT c.project_id AS id FROM project_bot_connections c \
              WHERE c.project_id = $1 AND c.bot_id = $2 AND c.enabled",
            vec![project_id.into(), bot.into()],
        ))
        .one(db)
        .await?;
        if connected.is_none() {
            // Not refused: the bot may be connected later, but a task for a bot that is not
            // connected to this project cannot reach a runner that will run it.
            warnings.push("bot_not_connected_to_project");
        }
    }
    if !rule.label_ids.is_empty() {
        let known = Found::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM labels WHERE id = ANY($1) AND workspace_id = $2",
            vec![rule.label_ids.clone().into(), workspace_id.into()],
        ))
        .all(db)
        .await?;
        if known.len() != rule.label_ids.len() {
            return Err(ApiError::BadRequest(
                "label_ids must be labels of this workspace".to_owned(),
            ));
        }
    }
    let members = notify::current_human_members(db, workspace_id, &rule.notify_user_ids).await?;
    if members.len() != rule.notify_user_ids.len() {
        return Err(ApiError::BadRequest(
            "notify_user_ids must be people who are members of this workspace".to_owned(),
        ));
    }
    Ok(warnings)
}

fn validated(input: RuleInput, caller: Uuid) -> Result<ValidatedRule, ApiError> {
    input.validate(caller).map_err(ApiError::BadRequest)
}

#[derive(Serialize)]
struct RuleWithWarnings {
    rule: RuleRow,
    warnings: Vec<&'static str>,
}

pub async fn list_rules(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = project_workspace(&state, project_id).await?;
    require_workspace_admin(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    let rules = RuleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {RULE_COLUMNS} FROM alert_rules WHERE project_id = $1 ORDER BY position, id"),
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(rules))
}

pub async fn create_rule(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(input): Json<RuleInput>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = project_workspace(&state, project_id).await?;
    let user = require_workspace_admin(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    if input.expected_version.is_some() {
        return Err(ApiError::BadRequest(
            "expected_version is only valid when replacing a rule".to_owned(),
        ));
    }
    let rule = validated(input, user)?;
    let tx = state.db.begin().await?;
    let warnings = check_rule_references(&tx, workspace_id, project_id, &rule).await?;
    let id = Uuid::new_v4();
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO alert_rules (id, workspace_id, project_id, name, position, enabled, source_id, kind_patterns, \
         min_severity, action, bot_user_id, instructions, priority_map, label_ids, notify_user_ids, \
         max_tasks_per_hour, max_active_tasks, dedup_window_secs, rearm_after_secs, task_max_attempts, \
         created_by, updated_by) \
         VALUES ($1, $2, $3, $4, (SELECT COALESCE(MAX(position), 0) + 10 FROM alert_rules WHERE project_id = $3), \
                 $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $20)",
        rule_values(id, workspace_id, project_id, &rule, user),
    ))
    .await?;
    let created = load_rule(&tx, id).await?;
    audit::record(
        &tx,
        workspace_id,
        project_id,
        "alert_rule.created",
        "alert_rule",
        id,
        Some(user),
        actor_source(user),
        json!({ "rule_id": id, "new": created }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(RuleWithWarnings {
        rule: created,
        warnings,
    }))
}

/// The bind values shared by the create statement, in the order of its placeholders. The replace
/// statement uses the same order after its own first two.
fn rule_values(
    id: Uuid,
    workspace_id: Uuid,
    project_id: Uuid,
    rule: &ValidatedRule,
    user: Uuid,
) -> Vec<sea_orm::Value> {
    vec![
        id.into(),
        workspace_id.into(),
        project_id.into(),
        rule.name.clone().into(),
        rule.enabled.into(),
        rule.source_id.into(),
        rule.kind_patterns.clone().into(),
        rule.min_severity.as_str().into(),
        rule.action.into(),
        rule.bot_user_id.into(),
        rule.instructions.clone().into(),
        rule.priority_map.clone().into(),
        rule.label_ids.clone().into(),
        rule.notify_user_ids.clone().into(),
        rule.max_tasks_per_hour.into(),
        rule.max_active_tasks.into(),
        rule.dedup_window_secs.into(),
        rule.rearm_after_secs.into(),
        rule.task_max_attempts.into(),
        user.into(),
    ]
}

pub async fn get_rule(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let (rule, _) = admin_rule(&state, &claims, bot.as_ref().map(|b| &b.0), id).await?;
    Ok(ApiResponse::success(rule))
}

pub async fn replace_rule(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
    Json(input): Json<RuleInput>,
) -> Result<impl IntoResponse, ApiError> {
    let (before, user) = admin_rule(&state, &claims, bot.as_ref().map(|b| &b.0), id).await?;
    let expected = input
        .expected_version
        .ok_or_else(|| ApiError::BadRequest("expected_version is required to replace a rule".to_owned()))?;
    let rule = validated(input, user)?;
    let tx = state.db.begin().await?;
    let warnings = check_rule_references(&tx, before.workspace_id, before.project_id, &rule).await?;
    let updated = Found::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE alert_rules SET name = $4, enabled = $5, source_id = $6, kind_patterns = $7, min_severity = $8, \
                action = $9, bot_user_id = $10, instructions = $11, priority_map = $12, label_ids = $13, \
                notify_user_ids = $14, max_tasks_per_hour = $15, max_active_tasks = $16, dedup_window_secs = $17, \
                rearm_after_secs = $18, task_max_attempts = $19, updated_by = $20, version = version + 1, \
                updated_at = now() \
          WHERE id = $1 AND version = $21 AND workspace_id = $2 AND project_id = $3 RETURNING id",
        {
            let mut values = rule_values(id, before.workspace_id, before.project_id, &rule, user);
            values.push(expected.into());
            values
        },
    ))
    .one(&tx)
    .await?;
    if updated.is_none() {
        return Err(ApiError::VersionConflict("alert_rule"));
    }
    let after = load_rule(&tx, id).await?;
    audit::record(
        &tx,
        after.workspace_id,
        after.project_id,
        "alert_rule.updated",
        "alert_rule",
        id,
        Some(user),
        actor_source(user),
        json!({ "rule_id": id, "old": before, "new": after }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(RuleWithWarnings { rule: after, warnings }))
}

pub async fn delete_rule(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let (rule, user) = admin_rule(&state, &claims, bot.as_ref().map(|b| &b.0), id).await?;
    let tx = state.db.begin().await?;
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM alert_rules WHERE id = $1",
        vec![id.into()],
    ))
    .await?;
    audit::record(
        &tx,
        rule.workspace_id,
        rule.project_id,
        "alert_rule.deleted",
        "alert_rule",
        id,
        Some(user),
        actor_source(user),
        json!({ "rule_id": id, "old": rule }),
    )
    .await?;
    tx.commit().await?;
    Ok(ApiResponse::success(json!({ "deleted": true })))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReorderRequest {
    pub rule_ids: Vec<Uuid>,
}

pub async fn reorder_rules(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(request): Json<ReorderRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = project_workspace(&state, project_id).await?;
    let user = require_workspace_admin(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    let tx = state.db.begin().await?;
    let current: Vec<Uuid> = Found::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM alert_rules WHERE project_id = $1 ORDER BY position, id FOR UPDATE",
        vec![project_id.into()],
    ))
    .all(&tx)
    .await?
    .into_iter()
    .map(|row| row.id)
    .collect();
    let mut wanted = request.rule_ids.clone();
    wanted.sort_unstable();
    wanted.dedup();
    let mut have = current.clone();
    have.sort_unstable();
    if wanted.len() != request.rule_ids.len() || wanted != have {
        return Err(ApiError::BadRequest(
            "rule_ids must list every rule of the project exactly once".to_owned(),
        ));
    }
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE alert_rules r SET position = ordered.pos * 10, updated_at = now() \
           FROM unnest($1::uuid[]) WITH ORDINALITY AS ordered(id, pos) WHERE r.id = ordered.id",
        vec![request.rule_ids.clone().into()],
    ))
    .await?;
    audit::record(
        &tx,
        workspace_id,
        project_id,
        "alert_rule.reordered",
        "alert_rule",
        project_id,
        Some(user),
        actor_source(user),
        json!({ "project_id": project_id, "old_order": current, "new_order": request.rule_ids }),
    )
    .await?;
    tx.commit().await?;
    let rules = RuleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT {RULE_COLUMNS} FROM alert_rules WHERE project_id = $1 ORDER BY position, id"),
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;
    Ok(ApiResponse::success(rules))
}

// ---------------------------------------------------------------------------------------------
// Rule test
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestRulesRequest {
    /// The source of the project the sample event is posted as.
    pub source_id: Uuid,
    /// A sample envelope, validated like a real one. Exactly one of `envelope` and
    /// `inbound_event_id` is given.
    pub envelope: Option<Value>,
    /// A stored event of that source, tested again against the rules as they are now.
    pub inbound_event_id: Option<Uuid>,
}

/// POST `/api/v1/projects/{project_id}/alert-rules/test`: which rule would handle the event and
/// what would happen. Creates nothing and counts against no limit.
pub async fn test_rules(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Json(request): Json<TestRulesRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let workspace_id = project_workspace(&state, project_id).await?;
    require_workspace_admin(&state, &claims, bot.as_ref().map(|b| &b.0), workspace_id).await?;
    let source = Found::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM inbound_sources WHERE id = $1 AND project_id = $2 AND workspace_id = $3",
        vec![request.source_id.into(), project_id.into(), workspace_id.into()],
    ))
    .one(&state.db)
    .await?;
    if source.is_none() {
        return Err(ApiError::BadRequest(
            "source_id must be a source of this project".to_owned(),
        ));
    }
    let name = SourceName::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT name FROM inbound_sources WHERE id = $1",
        vec![request.source_id.into()],
    ))
    .one(&state.db)
    .await?
    .map(|row| row.name)
    .ok_or_else(|| ApiError::NotFound("inbound source not found".to_owned()))?;

    let facts = match (request.envelope, request.inbound_event_id) {
        (Some(envelope), None) => {
            let bytes = serde_json::to_vec(&envelope)
                .map_err(|_| ApiError::BadRequest("envelope must be a JSON object".to_owned()))?;
            let parsed = parse_envelope(&bytes, Timestamp::now()).map_err(|error| match error {
                EnvelopeError::Invalid { path, reason } => ApiError::BadRequest(format!("envelope {path}: {reason}")),
                EnvelopeError::OccurredAtOutOfRange => {
                    ApiError::BadRequest("envelope occurred_at is out of range".to_owned())
                }
            })?;
            EventFacts {
                kind: parsed.kind,
                severity: parsed.severity,
                dedup_key: parsed.dedup_key,
                subject: parsed.subject,
            }
        }
        (None, Some(event_id)) => dry_run::stored_event(&state.db, project_id, request.source_id, event_id)
            .await?
            .ok_or_else(|| ApiError::NotFound("inbound event not found".to_owned()))?,
        _ => {
            return Err(ApiError::BadRequest(
                "give exactly one of envelope and inbound_event_id".to_owned(),
            ));
        }
    };
    let workflow = resolve_effective_workflow_for_project(&state, project_id).await?;
    let config = crate::config::runtime().inbound;
    let report = dry_run::evaluate(
        &state.db,
        &workflow,
        &SourceFacts {
            id: request.source_id,
            workspace_id,
            project_id,
            name: &name,
        },
        &facts,
        Limits {
            workspace_active_tasks: config.max_active_alert_tasks_per_workspace,
            alerts_per_rule_per_hour: config.max_alerts_per_rule_per_hour,
        },
    )
    .await?;
    Ok(ApiResponse::success(report))
}

#[derive(FromQueryResult)]
struct SourceName {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_rates_and_allowlist_are_bounded() {
        assert!(valid_name("  prx-waf edge-01 ").is_ok_and(|name| name == "prx-waf edge-01"));
        assert!(valid_name("   ").is_err());
        assert!(valid_name(&"n".repeat(101)).is_err());
        assert!(valid_rates(Some(1), Some(1)).is_ok());
        assert!(valid_rates(Some(6_000), Some(1_000_000)).is_ok());
        assert!(valid_rates(Some(0), None).is_err());
        assert!(valid_rates(Some(6_001), None).is_err());
        assert!(valid_rates(None, Some(0)).is_err());
        assert!(valid_rates(None, Some(1_000_001)).is_err());
        assert!(valid_allowlist(&[]).is_ok_and(|list| list.is_empty()));
        assert!(valid_allowlist(&["192.0.2.0/24".to_owned()]).is_ok());
        assert!(valid_allowlist(&["192.0.2.1/24".to_owned()]).is_err());
    }
}
