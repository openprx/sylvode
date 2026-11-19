pub mod compaction;
pub mod integrity;
pub mod projection;
pub mod retention;
pub mod search;

#[cfg(test)]
pub mod test_readers {
    use api::flow::scope_runtime::renew_for;
    use sea_orm::DatabaseConnection;
    use uuid::Uuid;

    /// Seconds in a day: scratch-database leases outlive every test, because no process renews them.
    const DAY_SECONDS: f64 = 86_400.0;

    /// Live leases for api, worker and maintenance, as a running api and worker would hold. They are
    /// registered through the production `renew_for`, with a day-long duration.
    pub async fn register_running_readers(db: &DatabaseConnection) {
        for component in ["api", "worker", "maintenance"] {
            renew_for(db, Uuid::new_v4(), component, DAY_SECONDS)
                .await
                .expect("a fixture reader lease is registered");
        }
    }
}
