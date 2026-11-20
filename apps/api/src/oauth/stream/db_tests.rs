//! The event stream through the real router, over a real socket, against PostgreSQL.
//!
//! Feed rows are inserted directly, as the worker's writer would write them (the writer itself is
//! tested in the worker crate, together with the triggers and this endpoint). Every test starts
//! its own scratch database and its own server, reads the stream incrementally with an HTTP
//! client, and uses generous bounds for "arrives within" except where the contract fixes one (5 s
//! from revocation to stop).
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
    clippy::struct_field_names,
    clippy::large_futures
)]

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use platform::app::{AppState, FlowPermissionCacheSlot};
use platform::config::{AppConfig, OAuthConfig, Secret};
use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::cursor::{CursorKey, Position};
use super::{Hub, StreamLimits, StreamTuning, routes_with};
use crate::oauth::grants::{
    Actor, GRANT_CHANGED_CHANNEL, RevokeReason, revoke_all_user_grants, revoke_client_grants, revoke_grant,
    revoke_user_workspace_grants, revoke_workspace_grants,
};
use crate::oauth::test_support::{Scratch, Start, count, oauth_scratch_or_skip, seed_access_token, seed_grant};
use crate::routes::context::tenant_fixture::{Tenant, exec, seed_tenant};

const ISSUER: &str = "http://127.0.0.1:18097";
const JWT_SECRET: &str = "wp4-stream-test-jwt-secret-0123456789abcdef";
const BOTH: [&str; 2] = ["notifications:read", "tasks:read"];
/// The contract's bound from a revocation commit to the stream stopping.
const REVOCATION_BOUND: Duration = Duration::from_secs(5);
/// How long a test waits for something that must arrive.
const PATIENCE: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------------------------
// SSE client
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Msg {
    id: Option<String>,
    event: Option<String>,
    data: Option<Value>,
    comment: Option<String>,
}

impl Msg {
    fn is_event(&self) -> bool {
        self.event.as_deref() == Some("sv.event")
    }

    fn control_type(&self) -> Option<&str> {
        if self.event.as_deref() == Some("sv.control") {
            self.data.as_ref().and_then(|data| data["type"].as_str())
        } else {
            None
        }
    }

    fn data(&self) -> &Value {
        self.data.as_ref().expect("the message carries data")
    }
}

enum Next {
    Msg(Msg),
    Eof,
    Timeout,
}

struct Sse {
    response: reqwest::Response,
    buf: Vec<u8>,
}

impl Sse {
    fn parse_one(&mut self) -> Option<Msg> {
        let end = self.buf.windows(2).position(|pair| pair == b"\n\n")?;
        let raw: Vec<u8> = self.buf.drain(..end + 2).collect();
        let text = String::from_utf8(raw).expect("utf-8 frame");
        let mut msg = Msg {
            id: None,
            event: None,
            data: None,
            comment: None,
        };
        for line in text.lines() {
            if let Some(comment) = line.strip_prefix(':') {
                msg.comment = Some(comment.trim().to_owned());
            } else if let Some(id) = line.strip_prefix("id: ") {
                msg.id = Some(id.to_owned());
            } else if let Some(event) = line.strip_prefix("event: ") {
                msg.event = Some(event.to_owned());
            } else if let Some(data) = line.strip_prefix("data: ") {
                assert!(msg.data.is_none(), "one data line per message: {text}");
                msg.data = Some(serde_json::from_str(data).unwrap_or_else(|e| panic!("data is JSON ({e}): {data}")));
            } else if !line.is_empty() {
                panic!("unexpected line in an event stream frame: {line:?}");
            }
        }
        Some(msg)
    }

    async fn next(&mut self, within: Duration) -> Next {
        let deadline = Instant::now() + within;
        loop {
            if let Some(msg) = self.parse_one() {
                return Next::Msg(msg);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Next::Timeout;
            }
            match tokio::time::timeout(left, self.response.chunk()).await {
                Err(_) => return Next::Timeout,
                Ok(Ok(Some(bytes))) => self.buf.extend_from_slice(&bytes),
                Ok(Ok(None) | Err(_)) => return Next::Eof,
            }
        }
    }

    /// The next message that is not a heartbeat.
    async fn message(&mut self, within: Duration) -> Msg {
        let deadline = Instant::now() + within;
        loop {
            match self.next(deadline.saturating_duration_since(Instant::now())).await {
                Next::Msg(msg) if msg.comment.is_some() && msg.event.is_none() => {}
                Next::Msg(msg) => return msg,
                Next::Eof => panic!("the stream ended while a message was expected"),
                Next::Timeout => panic!("no message within {within:?}"),
            }
        }
    }

    async fn event(&mut self, within: Duration) -> Msg {
        let msg = self.message(within).await;
        assert!(msg.is_event(), "expected an sv.event, got {msg:?}");
        msg
    }

    async fn control(&mut self, kind: &str, within: Duration) -> Msg {
        let msg = self.message(within).await;
        assert_eq!(msg.control_type(), Some(kind), "expected control {kind}, got {msg:?}");
        msg
    }

    /// Whether anything but heartbeats arrives within `within`.
    async fn silent_for(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            match self.next(deadline.saturating_duration_since(Instant::now())).await {
                Next::Msg(msg) if msg.comment.is_some() && msg.event.is_none() => {}
                Next::Msg(msg) => {
                    eprintln!("silent_for saw {msg:?}");
                    return false;
                }
                Next::Eof => {
                    eprintln!("silent_for saw the end of the stream");
                    return false;
                }
                Next::Timeout => return true,
            }
        }
    }

    /// Reads to the end of the stream; the messages that came before it.
    async fn read_to_end(&mut self, within: Duration) -> Vec<Msg> {
        let deadline = Instant::now() + within;
        let mut out = Vec::new();
        loop {
            match self.next(deadline.saturating_duration_since(Instant::now())).await {
                Next::Msg(msg) => out.push(msg),
                Next::Eof => return out,
                Next::Timeout => panic!("the stream did not end within {within:?}; got {out:?}"),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// World
// ---------------------------------------------------------------------------------------------

struct Grant {
    id: Uuid,
    token_id: Uuid,
    token: String,
}

struct World {
    scratch: Scratch,
    base: String,
    hub: Arc<Hub>,
    http: reqwest::Client,
    key: CursorKey,
    tenant: Tenant,
    /// A second project of the first workspace.
    p2: Uuid,
    /// A second workspace the same user belongs to, and its project.
    w2: Uuid,
    p3: Uuid,
    /// Another member of the first workspace.
    other: Uuid,
    clear_rate: bool,
    server: tokio::task::JoinHandle<()>,
}

fn small_tuning() -> StreamTuning {
    StreamTuning::default()
}

impl World {
    async fn new(label: &str) -> Option<Self> {
        Self::with(
            label,
            |_| {},
            StreamLimits::from_config(&OAuthConfig::with_defaults(ISSUER)),
            small_tuning(),
            None,
        )
        .await
    }

    async fn with(
        label: &str,
        tweak: impl FnOnce(&mut OAuthConfig),
        limits: StreamLimits,
        tuning: StreamTuning,
        pool_size: Option<u32>,
    ) -> Option<Self> {
        let Some(scratch) = crate::oauth::test_support::scratch(&format!("stream_{label}"), Start::Complete).await
        else {
            eprintln!(
                "SKIPPED (no database): set {} to run this test",
                crate::oauth::test_support::TEST_DATABASE_URL_ENV
            );
            return None;
        };
        let seed = &scratch.db;
        let tenant = seed_tenant(seed, "wp4").await;
        let p2 = Uuid::new_v4();
        exec(
            seed,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'P2', 'second project', $3)",
            vec![p2.into(), tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        let (w2, p3) = (Uuid::new_v4(), Uuid::new_v4());
        exec(
            seed,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'second workspace', $3)",
            vec![w2.into(), format!("ws-{w2}").into(), tenant.member_id.into()],
        )
        .await;
        exec(
            seed,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
            vec![w2.into(), tenant.member_id.into()],
        )
        .await;
        exec(
            seed,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'P3', 'third project', $3)",
            vec![p3.into(), w2.into(), tenant.member_id.into()],
        )
        .await;
        let other = Uuid::new_v4();
        exec(
            seed,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'other member', 'user', true)",
            vec![other.into(), format!("{other}@tenant.test").into()],
        )
        .await;
        exec(
            seed,
            "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
            vec![tenant.workspace_id.into(), other.into()],
        )
        .await;

        // The server's own pool, optionally tiny, to show that idle streams do not hold connections.
        let db: DatabaseConnection = match pool_size {
            Some(max) => {
                let admin = std::env::var(crate::oauth::test_support::TEST_DATABASE_URL_ENV).expect("database url");
                let (prefix, _) = admin.rsplit_once('/').expect("url");
                let mut options = ConnectOptions::new(format!("{prefix}/sylvode_oauth_stream_{label}"));
                options
                    .max_connections(max)
                    .acquire_timeout(Duration::from_secs(5))
                    .sqlx_logging(false);
                Database::connect(options).await.expect("pool")
            }
            None => scratch.db.clone(),
        };
        let state = AppState {
            cfg: AppConfig {
                app_name: "api-test".to_owned(),
                bind_addr: "127.0.0.1:0".to_owned(),
                database_url: Secret::new("postgres://unused"),
                jwt_secret: Secret::new(JWT_SECRET),
                jwt_access_ttl_seconds: 900,
                jwt_refresh_ttl_seconds: 3600,
                default_author_id: None,
                allow_insecure_cookies: false,
                collab_allowed_origins: Vec::new(),
            },
            db,
            flow_permission_cache: FlowPermissionCacheSlot::default(),
        };
        let mut config = OAuthConfig::with_defaults(ISSUER);
        tweak(&mut config);
        let (router, hub) = routes_with(Arc::new(config), limits, tuning);
        let app = router.with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await;
        });
        Some(Self {
            scratch,
            base: format!("http://{addr}"),
            hub,
            http: reqwest::Client::builder().no_proxy().build().expect("client"),
            key: CursorKey::derive(JWT_SECRET).expect("key"),
            tenant,
            p2,
            w2,
            p3,
            other,
            clear_rate: true,
            server,
        })
    }

    fn db(&self) -> &DatabaseConnection {
        &self.scratch.db
    }

    fn user(&self) -> Uuid {
        self.tenant.member_id
    }

    fn ws(&self) -> Uuid {
        self.tenant.workspace_id
    }

    fn p1(&self) -> Uuid {
        self.tenant.project_id
    }

    async fn finish(self) {
        self.server.abort();
        self.scratch.drop_self().await;
    }

    /// An active grant of the main user in the first workspace with a live API token.
    async fn grant(&self, mode: &str, projects: &[Uuid], scopes: &[&str]) -> Grant {
        self.grant_for(self.user(), self.ws(), mode, projects, scopes).await
    }

    async fn grant_for(&self, user: Uuid, workspace: Uuid, mode: &str, projects: &[Uuid], scopes: &[&str]) -> Grant {
        let id = seed_grant(self.db(), user, workspace, mode, scopes).await;
        for project in projects {
            exec(
                self.db(),
                "INSERT INTO oauth_grant_projects (grant_id, project_id) VALUES ($1, $2)",
                vec![id.into(), (*project).into()],
            )
            .await;
        }
        let (token_id, token) = seed_access_token(self.db(), id, "api", scopes).await;
        Grant { id, token_id, token }
    }

    async fn another_token(&self, grant: &Grant, resource: &str) -> String {
        seed_access_token(self.db(), grant.id, resource, &BOTH).await.1
    }

    async fn get(&self, token: Option<&str>, headers: &[(&str, &str)], query: &str) -> reqwest::Response {
        if self.clear_rate {
            exec(
                self.db(),
                "DELETE FROM oauth_rate_hits WHERE bucket = 'stream_connect'",
                vec![],
            )
            .await;
        }
        let url = format!("{}{}{}", self.base, super::STREAM_PATH, query);
        let mut request = self.http.get(url).header("accept", "text/event-stream");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        request.send().await.expect("the server answers")
    }

    async fn open(&self, token: &str) -> Sse {
        self.open_with(token, &[], "").await
    }

    async fn open_with(&self, token: &str, headers: &[(&str, &str)], query: &str) -> Sse {
        let response = self.get(Some(token), headers, query).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "{}",
            response.text().await.unwrap_or_default()
        );
        Sse {
            response,
            buf: Vec::new(),
        }
    }

    /// Opens a stream and consumes its `hello`; returns it with the hello's cursor.
    async fn open_ready(&self, token: &str) -> (Sse, String) {
        let mut sse = self.open(token).await;
        let hello = sse.control("hello", PATIENCE).await;
        let cursor = hello.data()["cursor"].as_str().expect("cursor").to_owned();
        (sse, cursor)
    }

    /// Writes one feed row the way the worker's writer does and wakes the streams.
    async fn feed(
        &self,
        event_type: &str,
        workspace: Uuid,
        project: Option<Uuid>,
        user: Option<Uuid>,
        subject: Value,
    ) -> i64 {
        #[derive(FromQueryResult)]
        struct Seq {
            seq: i64,
        }
        let row = Seq::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO oauth_event_feed (event_type, workspace_id, project_id, user_id, subject, occurred_at) \
             VALUES ($1, $2, $3, $4, $5, now()) RETURNING seq",
            vec![
                event_type.into(),
                workspace.into(),
                project.into(),
                user.into(),
                subject.into(),
            ],
        ))
        .one(self.db())
        .await
        .expect("insert")
        .expect("seq");
        exec(
            self.db(),
            "SELECT pg_notify('sv_oauth_feed', $1)",
            vec![row.seq.to_string().into()],
        )
        .await;
        row.seq
    }

    /// A notification for the main user in `project`.
    async fn note(&self, project: Option<Uuid>, content: &str) -> i64 {
        self.feed(
            "notification.created",
            self.ws(),
            project,
            Some(self.user()),
            note_subject(content, project),
        )
        .await
    }

    async fn task(&self, project: Uuid, status: &str) -> i64 {
        self.feed(
            "task.status_changed",
            self.ws(),
            Some(project),
            None,
            task_subject(status, "pending"),
        )
        .await
    }

    /// The opaque id the stream gives to feed row `seq`.
    async fn id_of(&self, grant: &Grant, seq: i64) -> String {
        #[derive(FromQueryResult)]
        struct At {
            at: i64,
        }
        let at = At::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT FLOOR(EXTRACT(EPOCH FROM created_at))::bigint AS at FROM oauth_event_feed WHERE seq = $1",
            vec![seq.into()],
        ))
        .one(self.db())
        .await
        .expect("query")
        .expect("row")
        .at;
        self.key.encode(grant.id, Position { seq, at }).expect("sealed")
    }

    async fn grant_state(&self, grant: &Grant) -> (String, Option<String>) {
        #[derive(FromQueryResult)]
        struct Row {
            status: String,
            revoke_reason: Option<String>,
        }
        let row = Row::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT status, revoke_reason FROM oauth_grants WHERE id = $1",
            vec![grant.id.into()],
        ))
        .one(self.db())
        .await
        .expect("query")
        .expect("grant");
        (row.status, row.revoke_reason)
    }
}

fn note_subject(content: &str, project: Option<Uuid>) -> Value {
    json!({
        "kind": "notification",
        "notification_id": Uuid::new_v4(),
        "notification_type": "assignment",
        "title_key": "notification.issueAssignedTitle",
        "content": content,
        "related": {"issue_id": null, "comment_id": null, "project_id": project},
        "is_read": false,
    })
}

fn task_subject(status: &str, previous: &str) -> Value {
    json!({
        "kind": "ai_task",
        "task_id": Uuid::new_v4(),
        "task_type": "issue_assigned",
        "status": status,
        "previous_status": previous,
        "attempts": 1,
        "max_attempts": 3,
        "agent": {"id": Uuid::new_v4(), "name": "Triage agent"},
        "reference": {"type": "work_item", "id": Uuid::new_v4(), "identifier": "SEC-42", "title": "Investigate"},
    })
}

fn content_of(msg: &Msg) -> String {
    msg.data()["subject"]["content"].as_str().unwrap_or_default().to_owned()
}

fn contents(messages: &[Msg]) -> Vec<String> {
    messages.iter().filter(|msg| msg.is_event()).map(content_of).collect()
}

// ---------------------------------------------------------------------------------------------
// Opening a stream
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_endpoint_refuses_what_it_should() {
    let Some(world) = World::new("refuse").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;

    // No credentials, and a delegated token in a cookie, are the same 401 with a challenge.
    for headers in [vec![], vec![("cookie", "access_token=sva_x")]] {
        let response = world.get(None, &headers, "").await;
        assert_eq!(response.status().as_u16(), 401);
        let challenge = response.headers()["www-authenticate"].to_str().unwrap().to_owned();
        assert!(challenge.starts_with("Bearer realm=\"sylvode\""), "{challenge}");
        assert!(
            challenge.contains("resource_metadata=\"http://127.0.0.1:18097/.well-known/oauth-protected-resource/api\""),
            "{challenge}"
        );
    }
    let cookie_only = world
        .get(None, &[("cookie", &format!("access_token={}", grant.token))], "")
        .await;
    assert_eq!(
        cookie_only.status().as_u16(),
        401,
        "a delegated token is never a cookie"
    );

    // A token in the URL is refused even when the header is right.
    let response = world
        .get(Some(&grant.token), &[], &format!("?access_token={}", grant.token))
        .await;
    assert_eq!(response.status().as_u16(), 400);
    assert_eq!(response.json::<Value>().await.unwrap()["error"], "invalid_request");

    // An MCP-audience token never opens the API stream, even with the right header values.
    let mcp = world.another_token(&grant, "mcp").await;
    assert_eq!(world.get(Some(&mcp), &[], "").await.status().as_u16(), 401);
    // A token that is not a token, a revoked one and an expired one.
    assert_eq!(world.get(Some("sva_nothing"), &[], "").await.status().as_u16(), 401);
    exec(
        world.db(),
        "UPDATE oauth_tokens SET revoked_at = now() WHERE id = $1",
        vec![grant.token_id.into()],
    )
    .await;
    assert_eq!(world.get(Some(&grant.token), &[], "").await.status().as_u16(), 401);
    let expired = world.grant("all", &[], &BOTH).await;
    exec(
        world.db(),
        "UPDATE oauth_tokens SET expires_at = now() - interval '1 second' WHERE id = $1",
        vec![expired.token_id.into()],
    )
    .await;
    assert_eq!(world.get(Some(&expired.token), &[], "").await.status().as_u16(), 401);

    // A grant without the stream's scopes, and a request for types the scopes do not allow.
    let narrow = world.grant("all", &[], &["projects:read", "work_items:read"]).await;
    let response = world.get(Some(&narrow.token), &[], "").await;
    assert_eq!(response.status().as_u16(), 403);
    let challenge = response.headers()["www-authenticate"].to_str().unwrap().to_owned();
    assert!(challenge.contains("error=\"insufficient_scope\""), "{challenge}");
    assert!(challenge.contains("notifications:read"), "{challenge}");
    let notifications_only = world.grant("all", &[], &["notifications:read"]).await;
    let response = world
        .get(Some(&notifications_only.token), &[], "?types=task.created")
        .await;
    assert_eq!(response.status().as_u16(), 403);
    assert!(
        response.headers()["www-authenticate"]
            .to_str()
            .unwrap()
            .contains("tasks:read")
    );

    // Bad types and bad cursors.
    let fresh = world.grant("all", &[], &BOTH).await;
    assert_eq!(
        world
            .get(Some(&fresh.token), &[], "?types=bogus")
            .await
            .status()
            .as_u16(),
        400
    );
    assert_eq!(
        world.get(Some(&fresh.token), &[], "?types=").await.status().as_u16(),
        400
    );
    assert_eq!(
        world
            .get(Some(&fresh.token), &[], "?cursor=18342")
            .await
            .status()
            .as_u16(),
        400
    );
    assert_eq!(
        world
            .get(Some(&fresh.token), &[("last-event-id", "garbage!!")], "")
            .await
            .status()
            .as_u16(),
        400
    );
    // A cursor sealed for another grant does not open here.
    let foreign = world.key.encode(Uuid::new_v4(), Position { seq: 0, at: 1 }).unwrap();
    let response = world.get(Some(&fresh.token), &[("last-event-id", &foreign)], "").await;
    assert_eq!(response.status().as_u16(), 400);
    assert_eq!(response.json::<Value>().await.unwrap()["error"], "invalid_request");
    let other = world.grant("all", &[], &BOTH).await;
    let own_cursor = world.key.encode(other.id, Position { seq: 0, at: 1 }).unwrap();
    assert_eq!(
        world
            .get(Some(&fresh.token), &[], &format!("?cursor={own_cursor}"))
            .await
            .status()
            .as_u16(),
        400,
        "another grant's cursor is refused even when it is well formed"
    );
    world.finish().await;
}

#[tokio::test]
async fn without_configuration_the_stream_path_is_a_404() {
    let scratch = oauth_scratch_or_skip!("stream_off", Start::Complete);
    let state = AppState {
        cfg: AppConfig {
            app_name: "api-test".to_owned(),
            bind_addr: "127.0.0.1:0".to_owned(),
            database_url: Secret::new("postgres://unused"),
            jwt_secret: Secret::new(JWT_SECRET),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            default_author_id: None,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db: scratch.db.clone(),
        flow_permission_cache: FlowPermissionCacheSlot::default(),
    };
    use tower::ServiceExt as _;
    let response = super::routes(None)
        .with_state(state)
        .oneshot(
            axum::http::Request::builder()
                .uri(super::STREAM_PATH)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_stream_opens_with_the_right_headers_and_a_hello() {
    let Some(world) = World::new("hello").await else { return };
    let grant = world.grant("all", &[], &BOTH).await;
    let response = world.get(Some(&grant.token), &[], "").await;
    assert_eq!(response.status().as_u16(), 200);
    let headers = response.headers().clone();
    assert_eq!(headers["content-type"], "text/event-stream; charset=utf-8");
    assert_eq!(headers["cache-control"], "no-store");
    assert_eq!(headers["x-accel-buffering"], "no");
    assert!(headers.get("content-length").is_none(), "a stream has no length");
    assert!(
        headers.get("content-encoding").is_none(),
        "a stream is never compressed"
    );
    let mut sse = Sse {
        response,
        buf: Vec::new(),
    };
    let hello = sse.message(PATIENCE).await;
    assert_eq!(hello.control_type(), Some("hello"));
    assert!(hello.id.is_none(), "a control message never moves the resume position");
    let data = hello.data();
    assert_eq!(data["grant_id"], grant.id.to_string());
    assert_eq!(data["workspace_id"], world.ws().to_string());
    assert!(data["cursor"].as_str().is_some_and(|cursor| cursor.len() == 60));
    assert!(data["server_time"].as_str().is_some_and(|time| time.ends_with('Z')));
    // Opening and closing are audited.
    assert_eq!(
        count(
            world.db(),
            "SELECT count(*) AS n FROM oauth_audit_events WHERE event = 'stream_opened' AND grant_id = $1",
            vec![grant.id.into()]
        )
        .await,
        1
    );
    drop(sse);
    let mut closed = 0;
    for _ in 0..40 {
        closed = count(
            world.db(),
            "SELECT count(*) AS n FROM oauth_audit_events WHERE event = 'stream_closed' AND grant_id = $1 \
             AND detail->>'reason' = 'client_closed'",
            vec![grant.id.into()],
        )
        .await;
        if closed == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(closed, 1, "a client that leaves is audited as client_closed");
    world.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Delivery
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn events_arrive_with_the_frozen_envelope_and_opaque_ids() {
    let Some(world) = World::new("envelope").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    let sent = Instant::now();
    let first = world.note(Some(world.p1()), "issue_assigned:Investigate").await;
    let msg = sse.event(PATIENCE).await;
    eprintln!("latency notification event: {:?}", sent.elapsed());
    let data = msg.data();
    assert_eq!(data["spec"], "sylvode.delegated_event.v1");
    assert_eq!(data["type"], "notification.created");
    assert_eq!(data["id"].as_str(), msg.id.as_deref(), "the envelope id is the SSE id");
    assert_eq!(data["id"].as_str().unwrap(), world.id_of(&grant, first).await);
    assert_eq!(data["workspace_id"], world.ws().to_string());
    assert_eq!(data["project"]["id"], world.p1().to_string());
    assert_eq!(data["project"]["key"], "Pwp4");
    assert_eq!(data["project"]["name"], "project wp4");
    assert_eq!(data["subject"]["content"], "issue_assigned:Investigate");
    assert_eq!(
        data["links"]["web"],
        format!("{ISSUER}/workspace/{}/projects/{}", world.ws(), world.p1())
    );
    let mut keys: Vec<&str> = data.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "id",
            "links",
            "occurred_at",
            "project",
            "spec",
            "subject",
            "type",
            "workspace_id"
        ]
    );
    // The id says nothing about the sequence.
    let id = msg.id.unwrap();
    assert_eq!(id.len(), 60, "{id}");

    // A task event, in order after it, with its own subject.
    world.task(world.p1(), "completed").await;
    let task = sse.event(PATIENCE).await;
    assert_eq!(task.data()["type"], "task.status_changed");
    assert_eq!(task.data()["subject"]["status"], "completed");
    assert_eq!(task.data()["subject"]["previous_status"], "pending");
    assert_eq!(task.data()["subject"]["agent"]["name"], "Triage agent");
    assert_ne!(task.id, Some(id));
    world.finish().await;
}

#[tokio::test]
async fn events_come_in_feed_order_and_ids_are_stable_across_connections() {
    let Some(world) = World::new("order").await else { return };
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, cursor) = world.open_ready(&grant.token).await;
    for n in 1..=4 {
        world.note(Some(world.p1()), &format!("n{n}")).await;
    }
    let mut seen = Vec::new();
    for _ in 0..4 {
        seen.push(sse.event(PATIENCE).await);
    }
    assert_eq!(contents(&seen), ["n1", "n2", "n3", "n4"]);
    let ids: Vec<String> = seen.iter().map(|msg| msg.id.clone().unwrap()).collect();
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), 4);
    drop(sse);

    // Resume after the first event: the remaining three carry the very ids they had before.
    let mut again = world.open_with(&grant.token, &[("last-event-id", &ids[0])], "").await;
    let hello = again.control("hello", PATIENCE).await;
    assert_eq!(
        hello.data()["cursor"].as_str(),
        Some(ids[0].as_str()),
        "hello names where the stream starts"
    );
    let mut replay = Vec::new();
    for _ in 0..3 {
        replay.push(again.event(PATIENCE).await);
    }
    assert_eq!(contents(&replay), ["n2", "n3", "n4"]);
    assert_eq!(
        replay.iter().map(|msg| msg.id.clone().unwrap()).collect::<Vec<_>>(),
        ids[1..].to_vec()
    );
    assert!(!cursor.is_empty());
    world.finish().await;
}

#[tokio::test]
async fn resume_delivers_exactly_the_missed_matching_events() {
    let Some(world) = World::new("resume").await else {
        return;
    };
    let grant = world.grant("selected", &[world.p1()], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    world.note(Some(world.p1()), "seen-1").await;
    world.note(Some(world.p1()), "seen-2").await;
    let first = sse.event(PATIENCE).await;
    let second = sse.event(PATIENCE).await;
    assert_eq!(contents(&[first, second.clone()]), ["seen-1", "seen-2"]);
    drop(sse);

    // While disconnected: two matching events, with several the grant must not see between them.
    world.note(Some(world.p1()), "missed-1").await;
    world.note(Some(world.p2), "hidden: other project").await;
    world
        .feed(
            "notification.created",
            world.ws(),
            Some(world.p1()),
            Some(world.other),
            note_subject("hidden: other user", Some(world.p1())),
        )
        .await;
    world
        .feed(
            "task.created",
            world.w2,
            Some(world.p3),
            None,
            task_subject("pending", "none"),
        )
        .await;
    world.task(world.p1(), "completed").await;
    world.task(world.p2, "completed").await;

    let last_id = second.id.unwrap();
    let mut resumed = world.open_with(&grant.token, &[("last-event-id", &last_id)], "").await;
    resumed.control("hello", PATIENCE).await;
    let one = resumed.event(PATIENCE).await;
    let two = resumed.event(PATIENCE).await;
    assert_eq!(content_of(&one), "missed-1");
    assert_eq!(two.data()["type"], "task.status_changed");
    assert_eq!(two.data()["project"]["id"], world.p1().to_string());
    assert!(
        resumed.silent_for(Duration::from_millis(1200)).await,
        "nothing else arrives: the hidden events leave no trace"
    );
    drop(resumed);

    // The query parameter works the same way, and the header wins when both are given.
    let mut by_query = world.open_with(&grant.token, &[], &format!("?cursor={last_id}")).await;
    by_query.control("hello", PATIENCE).await;
    assert_eq!(content_of(&by_query.event(PATIENCE).await), "missed-1");
    drop(by_query);
    let after_missed = one.id.clone().unwrap();
    let mut both = world
        .open_with(
            &grant.token,
            &[("last-event-id", &after_missed)],
            &format!("?cursor={last_id}"),
        )
        .await;
    both.control("hello", PATIENCE).await;
    assert_eq!(both.event(PATIENCE).await.data()["type"], "task.status_changed");
    world.finish().await;
}

#[tokio::test]
async fn a_resume_after_everything_was_delivered_waits_for_the_next_event() {
    let Some(world) = World::new("resume_idle").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    world.note(Some(world.p1()), "only").await;
    let only = sse.event(PATIENCE).await;
    drop(sse);
    let mut again = world
        .open_with(&grant.token, &[("last-event-id", only.id.as_deref().unwrap())], "")
        .await;
    again.control("hello", PATIENCE).await;
    assert!(
        again.silent_for(Duration::from_millis(800)).await,
        "nothing is repeated"
    );
    world.note(Some(world.p1()), "next").await;
    assert_eq!(content_of(&again.event(PATIENCE).await), "next");
    world.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Backfill limits
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_cursor_older_than_the_window_gets_resync_required_and_starts_at_the_head() {
    let Some(world) = World::new("too_old").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    world.note(Some(world.p1()), "before the connection").await;
    let four_days_ago = Position {
        seq: 0,
        at: platform::timestamp::Timestamp::now().unix_timestamp() - 4 * 86_400,
    };
    let old = world.key.encode(grant.id, four_days_ago).unwrap();
    let mut sse = world.open_with(&grant.token, &[("last-event-id", &old)], "").await;
    let hello = sse.control("hello", PATIENCE).await;
    let resync = sse.control("resync_required", PATIENCE).await;
    assert_eq!(resync.data()["reason"], "cursor_too_old");
    assert!(resync.data()["oldest_available"].as_str().unwrap().ends_with('Z'));
    assert!(resync.id.is_none());
    assert!(
        hello.data()["cursor"].as_str().unwrap() != old,
        "the stream starts at the head, not at the stale cursor"
    );
    assert!(
        sse.silent_for(Duration::from_millis(800)).await,
        "no backfill after a resync"
    );
    world.note(Some(world.p1()), "after").await;
    assert_eq!(
        content_of(&sse.event(PATIENCE).await),
        "after",
        "then it continues live"
    );
    world.finish().await;
}

#[tokio::test]
async fn a_cursor_beyond_the_retention_is_too_old_even_inside_the_age_limit() {
    let Some(world) = World::with(
        "too_old_retention",
        |config| {
            config.event_backfill_max_age_hours = 168;
            config.event_feed_retention_days = 1;
        },
        StreamLimits::from_config(&OAuthConfig::with_defaults(ISSUER)),
        small_tuning(),
        None,
    )
    .await
    else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let two_days_ago = Position {
        seq: 0,
        at: platform::timestamp::Timestamp::now().unix_timestamp() - 2 * 86_400,
    };
    let old = world.key.encode(grant.id, two_days_ago).unwrap();
    let mut sse = world.open_with(&grant.token, &[("last-event-id", &old)], "").await;
    sse.control("hello", PATIENCE).await;
    assert_eq!(
        sse.control("resync_required", PATIENCE).await.data()["reason"],
        "cursor_too_old"
    );
    world.finish().await;
}

#[tokio::test]
async fn too_many_matching_events_behind_the_cursor_means_resync() {
    let Some(world) = World::with(
        "too_many",
        |config| config.event_backfill_max_events = 5,
        StreamLimits::from_config(&OAuthConfig::with_defaults(ISSUER)),
        small_tuning(),
        None,
    )
    .await
    else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    world.note(Some(world.p1()), "anchor").await;
    let anchor = sse.event(PATIENCE).await.id.unwrap();
    drop(sse);

    // Exactly the limit: the backfill is carried.
    for n in 1..=5 {
        world.note(Some(world.p1()), &format!("m{n}")).await;
    }
    let mut within = world.open_with(&grant.token, &[("last-event-id", &anchor)], "").await;
    within.control("hello", PATIENCE).await;
    let mut got = Vec::new();
    for _ in 0..5 {
        got.push(within.event(PATIENCE).await);
    }
    assert_eq!(contents(&got), ["m1", "m2", "m3", "m4", "m5"]);
    drop(within);

    // One more matching event: over the limit, so a resync instead of a partial replay.
    world.note(Some(world.p1()), "m6").await;
    let mut over = world.open_with(&grant.token, &[("last-event-id", &anchor)], "").await;
    over.control("hello", PATIENCE).await;
    assert_eq!(
        over.control("resync_required", PATIENCE).await.data()["reason"],
        "too_many_events"
    );
    assert!(
        over.silent_for(Duration::from_millis(800)).await,
        "no part of the backlog is replayed"
    );

    // Events the grant may not see do not count toward the limit.
    let narrow = world.grant("selected", &[world.p1()], &BOTH).await;
    let (mut seen, _) = world.open_ready(&narrow.token).await;
    world.note(Some(world.p1()), "narrow-anchor").await;
    let narrow_anchor = seen.event(PATIENCE).await.id.unwrap();
    drop(seen);
    for _ in 0..4 {
        world.note(Some(world.p2), "hidden").await;
    }
    world.note(Some(world.p1()), "visible").await;
    let mut hidden_ok = world
        .open_with(&narrow.token, &[("last-event-id", &narrow_anchor)], "")
        .await;
    hidden_ok.control("hello", PATIENCE).await;
    assert_eq!(content_of(&hidden_ok.event(PATIENCE).await), "visible");
    world.finish().await;
}

#[tokio::test]
async fn the_resume_scan_is_bounded_even_when_nothing_matches() {
    let Some(world) = World::with(
        "scan_cap",
        |config| config.event_backfill_max_events = 4,
        StreamLimits::from_config(&OAuthConfig::with_defaults(ISSUER)),
        StreamTuning {
            resume_scan_factor: 2,
            ..small_tuning()
        },
        None,
    )
    .await
    else {
        return;
    };
    let grant = world.grant("selected", &[world.p1()], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    world.note(Some(world.p1()), "anchor").await;
    let anchor = sse.event(PATIENCE).await.id.unwrap();
    drop(sse);
    // None of these is visible to the grant, but there are more than 4 * 2 + 1 of them.
    for _ in 0..12 {
        world.note(Some(world.p2), "hidden").await;
    }
    let mut resumed = world.open_with(&grant.token, &[("last-event-id", &anchor)], "").await;
    resumed.control("hello", PATIENCE).await;
    assert_eq!(
        resumed.control("resync_required", PATIENCE).await.data()["reason"],
        "too_many_events",
        "the check reads a bounded number of rows and gives up"
    );
    world.finish().await;
}

#[tokio::test]
async fn a_cursor_ahead_of_the_feed_means_resync() {
    let Some(world) = World::new("ahead").await else { return };
    let grant = world.grant("all", &[], &BOTH).await;
    let ahead = world
        .key
        .encode(
            grant.id,
            Position {
                seq: 1_000_000,
                at: platform::timestamp::Timestamp::now().unix_timestamp(),
            },
        )
        .unwrap();
    let mut sse = world.open_with(&grant.token, &[("last-event-id", &ahead)], "").await;
    sse.control("hello", PATIENCE).await;
    assert_eq!(
        sse.control("resync_required", PATIENCE).await.data()["reason"],
        "cursor_ahead"
    );
    world.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Delivery-time filtering
// ---------------------------------------------------------------------------------------------

/// A user in two workspaces and three projects, one grant on one workspace and one project: the
/// events of everything else never arrive, live and in backfill.
#[tokio::test]
async fn a_selected_grant_sees_only_its_workspace_project_and_user() {
    let Some(world) = World::new("filter_selected").await else {
        return;
    };
    let grant = world.grant("selected", &[world.p1()], &BOTH).await;
    let (mut sse, cursor) = world.open_ready(&grant.token).await;
    let emit = |tag: &'static str| {
        let world = &world;
        async move {
            match tag {
                "p1-note" => world.note(Some(world.p1()), "visible: note in p1").await,
                "p2-note" => world.note(Some(world.p2), "HIDDEN p2 note").await,
                "other-user" => {
                    world
                        .feed(
                            "notification.created",
                            world.ws(),
                            Some(world.p1()),
                            Some(world.other),
                            note_subject("HIDDEN other user", Some(world.p1())),
                        )
                        .await
                }
                "w2-note" => {
                    world
                        .feed(
                            "notification.created",
                            world.w2,
                            Some(world.p3),
                            Some(world.user()),
                            note_subject("HIDDEN w2", Some(world.p3)),
                        )
                        .await
                }
                "w2-task" => {
                    world
                        .feed(
                            "task.created",
                            world.w2,
                            Some(world.p3),
                            None,
                            task_subject("pending", "x"),
                        )
                        .await
                }
                "p2-task" => world.task(world.p2, "completed").await,
                "no-project" => world.note(None, "HIDDEN unresolved project on a selected grant").await,
                "p1-task" => world.task(world.p1(), "completed").await,
                other => panic!("unknown event tag {other}"),
            }
        }
    };
    let order = [
        "p2-note",
        "other-user",
        "w2-note",
        "w2-task",
        "p2-task",
        "no-project",
        "p1-note",
        "p1-task",
    ];
    for tag in order {
        emit(tag).await;
    }
    let live_note = sse.event(PATIENCE).await;
    assert_eq!(content_of(&live_note), "visible: note in p1");
    let live_task = sse.event(PATIENCE).await;
    assert_eq!(live_task.data()["type"], "task.status_changed");
    assert!(
        sse.silent_for(Duration::from_millis(1200)).await,
        "live: nothing else arrives"
    );
    drop(sse);

    // Backfill from the connection's own starting cursor carries the same two events and no more.
    let mut backfill = world.open_with(&grant.token, &[("last-event-id", &cursor)], "").await;
    backfill.control("hello", PATIENCE).await;
    let one = backfill.event(PATIENCE).await;
    let two = backfill.event(PATIENCE).await;
    assert_eq!(
        (one.id, two.id),
        (live_note.id, live_task.id),
        "the same events, with the same ids"
    );
    assert!(
        backfill.silent_for(Duration::from_millis(1200)).await,
        "backfill: nothing else arrives"
    );
    world.finish().await;
}

#[tokio::test]
async fn an_all_projects_grant_sees_every_project_of_its_workspace_and_no_other() {
    let Some(world) = World::new("filter_all").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, cursor) = world.open_ready(&grant.token).await;
    world.note(Some(world.p1()), "p1").await;
    world.note(Some(world.p2), "p2").await;
    world.note(None, "unresolved").await;
    world
        .feed(
            "notification.created",
            world.w2,
            Some(world.p3),
            Some(world.user()),
            note_subject("HIDDEN w2", Some(world.p3)),
        )
        .await;
    world.task(world.p2, "completed").await;
    world
        .feed("task.created", world.ws(), None, None, task_subject("pending", "x"))
        .await;
    let mut live = Vec::new();
    for _ in 0..4 {
        live.push(sse.event(PATIENCE).await);
    }
    assert_eq!(contents(&live[..3]), ["p1", "p2", "unresolved"]);
    assert_eq!(live[3].data()["type"], "task.status_changed");
    assert!(live[2].data()["project"].is_null());
    assert!(sse.silent_for(Duration::from_millis(1200)).await);
    drop(sse);
    let mut backfill = world.open_with(&grant.token, &[("last-event-id", &cursor)], "").await;
    backfill.control("hello", PATIENCE).await;
    for expected in &live {
        assert_eq!(backfill.event(PATIENCE).await.id, expected.id);
    }
    assert!(backfill.silent_for(Duration::from_millis(1200)).await);

    // The `types` filter narrows a stream further, and only to what the scopes allow.
    let mut only_tasks = world
        .open_with(
            &grant.token,
            &[("last-event-id", &cursor)],
            "?types=task.status_changed",
        )
        .await;
    only_tasks.control("hello", PATIENCE).await;
    assert_eq!(only_tasks.event(PATIENCE).await.data()["type"], "task.status_changed");
    assert!(only_tasks.silent_for(Duration::from_millis(800)).await);
    world.finish().await;
}

#[tokio::test]
async fn a_grant_without_a_scope_never_sees_that_family() {
    let Some(world) = World::new("filter_scope").await else {
        return;
    };
    let notifications = world.grant("all", &[], &["notifications:read"]).await;
    let tasks = world.grant("all", &[], &["tasks:read"]).await;
    let (mut n_stream, _) = world.open_ready(&notifications.token).await;
    let (mut t_stream, _) = world.open_ready(&tasks.token).await;
    world.task(world.p1(), "completed").await;
    world.note(Some(world.p1()), "a note").await;
    assert_eq!(content_of(&n_stream.event(PATIENCE).await), "a note");
    assert_eq!(t_stream.event(PATIENCE).await.data()["type"], "task.status_changed");
    assert!(n_stream.silent_for(Duration::from_millis(800)).await);
    assert!(t_stream.silent_for(Duration::from_millis(800)).await);
    world.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Permission lost after the event was produced
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_event_about_a_project_that_is_gone_is_not_sent_even_in_backfill() {
    let Some(world) = World::new("project_gone").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let selected = world.grant("selected", &[world.p2], &BOTH).await;
    let (mut sse, cursor) = world.open_ready(&grant.token).await;
    let (mut narrow, narrow_cursor) = world.open_ready(&selected.token).await;
    world.note(Some(world.p2), "about p2 before it is deleted").await;
    world.note(Some(world.p1()), "about p1").await;
    assert_eq!(content_of(&sse.event(PATIENCE).await), "about p2 before it is deleted");
    assert_eq!(content_of(&sse.event(PATIENCE).await), "about p1");
    assert_eq!(
        content_of(&narrow.event(PATIENCE).await),
        "about p2 before it is deleted"
    );
    drop((sse, narrow));

    // The project is deleted afterwards; the feed rows still name it.
    exec(world.db(), "DELETE FROM projects WHERE id = $1", vec![world.p2.into()]).await;
    let mut backfill = world.open_with(&grant.token, &[("last-event-id", &cursor)], "").await;
    backfill.control("hello", PATIENCE).await;
    assert_eq!(content_of(&backfill.event(PATIENCE).await), "about p1");
    assert!(
        backfill.silent_for(Duration::from_secs(1)).await,
        "the deleted project's event is not sent"
    );
    let mut gone = world
        .open_with(&selected.token, &[("last-event-id", &narrow_cursor)], "")
        .await;
    gone.control("hello", PATIENCE).await;
    assert!(
        gone.silent_for(Duration::from_secs(1)).await,
        "its grant lost the project with the project"
    );
    world.finish().await;
}

#[tokio::test]
async fn losing_the_project_from_the_grant_stops_its_events_within_the_cache_bound() {
    let Some(world) = World::new("project_dropped").await else {
        return;
    };
    let grant = world.grant("selected", &[world.p1(), world.p2], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    world.note(Some(world.p2), "p2 while granted").await;
    assert_eq!(content_of(&sse.event(PATIENCE).await), "p2 while granted");
    exec(
        world.db(),
        "DELETE FROM oauth_grant_projects WHERE grant_id = $1 AND project_id = $2",
        vec![grant.id.into(), world.p2.into()],
    )
    .await;
    // State older than the cache bound is never used for a delivery.
    tokio::time::sleep(world.hub.tuning.grant_cache_ttl + Duration::from_millis(600)).await;
    world.note(Some(world.p2), "p2 after it was withdrawn").await;
    world.note(Some(world.p1()), "p1 still granted").await;
    assert_eq!(
        content_of(&sse.event(PATIENCE).await),
        "p1 still granted",
        "the withdrawn project's event was skipped"
    );
    world.finish().await;
}

#[tokio::test]
async fn losing_membership_ends_the_stream_before_any_further_event() {
    let Some(world) = World::new("membership_lazy").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    // The membership row disappears with no hook and no notification.
    exec(
        world.db(),
        "DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![world.ws().into(), world.user().into()],
    )
    .await;
    let removed = Instant::now();
    tokio::time::sleep(world.hub.tuning.grant_cache_ttl + Duration::from_millis(300)).await;
    world.note(Some(world.p1()), "after the removal").await;
    let messages = sse.read_to_end(REVOCATION_BOUND).await;
    assert!(removed.elapsed() < REVOCATION_BOUND + Duration::from_secs(1));
    assert!(
        messages.iter().all(|msg| !msg.is_event()),
        "no event after the removal: {messages:?}"
    );
    let last = messages.last().expect("a closing message");
    assert_eq!(last.control_type(), Some("revoked"));
    assert_eq!(last.data()["reason"], "membership_removed");
    let (status, reason) = world.grant_state(&grant).await;
    assert_eq!(
        (status.as_str(), reason.as_deref()),
        ("revoked", Some("membership_removed"))
    );
    assert_eq!(world.get(Some(&grant.token), &[], "").await.status().as_u16(), 401);
    world.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Revocation
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Path {
    UserRevoked,
    ClientRevoked,
    WorkspaceAdminRevoked,
    WorkspaceDisabled,
    PasswordChanged,
    AccountDisabled,
    MembershipRemoved,
    RefreshReuse,
    CodeReplay,
    ClientDisabled,
}

impl Path {
    const fn reason(self) -> RevokeReason {
        match self {
            Self::UserRevoked => RevokeReason::UserRevoked,
            Self::ClientRevoked => RevokeReason::ClientRevoked,
            Self::WorkspaceAdminRevoked => RevokeReason::WorkspaceAdminRevoked,
            Self::WorkspaceDisabled => RevokeReason::WorkspaceDisabled,
            Self::PasswordChanged => RevokeReason::PasswordChanged,
            Self::AccountDisabled => RevokeReason::AccountDisabled,
            Self::MembershipRemoved => RevokeReason::MembershipRemoved,
            Self::RefreshReuse => RevokeReason::RefreshReuse,
            Self::CodeReplay => RevokeReason::CodeReplay,
            Self::ClientDisabled => RevokeReason::ClientDisabled,
        }
    }
}

/// Performs the revocation the way its hook does, through WP1's helpers, and commits.
async fn revoke_via(world: &World, grant: &Grant, path: Path) {
    let tx = world.db().begin().await.expect("begin");
    let user = world.user();
    match path {
        Path::UserRevoked | Path::ClientRevoked | Path::RefreshReuse | Path::CodeReplay => {
            let actor = if matches!(path, Path::UserRevoked) {
                Actor::User(user)
            } else {
                Actor::System
            };
            assert!(revoke_grant(&tx, grant.id, path.reason(), actor).await.expect("revoke"));
        }
        Path::WorkspaceAdminRevoked => {
            assert!(
                revoke_grant(&tx, grant.id, path.reason(), Actor::User(world.other))
                    .await
                    .expect("revoke")
            );
        }
        Path::WorkspaceDisabled => {
            exec(
                &tx,
                "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps) VALUES ($1, false) \
                 ON CONFLICT (workspace_id) DO UPDATE SET allow_connected_apps = false",
                vec![world.ws().into()],
            )
            .await;
            let revoked = revoke_workspace_grants(&tx, world.ws(), path.reason(), Actor::User(user))
                .await
                .expect("revoke");
            assert!(revoked.contains(&grant.id));
        }
        Path::PasswordChanged => {
            exec(
                &tx,
                "UPDATE users SET session_version = session_version + 1 WHERE id = $1",
                vec![user.into()],
            )
            .await;
            let revoked = revoke_all_user_grants(&tx, user, path.reason(), Actor::System)
                .await
                .expect("revoke");
            assert!(revoked.contains(&grant.id));
        }
        Path::AccountDisabled => {
            exec(
                &tx,
                "UPDATE users SET is_active = false WHERE id = $1",
                vec![user.into()],
            )
            .await;
            let revoked = revoke_all_user_grants(&tx, user, path.reason(), Actor::System)
                .await
                .expect("revoke");
            assert!(revoked.contains(&grant.id));
        }
        Path::MembershipRemoved => {
            exec(
                &tx,
                "DELETE FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
                vec![world.ws().into(), user.into()],
            )
            .await;
            let revoked = revoke_user_workspace_grants(&tx, user, world.ws(), path.reason(), Actor::System)
                .await
                .expect("revoke");
            assert!(revoked.contains(&grant.id));
        }
        Path::ClientDisabled => {
            exec(
                &tx,
                "UPDATE oauth_clients SET status = 'disabled' WHERE client_id = 'ramflux'",
                vec![],
            )
            .await;
            let revoked = revoke_client_grants(&tx, "ramflux", path.reason(), Actor::System)
                .await
                .expect("revoke");
            assert!(revoked.contains(&grant.id));
        }
    }
    tx.commit().await.expect("commit");
}

/// Opens a stream, revokes its grant by `path`, and checks that it stops within the bound with the
/// right reason, that nothing but the closing message follows the revocation, and that a reconnect
/// is a 401. Returns the time from the revocation's commit to the closing message.
async fn check_revocation(label: &str, path: Path, tuning: StreamTuning) -> Option<Duration> {
    let world = World::with(
        label,
        |_| {},
        StreamLimits::from_config(&OAuthConfig::with_defaults(ISSUER)),
        tuning,
        None,
    )
    .await?;
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    world.note(Some(world.p1()), "before the revocation").await;
    assert_eq!(content_of(&sse.event(PATIENCE).await), "before the revocation");

    revoke_via(&world, &grant, path).await;
    let committed = Instant::now();
    // An event produced right after the revocation must not reach the client.
    world.note(Some(world.p1()), "after the revocation").await;
    let messages = sse.read_to_end(REVOCATION_BOUND + Duration::from_secs(2)).await;
    let stopped = committed.elapsed();
    let last = messages.last().expect("a closing message");
    assert!(
        messages.iter().all(|msg| !msg.is_event()),
        "{path:?}: no event follows the revocation: {messages:?}"
    );
    assert_eq!(last.control_type(), Some("revoked"), "{path:?}: {messages:?}");
    assert_eq!(last.data()["reason"], path.reason().as_str(), "{path:?}");
    assert!(
        stopped < REVOCATION_BOUND,
        "{path:?}: the stream stopped after {stopped:?}"
    );
    // A reconnect with any token of the grant is a 401.
    assert_eq!(
        world.get(Some(&grant.token), &[], "").await.status().as_u16(),
        401,
        "{path:?}"
    );
    let spare = world.another_token(&grant, "api").await;
    assert_eq!(
        world.get(Some(&spare), &[], "").await.status().as_u16(),
        401,
        "{path:?}"
    );
    eprintln!("revocation path={path:?} stop_ms={}", stopped.as_millis());
    world.finish().await;
    Some(stopped)
}

macro_rules! revocation_test {
    ($name:ident, $path:expr) => {
        #[tokio::test]
        async fn $name() {
            check_revocation(stringify!($name), $path, StreamTuning::default()).await;
        }
    };
}

revocation_test!(revoked_user_disconnects_in_sylvode, Path::UserRevoked);
revocation_test!(revoked_user_disconnects_in_the_client, Path::ClientRevoked);
revocation_test!(revoked_workspace_admin, Path::WorkspaceAdminRevoked);
revocation_test!(revoked_workspace_turns_connected_apps_off, Path::WorkspaceDisabled);
revocation_test!(revoked_password_change, Path::PasswordChanged);
revocation_test!(revoked_account_disabled, Path::AccountDisabled);
revocation_test!(revoked_membership_removed, Path::MembershipRemoved);
revocation_test!(revoked_refresh_token_replay, Path::RefreshReuse);
revocation_test!(revoked_authorization_code_replay, Path::CodeReplay);
revocation_test!(revoked_client_disabled, Path::ClientDisabled);

/// With the listener switched off, the timers alone must still meet the bound.
#[tokio::test]
async fn revocation_without_the_listener_stops_the_stream_by_timer() {
    let tuning = StreamTuning {
        listen: false,
        ..StreamTuning::default()
    };
    for (label, path) in [
        ("nolisten_user", Path::UserRevoked),
        ("nolisten_password", Path::PasswordChanged),
    ] {
        let stopped = check_revocation(label, path, tuning).await;
        if let Some(stopped) = stopped {
            assert!(
                stopped <= tuning.idle_tick + Duration::from_secs(1),
                "{path:?}: {stopped:?}"
            );
        }
    }
}

/// Live-state failures that no hook announced: the stream still ends, with the reason the hook
/// would have recorded, and the grant is revoked for the next connection.
#[tokio::test]
async fn state_changes_without_a_hook_end_the_stream_by_timer() {
    type Mutation = (&'static str, &'static str, &'static str);
    let cases: [Mutation; 4] = [
        (
            "lazy_account",
            "UPDATE users SET is_active = false WHERE id = $1",
            "account_disabled",
        ),
        (
            "lazy_password",
            "UPDATE users SET session_version = session_version + 1 WHERE id = $1",
            "password_changed",
        ),
        (
            "lazy_client",
            "UPDATE oauth_clients SET status = 'disabled' WHERE $1::uuid IS NOT NULL AND client_id = 'ramflux'",
            "client_disabled",
        ),
        (
            "lazy_workspace",
            "INSERT INTO workspace_delegation_settings (workspace_id, allow_connected_apps) \
             SELECT workspace_id, false FROM oauth_grants WHERE user_id = $1 \
             ON CONFLICT (workspace_id) DO UPDATE SET allow_connected_apps = false",
            "workspace_disabled",
        ),
    ];
    for (label, sql, expected) in cases {
        let Some(world) = World::new(label).await else { return };
        let grant = world.grant("all", &[], &BOTH).await;
        let (mut sse, _) = world.open_ready(&grant.token).await;
        exec(world.db(), sql, vec![world.user().into()]).await;
        let changed = Instant::now();
        // Nothing announced the change, so an event may still be delivered on state at most the
        // cache bound old; one produced after that bound never is.
        tokio::time::sleep(world.hub.tuning.grant_cache_ttl + Duration::from_millis(300)).await;
        world.note(Some(world.p1()), "after the change").await;
        let messages = sse.read_to_end(REVOCATION_BOUND + Duration::from_secs(2)).await;
        assert!(changed.elapsed() < REVOCATION_BOUND, "{label}: {:?}", changed.elapsed());
        assert!(messages.iter().all(|msg| !msg.is_event()), "{label}: {messages:?}");
        let last = messages.last().expect("closing message");
        assert_eq!(last.control_type(), Some("revoked"), "{label}");
        assert_eq!(last.data()["reason"], expected, "{label}");
        let (status, reason) = world.grant_state(&grant).await;
        assert_eq!(
            (status.as_str(), reason.as_deref()),
            ("revoked", Some(expected)),
            "{label}"
        );
        assert_eq!(
            world.get(Some(&grant.token), &[], "").await.status().as_u16(),
            401,
            "{label}"
        );
        world.finish().await;
    }
}

#[tokio::test]
async fn a_revoked_access_token_ends_only_the_streams_opened_with_it() {
    let Some(world) = World::new("token_revoked").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let spare = world.another_token(&grant, "api").await;
    let (mut with_first, _) = world.open_ready(&grant.token).await;
    let (mut with_spare, _) = world.open_ready(&spare).await;
    exec(
        world.db(),
        "UPDATE oauth_tokens SET revoked_at = now() WHERE id = $1",
        vec![grant.token_id.into()],
    )
    .await;
    exec(
        world.db(),
        "SELECT pg_notify($1, $2)",
        vec![GRANT_CHANGED_CHANNEL.into(), grant.id.to_string().into()],
    )
    .await;
    let started = Instant::now();
    let messages = with_first.read_to_end(REVOCATION_BOUND).await;
    let last = messages.last().expect("closing message");
    assert_eq!(last.control_type(), Some("reauth"));
    assert_eq!(last.data()["reason"], "token_revoked");
    eprintln!("token revocation stop_ms={}", started.elapsed().as_millis());
    // The grant and its other token are untouched: that stream keeps delivering.
    world.note(Some(world.p1()), "still flowing").await;
    assert_eq!(content_of(&with_spare.event(PATIENCE).await), "still flowing");
    assert_eq!(world.get(Some(&grant.token), &[], "").await.status().as_u16(), 401);
    world.finish().await;
}

#[tokio::test]
async fn a_stream_does_not_outlive_its_token() {
    let Some(world) = World::new("token_expiry").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    #[derive(FromQueryResult)]
    struct Expiry {
        at_ms: i64,
    }
    exec(
        world.db(),
        "UPDATE oauth_tokens SET expires_at = now() + interval '2 seconds' WHERE id = $1",
        vec![grant.token_id.into()],
    )
    .await;
    let expiry = Expiry::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT (EXTRACT(EPOCH FROM expires_at) * 1000)::bigint AS at_ms FROM oauth_tokens WHERE id = $1",
        vec![grant.token_id.into()],
    ))
    .one(world.db())
    .await
    .unwrap()
    .unwrap();
    // The database may run on a clock of its own (a container VM); measure against it.
    #[derive(FromQueryResult)]
    struct Clock {
        ms: i64,
    }
    let host_ms = || {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    };
    let before = host_ms();
    let db_ms = Clock::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::bigint AS ms",
        vec![],
    ))
    .one(world.db())
    .await
    .unwrap()
    .unwrap()
    .ms;
    let skew = i64::midpoint(before, host_ms()) - db_ms;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    world.note(Some(world.p1()), "inside the lifetime").await;
    let applied = sse.event(PATIENCE).await;
    assert_eq!(content_of(&applied), "inside the lifetime");
    // The client's resume position is the id of the last event it applied.
    let applied = applied.id.unwrap();
    let messages = sse.read_to_end(Duration::from_secs(8)).await;
    let overshoot = host_ms() - (expiry.at_ms + skew);
    let last = messages.last().expect("closing message");
    assert_eq!(last.control_type(), Some("reauth"));
    assert_eq!(last.data()["reason"], "token_expired");
    eprintln!("token expiry overshoot_ms={overshoot} (clock skew to the database {skew} ms)");
    assert!(
        overshoot >= -100,
        "the stream closed before the token expired: {overshoot}"
    );
    assert!(overshoot < 1500, "the stream outlived its token by {overshoot} ms");
    // The old token is refused; a fresh one on the same grant resumes where the client was.
    assert_eq!(world.get(Some(&grant.token), &[], "").await.status().as_u16(), 401);
    let fresh = world.another_token(&grant, "api").await;
    world.note(Some(world.p1()), "while reconnecting").await;
    let mut resumed = world.open_with(&fresh, &[("last-event-id", &applied)], "").await;
    resumed.control("hello", PATIENCE).await;
    assert_eq!(content_of(&resumed.event(PATIENCE).await), "while reconnecting");
    world.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Limits, back-pressure, heartbeats
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_per_grant_limit_answers_429_and_frees_a_place_when_a_stream_closes() {
    let Some(world) = World::new("limit_grant").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let mut open = Vec::new();
    for _ in 0..3 {
        open.push(world.open_ready(&grant.token).await.0);
    }
    let refused = world.get(Some(&grant.token), &[], "").await;
    assert_eq!(refused.status().as_u16(), 429);
    assert_eq!(refused.headers()["retry-after"], "30");
    assert_eq!(world.hub.open_streams(), 3);
    drop(open.pop());
    let mut accepted = false;
    for _ in 0..40 {
        if world.get(Some(&grant.token), &[], "").await.status().as_u16() == 200 {
            accepted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(accepted, "a closed stream gives its place back");
    world.finish().await;
}

#[tokio::test]
async fn the_per_user_and_per_process_limits_answer_429() {
    let limits = StreamLimits {
        per_grant: 3,
        per_user: 2,
        per_process: 3,
    };
    let Some(world) = World::with("limit_user", |_| {}, limits, small_tuning(), None).await else {
        return;
    };
    let (first, second, third) = (
        world.grant("all", &[], &BOTH).await,
        world.grant("all", &[], &BOTH).await,
        world.grant("all", &[], &BOTH).await,
    );
    let mut open = vec![
        world.open_ready(&first.token).await.0,
        world.open_ready(&second.token).await.0,
    ];
    // Same user, third grant: the user limit, though no single grant is at its limit.
    assert_eq!(world.get(Some(&third.token), &[], "").await.status().as_u16(), 429);
    // Another user is admitted until the process cap.
    let other = world.grant_for(world.other, world.ws(), "all", &[], &BOTH).await;
    open.push(world.open_ready(&other.token).await.0);
    let more = world.grant_for(world.other, world.ws(), "all", &[], &BOTH).await;
    let refused = world.get(Some(&more.token), &[], "").await;
    assert_eq!(refused.status().as_u16(), 429);
    assert_eq!(refused.headers()["retry-after"], "30");
    drop(open);
    world.finish().await;
}

#[tokio::test]
async fn connection_attempts_are_limited_per_grant_per_minute() {
    let Some(mut world) = World::new("limit_rate").await else {
        return;
    };
    world.clear_rate = false;
    let grant = world.grant("all", &[], &BOTH).await;
    for _ in 0..10 {
        let response = world.get(Some(&grant.token), &[], "").await;
        assert_eq!(response.status().as_u16(), 200);
        drop(response);
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    let refused = world.get(Some(&grant.token), &[], "").await;
    assert_eq!(refused.status().as_u16(), 429);
    let retry: u64 = refused.headers()["retry-after"].to_str().unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry), "{retry}");
    world.finish().await;
}

/// Many idle streams share a pool far smaller than their number: an idle stream holds no
/// database connection.
#[tokio::test]
async fn idle_streams_hold_no_database_connection() {
    let limits = StreamLimits {
        per_grant: 20,
        per_user: 40,
        per_process: 100,
    };
    let Some(world) = World::with("pool", |_| {}, limits, small_tuning(), Some(4)).await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let mut streams = Vec::new();
    for _ in 0..16 {
        streams.push(world.open_ready(&grant.token).await.0);
    }
    assert_eq!(world.hub.open_streams(), 16);
    // With a pool of four, sixteen streams that each kept a connection could not all be open.
    tokio::time::sleep(Duration::from_millis(500)).await;
    world.note(Some(world.p1()), "to every stream").await;
    for stream in &mut streams {
        assert_eq!(content_of(&stream.event(PATIENCE).await), "to every stream");
    }
    world.finish().await;
}

#[tokio::test]
async fn heartbeats_are_sent_when_the_stream_is_silent() {
    let tuning = StreamTuning {
        keepalive: Duration::from_millis(400),
        ..small_tuning()
    };
    let Some(world) = World::with(
        "heartbeat",
        |_| {},
        StreamLimits::from_config(&OAuthConfig::with_defaults(ISSUER)),
        tuning,
        None,
    )
    .await
    else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let mut sse = world.open(&grant.token).await;
    sse.control("hello", PATIENCE).await;
    let mut beats = Vec::new();
    while beats.len() < 3 {
        match sse.next(Duration::from_secs(3)).await {
            Next::Msg(msg) => {
                assert_eq!(msg.comment.as_deref(), Some("keep-alive"), "{msg:?}");
                assert!(msg.event.is_none() && msg.id.is_none() && msg.data.is_none());
                beats.push(Instant::now());
            }
            Next::Eof => panic!("the stream ended"),
            Next::Timeout => panic!("no heartbeat within 3 s after {} beats", beats.len()),
        }
    }
    for pair in beats.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= Duration::from_millis(250) && gap <= Duration::from_millis(1500),
            "heartbeat gap {gap:?}"
        );
    }
    // A heartbeat does not disturb delivery.
    world.note(Some(world.p1()), "between beats").await;
    assert_eq!(content_of(&sse.event(PATIENCE).await), "between beats");
    world.finish().await;
}

/// A client that stops reading is sent `lagged` and closed instead of letting the queue grow.
#[tokio::test]
async fn a_client_that_does_not_drain_is_disconnected_with_lagged() {
    let tuning = StreamTuning {
        channel_capacity: 2,
        lag_timeout: Duration::from_millis(400),
        ..small_tuning()
    };
    let Some(world) = World::with(
        "lagged",
        |_| {},
        StreamLimits::from_config(&OAuthConfig::with_defaults(ISSUER)),
        tuning,
        None,
    )
    .await
    else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    // Many large events at once; the client reads nothing for a while.
    exec(
        world.db(),
        "INSERT INTO oauth_event_feed (event_type, workspace_id, project_id, user_id, subject, occurred_at) \
         SELECT 'notification.created', $1, $2, $3, \
                jsonb_build_object('kind','notification','notification_id', gen_random_uuid(), \
                  'notification_type','assignment','title_key','k','content', repeat('x', 4000), \
                  'related', jsonb_build_object('issue_id', NULL, 'comment_id', NULL, 'project_id', $2), \
                  'is_read', false), now() \
         FROM generate_series(1, 3000)",
        vec![world.ws().into(), world.p1().into(), world.user().into()],
    )
    .await;
    exec(world.db(), "SELECT pg_notify('sv_oauth_feed', '1')", vec![]).await;
    // The server must give up on the stalled client by itself and release its place.
    let mut released = false;
    for _ in 0..100 {
        let closed = count(
            world.db(),
            "SELECT count(*) AS n FROM oauth_audit_events WHERE event = 'stream_closed' AND detail->>'reason' = 'lagged'",
            vec![],
        )
        .await;
        if closed == 1 {
            released = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        released,
        "the server closed the stream as lagged while the client was not reading"
    );
    // The client now reads what was already on the wire, then the closing message.
    let messages = sse.read_to_end(PATIENCE).await;
    let delivered = messages.iter().filter(|msg| msg.is_event()).count();
    let last = messages.last().expect("closing message");
    assert_eq!(last.control_type(), Some("lagged"), "{last:?}");
    assert!(
        last.data()["last_sent"]
            .as_str()
            .is_some_and(|cursor| cursor.len() == 60)
    );
    assert!(delivered < 3000, "the backlog was not delivered in full: {delivered}");
    eprintln!("lagged: {delivered} of 3000 events reached the client before the close");
    world.finish().await;
}

#[tokio::test]
async fn streams_end_on_shutdown_and_the_client_can_resume() {
    let Some(world) = World::new("shutdown").await else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    world.note(Some(world.p1()), "delivered").await;
    let applied = sse.event(PATIENCE).await.id.unwrap();
    world.hub.shutdown();
    let messages = sse.read_to_end(Duration::from_secs(3)).await;
    assert!(messages.iter().all(|msg| !msg.is_event()), "{messages:?}");
    assert!(
        messages.iter().all(|msg| msg.control_type().is_none()),
        "a shutdown sends no control message"
    );
    // Another process (a fresh hub is simulated by a fresh world below) would accept the resume.
    assert!(!applied.is_empty());
    world.finish().await;
}

#[tokio::test]
async fn delivery_works_without_the_listener_by_timer() {
    let tuning = StreamTuning {
        listen: false,
        ..small_tuning()
    };
    let Some(world) = World::with(
        "nolisten_feed",
        |_| {},
        StreamLimits::from_config(&OAuthConfig::with_defaults(ISSUER)),
        tuning,
        None,
    )
    .await
    else {
        return;
    };
    let grant = world.grant("all", &[], &BOTH).await;
    let (mut sse, _) = world.open_ready(&grant.token).await;
    let sent = Instant::now();
    world.note(Some(world.p1()), "found by polling").await;
    let msg = sse.event(Duration::from_secs(6)).await;
    eprintln!("latency without the listener: {:?}", sent.elapsed());
    assert_eq!(content_of(&msg), "found by polling");
    assert!(sent.elapsed() <= tuning.idle_tick + Duration::from_secs(2));
    world.finish().await;
}

// ---------------------------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------------------------

mod capture {
    use std::fmt::Write as _;
    use std::sync::Arc;

    use parking_lot::Mutex;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    /// Records every event and span field. `everything` holds all levels; `info_up` holds what a
    /// deployment logging at its default level (info) would emit.
    #[derive(Clone, Default)]
    pub struct Capture {
        pub everything: Arc<Mutex<String>>,
        pub info_up: Arc<Mutex<String>>,
    }

    struct Fields<'a>(&'a mut String);

    impl Visit for Fields<'_> {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            let _ = write!(self.0, " {}={value:?}", field.name());
        }
    }

    impl Capture {
        fn push(&self, level: Level, mut line: String) {
            line.push('\n');
            self.everything.lock().push_str(&line);
            if level <= Level::INFO {
                self.info_up.lock().push_str(&line);
            }
        }
    }

    impl Subscriber for Capture {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, attributes: &Attributes<'_>) -> Id {
            let mut line = String::from("span");
            attributes.record(&mut Fields(&mut line));
            self.push(*attributes.metadata().level(), line);
            Id::from_u64(1)
        }

        fn record(&self, _: &Id, values: &Record<'_>) {
            let mut line = String::from("record");
            values.record(&mut Fields(&mut line));
            self.push(Level::TRACE, line);
        }

        fn record_follows_from(&self, _: &Id, _: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut line = format!("{} {}", event.metadata().level(), event.metadata().target());
            event.record(&mut Fields(&mut line));
            self.push(*event.metadata().level(), line);
        }

        fn enter(&self, _: &Id) {}

        fn exit(&self, _: &Id) {}
    }
}

/// Everything the stream, token validation and the database layer log while a stream is opened,
/// refused, limited, revoked and closed contains no token, no cursor and no event payload.
#[tokio::test(flavor = "current_thread")]
async fn nothing_secret_reaches_the_logs() {
    let capture = capture::Capture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());
    let Some(world) = World::new("logs").await else { return };
    let grant = world.grant("all", &[], &BOTH).await;
    let spare = world.another_token(&grant, "api").await;
    let mcp = world.another_token(&grant, "mcp").await;

    // A normal session with an event whose content must stay out of the logs.
    let (mut sse, cursor) = world.open_ready(&grant.token).await;
    world
        .note(Some(world.p1()), "SECRET-PAYLOAD-MARKER issue_assigned:Private title")
        .await;
    let event = sse.event(PATIENCE).await;
    let event_id = event.id.clone().unwrap();
    // Refusals: bad token, wrong audience, token in a URL, bad cursor, limits.
    let _ = world
        .get(
            Some("sva_0000000000000000000000000000000000000000000000000000000000000000"),
            &[],
            "",
        )
        .await;
    let _ = world.get(Some(&mcp), &[], "").await;
    let _ = world
        .get(Some(&grant.token), &[], &format!("?access_token={}", grant.token))
        .await;
    let _ = world
        .get(Some(&spare), &[("last-event-id", "forged-cursor-value")], "")
        .await;
    let mut extra = Vec::new();
    for _ in 0..3 {
        extra.push(world.get(Some(&spare), &[], "").await);
    }
    // Revocation and close.
    revoke_via(&world, &grant, Path::UserRevoked).await;
    let _ = sse.read_to_end(REVOCATION_BOUND + Duration::from_secs(2)).await;
    drop(extra);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let everything = capture.everything.lock().clone();
    let info_up = capture.info_up.lock().clone();
    assert!(
        info_up.lines().count() > 2 && everything.lines().count() > 5,
        "the capture saw the process's logging, so the assertions below mean something:\n{everything}"
    );
    // What must never be logged at any level: raw tokens, cursors, event ids, the signing secret.
    let never = [
        grant.token.clone(),
        spare.clone(),
        mcp.clone(),
        cursor,
        event_id,
        JWT_SECRET.to_owned(),
    ];
    // What is also kept out of a deployment's default (info) log: token hashes and event content.
    // Statement logging of the database layer prints bound values at debug level and below; that is
    // how the whole code base's queries (this stream's included) behave when it is switched on.
    let hashes: Vec<String> = [&grant.token, &spare, &mcp]
        .into_iter()
        .map(|raw| crate::oauth::tokens::hash_secret(raw).as_hex().to_owned())
        .collect();
    let at_info = [
        hashes.as_slice(),
        &["SECRET-PAYLOAD-MARKER".to_owned(), "Private title".to_owned()],
    ]
    .concat();
    let mut report = String::new();
    for (needles, text, level) in [(&never[..], &everything, "any level"), (&at_info[..], &info_up, "info")] {
        for needle in needles {
            if text.contains(needle.as_str()) {
                let _ = writeln!(
                    report,
                    "found {} chars of secret material in the logs at {level}",
                    needle.len()
                );
            }
        }
    }
    assert!(report.is_empty(), "{report}\n{info_up}");
    // Ids and reason codes are what the stream logs.
    assert!(
        info_up.contains("too many open streams"),
        "the limit refusal is logged by reason only:\n{info_up}"
    );
    world.finish().await;
}
