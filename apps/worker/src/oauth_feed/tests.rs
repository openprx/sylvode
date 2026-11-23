//! The feed writer, the retention and the whole path from a database write to a stream, against
//! PostgreSQL. Without `OPENPR_TEST_DATABASE_URL` these tests report a skip and pass.
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
    clippy::struct_field_names
)]

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use api::oauth::stream::{StreamLimits, StreamTuning, routes_with};
use api::oauth::tokens::{TokenKind, mint};
use platform::app::{AppState, FlowPermissionCacheSlot};
use platform::config::{AppConfig, OAuthConfig, Secret};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{BATCH, PRUNE_BATCH, WRITER_LOCK_KEY, process_batch, prune_daily, run, run_once, sweep_feed};

const URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";
const ISSUER: &str = "http://127.0.0.1:18097";
const JWT_SECRET: &str = "wp4-worker-test-jwt-secret-0123456789abcdef";

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    async fn drop_self(self) {
        let Self { db, name, admin_url } = self;
        drop(db);
        if let Ok(admin) = Database::connect(&admin_url).await {
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }
}

/// A scratch database with every migration applied, or `None` (and a visible note) without a server.
async fn scratch(label: &str) -> Option<Scratch> {
    let Ok(admin_url) = std::env::var(URL_ENV) else {
        eprintln!("SKIPPED (no database): set {URL_ENV} to run this test");
        return None;
    };
    let admin = Database::connect(&admin_url)
        .await
        .expect("the test database is usable");
    let name = format!("sylvode_worker_feed_{label}");
    admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .await
        .expect("reset");
    admin
        .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
        .await
        .expect("create");
    let (prefix, _) = admin_url.rsplit_once('/').expect("a database url");
    let db = Database::connect(&format!("{prefix}/{name}")).await.expect("connect");
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .expect("migrations")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    for path in files {
        let sql = std::fs::read_to_string(&path).expect("read");
        db.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
    }
    Some(Scratch { db, name, admin_url })
}

macro_rules! scratch_or_skip {
    ($label:expr) => {
        match scratch($label).await {
            Some(scratch) => scratch,
            None => return,
        }
    };
}

async fn exec(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("statement failed: {err}\n{sql}"));
}

#[derive(FromQueryResult)]
struct Count {
    n: i64,
}

async fn count(db: &impl ConnectionTrait, sql: &str) -> i64 {
    Count::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, vec![]))
        .one(db)
        .await
        .expect("count")
        .map_or(0, |row| row.n)
}

struct World {
    user: Uuid,
    workspace: Uuid,
    project: Uuid,
    agent: Uuid,
    issue: Uuid,
}

async fn seed(db: &impl ConnectionTrait) -> World {
    let (user, workspace, project, agent, issue) = (
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
        Uuid::new_v4(),
    );
    for (id, name, entity) in [(user, "member", "human"), (agent, "Triage agent", "bot")] {
        exec(
            db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active, entity_type) \
             VALUES ($1, $2, '!', $3, 'user', true, $4)",
            vec![
                id.into(),
                format!("{id}@worker.test").into(),
                name.into(),
                entity.into(),
            ],
        )
        .await;
    }
    exec(
        db,
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'feed', $3)",
        vec![workspace.into(), format!("ws-{workspace}").into(), user.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'member')",
        vec![workspace.into(), user.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'SEC', 'Security', $3)",
        vec![project.into(), workspace.into(), user.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO work_items (id, project_id, title, state, created_by) VALUES ($1, $2, 'Investigate WAF block spike', 'todo', $3)",
        vec![issue.into(), project.into(), user.into()],
    )
    .await;
    World {
        user,
        workspace,
        project,
        agent,
        issue,
    }
}

async fn insert_notification(db: &impl ConnectionTrait, world: &World, content: &str, project: Option<Uuid>) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO notifications (id, user_id, workspace_id, type, kind, payload, title, content, link, \
         related_issue_id, related_project_id, is_read) \
         VALUES ($1, $2, $3, 'assignment', 'assignment', '{}'::jsonb, 'notification.issueAssignedTitle', $4, '/x', $5, $6, false)",
        vec![
            id.into(),
            world.user.into(),
            world.workspace.into(),
            content.into(),
            project.map(|_| world.issue).into(),
            project.into(),
        ],
    )
    .await;
    id
}

async fn insert_task(db: &impl ConnectionTrait, world: &World, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, reference_type, reference_id, status) \
         VALUES ($1, $2, $3, 'issue_assigned', 'work_item', $4, $5)",
        vec![
            id.into(),
            world.project.into(),
            world.agent.into(),
            world.issue.into(),
            status.into(),
        ],
    )
    .await;
    id
}

#[derive(FromQueryResult)]
struct FeedRow {
    seq: i64,
    event_type: String,
    workspace_id: Uuid,
    project_id: Option<Uuid>,
    user_id: Option<Uuid>,
    subject: Value,
}

async fn feed(db: &impl ConnectionTrait) -> Vec<FeedRow> {
    FeedRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT seq, event_type, workspace_id, project_id, user_id, subject FROM oauth_event_feed ORDER BY seq",
        vec![],
    ))
    .all(db)
    .await
    .expect("feed")
}

fn subject_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value.as_object().expect("object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

// ---------------------------------------------------------------------------------------------
// The writer
// ---------------------------------------------------------------------------------------------

/// Two producers commit out of order: the one that commits later gets the higher `seq`, and the
/// one still open is not skipped over when it commits.
#[tokio::test]
async fn feed_order_equals_commit_order_with_concurrent_producers() {
    let scratch = scratch_or_skip!("order");
    let db = &scratch.db;
    let world = seed(db).await;

    let first = db.begin().await.expect("begin");
    let first_note = insert_notification(&first, &world, "first started, commits last", Some(world.project)).await;
    let second = db.begin().await.expect("begin");
    let second_note = insert_notification(&second, &world, "second started, commits first", Some(world.project)).await;
    second.commit().await.expect("commit");

    let report = run_once(db).await.expect("writer");
    assert_eq!((report.moved, report.dropped, report.contended), (1, 0, false));
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_event_feed").await,
        1,
        "the uncommitted producer's row is not visible, so only the committed one was written"
    );

    first.commit().await.expect("commit");
    let report = run_once(db).await.expect("writer");
    assert_eq!((report.moved, report.dropped), (1, 0));
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await, 0);

    let rows = feed(db).await;
    let ids: Vec<Value> = rows.iter().map(|row| row.subject["notification_id"].clone()).collect();
    assert_eq!(
        ids,
        [json!(second_note), json!(first_note)],
        "seq follows the order the rows became visible, not the order they were written"
    );
    assert!(rows[0].seq < rows[1].seq);
    // Many producers committing in a shuffled order: sequence numbers are strictly increasing and
    // the feed holds each event once.
    let mut open = Vec::new();
    for n in 0..8 {
        let tx = db.begin().await.expect("begin");
        insert_notification(&tx, &world, &format!("n{n}"), Some(world.project)).await;
        open.push(Some(tx));
    }
    for index in [5, 2, 7, 0, 3, 6, 1, 4] {
        open[index]
            .take()
            .expect("not yet committed")
            .commit()
            .await
            .expect("commit");
        run_once(db).await.expect("writer");
    }
    let rows = feed(db).await;
    assert_eq!(rows.len(), 10);
    assert!(rows.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    let unique: HashSet<String> = rows
        .iter()
        .map(|row| row.subject["notification_id"].to_string())
        .collect();
    assert_eq!(unique.len(), 10);
    scratch.drop_self().await;
}

/// All four event types, with exactly the frozen fields.
#[tokio::test]
async fn payload_shapes_for_all_four_types() {
    let scratch = scratch_or_skip!("shapes");
    let db = &scratch.db;
    let world = seed(db).await;

    let note = insert_notification(
        db,
        &world,
        "issue_assigned:Investigate WAF block spike",
        Some(world.project),
    )
    .await;
    exec(
        db,
        "UPDATE notifications SET is_read = true WHERE id = $1",
        vec![note.into()],
    )
    .await;
    let task = insert_task(db, &world, "pending").await;
    exec(
        db,
        "UPDATE ai_tasks SET status = 'completed', attempts = 1, result = '{\"summary\":\"all clear\"}'::jsonb WHERE id = $1",
        vec![task.into()],
    )
    .await;
    // An update that keeps the status writes nothing.
    exec(db, "UPDATE ai_tasks SET priority = 5 WHERE id = $1", vec![task.into()]).await;

    let report = run_once(db).await.expect("writer");
    assert_eq!(report.moved, 4);
    let rows = feed(db).await;
    let kinds: Vec<&str> = rows.iter().map(|row| row.event_type.as_str()).collect();
    assert_eq!(
        kinds,
        [
            "notification.created",
            "notification.read",
            "task.created",
            "task.status_changed"
        ]
    );
    for row in &rows {
        assert_eq!(row.workspace_id, world.workspace);
        assert_eq!(row.project_id, Some(world.project));
    }
    assert_eq!(rows[0].user_id, Some(world.user));
    assert_eq!(rows[1].user_id, Some(world.user));
    assert_eq!(rows[2].user_id, None);

    let created = &rows[0].subject;
    assert_eq!(
        subject_keys(created),
        [
            "content",
            "is_read",
            "kind",
            "notification_id",
            "notification_type",
            "related",
            "title_key"
        ]
    );
    assert_eq!(created["notification_type"], "assignment");
    assert_eq!(created["title_key"], "notification.issueAssignedTitle");
    assert_eq!(created["content"], "issue_assigned:Investigate WAF block spike");
    assert_eq!(
        subject_keys(&created["related"]),
        ["comment_id", "issue_id", "project_id"]
    );
    assert_eq!(created["related"]["issue_id"], json!(world.issue));
    assert_eq!(created["related"]["project_id"], json!(world.project));

    let read = &rows[1].subject;
    assert_eq!(subject_keys(read), ["is_read", "kind", "notification_id"]);
    assert_eq!(read["is_read"], json!(true));
    assert_eq!(read["notification_id"], json!(note));

    let task_created = &rows[2].subject;
    assert_eq!(
        subject_keys(task_created),
        [
            "agent",
            "attempts",
            "kind",
            "max_attempts",
            "reference",
            "status",
            "summary",
            "task_id",
            "task_type"
        ],
        "a created task has no previous status; its summary is read when the event is written, and by then the task had a result"
    );
    // The created event records the status at creation, not the status the task has now.
    assert_eq!(task_created["status"], "pending");
    assert_eq!(task_created["max_attempts"], 3);
    assert_eq!(
        task_created["agent"],
        json!({"id": world.agent, "name": "Triage agent"})
    );
    assert_eq!(
        task_created["reference"],
        json!({"type": "work_item", "id": world.issue, "identifier": "SEC-1", "title": "Investigate WAF block spike"})
    );

    let changed = &rows[3].subject;
    assert_eq!(
        subject_keys(changed),
        [
            "agent",
            "attempts",
            "kind",
            "max_attempts",
            "previous_status",
            "reference",
            "status",
            "summary",
            "task_id",
            "task_type"
        ]
    );
    assert_eq!(changed["status"], "completed");
    assert_eq!(changed["previous_status"], "pending");
    assert_eq!(changed["attempts"], 1);
    assert_eq!(changed["summary"], "all clear");
    assert!(changed.get("error").is_none());

    // A failed task carries its error, cut to 500 characters, and nothing else extra.
    let failing = insert_task(db, &world, "pending").await;
    exec(
        db,
        "UPDATE ai_tasks SET status = 'failed', error_message = repeat('e', 900) WHERE id = $1",
        vec![failing.into()],
    )
    .await;
    run_once(db).await.expect("writer");
    let rows = feed(db).await;
    let failed = &rows.last().expect("row").subject;
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["error"].as_str().map(|text| text.chars().count()), Some(500));
    assert!(!failed.to_string().contains("payload"));
    scratch.drop_self().await;
}

#[tokio::test]
async fn an_outbox_row_whose_source_is_gone_is_dropped() {
    let scratch = scratch_or_skip!("dropped");
    let db = &scratch.db;
    let world = seed(db).await;
    let doomed = insert_notification(db, &world, "deleted before the writer ran", Some(world.project)).await;
    insert_notification(db, &world, "kept", Some(world.project)).await;
    exec(db, "DELETE FROM notifications WHERE id = $1", vec![doomed.into()]).await;
    let report = run_once(db).await.expect("writer");
    assert_eq!((report.moved, report.dropped), (1, 1));
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await, 0);
    let rows = feed(db).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subject["content"], "kept");
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_notification_without_a_workspace_writes_no_event() {
    let scratch = scratch_or_skip!("no_workspace");
    let db = &scratch.db;
    let world = seed(db).await;
    exec(
        db,
        "INSERT INTO notifications (id, user_id, type, kind, payload, title, content, is_read) \
         VALUES ($1, $2, 'assignment', 'assignment', '{}'::jsonb, 't', 'c', false)",
        vec![Uuid::new_v4().into(), world.user.into()],
    )
    .await;
    assert_eq!(run_once(db).await.expect("writer").moved, 0);
    assert!(feed(db).await.is_empty());
    scratch.drop_self().await;
}

/// While one process holds the writer lock, another writes nothing; when it lets go, the rows are
/// written once. Run concurrently, two writers never write a row twice.
#[tokio::test]
async fn two_writers_cannot_both_assign_sequence_numbers() {
    let scratch = scratch_or_skip!("single_writer");
    let db = &scratch.db;
    let world = seed(db).await;
    for n in 0..20 {
        insert_notification(db, &world, &format!("n{n}"), Some(world.project)).await;
    }

    // Another connection holds the lock, as a second worker process in the middle of a batch would.
    let holder = db.begin().await.expect("begin");
    exec(
        &holder,
        "SELECT pg_advisory_xact_lock($1)",
        vec![WRITER_LOCK_KEY.into()],
    )
    .await;
    let blocked = run_once(db).await.expect("writer");
    assert!(blocked.contended, "the lock is taken");
    assert_eq!((blocked.moved, blocked.dropped), (0, 0));
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await, 20);
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_feed").await, 0);
    holder.commit().await.expect("commit");

    // Six writers at once: whoever gets the lock writes; nobody writes twice.
    let results = run_writers_concurrently(db, 6).await;
    let moved: usize = results.iter().map(|report| report.moved).sum();
    assert_eq!(
        moved, 20,
        "every row is written exactly once across the writers: {results:?}"
    );
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await, 0);
    let rows = feed(db).await;
    assert_eq!(rows.len(), 20);
    let unique: HashSet<String> = rows
        .iter()
        .map(|row| row.subject["notification_id"].to_string())
        .collect();
    assert_eq!(unique.len(), 20);
    assert!(rows.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    scratch.drop_self().await;
}

async fn run_writers_concurrently(db: &DatabaseConnection, writers: usize) -> Vec<super::TickReport> {
    let mut handles = Vec::new();
    for _ in 0..writers {
        let db = db.clone();
        handles.push(tokio::spawn(async move { run_once(&db).await.expect("writer") }));
    }
    let mut out = Vec::new();
    for handle in handles {
        out.push(handle.await.expect("task"));
    }
    out
}

/// A crash between reading the outbox and committing the feed rows leaves everything as it was;
/// the restart writes each event once.
#[tokio::test]
async fn a_crash_before_commit_neither_loses_nor_duplicates() {
    let scratch = scratch_or_skip!("crash");
    let db = &scratch.db;
    let world = seed(db).await;
    for n in 0..5 {
        insert_notification(db, &world, &format!("n{n}"), Some(world.project)).await;
    }
    insert_task(db, &world, "pending").await;

    // The writer does all of its work and the process dies before COMMIT.
    let tx = db.begin().await.expect("begin");
    let doomed = process_batch(&tx).await.expect("batch");
    assert_eq!(doomed.moved, 6);
    drop(tx);
    // (Dropping an open transaction rolls it back, which is what the server does for a dead client.)
    let mut settled = false;
    for _ in 0..50 {
        if count(db, "SELECT count(*) AS n FROM oauth_event_feed").await == 0
            && count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await == 6
        {
            settled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        settled,
        "nothing of the aborted batch is visible, and the outbox is intact"
    );

    // The restart.
    let report = run_once(db).await.expect("writer");
    assert_eq!((report.moved, report.dropped), (6, 0));
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await, 0);
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_feed").await, 6);
    // And again: nothing is written twice.
    let again = run_once(db).await.expect("writer");
    assert_eq!((again.moved, again.dropped), (0, 0));
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_feed").await, 6);
    let unique = count(db, "SELECT count(DISTINCT subject->>'notification_id') + count(DISTINCT subject->>'task_id') AS n FROM oauth_event_feed").await;
    assert_eq!(unique, 6);
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_large_backlog_is_written_in_batches_in_order() {
    let scratch = scratch_or_skip!("batches");
    let db = &scratch.db;
    let world = seed(db).await;
    let total = BATCH + 150;
    let note = insert_notification(db, &world, "source", Some(world.project)).await;
    exec(db, "DELETE FROM oauth_event_outbox", vec![]).await;
    exec(
        db,
        "INSERT INTO oauth_event_outbox (source, source_id, event_type, workspace_id, project_id, user_id) \
         SELECT 'notification', $5, 'notification.created', $1, $2, $3 FROM generate_series(1, $4::int)",
        vec![
            world.workspace.into(),
            world.project.into(),
            world.user.into(),
            i32::try_from(total).unwrap().into(),
            note.into(),
        ],
    )
    .await;
    let first = run_once(db).await.expect("writer");
    assert_eq!(first.moved, usize::try_from(BATCH).unwrap());
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await, 150);
    let second = run_once(db).await.expect("writer");
    assert_eq!(second.moved, 150);
    let rows = feed(db).await;
    assert_eq!(rows.len(), usize::try_from(total).unwrap());
    assert!(rows.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// Retention
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn the_sweep_deletes_old_feed_and_outbox_rows_in_bounded_batches() {
    let scratch = scratch_or_skip!("sweep");
    let db = &scratch.db;
    let world = seed(db).await;
    let old = i32::try_from(PRUNE_BATCH * 2 + 300).unwrap();
    exec(
        db,
        "INSERT INTO oauth_event_feed (event_type, workspace_id, project_id, user_id, subject, occurred_at, created_at) \
         SELECT 'task.created', $1, $2, NULL, '{}'::jsonb, now() - interval '9 days', now() - interval '8 days' \
         FROM generate_series(1, $3::int)",
        vec![world.workspace.into(), world.project.into(), old.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO oauth_event_feed (event_type, workspace_id, project_id, user_id, subject, occurred_at, created_at) \
         SELECT 'task.created', $1, $2, NULL, '{}'::jsonb, now(), now() - interval '6 days' FROM generate_series(1, 5)",
        vec![world.workspace.into(), world.project.into()],
    )
    .await;
    let note = insert_notification(db, &world, "source", Some(world.project)).await;
    exec(db, "DELETE FROM oauth_event_outbox", vec![]).await;
    exec(
        db,
        "INSERT INTO oauth_event_outbox (source, source_id, event_type, workspace_id, created_at) \
         VALUES ('notification', $1, 'notification.created', $2, now() - interval '8 days'), \
                ('notification', $1, 'notification.created', $2, now())",
        vec![note.into(), world.workspace.into()],
    )
    .await;

    // A writer working at the same time is not held up by the sweep.
    let writer_db = db.clone();
    let world_ids = (world.user, world.workspace, world.project);
    let concurrent = tokio::spawn(async move {
        let mut slowest = Duration::ZERO;
        for n in 0..20 {
            let started = Instant::now();
            exec(
                &writer_db,
                "INSERT INTO oauth_event_feed (event_type, workspace_id, project_id, user_id, subject, occurred_at) \
                 VALUES ('notification.created', $1, $2, $3, jsonb_build_object('n', $4::int), now())",
                vec![world_ids.1.into(), world_ids.2.into(), world_ids.0.into(), n.into()],
            )
            .await;
            slowest = slowest.max(started.elapsed());
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        slowest
    });
    let report = sweep_feed(db, 7).await.expect("sweep");
    let slowest = concurrent.await.expect("writer task");
    eprintln!("slowest concurrent feed insert during the sweep: {slowest:?}");
    assert!(
        slowest < Duration::from_secs(2),
        "inserts are not blocked by the sweep: {slowest:?}"
    );
    assert_eq!(report.feed, u64::try_from(old).unwrap());
    assert_eq!(report.outbox, 1);
    assert_eq!(
        count(
            db,
            "SELECT count(*) AS n FROM oauth_event_feed WHERE created_at < now() - interval '7 days'"
        )
        .await,
        0
    );
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_event_feed").await, 5 + 20);
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await,
        1,
        "only the old outbox row went"
    );
    // Nothing left to do: a second sweep deletes nothing.
    let again = sweep_feed(db, 7).await.expect("sweep");
    assert_eq!((again.feed, again.outbox), (0, 0));
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_daily_prune_removes_what_is_past_its_retention_and_nothing_else() {
    let scratch = scratch_or_skip!("prune");
    let db = &scratch.db;
    let world = seed(db).await;

    for (days, keep) in [(181, false), (179, true)] {
        exec(
            db,
            "INSERT INTO oauth_audit_events (id, occurred_at, event) VALUES ($1, now() - make_interval(days => $2::int), 'grant_revoked')",
            vec![Uuid::new_v4().into(), days.into()],
        )
        .await;
        let _ = keep;
    }
    for days in [91, 89] {
        exec(
            db,
            "INSERT INTO oauth_operation_logs (id, grant_id, client_id, user_id, workspace_id, surface, method, path, \
             business_code, outcome, duration_ms, request_id, created_at) \
             VALUES ($1, $2, 'ramflux', $3, $4, 'rest', 'GET', '/x', 200, 'ok', 1, $5, now() - make_interval(days => $6::int))",
            vec![
                Uuid::new_v4().into(),
                Uuid::new_v4().into(),
                world.user.into(),
                world.workspace.into(),
                Uuid::new_v4().into(),
                days.into(),
            ],
        )
        .await;
    }
    for expired_hours in [30, 12] {
        exec(
            db,
            "INSERT INTO oauth_authorization_requests (id, client_id, redirect_uri, state, code_challenge, requested_scopes, \
             resources, expires_at) \
             VALUES ($1, 'ramflux', 'http://127.0.0.1/cb', 'state-value-0123456789', 'c', ARRAY['projects:read'], ARRAY['api'], \
             now() - make_interval(hours => $2::int))",
            vec![Uuid::new_v4().into(), expired_hours.into()],
        )
        .await;
    }
    let grant = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO oauth_grants (id, client_id, user_id, workspace_id, scopes, resources, project_mode, user_session_version, expires_at) \
         VALUES ($1, 'ramflux', $2, $3, ARRAY['projects:read'], ARRAY['api'], 'all', 0, now() + interval '30 days')",
        vec![grant.into(), world.user.into(), world.workspace.into()],
    )
    .await;
    for (n, expired_hours) in [(0, 30), (1, 12)] {
        exec(
            db,
            "INSERT INTO oauth_authorization_codes (code_hash, grant_id, client_id, redirect_uri, code_challenge, resources, expires_at) \
             VALUES ($1, $2, 'ramflux', 'http://127.0.0.1/cb', 'c', ARRAY['api'], now() - make_interval(hours => $3::int))",
            vec![format!("{n:064}").into(), grant.into(), expired_hours.into()],
        )
        .await;
    }
    // A consumed refresh token is kept a week past its expiry, an access token likewise.
    for (n, expired_days, kind) in [(0, 8, "refresh"), (1, 6, "refresh"), (2, 9, "access"), (3, 1, "access")] {
        exec(
            db,
            "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, resource, scopes, expires_at) \
             VALUES ($1, $2, $3, $4, 'ramflux', CASE WHEN $3 = 'access' THEN 'api' END, ARRAY['projects:read'], \
             now() - make_interval(days => $5::int))",
            vec![
                Uuid::new_v4().into(),
                format!("{:064}", n + 10).into(),
                kind.into(),
                grant.into(),
                expired_days.into(),
            ],
        )
        .await;
    }
    // A live token never goes.
    exec(
        db,
        "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, resource, scopes, expires_at) \
         VALUES ($1, $2, 'access', $3, 'ramflux', 'api', ARRAY['projects:read'], now() + interval '10 minutes')",
        vec![Uuid::new_v4().into(), format!("{:064}", 99).into(), grant.into()],
    )
    .await;

    let report = prune_daily(db).await.expect("prune");
    assert_eq!(report.audit_events, 1);
    assert_eq!(report.operation_logs, 1);
    assert_eq!(report.authorization_requests, 1);
    assert_eq!(report.authorization_codes, 1);
    assert_eq!(report.tokens, 2);
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_audit_events").await, 1);
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_operation_logs").await, 1);
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_authorization_requests").await,
        1
    );
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_authorization_codes").await,
        1
    );
    assert_eq!(count(db, "SELECT count(*) AS n FROM oauth_tokens").await, 3);
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_tokens WHERE expires_at > now()").await,
        1,
        "the live token is untouched"
    );
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_grants").await,
        1,
        "grants are not pruned here"
    );
    scratch.drop_self().await;
}

/// Without `[oauth]` the writer does not run, but the outbox the triggers keep filling is kept in check.
#[tokio::test]
async fn without_configuration_only_the_retention_runs() {
    let scratch = scratch_or_skip!("disabled");
    let db = &scratch.db;
    let world = seed(db).await;
    insert_notification(db, &world, "recent", Some(world.project)).await;
    insert_notification(db, &world, "old", Some(world.project)).await;
    exec(
        db,
        "UPDATE oauth_event_outbox SET created_at = now() - interval '9 days' WHERE id = (SELECT min(id) FROM oauth_event_outbox)",
        vec![],
    )
    .await;
    let task = tokio::spawn(run(db.clone(), None));
    let mut pruned = false;
    for _ in 0..80 {
        if count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await == 1 {
            pruned = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    task.abort();
    assert!(pruned, "the stale outbox row was deleted");
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_event_outbox").await,
        1,
        "the recent one stays"
    );
    assert_eq!(
        count(db, "SELECT count(*) AS n FROM oauth_event_feed").await,
        0,
        "nothing is written to the feed"
    );
    scratch.drop_self().await;
}

// ---------------------------------------------------------------------------------------------
// The whole path
// ---------------------------------------------------------------------------------------------

struct Stream {
    response: reqwest::Response,
    buf: Vec<u8>,
}

impl Stream {
    /// The next `sv.event` or `sv.control` message as (event, id, data), or `None` on timeout.
    async fn next(&mut self, within: Duration) -> Option<(String, Option<String>, Value)> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(end) = self.buf.windows(2).position(|pair| pair == b"\n\n") {
                let raw: Vec<u8> = self.buf.drain(..end + 2).collect();
                let text = String::from_utf8(raw).expect("utf-8");
                let (mut event, mut id, mut data) = (String::new(), None, Value::Null);
                for line in text.lines() {
                    if let Some(value) = line.strip_prefix("event: ") {
                        value.clone_into(&mut event);
                    } else if let Some(value) = line.strip_prefix("id: ") {
                        id = Some(value.to_owned());
                    } else if let Some(value) = line.strip_prefix("data: ") {
                        data = serde_json::from_str(value).expect("json");
                    }
                }
                if event.is_empty() {
                    continue; // a heartbeat comment
                }
                return Some((event, id, data));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            match tokio::time::timeout(left, self.response.chunk()).await {
                Ok(Ok(Some(bytes))) => self.buf.extend_from_slice(&bytes),
                Ok(Ok(None) | Err(_)) => panic!("the stream ended"),
                Err(_) => return None,
            }
        }
    }

    async fn event(&mut self, within: Duration) -> (Option<String>, Value) {
        let (event, id, data) = self
            .next(within)
            .await
            .unwrap_or_else(|| panic!("nothing within {within:?}"));
        assert_eq!(event, "sv.event", "{data}");
        (id, data)
    }
}

/// A domain write becomes an event on a client's stream: the trigger, the writer task, the NOTIFY
/// wake-up, the delivery filter and the wire format, together. Prints the measured latencies.
#[tokio::test]
async fn a_domain_write_reaches_a_connected_client_within_the_bound() {
    let scratch = scratch_or_skip!("e2e");
    let db = &scratch.db;
    let world = seed(db).await;

    let config = Arc::new(OAuthConfig::with_defaults(ISSUER));
    let state = AppState {
        cfg: AppConfig {
            app_name: "worker-test".to_owned(),
            bind_addr: "127.0.0.1:0".to_owned(),
            database_url: Secret::new("postgres://unused"),
            jwt_secret: Secret::new(JWT_SECRET),
            jwt_access_ttl_seconds: 900,
            jwt_refresh_ttl_seconds: 3600,
            allow_insecure_cookies: false,
            collab_allowed_origins: Vec::new(),
        },
        db: db.clone(),
        flow_permission_cache: FlowPermissionCacheSlot::default(),
    };
    let (router, _hub) = routes_with(
        Arc::clone(&config),
        StreamLimits::from_config(&config),
        StreamTuning::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router.with_state(state)).await;
    });
    let writer = tokio::spawn(run(db.clone(), Some(Arc::clone(&config))));

    // A grant over the one project, with an API token.
    let grant = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO oauth_grants (id, client_id, user_id, workspace_id, scopes, resources, project_mode, \
         user_session_version, expires_at) \
         VALUES ($1, 'ramflux', $2, $3, ARRAY['notifications:read','tasks:read'], ARRAY['api'], 'selected', \
         (SELECT session_version FROM users WHERE id = $2), now() + interval '30 days')",
        vec![grant.into(), world.user.into(), world.workspace.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO oauth_grant_projects (grant_id, project_id) VALUES ($1, $2)",
        vec![grant.into(), world.project.into()],
    )
    .await;
    let minted = mint(TokenKind::Access).expect("token");
    exec(
        db,
        "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, resource, scopes, expires_at) \
         VALUES ($1, $2, 'access', $3, 'ramflux', 'api', ARRAY['notifications:read','tasks:read'], now() + interval '10 minutes')",
        vec![Uuid::new_v4().into(), minted.hash.as_hex().into(), grant.into()],
    )
    .await;

    let http = reqwest::Client::builder().no_proxy().build().expect("client");
    let response = http
        .get(format!("{base}/api/v1/oauth/events/stream"))
        .header("authorization", format!("Bearer {}", minted.raw.expose()))
        .send()
        .await
        .expect("connect");
    assert_eq!(response.status().as_u16(), 200);
    let mut stream = Stream {
        response,
        buf: Vec::new(),
    };
    let (event, _, hello) = stream.next(Duration::from_secs(10)).await.expect("hello");
    assert_eq!((event.as_str(), hello["type"].as_str()), ("sv.control", Some("hello")));

    // 1. A notification inserted by an ordinary write.
    let started = Instant::now();
    let note = insert_notification(
        db,
        &world,
        "issue_assigned:Investigate WAF block spike",
        Some(world.project),
    )
    .await;
    let (id, data) = stream.event(Duration::from_secs(10)).await;
    let notification_latency = started.elapsed();
    assert_eq!(data["type"], "notification.created");
    assert_eq!(data["subject"]["notification_id"], json!(note));
    assert_eq!(data["project"]["key"], "SEC");
    assert_eq!(data["id"].as_str(), id.as_deref());
    assert!(
        notification_latency < Duration::from_secs(5),
        "{notification_latency:?}"
    );

    // 2. A task state change by an ordinary update, after the task was created.
    let task = insert_task(db, &world, "pending").await;
    let (_, created) = stream.event(Duration::from_secs(10)).await;
    assert_eq!(created["type"], "task.created");
    assert_eq!(created["subject"]["task_id"], json!(task));
    let started = Instant::now();
    exec(
        db,
        "UPDATE ai_tasks SET status = 'processing', attempts = 1 WHERE id = $1",
        vec![task.into()],
    )
    .await;
    let (_, changed) = stream.event(Duration::from_secs(10)).await;
    let task_latency = started.elapsed();
    assert_eq!(changed["type"], "task.status_changed");
    assert_eq!(changed["subject"]["status"], "processing");
    assert_eq!(changed["subject"]["previous_status"], "pending");
    assert!(task_latency < Duration::from_secs(5), "{task_latency:?}");
    eprintln!("latency commit to delivery: notification {notification_latency:?}, task status change {task_latency:?}");

    // 3. A state change in a project the grant does not reach produces no event.
    let other_project = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'OTH', 'Other', $3)",
        vec![other_project.into(), world.workspace.into(), world.user.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status) VALUES ($1, $2, $3, 'issue_assigned', 'pending')",
        vec![Uuid::new_v4().into(), other_project.into(), world.agent.into()],
    )
    .await;
    assert!(
        stream.next(Duration::from_secs(3)).await.is_none(),
        "nothing arrives for the other project"
    );

    // 4. A burst arrives complete and in order.
    let burst_start = Instant::now();
    for n in 0..30 {
        insert_notification(db, &world, &format!("burst-{n}"), Some(world.project)).await;
    }
    for n in 0..30 {
        let (_, data) = stream.event(Duration::from_secs(10)).await;
        assert_eq!(data["subject"]["content"], format!("burst-{n}"));
    }
    eprintln!("burst of 30 delivered in {:?}", burst_start.elapsed());

    writer.abort();
    server.abort();
    scratch.drop_self().await;
}
