//! The shared form schema write used by draft apply: version bump, history row, event, and
//! refusal of an invalid schema or a stale expected version. Needs `OPENPR_TEST_DATABASE_URL`.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use api::forms::{
    native_create::{NativeFormCreateRequest, create_form_in_transaction},
    native_update::{NativeFormSchemaUpdate, update_form_schema_in_transaction},
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, QueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;
#[path = "fixtures/page_service_postgres.rs"]
mod fixture;

fn schema(label: &str) -> Value {
    json!({ "fields": [{ "key": "title", "label": label, "type": "text" }] })
}

async fn count(db: &DatabaseConnection, sql: &str, form: Uuid) -> i64 {
    let row: QueryResult = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            vec![form.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get("", "total").unwrap()
}

#[tokio::test]
#[allow(clippy::print_stderr, reason = "report the skip without a tracing subscriber")]
async fn schema_update_bumps_version_records_history_and_refuses_bad_writes() {
    let Some(scratch) = fixture::scratch(&format!("formupd_{}", Uuid::new_v4().simple())).await else {
        eprintln!("NOT EXECUTED: OPENPR_TEST_DATABASE_URL is not set");
        return;
    };
    let db = &scratch.db;
    let (owner, workspace, project) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    fixture::exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'Owner', 'user', true)",
        vec![owner.into(), format!("{owner}@form.test").into()],
    )
    .await;
    fixture::exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'W', $3)",
        vec![workspace.into(), format!("ws-{workspace}").into(), owner.into()],
    )
    .await;
    fixture::exec(
        db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'FRM', 'Forms', $3)",
        vec![project.into(), workspace.into(), owner.into()],
    )
    .await;
    let source = json!({ "type": "user", "actor_id": owner, "origin": "draft" });

    let tx = db.begin().await.unwrap();
    let form = create_form_in_transaction(
        &tx,
        NativeFormCreateRequest {
            form_id: None,
            workspace_id: workspace,
            project_id: project,
            key: "intake".into(),
            name: "Intake".into(),
            description: String::new(),
            icon: None,
            color: None,
            title_template: None,
            schema: schema("Title"),
            detail_layout: json!({}),
            created_by: Some(owner),
            source: source.clone(),
            change_summary: "initial".into(),
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let request = |schema: Value, expected: Option<i32>| NativeFormSchemaUpdate {
        form_id: form.id,
        schema,
        expected_schema_version: expected,
        actor_id: Some(owner),
        source: source.clone(),
        change_summary: "schema from an approved draft".into(),
    };

    // A valid schema bumps the version and writes the history row and the event.
    let tx = db.begin().await.unwrap();
    let updated = update_form_schema_in_transaction(&tx, request(schema("Subject"), Some(form.schema_version)))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(updated.schema_version, form.schema_version + 1);
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*)::bigint AS total FROM form_schema_versions WHERE form_id = $1",
            form.id
        )
        .await,
        2
    );
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*)::bigint AS total FROM form_events WHERE form_id = $1 AND event_type = 'form.updated'",
            form.id
        )
        .await,
        1
    );

    // An invalid schema is refused and leaves the version alone.
    let tx = db.begin().await.unwrap();
    let invalid = json!({ "fields": [{ "key": "title", "type": "no-such-type" }] });
    assert!(
        update_form_schema_in_transaction(&tx, request(invalid, None))
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    // A stale expected version is refused.
    let tx = db.begin().await.unwrap();
    assert!(
        update_form_schema_in_transaction(&tx, request(schema("Other"), Some(form.schema_version)))
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    scratch.drop_self().await;
}
