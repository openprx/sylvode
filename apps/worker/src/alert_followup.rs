//! The worker's alert follow-up: announces the outcome of alert tasks, closes finished alerts and
//! prunes stored inbound events. The logic lives in `api::inbound::followup` so it is tested next
//! to the data it reads; this loop only schedules it.

use std::time::Duration;

use platform::config::InboundConfig;
use sea_orm::DatabaseConnection;
use tokio::time::Instant;

/// How often stored events are checked against their retention.
const PRUNE_INTERVAL: Duration = Duration::from_hours(1);

/// Runs the follow-up for the life of the process.
pub async fn run(db: DatabaseConnection, config: InboundConfig) {
    let tick = Duration::from_secs(u64::from(config.followup_interval_secs));
    let mut next_prune = Instant::now();
    tracing::info!(
        interval_secs = config.followup_interval_secs,
        retention_days = config.event_retention_days,
        "alert follow-up worker started"
    );
    loop {
        match api::inbound::followup::run_once(&db).await {
            Ok(report) if report.announced + report.closed > 0 => {
                tracing::info!(
                    announced = report.announced,
                    closed = report.closed,
                    "alert follow-up tick"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(error = %error, "alert follow-up tick failed"),
        }
        if Instant::now() >= next_prune {
            match api::inbound::followup::prune_events(&db, config.event_retention_days).await {
                Ok(0) => {}
                Ok(deleted) => tracing::info!(deleted, "inbound event retention removed events"),
                Err(error) => tracing::warn!(error = %error, "inbound event retention failed"),
            }
            next_prune = Instant::now() + PRUNE_INTERVAL;
        }
        tokio::time::sleep(tick).await;
    }
}
