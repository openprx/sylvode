//! `objects.export` through the production MCP tool dispatch honours Record field secrecy
//! (`rest-api-v1.md` v0.6 "导出与字段保密"): the MCP tool posts to the same REST endpoint, so a
//! `json` export of a secret Record carries the redacted `semantic_content` and a `package` export
//! is refused with `policy_rejected{reason:"field_secrecy"}`.
#![allow(clippy::indexing_slicing)]
#![allow(
    clippy::print_stderr,
    reason = "the harness prints an explicit notice when no test database is configured"
)]

use std::{error::Error, time::Duration};

use api::{
    flow::{
        command::{
            CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput, create_object, execute_command,
            set_flow_feature,
        },
        event_origin::{CommandOrigin, EventSurface},
        model::AcceptedChange,
    },
    middleware::bot_auth::bot_or_user_auth_middleware,
    routes::flow::{get_flow_export, get_flow_export_artifact, get_flow_object, post_flow_object_export},
};
use axum::{
    Router, middleware,
    routing::{get, post},
};
use mcp_server::{
    client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_STDIO},
    protocol::{CallToolResult, ToolContent},
    server::McpServer,
};
use platform::{
    app::AppState,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const BOT_TOKEN: &str = "opr_export_secrecy_mcp_token";

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
        let name = format!("sylvode_export_secrecy_{}", Uuid::new_v4().simple());
        admin.execute_unprepared(&format!("CREATE DATABASE \"{name}\"")).await?;
        let (prefix, _) = admin_url
            .rsplit_once('/')
            .ok_or("test database URL has no database component")?;
        drop(admin);
        let mut options = ConnectOptions::new(format!("{prefix}/{name}"));
        options.acquire_timeout(Duration::from_mins(3));
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
            app_name: "export-secrecy-mcp-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("export-secrecy-mcp-secret"),
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

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> Result<(), Box<dyn Error>> {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await?;
    Ok(())
}

async fn seed_workspace(state: &AppState) -> Result<(Uuid, Uuid), Box<dyn Error>> {
    let workspace_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    exec(
        &state.db,
        "INSERT INTO users (id,email,password_hash,name,role,is_active) VALUES ($1,$2,'!','owner','user',true)",
        vec![owner_id.into(), format!("{owner_id}@export-secrecy.test").into()],
    )
    .await?;
    exec(
        &state.db,
        "INSERT INTO workspaces (id,slug,name,created_by) VALUES ($1,$2,'export secrecy',$3)",
        vec![
            workspace_id.into(),
            format!("export-secrecy-{workspace_id}").into(),
            owner_id.into(),
        ],
    )
    .await?;
    exec(
        &state.db,
        "INSERT INTO workspace_members (workspace_id,user_id,role) VALUES ($1,$2,'owner')",
        vec![workspace_id.into(), owner_id.into()],
    )
    .await?;
    set_flow_feature(
        state,
        SetFlowFeatureInput {
            workspace_id,
            actor_id: owner_id,
            actor_is_bot: false,
            enabled: Some(true),
            default_member_level: Some("edit".to_string()),
            idempotency_key: "enable-export-secrecy".to_string(),
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        },
    )
    .await?;
    let token_hash = hex::encode(Sha256::digest(BOT_TOKEN.as_bytes()));
    exec(
        &state.db,
        "INSERT INTO workspace_bots (id,workspace_id,name,token_hash,token_prefix,permissions,created_by,transport_surface) \
         VALUES ($1,$2,'export secrecy bot',$3,$4,$5,$6,'mcp_stdio')",
        vec![
            Uuid::new_v4().into(),
            workspace_id.into(),
            token_hash.into(),
            BOT_TOKEN[..8].into(),
            json!(["read", "write"]).into(),
            owner_id.into(),
        ],
    )
    .await?;
    Ok((workspace_id, owner_id))
}

async fn command(
    state: &AppState,
    owner_id: Uuid,
    object_id: Uuid,
    command_type: &str,
    payload: Value,
) -> Result<AcceptedChange, Box<dyn Error>> {
    let key = Uuid::new_v4().to_string();
    let mut attempts = 0;
    loop {
        let result = execute_command(
            state,
            ExecuteCommandInput {
                object_id,
                actor_id: owner_id,
                principal_kind: "user".to_string(),
                role: "owner".to_string(),
                command_type: command_type.to_string(),
                payload: payload.clone(),
                expected_frontier: None,
                idempotency_key: key.clone(),
                message: None,
                origin_client_id: "export-secrecy-fixture".to_string(),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await;
        match result {
            Ok(change) => return Ok(change),
            Err(error) if attempts < 5 && error.kind().stable_code() == "server_draining" => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(format!("{command_type} failed: {error:?}").into()),
        }
    }
}

fn export_router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/flow/objects/{object_id}", get(get_flow_object))
        .route(
            "/api/v1/flow/objects/{object_id}/exports",
            post(post_flow_object_export),
        )
        .route("/api/v1/flow/exports/{job_id}", get(get_flow_export))
        .route("/api/v1/flow/exports/{job_id}/artifact", get(get_flow_export_artifact))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            bot_or_user_auth_middleware,
        ))
        .with_state(state)
}

fn result_text(result: &CallToolResult) -> Result<&str, Box<dyn Error>> {
    let Some(ToolContent::Text { text }) = result.content.first() else {
        return Err("MCP result has no text".into());
    };
    Ok(text)
}

#[tokio::test]
async fn mcp_object_export_of_a_secret_record_is_redacted_and_its_package_refused() -> Result<(), Box<dyn Error>> {
    let Some(scratch) = Scratch::create().await? else {
        eprintln!("SKIPPED (no database): set {TEST_DATABASE_URL_ENV} to run this test");
        return Ok(());
    };
    let state = state_for(scratch.db.clone());
    let (workspace_id, owner_id) = seed_workspace(&state).await?;
    let collection_id = create_object(
        &state,
        CreateObjectInput {
            workspace_id,
            actor_id: owner_id,
            actor_is_bot: false,
            object_type: "collection".to_string(),
            project_id: None,
            parent_object_id: None,
            title: "Restricted export".to_string(),
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        },
    )
    .await?
    .object
    .id;
    let diagnosis = Uuid::new_v4();
    command(
        &state,
        owner_id,
        collection_id,
        "field_create",
        json!({"field_id": diagnosis, "label": "Diagnosis", "field_type": "text", "config": {"restricted": true}}),
    )
    .await?;
    let record_id = command(
        &state,
        owner_id,
        collection_id,
        "record_create",
        json!({"properties": {diagnosis.to_string(): "SECRETDIAG"}}),
    )
    .await?
    .object
    .id;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let server_task = tokio::spawn(async move {
        let _ = axum::serve(listener, export_router(state)).await;
    });
    let server = McpServer::new(OpenPrClient::new(ClientConfig {
        base_url: format!("http://{address}"),
        credential: Some(Secret::new(BOT_TOKEN)),
        workspace_id: workspace_id.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?);

    let exported = server
        .call_tool(
            "objects.export",
            json!({"object_id": record_id, "format": "json", "idempotency_key": "secret-json"}),
        )
        .await;
    assert!(exported.is_error.is_none(), "json export runs: {:?}", exported.content);
    let receipt: Value = serde_json::from_str(result_text(&exported)?)?;
    let job_id = receipt["job_id"].as_str().ok_or("export job id missing")?;
    let artifact = reqwest::Client::new()
        .get(format!("http://{address}/api/v1/flow/exports/{job_id}/artifact"))
        .bearer_auth(BOT_TOKEN)
        .header("x-sylvode-mcp-surface", TRANSPORT_LABEL_STDIO)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let body: Value = serde_json::from_slice(&artifact)?;
    assert!(
        !String::from_utf8_lossy(&artifact).contains("SECRETDIAG"),
        "the MCP json export leaked a restricted value: {body}"
    );
    assert_eq!(body["semantic_content"], json!({}), "{body}");

    let refused = server
        .call_tool(
            "objects.export",
            json!({"object_id": record_id, "format": "package", "idempotency_key": "secret-package"}),
        )
        .await;
    assert_eq!(refused.is_error, Some(true), "{:?}", refused.content);
    let text = result_text(&refused)?;
    assert!(
        text.contains("policy_rejected") && text.contains("field_secrecy"),
        "the MCP package export is refused with policy_rejected/field_secrecy: {text}"
    );

    server_task.abort();
    scratch.drop_self().await;
    Ok(())
}
