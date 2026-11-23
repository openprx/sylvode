//! The project bot connection against a real database: every row it writes, every refusal, the
//! versions, the secrets that must never be stored or logged, and the removal that releases a bot.
//!
//! Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped.
#![cfg(test)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names
)]

use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Request, header},
    response::{IntoResponse, Response},
    routing::get,
};
use platform::app::AppState;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::error::ApiError;
use crate::oauth::delegated::{DelegatedAuthContext, with_context};
use crate::oauth::scopes::{ProjectScope, ScopeSet, TokenResource};
use crate::routes::context::tenant_fixture::{
    Tenant, bot_context_for, claims_for, count, exec, seed_bot_user, seed_member, seed_tenant, state_for,
};
use crate::routes::project_bot_connection as route;
use crate::scratch_or_skip;
use crate::services::bot_connections::{
    self, ConnectRequest, Connected, ConnectionView, Outcome, Removed, Rotated, UpdateRequest,
};
use crate::services::execution_settings::project_ref;

/// A public literal address: it needs no DNS and is not on the blocked list.
const RUNNER: &str = "https://93.184.216.34:9090";

pub struct W {
    pub tenant: Tenant,
    pub owner: Uuid,
    pub state: AppState,
}

pub async fn world(db: &DatabaseConnection, tag: &str) -> W {
    let tenant = seed_tenant(db, tag).await;
    exec(
        db,
        "UPDATE workspace_members SET role = 'owner' WHERE workspace_id = $1 AND user_id = $2",
        vec![tenant.workspace_id.into(), tenant.member_id.into()],
    )
    .await;
    W {
        owner: tenant.member_id,
        state: state_for(db.clone()),
        tenant,
    }
}

pub async fn second_project(db: &DatabaseConnection, w: &W, key: &str) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, $3, $4, $5)",
        vec![
            id.into(),
            w.tenant.workspace_id.into(),
            key.to_string().into(),
            format!("project {key}").into(),
            w.owner.into(),
        ],
    )
    .await;
    id
}

pub fn request(value: Value) -> ConnectRequest {
    serde_json::from_value(value).expect("a connect request")
}

pub fn new_bot(name: &str) -> Value {
    json!({
        "bot": {"name": name},
        "runner_url": RUNNER,
        "cli": "claude",
        "working_directory": "/srv/dev/work/svwl",
    })
}

pub fn existing_bot(id: Uuid) -> Value {
    json!({
        "bot": {"id": id},
        "runner_url": RUNNER,
        "cli": "claude",
        "working_directory": "/srv/dev/work/other",
    })
}

pub async fn connect(w: &W, project: Uuid, body: Value) -> Result<Connected, ApiError> {
    let project = project_ref(&w.state.db, project).await?;
    bot_connections::connect(&w.state, &project, w.owner, request(body), None).await
}

pub async fn expect_connect(w: &W, project: Uuid, body: Value) -> Connected {
    connect(w, project, body)
        .await
        .unwrap_or_else(|err| panic!("connect failed: {err:?}"))
}

pub fn update_request(value: Value) -> UpdateRequest {
    serde_json::from_value(value).expect("an update request")
}

/// The bot of the project's only connection; a project without one answers the 404 of a missing
/// connection, and a project with several is a mistake of the test.
pub async fn sole_bot(db: &DatabaseConnection, project: Uuid) -> Result<Uuid, ApiError> {
    let mut bots = bot_connections::list_member_views(db, project).await?;
    assert!(bots.len() <= 1, "this helper is for a project with one connection");
    bots.pop()
        .map(|view| view.bot.id)
        .ok_or_else(|| ApiError::NotFound(bot_connections::NO_CONNECTION.to_string()))
}

pub async fn update(w: &W, project: Uuid, body: Value) -> Result<Outcome<ConnectionView>, ApiError> {
    let bot = sole_bot(&w.state.db, project).await?;
    update_for(w, project, bot, body).await
}

pub async fn update_for(w: &W, project: Uuid, bot: Uuid, body: Value) -> Result<Outcome<ConnectionView>, ApiError> {
    let project = project_ref(&w.state.db, project).await?;
    bot_connections::update(&w.state, &project, bot, w.owner, update_request(body)).await
}

#[allow(
    clippy::unused_async,
    reason = "callers await it next to the write that produced the outcome"
)]
pub async fn done(outcome: Result<Outcome<ConnectionView>, ApiError>) -> ConnectionView {
    match outcome {
        Ok(Outcome::Done(view)) => view,
        other => panic!("expected the write to happen, got {:?}", other.map(|_| ())),
    }
}

pub async fn set_enabled(
    w: &W,
    project: Uuid,
    version: i32,
    enabled: bool,
) -> Result<Outcome<ConnectionView>, ApiError> {
    let bot = sole_bot(&w.state.db, project).await?;
    set_enabled_for(w, project, bot, version, enabled).await
}

pub async fn set_enabled_for(
    w: &W,
    project: Uuid,
    bot: Uuid,
    version: i32,
    enabled: bool,
) -> Result<Outcome<ConnectionView>, ApiError> {
    let project = project_ref(&w.state.db, project).await?;
    bot_connections::set_enabled(&w.state, &project, bot, w.owner, version, enabled).await
}

pub async fn rotate(w: &W, project: Uuid, version: i32) -> Result<Outcome<Rotated>, ApiError> {
    let bot = sole_bot(&w.state.db, project).await?;
    let project = project_ref(&w.state.db, project).await?;
    bot_connections::rotate(&w.state, &project, bot, w.owner, version, None).await
}

pub async fn remove(w: &W, project: Uuid, version: i32) -> Result<Outcome<Removed>, ApiError> {
    let bot = sole_bot(&w.state.db, project).await?;
    remove_for(w, project, bot, version).await
}

pub async fn remove_for(w: &W, project: Uuid, bot: Uuid, version: i32) -> Result<Outcome<Removed>, ApiError> {
    let project = project_ref(&w.state.db, project).await?;
    bot_connections::remove(&w.state, &project, bot, w.owner, version).await
}

pub fn bad_request(result: Result<impl std::fmt::Debug, ApiError>) -> String {
    match result {
        Err(ApiError::BadRequest(message)) => message,
        other => panic!("expected a 400, got {other:?}"),
    }
}

pub async fn n(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    count(
        db,
        &format!("SELECT COUNT(*)::bigint AS count FROM ({sql}) AS t"),
        values,
    )
    .await
}

pub async fn text_of(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> String {
    db.query_one(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .expect("query")
        .expect("row")
        .try_get::<String>("", "v")
        .expect("v")
}

/// The token of a connect or rotate answer, from its block.
pub fn block_token(block: &crate::services::runner_config_block::ConfigBlock) -> String {
    let table: toml::Table = block.text.parse().expect("valid TOML");
    table["ai_task_callback"]["bots"][0]["token"]
        .as_str()
        .expect("token")
        .to_string()
}

/// The workspace the block names on the bot's entry.
pub fn block_workspace(block: &crate::services::runner_config_block::ConfigBlock) -> String {
    let table: toml::Table = block.text.parse().expect("valid TOML");
    table["ai_task_callback"]["bots"][0]["workspace_id"]
        .as_str()
        .expect("workspace_id")
        .to_string()
}

pub fn block_secret(block: &crate::services::runner_config_block::ConfigBlock) -> String {
    let table: toml::Table = block.text.parse().expect("valid TOML");
    table["security"]["webhook_secrets"][0]
        .as_str()
        .expect("secret")
        .to_string()
}

/// Calls `GET /api/v1/ai/protocol` through the real bot authentication.
pub async fn protocol_with(db: &DatabaseConnection, token: &str, surface: Option<&str>) -> (u16, Value) {
    let state = state_for(db.clone());
    let app = Router::new()
        .route(
            "/api/v1/ai/protocol",
            get(crate::routes::ai_task_protocol::get_protocol).route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .with_state(state);
    let mut builder = Request::builder()
        .uri("/api/v1/ai/protocol")
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    if let Some(surface) = surface {
        builder = builder.header("x-sylvode-mcp-surface", surface);
    }
    let response = app
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("answer");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

pub fn authenticated(answer: &(u16, Value)) -> bool {
    answer.0 == 200 && answer.1["code"] == 0
}

pub async fn body_of(response: Response) -> (axum::http::HeaderMap, Value) {
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (headers, serde_json::from_slice(&bytes).expect("json"))
}

// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_owner_connects_a_new_bot_and_every_row_exists() {
    let scratch = scratch_or_skip!("bc_connect");
    let db = &scratch.db;
    let w = world(db, "c1").await;
    let connected = expect_connect(&w, w.tenant.project_id, new_bot("Build bot")).await;
    let view = &connected.connection;
    let bot = view.bot.id;
    let block = connected.config_block.as_ref().expect("a new bot gets a block");
    // The MCP server the runner starts for the CLI needs the bot's workspace.
    assert_eq!(block_workspace(block), w.tenant.workspace_id.to_string());

    assert_eq!(view.version, 1);
    assert!(view.enabled);
    assert_eq!(view.bot.name, "Build bot");
    assert!(view.bot.token_active);
    assert_eq!(view.runner.url, RUNNER);
    assert_eq!(view.runner.shared_with_projects.len(), 1);
    assert_eq!(view.governance.max_domain_level, "voter");
    assert_eq!(view.governance.reason_min_length, 50);
    assert!(!view.governance.can_veto_human_consensus);
    assert_eq!(view.execution.channel, "tmux");
    assert_eq!(view.execution.on_complete_state, "auto");
    assert_eq!(view.execution.approval_gate, "none");

    let one = |sql: &'static str| n(db, sql, vec![bot.into()]);
    assert_eq!(
        one("SELECT 1 FROM users WHERE id = $1 AND entity_type = 'bot' AND is_active").await,
        1
    );
    assert_eq!(
        one("SELECT 1 FROM workspace_members WHERE user_id = $1 AND role = 'member'").await,
        1,
        "a seat with the member role"
    );
    assert_eq!(
        one(
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND transport_surface = 'mcp_stdio' AND is_active \
             AND permissions = '[\"read\",\"write\"]'::jsonb"
        )
        .await,
        1,
        "a token on the mcp_stdio surface with read and write"
    );
    assert_eq!(
        one("SELECT 1 FROM bot_runners r JOIN webhooks h ON h.id = r.webhook_id \
             WHERE r.bot_id = $1 AND h.events = '[]'::jsonb AND h.active AND h.bot_user_id = r.bot_id \
               AND h.url = r.runner_url || '/webhook'")
        .await,
        1,
        "a managed webhook with no events, pointing at the address plus /webhook"
    );
    assert_eq!(
        one("SELECT 1 FROM ai_participants WHERE id = $1::text AND is_active AND max_domain_level = 'voter'").await,
        1
    );
    assert_eq!(
        one("SELECT 1 FROM project_bot_connections c WHERE c.bot_id = $1 AND c.enabled AND c.version = 1").await,
        1
    );
    for event in ["project.bot_connected", "webhook.created"] {
        assert_eq!(
            n(
                db,
                "SELECT 1 FROM business_events WHERE workspace_id = $1 AND event_type = $2",
                vec![w.tenant.workspace_id.into(), event.into()]
            )
            .await,
            1,
            "{event}"
        );
    }

    // The invariant of section 3.3.
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM project_bot_connections c \
             JOIN ai_participants a ON a.project_id = c.project_id AND a.id = c.bot_id::text \
             WHERE c.enabled <> a.is_active",
            vec![]
        )
        .await,
        0
    );

    // The token in the block authenticates on the surface it was issued for, and only there.
    let token = block_token(block);
    assert!(authenticated(&protocol_with(db, &token, Some("mcp_stdio")).await));
    assert!(
        !authenticated(&protocol_with(db, &token, Some("rest")).await),
        "another surface is refused"
    );
    // The secret in the block is the key deliveries are signed with.
    assert_eq!(
        text_of(
            db,
            "SELECT secret AS v FROM webhooks WHERE bot_user_id = $1",
            vec![bot.into()]
        )
        .await,
        block_secret(block)
    );
    // What is stored is the hash and the prefix, never the token.
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND token_hash <> $2 AND token_prefix = $3",
            vec![bot.into(), token.clone().into(), token[..8].to_string().into()]
        )
        .await,
        1
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_failure_at_any_step_leaves_no_row_behind() {
    let scratch = scratch_or_skip!("bc_atomic");
    let db = &scratch.db;
    let w = world(db, "a1").await;
    let tables = [
        "users",
        "workspace_members",
        "workspace_bots",
        "webhooks",
        "bot_runners",
        "ai_participants",
        "project_bot_connections",
        "business_events",
    ];
    let snapshot = || async {
        let mut counts = Vec::new();
        for table in tables {
            counts.push(n(db, &format!("SELECT 1 FROM {table}"), vec![]).await);
        }
        counts
    };
    let before = snapshot().await;
    // One trigger per write of the connect, each raising once it is reached.
    let steps: [(&str, &str); 8] = [
        ("users", ""),
        ("workspace_members", ""),
        ("workspace_bots", ""),
        ("webhooks", ""),
        ("bot_runners", ""),
        ("ai_participants", ""),
        ("project_bot_connections", ""),
        ("business_events", "WHEN (NEW.event_type = 'project.bot_connected')"),
    ];
    for (index, (table, condition)) in steps.iter().enumerate() {
        db.execute_unprepared(&format!(
            "CREATE FUNCTION fail_step_{index}() RETURNS trigger LANGUAGE plpgsql AS \
             $$ BEGIN RAISE EXCEPTION 'injected failure {index}'; END $$; \
             CREATE TRIGGER fail_step_{index} BEFORE INSERT ON {table} FOR EACH ROW {condition} \
             EXECUTE FUNCTION fail_step_{index}();"
        ))
        .await
        .expect("trigger");
        let result = connect(&w, w.tenant.project_id, new_bot("Atomic bot")).await;
        assert!(result.is_err(), "step {index} on {table} must fail the connect");
        db.execute_unprepared(&format!(
            "DROP TRIGGER fail_step_{index} ON {table}; DROP FUNCTION fail_step_{index}();"
        ))
        .await
        .expect("drop trigger");
        assert_eq!(
            snapshot().await,
            before,
            "a failure at step {index} ({table}) left rows behind"
        );
    }
    // And with the failures gone, the same connect works.
    expect_connect(&w, w.tenant.project_id, new_bot("Atomic bot")).await;
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_connected_bot_joins_a_second_project_without_new_credentials() {
    let scratch = scratch_or_skip!("bc_second");
    let db = &scratch.db;
    let w = world(db, "s1").await;
    let p2 = second_project(db, &w, "PS2").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Shared bot")).await;
    let bot = first.connection.bot.id;

    let before_tokens = n(db, "SELECT 1 FROM workspace_bots", vec![]).await;
    let before_hooks = n(db, "SELECT 1 FROM webhooks", vec![]).await;
    let second = expect_connect(&w, p2, existing_bot(bot)).await;
    assert!(second.config_block.is_none(), "no block for an already connected bot");
    assert_eq!(second.connection.bot.id, bot);
    assert_eq!(second.connection.version, 1);
    assert_eq!(second.connection.runner.shared_with_projects.len(), 2);
    assert_eq!(n(db, "SELECT 1 FROM workspace_bots", vec![]).await, before_tokens);
    assert_eq!(n(db, "SELECT 1 FROM webhooks", vec![]).await, before_hooks);
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM ai_participants WHERE id = $1::text",
            vec![bot.into()]
        )
        .await,
        2,
        "one registration per project"
    );

    // A different address is refused with a 409 that names the stored one.
    let p3 = second_project(db, &w, "PS3").await;
    let mut other = existing_bot(bot);
    other["runner_url"] = json!("https://93.184.216.35:9090");
    match connect(&w, p3, other).await {
        Err(ApiError::Conflict(message)) => assert_eq!(
            message,
            format!("runner_url: this bot already delivers to {RUNNER}; change the address on its connection")
        ),
        other => panic!("expected a 409, got {:?}", other.map(|_| ())),
    }
    // A bot made on the Bots page is neither offered nor accepted.
    let bots_page_bot = seed_bot_user(db, &w.tenant).await;
    assert_eq!(
        bad_request(connect(&w, p3, existing_bot(bots_page_bot)).await),
        "bot.id: not a connected bot of this workspace"
    );
    assert_eq!(
        bad_request(connect(&w, p3, existing_bot(Uuid::new_v4())).await),
        "bot.id: not a connected bot of this workspace"
    );
    // The same bot twice in one project is refused; a second tmux connection for claude is refused
    // with the name of the bot that holds the session.
    match connect(&w, w.tenant.project_id, existing_bot(bot)).await {
        Err(ApiError::Conflict(message)) => assert_eq!(message, "project already has a connection for this bot"),
        other => panic!("expected a 409, got {:?}", other.map(|_| ())),
    }
    match connect(&w, w.tenant.project_id, new_bot("Second bot")).await {
        Err(ApiError::Conflict(message)) => assert_eq!(
            message,
            "channel: project already has a tmux connection for claude (bot Shared bot)"
        ),
        other => panic!("expected a 409, got {:?}", other.map(|_| ())),
    }
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_bot_of_another_workspace_cannot_be_picked() {
    let scratch = scratch_or_skip!("bc_foreign_bot");
    let db = &scratch.db;
    let w = world(db, "f1").await;
    let other = world(db, "f2").await;
    let foreign = expect_connect(&other, other.tenant.project_id, new_bot("Foreign bot")).await;
    assert_eq!(
        bad_request(connect(&w, w.tenant.project_id, existing_bot(foreign.connection.bot.id)).await),
        "bot.id: not a connected bot of this workspace"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn two_simultaneous_connects_make_one_connection_and_one_bot() {
    let scratch = scratch_or_skip!("bc_race");
    let db = &scratch.db;
    let w = world(db, "r1").await;
    let both = tokio::join!(
        connect(&w, w.tenant.project_id, new_bot("Racer one")),
        connect(&w, w.tenant.project_id, new_bot("Racer two"))
    );
    let results: [Result<Connected, ApiError>; 2] = both.into();
    assert_eq!(
        results.iter().filter(|r| r.is_ok()).count(),
        1,
        "exactly one connect wins"
    );
    let loser = results.iter().find_map(|r| r.as_ref().err()).expect("one loses");
    assert!(
        matches!(loser, ApiError::Conflict(message)
            if message.starts_with("channel: project already has a tmux connection for claude (bot Racer ")),
        "{loser:?}"
    );
    assert_eq!(
        n(db, "SELECT 1 FROM workspace_bots", vec![]).await,
        1,
        "exactly one bot was created"
    );
    assert_eq!(n(db, "SELECT 1 FROM bot_runners", vec![]).await, 1);
    assert_eq!(n(db, "SELECT 1 FROM webhooks", vec![]).await, 1);
    scratch.drop_self().await;
}

#[tokio::test]
async fn only_owners_and_admins_manage_and_machines_never_do() {
    let scratch = scratch_or_skip!("bc_authz");
    let db = &scratch.db;
    let w = world(db, "z1").await;
    let admin = seed_member(db, &w.tenant, "admin").await;
    let member = seed_member(db, &w.tenant, "member").await;
    let stranger = seed_tenant(db, "z2").await;
    let project = w.tenant.project_id;

    let call = |user: Uuid, bot: Option<crate::middleware::bot_auth::BotAuthContext>, body: Value| {
        let state = w.state.clone();
        async move {
            route::connect_bot(
                State(state),
                Extension(claims_for(user)),
                bot.map(Extension),
                Path(project),
                HeaderMap::new(),
                Json(request(body)),
            )
            .await
        }
    };
    let forbidden = |result: Result<Response, ApiError>, message: &str| match result {
        Err(ApiError::Forbidden(got)) => assert_eq!(got, message),
        other => panic!("expected a 403, got {:?}", other.map(|_| ())),
    };
    forbidden(
        call(member, None, new_bot("Member bot")).await,
        "only workspace owners and admins can manage bot connections",
    );
    forbidden(
        call(stranger.member_id, None, new_bot("Stranger bot")).await,
        "only workspace owners and admins can manage bot connections",
    );
    // A bot, even one seated as an admin with every permission.
    let bot = seed_bot_user(db, &w.tenant).await;
    exec(
        db,
        "UPDATE workspace_members SET role = 'admin' WHERE workspace_id = $1 AND user_id = $2",
        vec![w.tenant.workspace_id.into(), bot.into()],
    )
    .await;
    // seed_bot_user does not seat the bot; seat it so the refusal cannot be a membership one.
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'admin') ON CONFLICT DO NOTHING",
        vec![w.tenant.workspace_id.into(), bot.into()],
    )
    .await;
    forbidden(
        call(
            bot,
            Some(bot_context_for(bot, w.tenant.workspace_id, &["read", "write", "admin"])),
            new_bot("Bot bot"),
        )
        .await,
        "bots cannot manage bot connections",
    );
    // A connected application acting for the owner.
    let delegated = DelegatedAuthContext {
        grant_id: Uuid::new_v4(),
        token_id: Uuid::new_v4(),
        client_id: Arc::from("ramflux"),
        user_id: w.owner,
        workspace_id: w.tenant.workspace_id,
        scopes: ScopeSet::all(),
        projects: ProjectScope::All,
        resource: TokenResource::Api,
        surface: crate::flow::event_origin::EventSurface::Rest,
        tool_name: None,
        request_id: Uuid::new_v4(),
    };
    forbidden(
        with_context(delegated.clone(), call(w.owner, None, new_bot("Delegated bot"))).await,
        "bots cannot manage bot connections",
    );
    assert_eq!(
        n(db, "SELECT 1 FROM project_bot_connections", vec![]).await,
        0,
        "nothing was connected"
    );

    // An admin connects.
    let admin_connect = call(admin, None, new_bot("Admin bot"))
        .await
        .expect("an admin connects");
    let admin_bot: Uuid = body_of(admin_connect).await.1["data"]["connection"]["bot"]["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("the bot id");

    // Reading: owners and admins see the full view, members the reduced one, machines nothing.
    let get = |user: Uuid, bot: Option<crate::middleware::bot_auth::BotAuthContext>| {
        let state = w.state.clone();
        async move {
            route::get_bot_connection(
                State(state),
                Extension(claims_for(user)),
                bot.map(Extension),
                Path((project, admin_bot)),
            )
            .await
        }
    };
    let (_, full) = body_of(get(w.owner, None).await.expect("owner reads")).await;
    assert_eq!(full["data"]["connection"]["runner"]["url"], RUNNER);
    assert_eq!(full["data"]["connection"]["bot"]["name"], "Admin bot");
    let (_, reduced) = body_of(get(member, None).await.expect("member reads")).await;
    assert_eq!(
        reduced["data"]["connection"],
        json!({"bot": {"id": reduced["data"]["connection"]["bot"]["id"], "name": "Admin bot"},
               "enabled": true, "cli": "claude", "channel": "tmux"})
    );
    forbidden(
        get(
            bot,
            Some(bot_context_for(bot, w.tenant.workspace_id, &["read", "write", "admin"])),
        )
        .await,
        "bots cannot manage bot connections",
    );
    forbidden(
        with_context(delegated, get(w.owner, None)).await,
        "bots cannot manage bot connections",
    );
    match get(stranger.member_id, None).await {
        Err(ApiError::Forbidden(message)) => assert_eq!(message, "project access denied"),
        other => panic!("expected a 403, got {:?}", other.map(|_| ())),
    }
    // A project with no connection for the bot is a 404; the list is empty.
    let none = route::get_bot_connection(
        State(w.state.clone()),
        Extension(claims_for(stranger.member_id)),
        None,
        Path((stranger.project_id, admin_bot)),
    )
    .await;
    assert!(
        matches!(none, Err(ApiError::NotFound(ref message)) if message == "project has no connection for this bot"),
        "{:?}",
        none.map(|_| ())
    );
    let listed = route::list_bot_connections(
        State(w.state.clone()),
        Extension(claims_for(stranger.member_id)),
        None,
        Path(stranger.project_id),
    )
    .await
    .expect("a member of that workspace lists");
    assert_eq!(body_of(listed).await.1["data"]["connections"], json!([]));
    // The list shows the connection too, in each reader's view.
    let list = |user: Uuid| {
        let state = w.state.clone();
        async move {
            let answer = route::list_bot_connections(State(state), Extension(claims_for(user)), None, Path(project))
                .await
                .expect("list");
            body_of(answer).await.1["data"]["connections"].clone()
        }
    };
    let full = list(w.owner).await;
    assert_eq!(full[0]["connection"]["bot"]["name"], "Admin bot");
    assert!(full[0]["status"].is_object());
    let reduced = list(member).await;
    assert_eq!(reduced[0]["bot"]["name"], "Admin bot");
    assert!(reduced[0].get("runner").is_none());
    scratch.drop_self().await;
}

#[tokio::test]
async fn every_field_is_validated_and_a_refusal_writes_nothing() {
    let scratch = scratch_or_skip!("bc_validation");
    let db = &scratch.db;
    let w = world(db, "v1").await;
    let project = w.tenant.project_id;
    let mut cases: Vec<(Value, &str)> = Vec::new();
    let base = new_bot("Valid bot");
    let with = |key: &str, value: Value| {
        let mut body = base.clone();
        body[key] = value;
        body
    };
    cases.push((
        with("bot", json!({"name": "A", "id": Uuid::new_v4()})),
        "bot: give either name or id",
    ));
    cases.push((with("bot", json!({})), "bot: give either name or id"));
    cases.push((with("bot", json!({"name": "   "})), "bot.name: required"));
    cases.push((
        with("bot", json!({"name": "x".repeat(101)})),
        "bot.name: at most 100 characters",
    ));
    cases.push((
        with("bot", json!({"name": "a\u{1}b"})),
        "bot.name: must not contain control, bidirectional or zero-width characters",
    ));
    for hidden in ["a\u{202e}b", "a\u{200b}b", "a\u{2066}b", "\u{feff}ab"] {
        cases.push((
            with("bot", json!({"name": hidden})),
            "bot.name: must not contain control, bidirectional or zero-width characters",
        ));
    }
    for directory in ["/srv/dev/a#b", "/srv/dev/a\u{202e}b", "/srv/dev/a\u{200b}b"] {
        cases.push((
            with("working_directory", json!(directory)),
            if directory.contains('#') {
                "working_directory: must not contain #"
            } else {
                "working_directory: must not contain control, bidirectional or zero-width characters"
            },
        ));
    }
    cases.push((
        with("runner_url", json!("http://93.184.216.34:9090")),
        "runner_url: must use https unless the host is listed in [outbound] allowed_hosts",
    ));
    cases.push((
        with("runner_url", json!("https://93.184.216.34\\.x.example")),
        "runner_url: must not contain a backslash",
    ));
    cases.push((with("cli", json!("vim")), "cli: must be claude, codex or opencode"));
    cases.push((with("channel", json!("pigeon")), "channel: must be tmux or headless"));
    cases.push((
        with("working_directory", json!("relative")),
        "working_directory: must be an absolute path",
    ));
    cases.push((
        with("working_directory", json!("/a/../b")),
        "working_directory: must not contain . or .. segments",
    ));
    cases.push((
        with("on_complete_state", json!("sometimes")),
        "on_complete_state: must be auto, none or state:<key>",
    ));
    cases.push((
        with("on_fail_label_id", json!(Uuid::new_v4())),
        "on_fail_label_id: is not a label of the project's workspace",
    ));
    cases.push((
        with("runner_url", json!("runner.example.net")),
        "runner_url: must start with http:// or https://",
    ));
    cases.push((
        with("runner_url", json!("http://u:p@93.184.216.34")),
        "runner_url: must not embed credentials",
    ));
    cases.push((
        with("runner_url", json!("http://93.184.216.34?x=1")),
        "runner_url: must not contain a query or a fragment",
    ));
    cases.push((
        with("governance", json!({"max_domain_level": "boss"})),
        "governance.max_domain_level: must be observer, advisor, voter, vetoer or autonomous",
    ));
    cases.push((
        with("governance", json!({"can_veto_human_consensus": true})),
        "governance.can_veto_human_consensus: requires max_domain_level vetoer or autonomous",
    ));
    cases.push((
        with("governance", json!({"reason_min_length": 9000})),
        "governance.reason_min_length: must be between 0 and 2000",
    ));
    cases.push((
        with("governance", json!({"domain_overrides": {"governance": "voter"}})),
        "governance.domain_overrides: governance: an AI is always an observer in governance",
    ));
    for (body, expected) in cases {
        let message = bad_request(connect(&w, project, body.clone()).await);
        assert_eq!(message, expected, "{body}");
    }
    // A blocked address is refused with the hint, and the allowlist is the way out.
    let blocked = bad_request(connect(&w, project, with("runner_url", json!("http://127.0.0.1:9090"))).await);
    assert!(
        blocked.starts_with("runner_url: ") && blocked.contains("blocked address"),
        "{blocked}"
    );
    assert!(blocked.contains("[outbound] allowed_hosts"), "{blocked}");
    let missing_cli = {
        let mut body = base.clone();
        body.as_object_mut().expect("object").remove("cli");
        bad_request(connect(&w, project, body).await)
    };
    assert_eq!(missing_cli, "cli: is required");
    assert_eq!(n(db, "SELECT 1 FROM workspace_bots", vec![]).await, 0);
    assert_eq!(n(db, "SELECT 1 FROM project_bot_connections", vec![]).await, 0);

    // A name is unique among the workspace's active bots, whatever its case.
    let p2 = second_project(db, &w, "PV2").await;
    expect_connect(&w, project, new_bot("Build Bot")).await;
    assert_eq!(
        bad_request(connect(&w, p2, new_bot("build bot")).await),
        "bot.name: a bot with this name already exists in this workspace"
    );
    // A valid label and a state of the workflow are accepted.
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_stale_version_changes_nothing_and_a_shared_change_bumps_every_connection() {
    let scratch = scratch_or_skip!("bc_versions");
    let db = &scratch.db;
    let w = world(db, "e1").await;
    let p2 = second_project(db, &w, "PE2").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Versioned bot")).await;
    let bot = first.connection.bot.id;
    expect_connect(&w, p2, existing_bot(bot)).await;
    let version = |project: Uuid| {
        let project = project.to_string();
        async move {
            count(
                db,
                &format!("SELECT version::bigint AS count FROM project_bot_connections WHERE project_id = '{project}'"),
                vec![],
            )
            .await
        }
    };
    assert_eq!((version(w.tenant.project_id).await, version(p2).await), (1, 1));

    // An empty change set is refused.
    assert_eq!(
        bad_request(update(&w, w.tenant.project_id, json!({"expected_version": 1})).await),
        "no changes provided"
    );
    // A per-project change bumps this connection only.
    let changed = done(
        update(
            &w,
            w.tenant.project_id,
            json!({"expected_version": 1, "cli": "codex", "channel": "headless"}),
        )
        .await,
    )
    .await;
    assert_eq!(
        (
            changed.version,
            changed.execution.cli.as_str(),
            changed.execution.channel.as_str()
        ),
        (2, "codex", "headless")
    );
    assert_eq!(version(p2).await, 1);
    assert_eq!(changed.updated_by, Some(w.owner));

    // A stale version is a conflict that carries the current connection and writes nothing.
    let events_before = n(db, "SELECT 1 FROM business_events", vec![]).await;
    match update(
        &w,
        w.tenant.project_id,
        json!({"expected_version": 1, "cli": "opencode"}),
    )
    .await
    {
        Ok(Outcome::Conflict { current }) => {
            assert_eq!((current.version, current.execution.cli.as_str()), (2, "codex"));
        }
        other => panic!("expected a conflict, got {:?}", other.map(|_| ())),
    }
    assert_eq!(n(db, "SELECT 1 FROM business_events", vec![]).await, events_before);

    // Changing the address changes the webhook and bumps every connection of the bot.
    let moved = done(
        update(
            &w,
            w.tenant.project_id,
            json!({"expected_version": 2, "runner_url": "https://93.184.216.36:9443/"}),
        )
        .await,
    )
    .await;
    assert_eq!(moved.runner.url, "https://93.184.216.36:9443");
    assert_eq!((version(w.tenant.project_id).await, version(p2).await), (3, 2));
    assert_eq!(
        text_of(
            db,
            "SELECT url AS v FROM webhooks WHERE bot_user_id = $1",
            vec![bot.into()]
        )
        .await,
        "https://93.184.216.36:9443/webhook"
    );
    // The other project's page, still on version 1, conflicts instead of overwriting.
    match update(&w, p2, json!({"expected_version": 1, "working_directory": "/work/x"})).await {
        Ok(Outcome::Conflict { current }) => assert_eq!(current.version, 2),
        other => panic!("expected a conflict, got {:?}", other.map(|_| ())),
    }
    // Governance changes are checked against the stored values.
    let governed = done(
        update(
            &w,
            p2,
            json!({"expected_version": 2, "governance": {"max_domain_level": "vetoer", "can_veto_human_consensus": true, "reason_min_length": 10}}),
        )
        .await,
    )
    .await;
    assert_eq!(governed.governance.max_domain_level, "vetoer");
    assert!(governed.governance.can_veto_human_consensus);
    assert_eq!(
        bad_request(
            update(
                &w,
                p2,
                json!({"expected_version": 3, "governance": {"max_domain_level": "voter"}})
            )
            .await
        ),
        "governance.can_veto_human_consensus: requires max_domain_level vetoer or autonomous",
        "lowering the level while the veto stays on is refused"
    );
    // Clearing the failure label is an explicit null.
    let label = crate::routes::context::tenant_fixture::seed_label(db, &w.tenant, "failed").await;
    let labelled = done(update(&w, p2, json!({"expected_version": 3, "on_fail_label_id": label})).await).await;
    assert_eq!(labelled.execution.on_fail_label_id, Some(label));
    let cleared = done(update(&w, p2, json!({"expected_version": 4, "on_fail_label_id": null})).await).await;
    assert_eq!(cleared.execution.on_fail_label_id, None);
    scratch.drop_self().await;
}

#[tokio::test]
async fn disable_and_enable_follow_the_last_enabled_connection() {
    let scratch = scratch_or_skip!("bc_enable");
    let db = &scratch.db;
    let w = world(db, "d1").await;
    let p2 = second_project(db, &w, "PD2").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Switch bot")).await;
    let bot = first.connection.bot.id;
    expect_connect(&w, p2, existing_bot(bot)).await;
    let state_of = |project: Uuid| async move {
        text_of(
            db,
            "SELECT (c.enabled::text || '/' || a.is_active::text) AS v \
             FROM project_bot_connections c JOIN ai_participants a ON a.project_id = c.project_id \
             WHERE c.project_id = $1",
            vec![project.into()],
        )
        .await
    };
    let shared = || async {
        text_of(
            db,
            "SELECT (b.is_active::text || '/' || h.active::text) AS v \
             FROM workspace_bots b JOIN bot_runners r ON r.bot_id = b.id JOIN webhooks h ON h.id = r.webhook_id \
             WHERE b.id = $1",
            vec![bot.into()],
        )
        .await
    };

    // One of two connections off: the participant and the settings switch, the shared parts stay.
    let off = done(set_enabled(&w, w.tenant.project_id, 1, false).await).await;
    assert!(!off.enabled);
    assert_eq!(state_of(w.tenant.project_id).await, "false/false");
    assert_eq!(shared().await, "true/true", "the bot still serves another project");
    // The second off is the last: token and webhook switch off too.
    let off2 = done(set_enabled(&w, p2, 1, false).await).await;
    assert!(!off2.enabled && !off2.bot.token_active);
    assert_eq!(shared().await, "false/false");
    // The token is refused while off.
    // Repeating the state is a no-op: no version change, no event.
    let events = n(db, "SELECT 1 FROM business_events", vec![]).await;
    let again = done(set_enabled(&w, p2, off2.version, false).await).await;
    assert_eq!(again.version, off2.version);
    assert_eq!(n(db, "SELECT 1 FROM business_events", vec![]).await, events);
    // Enabling one switches the shared parts back on, for every connection of the bot.
    let on = done(set_enabled(&w, w.tenant.project_id, off.version + 1, true).await).await;
    assert!(on.enabled && on.bot.token_active);
    assert_eq!(shared().await, "true/true");
    assert_eq!(state_of(w.tenant.project_id).await, "true/true");
    assert_eq!(state_of(p2).await, "false/false");
    // A stale version is a conflict.
    assert!(matches!(
        set_enabled(&w, p2, 1, true).await,
        Ok(Outcome::Conflict { .. })
    ));
    scratch.drop_self().await;
}

#[tokio::test]
async fn rotating_replaces_the_token_and_the_secret_for_every_project() {
    let scratch = scratch_or_skip!("bc_rotate");
    let db = &scratch.db;
    let w = world(db, "t1").await;
    let p2 = second_project(db, &w, "PT2").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Rotating bot")).await;
    let bot = first.connection.bot.id;
    let old_block = first.config_block.expect("block");
    let (old_token, old_secret) = (block_token(&old_block), block_secret(&old_block));
    let second = expect_connect(&w, p2, existing_bot(bot)).await;
    assert!(authenticated(&protocol_with(db, &old_token, Some("mcp_stdio")).await));
    let issued_before = first.connection.runner.credentials_issued_at;

    assert!(matches!(
        rotate(&w, w.tenant.project_id, 99).await,
        Ok(Outcome::Conflict { .. })
    ));
    let Ok(Outcome::Done(rotated)) = rotate(&w, w.tenant.project_id, first.connection.version).await else {
        panic!("rotate failed");
    };
    let (new_token, new_secret) = (block_token(&rotated.config_block), block_secret(&rotated.config_block));
    assert_eq!(block_workspace(&rotated.config_block), block_workspace(&old_block));
    assert_ne!(new_token, old_token);
    assert_ne!(new_secret, old_secret);
    assert!(
        !authenticated(&protocol_with(db, &old_token, Some("mcp_stdio")).await),
        "the old token stops working"
    );
    assert!(authenticated(&protocol_with(db, &new_token, Some("mcp_stdio")).await));
    assert_eq!(
        text_of(
            db,
            "SELECT secret AS v FROM webhooks WHERE bot_user_id = $1",
            vec![bot.into()]
        )
        .await,
        new_secret,
        "the old secret no longer signs deliveries"
    );
    assert!(rotated.connection.runner.credentials_issued_at > issued_before);
    assert_eq!(rotated.connection.bot.token_prefix, &new_token[..8]);
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM project_bot_connections WHERE bot_id = $1 AND version = 2",
            vec![bot.into()]
        )
        .await,
        2,
        "both connections of the bot moved from version 1 to 2"
    );
    let other = text_of(
        db,
        "SELECT version::text AS v FROM project_bot_connections WHERE project_id = $1",
        vec![p2.into()],
    )
    .await;
    assert_eq!(
        other,
        (second.connection.version + 1).to_string(),
        "every connection of the bot moved"
    );
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM business_events WHERE event_type = 'bot.credentials_rotated' AND aggregate_id = $1",
            vec![bot.to_string().into()]
        )
        .await,
        1
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn removing_the_last_connection_releases_the_bot_but_keeps_the_account() {
    let scratch = scratch_or_skip!("bc_remove");
    let db = &scratch.db;
    let w = world(db, "m1").await;
    let p2 = second_project(db, &w, "PM2").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Leaving bot")).await;
    let bot = first.connection.bot.id;
    let token = block_token(first.config_block.as_ref().expect("block"));
    let second = expect_connect(&w, p2, existing_bot(bot)).await;

    // A stale version is a conflict.
    assert!(matches!(
        remove(&w, w.tenant.project_id, 7).await,
        Ok(Outcome::Conflict { .. })
    ));
    // Removing one of two keeps the token and the webhook, and says what is left.
    let Ok(Outcome::Done(removed)) = remove(&w, w.tenant.project_id, first.connection.version).await else {
        panic!("remove failed");
    };
    assert!(removed.removed);
    assert_eq!(removed.bot_still_connected_to.len(), 1);
    assert_eq!(removed.bot_still_connected_to[0].id, p2);
    for sql in [
        "SELECT 1 FROM ai_participants WHERE project_id = $1",
        "SELECT 1 FROM project_bot_connections WHERE project_id = $1",
    ] {
        assert_eq!(n(db, sql, vec![w.tenant.project_id.into()]).await, 0, "{sql}");
    }
    assert!(
        authenticated(&protocol_with(db, &token, Some("mcp_stdio")).await),
        "the token still works"
    );
    assert_eq!(
        n(db, "SELECT 1 FROM bot_runners WHERE bot_id = $1", vec![bot.into()]).await,
        1
    );

    // Removing the last one revokes the token, deletes the webhook, removes the seat and keeps
    // the account (its tasks, comments and votes name it).
    let current = second.connection.version;
    let Ok(Outcome::Done(removed)) = remove(&w, p2, current).await else {
        panic!("remove failed");
    };
    assert!(removed.bot_still_connected_to.is_empty());
    assert!(
        !authenticated(&protocol_with(db, &token, Some("mcp_stdio")).await),
        "the token is revoked"
    );
    assert_eq!(
        n(db, "SELECT 1 FROM bot_runners WHERE bot_id = $1", vec![bot.into()]).await,
        0
    );
    assert_eq!(
        n(db, "SELECT 1 FROM webhooks WHERE bot_user_id = $1", vec![bot.into()]).await,
        0,
        "the webhook is deleted"
    );
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM workspace_members WHERE user_id = $1",
            vec![bot.into()]
        )
        .await,
        0,
        "the seat is gone"
    );
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND NOT is_active",
            vec![bot.into()]
        )
        .await,
        1,
        "the token row stays, revoked"
    );
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM users WHERE id = $1 AND NOT is_active",
            vec![bot.into()]
        )
        .await,
        1,
        "the user row stays, deactivated"
    );
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM ai_participants WHERE id = $1::text",
            vec![bot.into()]
        )
        .await,
        0
    );
    // It cannot be picked again.
    let p3 = second_project(db, &w, "PM3").await;
    assert_eq!(
        bad_request(connect(&w, p3, existing_bot(bot)).await),
        "bot.id: not a connected bot of this workspace"
    );
    // A removed connection can be made again, with a new bot.
    expect_connect(&w, w.tenant.project_id, new_bot("Leaving bot")).await;
    scratch.drop_self().await;
}

#[tokio::test]
async fn deleting_a_project_releases_its_bot() {
    let scratch = scratch_or_skip!("bc_project_delete");
    let db = &scratch.db;
    let w = world(db, "p1").await;
    let connected = expect_connect(&w, w.tenant.project_id, new_bot("Doomed bot")).await;
    let bot = connected.connection.bot.id;
    let token = block_token(connected.config_block.as_ref().expect("block"));

    let tx = db.begin().await.expect("begin");
    crate::services::teardown::delete_project_content(&tx, w.tenant.project_id)
        .await
        .expect("teardown");
    tx.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "DELETE FROM projects WHERE id = $1",
        vec![w.tenant.project_id.into()],
    ))
    .await
    .expect("delete");
    tx.commit().await.expect("commit");

    assert_eq!(n(db, "SELECT 1 FROM bot_runners", vec![]).await, 0);
    assert_eq!(
        n(db, "SELECT 1 FROM webhooks WHERE bot_user_id = $1", vec![bot.into()]).await,
        0
    );
    assert!(!authenticated(&protocol_with(db, &token, Some("mcp_stdio")).await));
    assert_eq!(
        n(db, "SELECT 1 FROM users WHERE id = $1", vec![bot.into()]).await,
        1,
        "the account stays"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_answers_say_no_store_and_the_block_is_never_readable_again() {
    let scratch = scratch_or_skip!("bc_answers");
    let db = &scratch.db;
    let w = world(db, "h1").await;
    let project = w.tenant.project_id;
    let mut headers = HeaderMap::new();
    headers.insert(header::ORIGIN, HeaderValue::from_static("https://sylvode.example.net"));

    let response = route::connect_bot(
        State(w.state.clone()),
        Extension(claims_for(w.owner)),
        None,
        Path(project),
        headers.clone(),
        Json(request(new_bot("Answer bot"))),
    )
    .await
    .expect("connect");
    let (head, body) = body_of(response).await;
    assert_eq!(
        head.get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let block = &body["data"]["config_block"];
    assert_eq!(block["format"], "sylvode-runner-toml-1");
    assert_eq!(block["filename"], "sylvode-runner.toml");
    let text = block["text"].as_str().expect("text").to_string();
    let table: toml::Table = text.parse().expect("valid TOML");
    assert_eq!(
        table["ai_task_callback"]["api_url"].as_str(),
        Some("https://sylvode.example.net"),
        "the request's Origin is the fallback for api_url"
    );
    assert_eq!(block["token_prefix"].as_str().map(str::len), Some(8));
    let version = i32::try_from(body["data"]["connection"]["version"].as_i64().expect("version")).expect("small");
    let bot_id: Uuid = body["data"]["connection"]["bot"]["id"]
        .as_str()
        .and_then(|id| id.parse().ok())
        .expect("the bot id");

    // Reading never returns a token or a secret, whoever reads.
    let token = block_token(&crate::services::runner_config_block::ConfigBlock {
        format: "x",
        filename: "x",
        bot_id: Uuid::nil(),
        token_prefix: String::new(),
        issued_at: String::new(),
        text: text.clone(),
    });
    let secret = table["security"]["webhook_secrets"][0]
        .as_str()
        .expect("secret")
        .to_string();
    for user in [w.owner, seed_member(db, &w.tenant, "member").await] {
        let read = route::get_bot_connection(
            State(w.state.clone()),
            Extension(claims_for(user)),
            None,
            Path((project, bot_id)),
        )
        .await
        .expect("read");
        let (_, read) = body_of(read).await;
        let printed = read.to_string();
        assert!(
            !printed.contains(&token) && !printed.contains(&secret),
            "a read returned a secret"
        );
        assert!(read["data"].get("config_block").is_none());
    }
    // The webhook list does not either, and it reports the row as managed.
    let hooks = crate::routes::webhook::list_webhooks(
        State(w.state.clone()),
        Extension(claims_for(w.owner)),
        Path(w.tenant.workspace_id),
    )
    .await
    .expect("list");
    let (_, hooks) = body_of(hooks.into_response()).await;
    assert!(!hooks.to_string().contains(&secret));
    assert_eq!(hooks["data"]["items"][0]["managed_by"]["kind"], "project_connection");

    // Rotate answers no-store too, with a fresh block.
    let rotated = route::rotate_bot_connection(
        State(w.state.clone()),
        Extension(claims_for(w.owner)),
        None,
        Path((project, bot_id)),
        headers,
        Json(route::VersionBody {
            expected_version: version,
        }),
    )
    .await
    .expect("rotate");
    let (head, body) = body_of(rotated).await;
    assert_eq!(
        head.get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    assert_ne!(body["data"]["config_block"]["text"].as_str(), Some(text.as_str()));
    // A stale version answers the business conflict with the current connection.
    let stale = route::rotate_bot_connection(
        State(w.state.clone()),
        Extension(claims_for(w.owner)),
        None,
        Path((project, bot_id)),
        HeaderMap::new(),
        Json(route::VersionBody {
            expected_version: version,
        }),
    )
    .await
    .expect("answers");
    let (_, stale) = body_of(stale).await;
    assert_eq!(stale["code"], 409);
    assert_eq!(stale["message"], "connection changed; reload");
    assert_eq!(stale["data"]["version"], version + 1);
    assert!(!stale.to_string().contains("config_block"));
    // Remove through the route with the query version.
    let removed = route::remove_bot_connection(
        State(w.state.clone()),
        Extension(claims_for(w.owner)),
        None,
        Path((project, bot_id)),
        Query(route::RemoveQuery {
            expected_version: version + 1,
        }),
    )
    .await
    .expect("remove");
    let (_, removed) = body_of(removed).await;
    assert_eq!(removed["data"]["removed"], true);
    assert_eq!(removed["data"]["bot_still_connected_to"], json!([]));
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// Status, report and protocol
// ---------------------------------------------------------------------------------------------

async fn report_as(
    w: &W,
    bot: Option<crate::middleware::bot_auth::BotAuthContext>,
    body: &[u8],
) -> Result<Response, ApiError> {
    crate::routes::ai_task_protocol::put_runner_report(
        State(w.state.clone()),
        bot.map(Extension),
        Bytes::copy_from_slice(body),
    )
    .await
}

async fn status_json(w: &W, project: Uuid) -> Value {
    let bot = sole_bot(&w.state.db, project).await.expect("one connection");
    let read = route::get_bot_connection(
        State(w.state.clone()),
        Extension(claims_for(w.owner)),
        None,
        Path((project, bot)),
    )
    .await
    .expect("read");
    body_of(read).await.1["data"]["status"].clone()
}

#[tokio::test]
async fn the_protocol_lists_the_report_feature_and_interval() {
    let answer = crate::routes::ai_task_protocol::get_protocol().await.into_response();
    let (_, body) = body_of(answer).await;
    assert!(
        body["data"]["features"]
            .as_array()
            .expect("features")
            .contains(&json!("runner_report")),
        "{body}"
    );
    assert_eq!(body["data"]["runner_report_interval_secs"], 600);
}

#[tokio::test]
async fn the_runner_report_is_limited_sanitised_and_for_runner_bots_only() {
    let scratch = scratch_or_skip!("bc_report");
    let db = &scratch.db;
    let w = world(db, "q1").await;
    let connected = expect_connect(&w, w.tenant.project_id, new_bot("Reporting bot")).await;
    let bot = connected.connection.bot.id;
    let runner_bot = bot_context_for(bot, w.tenant.workspace_id, &["read", "write"]);
    let good = serde_json::to_vec(&json!({
        "schema": "sylvode.doctor.v1",
        "generated_by": {"binary": "sylvode-runner", "version": "0.5.1", "task_protocol": 1},
        "components": [
            {"component": "tmux", "status": "ok", "version": "3.4", "detail": "/usr/bin/tmux"},
            {"component": "cli.claude", "status": "ok", "version": "2.1.0", "hint": "/srv/dev/.claude"}
        ],
        "projects": [{"working_directory": "/srv/dev/work"}]
    }))
    .expect("json");

    // A person is refused; so is a bot that no connection made.
    match report_as(&w, None, &good).await {
        Err(ApiError::Forbidden(message)) => assert_eq!(message, "only a runner bot can report"),
        other => panic!("expected a 403, got {:?}", other.map(|_| ())),
    }
    let bots_page_bot = seed_bot_user(db, &w.tenant).await;
    let unconnected = bot_context_for(bots_page_bot, w.tenant.workspace_id, &["read", "write"]);
    assert!(matches!(
        report_as(&w, Some(unconnected), &good).await,
        Err(ApiError::NotFound(_))
    ));
    assert_eq!(
        n(db, "SELECT 1 FROM bot_runners WHERE report IS NOT NULL", vec![]).await,
        0
    );

    // The runner's own report is stored, reduced to names, states and versions.
    let accepted = report_as(&w, Some(runner_bot.clone()), &good).await.expect("accepted");
    let (_, answer) = body_of(accepted).await;
    assert_eq!(answer["data"], json!({"accepted": true, "interval_secs": 600}));
    let stored = text_of(
        db,
        "SELECT report::text AS v FROM bot_runners WHERE bot_id = $1",
        vec![bot.into()],
    )
    .await;
    assert!(!stored.contains('/'), "no path reaches the database: {stored}");
    assert!(
        !stored.contains("detail") && !stored.contains("hint") && !stored.contains("projects"),
        "{stored}"
    );
    let stored: Value = serde_json::from_str(&stored).expect("json");
    assert_eq!(stored["generated_by"], json!({"version": "0.5.1", "task_protocol": 1}));
    assert_eq!(stored["components"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM bot_runners WHERE bot_id = $1 AND reported_at IS NOT NULL",
            vec![bot.into()]
        )
        .await,
        1
    );

    // Over 16 KiB is a 413 and replaces nothing.
    let huge = serde_json::to_vec(&json!({"components": [], "padding": "x".repeat(17 * 1024)})).expect("json");
    let refused = report_as(&w, Some(runner_bot.clone()), &huge).await.expect("answers");
    assert_eq!(refused.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    let exactly = serde_json::to_vec(&json!({"components": [], "padding": "x".repeat(16 * 1024 - 40)})).expect("json");
    assert!(exactly.len() <= 16 * 1024);
    assert!(report_as(&w, Some(runner_bot.clone()), &exactly).await.is_ok());
    // Not a JSON object is a 400.
    for body in [&b"not json"[..], b"[]", b"3"] {
        assert!(matches!(
            report_as(&w, Some(runner_bot.clone()), body).await,
            Err(ApiError::BadRequest(_))
        ));
    }
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_status_follows_token_use_reports_and_deliveries() {
    let scratch = scratch_or_skip!("bc_status");
    let db = &scratch.db;
    let w = world(db, "u1").await;
    let project = w.tenant.project_id;
    let connected = expect_connect(&w, project, new_bot("Status bot")).await;
    let bot = connected.connection.bot.id;
    let runner_bot = bot_context_for(bot, w.tenant.workspace_id, &["read", "write"]);

    // Right after connecting nothing has used the new credentials.
    let status = status_json(&w, project).await;
    assert_eq!(status["runner"]["state"], "waiting");
    assert_eq!(status["cli"]["state"], "unknown");
    assert_eq!(status["tmux"]["state"], "unknown");
    assert_eq!(status["last_delivery"]["state"], "none");
    // The connect answer carries it too.
    let answered = route::connect_bot(
        State(w.state.clone()),
        Extension(claims_for(w.owner)),
        None,
        Path(second_project(db, &w, "PU2").await),
        HeaderMap::new(),
        Json(request(existing_bot(bot))),
    )
    .await
    .expect("connect");
    assert_eq!(
        body_of(answered).await.1["data"]["status"]["runner"]["state"],
        "waiting"
    );

    // The runner uses its token: used, but not reporting.
    exec(
        db,
        "UPDATE workspace_bots SET last_used_at = now() + interval '1 second' WHERE id = $1",
        vec![bot.into()],
    )
    .await;
    assert_eq!(status_json(&w, project).await["runner"]["state"], "not_reporting");
    // A report makes it online and names the CLI and tmux.
    let report = serde_json::to_vec(&json!({
        "generated_by": {"version": "0.5.1", "task_protocol": 1},
        "components": [
            {"component": "tmux", "status": "ok", "version": "3.4"},
            {"component": "cli.claude", "status": "warn", "version": "2.1.0"}
        ]
    }))
    .expect("json");
    report_as(&w, Some(runner_bot), &report).await.expect("report");
    let online = status_json(&w, project).await;
    assert_eq!(online["runner"]["state"], "online");
    assert_eq!(online["runner"]["version"], "0.5.1");
    assert_eq!(online["runner"]["task_protocol"], 1);
    assert_eq!(
        online["cli"],
        json!({"state": "warning", "source": "report", "version": "2.1.0"})
    );
    assert_eq!(
        online["tmux"],
        json!({"state": "ready", "source": "report", "version": "3.4"})
    );
    assert_eq!(
        online["tmux_session"],
        Value::Null,
        "the report has no row for this session"
    );
    assert_eq!(online["bypass"], Value::Null);
    // A report with the session row of this connection and the posture row.
    let key: String = text_of(db, "SELECT key AS v FROM projects WHERE id = $1", vec![project.into()]).await;
    exec(
        db,
        "UPDATE bot_runners SET report = jsonb_set(report, '{components}', report->'components' || $2::jsonb) \
         WHERE bot_id = $1",
        vec![
            bot.into(),
            json!([
                {"component": "tmux_session", "project_key": key, "cli": "claude", "status": "warn",
                 "state": "running", "created_by": "operator", "hooks": "seen", "permissions": "bypass",
                 "mcp": "ok"},
                {"component": "posture", "status": "warn", "bypass": {"tmux": true}}
            ])
            .into(),
        ],
    )
    .await;
    let with_session = status_json(&w, project).await;
    assert_eq!(
        with_session["tmux_session"],
        json!({"session_name": format!("{key}-claude"), "status": "warn", "state": "running",
               "created_by": "operator", "hooks": "seen", "permissions": "bypass", "mcp": "ok"})
    );
    assert_eq!(with_session["bypass"], true);
    // A stale report stops counting.
    exec(
        db,
        "UPDATE bot_runners SET reported_at = now() - interval '2 hours' WHERE bot_id = $1",
        vec![bot.into()],
    )
    .await;
    let stale = status_json(&w, project).await;
    assert_eq!(stale["runner"]["state"], "not_reporting");
    assert_eq!(stale["cli"]["state"], "unknown");
    assert_eq!(stale["tmux_session"], Value::Null);
    assert_eq!(stale["bypass"], Value::Null);

    // Deliveries: a refusal with its code, then a failed delivery.
    let task = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, attempts, max_attempts) \
         VALUES ($1, $2, $3, 'issue_assigned', 'failed', 1, 3)",
        vec![task.into(), project.into(), bot.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO ai_task_events (task_id, event_type, payload, created_at) VALUES \
         ($1, 'failed', $2, now() - interval '10 minutes')",
        vec![
            task.into(),
            json!({"reason": "runner_declined", "reason_code": "cli_unavailable",
                   "error_message": "the runner refused the task (cli_unavailable): ".to_string() + &"d".repeat(300)})
            .into(),
        ],
    )
    .await;
    let refused = status_json(&w, project).await;
    assert_eq!(refused["last_delivery"]["state"], "refused");
    assert_eq!(refused["last_delivery"]["reason_code"], "cli_unavailable");
    assert_eq!(refused["last_delivery"]["task_id"], json!(task));
    assert_eq!(refused["last_delivery"]["project_key"], "Pu1");
    assert_eq!(
        refused["last_delivery"]["detail"]
            .as_str()
            .map(|detail| detail.chars().count()),
        Some(200),
        "the first 200 characters of the text"
    );
    assert_eq!(
        refused["cli"],
        json!({"state": "missing", "source": "delivery", "version": null})
    );
    exec(
        db,
        "INSERT INTO ai_task_events (task_id, event_type, payload) VALUES ($1, 'retried', $2)",
        vec![
            task.into(),
            json!({"reason": "delivery_failed", "error_message": "the runner could not be reached: x"}).into(),
        ],
    )
    .await;
    let failed = status_json(&w, project).await;
    assert_eq!(failed["last_delivery"]["state"], "failed");
    assert_eq!(failed["runner"]["state"], "unreachable");
    // An accepted delivery makes the CLI ready again.
    exec(
        db,
        "INSERT INTO ai_task_events (task_id, event_type, payload) VALUES ($1, 'delivered', '{}'::jsonb)",
        vec![task.into()],
    )
    .await;
    let delivered = status_json(&w, project).await;
    assert_eq!(delivered["last_delivery"]["state"], "delivered");
    assert_eq!(delivered["cli"]["state"], "ready");
    assert_eq!(delivered["runner"]["state"], "not_reporting");
    // Events older than 30 days are not evidence.
    exec(
        db,
        "UPDATE ai_task_events SET created_at = now() - interval '31 days' WHERE task_id = $1",
        vec![task.into()],
    )
    .await;
    assert_eq!(status_json(&w, project).await["last_delivery"]["state"], "none");
    scratch.drop_self().await;
}

mod capture {
    use std::sync::Arc;

    use parking_lot::Mutex;

    pub struct Capture(pub Arc<Mutex<String>>);

    impl tracing::Subscriber for Capture {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            use std::fmt::Write as _;
            struct Visitor<'a>(&'a mut String);
            impl tracing::field::Visit for Visitor<'_> {
                fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                    let _ = write!(self.0, "{}={value:?} ", field.name());
                }
            }
            let mut line = String::new();
            event.record(&mut Visitor(&mut line));
            let mut sink = self.0.lock();
            sink.push_str(&line);
            sink.push('\n');
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }
}

/// The token, the signing secret and the block text appear in no log line, no error text, no
/// business event, and no stored column other than the hash and the webhook's own secret column.
#[tokio::test]
async fn no_secret_reaches_a_log_an_event_or_an_error() {
    let scratch = scratch_or_skip!("bc_secrets");
    let db = &scratch.db;
    let w = world(db, "x1").await;
    let p2 = second_project(db, &w, "PX2").await;
    let captured = Arc::new(parking_lot::Mutex::new(String::new()));
    let _guard = tracing::subscriber::set_default(capture::Capture(Arc::clone(&captured)));

    let first = expect_connect(&w, w.tenant.project_id, new_bot("Quiet bot")).await;
    let bot = first.connection.bot.id;
    let block = first.config_block.expect("block");
    let mut secrets = vec![block_token(&block), block_secret(&block), block.text.clone()];
    expect_connect(&w, p2, existing_bot(bot)).await;
    // A refused connect: its error text carries no secret either.
    let refusal = format!("{:?}", connect(&w, p2, new_bot("Another")).await.map(|_| ()));
    let Ok(Outcome::Done(rotated)) = rotate(&w, w.tenant.project_id, 1).await else {
        panic!("rotate failed");
    };
    secrets.push(block_token(&rotated.config_block));
    secrets.push(block_secret(&rotated.config_block));
    secrets.push(rotated.config_block.text.clone());
    let debugged = format!("{rotated:?}");
    let current = rotated.connection.version;
    let _ = update(
        &w,
        w.tenant.project_id,
        json!({"expected_version": current, "cli": "codex"}),
    )
    .await;
    let _ = remove(&w, p2, 2).await;
    let _ = remove(&w, w.tenant.project_id, current + 1).await;

    let logs = captured.lock().clone();
    for secret in &secrets {
        assert!(!logs.contains(secret.as_str()), "a log line carries a secret");
        assert!(!refusal.contains(secret.as_str()), "an error carries a secret");
        assert!(
            !debugged.contains(secret.as_str()),
            "a Debug rendering carries a secret"
        );
        assert_eq!(
            n(
                db,
                "SELECT 1 FROM business_events \
                 WHERE payload::text LIKE $1 OR metadata::text LIKE $1 OR source::text LIKE $1",
                vec![format!("%{secret}%").into()]
            )
            .await,
            0,
            "a business event carries a secret"
        );
    }
    assert!(!logs.contains("opr_"), "no token-shaped text in any log line: {logs}");
    // Events carry the token prefix and nothing longer.
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM business_events WHERE payload::text ~ 'opr_[0-9a-f]{9,}'",
            vec![]
        )
        .await,
        0
    );
    scratch.drop_self().await;
}

/// The delivery hold compares `credentials_issued_at` with `last_used_at`; both come from the
/// database's clock, so the API host's clock never decides it. The write is one SQL statement with
/// `now()`, and a request through the real bot authentication sets it.
#[tokio::test]
async fn token_use_is_stamped_by_the_database_clock() {
    let source = include_str!("../middleware/bot_auth.rs");
    let update = source
        .lines()
        .find(|line| line.contains("UPDATE workspace_bots SET last_used_at"))
        .expect("the last-use update");
    assert!(update.contains("last_used_at = now()"), "{update}");
    assert!(
        update.contains("WHERE id = $1"),
        "the only parameter is the bot id: {update}"
    );

    let scratch = scratch_or_skip!("bc_clock");
    let db = &scratch.db;
    let w = world(db, "ck").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Clock bot")).await;
    let token = block_token(first.config_block.as_ref().expect("block"));
    let bot = first.connection.bot.id;
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND last_used_at IS NULL",
            vec![bot.into()]
        )
        .await,
        1
    );
    assert!(authenticated(&protocol_with(db, &token, Some("mcp_stdio")).await));
    // The stamp is written after the request, in the background: wait for it, bounded.
    let mut stamped = 0;
    for _ in 0..50 {
        stamped = n(
            db,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND last_used_at IS NOT NULL AND last_used_at <= now()",
            vec![bot.into()],
        )
        .await;
        if stamped == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(stamped, 1);
    scratch.drop_self().await;
}

/// Reports are limited per bot (12 a minute), and the route refuses a body over 16 KiB before the
/// handler reads it.
#[tokio::test]
async fn the_report_route_is_rate_limited_and_body_limited() {
    let scratch = scratch_or_skip!("bc_report_limits");
    let db = &scratch.db;
    let w = world(db, "rl").await;
    let connected = expect_connect(&w, w.tenant.project_id, new_bot("Limited bot")).await;
    let runner_bot = bot_context_for(connected.connection.bot.id, w.tenant.workspace_id, &["read", "write"]);
    let body = serde_json::to_vec(&json!({"components": []})).expect("json");
    for round in 0..12 {
        let answer = report_as(&w, Some(runner_bot.clone()), &body).await.expect("answers");
        assert_eq!(answer.status(), axum::http::StatusCode::OK, "report {round}");
    }
    let limited = report_as(&w, Some(runner_bot.clone()), &body).await.expect("answers");
    assert_eq!(limited.status(), axum::http::StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().contains_key(header::RETRY_AFTER));
    let (_, answer) = body_of(limited).await;
    assert_eq!(answer["code"], 429);

    // The route's body limit, built as the router builds it: the body is cut off before the handler.
    assert!(
        crate::router_source::text().contains("runner_report_body_limit()"),
        "the router applies it"
    );
    let app = Router::new()
        .route(
            "/report",
            axum::routing::put(crate::routes::ai_task_protocol::put_runner_report)
                .layer(crate::routes::ai_task_protocol::runner_report_body_limit()),
        )
        .with_state(w.state.clone());
    let huge = vec![b'x'; 64 * 1024];
    let response = app
        .oneshot(Request::put("/report").body(Body::from(huge)).expect("request"))
        .await
        .expect("answer");
    assert_eq!(response.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    scratch.drop_self().await;
}

/// A caller who may not manage or read the connection gets the same answer for a project that
/// exists and for one that does not: authorization comes before the existence check.
#[tokio::test]
async fn an_outsider_cannot_tell_a_missing_project_from_one_that_exists() {
    let scratch = scratch_or_skip!("bc_oracle");
    let db = &scratch.db;
    let w = world(db, "or").await;
    let stranger = seed_tenant(db, "or2").await.member_id;
    let missing = Uuid::new_v4();
    let version = |expected_version: i32| route::VersionBody { expected_version };
    let describe = |result: Result<Response, ApiError>| match result {
        Err(error) => format!("{error:?}"),
        Ok(_) => "answered".to_string(),
    };
    let bot = Uuid::new_v4();
    for project in [w.tenant.project_id, missing] {
        let claims = || Extension(claims_for(stranger));
        let state = || State(w.state.clone());
        let answers = [
            describe(route::get_bot_connection(state(), claims(), None, Path((project, bot))).await),
            describe(route::list_bot_connections(state(), claims(), None, Path(project)).await),
            describe(
                route::connect_bot(
                    state(),
                    claims(),
                    None,
                    Path(project),
                    HeaderMap::new(),
                    Json(request(new_bot("Oracle bot"))),
                )
                .await,
            ),
            describe(
                route::disable_bot_connection(state(), claims(), None, Path((project, bot)), Json(version(1))).await,
            ),
            describe(
                route::rotate_bot_connection(
                    state(),
                    claims(),
                    None,
                    Path((project, bot)),
                    HeaderMap::new(),
                    Json(version(1)),
                )
                .await,
            ),
            describe(
                route::remove_bot_connection(
                    state(),
                    claims(),
                    None,
                    Path((project, bot)),
                    Query(route::RemoveQuery { expected_version: 1 }),
                )
                .await,
            ),
        ];
        for answer in &answers {
            assert!(answer.starts_with("Forbidden("), "{project}: {answer}");
        }
    }
    scratch.drop_self().await;
}

/// An instance administrator who disables a bot's user stops its token at once, and enabling the
/// user brings it back: the token path reads `users.is_active` as well as the token's own flag.
#[tokio::test]
async fn disabling_the_bot_user_stops_its_token() {
    let scratch = scratch_or_skip!("bc_user_disable");
    let db = &scratch.db;
    let w = world(db, "ud").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Disabled user bot")).await;
    let token = block_token(first.config_block.as_ref().expect("block"));
    let bot = first.connection.bot.id;
    assert!(authenticated(&protocol_with(db, &token, Some("mcp_stdio")).await));

    crate::routes::admin::delete_user(State(w.state.clone()), Path(bot))
        .await
        .expect("the admin route disables the user");
    let refused = protocol_with(db, &token, Some("mcp_stdio")).await;
    assert!(!authenticated(&refused), "{refused:?}");
    assert_eq!(refused.1["code"], 401);

    exec(db, "UPDATE users SET is_active = true WHERE id = $1", vec![bot.into()]).await;
    assert!(authenticated(&protocol_with(db, &token, Some("mcp_stdio")).await));
    scratch.drop_self().await;
}
