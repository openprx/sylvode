//! Worker-owned deep document verification jobs.

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

#[derive(Debug, FromQueryResult)]
struct Job {
    id: Uuid,
    document_id: Uuid,
    expected_head_seq: Option<i64>,
}

#[derive(Debug, Default)]
pub struct TickReport {
    pub verified: u64,
    pub failed: u64,
}

pub async fn run_tick(db: &DatabaseConnection, requested_batch_size: usize) -> anyhow::Result<TickReport> {
    let mut report = TickReport::default();
    for _ in 0..requested_batch_size.clamp(1, 100) {
        let job = Job::find_by_statement(Statement::from_string(
            DbBackend::Postgres,
            "UPDATE flow_operation_runs SET status = 'running' \
             WHERE id = (SELECT id FROM flow_operation_runs \
               WHERE operation = 'verify_document' AND status = 'planned' \
               ORDER BY created_at, id FOR UPDATE SKIP LOCKED LIMIT 1) \
             RETURNING id, scope_id AS document_id, expected_head_seq"
                .to_string(),
        ))
        .one(db)
        .await?;
        let Some(job) = job else { break };
        let (_authority, outcome) = match api::flow::scope_operations::job_authority(db, job.id).await {
            Ok(guard) => (
                guard,
                api::flow::collab::integrity::document_fingerprint(db, job.document_id)
                    .await
                    .and_then(|fingerprint| {
                        if job.expected_head_seq.is_some_and(|seq| seq != fingerprint.head_seq) {
                            Err(api::error::ApiError::stale_frontier(
                                "maintenance head changed",
                                Some(fingerprint.head_seq),
                                None,
                            ))
                        } else {
                            Ok(fingerprint)
                        }
                    }),
            ),
            Err(error) => (None, Err(error)),
        };
        match outcome {
            Ok(fingerprint) => {
                let result = serde_json::to_value(fingerprint)?;
                db.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_operation_runs SET status = 'completed', result_redacted = $2, \
                     finished_at = now() WHERE id = $1",
                    vec![job.id.into(), result.into()],
                ))
                .await?;
                report.verified += 1;
            }
            Err(error) => {
                db.execute(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "UPDATE flow_operation_runs SET status = 'failed', \
                     result_redacted = jsonb_build_object('reason', 'document_verification_failed'), \
                     finished_at = now() WHERE id = $1",
                    vec![job.id.into()],
                ))
                .await?;
                tracing::warn!(job_id = %job.id, %error, "flow document verification job failed");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}
