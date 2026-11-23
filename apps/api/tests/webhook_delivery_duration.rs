//! The delivery log reports the time a legacy webhook delivery really took.
//!
//! A real local receiver that answers after 200 ms, a real `PostgreSQL` with every migration
//! applied, and the real `trigger_webhooks`. Skipped without `OPENPR_TEST_DATABASE_URL`.
//!
//! This is its own test binary because the outbound allowlist (which admits the loopback receiver)
//! is process-wide and can be installed only once.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stderr)]

use std::path::PathBuf;
use std::time::Duration;

use api::webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks};
use platform::{
    app::AppState,
    config::{
        AiTasksConfig, AppConfig, AuditConfig, AuthConfig, DatabaseConfig, FlowConfig, LoggingConfig, McpConfig,
        MigrationsConfig, OpenPrConfig, OutboundConfig, Secret, ServerConfig, StorageBackend, StorageConfig,
    },
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use uuid::Uuid;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const RECEIVER_DELAY: Duration = Duration::from_millis(200);

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

async fn scratch_database(admin_url: &str, name: &str) -> DatabaseConnection {
    let admin = Database::connect(admin_url)
        .await
        .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));
    admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .await
        .expect("scratch database reset");
    admin
        .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
        .await
        .expect("scratch database created");
    let (prefix, _) = admin_url.rsplit_once('/').expect("a database url has a path");
    let db = Database::connect(&format!("{prefix}/{name}"))
        .await
        .expect("scratch database connects");
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .expect("migrations directory is readable")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    for path in files {
        let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
        db.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
    }
    db
}

fn install_config(receiver_port: u16) {
    api::config::install(&OpenPrConfig {
        origin: PathBuf::from("webhook-delivery-duration"),
        server: ServerConfig::default(),
        database: DatabaseConfig::default(),
        auth: AuthConfig::default(),
        logging: LoggingConfig::default(),
        storage: StorageConfig {
            backend: StorageBackend::Local,
            dir: PathBuf::from("."),
            s3: None,
        },
        audit: AuditConfig::default(),
        ai_tasks: AiTasksConfig::default(),
        migrations: MigrationsConfig::default(),
        outbound: OutboundConfig {
            allowed_hosts: vec![format!("127.0.0.1:{receiver_port}")],
            allow_private: false,
        },
        mcp: McpConfig::default(),
        flow: FlowConfig::default(),
        oauth: None,
        inbound: platform::config::InboundConfig::default(),
    })
    .expect("the configuration is installed once");
}

#[tokio::test]
async fn delivery_log_reports_the_measured_duration() {
    let Ok(admin_url) = std::env::var(TEST_DATABASE_URL_ENV) else {
        eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
        return;
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("addr").port();
    install_config(port);
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 64 * 1024];
                let _ = socket.read(&mut buffer).await;
                tokio::time::sleep(RECEIVER_DELAY).await;
                let _ = socket
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                    .await;
            });
        }
    });

    let db = scratch_database(&admin_url, "sylvode_webhook_delivery_duration").await;
    let state = AppState {
        cfg: AppConfig {
            app_name: "webhook-delivery-duration".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("webhook-delivery-duration-secret"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            default_author_id: None,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db: db.clone(),
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    };

    let [user, workspace, project, issue, webhook] = std::array::from_fn(|_| Uuid::new_v4());
    exec(
        &db,
        "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'actor', 'user', true)",
        vec![user.into(), format!("{user}@duration.test").into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'duration', $3)",
        vec![workspace.into(), format!("ws-{workspace}").into(), user.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
         VALUES ($1, $2, 'DUR', 'duration', $3, 'code_project')",
        vec![project.into(), workspace.into(), user.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO work_items (id, project_id, title, state, created_by) VALUES ($1, $2, 'timed', 'todo', $3)",
        vec![issue.into(), project.into(), user.into()],
    )
    .await;
    exec(
        &db,
        "INSERT INTO webhooks (id, workspace_id, name, url, secret, events, active, created_by) \
         VALUES ($1, $2, 'receiver', $3, 'test-secret', '[\"issue.created\"]'::jsonb, true, $4)",
        vec![
            webhook.into(),
            workspace.into(),
            format!("http://127.0.0.1:{port}/hook").into(),
            user.into(),
        ],
    )
    .await;

    trigger_webhooks(
        state,
        TriggerContext {
            event: WebhookEvent::IssueCreated,
            workspace_id: workspace,
            project_id: project,
            actor_id: user,
            issue_id: Some(issue),
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: None,
        },
    );

    let mut logged = None;
    for _ in 0..200 {
        let row = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT duration_ms, response_status, error FROM webhook_deliveries WHERE webhook_id = $1",
                vec![webhook.into()],
            ))
            .await
            .expect("the log is readable");
        if let Some(row) = row {
            logged = Some((
                row.try_get::<Option<i64>>("", "duration_ms").expect("duration_ms"),
                row.try_get::<Option<i32>>("", "response_status").expect("status"),
                row.try_get::<Option<String>>("", "error").expect("error"),
            ));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (duration_ms, status, error) = logged.expect("the delivery was logged");
    assert_eq!(status, Some(200), "delivered: {error:?}");
    let measured = duration_ms.expect("a delivered attempt has a measured duration");
    assert!(
        measured >= i64::try_from(RECEIVER_DELAY.as_millis()).expect("fits"),
        "the receiver took 200 ms, the log says {measured} ms"
    );
    assert!(measured < 5_000, "{measured} ms is not a plausible measurement");
}
