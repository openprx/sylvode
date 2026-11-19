use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::entities::webhook::{CreateWebhookRequest, UpdateWebhookRequest, WEBHOOK_EVENTS};
use crate::{
    error::ApiError,
    events::{BusinessEventInput, dispatcher::ensure_workspace_subscriber_slot, insert_business_event},
    outbound::validate_outbound_url,
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, FromQueryResult)]
struct WebhookRow {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: Option<String>,
    pub url: String,
    pub events: serde_json::Value,
    pub active: bool,
    pub bot_user_id: Option<Uuid>,
    pub created_by: Uuid,
    pub created_at: platform::timestamp::Timestamp,
    pub updated_at: platform::timestamp::Timestamp,
    pub last_triggered_at: Option<platform::timestamp::Timestamp>,
}

#[derive(Debug, Serialize)]
pub struct WebhookResponse {
    pub id: Uuid,
    pub workspace_id: Uuid,
    pub name: Option<String>,
    pub url: String,
    pub events: Vec<String>,
    pub active: bool,
    pub bot_user_id: Option<Uuid>,
    pub created_by: Uuid,
    pub created_at: platform::timestamp::Timestamp,
    pub updated_at: platform::timestamp::Timestamp,
    pub last_triggered_at: Option<platform::timestamp::Timestamp>,
}

/// The answer to creating a webhook or rotating its secret: the webhook plus its signing secret.
///
/// This is the only place the secret ever leaves the API, once, so the receiver can be configured
/// with it (including one the server generated). [`WebhookResponse`], which list, get and update
/// answer with and which is copied into the webhook's business events, has no secret field.
#[derive(Debug, Serialize)]
pub struct WebhookWithSecretResponse {
    #[serde(flatten)]
    pub webhook: WebhookResponse,
    /// HMAC-SHA256 key of `X-Webhook-Signature`; not retrievable afterwards.
    pub secret: String,
}

#[derive(Debug, Deserialize)]
pub struct ListDeliveriesQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
}

#[derive(Debug, Serialize, FromQueryResult)]
pub struct WebhookDeliveryResponse {
    pub id: Uuid,
    pub webhook_id: Uuid,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub request_headers: Option<serde_json::Value>,
    pub response_status: Option<i32>,
    pub response_body: Option<String>,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    pub success: bool,
    pub delivered_at: Option<platform::timestamp::Timestamp>,
    pub created_at: platform::timestamp::Timestamp,
}

impl TryFrom<WebhookRow> for WebhookResponse {
    type Error = ApiError;

    fn try_from(row: WebhookRow) -> Result<Self, Self::Error> {
        let events: Vec<String> = serde_json::from_value(row.events).map_err(|_| ApiError::Internal)?;
        Ok(Self {
            id: row.id,
            workspace_id: row.workspace_id,
            name: row.name,
            url: row.url,
            events,
            active: row.active,
            bot_user_id: row.bot_user_id,
            created_by: row.created_by,
            created_at: row.created_at,
            updated_at: row.updated_at,
            last_triggered_at: row.last_triggered_at,
        })
    }
}

/// GET /`api/v1/workspaces/:workspace_id/webhooks/:webhook_id/deliveries`
pub async fn list_deliveries(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
    Query(query): Query<ListDeliveriesQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_access(&state, workspace_id, user_id).await?;
    ensure_webhook_in_workspace(&state, workspace_id, webhook_id).await?;

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.per_page.unwrap_or(20).clamp(1, 100);
    let offset = (page - 1) * per_page;

    let count_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT COUNT(*) AS count FROM webhook_deliveries WHERE webhook_id = $1",
            vec![webhook_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let total: i64 = count_result.try_get("", "count")?;
    let total_pages = if total == 0 {
        0
    } else {
        total / i64::from(per_page) + i64::from(total % i64::from(per_page) != 0)
    };

    let rows = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT
                    id,
                    webhook_id,
                    event_type,
                    payload,
                    request_headers,
                    response_status,
                    response_body,
                    error,
                    CASE
                        WHEN delivered_at IS NULL THEN NULL
                        ELSE (EXTRACT(EPOCH FROM (delivered_at - created_at)) * 1000)::BIGINT
                    END AS duration_ms,
                    CASE
                        WHEN response_status BETWEEN 200 AND 299 THEN true
                        ELSE false
                    END AS success,
                    delivered_at,
                    created_at
                FROM webhook_deliveries
                WHERE webhook_id = $1
                ORDER BY created_at DESC
                LIMIT $2 OFFSET $3
            ",
            vec![webhook_id.into(), i64::from(per_page).into(), i64::from(offset).into()],
        ))
        .await?;

    let items: Vec<WebhookDeliveryResponse> = rows
        .iter()
        .map(|r| WebhookDeliveryResponse::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ApiResponse::success(PaginatedData {
        items,
        total,
        page: i64::from(page),
        per_page: i64::from(per_page),
        total_pages,
    }))
}

/// GET /`api/v1/workspaces/:workspace_id/webhooks/:webhook_id/deliveries/:delivery_id`
pub async fn get_delivery(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id, delivery_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_access(&state, workspace_id, user_id).await?;
    ensure_webhook_in_workspace(&state, workspace_id, webhook_id).await?;

    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT
                    id,
                    webhook_id,
                    event_type,
                    payload,
                    request_headers,
                    response_status,
                    response_body,
                    error,
                    CASE
                        WHEN delivered_at IS NULL THEN NULL
                        ELSE (EXTRACT(EPOCH FROM (delivered_at - created_at)) * 1000)::BIGINT
                    END AS duration_ms,
                    CASE
                        WHEN response_status BETWEEN 200 AND 299 THEN true
                        ELSE false
                    END AS success,
                    delivered_at,
                    created_at
                FROM webhook_deliveries
                WHERE webhook_id = $1 AND id = $2
            ",
            vec![webhook_id.into(), delivery_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::NotFound("Webhook delivery not found".to_string()))?;

    let delivery = WebhookDeliveryResponse::from_query_result(&row, "")?;
    Ok(ApiResponse::success(delivery))
}

/// POST /`api/v1/workspaces/:workspace_id/webhooks`
pub async fn create_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<CreateWebhookRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let name = req.name.clone().unwrap_or_else(|| req.url.clone());
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    // Creating an outbound webhook is an infrastructure change: it must match the privilege
    // required by update/delete in this module.
    verify_workspace_admin(&state, workspace_id, user_id).await?;

    validate_outbound_url(&req.url)
        .await
        .map_err(|err| ApiError::BadRequest(format!("invalid url: {err}")))?;

    // Validate events
    for event in &req.events {
        if !WEBHOOK_EVENTS.contains(&event.as_str()) {
            return Err(ApiError::BadRequest(format!("Invalid event type: {event}")));
        }
    }

    if let Some(bot_user_id) = req.bot_user_id {
        ensure_bot_user(&state, bot_user_id).await?;
    }

    // Prefer explicit secret, otherwise generate one. Either way it is returned in this response
    // only, so a generated one can be handed to the receiver.
    let secret = req.secret.unwrap_or_else(generate_secret);

    let webhook_id = Uuid::new_v4();
    let now = platform::timestamp::Timestamp::now();
    let active = req.enabled.or(req.active).unwrap_or(true);

    let tx = state.db.begin().await?;

    // `subscribers_per_workspace_max` (`limits-v1.md`, `limit_kind: workspace_subscribers`, frozen
    // at 100): a webhook is v0.4's only subscriber kind, so this endpoint is where a workspace's
    // subscriber directory grows. Checked inside the transaction that inserts the row, and only
    // for a row that would actually be active — an inactive webhook is not a subscriber (the
    // dispatcher's own directory query and `webhook_trigger.rs` both require `active = true`).
    if active {
        ensure_workspace_subscriber_slot(&tx, workspace_id, None).await?;
    }

    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO webhooks
               (id, workspace_id, name, url, secret, events, active, bot_user_id, created_by, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        vec![
            webhook_id.into(),
            workspace_id.into(),
            name.into(),
            req.url.into(),
            secret.clone().into(),
            serde_json::to_value(&req.events)
                .map_err(|_| ApiError::Internal)?
                .into(),
            active.into(),
            req.bot_user_id.into(),
            user_id.into(),
            now.into(),
            now.into(),
        ],
    ))
    .await?;

    let response = find_webhook_response_with_conn(&tx, webhook_id).await?;
    insert_webhook_event(&tx, &response, "webhook.created", user_id).await?;
    tx.commit().await?;

    Ok(ApiResponse::success(WebhookWithSecretResponse {
        webhook: response,
        secret,
    }))
}

/// POST /`api/v1/workspaces/:workspace_id/webhooks/:webhook_id/rotate-secret`
///
/// Replaces the signing secret with a newly generated one and returns it, once. Deliveries signed
/// from now on use the new secret, so the receiver has to be given it (`sylvode-webhook` accepts
/// several `webhook_secrets` at once, which allows a rotation without dropping deliveries).
pub async fn rotate_webhook_secret(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_admin(&state, workspace_id, user_id).await?;

    let secret = generate_secret();
    let tx = state.db.begin().await?;
    let result = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE webhooks SET secret = $1, updated_at = $2 WHERE id = $3 AND workspace_id = $4",
            vec![
                secret.clone().into(),
                platform::timestamp::Timestamp::now().into(),
                webhook_id.into(),
                workspace_id.into(),
            ],
        ))
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Webhook not found".to_string()));
    }

    let response = find_webhook_response_with_conn(&tx, webhook_id).await?;
    insert_webhook_event(&tx, &response, "webhook.secret_rotated", user_id).await?;
    tx.commit().await?;

    Ok(ApiResponse::success(WebhookWithSecretResponse {
        webhook: response,
        secret,
    }))
}

/// GET /`api/v1/workspaces/:workspace_id/webhooks`
pub async fn list_webhooks(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_access(&state, workspace_id, user_id).await?;

    let webhooks = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, name, url, events, active, bot_user_id, created_by, created_at, updated_at, last_triggered_at FROM webhooks WHERE workspace_id = $1 ORDER BY created_at DESC",
            vec![workspace_id.into()],
        ))
        .await?;

    let responses: Vec<WebhookResponse> = webhooks
        .iter()
        .map(|r| {
            let row = WebhookRow::from_query_result(r, "")?;
            WebhookResponse::try_from(row)
        })
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ApiResponse::success(PaginatedData::from_items(responses)))
}

/// GET /`api/v1/workspaces/:workspace_id/webhooks/:webhook_id`
pub async fn get_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_access(&state, workspace_id, user_id).await?;

    let webhook = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, name, url, events, active, bot_user_id, created_by, created_at, updated_at, last_triggered_at FROM webhooks WHERE id = $1 AND workspace_id = $2",
            vec![webhook_id.into(), workspace_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::NotFound("Webhook not found".to_string()))?;

    let response = WebhookResponse::try_from(WebhookRow::from_query_result(&webhook, "")?)?;

    Ok(ApiResponse::success(response))
}

/// PATCH /`api/v1/workspaces/:workspace_id/webhooks/:webhook_id`
pub async fn update_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateWebhookRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_admin(&state, workspace_id, user_id).await?;

    // Validate events if provided
    if let Some(ref events) = req.events {
        for event in events {
            if !WEBHOOK_EVENTS.contains(&event.as_str()) {
                return Err(ApiError::BadRequest(format!("Invalid event type: {event}")));
            }
        }
    }
    if let Some(bot_user_id) = req.bot_user_id {
        ensure_bot_user(&state, bot_user_id).await?;
    }
    if let Some(url) = req.url.as_deref() {
        validate_outbound_url(url)
            .await
            .map_err(|err| ApiError::BadRequest(format!("invalid url: {err}")))?;
    }

    // Build dynamic UPDATE query
    let mut updates = Vec::new();
    let mut params: Vec<sea_orm::Value> = Vec::new();
    let mut param_idx = 1;

    if let Some(name) = req.name {
        updates.push(format!("name = ${param_idx}"));
        params.push(name.into());
        param_idx += 1;
    }
    if let Some(url) = req.url {
        updates.push(format!("url = ${param_idx}"));
        params.push(url.into());
        param_idx += 1;
    }
    if let Some(secret) = req.secret {
        updates.push(format!("secret = ${param_idx}"));
        params.push(secret.into());
        param_idx += 1;
    }
    if let Some(events) = req.events {
        updates.push(format!("events = ${param_idx}"));
        params.push(serde_json::to_value(&events).map_err(|_| ApiError::Internal)?.into());
        param_idx += 1;
    }
    let activating = req.enabled.or(req.active).unwrap_or(false);
    if let Some(active) = req.enabled.or(req.active) {
        updates.push(format!("active = ${param_idx}"));
        params.push(active.into());
        param_idx += 1;
    }
    if req.bot_user_id.is_some() {
        updates.push(format!("bot_user_id = ${param_idx}"));
        params.push(req.bot_user_id.into());
        param_idx += 1;
    }

    if updates.is_empty() {
        return Err(ApiError::BadRequest("No fields to update".to_string()));
    }

    updates.push(format!("updated_at = ${param_idx}"));
    params.push(platform::timestamp::Timestamp::now().into());
    param_idx += 1;

    let webhook_id_idx = param_idx;
    params.push(webhook_id.into());
    param_idx += 1;
    let workspace_id_idx = param_idx;
    params.push(workspace_id.into());

    let sql = format!(
        "UPDATE webhooks SET {} WHERE id = ${} AND workspace_id = ${}",
        updates.join(", "),
        webhook_id_idx,
        workspace_id_idx
    );

    let tx = state.db.begin().await?;

    // `subscribers_per_workspace_max` (`limits-v1.md`, `limit_kind: workspace_subscribers`):
    // switching a webhook on grows the workspace's active subscriber directory exactly like
    // creating one, so the same ceiling applies here. The webhook being updated is excluded from
    // the tally and counted once as the slot it takes, so re-saving an already-active webhook in a
    // workspace sitting exactly at the ceiling is not rejected.
    if activating {
        ensure_workspace_subscriber_slot(&tx, workspace_id, Some(webhook_id)).await?;
    }

    let result = tx
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, &sql, params))
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Webhook not found".to_string()));
    }

    let response = find_webhook_response_with_conn(&tx, webhook_id).await?;
    insert_webhook_event(&tx, &response, "webhook.updated", user_id).await?;
    tx.commit().await?;

    Ok(ApiResponse::success(response))
}

/// DELETE /`api/v1/workspaces/:workspace_id/webhooks/:webhook_id`
pub async fn delete_webhook(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, webhook_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    verify_workspace_admin(&state, workspace_id, user_id).await?;

    let tx = state.db.begin().await?;
    let webhook = find_webhook_response_with_conn(&tx, webhook_id).await?;
    insert_webhook_event(&tx, &webhook, "webhook.deleted", user_id).await?;

    let result = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webhooks WHERE id = $1 AND workspace_id = $2",
            vec![webhook_id.into(), workspace_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Webhook not found".to_string()));
    }
    tx.commit().await?;

    Ok(ApiResponse::ok())
}

// Helper functions
async fn find_webhook_response_with_conn<C>(db: &C, webhook_id: Uuid) -> Result<WebhookResponse, ApiError>
where
    C: ConnectionTrait,
{
    let webhook = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, workspace_id, name, url, events, active, bot_user_id, created_by, created_at, updated_at, last_triggered_at FROM webhooks WHERE id = $1",
            vec![webhook_id.into()],
        ))
        .await?
        .ok_or_else(|| ApiError::NotFound("Webhook not found".to_string()))?;

    WebhookResponse::try_from(WebhookRow::from_query_result(&webhook, "")?)
}

async fn insert_webhook_event<C>(
    db: &C,
    webhook: &WebhookResponse,
    event_type: &str,
    actor_id: Uuid,
) -> Result<(), ApiError>
where
    C: ConnectionTrait,
{
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id: webhook.workspace_id,
            project_id: None,
            event_type: event_type.to_string(),
            aggregate_type: "webhook".to_string(),
            aggregate_id: webhook.id.to_string(),
            actor_id: Some(actor_id),
            source: json!({ "type": "user", "actor_id": actor_id }),
            payload: json!({
                "webhook_id": webhook.id,
                "workspace_id": webhook.workspace_id,
                "name": webhook.name,
                "url": webhook.url,
                "events": webhook.events,
                "active": webhook.active,
                "bot_user_id": webhook.bot_user_id
            }),
            metadata: json!({
                "webhook_id": webhook.id,
                "legacy_webhook": true
            }),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

async fn verify_workspace_admin(state: &AppState, workspace_id: Uuid, user_id: Uuid) -> Result<(), ApiError> {
    let result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![workspace_id.into(), user_id.into()],
        ))
        .await?;

    match result {
        Some(row) => {
            let role: String = row.try_get("", "role")?;
            let normalized_role = role.trim().to_lowercase();
            if normalized_role != "admin" && normalized_role != "owner" {
                return Err(ApiError::Forbidden("Workspace admin or owner required".to_string()));
            }
            Ok(())
        }
        None => Err(ApiError::Forbidden("Not a workspace member".to_string())),
    }
}

async fn verify_workspace_access(state: &AppState, workspace_id: Uuid, user_id: Uuid) -> Result<(), ApiError> {
    let result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![workspace_id.into(), user_id.into()],
        ))
        .await?;

    if result.is_none() {
        return Err(ApiError::Forbidden("Access denied".to_string()));
    }

    Ok(())
}

async fn ensure_bot_user(state: &AppState, bot_user_id: Uuid) -> Result<(), ApiError> {
    if !crate::services::bot_identity::is_bot_user(&state.db, bot_user_id).await? {
        return Err(ApiError::BadRequest(
            "bot_user_id must reference a bot user".to_string(),
        ));
    }
    Ok(())
}

async fn ensure_webhook_in_workspace(state: &AppState, workspace_id: Uuid, webhook_id: Uuid) -> Result<(), ApiError> {
    let row = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM webhooks WHERE id = $1 AND workspace_id = $2",
            vec![webhook_id.into(), workspace_id.into()],
        ))
        .await?;

    if row.is_none() {
        return Err(ApiError::NotFound("Webhook not found".to_string()));
    }

    Ok(())
}

/// Endpoint audit for webhook rows that predate outbound validation.
///
/// `create_webhook` and `update_webhook` validate the endpoint, but rows stored before that check
/// existed were never validated, and the legacy trigger path delivers to whatever URL a row holds.
/// Rewriting or deactivating those rows automatically would silently break working integrations, so
/// the audit only names them: one warning per offending webhook plus a summary, at startup.
pub fn spawn_stored_endpoint_audit(state: AppState) {
    tokio::spawn(async move {
        match audit_stored_endpoints(&state).await {
            Ok(0) => tracing::info!("stored webhook endpoints passed outbound validation"),
            Ok(rejected) => tracing::warn!(
                rejected,
                "stored webhook endpoints would be refused by outbound validation, legacy deliveries to them are unprotected"
            ),
            Err(err) => tracing::warn!(error = %err, "stored webhook endpoint audit could not run"),
        }
    });
}

/// Returns how many active webhook endpoints fail the current outbound validation.
async fn audit_stored_endpoints(state: &AppState) -> Result<usize, ApiError> {
    #[derive(FromQueryResult)]
    struct StoredEndpointRow {
        id: Uuid,
        workspace_id: Uuid,
        url: String,
    }

    let rows = StoredEndpointRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, url FROM webhooks WHERE active = true",
        vec![],
    ))
    .all(&state.db)
    .await?;

    let mut rejected = 0;
    for row in rows {
        // The URL can carry a token in its path or query, so only the host reaches the log line.
        let host = reqwest::Url::parse(row.url.trim())
            .ok()
            .and_then(|url| url.host_str().map(ToString::to_string))
            .unwrap_or_else(|| "unparseable".to_string());
        match tokio::time::timeout(STORED_ENDPOINT_AUDIT_TIMEOUT, validate_outbound_url(&row.url)).await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => {
                rejected += 1;
                tracing::warn!(
                    webhook_id = %row.id,
                    workspace_id = %row.workspace_id,
                    host = %host,
                    error = %error,
                    "stored webhook endpoint fails outbound validation"
                );
            }
            Err(_) => {
                rejected += 1;
                tracing::warn!(
                    webhook_id = %row.id,
                    workspace_id = %row.workspace_id,
                    host = %host,
                    "stored webhook endpoint could not be validated within the audit timeout"
                );
            }
        }
    }
    Ok(rejected)
}

/// Per endpoint budget for [`audit_stored_endpoints`], which resolves DNS.
const STORED_ENDPOINT_AUDIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn generate_secret() -> String {
    use rand::Rng;
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..64)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET.get(idx).copied().map_or('A', char::from)
        })
        .collect()
}

#[cfg(test)]
mod secret_exposure_tests {
    use super::{WebhookResponse, WebhookWithSecretResponse, generate_secret};
    use uuid::Uuid;

    fn webhook() -> WebhookResponse {
        let now = platform::timestamp::Timestamp::now();
        WebhookResponse {
            id: Uuid::new_v4(),
            workspace_id: Uuid::new_v4(),
            name: Some("receiver".to_string()),
            url: "https://receiver.example.com/webhook".to_string(),
            events: vec!["issue.assigned".to_string()],
            active: true,
            bot_user_id: None,
            created_by: Uuid::new_v4(),
            created_at: now,
            updated_at: now,
            last_triggered_at: None,
        }
    }

    #[test]
    fn only_the_create_and_rotate_answer_carries_the_secret() {
        let plain = serde_json::to_value(webhook()).expect("serializes");
        assert!(
            plain.get("secret").is_none(),
            "list, get and update never show the secret"
        );

        let secret = generate_secret();
        let created = serde_json::to_value(WebhookWithSecretResponse {
            webhook: webhook(),
            secret: secret.clone(),
        })
        .expect("serializes");
        assert_eq!(
            created.get("secret").and_then(serde_json::Value::as_str),
            Some(secret.as_str())
        );
        assert_eq!(
            created.get("url").and_then(serde_json::Value::as_str),
            Some("https://receiver.example.com/webhook"),
            "the webhook fields stay at the top level"
        );
    }

    #[test]
    fn generated_secrets_are_long_and_distinct() {
        let first = generate_secret();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(first, generate_secret());
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod secret_rotation_database_tests {
    use super::{get_webhook, rotate_webhook_secret};
    use crate::routes::context::tenant_fixture::{count, exec, seed_tenant};
    use crate::scratch_or_skip;
    use axum::extract::{Extension, Path, State};
    use axum::response::IntoResponse;
    use platform::auth::{JwtClaims, TokenType};
    use uuid::Uuid;

    fn state_for(db: sea_orm::DatabaseConnection) -> platform::app::AppState {
        use platform::config::{AppConfig, Secret};
        platform::app::AppState {
            cfg: AppConfig {
                app_name: "webhook-secret-test".to_string(),
                bind_addr: "127.0.0.1:0".to_string(),
                database_url: Secret::new("postgres://unused/unused"),
                jwt_secret: Secret::new("webhook-secret-test-secret"),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
        }
    }

    async fn body(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        serde_json::from_slice(&bytes).expect("json body")
    }

    #[tokio::test]
    async fn rotation_returns_the_new_secret_once_and_get_never_does() {
        let scratch = scratch_or_skip!("webhook_secret_rotation");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "ws").await;
        exec(
            db,
            "UPDATE workspace_members SET role = 'admin' WHERE workspace_id = $1 AND user_id = $2",
            vec![tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        let webhook_id = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, created_by) \
             VALUES ($1, $2, 'r', 'https://receiver.example.com/webhook', 'old-secret', '[]'::jsonb, true, $3)",
            vec![webhook_id.into(), tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        let state = state_for(db.clone());
        let claims = JwtClaims {
            sub: tenant.member_id.to_string(),
            email: format!("{}@tenant.test", tenant.member_id),
            token_type: TokenType::Access,
            iat: 0,
            exp: 0,
            sv: 0,
        };

        let rotated = rotate_webhook_secret(
            State(state.clone()),
            Extension(claims.clone()),
            Path((tenant.workspace_id, webhook_id)),
        )
        .await
        .unwrap_or_else(|err| panic!("rotate failed: {err:?}"))
        .into_response();
        let rotated = body(rotated).await;
        let secret = rotated
            .pointer("/data/secret")
            .and_then(serde_json::Value::as_str)
            .expect("the new secret is returned")
            .to_string();
        assert_ne!(secret, "old-secret");
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM webhooks WHERE id = $1 AND secret = $2",
                vec![webhook_id.into(), secret.clone().into()],
            )
            .await,
            1,
            "the returned secret is the one deliveries are signed with"
        );
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM business_events \
                 WHERE aggregate_id = $1 AND event_type = 'webhook.secret_rotated' AND payload::text NOT LIKE $2",
                vec![webhook_id.to_string().into(), format!("%{secret}%").into()],
            )
            .await,
            1,
            "the rotation is recorded without the secret"
        );

        let fetched = get_webhook(State(state), Extension(claims), Path((tenant.workspace_id, webhook_id)))
            .await
            .unwrap_or_else(|err| panic!("get failed: {err:?}"))
            .into_response();
        assert!(body(fetched).await.pointer("/data/secret").is_none());
        scratch.drop_self().await;
    }
}
