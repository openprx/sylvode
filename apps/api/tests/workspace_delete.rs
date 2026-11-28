//! Deleting a workspace or a project that holds real content, against a real `PostgreSQL`.
//!
//! A workspace with a Flow page that has an accepted collaboration update used to answer
//! `database error`: the `collab_updates.event_id` reference to `business_events` is not a
//! cascade, and the workspace cascade reached the event before the update. These tests pin the
//! outcome that replaced it: everything that belongs to the workspace goes in one transaction,
//! nothing that belongs to another workspace changes, and a delete that must be refused is refused
//! with a typed error and leaves everything in place.
//!
//! The accepted collaboration update is inserted with plain SQL (the `business_events` row, its
//! `event_dispatch` row and the `collab_updates` row `flow::collab::write` writes together). The
//! real write path needs the isolated apply process, which macOS does not support (ADR-0014,
//! `isolated_apply_unsupported_platform`); the rows are what the constraint cares about, so the
//! test runs the same on every platform.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names
)]

use std::collections::BTreeMap;

use api::error::ApiError;
use api::flow::command::{CreateObjectInput, create_object};
use api::flow::event_origin::{CommandOrigin, EventSurface};
use api::flow::scope;
use api::routes::admin::delete_user;
use api::routes::project::delete_project;
use api::routes::workspace::delete_workspace;
use axum::body::to_bytes;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use platform::app::{AppState, FlowPermissionCacheSlot};
use platform::auth::{JwtClaims, TokenType};
use platform::config::{AppConfig, Secret};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::Value;
use uuid::Uuid;

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    async fn new(label: &str) -> Self {
        let admin_url = std::env::var("OPENPR_TEST_DATABASE_URL").expect("set disposable OPENPR_TEST_DATABASE_URL");
        let admin = Database::connect(&admin_url).await.expect("maintenance connection");
        let name = format!("sv_wsdelete_{label}_{}", Uuid::new_v4().simple());
        admin
            .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
            .await
            .expect("scratch database");
        let (prefix, _) = admin_url.rsplit_once('/').expect("database url has a path");
        let db = Database::connect(format!("{prefix}/{name}"))
            .await
            .expect("scratch connection");
        let mut files: Vec<_> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations"))
            .expect("migrations directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
            .collect();
        files.sort();
        for file in files {
            db.execute_unprepared(&std::fs::read_to_string(&file).expect("migration"))
                .await
                .unwrap_or_else(|err| panic!("{}: {err}", file.display()));
        }
        // A scratch database models a running installation: leases a running api, worker and
        // maintenance process would hold, registered through the production `renew_for`.
        Self { db, name, admin_url }
    }

    async fn close(self) {
        let Self { db, name, admin_url } = self;
        drop(db);
        let admin = Database::connect(&admin_url).await.expect("maintenance connection");
        admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await
            .expect("drop scratch database");
    }
}

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "workspace-delete".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("workspace-delete-secret"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: FlowPermissionCacheSlot::default(),
    }
}

fn claims(user: Uuid) -> axum::Extension<JwtClaims> {
    axum::Extension(JwtClaims {
        sub: user.to_string(),
        email: format!("{user}@example.test"),
        token_type: TokenType::Access,
        iat: 0,
        exp: 0,
        sv: 0,
    })
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}\n{sql}"));
}

#[derive(FromQueryResult)]
struct Count {
    n: i64,
}

async fn count(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    Count::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(db)
        .await
        .expect("count query")
        .expect("count row")
        .n
}

/// What one workspace owns, table by table: `$1` is the workspace id. Every table the delete path
/// has to handle appears here, so "nothing else changed" is checked per table, not in total.
const OWNED: &[(&str, &str)] = &[
    ("workspaces", "SELECT count(*) AS n FROM workspaces WHERE id = $1"),
    (
        "workspace_members",
        "SELECT count(*) AS n FROM workspace_members WHERE workspace_id = $1",
    ),
    (
        "workspace_bots",
        "SELECT count(*) AS n FROM workspace_bots WHERE workspace_id = $1",
    ),
    ("projects", "SELECT count(*) AS n FROM projects WHERE workspace_id = $1"),
    (
        "work_items",
        "SELECT count(*) AS n FROM work_items i JOIN projects p ON p.id = i.project_id WHERE p.workspace_id = $1",
    ),
    (
        "comments",
        "SELECT count(*) AS n FROM comments c JOIN work_items i ON i.id = c.work_item_id \
         JOIN projects p ON p.id = i.project_id WHERE p.workspace_id = $1",
    ),
    (
        "activities",
        "SELECT count(*) AS n FROM activities WHERE workspace_id = $1",
    ),
    (
        "notifications",
        "SELECT count(*) AS n FROM notifications WHERE workspace_id = $1",
    ),
    (
        "project_forms",
        "SELECT count(*) AS n FROM project_forms WHERE workspace_id = $1",
    ),
    (
        "form_records",
        "SELECT count(*) AS n FROM form_records WHERE workspace_id = $1",
    ),
    (
        "form_plugin_packages",
        "SELECT count(*) AS n FROM form_plugin_packages k JOIN projects p ON p.id = k.project_id \
         WHERE p.workspace_id = $1",
    ),
    (
        "form_plugin_bindings",
        "SELECT count(*) AS n FROM form_plugin_bindings b JOIN projects p ON p.id = b.project_id \
         WHERE p.workspace_id = $1",
    ),
    ("webhooks", "SELECT count(*) AS n FROM webhooks WHERE workspace_id = $1"),
    (
        "webhook_deliveries",
        "SELECT count(*) AS n FROM webhook_deliveries d JOIN webhooks w ON w.id = d.webhook_id \
         WHERE w.workspace_id = $1",
    ),
    (
        "flow_objects",
        "SELECT count(*) AS n FROM flow_objects WHERE workspace_id = $1",
    ),
    (
        "collab_documents",
        "SELECT count(*) AS n FROM collab_documents d JOIN flow_objects o ON o.id = d.object_id \
         WHERE o.workspace_id = $1",
    ),
    (
        "collab_updates",
        "SELECT count(*) AS n FROM collab_updates u JOIN collab_documents d ON d.id = u.document_id \
         JOIN flow_objects o ON o.id = d.object_id WHERE o.workspace_id = $1",
    ),
    (
        "flow_page_assets",
        "SELECT count(*) AS n FROM flow_page_assets a JOIN flow_objects o ON o.id = a.object_id \
         WHERE o.workspace_id = $1",
    ),
    (
        "business_events",
        "SELECT count(*) AS n FROM business_events WHERE workspace_id = $1",
    ),
    (
        "event_dispatch",
        "SELECT count(*) AS n FROM event_dispatch WHERE workspace_id = $1",
    ),
    (
        "event_deliveries",
        "SELECT count(*) AS n FROM event_deliveries WHERE workspace_id = $1",
    ),
    (
        "event_delivery_sources",
        "SELECT count(*) AS n FROM event_delivery_sources WHERE workspace_id = $1",
    ),
    (
        "flow_object_lineage",
        "SELECT count(*) AS n FROM flow_object_lineage WHERE workspace_id = $1",
    ),
    (
        "flow_conversion_previews",
        "SELECT count(*) AS n FROM flow_conversion_previews WHERE workspace_id = $1",
    ),
    (
        "flow_conversion_jobs",
        "SELECT count(*) AS n FROM flow_conversion_jobs WHERE workspace_id = $1",
    ),
    (
        "flow_scope_command_receipts",
        "SELECT count(*) AS n FROM flow_scope_command_receipts r \
         JOIN flow_workspace_scope_bindings b ON b.scope_id = r.scope_id WHERE b.workspace_id = $1",
    ),
    (
        "flow_workspace_scope_bindings",
        "SELECT count(*) AS n FROM flow_workspace_scope_bindings WHERE workspace_id = $1",
    ),
    (
        "flow_scopes",
        "SELECT count(*) AS n FROM flow_scopes s JOIN flow_workspace_scope_bindings b ON b.scope_id = s.id \
         WHERE b.workspace_id = $1",
    ),
];

async fn counts(db: &DatabaseConnection, workspace: Uuid) -> BTreeMap<&'static str, i64> {
    let mut out = BTreeMap::new();
    for (table, sql) in OWNED {
        out.insert(*table, count(db, sql, vec![workspace.into()]).await);
    }
    out
}

struct Fixture {
    workspace: Uuid,
    owner: Uuid,
    project: Uuid,
    scope: Uuid,
    workspace_page: Uuid,
    /// The `business_events` row a `collab_updates` row references.
    accepted_event: Uuid,
}

async fn page(state: &AppState, fixture: (Uuid, Uuid), project: Option<Uuid>, title: &str) -> (Uuid, Uuid) {
    let accepted = create_object(
        state,
        CreateObjectInput {
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
            workspace_id: fixture.0,
            actor_id: fixture.1,
            actor_is_bot: false,
            object_type: "page".to_string(),
            project_id: project,
            parent_object_id: None,
            title: title.to_string(),
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
        },
    )
    .await
    .expect("page creation");
    (accepted.object.id, accepted.object.document_id)
}

/// A workspace holding one of everything the delete path has to handle.
async fn populate(state: &AppState, label: &str) -> Fixture {
    let db = &state.db;
    let workspace = Uuid::new_v4();
    let owner = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', $3, 'user', true)",
        vec![owner.into(), format!("{owner}@delete.test").into(), label.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, $3, $4)",
        vec![
            workspace.into(),
            format!("ws-{workspace}").into(),
            label.into(),
            owner.into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        vec![workspace.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![workspace.into()],
    )
    .await;

    // Project, issue, comment, and the activity and notification the issue produced.
    let project = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'DEL', 'Delete', $3)",
        vec![project.into(), workspace.into(), owner.into()],
    )
    .await;
    let issue = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO work_items (id, project_id, title, state, sequence_number, created_by) \
         VALUES ($1, $2, 'An issue', 'todo', 1, $3)",
        vec![issue.into(), project.into(), owner.into()],
    )
    .await;
    let comment = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO comments (id, work_item_id, body, author_id) VALUES ($1, $2, 'A comment', $3)",
        vec![comment.into(), issue.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO activities (resource_type, resource_id, event_type, payload, workspace_id, project_id, \
         issue_id, actor_id, user_id) VALUES ('issue', $1, 'issue.commented', '{}'::jsonb, $2, $3, $1, $4, $4)",
        vec![issue.into(), workspace.into(), project.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO notifications (user_id, kind, payload, workspace_id, related_project_id, related_issue_id) \
         VALUES ($1, 'comment', '{}'::jsonb, $2, $3, $4)",
        vec![owner.into(), workspace.into(), project.into(), issue.into()],
    )
    .await;

    // Form and record.
    let form = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO project_forms (id, workspace_id, project_id, key, name) VALUES ($1, $2, $3, 'intake', 'Intake')",
        vec![form.into(), workspace.into(), project.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO form_records (workspace_id, project_id, form_id) VALUES ($1, $2, $3)",
        vec![workspace.into(), project.into(), form.into()],
    )
    .await;

    // Plugin package and its binding, whose foreign key to the package has no cascade.
    let package = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO form_plugin_packages (id, project_id, key, version, manifest, package_sha256, wasm_bytes, html, \
         installed_by) VALUES ($1, $2, 'ext', '1.0.0', '{}'::jsonb, repeat('a', 64), '\\x00', '<p/>', $3)",
        vec![package.into(), project.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO form_plugin_bindings (project_id, key, package_id) VALUES ($1, 'ext', $2)",
        vec![project.into(), package.into()],
    )
    .await;

    // Webhook, a delivery of it, and a bot.
    let webhook = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, created_by) \
         VALUES ($1, $2, 'hook', 'https://hooks.example.test/x', 's', '[]'::jsonb, $3)",
        vec![webhook.into(), workspace.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO webhook_deliveries (webhook_id, event_type, payload) VALUES ($1, 'issue.created', '{}'::jsonb)",
        vec![webhook.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_bots (workspace_id, name, token_hash, token_prefix) \
         VALUES ($1, 'bot', repeat(replace($2::text, '-', ''), 2), left(replace($2::text, '-', ''), 8))",
        vec![workspace.into(), Uuid::new_v4().into()],
    )
    .await;

    // Flow: a page inside the project and one at workspace level, an asset on one of them.
    let ids = (workspace, owner);
    let (project_page, project_doc) = page(state, ids, Some(project), "Project page").await;
    let (workspace_page, workspace_doc) = page(state, ids, None, "Workspace page").await;
    let legacy = scope::legacy_scope(db, workspace).await.expect("legacy scope");
    exec(
        db,
        "INSERT INTO flow_page_assets (id, scope_id, object_id, kind, filename, content_type, byte_size, \
         content_sha256, bytes) VALUES ($1, $2, $3, 'image', 'a.png', 'image/png', 1, repeat('d', 64), '\\x00')",
        vec![Uuid::new_v4().into(), legacy.id.into(), workspace_page.into()],
    )
    .await;

    // The accepted collaboration update: its event, the dispatch row and delivery that expand it,
    // and the update itself, which references the event without a cascade.
    let accepted_event = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO business_events (id, workspace_id, event_type, aggregate_type, aggregate_id, source, payload, \
         metadata) VALUES ($1, $2, 'flow.content.accepted', 'flow_document', $3, '{}'::jsonb, '{}'::jsonb, '{}'::jsonb)",
        vec![accepted_event.into(), workspace.into(), workspace_doc.to_string().into()],
    )
    .await;
    let dispatch = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, document_id, accepted_seq, max_attempts) \
         VALUES ($1, $2, $3, 'flow.content.accepted', $4, 1, 3)",
        vec![
            dispatch.into(),
            accepted_event.into(),
            workspace.into(),
            workspace_doc.into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO event_deliveries (id, dispatch_id, event_id, workspace_id, subscriber_kind, subscriber_id) \
         VALUES ($1, $2, $3, $4, 'webhook', $5)",
        vec![
            Uuid::new_v4().into(),
            dispatch.into(),
            accepted_event.into(),
            workspace.into(),
            webhook.into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO event_delivery_sources (workspace_id, subscriber_kind, subscriber_id, source_event_id) \
         VALUES ($1, 'webhook', $2, $3)",
        vec![workspace.into(), webhook.into(), accepted_event.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO collab_updates (document_id, seq, update_id, content_hash, before_frontier, after_frontier, bytes, \
         origin_surface, projection_seq, event_id) \
         VALUES ($1, 1, $2, repeat('a', 64), '\\x00', '\\x00', '\\x00', 'rest', 1, $3)",
        vec![workspace_doc.into(), Uuid::new_v4().into(), accepted_event.into()],
    )
    .await;
    // A second update in the project page, tied to an event that belongs to the project.
    let project_event = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO business_events (id, workspace_id, project_id, event_type, aggregate_type, aggregate_id, source, \
         payload, metadata) VALUES ($1, $2, $3, 'flow.content.accepted', 'flow_document', $4, '{}'::jsonb, \
         '{}'::jsonb, '{}'::jsonb)",
        vec![
            project_event.into(),
            workspace.into(),
            project.into(),
            project_doc.to_string().into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO collab_updates (document_id, seq, update_id, content_hash, before_frontier, after_frontier, bytes, \
         origin_surface, projection_seq, event_id) \
         VALUES ($1, 1, $2, repeat('e', 64), '\\x00', '\\x00', '\\x00', 'rest', 1, $3)",
        vec![project_doc.into(), Uuid::new_v4().into(), project_event.into()],
    )
    .await;
    let project_dispatch = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO event_dispatch (id, event_id, workspace_id, event_type, document_id, accepted_seq, max_attempts) \
         VALUES ($1, $2, $3, 'flow.content.accepted', $4, 1, 3)",
        vec![
            project_dispatch.into(),
            project_event.into(),
            workspace.into(),
            project_doc.into(),
        ],
    )
    .await;
    // A receipt for the scope command that produced the project event.
    exec(
        db,
        "INSERT INTO flow_scope_command_receipts (scope_id, idempotency_key, actor_id, request_body, object_id, \
         event_id) VALUES ($1, 'receipt-key', $2, '{}'::jsonb, $3, $4)",
        vec![
            legacy.id.into(),
            owner.into(),
            project_page.into(),
            project_event.into(),
        ],
    )
    .await;
    // Conversion provenance: the page was converted into the form. Lineage restricts its source.
    let lineage = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO flow_object_lineage (id, workspace_id, source_object_id, source_frontier, target_type, \
         target_id, target_version, relation, created_by, scope_id) \
         VALUES ($6, $1, $2, 'f', 'form', $3, 1, 'derived_from', $4, $5)",
        vec![
            workspace.into(),
            workspace_page.into(),
            form.into(),
            owner.into(),
            legacy.id.into(),
            lineage.into(),
        ],
    )
    .await;
    // The conversion that produced the lineage: a preview and the job that committed it. Both
    // restrict their source object, and the job also restricts the preview and the lineage row.
    let preview = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO flow_conversion_previews (id, workspace_id, actor_id, actor_kind, source_object_id, \
         source_frontier, target_type, target_project_id, target_schema_version, mapping, permission_snapshot, \
         idempotency_key, expires_at, scope_id, request_key) \
         VALUES ($1, $2, $3, 'user', $4, 'f', 'form', $5, 1, '{}'::jsonb, '{}'::jsonb, 'preview-key', \
         now() + interval '1 day', $6, 'preview-request')",
        vec![
            preview.into(),
            workspace.into(),
            owner.into(),
            workspace_page.into(),
            project.into(),
            legacy.id.into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_conversion_jobs (id, workspace_id, preview_id, actor_id, actor_kind, source_object_id, \
         source_frontier, target_type, target_schema_version, status, lineage_id, idempotency_key, scope_id, \
         request_key) VALUES ($1, $2, $3, $4, 'user', $5, 'f', 'form', 1, 'completed', $6, 'job-key', $7, 'job-request')",
        vec![
            Uuid::new_v4().into(),
            workspace.into(),
            preview.into(),
            owner.into(),
            workspace_page.into(),
            lineage.into(),
            legacy.id.into(),
        ],
    )
    .await;

    Fixture {
        workspace,
        owner,
        project,
        scope: legacy.id,
        workspace_page,
        accepted_event,
    }
}

async fn envelope<T: IntoResponse>(result: Result<T, ApiError>) -> (u16, Value) {
    let response = match result {
        Ok(ok) => ok.into_response(),
        Err(err) => err.into_response(),
    };
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.expect("body");
    (status, serde_json::from_slice(&bytes).expect("json envelope"))
}

fn assert_all_present(counts: &BTreeMap<&'static str, i64>, what: &str) {
    for table in [
        "workspaces",
        "workspace_members",
        "projects",
        "work_items",
        "comments",
        "activities",
        "notifications",
        "project_forms",
        "form_records",
        "form_plugin_packages",
        "form_plugin_bindings",
        "webhooks",
        "webhook_deliveries",
        "workspace_bots",
        "flow_objects",
        "collab_documents",
        "collab_updates",
        "flow_page_assets",
        "business_events",
        "event_dispatch",
        "event_deliveries",
        "event_delivery_sources",
        "flow_object_lineage",
        "flow_conversion_previews",
        "flow_conversion_jobs",
        "flow_scope_command_receipts",
        "flow_workspace_scope_bindings",
        "flow_scopes",
    ] {
        assert!(
            counts.get(table).copied().unwrap_or(0) > 0,
            "{what}: the fixture holds no `{table}` row, so the test would not prove anything about it: {counts:?}"
        );
    }
}

#[tokio::test]
async fn deleting_a_workspace_with_content_removes_all_of_it_and_nothing_else() {
    let scratch = Scratch::new("workspace").await;
    let state = state_for(scratch.db.clone());
    let doomed = populate(&state, "doomed").await;
    let other = populate(&state, "untouched").await;
    let db = &scratch.db;

    let doomed_before = counts(db, doomed.workspace).await;
    let other_before = counts(db, other.workspace).await;
    assert_all_present(&doomed_before, "doomed workspace");
    assert_all_present(&other_before, "untouched workspace");
    let users_before = count(db, "SELECT count(*) AS n FROM users", vec![]).await;
    let logical_before = count(db, "SELECT count(*) AS n FROM flow_logical_blocks", vec![]).await;

    let (status, body) =
        envelope(delete_workspace(State(state.clone()), claims(doomed.owner), Path(doomed.workspace)).await).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["code"], 0, "the delete must succeed: {body}");

    let doomed_after = counts(db, doomed.workspace).await;
    for (table, n) in &doomed_after {
        assert_eq!(
            *n, 0,
            "`{table}` still holds rows of the deleted workspace: {doomed_after:?}"
        );
    }
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM flow_scope_settings WHERE scope_id = $1",
            vec![doomed.scope.into()]
        )
        .await,
        0,
        "the legacy scope's own rows go with the scope"
    );
    assert_eq!(
        counts(db, other.workspace).await,
        other_before,
        "the other workspace changed"
    );
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM users", vec![]).await,
        users_before,
        "deleting a workspace never deletes people"
    );
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM flow_logical_blocks", vec![]).await,
        logical_before,
        "the block registry keeps its tombstones"
    );
    scratch.close().await;
}

#[tokio::test]
async fn deleting_a_project_with_flow_pages_forms_and_issues_removes_only_that_project() {
    let scratch = Scratch::new("project").await;
    let state = state_for(scratch.db.clone());
    let doomed = populate(&state, "doomed").await;
    let other = populate(&state, "untouched").await;
    let db = &scratch.db;
    let other_before = counts(db, other.workspace).await;
    let page_in_project = |project: Uuid| {
        count(
            db,
            "SELECT count(*) AS n FROM flow_objects WHERE project_id = $1",
            vec![project.into()],
        )
    };
    assert!(page_in_project(doomed.project).await > 0);

    let (status, body) =
        envelope(delete_project(State(state.clone()), claims(doomed.owner), Path(doomed.project)).await).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["code"], 0, "the delete must succeed: {body}");

    for (sql, what) in [
        ("SELECT count(*) AS n FROM projects WHERE id = $1", "project"),
        ("SELECT count(*) AS n FROM work_items WHERE project_id = $1", "issues"),
        (
            "SELECT count(*) AS n FROM activities WHERE project_id = $1",
            "activities",
        ),
        ("SELECT count(*) AS n FROM project_forms WHERE project_id = $1", "forms"),
        (
            "SELECT count(*) AS n FROM form_records WHERE project_id = $1",
            "records",
        ),
        (
            "SELECT count(*) AS n FROM form_plugin_bindings WHERE project_id = $1",
            "plugin bindings",
        ),
        (
            "SELECT count(*) AS n FROM form_plugin_packages WHERE project_id = $1",
            "plugin packages",
        ),
        (
            "SELECT count(*) AS n FROM flow_objects WHERE project_id = $1",
            "flow pages",
        ),
        (
            "SELECT count(*) AS n FROM business_events WHERE project_id = $1",
            "project events",
        ),
    ] {
        assert_eq!(count(db, sql, vec![doomed.project.into()]).await, 0, "{what} survived");
    }
    // The workspace page and its accepted update live outside the project.
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM flow_objects WHERE id = $1",
            vec![doomed.workspace_page.into()]
        )
        .await,
        1
    );
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM business_events WHERE id = $1",
            vec![doomed.accepted_event.into()]
        )
        .await,
        1
    );
    assert_eq!(
        counts(db, other.workspace).await,
        other_before,
        "the other workspace changed"
    );
    scratch.close().await;
}

/// A reference from outside the workspace that restricts it on purpose must stop the delete with a
/// typed refusal and roll back everything the delete had already removed.
///
/// No table of the current schema can hold such a reference to a workspace's content (conversion
/// lineage, the obvious candidate, is bound to the source object's own scope by trigger), so the
/// scratch database gets a table that stands in for one: a `RESTRICT` reference to the workspace
/// and to a page. The delete path removes the page and the events first, so the refusal also
/// proves the transaction is all-or-nothing.
#[tokio::test]
async fn a_workspace_retained_by_an_outside_restrict_reference_is_refused_and_untouched() {
    let scratch = Scratch::new("refusal").await;
    let state = state_for(scratch.db.clone());
    let doomed = populate(&state, "doomed").await;
    let db = &scratch.db;
    exec(
        db,
        "CREATE TABLE external_hold (id uuid PRIMARY KEY, workspace_id uuid NOT NULL REFERENCES workspaces(id) \
         ON DELETE RESTRICT)",
        vec![],
    )
    .await;
    exec(
        db,
        "INSERT INTO external_hold (id, workspace_id) VALUES ($1, $2)",
        vec![Uuid::new_v4().into(), doomed.workspace.into()],
    )
    .await;
    let before = counts(db, doomed.workspace).await;
    assert_all_present(&before, "doomed workspace");

    let (_, body) =
        envelope(delete_workspace(State(state.clone()), claims(doomed.owner), Path(doomed.workspace)).await).await;
    assert_eq!(body["code"], 400, "{body}");
    assert_eq!(body["error_code"], "invalid_update", "{body}");
    assert_eq!(body["details"]["reason"], "retained_by_external_reference", "{body}");
    assert_ne!(body["message"], "database error", "{body}");
    assert_eq!(
        counts(db, doomed.workspace).await,
        before,
        "a refused delete must change nothing"
    );

    // Once the outside reference is gone the same request goes through.
    exec(db, "DELETE FROM external_hold", vec![]).await;
    let (_, body) =
        envelope(delete_workspace(State(state.clone()), claims(doomed.owner), Path(doomed.workspace)).await).await;
    assert_eq!(body["code"], 0, "{body}");
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM workspaces WHERE id = $1",
            vec![doomed.workspace.into()]
        )
        .await,
        0
    );
    scratch.close().await;
}

/// A constraint violation on a delete path maps to a typed refusal, never to the generic
/// `database error` / 500 envelope the API used to answer with.
#[tokio::test]
async fn a_constraint_violation_while_deleting_never_surfaces_as_a_generic_database_error() {
    let scratch = Scratch::new("mapping").await;
    let state = state_for(scratch.db.clone());
    let fixture = populate(&state, "doomed").await;
    let db = &scratch.db;

    // A genuine driver error: the very violation that was reported in production.
    let violation = || async {
        ApiError::Database(
            db.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "DELETE FROM business_events WHERE id = $1",
                vec![fixture.accepted_event.into()],
            ))
            .await
            .expect_err("collab_updates still references the event"),
        )
    };
    let unmapped = violation().await;
    assert_eq!(unmapped.database_sqlstate().as_deref(), Some("23503"));
    let (_, body) = envelope::<axum::response::Response>(Err(unmapped)).await;
    assert_eq!(
        body["message"], "database error",
        "the unmapped error is the one this change replaces on the delete path"
    );

    for what in ["workspace", "project"] {
        let mapped = api::services::teardown::refuse_blocked(violation().await, what);
        let (_, body) = envelope::<axum::response::Response>(Err(mapped)).await;
        assert_eq!(body["code"], 400, "{body}");
        assert_ne!(body["code"], 500, "{body}");
        assert_eq!(body["error_code"], "invalid_update", "{body}");
        assert_eq!(body["details"]["reason"], "retained_by_external_reference", "{body}");
        let message = body["message"].as_str().expect("message");
        assert!(
            message.contains(what) && !message.contains("business_events"),
            "{message}"
        );
    }

    // A failure that is not a constraint violation keeps its meaning.
    let other = ApiError::Internal;
    assert!(api::services::teardown::refuse_blocked(other, "workspace").is_internal_failure());
    scratch.close().await;
}

/// The third neighbour: deleting a user is a deactivation, and a user who owns personal Flow
/// content is refused with a typed 409 before anything changes. Pinned, not changed.
#[tokio::test]
async fn deleting_a_user_who_owns_a_personal_flow_scope_is_refused_with_a_typed_error() {
    let scratch = Scratch::new("user").await;
    let state = state_for(scratch.db.clone());
    let db = &scratch.db;
    let owner = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'p', 'user', true)",
        vec![owner.into(), format!("{owner}@personal.test").into()],
    )
    .await;
    exec(
        db,
        "UPDATE flow_runtime_capabilities SET personal_provisioning = true, personal_writes = true WHERE singleton",
        vec![],
    )
    .await;
    scope::personal_home(db, owner).await.expect("personal home");

    let (_, body) = envelope(delete_user(State(state.clone()), Path(owner)).await).await;
    assert_eq!(body["code"], 409, "{body}");
    assert_eq!(body["error_code"], "ownership_disposition_required", "{body}");
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM users WHERE id = $1 AND is_active",
            vec![owner.into()]
        )
        .await,
        1,
        "a refused delete leaves the account active"
    );
    scratch.close().await;
}
