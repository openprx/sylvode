//! Task rows for the database tests of the task protocol.
#![allow(clippy::expect_used, clippy::panic)]

use sea_orm::DatabaseConnection;
use serde_json::Value;
use uuid::Uuid;

use crate::routes::context::tenant_fixture::{Tenant, exec};

/// An active-looking task of `bot` that started `started_secs_ago` seconds ago and holds a lease
/// of one more hour, with no reference (a `manual` task).
pub async fn seed_task(
    db: &DatabaseConnection,
    tenant: &Tenant,
    bot: Uuid,
    status: &str,
    attempts: i32,
    started_secs_ago: i64,
) -> Uuid {
    seed_task_for(
        db,
        tenant,
        bot,
        status,
        attempts,
        started_secs_ago,
        ("manual", None, None),
        Value::Null,
    )
    .await
}

/// [`seed_task`] with a task type, a reference type and id, and a payload.
#[allow(clippy::too_many_arguments)]
pub async fn seed_task_for(
    db: &DatabaseConnection,
    tenant: &Tenant,
    bot: Uuid,
    status: &str,
    attempts: i32,
    started_secs_ago: i64,
    (task_type, reference_type, reference_id): (&str, Option<&str>, Option<Uuid>),
    payload: Value,
) -> Uuid {
    let task_id = Uuid::new_v4();
    let payload = if payload.is_null() {
        serde_json::json!({})
    } else {
        payload
    };
    exec(
        db,
        "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, reference_type, reference_id, status, \
                               attempts, max_attempts, payload, started_at, lease_expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 3, $9, now() - make_interval(secs => $10::bigint), \
                 now() + interval '1 hour')",
        vec![
            task_id.into(),
            tenant.project_id.into(),
            bot.into(),
            task_type.to_string().into(),
            reference_type.map(str::to_string).into(),
            reference_id.into(),
            status.to_string().into(),
            attempts.into(),
            payload.into(),
            started_secs_ago.into(),
        ],
    )
    .await;
    task_id
}
