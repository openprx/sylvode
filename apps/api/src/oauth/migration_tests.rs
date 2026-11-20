//! The authorization server migration against a real PostgreSQL server.
//!
//! Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped. The file
//! is located by its name suffix, so these tests do not change when the migration is renumbered.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::items_after_statements,
    clippy::assertions_on_constants
)]

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use super::test_support::{Start, count, oauth_scratch_or_skip, seed_grant};
use crate::routes::context::tenant_fixture::{Tenant, exec, seed_bot_user, seed_tenant, seed_work_item};

const TABLES: [&str; 12] = [
    "oauth_clients",
    "workspace_delegation_settings",
    "oauth_authorization_requests",
    "oauth_grants",
    "oauth_grant_projects",
    "oauth_authorization_codes",
    "oauth_tokens",
    "oauth_audit_events",
    "oauth_operation_logs",
    "oauth_rate_hits",
    "oauth_event_outbox",
    "oauth_event_feed",
];

const TRIGGERS: [(&str, &str); 4] = [
    ("notifications", "trg_oauth_outbox_notification_insert"),
    ("notifications", "trg_oauth_outbox_notification_read"),
    ("ai_tasks", "trg_oauth_outbox_ai_task_insert"),
    ("ai_tasks", "trg_oauth_outbox_ai_task_status"),
];

async fn schema_objects_present(db: &DatabaseConnection) -> (usize, usize) {
    let mut tables = 0;
    for table in TABLES {
        tables += usize::try_from(
            count(
                db,
                "SELECT count(*) AS n FROM pg_class WHERE oid = to_regclass($1) AND relkind = 'r'",
                vec![table.into()],
            )
            .await,
        )
        .unwrap_or(0);
    }
    let mut triggers = 0;
    for (table, name) in TRIGGERS {
        triggers += usize::try_from(
            count(
                db,
                "SELECT count(*) AS n FROM pg_trigger WHERE tgrelid = to_regclass($1) AND tgname = $2 AND NOT tgisinternal",
                vec![table.into(), name.into()],
            )
            .await,
        )
        .unwrap_or(0);
    }
    (tables, triggers)
}

#[derive(Debug, FromQueryResult)]
struct ClientRow {
    display_name: String,
    client_type: String,
    token_endpoint_auth_method: String,
    redirect_uris: Vec<String>,
    allowed_scopes: Vec<String>,
    allowed_resources: Vec<String>,
    embed_origins: Vec<String>,
    status: String,
}

async fn ramflux(db: &DatabaseConnection) -> ClientRow {
    ClientRow::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT display_name, client_type, token_endpoint_auth_method, redirect_uris, allowed_scopes, \
         allowed_resources, embed_origins, status FROM oauth_clients WHERE client_id = 'ramflux'"
            .to_owned(),
    ))
    .one(db)
    .await
    .expect("client query runs")
    .expect("the ramflux client is seeded")
}

#[tokio::test]
async fn a_fresh_database_gets_twelve_tables_four_triggers_and_the_ramflux_client() {
    let scratch = oauth_scratch_or_skip!("mig_fresh", Start::Complete);
    assert_eq!(schema_objects_present(&scratch.db).await, (12, 4));
    let client = ramflux(&scratch.db).await;
    assert_eq!(client.display_name, "Ramflux");
    assert_eq!(client.client_type, "public");
    assert_eq!(client.token_endpoint_auth_method, "none");
    assert_eq!(
        client.redirect_uris,
        [
            "http://127.0.0.1/oauth/sylvode/callback",
            "http://[::1]/oauth/sylvode/callback"
        ]
    );
    // The first release offers the read set and the right to stay connected, nothing that writes.
    assert_eq!(
        client.allowed_scopes,
        [
            "projects:read",
            "work_items:read",
            "notifications:read",
            "tasks:read",
            "offline_access"
        ]
    );
    assert_eq!(client.allowed_resources, ["api", "mcp"]);
    assert!(client.embed_origins.is_empty());
    assert_eq!(client.status, "active");
    assert_eq!(
        count(&scratch.db, "SELECT count(*) AS n FROM oauth_clients", vec![]).await,
        1
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_database_that_predates_the_migration_keeps_its_rows_and_gains_the_schema() {
    let scratch = oauth_scratch_or_skip!("mig_upgrade", Start::BeforeOauth);
    let db = &scratch.db;
    assert_eq!(schema_objects_present(db).await, (0, 0));

    let tenant = seed_tenant(db, "up").await;
    let bot = seed_bot_user(db, &tenant).await;
    insert_notification(db, &tenant, Some(tenant.workspace_id), None, None).await;
    insert_ai_task(db, &tenant, bot, "pending").await;
    let before = (
        count(db, "SELECT count(*) AS n FROM notifications", vec![]).await,
        count(db, "SELECT count(*) AS n FROM ai_tasks", vec![]).await,
    );
    assert_eq!(before, (1, 1));

    scratch
        .apply_oauth_migration()
        .await
        .expect("the migration applies on top of the previous state");

    assert_eq!(schema_objects_present(db).await, (12, 4));
    assert_eq!(
        (
            count(db, "SELECT count(*) AS n FROM notifications", vec![]).await,
            count(db, "SELECT count(*) AS n FROM ai_tasks", vec![]).await,
        ),
        before,
        "existing rows are untouched"
    );
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_event_outbox", vec![]).await,
        0,
        "rows that existed before the triggers are not replayed as events"
    );
    insert_notification(db, &tenant, Some(tenant.workspace_id), None, None).await;
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_event_outbox", vec![]).await,
        1
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn running_the_file_again_changes_nothing() {
    let scratch = oauth_scratch_or_skip!("mig_rerun", Start::Complete);
    let db = &scratch.db;
    let tenant = seed_tenant(db, "rr").await;
    exec(
        db,
        "UPDATE oauth_clients SET display_name = 'Edited by an operator' WHERE client_id = 'ramflux'",
        vec![],
    )
    .await;
    let (_, trigger_oid_before) = trigger_identity(db).await;

    scratch.apply_oauth_migration().await.expect("a second run succeeds");
    scratch.apply_oauth_migration().await.expect("a third run succeeds");

    assert_eq!(schema_objects_present(db).await, (12, 4));
    assert_eq!(
        ramflux(db).await.display_name,
        "Edited by an operator",
        "the seed row is never overwritten"
    );
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_clients", vec![]).await, 1);
    let (count_after, trigger_oid_after) = trigger_identity(db).await;
    assert_eq!(count_after, 4, "no trigger is added twice");
    assert_eq!(
        trigger_oid_before, trigger_oid_after,
        "an existing trigger is not dropped and recreated"
    );
    insert_notification(db, &tenant, Some(tenant.workspace_id), None, None).await;
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_event_outbox", vec![]).await,
        1,
        "one insert still produces exactly one outbox row after the re-runs"
    );
    scratch.drop_self().await;
}

#[derive(Debug, FromQueryResult)]
struct TriggerIdentity {
    n: i64,
    oids: String,
}

async fn trigger_identity(db: &DatabaseConnection) -> (i64, String) {
    let row = TriggerIdentity::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT count(*) AS n, COALESCE(string_agg(oid::text, ',' ORDER BY tgname), '') AS oids FROM pg_trigger \
         WHERE tgname LIKE 'trg_oauth_outbox_%' AND NOT tgisinternal"
            .to_owned(),
    ))
    .one(db)
    .await
    .expect("trigger query runs")
    .expect("one row");
    (row.n, row.oids)
}

async fn insert_notification(
    db: &impl ConnectionTrait,
    tenant: &Tenant,
    workspace: Option<Uuid>,
    project: Option<Uuid>,
    issue: Option<Uuid>,
) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO notifications (id, user_id, workspace_id, type, kind, payload, title, content, \
         related_project_id, related_issue_id, is_read) \
         VALUES ($1, $2, $3, 'assignment', 'assignment', '{}'::jsonb, 'notification.issueAssignedTitle', \
         'issue_assigned:test', $4, $5, false)",
        vec![
            id.into(),
            tenant.member_id.into(),
            workspace.into(),
            project.into(),
            issue.into(),
        ],
    )
    .await;
    id
}

async fn insert_ai_task(db: &impl ConnectionTrait, tenant: &Tenant, participant: Uuid, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status) \
         VALUES ($1, $2, $3, 'issue_assigned', $4)",
        vec![id.into(), tenant.project_id.into(), participant.into(), status.into()],
    )
    .await;
    id
}

#[derive(Debug, FromQueryResult, PartialEq, Eq)]
struct OutboxRow {
    source: String,
    source_id: Uuid,
    event_type: String,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    user_id: Option<Uuid>,
    detail: serde_json::Value,
}

async fn outbox(db: &DatabaseConnection) -> Vec<OutboxRow> {
    OutboxRow::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT source, source_id, event_type, workspace_id, project_id, user_id, detail \
         FROM oauth_event_outbox ORDER BY id"
            .to_owned(),
    ))
    .all(db)
    .await
    .expect("outbox query runs")
}

#[tokio::test]
async fn notification_triggers_write_exactly_the_specified_rows() {
    let scratch = oauth_scratch_or_skip!("mig_trg_notification", Start::Complete);
    let db = &scratch.db;
    let tenant = seed_tenant(db, "nt").await;
    let issue = seed_work_item(db, &tenant, "an item").await;

    // Created with a workspace and a project: one row carrying that project.
    let with_project = insert_notification(db, &tenant, Some(tenant.workspace_id), Some(tenant.project_id), None).await;
    // Created with only a related issue: the project comes from the work item.
    let with_issue = insert_notification(db, &tenant, Some(tenant.workspace_id), None, Some(issue)).await;
    // Created with neither: the project is unknown, the row is still written.
    let bare = insert_notification(db, &tenant, Some(tenant.workspace_id), None, None).await;
    // No workspace: nothing is written.
    insert_notification(db, &tenant, None, Some(tenant.project_id), None).await;

    let rows = outbox(db).await;
    let expected = |id, project| OutboxRow {
        source: "notification".into(),
        source_id: id,
        event_type: "notification.created".into(),
        workspace_id: tenant.workspace_id,
        project_id: project,
        user_id: Some(tenant.member_id),
        detail: serde_json::json!({}),
    };
    assert_eq!(
        rows,
        vec![
            expected(with_project, Some(tenant.project_id)),
            expected(with_issue, Some(tenant.project_id)),
            expected(bare, None),
        ]
    );

    // false -> true writes notification.read; nothing else about the row does.
    exec(
        db,
        "UPDATE notifications SET title = 'edited' WHERE id = $1",
        vec![with_project.into()],
    )
    .await;
    exec(
        db,
        "UPDATE notifications SET is_read = false WHERE id = $1",
        vec![with_project.into()],
    )
    .await;
    assert_eq!(
        outbox(db).await.len(),
        3,
        "an update that does not mark the row read writes nothing"
    );
    exec(
        db,
        "UPDATE notifications SET is_read = true WHERE id = $1",
        vec![with_project.into()],
    )
    .await;
    exec(
        db,
        "UPDATE notifications SET is_read = true WHERE id = $1",
        vec![with_project.into()],
    )
    .await;
    exec(
        db,
        "UPDATE notifications SET is_read = false WHERE id = $1",
        vec![with_project.into()],
    )
    .await;
    let rows = outbox(db).await;
    assert_eq!(rows.len(), 4, "only the false -> true transition counts, once");
    assert_eq!(rows[3].event_type, "notification.read");
    assert_eq!(rows[3].source_id, with_project);
    assert_eq!(rows[3].project_id, Some(tenant.project_id));
    assert_eq!(rows[3].user_id, Some(tenant.member_id));

    // A rolled back write leaves no outbox row behind.
    let before = outbox(db).await.len();
    {
        use sea_orm::TransactionTrait;
        let tx = db.begin().await.expect("transaction");
        insert_notification(&tx, &tenant, Some(tenant.workspace_id), None, None).await;
        tx.rollback().await.expect("rollback");
    }
    assert_eq!(outbox(db).await.len(), before);
    scratch.drop_self().await;
}

#[tokio::test]
async fn ai_task_triggers_write_exactly_the_specified_rows() {
    let scratch = oauth_scratch_or_skip!("mig_trg_task", Start::Complete);
    let db = &scratch.db;
    let tenant = seed_tenant(db, "tk").await;
    let bot = seed_bot_user(db, &tenant).await;

    let task = insert_ai_task(db, &tenant, bot, "pending").await;
    let rows = outbox(db).await;
    assert_eq!(
        rows,
        vec![OutboxRow {
            source: "ai_task".into(),
            source_id: task,
            event_type: "task.created".into(),
            workspace_id: tenant.workspace_id,
            project_id: Some(tenant.project_id),
            user_id: None,
            detail: serde_json::json!({"to": "pending"}),
        }]
    );

    for (from, to, attempts) in [
        ("pending", "processing", 1),
        ("processing", "acknowledged", 1),
        ("acknowledged", "completed", 1),
    ] {
        exec(
            db,
            "UPDATE ai_tasks SET status = $2, attempts = $3 WHERE id = $1",
            vec![task.into(), to.into(), attempts.into()],
        )
        .await;
        let rows = outbox(db).await;
        let last = rows.last().expect("a row was written");
        assert_eq!(last.event_type, "task.status_changed");
        assert_eq!(last.source_id, task);
        assert_eq!(
            last.detail,
            serde_json::json!({"from": from, "to": to, "attempts": attempts})
        );
    }
    assert_eq!(outbox(db).await.len(), 4);

    // Updates that keep the status write nothing, whether or not the column is in the SET list.
    exec(
        db,
        "UPDATE ai_tasks SET status = 'completed' WHERE id = $1",
        vec![task.into()],
    )
    .await;
    exec(
        db,
        "UPDATE ai_tasks SET priority = 5, error_message = 'x' WHERE id = $1",
        vec![task.into()],
    )
    .await;
    assert_eq!(outbox(db).await.len(), 4);
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_failing_outbox_never_fails_the_original_write() {
    let scratch = oauth_scratch_or_skip!("mig_trg_failing", Start::Complete);
    let db = &scratch.db;
    let tenant = seed_tenant(db, "fl").await;
    let bot = seed_bot_user(db, &tenant).await;

    // Every outbox insert now violates a constraint.
    exec(
        db,
        "ALTER TABLE oauth_event_outbox ADD CONSTRAINT test_refuses_every_row CHECK (false) NOT VALID",
        vec![],
    )
    .await;
    let notification = insert_notification(db, &tenant, Some(tenant.workspace_id), None, None).await;
    exec(
        db,
        "UPDATE notifications SET is_read = true WHERE id = $1",
        vec![notification.into()],
    )
    .await;
    let task = insert_ai_task(db, &tenant, bot, "pending").await;
    exec(
        db,
        "UPDATE ai_tasks SET status = 'processing' WHERE id = $1",
        vec![task.into()],
    )
    .await;
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM notifications WHERE is_read", vec![]).await,
        1
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM ai_tasks WHERE status = 'processing'",
            vec![]
        )
        .await,
        1
    );
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_event_outbox", vec![]).await,
        0
    );

    // The outbox table itself is gone: the writes still succeed.
    exec(db, "DROP TABLE oauth_event_outbox", vec![]).await;
    let second = insert_notification(db, &tenant, Some(tenant.workspace_id), None, None).await;
    exec(
        db,
        "UPDATE notifications SET is_read = true WHERE id = $1",
        vec![second.into()],
    )
    .await;
    exec(
        db,
        "UPDATE ai_tasks SET status = 'completed' WHERE id = $1",
        vec![task.into()],
    )
    .await;
    assert_eq!(count(db, "SELECT count(*) AS n FROM notifications", vec![]).await, 2);
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM ai_tasks WHERE status = 'completed'",
            vec![]
        )
        .await,
        1
    );
    scratch.drop_self().await;
}

/// Each statement differs from a valid row in exactly one respect and has to be refused by the
/// named constraint.
#[tokio::test]
async fn every_check_and_uniqueness_rule_is_proven_by_a_failing_insert() {
    let scratch = oauth_scratch_or_skip!("mig_constraints", Start::Complete);
    let db = &scratch.db;
    let tenant = seed_tenant(db, "cn").await;
    let grant = seed_grant(db, tenant.member_id, tenant.workspace_id, "all", &["projects:read"]).await;
    let (user, ws, project) = (tenant.member_id, tenant.workspace_id, tenant.project_id);

    let client = |id: &str, kind: &str, auth: &str, status: &str, redirects: &str, resources: &str| {
        format!(
            "INSERT INTO oauth_clients (client_id, display_name, client_type, token_endpoint_auth_method, \
             redirect_uris, allowed_scopes, allowed_resources, status) \
             VALUES ('{id}', 'x', '{kind}', '{auth}', {redirects}, ARRAY['projects:read'], {resources}, '{status}')"
        )
    };
    let one_uri = "ARRAY['http://127.0.0.1/cb']";
    let both = "ARRAY['api','mcp']";
    let request = |state: &str, status: &str, device: &str| {
        format!(
            "INSERT INTO oauth_authorization_requests (id, client_id, redirect_uri, state, code_challenge, \
             requested_scopes, resources, device_name, status, expires_at) \
             VALUES ('{}', 'ramflux', 'http://127.0.0.1/cb', {state}, 'c', ARRAY['projects:read'], ARRAY['api'], \
             {device}, '{status}', now() + interval '10 minutes')",
            Uuid::new_v4()
        )
    };
    let valid_state = "'0123456789abcdef'";
    let grant_sql = |mode: &str, status: &str, revoked_at: &str, reason: &str| {
        format!(
            "INSERT INTO oauth_grants (id, client_id, user_id, workspace_id, scopes, resources, project_mode, \
             user_session_version, status, expires_at, revoked_at, revoke_reason) \
             VALUES ('{}', 'ramflux', '{user}', '{ws}', ARRAY['projects:read'], ARRAY['api'], '{mode}', 0, \
             '{status}', now() + interval '1 day', {revoked_at}, {reason})",
            Uuid::new_v4()
        )
    };
    let token = |hash: &str, kind: &str, resource: &str, consumed: &str| {
        format!(
            "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, resource, scopes, expires_at, \
             consumed_at) VALUES ('{}', '{hash}', '{kind}', '{grant}', 'ramflux', {resource}, \
             ARRAY['projects:read'], now() + interval '1 day', {consumed})",
            Uuid::new_v4()
        )
    };
    let hash = |n: u8| format!("{n:0>64}");
    let audit = |event: &str, detail: &str, ua: &str| {
        format!(
            "INSERT INTO oauth_audit_events (id, event, detail, user_agent) VALUES ('{}', '{event}', {detail}, {ua})",
            Uuid::new_v4()
        )
    };
    let oplog = |outcome: &str, surface: &str| {
        format!(
            "INSERT INTO oauth_operation_logs (id, grant_id, client_id, user_id, workspace_id, surface, method, path, \
             business_code, outcome, duration_ms, request_id) \
             VALUES ('{}', '{grant}', 'ramflux', '{user}', '{ws}', '{surface}', 'GET', '/x', 200, '{outcome}', 1, '{}')",
            Uuid::new_v4(),
            Uuid::new_v4()
        )
    };
    let outbox_sql = |source: &str, event: &str| {
        format!(
            "INSERT INTO oauth_event_outbox (source, source_id, event_type, workspace_id) \
             VALUES ('{source}', '{}', '{event}', '{ws}')",
            Uuid::new_v4()
        )
    };

    // Rows that must be accepted, so a refusal below can only come from the one changed value.
    let valid = [
        client("other-app", "public", "none", "active", one_uri, both),
        request(valid_state, "pending", "'laptop'"),
        grant_sql("selected", "active", "NULL", "NULL"),
        grant_sql("all", "revoked", "now()", "'user_revoked'"),
        token(&hash(1), "access", "'api'", "NULL"),
        token(&hash(2), "refresh", "NULL", "now()"),
        audit("consent_granted", "'{}'::jsonb", "NULL"),
        oplog("ok", "rest"),
        oplog("error", "mcp_http"),
        outbox_sql("notification", "notification.read"),
        outbox_sql("ai_task", "task.status_changed"),
        format!(
            "INSERT INTO oauth_event_feed (event_type, workspace_id, subject, occurred_at) VALUES ('x', '{ws}', '{{}}'::jsonb, now())"
        ),
        format!("INSERT INTO oauth_grant_projects (grant_id, project_id) VALUES ('{grant}', '{project}')"),
        format!("INSERT INTO workspace_delegation_settings (workspace_id) VALUES ('{ws}')"),
        "INSERT INTO oauth_rate_hits (bucket, key_hash, window_started, hit_count) VALUES ('b', 'k', now(), 1)"
            .to_owned(),
    ];
    for sql in &valid {
        db.execute_unprepared(sql)
            .await
            .unwrap_or_else(|err| panic!("a valid row was refused: {err}\n{sql}"));
    }

    let long_agent = format!("'{}'", "u".repeat(257));
    let eleven_uris = format!(
        "ARRAY[{}]",
        (0..11)
            .map(|n| format!("'http://127.0.0.1/{n}'"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let refused: Vec<(&str, String)> = vec![
        (
            "oauth_clients_id_check",
            client("Bad", "public", "none", "active", one_uri, both),
        ),
        (
            "oauth_clients_id_check",
            client("ab", "public", "none", "active", one_uri, both),
        ),
        (
            "oauth_clients_type_check",
            client("app-two", "native", "none", "active", one_uri, both),
        ),
        (
            "oauth_clients_auth_check",
            client("app-two", "public", "client_secret_basic", "active", one_uri, both),
        ),
        (
            "oauth_clients_status_check",
            client("app-two", "public", "none", "paused", one_uri, both),
        ),
        (
            "oauth_clients_redirect_check",
            client("app-two", "public", "none", "active", "ARRAY[]::text[]", both),
        ),
        (
            "oauth_clients_redirect_check",
            client("app-two", "public", "none", "active", &eleven_uris, both),
        ),
        (
            "oauth_clients_resource_check",
            client("app-two", "public", "none", "active", one_uri, "ARRAY['api','flow']"),
        ),
        (
            "oauth_clients_pkey",
            client("ramflux", "public", "none", "active", one_uri, both),
        ),
        ("oauth_ar_status_check", request(valid_state, "open", "NULL")),
        ("oauth_ar_state_check", request("'short'", "pending", "NULL")),
        (
            "oauth_ar_state_check",
            request(&format!("'{}'", "s".repeat(513)), "pending", "NULL"),
        ),
        ("oauth_ar_device_check", request(valid_state, "pending", "''")),
        (
            "oauth_ar_device_check",
            request(valid_state, "pending", &format!("'{}'", "d".repeat(65))),
        ),
        ("oauth_grants_mode_check", grant_sql("some", "active", "NULL", "NULL")),
        ("oauth_grants_status_check", grant_sql("all", "paused", "NULL", "NULL")),
        (
            "oauth_grants_revoked_check",
            grant_sql("all", "revoked", "NULL", "NULL"),
        ),
        (
            "oauth_grants_revoked_check",
            grant_sql("all", "active", "now()", "NULL"),
        ),
        (
            "oauth_grants_reason_check",
            grant_sql("all", "revoked", "now()", "'because'"),
        ),
        ("oauth_tokens_kind_check", token(&hash(3), "id", "'api'", "NULL")),
        ("oauth_tokens_resource_check", token(&hash(3), "access", "NULL", "NULL")),
        (
            "oauth_tokens_resource_check",
            token(&hash(3), "refresh", "'api'", "NULL"),
        ),
        (
            "oauth_tokens_resource_value",
            token(&hash(3), "access", "'flow'", "NULL"),
        ),
        (
            "oauth_tokens_consumed_check",
            token(&hash(3), "access", "'api'", "now()"),
        ),
        (
            "oauth_tokens_token_hash_key",
            token(&hash(1), "access", "'api'", "NULL"),
        ),
        ("oauth_audit_event_check", audit("Consent", "'{}'::jsonb", "NULL")),
        ("oauth_audit_event_check", audit("x", "'{}'::jsonb", "NULL")),
        (
            "oauth_audit_detail_check",
            audit("consent_granted", "'[]'::jsonb", "NULL"),
        ),
        (
            "oauth_audit_ua_check",
            audit("consent_granted", "'{}'::jsonb", &long_agent),
        ),
        ("oauth_oplog_outcome_check", oplog("maybe", "rest")),
        ("oauth_oplog_surface_check", oplog("ok", "cli")),
        ("oauth_outbox_source_check", outbox_sql("comment", "task.created")),
        ("oauth_outbox_type_check", outbox_sql("ai_task", "task.deleted")),
        (
            "oauth_feed_subject_check",
            format!(
                "INSERT INTO oauth_event_feed (event_type, workspace_id, subject, occurred_at) VALUES ('x', '{ws}', '[]'::jsonb, now())"
            ),
        ),
        (
            "oauth_grant_projects_pkey",
            format!("INSERT INTO oauth_grant_projects (grant_id, project_id) VALUES ('{grant}', '{project}')"),
        ),
        (
            "workspace_delegation_settings_pkey",
            format!("INSERT INTO workspace_delegation_settings (workspace_id) VALUES ('{ws}')"),
        ),
        (
            "oauth_rate_hits_pkey",
            "INSERT INTO oauth_rate_hits (bucket, key_hash, window_started, hit_count) VALUES ('b', 'k', now(), 1)"
                .to_owned(),
        ),
        (
            "oauth_grants_client_id_fkey",
            grant_sql("all", "active", "NULL", "NULL").replacen("'ramflux'", "'unknown-app'", 1),
        ),
        ("oauth_authorization_codes_pkey", {
            let code = format!(
                "INSERT INTO oauth_authorization_codes (code_hash, grant_id, client_id, redirect_uri, code_challenge, \
                     resources, expires_at) VALUES ('{}', '{grant}', 'ramflux', 'http://127.0.0.1/cb', 'c', \
                     ARRAY['api'], now() + interval '1 minute')",
                hash(9)
            );
            db.execute_unprepared(&code).await.expect("the first code is accepted");
            code
        }),
    ];
    for (constraint, sql) in &refused {
        let error = db
            .execute_unprepared(sql)
            .await
            .err()
            .unwrap_or_else(|| panic!("expected {constraint} to refuse:\n{sql}"));
        assert!(
            error.to_string().contains(constraint),
            "expected {constraint}, got: {error}\n{sql}"
        );
    }
    scratch.drop_self().await;
}
