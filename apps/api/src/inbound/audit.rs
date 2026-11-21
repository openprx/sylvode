//! Business events of the inbound side (`inbound_event.*`, `alert.*`, `alert_rule.fired`).
//!
//! Payloads carry ids, names and decisions only: never a token, a signing secret or the raw
//! event payload.

use std::net::IpAddr;

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    events::{BusinessEventInput, insert_business_event},
};

/// How many rejection rows one source may write per reason per minute.
const REJECTION_SAMPLE_PER_MINUTE: i64 = 1;

/// The `source` object of every inbound business event.
pub fn inbound_source_value(source_id: Uuid) -> Value {
    json!({ "type": "inbound_source", "source_id": source_id })
}

/// Records an event of the inbound side.
#[allow(clippy::too_many_arguments)]
pub async fn record<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    project_id: Uuid,
    event_type: &str,
    aggregate_type: &str,
    aggregate_id: Uuid,
    actor_id: Option<Uuid>,
    source: Value,
    payload: Value,
) -> Result<(), ApiError> {
    insert_business_event(
        db,
        BusinessEventInput {
            workspace_id,
            project_id: Some(project_id),
            event_type: event_type.to_owned(),
            aggregate_type: aggregate_type.to_owned(),
            aggregate_id: aggregate_id.to_string(),
            actor_id,
            source,
            payload,
            metadata: json!({}),
            correlation_id: None,
            causation_id: None,
            idempotency_key: None,
        },
    )
    .await?;
    Ok(())
}

#[derive(FromQueryResult)]
struct CountRow {
    n: i64,
}

/// Records `inbound_event.rejected` for a known source, sampled to one row per source and reason
/// per minute so a flood of refusals cannot flood the audit trail.
pub async fn record_rejection<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    project_id: Uuid,
    source_id: Uuid,
    reason: &str,
    client: IpAddr,
) -> Result<(), ApiError> {
    let recent = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT COUNT(*) AS n FROM business_events \
         WHERE event_type = 'inbound_event.rejected' AND aggregate_id = $1 \
           AND payload ->> 'reason' = $2 AND created_at > now() - interval '1 minute'",
        vec![source_id.to_string().into(), reason.to_owned().into()],
    ))
    .one(db)
    .await?
    .map_or(0, |row| row.n);
    if recent >= REJECTION_SAMPLE_PER_MINUTE {
        return Ok(());
    }
    record(
        db,
        workspace_id,
        project_id,
        "inbound_event.rejected",
        "inbound_source",
        source_id,
        None,
        inbound_source_value(source_id),
        json!({ "source_id": source_id, "reason": reason, "client_address": client.to_string() }),
    )
    .await
}
