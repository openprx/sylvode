//! `work-items create` against the real API handler and a real database: the state a new work
//! item gets when the command names none.
//!
//! The MCP tool `work_items.create` leaves `state` unset so the API applies the project
//! workflow's initial state, which is not always `backlog`. The CLI used to send
//! `state=backlog` on every create, so for a project whose workflow starts elsewhere the item
//! landed in the wrong state, and for one with no `backlog` state at all the create failed. Two
//! workflows reproduce both, and `--state` is checked to still override.
#![allow(
    clippy::print_stderr,
    reason = "an explicit unavailable-environment notice when no test database is configured"
)]

mod support;

use std::{error::Error, path::Path, process::Output, time::Duration};

use api::{
    middleware::bot_auth::bot_or_user_auth_middleware,
    routes::{context::get_project_agent_policy, issue::create_issue},
};
use axum::{
    Router, middleware,
    routing::{get, post},
};
use platform::{
    app::AppState,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::{ConfigFile, McpSettings, write_config};
use tokio::process::Command;
use uuid::Uuid;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const TOKEN: &str = "opr_work_items_workflow_state_cli";

type TestResult = Result<(), Box<dyn Error>>;

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    async fn create() -> Result<Option<Self>, Box<dyn Error>> {
        let Ok(admin_url) = std::env::var(TEST_DATABASE_URL_ENV) else {
            return Ok(None);
        };
        let admin = Database::connect(&admin_url).await?;
        let name = format!("sylvode_work_item_state_{}", Uuid::new_v4().simple());
        admin.execute_unprepared(&format!("CREATE DATABASE \"{name}\"")).await?;
        let (prefix, _) = admin_url
            .rsplit_once('/')
            .ok_or("test database URL has no database component")?;
        drop(admin);
        let mut options = ConnectOptions::new(format!("{prefix}/{name}"));
        options.max_connections(5).acquire_timeout(Duration::from_mins(3));
        let db = Database::connect(options).await?;
        let mut migrations: Vec<_> = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations"))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
            .collect();
        migrations.sort();
        for path in migrations {
            db.execute_unprepared(&std::fs::read_to_string(path)?).await?;
        }
        Ok(Some(Self { db, name, admin_url }))
    }

    async fn drop_self(self) {
        let Self { db, name, admin_url } = self;
        drop(db);
        if let Ok(admin) = Database::connect(admin_url).await {
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }
}

fn state_for(db: DatabaseConnection) -> AppState {
    AppState {
        cfg: AppConfig {
            app_name: "work-item-state-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("work-item-state-test-secret"),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            default_author_id: None,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db,
        flow_permission_cache: platform::app::FlowPermissionCacheSlot::default(),
    }
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> TestResult {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?;
    Ok(())
}

/// A workspace, its owner, and a `cli` bot with write access, created the way
/// `POST /workspaces/{id}/bots` creates one.
async fn seed_workspace(db: &DatabaseConnection) -> Result<(Uuid, Uuid), Box<dyn Error>> {
    let workspace_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    let bot_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO users (id,email,password_hash,name,role,is_active) VALUES ($1,$2,'!','Owner','user',true)",
        vec![owner_id.into(), format!("{owner_id}@state.test").into()],
    )
    .await?;
    exec(
        db,
        "INSERT INTO workspaces (id,slug,name,created_by) VALUES ($1,$2,'State',$3)",
        vec![
            workspace_id.into(),
            format!("state-{workspace_id}").into(),
            owner_id.into(),
        ],
    )
    .await?;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id,user_id,role) VALUES ($1,$2,'owner')",
        vec![workspace_id.into(), owner_id.into()],
    )
    .await?;
    exec(
        db,
        "INSERT INTO users (id,email,password_hash,name,role,is_active,entity_type,agent_type) \
         VALUES ($1,$2,'!','CLI bot','user',true,'bot_mcp','mcp')",
        vec![bot_id.into(), format!("{bot_id}@bot.state.test").into()],
    )
    .await?;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id,user_id,role) VALUES ($1,$2,'member')",
        vec![workspace_id.into(), bot_id.into()],
    )
    .await?;
    let token_hash = hex::encode(Sha256::digest(TOKEN.as_bytes()));
    exec(
        db,
        "INSERT INTO workspace_bots (id,workspace_id,name,token_hash,token_prefix,permissions,created_by,transport_surface) \
         VALUES ($1,$2,'CLI bot',$3,$4,$5,$6,'cli')",
        vec![
            bot_id.into(),
            workspace_id.into(),
            token_hash.into(),
            TOKEN.get(..8).ok_or("token shorter than its prefix")?.into(),
            json!(["read", "write"]).into(),
            owner_id.into(),
        ],
    )
    .await?;
    Ok((workspace_id, owner_id))
}

/// A project bound to its own workflow whose states are `(key, is_initial)` in position order.
async fn seed_project(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    owner_id: Uuid,
    key: &str,
    states: &[(&str, bool)],
) -> Result<Uuid, Box<dyn Error>> {
    let workflow_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO workflows (id,workspace_id,name) VALUES ($1,$2,$3)",
        vec![
            workflow_id.into(),
            workspace_id.into(),
            format!("{key} workflow").into(),
        ],
    )
    .await?;
    for (position, (state, initial)) in (1_i32..).zip(states) {
        exec(
            db,
            "INSERT INTO workflow_states (workflow_id,key,display_name,position,is_initial) VALUES ($1,$2,$2,$3,$4)",
            vec![workflow_id.into(), (*state).into(), position.into(), (*initial).into()],
        )
        .await?;
    }
    let project_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO projects (id,workspace_id,key,name,created_by,workflow_id) VALUES ($1,$2,$3,$3,$4,$5)",
        vec![
            project_id.into(),
            workspace_id.into(),
            key.into(),
            owner_id.into(),
            workflow_id.into(),
        ],
    )
    .await?;
    Ok(project_id)
}

async fn run(binary: &str, config: &ConfigFile, args: &[&str]) -> Result<Output, Box<dyn Error>> {
    let cwd: &Path = config.path().parent().ok_or("config path has no parent")?;
    Ok(tokio::time::timeout(
        Duration::from_mins(1),
        Command::new(binary)
            .args(args)
            .arg("--config")
            .arg(config.path())
            .current_dir(cwd)
            .output(),
    )
    .await??)
}

async fn stored_state(db: &DatabaseConnection, project_id: Uuid, title: &str) -> Result<String, Box<dyn Error>> {
    let row = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT state FROM work_items WHERE project_id = $1 AND title = $2",
            vec![project_id.into(), title.into()],
        ))
        .await?
        .ok_or_else(|| format!("no work item {title:?} was created"))?;
    Ok(row.try_get::<String>("", "state")?)
}

/// Creates `title` in `project` under both executables and returns the state each stored.
async fn create(
    db: &DatabaseConnection,
    config: &ConfigFile,
    project: Uuid,
    title: &str,
    extra: &[&str],
) -> Result<Vec<String>, Box<dyn Error>> {
    let mut states = Vec::new();
    for (binary, suffix) in [
        (env!("CARGO_BIN_EXE_sylvode"), "sylvode"),
        (env!("CARGO_BIN_EXE_mcp-server"), "mcp-server"),
    ] {
        let title = format!("{title} ({suffix})");
        let project = project.to_string();
        let mut args = vec!["work-items", "create", "--project", &project, "--title", &title];
        args.extend_from_slice(extra);
        let output = run(binary, config, &args).await?;
        assert!(
            output.status.success(),
            "{suffix} work-items create {extra:?} failed with {:?}\nstdout: {}\nstderr: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let printed: Value = serde_json::from_slice(&output.stdout)?;
        let stored = stored_state(db, project.parse()?, &title).await?;
        assert_eq!(
            printed.pointer("/state").or_else(|| printed.pointer("/data/state")),
            Some(&json!(stored)),
            "{suffix} printed a state other than the stored one: {printed}"
        );
        states.push(stored);
    }
    Ok(states)
}

#[tokio::test]
async fn work_items_create_applies_the_project_workflows_initial_state() -> TestResult {
    let Some(scratch) = Scratch::create().await? else {
        eprintln!("SKIPPED (no database): set {TEST_DATABASE_URL_ENV} to run this test");
        return Ok(());
    };
    let db = scratch.db.clone();
    let (workspace_id, owner_id) = seed_workspace(&db).await?;
    // No `backlog` state at all: forcing it is not a wrong state but a refused create.
    let triage = seed_project(
        &db,
        workspace_id,
        owner_id,
        "TRI",
        &[("triage", true), ("doing", false), ("done", false)],
    )
    .await?;
    // `backlog` exists but the workflow starts at `todo`.
    let todo_first = seed_project(
        &db,
        workspace_id,
        owner_id,
        "TODO",
        &[("backlog", false), ("todo", true), ("done", false)],
    )
    .await?;

    let state = state_for(db.clone());
    let router = Router::new()
        .route("/api/v1/projects/{project_id}/issues", post(create_issue))
        // Every tool call is authorized against the project's agent policy first.
        .route(
            "/api/v1/projects/{project_id}/agent-policy",
            get(get_project_agent_policy),
        )
        .layer(middleware::from_fn_with_state(state.clone(), bot_or_user_auth_middleware))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    let api_url = format!("http://{address}");
    let workspace = workspace_id.to_string();
    let config = write_config(&McpSettings {
        api_url: &api_url,
        bot_token: Some(TOKEN),
        workspace_id: &workspace,
        transport: Some("stdio"),
        bind_addr: None,
    })?;

    let outcome = async {
        assert_eq!(
            create(&db, &config, triage, "No backlog here", &[]).await?,
            ["triage", "triage"],
            "a workflow without backlog must get its own initial state"
        );
        assert_eq!(
            create(&db, &config, todo_first, "Starts at todo", &[]).await?,
            ["todo", "todo"],
            "a workflow whose initial state is todo must not get backlog"
        );
        assert_eq!(
            create(&db, &config, triage, "Explicit state", &["--state", "doing"]).await?,
            ["doing", "doing"],
            "--state still overrides the initial state"
        );
        Ok::<(), Box<dyn Error>>(())
    }
    .await;

    server.abort();
    scratch.drop_self().await;
    outcome
}
