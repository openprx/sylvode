//! Versioned, leased registrations for processes that can read scope-native persisted rows.
use crate::error::ApiError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;
pub const READER_VERSION: i32 = 5;
pub async fn renew(db: &DatabaseConnection, instance: Uuid, component: &str) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_runtime_readers(instance_id,component,reader_version,lease_expires_at) VALUES($1,$2,$3,now()+interval '30 seconds') ON CONFLICT(instance_id) DO UPDATE SET reader_version=EXCLUDED.reader_version,lease_expires_at=EXCLUDED.lease_expires_at,updated_at=now()",vec![instance.into(),component.into(),READER_VERSION.into()])).await?;
    Ok(())
}
pub async fn spawn(db: DatabaseConnection, component: &'static str) -> Result<(), ApiError> {
    let instance = Uuid::new_v4();
    renew(&db, instance, component).await?;
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(std::time::Duration::from_secs(10));
        loop {
            timer.tick().await;
            if let Err(err) = renew(&db, instance, component).await {
                tracing::error!(%err,component,"Flow reader registration renewal failed; independent write barrier will fail closed");
            }
        }
    });
    Ok(())
}
