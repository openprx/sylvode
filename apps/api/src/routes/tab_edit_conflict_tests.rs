//! Real `PostgreSQL` optimistic-edit contracts for retained workspace tabs.
#![cfg(test)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::{context::tenant_fixture, form, project, project_type};
use axum::{Extension, Router, body::Body, http::Request, routing::patch};
use platform::{
    app::{AppState, FlowPermissionCacheSlot},
    auth::{JwtClaims, TokenType},
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

struct Fixture {
    scratch: tenant_fixture::Scratch,
    router: Router,
    project: Uuid,
    form: Uuid,
    record: Uuid,
    view: Uuid,
    resource: Uuid,
}
async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap();
}
async fn fixture() -> Fixture {
    let scratch = tenant_fixture::scratch(&format!("tab_cas_{}", Uuid::new_v4().simple()))
        .await
        .expect("explicit disposable OPENPR_TEST_DATABASE_URL required");
    let db = &scratch.db;
    let user = Uuid::new_v4();
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    let form = Uuid::new_v4();
    let record = Uuid::new_v4();
    let view = Uuid::new_v4();
    let resource = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'','Tab fixture','user',true)",
        vec![user.into(), format!("{user}@example.test").into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Tab fixture',$3)",
        vec![workspace.into(), workspace.to_string().into(), user.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_members(workspace_id,user_id,role) VALUES($1,$2,'owner')",
        vec![workspace.into(), user.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO projects(id,workspace_id,key,name,created_by) VALUES($1,$2,'TABCAS','Tab fixture',$3)",
        vec![project.into(), workspace.into(), user.into()],
    )
    .await;
    exec(db,"INSERT INTO project_forms(id,workspace_id,project_id,key,name,schema,created_by) VALUES($1,$2,$3,'tab_form','Tab form',$4,$5)",vec![form.into(),workspace.into(),project.into(),json!({"version":"sv.form.schema.v2","fields":[{"field_id":"fld_text","key":"text","type":"text"}]}).into(),user.into()]).await;
    exec(db,"INSERT INTO form_records(id,workspace_id,project_id,form_id,title,values,schema_version,created_by) VALUES($1,$2,$3,$4,'Initial',$5,1,$6)",vec![record.into(),workspace.into(),project.into(),form.into(),json!({"text":"Initial"}).into(),user.into()]).await;
    exec(db,"INSERT INTO form_views(id,workspace_id,project_id,form_id,key,name,view_type,config,created_by) VALUES($1,$2,$3,$4,'tab_view','Initial','grid','{}',$5)",vec![view.into(),workspace.into(),project.into(),form.into(),user.into()]).await;
    exec(
        db,
        "INSERT INTO project_resources(id,project_id,kind,name,created_by) VALUES($1,$2,'custom','Resource',$3)",
        vec![resource.into(), project.into(), user.into()],
    )
    .await;
    let state = AppState {
        db: db.clone(),
        cfg: AppConfig {
            app_name: "tab-cas-fixture".into(),
            bind_addr: "127.0.0.1:0".into(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("tab-fixture-not-runtime"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: vec![],
        },
        flow_permission_cache: FlowPermissionCacheSlot::default(),
    };
    // Principal extraction is mocked; the real handlers still resolve the resource and policy.
    let claims = JwtClaims {
        sub: user.to_string(),
        email: format!("{user}@example.test"),
        token_type: TokenType::Access,
        iat: 0,
        exp: 0,
        sv: 0,
    };
    let router = Router::new()
        .route("/projects/{id}", patch(project::update_project))
        .route("/forms/{id}", patch(form::update_form))
        .route("/records/{id}", patch(form::update_form_record))
        .route("/views/{id}", patch(form::update_form_view))
        .route(
            "/projects/{project_id}/resources/{resource_id}",
            patch(project_type::update_project_resource),
        )
        .layer(Extension(claims))
        .with_state(state);
    Fixture {
        scratch,
        router,
        project,
        form,
        record,
        view,
        resource,
    }
}
async fn request(router: Router, path: String, body: Value) -> (u16, Value) {
    let r = router
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri(path)
                .header("Content-Type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = r.status().as_u16();
    let body = serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    (status, body)
}
async fn version(db: &DatabaseConnection, table: &str, id: Uuid) -> String {
    db.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!("SELECT updated_at FROM {table} WHERE id=$1"),
        vec![id.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<platform::timestamp::Timestamp>("", "updated_at")
    .unwrap()
    .to_rfc3339()
}
async fn waits(db: &DatabaseConnection, n: i64) {
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let blocked=db.query_one(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS n FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'".to_string())).await.unwrap().unwrap().try_get::<i64>("","n").unwrap();
        if blocked >= n {
            return;
        }
        assert!(
            tokio::time::Instant::now() < until,
            "both requests must reach the real SQL lock before release, observed{blocked}/{n}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
async fn race(f: &Fixture, table: &str, id: Uuid, path: String, a: Value, b: Value) {
    let db = &f.scratch.db;
    let blocker = db.begin().await.unwrap();
    blocker
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT id FROM {table} WHERE id=$1 FOR UPDATE"),
            vec![id.into()],
        ))
        .await
        .unwrap();
    let first = tokio::spawn(request(f.router.clone(), path.clone(), a.clone()));
    let second = tokio::spawn(request(f.router.clone(), path.clone(), b));
    waits(db, 2).await;
    blocker.commit().await.unwrap();
    let (one, two) = (first.await.unwrap(), second.await.unwrap());
    assert_eq!(
        [one.0, two.0].into_iter().filter(|s| *s == 200).count(),
        1,
        "{one:?} {two:?}"
    );
    assert_eq!(
        [one.0, two.0].into_iter().filter(|s| *s == 409).count(),
        1,
        "{one:?} {two:?}"
    );
    let conflict = if one.0 == 409 { one.1 } else { two.1 };
    assert_eq!(conflict.get("code"), Some(&json!(409)));
    assert_eq!(conflict.get("error_code"), Some(&json!("version_conflict")));
    let before = version(db, table, id).await;
    let stale = request(f.router.clone(), path, a).await;
    assert_eq!(stale.0, 409);
    assert_eq!(
        version(db, table, id).await,
        before,
        "stale write must not touch the row"
    );
}
#[tokio::test]
async fn four_retained_edit_routes_atomically_refuse_same_base_and_stale_writes() {
    let f = fixture().await;
    let db = &f.scratch.db;
    let token = version(db, "projects", f.project).await;
    race(
        &f,
        "projects",
        f.project,
        format!("/projects/{}", f.project),
        json!({"name":"Project A","expected_updated_at":token}),
        json!({"name":"Project B","expected_updated_at":token}),
    )
    .await;
    let token = version(db, "form_records", f.record).await;
    race(
        &f,
        "form_records",
        f.record,
        format!("/records/{}", f.record),
        json!({"title":"Record A","values":{"text":"A"},"expected_updated_at":token,"expected_schema_version":1}),
        json!({"title":"Record B","values":{"text":"B"},"expected_updated_at":token,"expected_schema_version":1}),
    )
    .await;
    let token = version(db, "form_views", f.view).await;
    race(
        &f,
        "form_views",
        f.view,
        format!("/views/{}", f.view),
        json!({"name":"View A","expected_updated_at":token}),
        json!({"name":"View B","expected_updated_at":token}),
    )
    .await;
    race(
        &f,
        "project_forms",
        f.form,
        format!("/forms/{}", f.form),
        json!({"detail_layout":{"winner":"A"},"expected_schema_version":1}),
        json!({"detail_layout":{"winner":"B"},"expected_schema_version":1}),
    )
    .await;
    let before = version(db, "form_records", f.record).await;
    let stale_schema = request(
        f.router.clone(),
        format!("/records/{}", f.record),
        json!({"values":{"text":"must not win"},"expected_updated_at":before,"expected_schema_version":1}),
    )
    .await;
    assert_eq!(stale_schema.0, 409);
    assert_eq!(version(db, "form_records", f.record).await, before);
    let events=db.query_one(Statement::from_string(DbBackend::Postgres,"SELECT count(*) AS n FROM form_events WHERE event_type IN('form.record.updated','form.updated','form.view.updated')".to_string())).await.unwrap().unwrap().try_get::<i64>("","n").unwrap();
    assert_eq!(events, 3, "only each winning mutation emits a form event");
    f.scratch.drop_self().await;
}
#[tokio::test]
async fn record_validation_holds_schema_stable_until_write_and_tokens_always_advance() {
    let f = fixture().await;
    let db = &f.scratch.db;
    let base = version(db, "form_records", f.record).await;
    let blocker = db.begin().await.unwrap();
    blocker
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id FROM form_records WHERE id=$1 FOR UPDATE",
            vec![f.record.into()],
        ))
        .await
        .unwrap();
    let record = tokio::spawn(request(
        f.router.clone(),
        format!("/records/{}", f.record),
        json!({"values":{"text":"serialized"},"expected_updated_at":base,"expected_schema_version":1}),
    ));
    waits(db, 1).await;
    let design = tokio::spawn(request(
        f.router.clone(),
        format!("/forms/{}", f.form),
        json!({"detail_layout":{"changed":true},"expected_schema_version":1}),
    ));
    waits(db, 2).await;
    assert!(
        !design.is_finished(),
        "schema UPDATE must wait for in-flight record validation"
    );
    blocker.commit().await.unwrap();
    assert_eq!(record.await.unwrap().0, 200);
    assert_eq!(design.await.unwrap().0, 200);
    let next = version(db, "form_records", f.record).await;
    assert_ne!(next, base);
    let same = request(
        f.router.clone(),
        format!("/records/{}", f.record),
        json!({"values":{"text":"serialized"},"expected_updated_at":next,"expected_schema_version":2}),
    )
    .await;
    assert_eq!(same.0, 200);
    assert_ne!(
        version(db, "form_records", f.record).await,
        next,
        "an accepted write must never reuse the old token"
    );
    f.scratch.drop_self().await;
}

#[tokio::test]
async fn project_resource_config_uses_atomic_original_versions() {
    let f = fixture().await;
    let db = &f.scratch.db;
    let token = version(db, "project_resources", f.resource).await;
    race(
        &f,
        "project_resources",
        f.resource,
        format!("/projects/{}/resources/{}", f.project, f.resource),
        json!({"name":"Resource A","expected_updated_at":token}),
        json!({"name":"Resource B","expected_updated_at":token}),
    )
    .await;
    f.scratch.drop_self().await;
}
async fn form_state(db: &DatabaseConnection, form: Uuid) -> (i32, i64, Vec<Value>) {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT schema_version, (SELECT count(*) FROM form_schema_versions WHERE form_id=$1) AS versions \
             FROM project_forms WHERE id=$1",
            vec![form.into()],
        ))
        .await
        .unwrap()
        .unwrap();
    let events = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT payload FROM form_events WHERE form_id=$1 AND event_type='form.updated' ORDER BY created_at",
            vec![form.into()],
        ))
        .await
        .unwrap()
        .iter()
        .map(|event| event.try_get::<Value>("", "payload").unwrap())
        .collect();
    (
        row.try_get("", "schema_version").unwrap(),
        row.try_get("", "versions").unwrap(),
        events,
    )
}
#[tokio::test]
async fn form_update_writes_the_schema_once_and_emits_one_event_per_save() {
    let f = fixture().await;
    let db = &f.scratch.db;
    let path = format!("/forms/{}", f.form);
    let schema = |label: &str| json!({"fields":[{"key":"text","label":label,"type":"text"}]});
    let (version0, history0, _) = form_state(db, f.form).await;

    // A name-only save changes no version and writes no history row, but emits one event.
    let (status, _) = request(f.router.clone(), path.clone(), json!({"name":"Renamed"})).await;
    assert_eq!(status, 200);
    let (version, history, events) = form_state(db, f.form).await;
    assert_eq!((version, history), (version0, history0));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["changed_fields"], json!(["name"]));

    // A schema-only save bumps once, writes one history row and emits one event.
    let (status, _) = request(
        f.router.clone(),
        path.clone(),
        json!({"schema":schema("One"),"expected_schema_version":version0}),
    )
    .await;
    assert_eq!(status, 200);
    let (version, history, events) = form_state(db, f.form).await;
    assert_eq!((version, history, events.len()), (version0 + 1, history0 + 1, 2));
    assert_eq!(events[1]["changed_fields"], json!(["schema"]));

    // A combined save bumps once, writes one history row and emits one event naming every field.
    let (status, body) = request(
        f.router.clone(),
        path.clone(),
        json!({"name":"Combined","description":"d","schema":schema("Two"),"detail_layout":{"x":1},"expected_schema_version":version0 + 1}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["name"], "Combined");
    assert_eq!(body["data"]["schema_version"], version0 + 2);
    let (version, history, events) = form_state(db, f.form).await;
    assert_eq!((version, history, events.len()), (version0 + 2, history0 + 2, 3));
    assert_eq!(
        events[2]["changed_fields"],
        json!(["name", "description", "schema", "detail_layout"])
    );

    // A stale combined save is refused whole: no column, version, history row or event changes.
    let (status, _) = request(
        f.router.clone(),
        path,
        json!({"name":"Stale","schema":schema("Three"),"expected_schema_version":version0}),
    )
    .await;
    assert_eq!(status, 409);
    let (version, history, events) = form_state(db, f.form).await;
    assert_eq!((version, history, events.len()), (version0 + 2, history0 + 2, 3));
    let name = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT name FROM project_forms WHERE id=$1",
            vec![f.form.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<String>("", "name")
        .unwrap();
    assert_eq!(name, "Combined");
    f.scratch.drop_self().await;
}
