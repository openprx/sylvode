//! Sending of deliveries and the delivery body.

use std::collections::HashSet;

use platform::app::AppState;
use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    CHANGED_BLOCK_IDS_PER_DELIVERY_MAX, DELIVERY_ID_HEADER, DELIVERY_LEASE_TTL_MS, WEBHOOK_REQUEST_TIMEOUT_MS,
    delivery_backoff_ms, ms_interval,
};
use crate::error::ApiError;
use crate::outbound::{OutboundTarget, delivery_client, resolve_outbound_target};
use crate::webhook_trigger::{WEBHOOK_SIGNATURE_HEADER, WEBHOOK_TIMESTAMP_HEADER, WEBHOOK_USER_AGENT, sign_payload};

// ---------------------------------------------------------------------------------------------
// Sending (`events-v1.md` "满额封存与 sealed 状态" lease-claim SQL, "投递报文与 delivery_id 的位置")
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
pub(super) struct DeliveryLeaseRow {
    pub(super) id: Uuid,
    pub(super) event_id: Uuid,
    pub(super) subscriber_id: Uuid,
    pub(super) document_id: Option<Uuid>,
    pub(super) attempts: i32,
    pub(super) max_attempts: i32,
    pub(super) first_seq: Option<i64>,
    pub(super) latest_seq: Option<i64>,
}

/// Leases and sends exactly one `event_deliveries` row. `Ok(None)` means nothing was ready;
/// `Ok(Some(true))` a successful send, `Ok(Some(false))` a retry/terminal-failure/cancellation —
/// all three of those still "handled" the row and are not dispatcher errors.
pub(super) async fn send_one(state: &AppState, client: &reqwest::Client) -> Result<Option<bool>, ApiError> {
    let lease_token = Uuid::new_v4().to_string();
    // Literal SQL from `events-v1.md` "满额封存与 `sealed` 状态", parameterized.
    let leased = DeliveryLeaseRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r"
                UPDATE event_deliveries
                SET status = 'leased', lease_token = $1, lease_expires_at = now() + {ttl}
                WHERE id = (
                    SELECT d.id FROM event_deliveries d
                    WHERE d.status IN ('pending', 'sealed') AND d.next_attempt_at <= now()
                      AND (
                        d.document_id IS NULL OR d.first_seq IS NULL
                        OR NOT EXISTS (
                            SELECT 1 FROM event_deliveries o
                            WHERE o.subscriber_kind = d.subscriber_kind
                              AND o.subscriber_id   = d.subscriber_id
                              AND o.document_id     = d.document_id
                              AND o.status IN ('sealed', 'leased')
                              AND o.first_seq < d.first_seq)
                      )
                    ORDER BY d.next_attempt_at, d.id
                    LIMIT 1 FOR UPDATE SKIP LOCKED)
                RETURNING id, event_id, subscriber_id, document_id, attempts, max_attempts, first_seq, latest_seq
            ",
            ttl = ms_interval(2)
        ),
        vec![lease_token.clone().into(), DELIVERY_LEASE_TTL_MS.into()],
    ))
    .one(&state.db)
    .await?;

    let Some(delivery) = leased else { return Ok(None) };
    Ok(Some(attempt_delivery(state, client, &delivery, &lease_token).await))
}

#[derive(Debug, FromQueryResult)]
pub(super) struct WebhookRow {
    pub(super) url: String,
    pub(super) secret: String,
}

#[cfg(test)]
tokio::task_local! {
    pub(super) static TEST_ALLOW_PRIVATE_DELIVERY_TARGET: bool;
}

pub(super) async fn validate_delivery_target(raw: &str) -> Result<OutboundTarget, String> {
    #[cfg(test)]
    if TEST_ALLOW_PRIVATE_DELIVERY_TARGET
        .try_with(|allow| *allow)
        .unwrap_or(false)
    {
        return reqwest::Url::parse(raw)
            .map(|url| OutboundTarget { url, pin: None })
            .map_err(|error| error.to_string());
    }
    resolve_outbound_target(raw).await
}

pub(super) async fn attempt_delivery(
    state: &AppState,
    client: &reqwest::Client,
    delivery: &DeliveryLeaseRow,
    lease_token: &str,
) -> bool {
    // Re-evaluate every independent source against the recipient's CURRENT mount before each
    // network attempt. Revocation after an earlier failed attempt cancels later retries.
    #[derive(FromQueryResult)]
    struct Authorized {
        allowed: bool,
    }
    let authorized=Authorized::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT NOT EXISTS(SELECT 1 FROM event_delivery_sources ds JOIN business_events e ON e.id=ds.source_event_id JOIN webhooks w ON w.id=ds.subscriber_id WHERE ds.delivery_id=$1 AND e.source_flow_scope_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM flow_mounts m JOIN flow_objects o ON o.id=m.source_object_id WHERE m.destination_workspace_id=w.workspace_id AND o.scope_id=e.source_flow_scope_id AND o.id IN(SELECT object_id FROM flow_mount_acl_ancestors(COALESCE(e.payload->>'object_id',e.aggregate_id)::uuid,e.source_flow_scope_id)) AND (o.lifecycle_status='active' OR e.event_type IN ('flow.object.archived','flow.object.restored')) AND (e.event_type NOT IN ('flow.relation.linked','flow.relation.unlinked') OR EXISTS(SELECT 1 FROM flow_mounts dependency JOIN flow_objects dep_object ON dep_object.id=dependency.source_object_id WHERE dependency.destination_workspace_id=w.workspace_id AND dep_object.scope_id=e.source_flow_scope_id AND dep_object.lifecycle_status='active' AND dep_object.id IN(SELECT object_id FROM flow_mount_acl_ancestors((e.payload->>'target_object_id')::uuid,e.source_flow_scope_id)))))) AS allowed",vec![delivery.id.into()])).one(&state.db).await;
    match authorized {
        Ok(Some(Authorized { allowed: true })) => {}
        Ok(_) => {
            cancel_delivery(&state.db, delivery.id, lease_token, "source_audience_revoked").await;
            return false;
        }
        Err(err) => {
            tracing::warn!(%err,delivery_id=%delivery.id,"source audience recheck failed");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "source_authorization_unavailable").await;
            return false;
        }
    }
    // "发送前重新判定 active" (`events-v1.md` "订阅者在投递在飞时被改动"): read the current
    // endpoint/secret at send time, never a snapshot taken at expansion.
    let webhook = WebhookRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT url, secret FROM webhooks WHERE id = $1 AND active = true",
        vec![delivery.subscriber_id.into()],
    ))
    .one(&state.db)
    .await;

    let webhook = match webhook {
        Ok(Some(webhook)) => webhook,
        Ok(None) => {
            cancel_delivery(&state.db, delivery.id, lease_token, "subscriber_gone").await;
            return false;
        }
        Err(err) => {
            tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: subscriber lookup failed");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "subscriber_lookup_failed").await;
            return false;
        }
    };

    let body = match build_delivery_body(&state.db, delivery).await {
        Ok(body) => body,
        Err(err) => {
            tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: building delivery body failed");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "payload_build_failed").await;
            return false;
        }
    };

    let target = match validate_delivery_target(&webhook.url).await {
        Ok(target) => target,
        Err(err) => {
            tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: webhook url rejected");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "endpoint_rejected").await;
            return false;
        }
    };

    let Ok(raw_body) = serde_json::to_string(&body) else {
        retry_or_fail_delivery(&state.db, delivery, lease_token, "payload_encode_failed").await;
        return false;
    };
    let signature = sign_payload(&webhook.secret, &raw_body);

    // A name that was resolved and checked is connected to at exactly those addresses, so the
    // shared client is used only when there is nothing to pin.
    let pinned_client;
    let client = if target.pin.is_some() {
        let timeout = std::time::Duration::from_millis(u64::try_from(WEBHOOK_REQUEST_TIMEOUT_MS).unwrap_or(10_000));
        match delivery_client(target.pin.as_ref(), timeout, std::time::Duration::from_secs(5)) {
            Ok(built) => {
                pinned_client = built;
                &pinned_client
            }
            Err(err) => {
                tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: delivery client build failed");
                retry_or_fail_delivery(&state.db, delivery, lease_token, "request_failed").await;
                return false;
            }
        }
    } else {
        client
    };

    let response = client
        .post(target.url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::USER_AGENT, WEBHOOK_USER_AGENT)
        .header(WEBHOOK_SIGNATURE_HEADER, signature.signature)
        .header(WEBHOOK_TIMESTAMP_HEADER, signature.timestamp)
        .header(DELIVERY_ID_HEADER, delivery.id.to_string())
        .body(raw_body)
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            mark_delivered(&state.db, delivery.id, lease_token).await;
            true
        }
        Ok(resp) => {
            let code = format!("http_{}", resp.status().as_u16());
            retry_or_fail_delivery(&state.db, delivery, lease_token, &code).await;
            false
        }
        Err(err) => {
            tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: webhook request failed");
            retry_or_fail_delivery(&state.db, delivery, lease_token, "request_failed").await;
            false
        }
    }
}

pub(super) async fn mark_delivered(db: &DatabaseConnection, delivery_id: Uuid, lease_token: &str) {
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE event_deliveries
                SET status = 'dispatched', terminated_at = now(), lease_token = NULL, lease_expires_at = NULL
                WHERE id = $1 AND lease_token = $2
            ",
            vec![delivery_id.into(), lease_token.into()],
        ))
        .await;
    if let Err(err) = result {
        tracing::warn!(delivery_id = %delivery_id, error = %err, "dispatcher: marking delivery dispatched failed");
    }
}

pub(super) async fn cancel_delivery(db: &DatabaseConnection, delivery_id: Uuid, lease_token: &str, reason: &str) {
    // `events-v1.md` "订阅者在投递在飞时被改动": a gone/inactive subscriber terminates the row as
    // `cancelled`, never `failed` — it must not pollute dead-letter counts or trigger alerts.
    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE event_deliveries
                SET status = 'cancelled', terminated_at = now(), lease_token = NULL, lease_expires_at = NULL,
                    last_error_code = $2
                WHERE id = $1 AND lease_token = $3
            ",
            vec![delivery_id.into(), reason.into(), lease_token.into()],
        ))
        .await;
    if let Err(err) = result {
        tracing::warn!(delivery_id = %delivery_id, error = %err, "dispatcher: cancelling delivery failed");
    }
}

pub(super) async fn retry_or_fail_delivery(
    db: &DatabaseConnection,
    delivery: &DeliveryLeaseRow,
    lease_token: &str,
    reason: &str,
) {
    let next_attempts = i64::from(delivery.attempts) + 1;
    let exhausted = next_attempts >= i64::from(delivery.max_attempts);
    let next_status = if exhausted {
        "failed"
    } else if delivery.document_id.is_none() {
        "pending"
    } else {
        "sealed"
    };
    let backoff_ms = delivery_backoff_ms(next_attempts);

    let result = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                UPDATE event_deliveries
                SET attempts = $2,
                    status = $3,
                    terminated_at = CASE WHEN $3 = 'failed' THEN now() ELSE NULL END,
                    next_attempt_at = now() + ($4::bigint * interval '1 millisecond'),
                    lease_token = NULL,
                    lease_expires_at = NULL,
                    last_error_code = $5
                WHERE id = $1 AND lease_token = $6
            ",
            vec![
                delivery.id.into(),
                next_attempts.into(),
                next_status.into(),
                backoff_ms.into(),
                reason.into(),
                lease_token.into(),
            ],
        ))
        .await;
    if let Err(err) = result {
        tracing::warn!(delivery_id = %delivery.id, error = %err, "dispatcher: recording delivery failure failed");
    }
}

// ---------------------------------------------------------------------------------------------
// Delivery body (`events-v1.md` "投递报文与 delivery_id 的位置")
// ---------------------------------------------------------------------------------------------

#[derive(Debug, FromQueryResult)]
pub(super) struct BusinessEventRow {
    pub(super) id: Uuid,
    pub(super) workspace_id: Option<Uuid>,
    pub(super) source_flow_scope_id: Option<Uuid>,
    pub(super) project_id: Option<Uuid>,
    pub(super) event_type: String,
    pub(super) aggregate_type: String,
    pub(super) aggregate_id: String,
    pub(super) actor_id: Option<Uuid>,
    pub(super) source: Value,
    pub(super) payload: Value,
    pub(super) metadata: Value,
    pub(super) correlation_id: Option<Uuid>,
    pub(super) causation_id: Option<Uuid>,
    pub(super) created_at: Timestamp,
}

pub(super) async fn fetch_business_event<C: ConnectionTrait>(
    conn: &C,
    event_id: Uuid,
) -> Result<Option<BusinessEventRow>, ApiError> {
    Ok(BusinessEventRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT id, workspace_id, source_flow_scope_id, project_id, event_type, aggregate_type, aggregate_id, actor_id,
                   source, payload, metadata, correlation_id, causation_id, created_at
            FROM business_events WHERE id = $1
        ",
        vec![event_id.into()],
    ))
    .one(conn)
    .await?)
}

/// `events-v1.md` "每个 Flow event type 必须在 payload policy registry 中显式声明；未声明的
/// producer 测试失败，未知 payload 在 delivery 时整体 withheld": every payload reaching a webhook
/// body goes through `flow::event_policy` here, not just events whose type happens to start with
/// `flow.` -- a non-Flow producer's event type simply has no registry entry and is withheld the
/// same fail-closed way, which is the intended behavior (this dispatcher is a shared platform
/// component; `flow::event_policy` documents its policies are Flow's v0.4 registry, not the only
/// one that will ever exist here).
pub(super) fn envelope_json(
    event: &BusinessEventRow,
    event_id_override: Option<Uuid>,
    created_at_override: Option<Timestamp>,
    payload_override: Option<Value>,
) -> Value {
    let raw_payload = payload_override.unwrap_or_else(|| event.payload.clone());
    // Form record events carry ids and field keys only; every other type goes through the Flow
    // registry and is withheld when it has no entry there.
    let (payload, source) =
        crate::forms::event_redaction::delivery_parts(&event.event_type, &raw_payload, &event.metadata, &event.source)
            .unwrap_or_else(|| {
                (
                    crate::flow::event_policy::redact_flow_event_payload_for_delivery(&event.event_type, &raw_payload),
                    event.source.clone(),
                )
            });
    let metadata = crate::flow::event_policy::redact_flow_event_metadata_for_delivery(&event.metadata);
    let mut envelope = json!({
        "version": "openpr.event.v1",
        "event_id": event_id_override.unwrap_or(event.id),
        "event_type": event.event_type,
        "workspace_id": event.workspace_id,
        "project_id": event.project_id,
        "aggregate": { "type": event.aggregate_type, "id": event.aggregate_id },
        "actor_id": event.actor_id,
        "source": source,
        "payload": payload,
        "metadata": metadata,
        "correlation_id": event.correlation_id,
        "causation_id": event.causation_id,
        "created_at": created_at_override.unwrap_or(event.created_at).to_rfc3339(),
    });
    if let Some(scope) = event.source_flow_scope_id {
        if let Value::Object(fields) = &mut envelope {
            fields.insert("version".into(), json!("openpr.event.v2"));
            fields.insert("source_scope".into(), json!({"kind":"flow","id":scope}));
        }
        if let Some(object) = envelope.as_object_mut() {
            object.remove("workspace_id");
        }
    }
    envelope
}

#[derive(Debug, FromQueryResult)]
pub(super) struct SourceEventIdRow {
    pub(super) source_event_id: Uuid,
}

/// Builds `{delivery:{...}, event:{...openpr.event.v1}}` per `events-v1.md`.
///
/// "Coalesced" is judged by `event_delivery_sources` row count for this `delivery_id` being > 1
/// (the contract's exact rule — not `document_id IS NOT NULL`, which a `requeue_failed` revival can
/// clear on an otherwise still-coalesced row). Every delivery — content or not — has at least one
/// source row from [`expand_one_subscriber`]'s reservation, so this check is uniform.
pub(super) async fn build_delivery_body(
    db: &DatabaseConnection,
    delivery: &DeliveryLeaseRow,
) -> Result<Value, ApiError> {
    let source_ids = SourceEventIdRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT source_event_id FROM event_delivery_sources WHERE delivery_id = $1 ORDER BY created_at ASC",
        vec![delivery.id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|row| row.source_event_id)
    .collect::<Vec<_>>();

    let attempt = i64::from(delivery.attempts) + 1;

    if source_ids.len() <= 1 {
        let event = fetch_business_event(db, delivery.event_id)
            .await?
            .ok_or(ApiError::Internal)?;
        return Ok(json!({
            "delivery": { "id": delivery.id, "attempt": attempt, "coalesced": false },
            "event": envelope_json(&event, None, None, None),
        }));
    }

    // Coalesced: envelope anchors on the *first* source event (lineage), `changed_block_ids` is
    // the union across every source event bound to this row, capped at
    // `changed_block_ids_per_delivery_max`.
    let first_event = fetch_business_event(db, delivery.event_id)
        .await?
        .ok_or(ApiError::Internal)?;

    let mut block_ids: Vec<Uuid> = Vec::new();
    let mut seen: HashSet<Uuid> = HashSet::new();
    let mut truncated = false;
    for source_id in &source_ids {
        let Some(source_event) = fetch_business_event(db, *source_id).await? else {
            continue;
        };
        let Some(ids) = source_event.payload.get("changed_block_ids").and_then(Value::as_array) else {
            continue;
        };
        for id in ids {
            let Some(id) = id.as_str().and_then(|raw| Uuid::parse_str(raw).ok()) else {
                continue;
            };
            if block_ids.len() >= CHANGED_BLOCK_IDS_PER_DELIVERY_MAX {
                truncated = true;
                break;
            }
            if seen.insert(id) {
                block_ids.push(id);
            }
        }
    }

    let payload = if truncated {
        json!({ "changed_block_ids_truncated": true })
    } else {
        json!({ "changed_block_ids": block_ids })
    };

    Ok(json!({
        "delivery": {
            "id": delivery.id,
            "attempt": attempt,
            "coalesced": true,
            "range": { "first_seq": delivery.first_seq, "latest_seq": delivery.latest_seq },
            "source_event_ids": source_ids,
            "block_ids_truncated": truncated,
        },
        "event": envelope_json(&first_event, Some(first_event.id), Some(first_event.created_at), Some(payload)),
    }))
}
