//! Flow v0.6 Collection coverage through the shipped `sylvode` binary and the live MCP handler.
//!
//! The first tests capture the REST requests the two clients make. The `after_block_id` tests at
//! the end also run both clients against the real API handlers and a real database (set
//! `OPENPR_TEST_DATABASE_URL`), and read the Page's block order back from its document.
#![allow(
    clippy::print_stderr,
    reason = "the harness prints an explicit notice when no test database is configured"
)]

mod support;

use api::{
    flow::{
        collab::bootstrap,
        command::{
            CreateObjectInput, ExecuteCommandInput, SetFlowFeatureInput, create_object, execute_command,
            set_flow_feature,
        },
        event_origin::{CommandOrigin, EventSurface},
    },
    middleware::bot_auth::bot_or_user_auth_middleware,
    routes::flow::post_flow_object_command,
};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    middleware,
    routing::post,
};
use collab_core::{CollabEngine, LoroCollabEngine};
use mcp_server::{
    client::{ClientConfig, OpenPrClient, TRANSPORT_LABEL_STDIO},
    protocol::{CallToolResult, ToolContent},
    server::McpServer,
    tools::objects::create_flow_object,
};
use platform::{
    app::AppState,
    config::{AppConfig, Secret},
};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{error::Error, path::Path, process::Output, sync::Arc, time::Duration};
use support::{ConfigFile, McpSettings, write_config};
use tokio::process::Command;
use uuid::Uuid;

type TestResult = Result<(), Box<dyn Error>>;
type Calls = Arc<tokio::sync::Mutex<Vec<CapturedCall>>>;

const WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
const COLLECTION: &str = "22222222-2222-4222-8222-222222222222";
const PAGE: &str = "33333333-3333-4333-8333-333333333333";
const RECORD: &str = "44444444-4444-4444-8444-444444444444";
const FIELD: &str = "55555555-5555-4555-8555-555555555555";
const TOKEN: &str = "opr_flow_v06_collection_e2e";

#[derive(Debug, Clone, PartialEq)]
struct CapturedCall {
    method: String,
    uri: String,
    body: Value,
}

async fn capture(State(calls): State<Calls>, request: Request) -> Json<Value> {
    let method = request.method().to_string();
    let uri = request.uri().to_string();
    let body = to_bytes(request.into_body(), 1_048_576)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    calls.lock().await.push(CapturedCall {
        method: method.clone(),
        uri: uri.clone(),
        body,
    });

    let data = if method == "GET" && uri == format!("/api/v1/flow/objects/{RECORD}") {
        json!({"id": RECORD, "parent_id": COLLECTION, "object_type": "record"})
    } else {
        json!({"accepted": true})
    };
    Json(json!({"code": 0, "data": data}))
}

async fn api() -> TestResultValue<(String, Calls)> {
    let calls: Calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let router = Router::new().fallback(capture).with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok((format!("http://{address}"), calls))
}

type TestResultValue<T> = Result<T, Box<dyn Error>>;

fn config(api_url: &str) -> TestResultValue<ConfigFile> {
    write_config(&McpSettings {
        api_url,
        bot_token: Some(TOKEN),
        workspace_id: WORKSPACE,
        transport: Some("stdio"),
        bind_addr: None,
    })
}

async fn run(config: &ConfigFile, args: &[&str]) -> TestResultValue<Output> {
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    Ok(tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(env!("CARGO_BIN_EXE_sylvode"))
            .arg("--config")
            .arg(config.path())
            .args(args)
            .current_dir(cwd)
            .output(),
    )
    .await??)
}

fn assert_success(output: &Output, command: &str) -> TestResult {
    assert!(
        output.status.success(),
        "{command} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(envelope.pointer("/schema_version"), Some(&json!("sylvode.cli.v1")));
    assert_eq!(envelope.pointer("/ok"), Some(&json!(true)));
    assert_eq!(envelope.pointer("/command"), Some(&json!(command)));
    Ok(())
}

fn write_json(dir: &Path, name: &str, value: &Value) -> TestResultValue<String> {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec(value)?)?;
    Ok(path.to_string_lossy().to_string())
}

#[tokio::test]
async fn shipped_cli_covers_collection_create_describe_query_and_record_write_paths() -> TestResult {
    let (api_url, calls) = api().await?;
    let config = config(&api_url)?;
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let schema = write_json(
        cwd,
        "schema.json",
        &json!({
            "initial_fields": [{"field_id": FIELD, "label": "Score", "field_type": "number"}],
            "initial_view": {"view_id": "66666666-6666-4666-8666-666666666666", "label": "All"}
        }),
    )?;
    let query = write_json(
        cwd,
        "query.json",
        &json!({"filter": {"field_id": FIELD, "op": "gte", "value": 10}, "limit": 20}),
    )?;
    let values = write_json(cwd, "values.json", &json!({(FIELD): 42}))?;

    let invocations: Vec<(&str, Vec<&str>)> = vec![
        (
            "objects.create",
            vec![
                "objects",
                "create",
                "--workspace",
                WORKSPACE,
                "--type",
                "collection",
                "--title",
                "Scores",
                "--schema-file",
                &schema,
                "--idempotency-key",
                "standalone-create",
            ],
        ),
        (
            "objects.create",
            vec![
                "objects",
                "create",
                "--workspace",
                WORKSPACE,
                "--type",
                "collection",
                "--title",
                "Embedded",
                "--embed-page",
                PAGE,
                "--schema-file",
                &schema,
                "--idempotency-key",
                "embedded-create",
            ],
        ),
        ("collections.describe", vec!["collections", "describe", COLLECTION]),
        (
            "collections.query",
            vec![
                "collections",
                "query",
                COLLECTION,
                "--query-file",
                &query,
                "--cursor",
                "opaque-cursor",
            ],
        ),
        (
            "records.create",
            vec![
                "records",
                "create",
                "--collection",
                COLLECTION,
                "--values-file",
                &values,
                "--body",
                "row",
                "--idempotency-key",
                "record-create",
            ],
        ),
        (
            "records.patch",
            vec![
                "records",
                "patch",
                RECORD,
                "--values-file",
                &values,
                "--body",
                "updated",
                "--idempotency-key",
                "record-patch",
            ],
        ),
    ];
    for (command, args) in invocations {
        assert_success(&run(&config, &args).await?, command)?;
    }

    let calls = calls.lock().await.clone();
    assert_eq!(
        calls.len(),
        7,
        "record patch performs one ownership read and one command write"
    );
    let [
        standalone,
        embedded,
        describe,
        query_call,
        create_record,
        get_record,
        patch_record,
    ] = calls.as_slice()
    else {
        return Err("expected the seven captured Collection calls".into());
    };
    assert_eq!(standalone.uri, format!("/api/v1/workspaces/{WORKSPACE}/flow/objects"));
    assert_eq!(
        standalone.body.pointer("/initial_fields/0/field_id"),
        Some(&json!(FIELD))
    );
    assert_eq!(embedded.uri, format!("/api/v1/flow/objects/{PAGE}/commands"));
    assert_eq!(
        embedded.body.pointer("/command/type"),
        Some(&json!("create_collection_embed"))
    );
    assert_eq!(describe.uri, format!("/api/v1/flow/collections/{COLLECTION}"));
    assert_eq!(query_call.uri, format!("/api/v1/flow/collections/{COLLECTION}/query"));
    assert_eq!(query_call.body.pointer("/cursor"), Some(&json!("opaque-cursor")));
    assert_eq!(
        create_record.uri,
        format!("/api/v1/flow/collections/{COLLECTION}/records")
    );
    assert_eq!(
        create_record.body.pointer(&format!("/values_by_field_id/{FIELD}")),
        Some(&json!(42))
    );
    assert_eq!(get_record.uri, format!("/api/v1/flow/objects/{RECORD}"));
    assert_eq!(patch_record.uri, format!("/api/v1/flow/objects/{COLLECTION}/commands"));
    assert_eq!(patch_record.body.pointer("/command/type"), Some(&json!("record_patch")));
    Ok(())
}

#[tokio::test]
async fn mcp_cli_create_equivalence() -> TestResult {
    let (api_url, calls) = api().await?;
    let config = config(&api_url)?;
    let cwd = config.path().parent().ok_or("config path has no parent")?;
    let schema = json!({
        "initial_fields": [{"field_id": FIELD, "label": "Score", "field_type": "number"}],
        "initial_view": {"view_id": "66666666-6666-4666-8666-666666666666", "label": "All"}
    });
    let schema_file = write_json(cwd, "equivalent-schema.json", &schema)?;
    let client = OpenPrClient::new(ClientConfig {
        base_url: api_url,
        credential: Some(Secret::new(TOKEN)),
        workspace_id: WORKSPACE.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?;

    let mcp_result = create_flow_object(
        &client,
        json!({
            "workspace_id": WORKSPACE,
            "type": "collection",
            "title": "Same Collection",
            "initial_fields": schema.get("initial_fields").ok_or("schema has no initial_fields")?,
            "initial_view": schema.get("initial_view").ok_or("schema has no initial_view")?,
            "idempotency_key": "same-create-key"
        }),
    )
    .await;
    assert_eq!(mcp_result.is_error, None);
    let Some(ToolContent::Text { text }) = mcp_result.content.first() else {
        return Err("MCP create did not return JSON text".into());
    };
    assert_eq!(serde_json::from_str::<Value>(text)?, json!({"accepted": true}));

    assert_success(
        &run(
            &config,
            &[
                "objects",
                "create",
                "--workspace",
                WORKSPACE,
                "--type",
                "collection",
                "--title",
                "Same Collection",
                "--schema-file",
                &schema_file,
                "--idempotency-key",
                "same-create-key",
            ],
        )
        .await?,
        "objects.create",
    )?;

    let calls = calls.lock().await.clone();
    assert_eq!(calls.len(), 2);
    let [mcp_call, cli_call] = calls.as_slice() else {
        return Err("expected one MCP and one CLI call".into());
    };
    assert_eq!(
        mcp_call, cli_call,
        "MCP and shipped CLI must make the same REST request"
    );
    Ok(())
}

fn embed_args(extra: &Value) -> Value {
    let mut args = json!({
        "workspace_id": WORKSPACE,
        "type": "collection",
        "title": "Embedded",
        "embed_page_id": PAGE,
        "idempotency_key": "embed-key"
    });
    if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
        args.extend(extra.clone());
    }
    args
}

fn result_text(result: &CallToolResult) -> TestResultValue<&str> {
    let Some(ToolContent::Text { text }) = result.content.first() else {
        return Err("MCP result has no text".into());
    };
    Ok(text)
}

/// `objects.create` declares `after_block_id`, forwards it in the `create_collection_embed`
/// payload only when it is given, and refuses it without `embed_page_id` before any request.
#[tokio::test]
async fn mcp_objects_create_forwards_after_block_id_only_for_an_embed() -> TestResult {
    let schema = mcp_server::tools::objects::create_flow_object_tool().input_schema;
    assert_eq!(
        schema.pointer("/properties/after_block_id/type"),
        Some(&json!("string")),
        "{schema}"
    );
    assert!(
        schema
            .pointer("/required")
            .and_then(Value::as_array)
            .is_some_and(|required| !required.iter().any(|field| field == "after_block_id")),
        "after_block_id is optional"
    );

    let (api_url, calls) = api().await?;
    let client = OpenPrClient::new(ClientConfig {
        base_url: api_url,
        credential: Some(Secret::new(TOKEN)),
        workspace_id: WORKSPACE.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?;

    let placed = create_flow_object(&client, embed_args(&json!({"after_block_id": "block-1"}))).await;
    assert_eq!(placed.is_error, None, "{:?}", placed.content);
    let appended = create_flow_object(&client, embed_args(&json!({}))).await;
    assert_eq!(appended.is_error, None, "{:?}", appended.content);
    {
        let calls = calls.lock().await.clone();
        let [placed_call, appended_call] = calls.as_slice() else {
            return Err(format!("expected two embed calls, got {}", calls.len()).into());
        };
        assert_eq!(placed_call.uri, format!("/api/v1/flow/objects/{PAGE}/commands"));
        assert_eq!(
            placed_call.body.pointer("/command"),
            Some(&json!({
                "type": "create_collection_embed",
                "payload": {
                    "title": "Embedded", "initial_fields": [], "initial_view": null, "after_block_id": "block-1"
                }
            }))
        );
        // Without the argument the payload is what it was before the argument existed: no key.
        assert_eq!(
            appended_call.body.pointer("/command"),
            Some(&json!({
                "type": "create_collection_embed",
                "payload": {"title": "Embedded", "initial_fields": [], "initial_view": null}
            }))
        );
    }

    // Without `embed_page_id` there is no Page whose block it could name: refused locally, in
    // the wording of the neighbouring `embed_page_id` checks, for a Collection and for a Page.
    for object_type in ["collection", "page"] {
        let refused = create_flow_object(
            &client,
            json!({
                "workspace_id": WORKSPACE,
                "type": object_type,
                "title": "No host page",
                "after_block_id": "block-1",
                "idempotency_key": "no-host-key"
            }),
        )
        .await;
        assert_eq!(refused.is_error, Some(true), "{object_type}: {:?}", refused.content);
        assert_eq!(
            result_text(&refused)?,
            "Error: after_block_id is only valid with embed_page_id",
            "{object_type}"
        );
    }
    assert_eq!(calls.lock().await.len(), 2, "a refused call reached the API");
    Ok(())
}

/// `sylvode objects create --after-block` makes the same REST request as the MCP tool, and is a
/// usage error (exit 2, no request) without `--embed-page`.
#[tokio::test]
async fn cli_objects_create_after_block_matches_mcp_and_requires_embed_page() -> TestResult {
    let (api_url, calls) = api().await?;
    let config = config(&api_url)?;
    let client = OpenPrClient::new(ClientConfig {
        base_url: api_url,
        credential: Some(Secret::new(TOKEN)),
        workspace_id: WORKSPACE.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?;
    let mcp = create_flow_object(&client, embed_args(&json!({"after_block_id": "block-1"}))).await;
    assert_eq!(mcp.is_error, None, "{:?}", mcp.content);
    let embed = [
        "objects",
        "create",
        "--workspace",
        WORKSPACE,
        "--type",
        "collection",
        "--title",
        "Embedded",
        "--embed-page",
        PAGE,
        "--idempotency-key",
        "embed-key",
    ];
    let mut with_block = embed.to_vec();
    with_block.extend(["--after-block", "block-1"]);
    assert_success(&run(&config, &with_block).await?, "objects.create")?;
    assert_success(&run(&config, &embed).await?, "objects.create")?;
    {
        let calls = calls.lock().await.clone();
        let [mcp_call, cli_call, cli_plain] = calls.as_slice() else {
            return Err(format!("expected three embed calls, got {}", calls.len()).into());
        };
        assert_eq!(
            cli_call.body.pointer("/command/payload/after_block_id"),
            Some(&json!("block-1"))
        );
        assert_eq!(
            mcp_call, cli_call,
            "MCP and the shipped CLI must make the same REST request"
        );
        assert_eq!(
            cli_plain.body.pointer("/command/payload"),
            Some(&json!({"title": "Embedded", "initial_fields": [], "initial_view": null})),
            "without --after-block the payload has no after_block_id key"
        );
    }

    let refused = run(
        &config,
        &[
            "objects",
            "create",
            "--workspace",
            WORKSPACE,
            "--type",
            "collection",
            "--title",
            "No host page",
            "--after-block",
            "block-1",
            "--idempotency-key",
            "no-host-key",
        ],
    )
    .await?;
    assert_eq!(refused.status.code(), Some(2));
    let envelope: Value = serde_json::from_slice(&refused.stdout)?;
    assert_eq!(envelope.pointer("/error/code"), Some(&json!("usage_error")));
    assert_eq!(
        envelope.pointer("/error/message"),
        Some(&json!("--after-block requires --embed-page"))
    );
    assert_eq!(calls.lock().await.len(), 3, "a usage error reached the API");
    Ok(())
}

// -------------------------------------------------------------------------------------------------
// `after_block_id` against the real command handler and a real database
// -------------------------------------------------------------------------------------------------

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const MCP_BOT_TOKEN: &str = "opr_embed_after_block_mcp_token";
const CLI_BOT_TOKEN: &str = "opr_embed_after_block_cli_token";

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    async fn create() -> TestResultValue<Option<Self>> {
        let Ok(admin_url) = std::env::var(TEST_DATABASE_URL_ENV) else {
            return Ok(None);
        };
        let admin = Database::connect(&admin_url).await?;
        let name = format!("sylvode_embed_after_block_{}", Uuid::new_v4().simple());
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
            app_name: "embed-after-block-test".to_string(),
            bind_addr: "127.0.0.1:0".to_string(),
            database_url: Secret::new("postgres://unused/unused"),
            jwt_secret: Secret::new("embed-after-block-secret"),
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

/// One Flow-enabled workspace with an owner and one bot per client surface.
async fn seed_workspace(state: &AppState) -> TestResultValue<(Uuid, Uuid)> {
    let workspace_id = Uuid::new_v4();
    let owner_id = Uuid::new_v4();
    exec(
        &state.db,
        "INSERT INTO users (id,email,password_hash,name,role,is_active) VALUES ($1,$2,'!','owner','user',true)",
        vec![owner_id.into(), format!("{owner_id}@embed-after-block.test").into()],
    )
    .await?;
    exec(
        &state.db,
        "INSERT INTO workspaces (id,slug,name,created_by) VALUES ($1,$2,'embed after block',$3)",
        vec![
            workspace_id.into(),
            format!("embed-after-block-{workspace_id}").into(),
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
            idempotency_key: "enable-embed-after-block".to_string(),
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        },
    )
    .await?;
    for (token, surface) in [(MCP_BOT_TOKEN, "mcp_stdio"), (CLI_BOT_TOKEN, "cli")] {
        exec(
            &state.db,
            "INSERT INTO workspace_bots (id,workspace_id,name,token_hash,token_prefix,permissions,created_by,transport_surface) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8)",
            vec![
                Uuid::new_v4().into(),
                workspace_id.into(),
                format!("embed after block {surface}").into(),
                hex::encode(Sha256::digest(token.as_bytes())).into(),
                token.get(..8).ok_or("token is shorter than its prefix")?.into(),
                json!(["read", "write"]).into(),
                owner_id.into(),
                surface.into(),
            ],
        )
        .await?;
    }
    Ok((workspace_id, owner_id))
}

/// A Page whose root blocks are `b1`, `b2`, `b3`, in that order. Returns `(page_id, document_id)`.
async fn page_with_three_blocks(state: &AppState, workspace_id: Uuid, owner_id: Uuid) -> TestResultValue<(Uuid, Uuid)> {
    let page = create_object(
        state,
        CreateObjectInput {
            workspace_id,
            actor_id: owner_id,
            actor_is_bot: false,
            object_type: "page".to_string(),
            project_id: None,
            parent_object_id: None,
            title: "Embed host".to_string(),
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
            origin: CommandOrigin::first_request_from(EventSurface::Rest),
        },
    )
    .await?
    .object;
    for (index, block) in ["b1", "b2", "b3"].into_iter().enumerate() {
        execute_command(
            state,
            ExecuteCommandInput {
                object_id: page.id,
                actor_id: owner_id,
                principal_kind: "user".to_string(),
                role: "owner".to_string(),
                command_type: "insert_block".to_string(),
                payload: json!({"block_id": block, "index": index, "text": block}),
                expected_frontier: None,
                idempotency_key: Uuid::new_v4().to_string(),
                message: None,
                origin_client_id: "embed-after-block-fixture".to_string(),
                origin: CommandOrigin::first_request_from(EventSurface::Rest),
            },
        )
        .await
        .map_err(|error| format!("insert_block {block} failed: {error:?}"))?;
    }
    Ok((page.id, page.document_id))
}

/// The Page's live root blocks in document order, read from its stored document.
async fn root_blocks(state: &AppState, document_id: Uuid) -> TestResultValue<Vec<String>> {
    let boot = bootstrap::load(&state.db, document_id)
        .await
        .map_err(|error| format!("page document does not load: {error:?}"))?;
    let mut engine = LoroCollabEngine::load(&boot.snapshot)?;
    for update in &boot.tail_updates {
        engine.import_update(&update.bytes)?;
    }
    let snapshot = engine.semantic_snapshot()?;
    let mut roots: Vec<(String, String)> = snapshot
        .nodes
        .iter()
        .filter(|(_, node)| node.parent.is_none() && !node.deleted)
        .map(|(id, node)| (node.order_key.clone(), id.to_string()))
        .collect();
    roots.sort();
    Ok(roots.into_iter().map(|(_, id)| id).collect())
}

fn command_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/v1/flow/objects/{object_id}/commands",
            post(post_flow_object_command),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            bot_or_user_auth_middleware,
        ))
        .with_state(state)
}

async fn collection_count(state: &AppState) -> TestResultValue<i64> {
    Ok(state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM flow_objects WHERE object_type = 'collection'",
            vec![],
        ))
        .await?
        .ok_or("count row missing")?
        .try_get::<i64>("", "n")?)
}

fn block_id_of(result: &Value) -> TestResultValue<String> {
    Ok(result
        .pointer("/command_result/block_id")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("no command_result.block_id in {result}"))?
        .to_string())
}

/// A Page with three blocks, the real `POST /flow/objects/{id}/commands` handler and a real
/// database. Through the MCP tool: `after_block_id` of the first block puts the new embed block
/// right after it; without it the embed is appended as before; a block id the Page does not have
/// is the server's `invalid_update`, passed through, and writes nothing. The shipped CLI does the
/// same with `--after-block`.
#[tokio::test]
async fn mcp_and_cli_embed_a_collection_after_the_named_block_of_a_real_page() -> TestResult {
    let Some(scratch) = Scratch::create().await? else {
        eprintln!("SKIPPED (no database): set {TEST_DATABASE_URL_ENV} to run this test");
        return Ok(());
    };
    let state = state_for(scratch.db.clone());
    let (workspace_id, owner_id) = seed_workspace(&state).await?;
    let (page_id, document_id) = page_with_three_blocks(&state, workspace_id, owner_id).await?;
    assert_eq!(root_blocks(&state, document_id).await?, ["b1", "b2", "b3"]);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let api_url = format!("http://{}", listener.local_addr()?);
    let server_task = tokio::spawn({
        let router = command_router(state.clone());
        async move {
            let _ = axum::serve(listener, router).await;
        }
    });
    let server = McpServer::new(OpenPrClient::new(ClientConfig {
        base_url: api_url.clone(),
        credential: Some(Secret::new(MCP_BOT_TOKEN)),
        workspace_id: workspace_id.to_string(),
        transport_label: TRANSPORT_LABEL_STDIO,
    })?);
    let mcp_embed = |title: &'static str, key: &'static str, after: Option<&'static str>| {
        let mut args = json!({
            "workspace_id": workspace_id,
            "type": "collection",
            "title": title,
            "embed_page_id": page_id,
            "idempotency_key": key
        });
        if let (Some(after), Some(args)) = (after, args.as_object_mut()) {
            args.insert("after_block_id".to_string(), json!(after));
        }
        server.call_tool("objects.create", args)
    };

    // ---- MCP ----
    let placed = mcp_embed("After b1", "mcp-after-b1", Some("b1")).await;
    assert_eq!(placed.is_error, None, "{:?}", placed.content);
    let placed_block = block_id_of(&serde_json::from_str(result_text(&placed)?)?)?;
    assert_eq!(
        root_blocks(&state, document_id).await?,
        ["b1", placed_block.as_str(), "b2", "b3"],
        "the embed block follows b1"
    );

    let appended = mcp_embed("Appended", "mcp-appended", None).await;
    assert_eq!(appended.is_error, None, "{:?}", appended.content);
    let appended_block = block_id_of(&serde_json::from_str(result_text(&appended)?)?)?;
    assert_eq!(
        root_blocks(&state, document_id).await?,
        ["b1", placed_block.as_str(), "b2", "b3", appended_block.as_str()],
        "without after_block_id the embed is appended"
    );

    let collections = collection_count(&state).await?;
    let unknown = mcp_embed("Nowhere", "mcp-unknown-block", Some("no-such-block")).await;
    assert_eq!(unknown.is_error, Some(true), "{:?}", unknown.content);
    let refusal = result_text(&unknown)?;
    assert!(
        refusal.contains("invalid_update") && !refusal.contains("server_rejected"),
        "the server's invalid_update is passed through: {refusal}"
    );
    assert_eq!(
        collection_count(&state).await?,
        collections,
        "a refused embed writes nothing"
    );
    assert_eq!(root_blocks(&state, document_id).await?.len(), 5);

    // ---- shipped CLI ----
    let config = write_config(&McpSettings {
        api_url: &api_url,
        bot_token: Some(CLI_BOT_TOKEN),
        workspace_id: &workspace_id.to_string(),
        transport: Some("stdio"),
        bind_addr: None,
    })?;
    let (workspace, page) = (workspace_id.to_string(), page_id.to_string());
    let cli_embed = |title: &'static str, key: &'static str, after: &'static str| {
        let (config, workspace, page) = (&config, &workspace, &page);
        async move {
            run(
                config,
                &[
                    "objects",
                    "create",
                    "--workspace",
                    workspace,
                    "--type",
                    "collection",
                    "--title",
                    title,
                    "--embed-page",
                    page,
                    "--after-block",
                    after,
                    "--idempotency-key",
                    key,
                ],
            )
            .await
        }
    };
    let cli_placed = cli_embed("After b2", "cli-after-b2", "b2").await?;
    assert_success(&cli_placed, "objects.create")?;
    let envelope: Value = serde_json::from_slice(&cli_placed.stdout)?;
    let cli_block = block_id_of(envelope.pointer("/data").ok_or("CLI envelope has no data")?)?;
    assert_eq!(
        root_blocks(&state, document_id).await?,
        [
            "b1",
            placed_block.as_str(),
            "b2",
            cli_block.as_str(),
            "b3",
            appended_block.as_str()
        ],
        "the CLI embed block follows b2"
    );

    let collections = collection_count(&state).await?;
    let cli_unknown = cli_embed("Nowhere", "cli-unknown-block", "no-such-block").await?;
    assert_eq!(
        cli_unknown.status.code(),
        Some(7),
        "invalid_update exits 7: {}",
        String::from_utf8_lossy(&cli_unknown.stderr)
    );
    let envelope: Value = serde_json::from_slice(&cli_unknown.stdout)?;
    assert_eq!(envelope.pointer("/ok"), Some(&json!(false)));
    assert_eq!(envelope.pointer("/error/code"), Some(&json!("invalid_update")));
    assert_eq!(
        collection_count(&state).await?,
        collections,
        "a refused embed writes nothing"
    );
    assert_eq!(root_blocks(&state, document_id).await?.len(), 6);

    server_task.abort();
    scratch.drop_self().await;
    Ok(())
}
