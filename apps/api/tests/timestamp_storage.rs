use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, Database, DbBackend, Statement};
use serde_json::json;

#[tokio::test]
async fn historical_timestamptz_and_sql_null_use_the_native_timestamp_binding() {
    let Ok(url) = std::env::var("OPENPR_TEST_DATABASE_URL") else {
        return;
    };
    let db = Database::connect(url).await.unwrap();
    let instant = Timestamp::parse_rfc3339("2024-02-29T12:34:56.123400+04:30").unwrap();
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT '2024-02-29 12:34:56.123400+04:30'::timestamptz AS historical,
                $1::timestamptz AS bound, $2::timestamptz AS absent",
            vec![instant.into(), None::<Timestamp>.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let historical: Timestamp = row.try_get("", "historical").unwrap();
    let bound: Timestamp = row.try_get("", "bound").unwrap();
    let absent: Option<Timestamp> = row.try_get("", "absent").unwrap();
    assert_eq!(historical, instant);
    assert_eq!(bound, historical);
    assert_eq!(json!(bound), json!("2024-02-29T08:04:56.123400Z"));
    assert_eq!(bound.to_rfc3339(), "2024-02-29T08:04:56.123400+00:00");
    assert_eq!(json!(absent), json!(null));
}
