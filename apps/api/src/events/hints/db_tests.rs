//! The hint triggers and the hint stream against PostgreSQL, through the real bot-token
//! middleware, over a real socket. Every test starts its own scratch database and server (skipped
//! without `OPENPR_TEST_DATABASE_URL`).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::print_stderr,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::doc_markdown,
    clippy::large_futures
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{Extension, Router, middleware as axum_middleware, routing::get};
use platform::auth::JwtManager;
use sea_orm::sqlx::postgres::PgListener;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::{ACCESS_CHANNEL, HINT_CHANNEL, HINTS_PATH, HintHub, HintLimits, HintTuning, TOKEN_CHANNEL, change_hints};
use crate::middleware::bot_auth::{bot_or_user_auth_middleware, sha256_hex};
use crate::routes::context::tenant_fixture::{
    Scratch, Tenant, claims_for, count, exec, scratch, seed_bot_user, seed_connection, seed_tenant, seed_work_item,
    state_for,
};
use crate::services::ai_task_fixture::seed_task;

const PATIENCE: Duration = Duration::from_secs(10);

macro_rules! scratch_or_skip {
    ($label:expr) => {
        match scratch($label).await {
            Some(scratch) => scratch,
            None => {
                eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
                return;
            }
        }
    };
}

// ---------------------------------------------------------------------------------------------
// SSE client
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Msg {
    event: Option<String>,
    data: Option<Value>,
    comment: bool,
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
            event: None,
            data: None,
            comment: false,
        };
        for line in text.lines() {
            if line.starts_with(':') {
                msg.comment = true;
            } else if let Some(event) = line.strip_prefix("event: ") {
                msg.event = Some(event.to_owned());
            } else if let Some(data) = line.strip_prefix("data: ") {
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

    /// The next message that is not a keep-alive comment.
    async fn message(&mut self, within: Duration) -> Msg {
        let deadline = Instant::now() + within;
        loop {
            match self.next(deadline.saturating_duration_since(Instant::now())).await {
                Next::Msg(msg) if msg.comment && msg.event.is_none() => {}
                Next::Msg(msg) => return msg,
                Next::Eof => panic!("the stream ended while a message was expected"),
                Next::Timeout => panic!("no message within {within:?}"),
            }
        }
    }

    async fn event(&mut self, name: &str, within: Duration) -> Value {
        let msg = self.message(within).await;
        assert_eq!(msg.event.as_deref(), Some(name), "expected {name}, got {msg:?}");
        msg.data.expect("an event carries data")
    }

    /// Whether anything but keep-alives arrives within `within`.
    async fn silent_for(&mut self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            match self.next(deadline.saturating_duration_since(Instant::now())).await {
                Next::Msg(msg) if msg.comment && msg.event.is_none() => {}
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

    /// Reads to the end of the stream: the messages that came before it.
    async fn read_to_end(&mut self, within: Duration) -> Vec<Msg> {
        let deadline = Instant::now() + within;
        let mut out = Vec::new();
        loop {
            match self.next(deadline.saturating_duration_since(Instant::now())).await {
                Next::Msg(msg) if msg.comment && msg.event.is_none() => {}
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

/// A bot with a known token.
struct Bot {
    id: Uuid,
    token: String,
}

struct Rig {
    scratch: Scratch,
    base: String,
    hub: Arc<HintHub>,
    http: reqwest::Client,
    tenant: Tenant,
    /// A second project of the same workspace.
    p2: Uuid,
    server: JoinHandle<()>,
}

impl Rig {
    async fn start(label: &str, limits: HintLimits, tuning: HintTuning) -> Option<Self> {
        let scratch = scratch(label).await.or_else(|| {
            eprintln!("SKIPPED (no database): set OPENPR_TEST_DATABASE_URL to run this test");
            None
        })?;
        let tenant = seed_tenant(&scratch.db, "hint").await;
        let p2 = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'P2', 'second project', $3)",
            vec![p2.into(), tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        let state = state_for(scratch.db.clone());
        let hub = HintHub::new(limits, tuning);
        let app = Router::new()
            .route(
                HINTS_PATH,
                get(change_hints)
                    .route_layer(axum_middleware::from_fn_with_state(
                        state.clone(),
                        bot_or_user_auth_middleware,
                    ))
                    .layer(Extension(Arc::clone(&hub))),
            )
            .with_state(state);
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
            tenant,
            p2,
            server,
        })
    }

    async fn standard(label: &str) -> Option<Self> {
        Self::start(label, HintLimits::from_runtime(), HintTuning::default()).await
    }

    fn db(&self) -> &DatabaseConnection {
        &self.scratch.db
    }

    fn p1(&self) -> Uuid {
        self.tenant.project_id
    }

    /// A workspace-wide bot (reads every project of its workspace).
    async fn bot(&self) -> Bot {
        self.insert_bot("workspace").await
    }

    /// A project-scoped bot with no default access: it reads only the projects it is granted.
    async fn scoped_bot(&self) -> Bot {
        self.insert_bot("projects").await
    }

    async fn insert_bot(&self, scope: &str) -> Bot {
        let id = seed_bot_user(self.db(), &self.tenant).await;
        let token = format!("opr_hint{}", Uuid::new_v4().simple());
        exec(
            self.db(),
            "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, project_scope) \
             VALUES ($1, $2, 'hint bot', $3, 'opr_test', $4)",
            vec![
                id.into(),
                self.tenant.workspace_id.into(),
                sha256_hex(&token).into(),
                scope.to_string().into(),
            ],
        )
        .await;
        Bot { id, token }
    }

    async fn grant(&self, bot: &Bot, project: Uuid) {
        exec(
            self.db(),
            "INSERT INTO bot_project_grants (bot_id, project_id, level) VALUES ($1, $2, 'read')",
            vec![bot.id.into(), project.into()],
        )
        .await;
    }

    /// A connected (runner) bot: reads exactly the projects it is connected to.
    async fn runner_bot(&self, connected_to: &[Uuid]) -> Bot {
        let bot = self.bot().await;
        for project in connected_to {
            seed_connection(self.db(), &self.tenant, *project, bot.id, true, "/work").await;
        }
        bot
    }

    async fn open(&self, bot: &Bot) -> Sse {
        let response = self
            .http
            .get(format!("{}{HINTS_PATH}", self.base))
            .bearer_auth(&bot.token)
            .send()
            .await
            .expect("the request is sent");
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers().get("content-type").and_then(|v| v.to_str().ok()),
            Some("text/event-stream")
        );
        assert_eq!(
            response
                .headers()
                .get("x-accel-buffering")
                .and_then(|v| v.to_str().ok()),
            Some("no")
        );
        let mut sse = Sse {
            response,
            buf: Vec::new(),
        };
        let hello = sse.event("hello", PATIENCE).await;
        assert_eq!(hello["bot_id"], bot.id.to_string());
        assert_eq!(hello["workspace_id"], self.tenant.workspace_id.to_string());
        sse
    }

    async fn open_status(&self, token: Option<&str>) -> reqwest::Response {
        let mut request = self.http.get(format!("{}{HINTS_PATH}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        request.send().await.expect("the request is sent")
    }

    async fn item(&self, title: &str) -> Uuid {
        seed_work_item(self.db(), &self.tenant, title).await
    }

    async fn item_in(&self, project: Uuid, title: &str) -> Uuid {
        let id = Uuid::new_v4();
        exec(
            self.db(),
            "INSERT INTO work_items (id, project_id, title, state, created_by) VALUES ($1, $2, $3, 'todo', $4)",
            vec![
                id.into(),
                project.into(),
                title.to_string().into(),
                self.tenant.member_id.into(),
            ],
        )
        .await;
        id
    }

    async fn touch(&self, item: Uuid) {
        exec(
            self.db(),
            "UPDATE work_items SET title = title || '!' WHERE id = $1",
            vec![item.into()],
        )
        .await;
    }

    async fn finish(self) {
        self.server.abort();
        self.scratch.drop_self().await;
    }
}

/// Waits until exactly `expected` streams hold a place (a closed connection is noticed by the
/// server a moment after the client sees the end).
async fn wait_for_streams(rig: &Rig, expected: usize) {
    let started = Instant::now();
    while rig.hub.open_streams() != expected {
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "expected {expected} open streams, found {}",
            rig.hub.open_streams()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------------------------
// Triggers
// ---------------------------------------------------------------------------------------------

struct Listen(PgListener);

impl Listen {
    async fn start(db: &DatabaseConnection) -> Self {
        let mut listener = PgListener::connect_with(db.get_postgres_connection_pool())
            .await
            .expect("a listener connects");
        listener
            .listen_all([HINT_CHANNEL, ACCESS_CHANNEL, TOKEN_CHANNEL])
            .await
            .expect("LISTEN");
        Self(listener)
    }

    /// Everything that arrives until `quiet` passes without a notification, as (channel, JSON).
    async fn drain(&mut self, quiet: Duration) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        while let Ok(received) = tokio::time::timeout(quiet, self.0.recv()).await {
            let notification = received.expect("the listener stays up");
            assert!(notification.payload().len() <= 300, "a payload is at most 300 bytes");
            out.push((
                notification.channel().to_owned(),
                serde_json::from_str(notification.payload()).expect("JSON payload"),
            ));
        }
        out
    }
}

fn hint(payload: Value) -> (String, Value) {
    (HINT_CHANNEL.to_owned(), payload)
}

fn access(bot: Uuid) -> (String, Value) {
    (ACCESS_CHANNEL.to_owned(), json!({"b": bot.to_string()}))
}

fn token_changed(bot: Uuid) -> (String, Value) {
    (TOKEN_CHANNEL.to_owned(), json!({"b": bot.to_string()}))
}

#[tokio::test]
async fn the_triggers_send_exactly_the_documented_payloads_after_commit_and_nothing_after_rollback() {
    let scratch = scratch_or_skip!("hints_triggers");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "trig").await;
    let ws = tenant.workspace_id.to_string();
    let project = tenant.project_id.to_string();
    let bot = seed_bot_user(db, &tenant).await;
    exec(
        db,
        "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix) \
         VALUES ($1, $2, 'trigger bot', $3, 'opr_test')",
        vec![bot.into(), tenant.workspace_id.into(), sha256_hex("opr_trigger").into()],
    )
    .await;
    let mut listen = Listen::start(db).await;
    // Setup above (inserts of users, bots) may have notified before the listener existed; start clean.
    listen.drain(Duration::from_millis(200)).await;

    // A task: insert fires, a lease-only update does not, a progress update does.
    let task = seed_task(db, &tenant, bot, "pending", 0, 0).await;
    let task_hint = hint(json!({"k": "task", "id": task.to_string(), "p": project, "w": ws, "b": bot.to_string()}));
    assert_eq!(listen.drain(Duration::from_millis(300)).await, vec![task_hint.clone()]);
    exec(
        db,
        "UPDATE ai_tasks SET lease_expires_at = now() + interval '2 hours' WHERE id = $1",
        vec![task.into()],
    )
    .await;
    assert!(
        listen.drain(Duration::from_millis(300)).await.is_empty(),
        "lease-only updates are silent"
    );
    for sql in [
        "UPDATE ai_tasks SET status = 'acknowledged' WHERE id = $1",
        "UPDATE ai_tasks SET plan = 'plan' WHERE id = $1",
        "UPDATE ai_tasks SET last_progress_at = now() WHERE id = $1",
        "UPDATE ai_tasks SET runner_phase = 'queued' WHERE id = $1",
        "UPDATE ai_tasks SET result = '{}'::jsonb WHERE id = $1",
        "UPDATE ai_tasks SET error_message = 'x' WHERE id = $1",
        "UPDATE ai_tasks SET attempts = attempts + 1 WHERE id = $1",
        "UPDATE ai_tasks SET acknowledged_at = now() WHERE id = $1",
    ] {
        exec(db, sql, vec![task.into()]).await;
        assert_eq!(
            listen.drain(Duration::from_millis(300)).await,
            vec![task_hint.clone()],
            "{sql}"
        );
    }

    // A work item: update and delete fire, an insert does not.
    let item = seed_work_item(db, &tenant, "first").await;
    assert!(
        listen.drain(Duration::from_millis(300)).await.is_empty(),
        "a new work item is not a hint"
    );
    let issue_hint = hint(json!({"k": "issue", "id": item.to_string(), "p": project, "w": ws}));
    exec(
        db,
        "UPDATE work_items SET title = 'second' WHERE id = $1",
        vec![item.into()],
    )
    .await;
    assert_eq!(listen.drain(Duration::from_millis(300)).await, vec![issue_hint.clone()]);

    // A comment: insert, update and delete name the work item.
    let comment = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO comments (id, work_item_id, author_id, body) VALUES ($1, $2, $3, 'hello')",
        vec![comment.into(), item.into(), tenant.member_id.into()],
    )
    .await;
    assert_eq!(listen.drain(Duration::from_millis(300)).await, vec![issue_hint.clone()]);
    exec(
        db,
        "UPDATE comments SET body = 'edited' WHERE id = $1",
        vec![comment.into()],
    )
    .await;
    assert_eq!(listen.drain(Duration::from_millis(300)).await, vec![issue_hint.clone()]);
    exec(db, "DELETE FROM comments WHERE id = $1", vec![comment.into()]).await;
    assert_eq!(listen.drain(Duration::from_millis(300)).await, vec![issue_hint.clone()]);

    exec(db, "DELETE FROM work_items WHERE id = $1", vec![item.into()]).await;
    assert_eq!(listen.drain(Duration::from_millis(300)).await, vec![issue_hint]);

    // Tool changes: a project's type key; a project type's capabilities (global, even unchanged).
    exec(
        db,
        "UPDATE projects SET type_key = type_key WHERE id = $1",
        vec![tenant.project_id.into()],
    )
    .await;
    assert_eq!(
        listen.drain(Duration::from_millis(300)).await,
        vec![hint(json!({"k": "tools", "p": project, "w": ws}))]
    );
    exec(
        db,
        "UPDATE projects SET name = 'renamed' WHERE id = $1",
        vec![tenant.project_id.into()],
    )
    .await;
    assert!(
        listen.drain(Duration::from_millis(300)).await.is_empty(),
        "only the type key is a tool change"
    );
    let types = count(db, "SELECT count(*) AS count FROM project_types", vec![]).await;
    assert!(types > 0, "the migrations seed project types");
    exec(
        db,
        "UPDATE project_types SET enabled_capabilities = enabled_capabilities WHERE key = (SELECT key FROM project_types LIMIT 1)",
        vec![],
    )
    .await;
    assert_eq!(
        listen.drain(Duration::from_millis(300)).await,
        vec![hint(json!({"k": "tools"}))],
        "a re-save fires without a distinct clause"
    );

    // Access: grants, connections and the bot's own scope.
    let p2 = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'TR2', 'p2', $3)",
        vec![p2.into(), tenant.workspace_id.into(), tenant.member_id.into()],
    )
    .await;
    listen.drain(Duration::from_millis(200)).await;
    for sql in [
        "INSERT INTO bot_project_grants (bot_id, project_id, level) VALUES ($1, $2, 'read')",
        "UPDATE bot_project_grants SET level = 'write' WHERE bot_id = $1 AND project_id = $2",
        "DELETE FROM bot_project_grants WHERE bot_id = $1 AND project_id = $2",
    ] {
        exec(db, sql, vec![bot.into(), p2.into()]).await;
        assert_eq!(
            listen.drain(Duration::from_millis(300)).await,
            vec![access(bot)],
            "{sql}"
        );
    }
    seed_connection(db, &tenant, p2, bot, true, "/work").await;
    let connected = listen.drain(Duration::from_millis(300)).await;
    assert_eq!(connected, vec![access(bot)], "a connection insert");
    for sql in [
        "UPDATE project_bot_connections SET enabled = false WHERE bot_id = $1 AND project_id = $2",
        "DELETE FROM project_bot_connections WHERE bot_id = $1 AND project_id = $2",
    ] {
        exec(db, sql, vec![bot.into(), p2.into()]).await;
        assert_eq!(
            listen.drain(Duration::from_millis(300)).await,
            vec![access(bot)],
            "{sql}"
        );
    }
    for sql in [
        "UPDATE workspace_bots SET project_scope = 'projects' WHERE id = $1",
        "UPDATE workspace_bots SET default_project_level = 'read' WHERE id = $1",
        "UPDATE workspace_bots SET permissions = '[\"read\",\"write\"]'::jsonb WHERE id = $1",
    ] {
        exec(db, sql, vec![bot.into()]).await;
        assert_eq!(
            listen.drain(Duration::from_millis(300)).await,
            vec![access(bot)],
            "{sql}"
        );
    }
    exec(
        db,
        "UPDATE workspace_bots SET last_used_at = now() WHERE id = $1",
        vec![bot.into()],
    )
    .await;
    assert!(
        listen.drain(Duration::from_millis(300)).await.is_empty(),
        "last_used_at is not a change"
    );

    // The token: rotation, state, expiry, deletion; the user's state.
    for sql in [
        "UPDATE workspace_bots SET is_active = false WHERE id = $1",
        "UPDATE workspace_bots SET is_active = true WHERE id = $1",
        "UPDATE workspace_bots SET expires_at = now() + interval '1 day' WHERE id = $1",
    ] {
        exec(db, sql, vec![bot.into()]).await;
        assert_eq!(
            listen.drain(Duration::from_millis(300)).await,
            vec![token_changed(bot)],
            "{sql}"
        );
    }
    exec(
        db,
        "UPDATE workspace_bots SET token_hash = $2 WHERE id = $1",
        vec![bot.into(), sha256_hex("opr_rotated").into()],
    )
    .await;
    assert_eq!(listen.drain(Duration::from_millis(300)).await, vec![token_changed(bot)]);
    exec(db, "UPDATE users SET is_active = false WHERE id = $1", vec![bot.into()]).await;
    assert_eq!(listen.drain(Duration::from_millis(300)).await, vec![token_changed(bot)]);
    exec(
        db,
        "UPDATE users SET is_active = false WHERE id = $1",
        vec![tenant.member_id.into()],
    )
    .await;
    assert!(
        listen.drain(Duration::from_millis(300)).await.is_empty(),
        "a person is not a bot"
    );
    exec(db, "DELETE FROM workspace_bots WHERE id = $1", vec![bot.into()]).await;
    assert_eq!(listen.drain(Duration::from_millis(300)).await, vec![token_changed(bot)]);

    // Commit-time delivery: nothing before the commit, exactly one notification after it, and
    // nothing at all after a rollback.
    let item = seed_work_item(db, &tenant, "tx").await;
    listen.drain(Duration::from_millis(200)).await;
    let tx = db.begin().await.expect("begin");
    exec(
        &tx,
        "UPDATE work_items SET title = 'in tx' WHERE id = $1",
        vec![item.into()],
    )
    .await;
    assert!(
        listen.drain(Duration::from_millis(500)).await.is_empty(),
        "no hint before the transaction commits"
    );
    tx.commit().await.expect("commit");
    assert_eq!(
        listen.drain(Duration::from_millis(500)).await,
        vec![hint(
            json!({"k": "issue", "id": item.to_string(), "p": project, "w": ws})
        )]
    );
    let tx = db.begin().await.expect("begin");
    exec(
        &tx,
        "UPDATE work_items SET title = 'rolled back' WHERE id = $1",
        vec![item.into()],
    )
    .await;
    tx.rollback().await.expect("rollback");
    assert!(
        listen.drain(Duration::from_millis(500)).await.is_empty(),
        "a rolled back transaction sends nothing"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn a_failing_hint_trigger_cannot_fail_the_domain_write() {
    let scratch = scratch_or_skip!("hints_trigger_failure");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "fail").await;
    let item = seed_work_item(db, &tenant, "survivor").await;
    // The trigger functions look the workspace up in `projects`; with the table renamed the
    // lookup raises inside the function, which must be swallowed.
    exec(db, "ALTER TABLE projects RENAME TO projects_renamed", vec![]).await;
    exec(
        db,
        "UPDATE work_items SET title = 'still written' WHERE id = $1",
        vec![item.into()],
    )
    .await;
    let title = db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT title FROM work_items WHERE id = $1",
            vec![item.into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<String>("", "title")
        .expect("title");
    assert_eq!(
        title, "still written",
        "the domain write succeeded although the hint failed"
    );
    let comment = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO comments (id, work_item_id, author_id, body) VALUES ($1, $2, $3, 'x')",
        vec![comment.into(), item.into(), tenant.member_id.into()],
    )
    .await;
    exec(db, "DELETE FROM work_items WHERE id = $1", vec![item.into()]).await;
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_migration_can_run_again_on_a_migrated_database() {
    let scratch = scratch_or_skip!("hints_migration_rerun");
    let sql = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../migrations/0132_change_hints.sql"
    ))
    .expect("the migration is readable");
    for run in 1..=2 {
        scratch
            .db
            .execute_unprepared(&sql)
            .await
            .unwrap_or_else(|error| panic!("run {run} of 0132: {error}"));
    }
    let triggers = count(
        &scratch.db,
        "SELECT count(*) AS count FROM pg_trigger WHERE tgname LIKE 'trg_sv_hint_%' AND NOT tgisinternal",
        vec![],
    )
    .await;
    assert_eq!(triggers, 13, "no trigger is created twice");
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// The stream
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn unauthenticated_callers_and_people_are_refused() {
    let Some(rig) = Rig::standard("hints_auth").await else {
        return;
    };

    // No credential, a bot token nobody holds, a malformed credential.
    for token in [None, Some("opr_nobody"), Some("garbage")] {
        let response = rig.open_status(token).await;
        let body: Value = response.json().await.expect("an envelope");
        assert_eq!(body["code"], 401, "{token:?}: {body}");
    }
    assert_eq!(rig.hub.open_streams(), 0, "a refused request holds no place");

    // A person with a valid access token: 403.
    let jwt = JwtManager::new("tenant-fixture-test-secret", 900, 3600)
        .issue_access_token(
            &rig.tenant.member_id.to_string(),
            &claims_for(rig.tenant.member_id).email,
        )
        .expect("a token is issued");
    let response = rig.open_status(Some(&jwt)).await;
    let body: Value = response.json().await.expect("an envelope");
    assert_eq!(body["code"], 403, "{body}");
    assert_eq!(body["message"], "change hints are available to bot tokens");
    assert_eq!(rig.hub.open_streams(), 0);
    rig.finish().await;
}

#[tokio::test]
async fn an_unscoped_bot_gets_task_inbox_issue_and_tools_hints_with_ids_only() {
    let Some(rig) = Rig::standard("hints_basic").await else {
        return;
    };
    let bot = rig.bot().await;
    let mut sse = rig.open(&bot).await;

    let task = seed_task(rig.db(), &rig.tenant, bot.id, "pending", 0, 0).await;
    let data = sse.event("task", PATIENCE).await;
    assert_eq!(data, json!({"id": task.to_string()}));
    assert_eq!(sse.event("inbox", PATIENCE).await, json!({}));

    let item = rig.item("an issue").await;
    assert!(
        sse.silent_for(Duration::from_millis(300)).await,
        "a new work item is not a hint"
    );
    rig.touch(item).await;
    assert_eq!(sse.event("issue", PATIENCE).await, json!({"id": item.to_string()}));
    exec(
        rig.db(),
        "INSERT INTO comments (id, work_item_id, author_id, body) VALUES (gen_random_uuid(), $1, $2, 'secret words')",
        vec![item.into(), rig.tenant.member_id.into()],
    )
    .await;
    assert_eq!(sse.event("issue", PATIENCE).await, json!({"id": item.to_string()}));

    exec(
        rig.db(),
        "UPDATE projects SET type_key = type_key WHERE id = $1",
        vec![rig.p1().into()],
    )
    .await;
    assert_eq!(sse.event("tools", PATIENCE).await, json!({}));
    exec(
        rig.db(),
        "UPDATE project_types SET enabled_capabilities = enabled_capabilities \
         WHERE key = (SELECT key FROM project_types LIMIT 1)",
        vec![],
    )
    .await;
    assert_eq!(sse.event("tools", PATIENCE).await, json!({}));

    // The keep-alive is a comment.
    rig.finish().await;
}

#[tokio::test]
async fn a_bot_never_receives_hints_for_projects_it_cannot_read_or_tasks_of_other_bots() {
    let Some(rig) = Rig::standard("hints_scoping").await else {
        return;
    };
    let p1 = rig.p1();
    let p2 = rig.p2;
    // Connected to p1 only.
    let bot = rig.runner_bot(&[p1]).await;
    let other = rig.bot().await;
    let mut sse = rig.open(&bot).await;
    // The connection rows above notified `access_changed` before the stream opened; none arrives now.
    assert!(sse.silent_for(Duration::from_millis(300)).await);

    let hidden = rig.item_in(p2, "hidden").await;
    let visible = rig.item_in(p1, "visible").await;
    rig.touch(hidden).await;
    exec(
        rig.db(),
        "INSERT INTO comments (id, work_item_id, author_id, body) VALUES (gen_random_uuid(), $1, $2, 'x')",
        vec![hidden.into(), rig.tenant.member_id.into()],
    )
    .await;
    assert!(
        sse.silent_for(Duration::from_millis(500)).await,
        "no hint for a project the bot is not connected to"
    );
    rig.touch(visible).await;
    assert_eq!(sse.event("issue", PATIENCE).await, json!({"id": visible.to_string()}));

    // A task of another bot, in a project this bot reads: not delivered. Its own task: delivered.
    seed_task(rig.db(), &rig.tenant, other.id, "pending", 0, 0).await;
    assert!(sse.silent_for(Duration::from_millis(500)).await, "another bot's task");
    let own = seed_task(rig.db(), &rig.tenant, bot.id, "pending", 0, 0).await;
    assert_eq!(sse.event("task", PATIENCE).await, json!({"id": own.to_string()}));
    assert_eq!(sse.event("inbox", PATIENCE).await, json!({}));

    // Tool changes: unreadable project no, readable project yes, global yes.
    exec(
        rig.db(),
        "UPDATE projects SET type_key = type_key WHERE id = $1",
        vec![p2.into()],
    )
    .await;
    assert!(
        sse.silent_for(Duration::from_millis(500)).await,
        "a tool change of an unreadable project"
    );
    exec(
        rig.db(),
        "UPDATE projects SET type_key = type_key WHERE id = $1",
        vec![p1.into()],
    )
    .await;
    assert_eq!(sse.event("tools", PATIENCE).await, json!({}));
    exec(
        rig.db(),
        "UPDATE project_types SET enabled_capabilities = enabled_capabilities \
         WHERE key = (SELECT key FROM project_types LIMIT 1)",
        vec![],
    )
    .await;
    assert_eq!(sse.event("tools", PATIENCE).await, json!({}));
    rig.finish().await;
}

#[tokio::test]
async fn a_project_scoped_bot_reads_only_its_grants_and_never_another_workspace() {
    let Some(rig) = Rig::standard("hints_scoped_and_ws").await else {
        return;
    };
    let bot = rig.scoped_bot().await;
    rig.grant(&bot, rig.p1()).await;
    // Another workspace with a bot-less project and an item.
    let other = seed_tenant(rig.db(), "w2").await;
    let foreign = seed_work_item(rig.db(), &other, "foreign").await;
    let mut sse = rig.open(&bot).await;
    assert!(sse.silent_for(Duration::from_millis(300)).await);

    let granted = rig.item("granted").await;
    let ungranted = rig.item_in(rig.p2, "ungranted").await;
    rig.touch(foreign).await;
    rig.touch(ungranted).await;
    assert!(sse.silent_for(Duration::from_millis(500)).await);
    rig.touch(granted).await;
    assert_eq!(sse.event("issue", PATIENCE).await, json!({"id": granted.to_string()}));
    // A task hint for a task of a bot in another workspace never arrives either.
    let foreign_bot = seed_bot_user(rig.db(), &other).await;
    seed_task(rig.db(), &other, foreign_bot, "pending", 0, 0).await;
    assert!(sse.silent_for(Duration::from_millis(500)).await);
    rig.finish().await;
}

#[tokio::test]
async fn delivery_stops_within_two_seconds_after_a_grant_is_removed_and_access_changed_is_emitted() {
    let Some(rig) = Rig::standard("hints_grant_removed").await else {
        return;
    };
    let bot = rig.scoped_bot().await;
    rig.grant(&bot, rig.p1()).await;
    let mut sse = rig.open(&bot).await;
    let item = rig.item("watched").await;
    rig.touch(item).await;
    assert_eq!(sse.event("issue", PATIENCE).await, json!({"id": item.to_string()}));

    exec(
        rig.db(),
        "DELETE FROM bot_project_grants WHERE bot_id = $1",
        vec![bot.id.into()],
    )
    .await;
    let removed = Instant::now();
    assert_eq!(sse.event("access_changed", Duration::from_secs(2)).await, json!({}));
    rig.touch(item).await;
    assert!(
        sse.silent_for(Duration::from_millis(800)).await,
        "no hint for the project after the grant is gone"
    );
    assert!(removed.elapsed() < Duration::from_secs(4));

    // Granted again: the change is reported and delivery resumes.
    rig.grant(&bot, rig.p1()).await;
    assert_eq!(sse.event("access_changed", Duration::from_secs(2)).await, json!({}));
    rig.touch(item).await;
    assert_eq!(sse.event("issue", PATIENCE).await, json!({"id": item.to_string()}));
    rig.finish().await;
}

#[tokio::test]
async fn an_access_change_without_a_notification_is_found_by_the_tick() {
    // LISTEN off: nothing is pushed by the database; the 3 s tick (shortened) finds the change
    // and the delivery filter still never works on a set older than its cache bound.
    let tuning = HintTuning {
        listen: false,
        tick: Duration::from_millis(300),
        readable_ttl: Duration::from_millis(200),
        ..HintTuning::default()
    };
    let Some(rig) = Rig::start("hints_tick_access", HintLimits::from_runtime(), tuning).await else {
        return;
    };
    let bot = rig.scoped_bot().await;
    let mut sse = rig.open(&bot).await;
    assert!(sse.silent_for(Duration::from_millis(500)).await);
    rig.grant(&bot, rig.p1()).await;
    assert_eq!(sse.event("access_changed", Duration::from_secs(3)).await, json!({}));
    rig.finish().await;
}

#[tokio::test]
async fn every_way_of_revoking_a_token_ends_the_stream_at_once_and_a_reconnect_is_refused() {
    let Some(rig) = Rig::standard("hints_revoked").await else {
        return;
    };
    let cases: [(&str, &str); 5] = [
        (
            "deactivated",
            "UPDATE workspace_bots SET is_active = false WHERE id = $1",
        ),
        (
            "rotated",
            "UPDATE workspace_bots SET token_hash = replace(id::text, '-', '') || replace(id::text, '-', '') WHERE id = $1",
        ),
        ("deleted", "DELETE FROM workspace_bots WHERE id = $1"),
        ("user disabled", "UPDATE users SET is_active = false WHERE id = $1"),
        (
            "expired by the clock",
            "UPDATE workspace_bots SET expires_at = now() - interval '1 second' WHERE id = $1",
        ),
    ];
    for (name, sql) in cases {
        let bot = rig.bot().await;
        let mut sse = rig.open(&bot).await;
        let other = rig.bot().await;
        let mut untouched = rig.open(&other).await;
        exec(rig.db(), sql, vec![bot.id.into()]).await;
        let started = Instant::now();
        let messages = sse.read_to_end(Duration::from_secs(3)).await;
        assert!(
            started.elapsed() < Duration::from_millis(2500),
            "{name}: revocation is pushed, not found by the 3 s tick ({:?})",
            started.elapsed()
        );
        let last = messages.last().unwrap_or_else(|| panic!("{name}: a closing frame"));
        assert_eq!(last.event.as_deref(), Some("revoked"), "{name}: {messages:?}");
        // The stream of another bot is untouched.
        assert!(untouched.silent_for(Duration::from_millis(300)).await, "{name}");
        wait_for_streams(&rig, 1).await;
        // A reconnect is refused by token validation.
        let response = rig.open_status(Some(&bot.token)).await;
        let body: Value = response.json().await.expect("an envelope");
        assert_eq!(body["code"], 401, "{name}: {body}");
        drop(untouched);
        // Let the other stream's permit go before the next case counts.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    rig.finish().await;
}

#[tokio::test]
async fn revocation_is_found_by_the_tick_when_listen_is_off() {
    // Default timing, LISTEN off, two streams of one bot (they share the token lookup): the worst
    // case is `tick` (2 s) plus a lookup up to `shared_ttl` (1 s) old, the contract's 3 s.
    let tuning = HintTuning {
        listen: false,
        ..HintTuning::default()
    };
    assert!(tuning.tick + tuning.shared_ttl <= Duration::from_secs(3));
    let Some(rig) = Rig::start("hints_revoked_tick", HintLimits::from_runtime(), tuning).await else {
        return;
    };
    let bot = rig.bot().await;
    let mut first = rig.open(&bot).await;
    let mut second = rig.open(&bot).await;
    exec(
        rig.db(),
        "UPDATE workspace_bots SET is_active = false WHERE id = $1",
        vec![bot.id.into()],
    )
    .await;
    let started = Instant::now();
    // 250 ms of scheduling slack on top of the 3 s bound.
    let bound = Duration::from_millis(3250);
    for sse in [&mut first, &mut second] {
        let messages = sse.read_to_end(bound).await;
        assert_eq!(
            messages.last().expect("closing frame").event.as_deref(),
            Some("revoked")
        );
    }
    assert!(
        started.elapsed() <= bound,
        "both streams end within the 3 s contract bound: {:?}",
        started.elapsed()
    );
    rig.finish().await;
}

#[tokio::test]
async fn a_stream_ends_with_revoked_expired_at_the_tokens_expiry() {
    let Some(rig) = Rig::standard("hints_expiry").await else {
        return;
    };
    let bot = rig.bot().await;
    exec(
        rig.db(),
        "UPDATE workspace_bots SET expires_at = now() + interval '2 seconds' WHERE id = $1",
        vec![bot.id.into()],
    )
    .await;
    let mut sse = rig.open(&bot).await;
    let started = Instant::now();
    let messages = sse.read_to_end(Duration::from_secs(6)).await;
    let last = messages.last().expect("a closing frame");
    assert_eq!(last.event.as_deref(), Some("revoked"));
    assert_eq!(last.data, Some(json!({"reason": "expired"})));
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    rig.finish().await;
}

#[tokio::test]
async fn no_hint_is_sent_before_the_transaction_commits() {
    let Some(rig) = Rig::standard("hints_commit_time").await else {
        return;
    };
    let bot = rig.bot().await;
    let item = rig.item("in a transaction").await;
    let mut sse = rig.open(&bot).await;

    let tx = rig.db().begin().await.expect("begin");
    exec(
        &tx,
        "UPDATE work_items SET title = 'changed' WHERE id = $1",
        vec![item.into()],
    )
    .await;
    exec(
        &tx,
        "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, attempts, max_attempts) \
         VALUES (gen_random_uuid(), $1, $2, 'manual', 'pending', 0, 3)",
        vec![rig.p1().into(), bot.id.into()],
    )
    .await;
    assert!(
        sse.silent_for(Duration::from_millis(700)).await,
        "the stream is silent while the transaction is open"
    );
    tx.commit().await.expect("commit");
    // The state the hint announces is readable by now.
    let mut seen = Vec::new();
    for _ in 0..3 {
        let msg = sse.message(PATIENCE).await;
        seen.push(msg.event.expect("an event"));
    }
    seen.sort();
    assert_eq!(seen, vec!["inbox", "issue", "task"]);

    let tx = rig.db().begin().await.expect("begin");
    exec(
        &tx,
        "UPDATE work_items SET title = 'rolled back' WHERE id = $1",
        vec![item.into()],
    )
    .await;
    tx.rollback().await.expect("rollback");
    assert!(
        sse.silent_for(Duration::from_millis(700)).await,
        "a rollback sends nothing"
    );
    rig.finish().await;
}

#[tokio::test]
async fn the_sixty_fifth_stream_of_a_bot_is_refused_and_a_closed_stream_frees_its_place() {
    let Some(rig) = Rig::standard("hints_per_bot_cap").await else {
        return;
    };
    let bot = rig.bot().await;
    let other = rig.bot().await;
    let mut open = Vec::new();
    for _ in 0..64 {
        open.push(rig.open(&bot).await);
    }
    let response = rig.open_status(Some(&bot.token)).await;
    assert_eq!(response.status(), 429);
    assert_eq!(
        response.headers().get("retry-after").and_then(|v| v.to_str().ok()),
        Some("30")
    );
    // Another bot is not affected.
    let _fine = rig.open(&other).await;
    // Closing one frees a place.
    drop(open.pop());
    let started = Instant::now();
    loop {
        let response = rig.open_status(Some(&bot.token)).await;
        if response.status() == 200 {
            break;
        }
        assert!(started.elapsed() < PATIENCE, "a released place never became free");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    rig.finish().await;
}

#[tokio::test]
async fn the_process_cap_refuses_streams_of_any_bot() {
    let limits = HintLimits {
        per_bot: 64,
        per_process: 2,
    };
    let Some(rig) = Rig::start("hints_process_cap", limits, HintTuning::default()).await else {
        return;
    };
    let (a, b, c) = (rig.bot().await, rig.bot().await, rig.bot().await);
    let _first = rig.open(&a).await;
    let _second = rig.open(&b).await;
    let response = rig.open_status(Some(&c.token)).await;
    assert_eq!(response.status(), 429);
    rig.finish().await;
}

#[tokio::test]
async fn a_server_shutdown_ends_open_streams_with_the_shutdown_event() {
    let Some(rig) = Rig::standard("hints_shutdown").await else {
        return;
    };
    let bot = rig.bot().await;
    let mut sse = rig.open(&bot).await;
    rig.hub.shutdown();
    let messages = sse.read_to_end(Duration::from_secs(3)).await;
    assert_eq!(
        messages.last().expect("closing frame").event.as_deref(),
        Some("shutdown")
    );
    rig.finish().await;
}

#[tokio::test]
async fn a_lost_listener_connection_tells_every_open_stream_to_re_read_with_lagged() {
    let Some(rig) = Rig::standard("hints_listener_lost").await else {
        return;
    };
    let (a, b) = (rig.bot().await, rig.bot().await);
    let mut first = rig.open(&a).await;
    let mut second = rig.open(&b).await;

    // Kill the listener's backend from outside: the connection drops without any error the
    // listener would report as a failure of its own, and what was notified meanwhile is lost.
    let killed = rig
        .db()
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(pg_terminate_backend(pid)) AS killed FROM pg_stat_activity \
             WHERE datname = current_database() AND pid <> pg_backend_pid() AND query ILIKE 'LISTEN%'",
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get::<i64>("", "killed")
        .expect("count");
    assert_eq!(killed, 1, "exactly the listener's backend was terminated");

    for sse in [&mut first, &mut second] {
        let messages = sse.read_to_end(Duration::from_secs(5)).await;
        assert_eq!(
            messages.last().expect("a closing frame").event.as_deref(),
            Some("lagged"),
            "{messages:?}"
        );
    }
    // The listener came back: a new stream gets hints again.
    let item = rig.item("after the loss").await;
    let mut fresh = rig.open(&a).await;
    rig.touch(item).await;
    assert_eq!(fresh.event("issue", PATIENCE).await, json!({"id": item.to_string()}));
    rig.finish().await;
}

#[tokio::test]
async fn a_bot_that_loses_read_permission_has_its_open_stream_ended_and_gets_no_more_hints() {
    let Some(rig) = Rig::standard("hints_read_removed").await else {
        return;
    };
    let bot = rig.bot().await;
    let mut sse = rig.open(&bot).await;
    let item = rig.item("watched").await;
    rig.touch(item).await;
    assert_eq!(sse.event("issue", PATIENCE).await, json!({"id": item.to_string()}));

    // `write` implies `read`: the stream stays open.
    exec(
        rig.db(),
        "UPDATE workspace_bots SET permissions = '[\"write\"]'::jsonb WHERE id = $1",
        vec![bot.id.into()],
    )
    .await;
    assert_eq!(sse.event("access_changed", PATIENCE).await, json!({}));
    rig.touch(item).await;
    assert_eq!(sse.event("issue", PATIENCE).await, json!({"id": item.to_string()}));

    exec(
        rig.db(),
        "UPDATE workspace_bots SET permissions = '[]'::jsonb WHERE id = $1",
        vec![bot.id.into()],
    )
    .await;
    let started = Instant::now();
    let messages = sse.read_to_end(Duration::from_secs(4)).await;
    assert!(
        started.elapsed() < Duration::from_millis(2500),
        "pushed, not found by the tick"
    );
    let messages: Vec<Msg> = messages
        .into_iter()
        .filter(|m| m.event.as_deref() != Some("access_changed"))
        .collect();
    let events: Vec<_> = messages.iter().filter_map(|m| m.event.as_deref()).collect();
    assert_eq!(events.last(), Some(&"revoked"), "{events:?}");
    assert!(
        !events.contains(&"issue") && !events.contains(&"tools"),
        "no hint after the permission is gone: {events:?}"
    );
    // The middleware refuses the same bot's next request, so a reconnect is refused.
    let response = rig.open_status(Some(&bot.token)).await;
    let body: Value = response.json().await.expect("an envelope");
    assert_eq!(body["code"], 403, "{body}");
    rig.finish().await;
}
