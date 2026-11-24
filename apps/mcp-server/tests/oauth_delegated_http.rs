//! Delegated (OAuth) access to the MCP server, end to end over real TCP and in-process.
//!
//! The router under test is assembled from the same library pieces `main.rs` wires: the inbound
//! authentication layer, the metadata routes and the JSON-RPC handler. The API is a recording
//! stand-in, and the token-context lookup is either a fake (so each verdict can be chosen) or the
//! shipped HTTP implementation talking to a stand-in endpoint.
//!
//! What every test asserts is what the *API* saw, not only what the caller was told: a refused call
//! must leave the API untouched, and a served one must carry exactly the credentials the contract
//! names.

use axum::{
    Extension, Json, Router,
    extract::{RawQuery, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::from_fn_with_state,
    response::IntoResponse,
    routing::{get, post},
};
use mcp_server::client::{ClientConfig, OpenPrClient};
use mcp_server::oauth::{
    BoxFuture, Caller, ContextOutcome, HttpTokenContextSource, InboundAuth, OAuthSettings, TokenContextSource,
    UnavailableReason, handle_rpc, inbound_auth, parse_token_context, well_known_routes,
};
use mcp_server::protocol::JsonRpcRequest;
use mcp_server::server::{ToolSurface, requested_tool_mode};
use platform::config::{McpOAuthRuntime, McpToolMode, Secret};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Mutex;

type TestResult = Result<(), Box<dyn Error>>;
/// A canned answer of the stand-in token-context endpoint: status, `Location`, body.
type Answer = (StatusCode, Option<String>, String);
/// What the stand-in endpoint saw: method, URI, authorization, forwarded-by, forward-auth.
type ContextRequest = (String, String, Option<String>, Option<String>, Option<String>);

const CONFIGURED_WORKSPACE: &str = "11111111-1111-4111-8111-111111111111";
/// The workspace the stand-in API reports for `BOT_TOKEN` on `GET /api/v1/auth/me`.
const BOT_WORKSPACE: &str = "55555555-5555-4555-8555-555555555555";
const GRANT_WORKSPACE: &str = "77777777-7777-4777-8777-777777777777";
const OTHER_WORKSPACE: &str = "99999999-9999-4999-8999-999999999999";
const PROJECT: &str = "22222222-2222-4222-8222-222222222222";
const FORWARD_SECRET: &str = "forward-secret-for-tests-0123456789abcdef";
const ISSUER: &str = "https://sv.example.test";
const RESOURCE: &str = "http://127.0.0.1:8090/mcp/rpc";
const METADATA_URL: &str = "http://127.0.0.1:8090/.well-known/oauth-protected-resource/mcp/rpc";
const BOT_TOKEN: &str = "opr_caller_bot_token_example";
const JWT_SHAPED_TOKEN: &str = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ0ZXN0In0.c2lnbmF0dXJl";

/// A well formed delegated token made of one hex digit, so each test can have its own.
fn delegated(digit: char) -> String {
    format!("sva_{}", digit.to_string().repeat(64))
}

fn active(scope: &str) -> ContextOutcome {
    active_until(scope, 4_102_444_800)
}

fn active_until(scope: &str, exp: i64) -> ContextOutcome {
    parse_token_context(&json!({
        "active": true,
        "client_id": "ramflux",
        "sub": "33333333-3333-4333-8333-333333333333",
        "grant_id": "44444444-4444-4444-8444-444444444444",
        "workspace_id": GRANT_WORKSPACE,
        "scope": scope,
        "project_mode": "selected",
        "project_ids": [PROJECT],
        "exp": exp,
    }))
}

// ---- Stand-in API ----------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
    forwarded_by: Option<String>,
    forward_auth: Option<String>,
}

type SeenLog = Arc<Mutex<Vec<Seen>>>;

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

async fn record_and_answer(State(log): State<SeenLog>, request: Request) -> Json<Value> {
    let path = request.uri().path().to_string();
    log.lock().await.push(Seen {
        method: request.method().to_string(),
        path: path.clone(),
        authorization: header(request.headers(), "authorization"),
        forwarded_by: header(request.headers(), "x-sylvode-forwarded-by"),
        forward_auth: header(request.headers(), "x-sylvode-forward-auth"),
    });
    if path.ends_with("/agent-policy") {
        Json(json!({"code": 0, "message": "ok", "data": {}}))
    } else if path == "/api/v1/auth/me" {
        // A bot token names its bot and workspace; every other token is a person.
        let bot = (header(request.headers(), "authorization").as_deref() == Some(&format!("Bearer {BOT_TOKEN}")))
            .then(|| json!({"bot_id": PROJECT, "workspace_id": BOT_WORKSPACE, "permissions": ["read", "write"], "project_scope": "workspace"}));
        Json(json!({"code": 0, "message": "ok", "data": {"user": {}, "bot": bot}}))
    } else {
        Json(json!({"code": 0, "message": "ok", "data": []}))
    }
}

async fn spawn(router: Router) -> Result<String, Box<dyn Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(format!("http://{addr}"))
}

async fn spawn_api() -> Result<(String, SeenLog), Box<dyn Error>> {
    let log: SeenLog = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new().fallback(record_and_answer).with_state(Arc::clone(&log));
    Ok((spawn(router).await?, log))
}

// ---- Fake token-context source ---------------------------------------------------------------

struct FakeSource {
    answers: HashMap<String, ContextOutcome>,
    calls: AtomicUsize,
}

impl FakeSource {
    fn new(answers: impl IntoIterator<Item = (String, ContextOutcome)>) -> Arc<Self> {
        Arc::new(Self {
            answers: answers.into_iter().collect(),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl TokenContextSource for FakeSource {
    fn resolve<'a>(&'a self, token: &'a Secret) -> BoxFuture<'a, ContextOutcome> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let answer = self
            .answers
            .get(token.expose())
            .cloned()
            .unwrap_or(ContextOutcome::Invalid);
        Box::pin(async move { answer })
    }
}

// ---- The server under test -------------------------------------------------------------------

#[derive(Clone)]
struct AppState {
    client: OpenPrClient,
    inbound: Arc<InboundAuth>,
}

/// The production handler's few lines: the connection's `?tools=` / header choice, then
/// `handle_rpc`. The server's own default is `search`, as in a deployment without `tool_mode`.
async fn rpc(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> axum::response::Response {
    let mode = match requested_tool_mode(query.as_deref(), &headers, McpToolMode::Search) {
        Ok(mode) => mode,
        Err(message) => return (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response(),
    };
    handle_rpc(
        &state.client,
        &state.inbound,
        caller,
        ToolSurface::new(mode, true),
        request,
    )
    .await
}

async fn sse_stub(Extension(caller): Extension<Caller>) -> StatusCode {
    if caller.into_legacy().is_some() {
        StatusCode::OK
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn runtime(resource: &str) -> McpOAuthRuntime {
    McpOAuthRuntime {
        issuer: ISSUER.to_string(),
        resource: resource.to_string(),
        forward_secret: Secret::new(FORWARD_SECRET),
    }
}

fn router(client: OpenPrClient, inbound: Arc<InboundAuth>) -> Router {
    let protected = Router::new()
        .route("/mcp/rpc", post(rpc))
        .route("/sse", get(sse_stub))
        .route_layer(from_fn_with_state(Arc::clone(&inbound), inbound_auth));
    let mut app = Router::new().merge(protected);
    if let Some(settings) = inbound.settings() {
        app = app.merge(well_known_routes(settings));
    }
    app.with_state(AppState { client, inbound })
}

struct Harness {
    base: String,
    api: SeenLog,
    http: reqwest::Client,
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    text: String,
}

impl Reply {
    fn json(&self) -> Result<Value, Box<dyn Error>> {
        Ok(serde_json::from_str(&self.text)?)
    }

    fn challenge(&self) -> Result<Vec<(String, String)>, Box<dyn Error>> {
        let value = header(&self.headers, "www-authenticate").ok_or("no WWW-Authenticate header")?;
        parse_challenge(&value)
    }
}

/// Parses the parameters of a `Bearer` challenge made of quoted values.
fn parse_challenge(challenge: &str) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let rest = challenge.strip_prefix("Bearer ").ok_or("not a Bearer challenge")?;
    let mut params = Vec::new();
    for part in rest.split(", ") {
        let (name, value) = part.split_once('=').ok_or("parameter without a value")?;
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .ok_or("value is not quoted")?;
        params.push((name.to_string(), value.to_string()));
    }
    Ok(params)
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

impl Harness {
    async fn with(inbound: InboundAuth) -> Result<Self, Box<dyn Error>> {
        log_sink();
        let (api_url, api) = spawn_api().await?;
        let client = OpenPrClient::new(ClientConfig {
            base_url: api_url,
            credential: None,
            workspace_id: Some(CONFIGURED_WORKSPACE.to_string()),
            transport_label: "mcp_http",
        })?;
        let base = spawn(router(client, Arc::new(inbound))).await?;
        Ok(Self {
            base,
            api,
            http: reqwest::Client::new(),
        })
    }

    async fn delegated(source: &Arc<FakeSource>) -> Result<Self, Box<dyn Error>> {
        let settings = OAuthSettings::from_runtime(&runtime(RESOURCE))?;
        Self::with(InboundAuth::delegated(
            settings,
            Arc::clone(source) as Arc<dyn TokenContextSource>,
        ))
        .await
    }

    async fn legacy_only() -> Result<Self, Box<dyn Error>> {
        Self::with(InboundAuth::legacy_only()).await
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Reply, Box<dyn Error>> {
        let response = request.send().await?;
        let status = response.status();
        let headers = response.headers().clone();
        let text = response.text().await?;
        Ok(Reply { status, headers, text })
    }

    async fn post(&self, token: Option<&str>, body: &Value) -> Result<Reply, Box<dyn Error>> {
        self.post_to("/mcp/rpc", token, body).await
    }

    /// `post` with the whole registry listed and callable: `?tools=full`.
    async fn post_full(&self, token: Option<&str>, body: &Value) -> Result<Reply, Box<dyn Error>> {
        self.post_to("/mcp/rpc?tools=full", token, body).await
    }

    async fn post_to(&self, path: &str, token: Option<&str>, body: &Value) -> Result<Reply, Box<dyn Error>> {
        let mut request = self.http.post(format!("{}{path}", self.base)).json(body);
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        self.send(request).await
    }

    async fn get(&self, path: &str, token: Option<&str>) -> Result<Reply, Box<dyn Error>> {
        let mut request = self.http.get(format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        self.send(request).await
    }

    async fn seen(&self) -> Vec<Seen> {
        self.api.lock().await.clone()
    }
}

fn call(name: &str, arguments: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": name, "arguments": arguments}})
}

fn list() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
}

fn listed(reply: &Reply) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let body = reply.json()?;
    let tools = body
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .ok_or("no tools array")?;
    Ok(tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect())
}

fn names(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|name| (*name).to_string()).collect()
}

const WORK_ITEM_READS: [&str; 6] = [
    "work_items.list",
    "work_items.get",
    "work_items.get_by_identifier",
    "work_items.list_labels",
    "comments.list",
    "work_items.list_pages",
];
/// The form routes that are delegated: reads under `forms:read`, record writes under `forms:write`.
const FORM_READS: [&str; 4] = ["forms.list", "forms.get", "form_records.list", "form_records.get"];
const FORM_WRITES: [&str; 2] = ["form_records.create", "form_records.update"];
/// Read tools that exist for bot and user tokens but that no delegated caller gets, because the
/// API routes they call are not on the delegated allow-list.
const WITHHELD_READS: [&str; 5] = [
    "work_items.search",
    "labels.list_by_project",
    "forms.schema_summary",
    "form_views.list",
    "form_records.children",
];
const PROJECT_READS: [&str; 3] = ["projects.list", "projects.get", "context.get_project"];

// ---- Metadata and challenges -----------------------------------------------------------------

#[tokio::test]
async fn the_metadata_document_is_served_at_both_paths_and_only_when_configured() -> TestResult {
    let source = FakeSource::new([]);
    let server = Harness::delegated(&source).await?;
    let expected = json!({
        "resource": RESOURCE,
        "authorization_servers": [ISSUER],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["projects:read", "work_items:read", "work_items:write", "forms:read", "forms:write"],
        "resource_name": "Sylvode MCP"
    });
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp/rpc",
    ] {
        let reply = server.get(path, None).await?;
        assert_eq!(reply.status, StatusCode::OK, "{path}");
        assert_eq!(reply.json()?, expected, "{path}");
        assert_eq!(
            header(&reply.headers, "cache-control").as_deref(),
            Some("public, max-age=3600")
        );
        let content_type = header(&reply.headers, "content-type").ok_or("no content type")?;
        assert!(content_type.starts_with("application/json"), "{content_type}");
        let document = reply.json()?;
        let map = document.as_object().ok_or("not an object")?;
        assert_eq!(map.len(), 5, "no field beyond the contract's");
        for (key, value) in expected.as_object().ok_or("not an object")? {
            assert_eq!(map.get(key), Some(value), "{path}: {key}");
        }
    }
    // The metadata needs no token and asks the API nothing.
    assert_eq!(source.calls(), 0);
    assert!(server.seen().await.is_empty());
    // Another path is not a metadata document.
    assert_eq!(
        server
            .get("/.well-known/oauth-protected-resource/other", None)
            .await?
            .status,
        StatusCode::NOT_FOUND
    );

    let unconfigured = Harness::legacy_only().await?;
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp/rpc",
    ] {
        assert_eq!(
            unconfigured.get(path, None).await?.status,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_request_without_a_token_gets_the_discovery_challenge() -> TestResult {
    let source = FakeSource::new([]);
    let server = Harness::delegated(&source).await?;
    for token_header in [None, Some("Basic abc"), Some("Bearer"), Some("Bearer   ")] {
        let mut request = server.http.post(format!("{}/mcp/rpc", server.base)).json(&list());
        if let Some(value) = token_header {
            request = request.header("Authorization", value);
        }
        let reply = server.send(request).await?;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{token_header:?}");
        let params = reply.challenge()?;
        assert_eq!(param(&params, "realm"), Some("sylvode"));
        assert_eq!(param(&params, "resource_metadata"), Some(METADATA_URL));
        assert_eq!(param(&params, "scope"), Some("projects:read work_items:read"));
        assert_eq!(param(&params, "error"), None, "a missing token is not an invalid one");
    }
    assert_eq!(source.calls(), 0);
    assert!(server.seen().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn without_oauth_the_missing_token_answer_is_what_it_always_was() -> TestResult {
    let server = Harness::legacy_only().await?;
    let reply = server.post(None, &list()).await?;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    assert_eq!(header(&reply.headers, "www-authenticate").as_deref(), Some("Bearer"));
    let body = reply.json()?;
    let text = body.get("error").and_then(Value::as_str).ok_or("no error text")?;
    assert!(text.starts_with("Missing Authorization: Bearer <caller credential>."));
    assert!(server.seen().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_malformed_delegated_token_is_refused_without_asking_the_api() -> TestResult {
    let source = FakeSource::new([]);
    let server = Harness::delegated(&source).await?;
    let upper = format!("sva_{}", "A".repeat(64));
    let long = format!("{}0", delegated('a'));
    let short = "sva_abc".to_string();
    let bare = "sva_".to_string();
    let wrong_alphabet = format!("sva_{}", "g".repeat(64));
    let mut replies = Vec::new();
    for token in [&upper, &long, &short, &bare, &wrong_alphabet] {
        let reply = server.post(Some(token), &list()).await?;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{token}");
        let params = reply.challenge()?;
        assert_eq!(param(&params, "error"), Some("invalid_token"));
        assert_eq!(param(&params, "resource_metadata"), Some(METADATA_URL));
        replies.push((reply.text.clone(), header(&reply.headers, "www-authenticate")));
    }
    assert!(
        replies.windows(2).all(|pair| pair.first() == pair.get(1)),
        "one answer for all"
    );
    assert_eq!(
        source.calls(),
        0,
        "a token that is not shaped like one is not looked up"
    );
    assert!(server.seen().await.is_empty());
    Ok(())
}

/// Unknown, expired, revoked and wrong-audience tokens are told apart by nobody outside the API:
/// the caller gets one answer for all of them, down to the byte.
#[tokio::test]
async fn unknown_expired_revoked_and_wrong_audience_tokens_get_one_identical_answer() -> TestResult {
    let source = FakeSource::new([
        (delegated('2'), ContextOutcome::Invalid),
        (delegated('3'), parse_token_context(&json!({"active": false}))),
        (delegated('4'), active_until("projects:read", 1)),
    ]);
    let server = Harness::delegated(&source).await?;
    let mut answers = Vec::new();
    // `1` is unknown to the source; `2` and `3` stand for revoked and wrong audience (the API says
    // 401 or inactive); `4` is a token whose expiry has passed by this server's clock.
    for digit in ['1', '2', '3', '4'] {
        let reply = server.post(Some(&delegated(digit)), &list()).await?;
        assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{digit}");
        let params = reply.challenge()?;
        assert_eq!(param(&params, "error"), Some("invalid_token"));
        assert_eq!(param(&params, "resource_metadata"), Some(METADATA_URL));
        assert_eq!(param(&params, "scope"), None);
        assert_eq!(param(&params, "error_description"), None);
        answers.push((reply.status, header(&reply.headers, "www-authenticate"), reply.text));
    }
    assert!(
        answers.windows(2).all(|pair| pair.first() == pair.get(1)),
        "the four answers differ: {answers:?}"
    );
    assert_eq!(source.calls(), 4);
    assert!(server.seen().await.is_empty());
    Ok(())
}

#[tokio::test]
async fn a_delegated_token_is_never_forwarded_when_delegated_access_is_off() -> TestResult {
    let server = Harness::legacy_only().await?;
    let reply = server
        .post(Some(&delegated('a')), &call("projects.list", &json!({})))
        .await?;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED);
    let params = reply.challenge()?;
    assert_eq!(param(&params, "error"), Some("invalid_token"));
    assert_eq!(param(&params, "resource_metadata"), None, "no metadata is advertised");
    assert!(server.seen().await.is_empty(), "the token reached the API");
    Ok(())
}

#[tokio::test]
async fn a_delegated_token_is_refused_on_every_other_endpoint() -> TestResult {
    let source = FakeSource::new([(delegated('a'), active("projects:read"))]);
    let server = Harness::delegated(&source).await?;
    let reply = server.get("/sse", Some(&delegated('a'))).await?;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    assert_eq!(source.calls(), 0);
    // A legacy token is served there as before.
    assert_eq!(server.get("/sse", Some(BOT_TOKEN)).await?.status, StatusCode::OK);
    Ok(())
}

#[tokio::test]
async fn the_token_is_not_accepted_from_a_query_string_or_a_cookie() -> TestResult {
    let source = FakeSource::new([(delegated('a'), active("projects:read"))]);
    let server = Harness::delegated(&source).await?;
    let token = delegated('a');
    let by_query = server
        .send(
            server
                .http
                .post(format!("{}/mcp/rpc?access_token={token}", server.base))
                .json(&list()),
        )
        .await?;
    assert_eq!(by_query.status, StatusCode::UNAUTHORIZED);
    let by_cookie = server
        .send(
            server
                .http
                .post(format!("{}/mcp/rpc", server.base))
                .header("Cookie", format!("access_token={token}"))
                .json(&list()),
        )
        .await?;
    assert_eq!(by_cookie.status, StatusCode::UNAUTHORIZED);
    assert_eq!(source.calls(), 0);
    Ok(())
}

// ---- tools/list ------------------------------------------------------------------------------

#[tokio::test]
async fn tools_list_contains_exactly_the_tools_the_scopes_allow() -> TestResult {
    let cases: Vec<(&str, BTreeSet<String>)> = vec![
        ("projects:read", names(&PROJECT_READS)),
        ("work_items:read", names(&WORK_ITEM_READS)),
        ("forms:read", names(&FORM_READS)),
        (
            "projects:read work_items:read",
            names(&[&PROJECT_READS[..], &WORK_ITEM_READS[..]].concat()),
        ),
        // A write scope implies its read scope. `work_items:write` unlocks nothing that changes a
        // work item; `forms:write` unlocks the two record writes.
        ("work_items:write", names(&WORK_ITEM_READS)),
        ("forms:write", names(&[&FORM_READS[..], &FORM_WRITES[..]].concat())),
        (
            "projects:read work_items:read work_items:write forms:read forms:write",
            names(
                &[
                    &PROJECT_READS[..],
                    &WORK_ITEM_READS[..],
                    &FORM_READS[..],
                    &FORM_WRITES[..],
                ]
                .concat(),
            ),
        ),
    ];
    for (scope, expected) in cases {
        let source = FakeSource::new([(delegated('a'), active(scope))]);
        let server = Harness::delegated(&source).await?;
        let reply = server.post(Some(&delegated('a')), &list()).await?;
        assert_eq!(reply.status, StatusCode::OK, "{scope}");
        assert_eq!(listed(&reply)?, expected, "{scope}");
        assert!(
            server.seen().await.is_empty(),
            "tools/list without a project asks the API nothing"
        );
    }
    Ok(())
}

/// A plain caller gets search mode unless its connection asks for the registry: nine tools by
/// default, the whole registry with `?tools=full`. The connection's choice never touches a
/// delegated caller (see the tests above, which list exactly the scoped tools).
#[tokio::test]
async fn a_legacy_caller_gets_search_mode_and_the_registry_on_request() -> TestResult {
    let source = FakeSource::new([]);
    let server = Harness::delegated(&source).await?;
    let search_mode = names(&[
        "sylvode_overview",
        "sylvode_search",
        "sylvode_read",
        "sylvode_write",
        "sylvode_task_get",
        "sylvode_task_ack",
        "sylvode_task_progress",
        "sylvode_task_complete",
        "sylvode_task_fail",
    ]);
    for token in [BOT_TOKEN, JWT_SHAPED_TOKEN] {
        let reply = server.post(Some(token), &list()).await?;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(listed(&reply)?, search_mode, "{token}");
        let full = server.post_full(Some(token), &list()).await?;
        assert_eq!(full.status, StatusCode::OK);
        assert_eq!(listed(&full)?.len(), 158, "{token}");
        assert!(
            !listed(&full)?.contains("sylvode_search"),
            "entry tools exist in search mode only"
        );
    }
    assert_eq!(
        source.calls(),
        0,
        "a legacy token is never looked up as a delegated one"
    );
    Ok(())
}

// ---- tools/call ------------------------------------------------------------------------------

#[tokio::test]
async fn an_allowed_call_acts_in_the_token_workspace_with_the_forwarding_credentials() -> TestResult {
    let token = delegated('a');
    let source = FakeSource::new([(token.clone(), active("projects:read work_items:read"))]);
    let server = Harness::delegated(&source).await?;

    let reply = server.post(Some(&token), &call("projects.list", &json!({}))).await?;
    assert_eq!(reply.status, StatusCode::OK);
    let seen = server.seen().await;
    assert_eq!(seen.len(), 1);
    let first = seen.first().ok_or("no request reached the API")?;
    assert_eq!(first.method, "GET");
    assert_eq!(
        first.path,
        format!("/api/v1/workspaces/{GRANT_WORKSPACE}/projects"),
        "the workspace is the token context's, not the configured one"
    );
    assert_eq!(first.authorization.as_deref(), Some(format!("Bearer {token}").as_str()));
    assert_eq!(first.forwarded_by.as_deref(), Some("mcp-server"));
    assert_eq!(first.forward_auth.as_deref(), Some(FORWARD_SECRET));

    // The policy lookup that precedes a project scoped tool carries the same credentials.
    let reply = server
        .post(Some(&token), &call("work_items.list", &json!({"project_id": PROJECT})))
        .await?;
    assert_eq!(reply.status, StatusCode::OK);
    let seen = server.seen().await;
    assert!(seen.len() >= 3, "{seen:?}");
    for request in seen {
        assert_eq!(
            request.authorization.as_deref(),
            Some(format!("Bearer {token}").as_str()),
            "{request:?}"
        );
        assert_eq!(request.forwarded_by.as_deref(), Some("mcp-server"), "{request:?}");
        assert_eq!(request.forward_auth.as_deref(), Some(FORWARD_SECRET), "{request:?}");
    }
    Ok(())
}

#[tokio::test]
async fn a_call_the_scopes_do_not_allow_gets_a_403_naming_the_scope_and_never_reaches_the_api() -> TestResult {
    let token = delegated('a');
    let source = FakeSource::new([(token.clone(), active("projects:read"))]);
    let server = Harness::delegated(&source).await?;

    for (tool, scope) in [
        ("work_items.list", "work_items:read"),
        ("comments.list", "work_items:read"),
    ] {
        let reply = server
            .post(Some(&token), &call(tool, &json!({"project_id": PROJECT})))
            .await?;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{tool}");
        let params = reply.challenge()?;
        assert_eq!(param(&params, "error"), Some("insufficient_scope"));
        assert_eq!(param(&params, "scope"), Some(scope));
        assert_eq!(param(&params, "resource_metadata"), Some(METADATA_URL));
        assert!(param(&params, "error_description").is_some());
        let body = reply.json()?;
        assert_eq!(body.pointer("/error/code").and_then(Value::as_i64), Some(-32001));
        assert_eq!(
            body.pointer("/error/message").and_then(Value::as_str),
            Some("insufficient_scope")
        );
        assert_eq!(
            body.get("id"),
            Some(&json!(7)),
            "the refusal answers the request that was made"
        );
        assert!(
            !reply.text.contains(tool),
            "the refusal names a tool the caller may not see: {}",
            reply.text
        );
    }
    assert!(server.seen().await.is_empty(), "a refused call reached the API");
    Ok(())
}

/// A tool that changes data, administers, deletes or dispatches is refused whatever the token's
/// scopes claim, and an unknown name gets the identical answer.
#[tokio::test]
async fn a_tool_that_is_not_available_is_refused_whatever_the_scopes_claim() -> TestResult {
    let token = delegated('a');
    let source = FakeSource::new([(
        token.clone(),
        active("projects:read work_items:read work_items:write forms:read forms:write"),
    )]);
    let server = Harness::delegated(&source).await?;
    let mut answers = Vec::new();
    for tool in [
        "work_items.create",
        "work_items.update",
        "comments.create",
        "work_items.link_page",
        "work_items.unlink_page",
        "form_records.child_create",
        "work_items.delete",
        "plugins.invoke",
        "proposals.create",
        "projects.create",
        // The task tools act for a bot; a connected application gets the same refusal.
        "sylvode_task_get",
        "sylvode_task_ack",
        "sylvode_task_progress",
        "sylvode_task_complete",
        "sylvode_task_fail",
        "no.such_tool",
    ] {
        let reply = server
            .post(
                Some(&token),
                &call(
                    tool,
                    &json!({"project_id": PROJECT, "task_id": PROJECT, "attempt": 1, "plan": "x", "note": "x"}),
                ),
            )
            .await?;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{tool}");
        assert!(
            reply.headers.get("www-authenticate").is_none(),
            "{tool}: a step-up cannot cure this"
        );
        let body = reply.json()?;
        assert_eq!(
            body.pointer("/error/code").and_then(Value::as_i64),
            Some(-32002),
            "{tool}"
        );
        answers.push(body.pointer("/error/message").cloned());
    }
    assert!(
        answers.windows(2).all(|pair| pair.first() == pair.get(1)),
        "{answers:?}"
    );
    assert_eq!(
        answers.first(),
        Some(&Some(json!("tool not available to connected applications")))
    );
    assert!(server.seen().await.is_empty(), "a refused call reached the API");
    Ok(())
}

/// A project argument outside a grant limited to some projects is refused before the API is
/// called, with one answer for a project that exists, one that does not and one that is not even
/// an id; a project inside the grant goes through, and a grant for all projects is not narrowed.
#[tokio::test]
async fn a_project_argument_outside_the_grant_is_refused_before_the_api_is_called() -> TestResult {
    let other_existing = "55555555-5555-4555-8555-555555555555";
    let unknown = "66666666-6666-4666-8666-666666666666";
    let token = delegated('a');
    let source = FakeSource::new([(token.clone(), active("projects:read work_items:read"))]);
    let server = Harness::delegated(&source).await?;

    let mut answers = Vec::new();
    for project in [other_existing, unknown, "not-a-project-id"] {
        for tool in ["work_items.list", "projects.get", "context.get_project"] {
            let reply = server
                .post(Some(&token), &call(tool, &json!({"project_id": project})))
                .await?;
            assert_eq!(reply.status, StatusCode::FORBIDDEN, "{tool} {project}");
            let body = reply.json()?;
            assert_eq!(body.pointer("/error/code").and_then(Value::as_i64), Some(-32002));
            assert!(
                !reply.text.contains(project) && !reply.text.contains(tool),
                "the refusal repeats what was asked: {}",
                reply.text
            );
            answers.push((reply.status, body.pointer("/error").cloned()));
        }
    }
    assert!(
        answers.windows(2).all(|pair| pair.first() == pair.get(1)),
        "refusals differ: {answers:?}"
    );
    assert!(server.seen().await.is_empty(), "a refused call reached the API");

    // The grant's own project is served, and the API sees the call.
    let inside = server
        .post(Some(&token), &call("work_items.list", &json!({"project_id": PROJECT})))
        .await?;
    assert_eq!(inside.status, StatusCode::OK, "{}", inside.text);
    assert!(!server.seen().await.is_empty());

    // A grant for all projects leaves the project argument to the API.
    let all_token = delegated('b');
    let all_source = FakeSource::new([(
        all_token.clone(),
        parse_token_context(&json!({
            "active": true, "client_id": "ramflux",
            "sub": "33333333-3333-4333-8333-333333333333",
            "grant_id": "44444444-4444-4444-8444-444444444444",
            "workspace_id": GRANT_WORKSPACE, "scope": "projects:read work_items:read",
            "project_mode": "all", "exp": 4_102_444_800_i64,
        })),
    )]);
    let all_server = Harness::delegated(&all_source).await?;
    let reply = all_server
        .post(
            Some(&all_token),
            &call("work_items.list", &json!({"project_id": other_existing})),
        )
        .await?;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text);
    assert!(!all_server.seen().await.is_empty());
    Ok(())
}

/// A value for each required argument of a tool, taken from its own input schema so that a tool
/// added later needs no entry here: ids and identifiers get values the API's routes accept, any
/// other required string a plain word.
fn required_arguments(schema: &Value) -> Value {
    let mut arguments = serde_json::Map::new();
    let properties = schema.get("properties").and_then(Value::as_object);
    for name in schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let property = properties.and_then(|properties| properties.get(name));
        let is_uuid = property
            .and_then(|property| property.get("pattern"))
            .and_then(Value::as_str)
            .is_some_and(|pattern| pattern.contains("[0-9a-f]{8}"));
        let value = if name == "identifier" {
            json!("AAA-1")
        } else if is_uuid || name.ends_with("_id") {
            json!(PROJECT)
        } else {
            json!("word")
        };
        arguments.insert(name.to_string(), value);
    }
    Value::Object(arguments)
}

/// Whether the concrete `path` is an instance of the router's `template`.
fn path_matches(template: &str, path: &str) -> bool {
    let mut wanted = template.split('/');
    let mut actual = path.split('/');
    loop {
        match (wanted.next(), actual.next()) {
            (None, None) => return true,
            (Some(segment), Some(value)) if segment == value => {}
            (Some(segment), Some(value)) if segment.starts_with('{') && !value.is_empty() => {}
            _ => return false,
        }
    }
}

/// Every tool a delegated caller can see must be one the API will serve it: the route the tool
/// really calls (recorded from the call, so no table of routes is kept here) is on the API's
/// delegated allow-list, and the scope the tool requires satisfies the scope that route requires.
/// This is what keeps the MCP tool map and the API allow-list, which live in two crates, from
/// drifting into a tool that is listed and then fails closed.
#[tokio::test]
async fn every_tool_available_to_delegated_callers_calls_a_route_the_api_allows_for_its_scope() -> TestResult {
    use api::oauth::delegated::DELEGATED_ROUTES;
    use api::oauth::scopes::ScopeSet;
    let definitions = mcp_server::get_all_tool_definitions();
    assert!(!mcp_server::oauth_scopes::SCOPED_TOOLS.is_empty());
    for (tool, scope) in mcp_server::oauth_scopes::SCOPED_TOOLS {
        let definition = definitions
            .iter()
            .find(|definition| definition.name == *tool)
            .ok_or_else(|| format!("{tool} is not a registered tool"))?;
        let token = delegated('a');
        let source = FakeSource::new([(token.clone(), active(scope.as_str()))]);
        let server = Harness::delegated(&source).await?;
        let reply = server
            .post(Some(&token), &call(tool, &required_arguments(&definition.input_schema)))
            .await?;
        assert_eq!(reply.status, StatusCode::OK, "{tool}: {}", reply.text);
        let seen = server.seen().await;
        assert!(
            !seen.is_empty(),
            "{tool}: the call never reached the API: {}",
            reply.text
        );
        let granted = ScopeSet::from_scopes(&[*scope]);
        for request in &seen {
            let entry = DELEGATED_ROUTES
                .iter()
                .find(|entry| entry.method == request.method && path_matches(entry.pattern, &request.path))
                .ok_or_else(|| {
                    format!(
                        "{tool} calls {} {}, which is not on the API's delegated allow-list",
                        request.method, request.path
                    )
                })?;
            assert!(
                entry.required.satisfied_by(granted),
                "{tool} holds {} but {} {} requires {:?}",
                scope.as_str(),
                entry.method,
                entry.pattern,
                entry.required
            );
        }
    }
    Ok(())
}

/// Read tools whose API routes are denied to connected applications are not offered and not
/// callable, so none can be listed only to fail closed.
#[tokio::test]
async fn read_tools_with_denied_routes_are_not_available_to_delegated_callers() -> TestResult {
    let token = delegated('a');
    let source = FakeSource::new([(
        token.clone(),
        active("projects:read work_items:read work_items:write forms:read forms:write"),
    )]);
    let server = Harness::delegated(&source).await?;
    let visible = listed(&server.post(Some(&token), &list()).await?)?;
    for tool in WITHHELD_READS {
        assert!(!visible.contains(tool), "{tool} is listed");
        let reply = server
            .post(Some(&token), &call(tool, &json!({"project_id": PROJECT})))
            .await?;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{tool}");
    }
    assert!(server.seen().await.is_empty(), "a refused call reached the API");
    Ok(())
}

/// What `tools/list` shows and what `tools/call` allows can not drift apart: for every registered
/// tool, listed means not refused at the gate and not listed means refused.
#[tokio::test]
async fn a_tool_that_is_not_listed_cannot_be_called_by_guessing_its_name() -> TestResult {
    for scope in ["projects:read", "projects:read work_items:read forms:read"] {
        let token = delegated('a');
        let source = FakeSource::new([(token.clone(), active(scope))]);
        let server = Harness::delegated(&source).await?;
        let visible = listed(&server.post(Some(&token), &list()).await?)?;
        for tool in mcp_server::get_all_tool_definitions() {
            let reply = server.post(Some(&token), &call(&tool.name, &json!({}))).await?;
            let refused = reply.status == StatusCode::FORBIDDEN;
            assert_eq!(
                refused,
                !visible.contains(&tool.name),
                "{scope}: {} listed={} status={}",
                tool.name,
                visible.contains(&tool.name),
                reply.status
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_workspace_argument_that_differs_from_the_grant_is_refused_not_replaced() -> TestResult {
    let token = delegated('a');
    let source = FakeSource::new([(token.clone(), active("projects:read"))]);
    let server = Harness::delegated(&source).await?;
    for value in [
        json!(OTHER_WORKSPACE),
        json!(CONFIGURED_WORKSPACE),
        json!("not-a-uuid"),
        json!(5),
    ] {
        let reply = server
            .post(Some(&token), &call("projects.list", &json!({"workspace_id": value})))
            .await?;
        assert_eq!(reply.status, StatusCode::FORBIDDEN, "{value}");
        assert!(reply.headers.get("www-authenticate").is_none());
        let body = reply.json()?;
        assert_eq!(body.pointer("/error/code").and_then(Value::as_i64), Some(-32002));
        assert!(
            !reply.text.contains(GRANT_WORKSPACE),
            "the refusal discloses the grant's workspace"
        );
        assert!(!reply.text.contains(OTHER_WORKSPACE));
    }
    assert!(server.seen().await.is_empty(), "a refused call reached the API");
    // The grant's own workspace, in any spelling of the same UUID, is not a mismatch.
    let reply = server
        .post(
            Some(&token),
            &call(
                "projects.list",
                &json!({"workspace_id": GRANT_WORKSPACE.to_uppercase()}),
            ),
        )
        .await?;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(server.seen().await.len(), 1);
    Ok(())
}

// ---- Resources, initialize -------------------------------------------------------------------

#[tokio::test]
async fn delegated_callers_read_the_static_guides_and_nothing_else() -> TestResult {
    let token = delegated('a');
    let source = FakeSource::new([(token.clone(), active("projects:read forms:read"))]);
    let server = Harness::delegated(&source).await?;
    let rpc = |method: &str, params: Value| json!({"jsonrpc": "2.0", "id": 3, "method": method, "params": params});

    let guide = server
        .post(
            Some(&token),
            &rpc("resources/read", json!({"uri": "sylvode://guides/workflows"})),
        )
        .await?;
    assert!(guide.json()?.pointer("/result/contents/0/text").is_some());

    for uri in [
        format!("sylvode://forms/{PROJECT}/records"),
        format!("sylvode://projects/{PROJECT}/issues"),
        "sylvode://scenario-templates".to_string(),
        format!("sylvode://workspaces/{GRANT_WORKSPACE}/navigator"),
    ] {
        let reply = server
            .post(Some(&token), &rpc("resources/read", json!({"uri": uri})))
            .await?;
        let body = reply.json()?;
        assert!(body.get("result").is_none(), "{uri}: {body}");
        assert_eq!(
            body.pointer("/error/code").and_then(Value::as_i64),
            Some(-32602),
            "{uri}"
        );
    }
    let listing = server
        .post(Some(&token), &rpc("resources/list", json!({})))
        .await?
        .json()?;
    let uris: Vec<&str> = listing
        .pointer("/result/resources")
        .and_then(Value::as_array)
        .ok_or("no resources")?
        .iter()
        .filter_map(|resource| resource.get("uri").and_then(Value::as_str))
        .collect();
    assert_eq!(
        uris,
        [
            "sylvode://skills/sylvode-mcp",
            "sylvode://skills/sylvode-mcp/tools",
            "sylvode://guides/workflows"
        ]
    );
    let templates = server
        .post(Some(&token), &rpc("resources/templates/list", json!({})))
        .await?
        .json()?;
    assert_eq!(templates.pointer("/result/resourceTemplates"), Some(&json!([])));
    assert!(
        server.seen().await.is_empty(),
        "a delegated resource read reached the API"
    );

    // A legacy caller still gets the data resources and the templates.
    let legacy = server
        .post(Some(BOT_TOKEN), &rpc("resources/templates/list", json!({})))
        .await?
        .json()?;
    assert!(
        legacy
            .pointer("/result/resourceTemplates")
            .and_then(Value::as_array)
            .is_some_and(|templates| !templates.is_empty())
    );
    Ok(())
}

#[tokio::test]
async fn initialize_keeps_the_protocol_version_it_always_declared() -> TestResult {
    let token = delegated('a');
    let source = FakeSource::new([(token.clone(), active("projects:read"))]);
    let server = Harness::delegated(&source).await?;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}});
    for caller in [token.as_str(), BOT_TOKEN] {
        let reply = server.post(Some(caller), &body).await?;
        assert_eq!(reply.status, StatusCode::OK);
        assert_eq!(
            reply.json()?.pointer("/result/protocolVersion").and_then(Value::as_str),
            Some("2024-11-05")
        );
    }
    Ok(())
}

// ---- Fail closed -----------------------------------------------------------------------------

#[tokio::test]
async fn a_token_context_failure_refuses_the_request() -> TestResult {
    let token = delegated('a');
    for reason in [
        UnavailableReason::Unreachable,
        UnavailableReason::Timeout,
        UnavailableReason::UnexpectedStatus(404),
        UnavailableReason::UnexpectedStatus(500),
        UnavailableReason::Malformed,
    ] {
        let source = FakeSource::new([(token.clone(), ContextOutcome::Unavailable(reason))]);
        let server = Harness::delegated(&source).await?;
        let reply = server.post(Some(&token), &call("projects.list", &json!({}))).await?;
        assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE, "{reason:?}");
        assert!(reply.headers.get("www-authenticate").is_none(), "{reason:?}");
        assert!(server.seen().await.is_empty(), "{reason:?}: served without a verdict");
        assert!(!reply.text.contains(&token));
    }
    Ok(())
}

/// The shipped lookup, against stand-ins for every way the API can fail to give a verdict.
#[tokio::test]
async fn the_http_token_context_source_fails_closed_on_every_bad_answer() -> TestResult {
    let token = Secret::new(delegated('a'));
    let hits = Arc::new(AtomicUsize::new(0));
    let redirect_target_hits = Arc::clone(&hits);
    let target = spawn(Router::new().fallback(move || {
        redirect_target_hits.fetch_add(1, Ordering::SeqCst);
        async { Json(json!({"active": true})) }
    }))
    .await?;
    let valid = json!({
        "active": true, "client_id": "ramflux",
        "sub": "33333333-3333-4333-8333-333333333333",
        "grant_id": "44444444-4444-4444-8444-444444444444",
        "workspace_id": GRANT_WORKSPACE, "scope": "projects:read", "project_mode": "all", "exp": 4_102_444_800_i64
    });
    let redirect_to = format!("{target}/stolen");
    let oversized = "x".repeat(32 * 1024);

    let cases: Vec<(&str, Answer)> = vec![
        ("200 valid", (StatusCode::OK, None, valid.to_string())),
        (
            "200 inactive",
            (StatusCode::OK, None, json!({"active": false}).to_string()),
        ),
        ("200 garbage", (StatusCode::OK, None, "not json".to_string())),
        ("200 empty", (StatusCode::OK, None, String::new())),
        ("200 oversized", (StatusCode::OK, None, oversized)),
        ("401", (StatusCode::UNAUTHORIZED, None, String::new())),
        ("404", (StatusCode::NOT_FOUND, None, String::new())),
        ("429", (StatusCode::TOO_MANY_REQUESTS, None, String::new())),
        ("500", (StatusCode::INTERNAL_SERVER_ERROR, None, String::new())),
        ("302", (StatusCode::FOUND, Some(redirect_to), String::new())),
    ];
    for (label, (status, location, body)) in cases {
        let api = spawn(Router::new().fallback(move || {
            let location = location.clone();
            let body = body.clone();
            async move {
                let mut response = axum::response::Response::new(axum::body::Body::from(body));
                *response.status_mut() = status;
                if let Some(location) = location
                    && let Ok(value) = axum::http::HeaderValue::from_str(&location)
                {
                    response.headers_mut().insert("location", value);
                }
                response
            }
        }))
        .await?;
        let source = HttpTokenContextSource::new(&api, Secret::new(FORWARD_SECRET))?;
        let outcome = source.resolve(&token).await;
        let expected = match label {
            "200 valid" => "Active",
            "200 inactive" | "401" => "Invalid",
            _ => "Unavailable",
        };
        let actual = match &outcome {
            ContextOutcome::Active(_) => "Active",
            ContextOutcome::Invalid => "Invalid",
            ContextOutcome::Unavailable(_) => "Unavailable",
        };
        assert_eq!(actual, expected, "{label}: {outcome:?}");
    }
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "a redirect was followed with the forwarding credential"
    );

    // Nothing is listening: the API is down.
    let down = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        format!("http://{}", listener.local_addr()?)
    };
    let source = HttpTokenContextSource::new(&down, Secret::new(FORWARD_SECRET))?;
    assert!(matches!(
        source.resolve(&token).await,
        ContextOutcome::Unavailable(UnavailableReason::Unreachable)
    ));
    Ok(())
}

#[tokio::test]
async fn the_http_token_context_source_asks_with_the_token_and_the_forwarding_credential() -> TestResult {
    let seen: Arc<Mutex<Vec<ContextRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let api = spawn(Router::new().fallback(move |request: Request| {
        let log = Arc::clone(&log);
        async move {
            log.lock().await.push((
                request.method().to_string(),
                request.uri().to_string(),
                header(request.headers(), "authorization"),
                header(request.headers(), "x-sylvode-forwarded-by"),
                header(request.headers(), "x-sylvode-forward-auth"),
            ));
            Json(json!({
                "active": true, "client_id": "ramflux",
                "sub": "33333333-3333-4333-8333-333333333333",
                "grant_id": "44444444-4444-4444-8444-444444444444",
                "workspace_id": GRANT_WORKSPACE, "scope": "projects:read work_items:read",
                "project_mode": "selected", "project_ids": [PROJECT], "exp": 4_102_444_800_i64
            }))
        }
    }))
    .await?;
    let token = delegated('c');
    let source = HttpTokenContextSource::new(&format!("{api}/"), Secret::new(FORWARD_SECRET))?;
    let outcome = source.resolve(&Secret::new(token.clone())).await;
    let ContextOutcome::Active(context) = outcome else {
        return Err(format!("not active: {outcome:?}").into());
    };
    assert_eq!(context.workspace_id.to_string(), GRANT_WORKSPACE);
    assert_eq!(context.scopes.to_scope_string(), "projects:read work_items:read");

    let seen = seen.lock().await.clone();
    let request = seen.first().ok_or("the API was not asked")?;
    assert_eq!(request.0, "GET");
    assert_eq!(
        request.1, "/api/v1/oauth/internal/token-context",
        "no query string, no token in the URL"
    );
    assert_eq!(request.2.as_deref(), Some(format!("Bearer {token}").as_str()));
    assert_eq!(request.3.as_deref(), Some("mcp-server"));
    assert_eq!(request.4.as_deref(), Some(FORWARD_SECRET));
    Ok(())
}

// ---- Existing token kinds are unchanged ------------------------------------------------------

#[tokio::test]
async fn opr_and_user_tokens_are_forwarded_as_before_with_no_forwarding_credential() -> TestResult {
    let source = FakeSource::new([]);
    let server = Harness::delegated(&source).await?;
    // A bot token acts in the workspace its own bot belongs to (asked of `auth/me`, never the
    // configured one); a person's token has no bot, so no workspace, and nothing else is sent.
    for token in [BOT_TOKEN, JWT_SHAPED_TOKEN] {
        let before = server.seen().await.len();
        let reply = server
            .post_full(Some(token), &call("projects.list", &json!({})))
            .await?;
        assert_eq!(reply.status, StatusCode::OK, "{token}");
        let seen = server.seen().await;
        let lookup = seen.get(before).ok_or("the call did not reach the API")?;
        assert_eq!(lookup.path, "/api/v1/auth/me", "{token}");
        let requests: Vec<&Seen> = if token == BOT_TOKEN {
            let request = seen.get(before + 1).ok_or("the projects call did not reach the API")?;
            assert_eq!(
                request.path,
                format!("/api/v1/workspaces/{BOT_WORKSPACE}/projects"),
                "a legacy caller acts in its bot's workspace, not the configured one"
            );
            vec![lookup, request]
        } else {
            assert_eq!(
                seen.len(),
                before + 1,
                "a token without a bot reaches no workspace route"
            );
            assert_eq!(
                reply.json()?.pointer("/result/isError"),
                Some(&json!(true)),
                "{}",
                reply.text
            );
            assert!(reply.text.contains("401"), "{}", reply.text);
            vec![lookup]
        };
        for request in requests {
            assert_eq!(
                request.authorization.as_deref(),
                Some(format!("Bearer {token}").as_str())
            );
            assert_eq!(
                request.forwarded_by, None,
                "a legacy caller never sends the forwarding headers"
            );
            assert_eq!(request.forward_auth, None);
        }
    }

    // Tools a delegated caller is denied are untouched for a legacy caller, and so is a
    // `workspace_id` argument: the API remains the authority for it.
    for (tool, arguments) in [
        ("members.list", json!({})),
        ("projects.list", json!({"workspace_id": OTHER_WORKSPACE})),
        ("work_items.list", json!({"project_id": PROJECT})),
    ] {
        let before = server.seen().await.len();
        let reply = server.post_full(Some(BOT_TOKEN), &call(tool, &arguments)).await?;
        assert_eq!(reply.status, StatusCode::OK, "{tool}");
        assert!(reply.json()?.get("error").is_none(), "{tool}: {}", reply.text);
        let seen = server.seen().await;
        assert!(seen.len() > before, "{tool} did not reach the API");
        assert!(
            seen.iter().skip(before).all(|request| request.forward_auth.is_none()),
            "{tool}"
        );
    }
    assert_eq!(
        source.calls(),
        0,
        "a legacy token is never looked up as a delegated one"
    );
    Ok(())
}

// ---- Nothing secret reaches a log ------------------------------------------------------------

#[derive(Clone, Default)]
struct LogSink(Arc<parking_lot::Mutex<Vec<u8>>>);

impl std::io::Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The one sink every test of this binary logs into, at the most verbose level.
///
/// Installed as the process wide subscriber, not per test: a per-thread subscriber loses events to
/// tracing's cached callsite interest when tests run in parallel. It also means the check below
/// covers the traffic of every test in the binary, not only its own.
fn log_sink() -> &'static LogSink {
    static SINK: std::sync::OnceLock<LogSink> = std::sync::OnceLock::new();
    SINK.get_or_init(|| {
        let sink = LogSink::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_ansi(false)
            .with_writer(sink.clone())
            .finish();
        // Another initializer in this process would be a test bug; the capture would then be empty
        // and the assertion on its content below would say so.
        let _ = tracing::subscriber::set_global_default(subscriber);
        sink
    })
}

/// Every kind of request the server can see, at the most verbose log level: no token and no
/// forwarding credential appears in a log line or in any response body.
#[tokio::test]
async fn no_token_or_credential_reaches_the_logs_or_a_response() -> TestResult {
    let sink = log_sink();

    let good = delegated('a');
    let narrow = delegated('b');
    let revoked = delegated('c');
    let unavailable = delegated('d');
    let expired = delegated('e');
    let source = FakeSource::new([
        (good.clone(), active("projects:read work_items:read forms:read")),
        (narrow.clone(), active("projects:read")),
        (revoked.clone(), ContextOutcome::Invalid),
        (
            unavailable.clone(),
            ContextOutcome::Unavailable(UnavailableReason::Unreachable),
        ),
        (expired.clone(), active_until("projects:read", 1)),
    ]);
    let server = Harness::delegated(&source).await?;
    // Every token any test of this binary uses, since the sink sees all of their traffic.
    let mut secrets: Vec<String> = ('a'..='f').map(delegated).collect();
    secrets.extend([
        BOT_TOKEN.to_string(),
        JWT_SHAPED_TOKEN.to_string(),
        FORWARD_SECRET.to_string(),
    ]);

    tracing::info!(control = "control-marker-7f3a", "capture control");
    let mut bodies = Vec::new();
    let mut requests: Vec<(Option<&str>, Value)> = vec![
        (None, list()),
        (Some(&good), list()),
        (Some(&good), call("projects.list", &json!({}))),
        (Some(&good), call("work_items.list", &json!({"project_id": PROJECT}))),
        (
            Some(&good),
            call("work_items.create", &json!({"project_id": PROJECT, "title": "t"})),
        ),
        (
            Some(&good),
            call("projects.list", &json!({"workspace_id": OTHER_WORKSPACE})),
        ),
        (Some(&narrow), call("work_items.list", &json!({"project_id": PROJECT}))),
        (Some(&revoked), list()),
        (Some(&unavailable), list()),
        (Some(&expired), list()),
        (Some("sva_not_a_real_token"), list()),
        (Some(BOT_TOKEN), call("projects.list", &json!({}))),
        (Some(JWT_SHAPED_TOKEN), call("projects.list", &json!({}))),
    ];
    let forbidden_delegate = delegated('f');
    requests.push((Some(&forbidden_delegate), list()));
    for (token, body) in &requests {
        let reply = server.post(*token, body).await?;
        bodies.push(format!("{:?} {}", reply.headers, reply.text));
    }
    let sse = server.get("/sse", Some(&good)).await?;
    bodies.push(format!("{:?} {}", sse.headers, sse.text));
    let by_query = server
        .send(
            server
                .http
                .post(format!("{}/mcp/rpc?access_token={good}", server.base))
                .json(&list()),
        )
        .await?;
    bodies.push(format!("{:?} {}", by_query.headers, by_query.text));

    let captured = String::from_utf8_lossy(&sink.0.lock()).into_owned();
    assert!(
        captured.contains("Handling MCP request"),
        "the capture saw no MCP log lines at all: {captured:.600}"
    );
    assert!(captured.len() > 500, "the capture is suspiciously small: {captured}");
    // Control: a marker logged on purpose is found, so the absence of the secrets below means
    // something.
    assert!(
        captured.contains("control-marker-7f3a"),
        "the capture does not see what is logged"
    );
    for secret in &secrets {
        assert!(
            !captured.contains(secret.as_str()),
            "a secret reached the log output: {secret}"
        );
        for body in &bodies {
            assert!(!body.contains(secret.as_str()), "a secret reached a response: {secret}");
        }
    }
    assert!(
        !captured.contains("Bearer "),
        "an authorization header value reached the log output"
    );
    Ok(())
}
