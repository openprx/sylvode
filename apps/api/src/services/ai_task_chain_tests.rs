//! The hop limit of bot-to-bot chains, enforced by the central dispatch guard.
//!
//! A task a human or an inbound alert starts is depth 0. A task a bot causes while it runs a task
//! (a mention or an assignment) is one deeper; beyond `[ai_tasks] max_chain_depth` or when the
//! bot is already in the chain for the same work item, no task is created.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_lines)]

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::HeaderMap,
};
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::json;
use uuid::Uuid;

use super::ai_task_fixture::seed_task;
use super::ai_task_reply::{CompletionInput, Outcome, route_completion};
use super::ai_task_service::{
    AiTaskRow, ChainRefusal, CreateAiTaskInput, DispatchOrigin, DispatchOutcome, create_ai_task, dispatch_ai_task,
};
use super::bot_connection_tests::{W, n, world};
use crate::routes::context::tenant_fixture::{claims_for, exec, seed_bot_user, seed_work_item};
use crate::routes::issue::{AssignmentTaskInput, queue_assignment_ai_task};
use crate::scratch_or_skip;

/// The configured limit the tests run under (the file's default).
const LIMIT: i32 = 3;

#[derive(FromQueryResult)]
#[allow(clippy::struct_field_names, reason = "the fields are the columns")]
struct Placement {
    id: Uuid,
    chain_root_id: Option<Uuid>,
    chain_parent_id: Option<Uuid>,
    chain_depth: i32,
}

async fn task_of(db: &DatabaseConnection, bot: Uuid) -> Option<Placement> {
    Placement::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, chain_root_id, chain_parent_id, chain_depth FROM ai_tasks \
         WHERE ai_participant_id = $1 ORDER BY created_at DESC LIMIT 1",
        vec![bot.into()],
    ))
    .one(db)
    .await
    .expect("query")
}

async fn tasks_of(db: &DatabaseConnection, bot: Uuid) -> i64 {
    n(
        db,
        "SELECT 1 FROM ai_tasks WHERE ai_participant_id = $1",
        vec![bot.into()],
    )
    .await
}

/// The bot's task is being run: the state in which its actions are caused by it.
async fn run(db: &DatabaseConnection, bot: Uuid) {
    exec(
        db,
        "UPDATE ai_tasks SET status = $2, started_at = now() WHERE ai_participant_id = $1",
        vec![bot.into(), "acknowledged".to_owned().into()],
    )
    .await;
}

async fn comment(w: &W, author: Uuid, issue: Uuid, mentions: Vec<Uuid>) -> Result<(), crate::error::ApiError> {
    crate::routes::comment::create_comment(
        State(w.state.clone()),
        Extension(claims_for(author)),
        None,
        HeaderMap::new(),
        Path(issue),
        Json(crate::routes::comment::CreateCommentRequest {
            content: Some("please look at this".to_owned()),
            body: None,
            mentions: Some(mentions),
        }),
    )
    .await
    .map(|_| ())
}

struct Cast {
    w: W,
    issue: Uuid,
    bots: Vec<Uuid>,
}

async fn cast(db: &DatabaseConnection, tag: &str, bots: usize) -> Cast {
    let w = world(db, tag).await;
    exec(db, "UPDATE projects SET type_key = 'code_project'", vec![]).await;
    let issue = seed_work_item(db, &w.tenant, "Chain").await;
    let mut ids = Vec::new();
    for _ in 0..bots {
        ids.push(seed_bot_user(db, &w.tenant).await);
    }
    Cast { w, issue, bots: ids }
}

fn refusals(db: &DatabaseConnection) -> impl std::future::Future<Output = i64> + '_ {
    n(
        db,
        "SELECT 1 FROM business_events WHERE event_type = 'ai_task.chain_refused'",
        vec![],
    )
}

#[tokio::test]
async fn a_chain_runs_to_the_limit_and_the_next_hop_is_refused() {
    let scratch = scratch_or_skip!("chain_limit");
    let db = &scratch.db;
    let c = cast(db, "c1", 5).await;
    let owner = c.w.owner;

    comment(&c.w, owner, c.issue, vec![c.bots[0]])
        .await
        .expect("human mention");
    let root = task_of(db, c.bots[0]).await.expect("the first bot's task");
    assert_eq!(
        (root.chain_depth, root.chain_root_id, root.chain_parent_id),
        (0, Some(root.id), None)
    );

    let mut parent = root;
    for hop in 1..=LIMIT {
        let author = c.bots[usize::try_from(hop - 1).expect("small")];
        let next = c.bots[usize::try_from(hop).expect("small")];
        run(db, author).await;
        comment(&c.w, author, c.issue, vec![next]).await.expect("a bot mention");
        let task = task_of(db, next)
            .await
            .unwrap_or_else(|| panic!("hop {hop} is within the limit"));
        assert_eq!(task.chain_depth, hop);
        assert_eq!(task.chain_root_id, parent.chain_root_id, "one chain");
        assert_eq!(task.chain_parent_id, Some(parent.id));
        parent = task;
    }
    assert_eq!(refusals(db).await, 0);

    // bots[3] is at depth 3 and mentions bots[4]: depth 4 is beyond the limit.
    run(db, c.bots[3]).await;
    comment(&c.w, c.bots[3], c.issue, vec![c.bots[4]])
        .await
        .expect("the comment itself is still written");
    assert_eq!(tasks_of(db, c.bots[4]).await, 0, "the refused hop created no task");
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM comments WHERE work_item_id = $1",
            vec![c.issue.into()]
        )
        .await,
        5,
        "all five comments exist"
    );
    assert_eq!(refusals(db).await, 1, "the refusal is an audit event");
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM ai_task_events WHERE task_id = $1 AND event_type = 'chain_refused' \
             AND payload ->> 'reason' = 'chain_too_deep'",
            vec![parent.id.into()],
        )
        .await,
        1,
        "the acting task shows why"
    );

    // A person mentioning a bot on the same item starts a chain of its own.
    comment(&c.w, owner, c.issue, vec![c.bots[4]])
        .await
        .expect("human mention");
    let fresh = task_of(db, c.bots[4]).await.expect("a human can still start a bot");
    assert_eq!((fresh.chain_depth, fresh.chain_root_id), (0, Some(fresh.id)));
    // Including a bot that is already in the old chain.
    comment(&c.w, owner, c.issue, vec![c.bots[1]])
        .await
        .expect("human mention");
    let again = task_of(db, c.bots[1]).await.expect("task");
    assert_eq!((again.chain_depth, again.chain_root_id), (0, Some(again.id)));
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_same_bot_twice_in_a_chain_for_one_work_item_is_refused() {
    let scratch = scratch_or_skip!("chain_cycle");
    let db = &scratch.db;
    let c = cast(db, "c2", 2).await;
    let (a, b) = (c.bots[0], c.bots[1]);
    let other_issue = seed_work_item(db, &c.w.tenant, "Elsewhere").await;

    comment(&c.w, c.w.owner, c.issue, vec![a]).await.expect("human");
    run(db, a).await;
    comment(&c.w, a, c.issue, vec![b]).await.expect("a mentions b");
    run(db, b).await;
    assert_eq!(tasks_of(db, b).await, 1);

    // b calls a back on the same item: a cycle, although the depth is within the limit.
    comment(&c.w, b, c.issue, vec![a])
        .await
        .expect("the comment is written");
    assert_eq!(tasks_of(db, a).await, 1, "no second task for a in its own chain");
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM business_events WHERE event_type = 'ai_task.chain_refused' \
             AND payload ->> 'reason' = 'chain_cycle'",
            vec![],
        )
        .await,
        1
    );

    // The same bot on another work item is another piece of work.
    comment(&c.w, b, other_issue, vec![a]).await.expect("other item");
    let task = task_of(db, a).await.expect("task");
    assert_eq!(tasks_of(db, a).await, 2);
    assert_eq!(task.chain_depth, 2);
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_bot_mentioning_itself_creates_nothing() {
    let scratch = scratch_or_skip!("chain_self");
    let db = &scratch.db;
    let c = cast(db, "c3", 1).await;
    let a = c.bots[0];
    comment(&c.w, c.w.owner, c.issue, vec![a]).await.expect("human");
    run(db, a).await;
    comment(&c.w, a, c.issue, vec![a]).await.expect("self mention");
    assert_eq!(tasks_of(db, a).await, 1, "a bot does not trigger itself");
    assert_eq!(refusals(db).await, 0, "and that is not a chain refusal either");
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_system_reply_comment_creates_nothing() {
    let scratch = scratch_or_skip!("chain_reply");
    let db = &scratch.db;
    let c = cast(db, "c4", 2).await;
    let (a, b) = (c.bots[0], c.bots[1]);
    comment(&c.w, c.w.owner, c.issue, vec![a]).await.expect("human");
    run(db, a).await;
    let task = AiTaskRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, project_id, ai_participant_id, task_type, reference_type, reference_id, status, priority, \
                payload, result, error_message, idempotency_key, attempts, max_attempts, next_retry_at, \
                started_at, completed_at, created_at, updated_at \
         FROM ai_tasks WHERE ai_participant_id = $1",
        vec![a.into()],
    ))
    .one(db)
    .await
    .expect("query")
    .expect("a task row");
    let txn = db.begin().await.expect("begin");
    route_completion(
        &c.w.state,
        &txn,
        &task,
        &CompletionInput {
            outcome: Outcome::Done,
            summary: format!("handing this on to @{b}"),
            artifacts: Vec::new(),
            vote: None,
            vote_reason: None,
        },
    )
    .await
    .expect("the reply is routed");
    txn.commit().await.expect("commit");
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM comments WHERE work_item_id = $1",
            vec![c.issue.into()]
        )
        .await,
        2
    );
    assert_eq!(tasks_of(db, b).await, 0, "the reply starts nothing, whatever it says");
    scratch.drop_self().await;
}

#[tokio::test]
async fn an_assignment_by_a_bot_is_a_hop_and_a_refused_one_keeps_the_assignment() {
    let scratch = scratch_or_skip!("chain_assign");
    let db = &scratch.db;
    let c = cast(db, "c5", 2).await;
    let (a, b) = (c.bots[0], c.bots[1]);
    comment(&c.w, c.w.owner, c.issue, vec![a]).await.expect("human");
    run(db, a).await;
    let assign = |key: &str| AssignmentTaskInput {
        project_id: c.w.tenant.project_id,
        issue_id: c.issue,
        actor_id: a,
        assignee_id: b,
        trigger: "issue.assigned",
        idempotency_key: key.to_owned(),
    };
    let outcome = queue_assignment_ai_task(db, assign("assign-1"))
        .await
        .expect("assignment");
    assert_eq!(outcome.ai_task_bots, vec![b]);
    assert_eq!(task_of(db, b).await.expect("task").chain_depth, 1);

    // Deepen the chain to the limit by hand, then assign once more.
    exec(
        db,
        "UPDATE ai_tasks SET chain_depth = $2 WHERE ai_participant_id = $1",
        vec![a.into(), LIMIT.into()],
    )
    .await;
    exec(db, "DELETE FROM ai_tasks WHERE ai_participant_id = $1", vec![b.into()]).await;
    let outcome = queue_assignment_ai_task(db, assign("assign-2"))
        .await
        .expect("not an error");
    assert_eq!(tasks_of(db, b).await, 0, "the hop is refused");
    assert_eq!(
        outcome.ai_task_bots,
        vec![b],
        "the bot stays listed so that no webhook delivery starts it with a bot context"
    );
    assert_eq!(refusals(db).await, 1);
    scratch.drop_self().await;
}

#[tokio::test]
async fn the_guard_places_every_origin() {
    let scratch = scratch_or_skip!("chain_origin");
    let db = &scratch.db;
    let c = cast(db, "c6", 3).await;
    let (deep, target, other) = (c.bots[0], c.bots[1], c.bots[2]);
    let w = &c.w;
    // `deep` runs a task at the limit, started by a human.
    let seeded = seed_task(db, &w.tenant, deep, "processing", 1, 5).await;
    exec(
        db,
        "UPDATE ai_tasks SET chain_depth = $2, chain_root_id = id WHERE id = $1",
        vec![seeded.into(), LIMIT.into()],
    )
    .await;
    let input = |origin: DispatchOrigin, actor: Option<Uuid>, bot: Uuid, key: &str| CreateAiTaskInput {
        origin,
        project_id: w.tenant.project_id,
        ai_participant_id: bot,
        actor_id: actor,
        task_type: "manual".to_owned(),
        reference_type: None,
        reference_id: None,
        priority: 0,
        payload: json!({}),
        idempotency_key: Some(key.to_owned()),
        max_attempts: 3,
    };
    let alert = DispatchOrigin::InboundRule {
        rule_id: Uuid::new_v4(),
        inbound_event_id: Uuid::new_v4(),
        source_id: Uuid::new_v4(),
    };

    // An alert starts a chain at depth 0, whoever the recorded actor is.
    let created = dispatch_ai_task(db, input(alert, Some(deep), target, "alert"))
        .await
        .expect("alert");
    assert!(matches!(created, DispatchOutcome::Created(_)));
    let row = task_of(db, target).await.expect("task");
    assert_eq!((row.chain_depth, row.chain_root_id), (0, Some(row.id)));

    // A person (Request or System) starts one too.
    for (origin, key) in [
        (DispatchOrigin::Request, "human-request"),
        (DispatchOrigin::System, "human-system"),
    ] {
        dispatch_ai_task(db, input(origin, Some(w.owner), other, key))
            .await
            .expect("human");
        let row = task_of(db, other).await.expect("task");
        assert_eq!(row.chain_depth, 0, "{key}");
    }

    // A bot at the limit may not cause a task through either origin, nor through the wrapper.
    for (origin, key) in [
        (DispatchOrigin::Request, "bot-request"),
        (DispatchOrigin::System, "bot-system"),
    ] {
        let outcome = dispatch_ai_task(db, input(origin, Some(deep), other, key))
            .await
            .expect("outcome");
        assert!(
            matches!(
                outcome,
                DispatchOutcome::ChainRefused(ChainRefusal::TooDeep { depth: 4, max: 3 })
            ),
            "{key}: {outcome:?}"
        );
    }
    assert!(
        create_ai_task(db, input(DispatchOrigin::Request, Some(deep), other, "bot-wrapper"))
            .await
            .is_err(),
        "a caller that cannot handle the outcome gets an error"
    );
    assert_eq!(
        tasks_of(db, other).await,
        2,
        "only the two human tasks of that bot exist"
    );

    // A bot with no active task is not executing anything: it starts a chain.
    exec(
        db,
        "UPDATE ai_tasks SET status = 'completed' WHERE id = $1",
        vec![seeded.into()],
    )
    .await;
    dispatch_ai_task(db, input(DispatchOrigin::Request, Some(deep), target, "idle-bot"))
        .await
        .expect("idle bot");
    assert_eq!(task_of(db, target).await.expect("task").chain_depth, 0);
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_task_from_before_the_migration_is_its_own_root_at_depth_zero() {
    let scratch = scratch_or_skip!("chain_legacy");
    let db = &scratch.db;
    let c = cast(db, "c7", 2).await;
    let (a, b) = (c.bots[0], c.bots[1]);
    // A row that predates the columns has no root.
    let legacy = seed_task(db, &c.w.tenant, a, "processing", 1, 5).await;
    exec(
        db,
        "UPDATE ai_tasks SET chain_root_id = NULL WHERE id = $1",
        vec![legacy.into()],
    )
    .await;
    let migration = include_str!("../../../../migrations/0113_ai_task_chain.sql");
    for _ in 0..2 {
        sea_orm::ConnectionTrait::execute_unprepared(db, migration)
            .await
            .expect("the migration re-runs");
    }
    let row = Placement::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, chain_root_id, chain_parent_id, chain_depth FROM ai_tasks WHERE id = $1",
        vec![legacy.into()],
    ))
    .one(db)
    .await
    .expect("query")
    .expect("row");
    assert_eq!(
        (row.chain_depth, row.chain_root_id, row.chain_parent_id),
        (0, Some(legacy), None)
    );

    // Such a task heads a chain for the tasks its bot causes.
    comment(&c.w, a, c.issue, vec![b]).await.expect("mention");
    let child = task_of(db, b).await.expect("task");
    assert_eq!(
        (child.chain_depth, child.chain_root_id, child.chain_parent_id),
        (1, Some(legacy), Some(legacy))
    );
    scratch.drop_self().await;
}

/// Every task is inserted by the guard: no other non-test source of the API or the worker writes
/// the table's rows.
#[test]
fn only_the_guard_inserts_tasks() {
    fn walk(dir: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read dir") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                // Test modules are declared `#[cfg(test)]` in their parent: by file name.
                let stem = path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if stem.ends_with("tests") || stem.ends_with("_fixture") {
                    continue;
                }
                let text = std::fs::read_to_string(&path).expect("read source");
                let production = text.split("#[cfg(test)]").next().unwrap_or_default();
                if production.contains("INSERT INTO ai_tasks") {
                    found.push(path.file_name().expect("name").to_string_lossy().into_owned());
                }
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut found = Vec::new();
    walk(&root.join("src"), &mut found);
    walk(&root.join("../worker/src"), &mut found);
    assert_eq!(
        found,
        vec!["ai_task_service.rs".to_owned()],
        "a second writer of ai_tasks"
    );
}
