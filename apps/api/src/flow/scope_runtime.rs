//! Versioned, leased registrations for processes that can read scope-native persisted rows.
use crate::error::ApiError;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;
pub const READER_VERSION: i32 = 5;
pub const LEASE_SECONDS: f64 = 30.0;
pub async fn renew(db: &DatabaseConnection, instance: Uuid, component: &str) -> Result<(), ApiError> {
    renew_for(db, instance, component, LEASE_SECONDS).await
}
/// Registers or renews one lease that lasts `lease_seconds` from the database clock. `renew` is the
/// production entry point; a negative duration yields an already expired lease.
pub async fn renew_for(
    db: &DatabaseConnection,
    instance: Uuid,
    component: &str,
    lease_seconds: f64,
) -> Result<(), ApiError> {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO flow_runtime_readers(instance_id,component,reader_version,lease_expires_at) VALUES($1,$2,$3,now()+make_interval(secs=>$4)) ON CONFLICT(instance_id) DO UPDATE SET reader_version=EXCLUDED.reader_version,lease_expires_at=EXCLUDED.lease_expires_at,updated_at=now()",vec![instance.into(),component.into(),READER_VERSION.into(),lease_seconds.into()])).await?;
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

#[cfg(test)]
pub(crate) mod fixture {
    //! Reader leases for scratch databases. Nothing runs in a scratch database, so a test that
    //! models a running installation registers the leases the api and worker processes would hold,
    //! through the same `renew_for` the processes use. They last a day so no test outlives them;
    //! production leases last 30 seconds.
    use super::renew_for;
    use sea_orm::{ConnectionTrait, DatabaseConnection};
    use uuid::Uuid;

    const DAY_SECONDS: f64 = 86_400.0;

    /// Live leases for api, worker and maintenance, as a running api and worker would hold.
    pub async fn register_running_readers(db: &DatabaseConnection) {
        for component in ["api", "worker", "maintenance"] {
            register_reader(db, component, true).await;
        }
    }

    /// One lease for `component`: live for a day, or already expired when `live` is false.
    pub async fn register_reader(db: &DatabaseConnection, component: &str, live: bool) {
        let seconds = if live { DAY_SECONDS } else { -1.0 };
        renew_for(db, Uuid::new_v4(), component, seconds)
            .await
            .expect("a fixture reader lease is registered");
    }

    /// Removes every lease, so a test starts from "no process is running".
    pub async fn clear_readers(db: &DatabaseConnection) {
        db.execute_unprepared("DELETE FROM flow_runtime_readers")
            .await
            .expect("fixture reader leases are cleared");
    }
}
