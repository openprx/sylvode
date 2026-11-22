//! Workspace webhooks: create, read, update, rotate the signing secret, delete.
//!
//! Every write takes the connection or transaction to run on and writes its business event on the
//! same connection, so the Webhooks routes and the project bot connection service run the same
//! code inside whichever transaction they own. Authorization and the outbound-URL check (which
//! resolves DNS and so must not run inside a transaction) stay with the caller.
//!
//! A webhook is *managed* when a `bot_runners` row names it in `webhook_id`. List and get answers
//! compute that with a `LEFT JOIN`; there is no column to drift. The Webhooks routes pass
//! [`ManagedPolicy::Refuse`], which turns every edit of a managed row into a 409 and refuses to
//! name a managed bot as `bot_user_id`.

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use crate::entities::webhook::{UpdateWebhookRequest, WEBHOOK_EVENTS};
use crate::error::ApiError;
use crate::events::{BusinessEventInput, dispatcher::ensure_workspace_subscriber_slot, insert_business_event};
use crate::services::bots::{self, MANAGED_BOT_USER_REFUSAL, MANAGED_REFUSAL, ManagedBy, ManagedPolicy};

/// Columns of one webhook answer. `managed_bot_id` is the bot whose `bot_runners` row names the
/// webhook; the signing secret is never selected.
const SELECT_WEBHOOK: &str = "SELECT w.id, w.workspace_id, w.name, w.url, w.events, w.active, w.bot_user_id, \
     w.created_by, w.created_at, w.updated_at, w.last_triggered_at, r.bot_id AS managed_bot_id \
     FROM webhooks w LEFT JOIN bot_runners r ON r.webhook_id = w.id";

#[derive(Debug, FromQueryResult)]
struct WebhookRow {
    id: Uuid,
    workspace_id: Uuid,
    name: Option<String>,
    url: String,
    events: serde_json::Value,
    active: bool,
    bot_user_id: Option<Uuid>,
    created_by: Uuid,
    created_at: platform::timestamp::Timestamp,
    updated_at: platform::timestamp::Timestamp,
    last_triggered_at: Option<platform::timestamp::Timestamp>,
    managed_bot_id: Option<Uuid>,
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
    /// Set when a project bot connection manages this webhook.
    pub managed_by: Option<ManagedBy>,
}

/// The answer to creating a webhook or rotating its secret: the webhook plus its signing secret.
///
/// This is the only place the secret ever leaves the API, once, so the receiver can be configured
/// with it (including one the server generated). [`WebhookResponse`], which list, get and update
/// answer with and which is copied into the webhook's business events, has no secret field.
#[derive(Serialize)]
pub struct WebhookWithSecretResponse {
    #[serde(flatten)]
    pub webhook: WebhookResponse,
    /// HMAC-SHA256 key of `X-Webhook-Signature`; not retrievable afterwards.
    pub secret: String,
}

impl std::fmt::Debug for WebhookWithSecretResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookWithSecretResponse")
            .field("webhook", &self.webhook)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// What it takes to create a webhook.
#[derive(Debug)]
pub struct NewWebhook {
    pub workspace_id: Uuid,
    pub actor_id: Uuid,
    pub name: String,
    pub url: String,
    /// The signing secret; one is generated when `None`.
    pub secret: Option<String>,
    pub events: Vec<String>,
    pub active: bool,
    pub bot_user_id: Option<Uuid>,
}

/// A 64-character alphanumeric signing secret.
pub fn generate_secret() -> String {
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

async fn response_of(db: &impl ConnectionTrait, row: WebhookRow) -> Result<WebhookResponse, ApiError> {
    let events: Vec<String> = serde_json::from_value(row.events).map_err(|_| ApiError::Internal)?;
    let managed_by = match row.managed_bot_id {
        Some(bot_id) => Some(bots::managed_projects(db, bot_id).await?),
        None => None,
    };
    Ok(WebhookResponse {
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
        managed_by,
    })
}

/// Webhooks of a workspace, newest first.
pub async fn list(db: &impl ConnectionTrait, workspace_id: Uuid) -> Result<Vec<WebhookResponse>, ApiError> {
    let rows = WebhookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{SELECT_WEBHOOK} WHERE w.workspace_id = $1 ORDER BY w.created_at DESC"),
        vec![workspace_id.into()],
    ))
    .all(db)
    .await?;
    let mut responses = Vec::with_capacity(rows.len());
    for row in rows {
        responses.push(response_of(db, row).await?);
    }
    Ok(responses)
}

/// One webhook of a workspace; 404 when the workspace has no such webhook.
pub async fn get(db: &impl ConnectionTrait, workspace_id: Uuid, webhook_id: Uuid) -> Result<WebhookResponse, ApiError> {
    let row = WebhookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{SELECT_WEBHOOK} WHERE w.id = $1 AND w.workspace_id = $2"),
        vec![webhook_id.into(), workspace_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("Webhook not found".to_string()))?;
    response_of(db, row).await
}

async fn find(db: &impl ConnectionTrait, webhook_id: Uuid) -> Result<WebhookResponse, ApiError> {
    let row = WebhookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("{SELECT_WEBHOOK} WHERE w.id = $1"),
        vec![webhook_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("Webhook not found".to_string()))?;
    response_of(db, row).await
}

fn validate_events(events: &[String]) -> Result<(), ApiError> {
    for event in events {
        if !WEBHOOK_EVENTS.contains(&event.as_str()) {
            return Err(ApiError::BadRequest(format!("Invalid event type: {event}")));
        }
    }
    Ok(())
}

/// `bot_user_id` must be a bot, and under [`ManagedPolicy::Refuse`] not a managed one.
async fn validate_bot_user(
    db: &impl ConnectionTrait,
    bot_user_id: Uuid,
    policy: ManagedPolicy,
) -> Result<(), ApiError> {
    if !crate::services::bot_identity::is_bot_user(db, bot_user_id).await? {
        return Err(ApiError::BadRequest(
            "bot_user_id must reference a bot user".to_string(),
        ));
    }
    if policy == ManagedPolicy::Refuse && bots::managed_by(db, bot_user_id).await?.is_some() {
        return Err(ApiError::BadRequest(MANAGED_BOT_USER_REFUSAL.to_string()));
    }
    Ok(())
}

/// 409 for a managed webhook unless `policy` allows managed rows.
async fn ensure_editable(db: &impl ConnectionTrait, webhook_id: Uuid, policy: ManagedPolicy) -> Result<(), ApiError> {
    if policy == ManagedPolicy::Refuse {
        let managed = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT 1 FROM bot_runners WHERE webhook_id = $1",
                vec![webhook_id.into()],
            ))
            .await?
            .is_some();
        if managed {
            return Err(ApiError::Conflict(MANAGED_REFUSAL.to_string()));
        }
    }
    Ok(())
}

/// Creates a webhook on `db` (a transaction, so the subscriber-limit check and the insert are one
/// step) and writes `webhook.created`. The returned secret is the only copy.
pub async fn create(
    db: &impl ConnectionTrait,
    input: NewWebhook,
    policy: ManagedPolicy,
) -> Result<WebhookWithSecretResponse, ApiError> {
    validate_events(&input.events)?;
    if let Some(bot_user_id) = input.bot_user_id {
        validate_bot_user(db, bot_user_id, policy).await?;
    }

    // Prefer an explicit secret, otherwise generate one. Either way it is returned in this
    // answer only, so a generated one can be handed to the receiver.
    let secret = input.secret.unwrap_or_else(generate_secret);
    let webhook_id = Uuid::new_v4();
    let now = platform::timestamp::Timestamp::now();

    // `subscribers_per_workspace_max` (`limits-v1.md`, `limit_kind: workspace_subscribers`, frozen
    // at 100): a webhook is v0.4's only subscriber kind, so this is where a workspace's subscriber
    // directory grows. Only a row that would actually be active counts: an inactive webhook is not
    // a subscriber (the dispatcher's own directory query and `webhook_trigger.rs` both require
    // `active = true`).
    if input.active {
        ensure_workspace_subscriber_slot(db, input.workspace_id, None).await?;
    }

    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"INSERT INTO webhooks
               (id, workspace_id, name, url, secret, events, active, bot_user_id, created_by, created_at, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        vec![
            webhook_id.into(),
            input.workspace_id.into(),
            input.name.into(),
            input.url.into(),
            secret.clone().into(),
            serde_json::to_value(&input.events)
                .map_err(|_| ApiError::Internal)?
                .into(),
            input.active.into(),
            input.bot_user_id.into(),
            input.actor_id.into(),
            now.into(),
            now.into(),
        ],
    ))
    .await?;

    let response = find(db, webhook_id).await?;
    insert_webhook_event(db, &response, "webhook.created", input.actor_id, policy).await?;
    Ok(WebhookWithSecretResponse {
        webhook: response,
        secret,
    })
}

/// Replaces the signing secret with a newly generated one and returns it, once. Deliveries signed
/// from now on use the new secret. 404 when the workspace has no such webhook.
pub async fn rotate_secret(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    webhook_id: Uuid,
    actor_id: Uuid,
    policy: ManagedPolicy,
) -> Result<WebhookWithSecretResponse, ApiError> {
    ensure_editable(db, webhook_id, policy).await?;
    let secret = generate_secret();
    let result = db
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

    let response = find(db, webhook_id).await?;
    insert_webhook_event(db, &response, "webhook.secret_rotated", actor_id, policy).await?;
    Ok(WebhookWithSecretResponse {
        webhook: response,
        secret,
    })
}

/// Applies the fields of `req` to a webhook and writes `webhook.updated`. 400 when `req` changes
/// nothing, 404 when the workspace has no such webhook.
pub async fn update(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    webhook_id: Uuid,
    actor_id: Uuid,
    req: UpdateWebhookRequest,
    policy: ManagedPolicy,
) -> Result<WebhookResponse, ApiError> {
    ensure_editable(db, webhook_id, policy).await?;
    if let Some(events) = req.events.as_deref() {
        validate_events(events)?;
    }
    if let Some(bot_user_id) = req.bot_user_id {
        validate_bot_user(db, bot_user_id, policy).await?;
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

    // `subscribers_per_workspace_max` (`limits-v1.md`, `limit_kind: workspace_subscribers`):
    // switching a webhook on grows the workspace's active subscriber directory exactly like
    // creating one, so the same ceiling applies here. The webhook being updated is excluded from
    // the tally and counted once as the slot it takes, so re-saving an already-active webhook in a
    // workspace sitting exactly at the ceiling is not rejected.
    if activating {
        ensure_workspace_subscriber_slot(db, workspace_id, Some(webhook_id)).await?;
    }

    let result = db
        .execute(Statement::from_sql_and_values(DbBackend::Postgres, &sql, params))
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Webhook not found".to_string()));
    }

    let response = find(db, webhook_id).await?;
    insert_webhook_event(db, &response, "webhook.updated", actor_id, policy).await?;
    Ok(response)
}

/// Deletes a webhook of `workspace_id` and writes `webhook.deleted`. 404 when there is no such
/// webhook.
pub async fn delete(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    webhook_id: Uuid,
    actor_id: Uuid,
    policy: ManagedPolicy,
) -> Result<(), ApiError> {
    ensure_editable(db, webhook_id, policy).await?;
    let webhook = find(db, webhook_id).await?;
    if webhook.workspace_id != workspace_id {
        return Err(ApiError::NotFound("Webhook not found".to_string()));
    }
    insert_webhook_event(db, &webhook, "webhook.deleted", actor_id, policy).await?;

    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM webhooks WHERE id = $1 AND workspace_id = $2",
            vec![webhook_id.into(), workspace_id.into()],
        ))
        .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Webhook not found".to_string()));
    }
    Ok(())
}

/// The business event of a webhook write. The payload is the webhook's own answer, which carries
/// no secret; a write made for a project connection says so in its metadata.
async fn insert_webhook_event(
    db: &impl ConnectionTrait,
    webhook: &WebhookResponse,
    event_type: &str,
    actor_id: Uuid,
    policy: ManagedPolicy,
) -> Result<(), ApiError> {
    let mut metadata = json!({
        "webhook_id": webhook.id,
        "legacy_webhook": policy == ManagedPolicy::Refuse
    });
    if policy == ManagedPolicy::Allow
        && let Some(object) = metadata.as_object_mut()
    {
        object.insert("managed_by_connection".to_string(), json!(true));
    }
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
            metadata,
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
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
            managed_by: None,
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
