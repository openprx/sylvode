//! A project's tasks reach a bot only while that project has an enabled connection for it: at
//! task creation, at pick-up and at delivery. A disabled connection, a connection that was removed
//! while the bot stays connected elsewhere, and a project that was never connected all stop
//! tasks, however the task was started.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_lines)]

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::HeaderMap,
};
use serde_json::json;
use uuid::Uuid;

use super::ai_task_service::{CreateAiTaskInput, DispatchOrigin, create_ai_task};
use super::bot_connection_tests::{
    W, existing_bot, expect_connect, n, new_bot, remove, second_project, set_enabled, world,
};
use crate::error::ApiError;
use crate::routes::context::tenant_fixture::{claims_for, exec, seed_bot_user, seed_work_item};
use crate::routes::issue::{AssignmentTaskInput, queue_assignment_ai_task};
use crate::scratch_or_skip;

fn task_input(origin: DispatchOrigin, project: Uuid, bot: Uuid, key: &str) -> CreateAiTaskInput {
    CreateAiTaskInput {
        origin,
        project_id: project,
        ai_participant_id: bot,
        actor_id: None,
        task_type: "manual".to_owned(),
        reference_type: None,
        reference_id: None,
        priority: 0,
        payload: json!({}),
        idempotency_key: Some(key.to_owned()),
        max_attempts: 3,
    }
}

struct Cast {
    w: W,
    /// The bot, connected to `p1` and `p2`.
    bot: Uuid,
    p1: Uuid,
    p2: Uuid,
    /// A project of the workspace the bot was never connected to.
    p3: Uuid,
    v1: i32,
}

async fn cast(db: &sea_orm::DatabaseConnection, tag: &str) -> Cast {
    let w = world(db, tag).await;
    let p1 = w.tenant.project_id;
    let p2 = second_project(db, &w, &format!("G{tag}2")).await;
    let p3 = second_project(db, &w, &format!("G{tag}3")).await;
    let first = expect_connect(&w, p1, new_bot("Gate bot")).await;
    let bot = first.connection.bot.id;
    expect_connect(&w, p2, existing_bot(bot)).await;
    Cast {
        v1: first.connection.version,
        w,
        bot,
        p1,
        p2,
        p3,
    }
}

fn refused_with(result: Result<Option<super::ai_task_service::AiTaskRow>, ApiError>) -> String {
    match result {
        Err(ApiError::Conflict(message)) => message,
        other => panic!("expected the business refusal, got {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn a_task_is_created_only_for_an_enabled_connection_of_the_project() {
    let scratch = scratch_or_skip!("gate_create");
    let db = &scratch.db;
    let c = cast(db, "c1").await;
    let origins = [
        DispatchOrigin::Request,
        DispatchOrigin::System,
        DispatchOrigin::InboundRule {
            rule_id: Uuid::new_v4(),
            inbound_event_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
        },
    ];

    // Enabled: every origin may create a task.
    for (index, origin) in origins.iter().enumerate() {
        let task = create_ai_task(db, task_input(*origin, c.p1, c.bot, &format!("enabled-{index}")))
            .await
            .expect("an enabled connection takes tasks");
        assert!(task.is_some());
    }

    // A project that was never connected.
    for (index, origin) in origins.iter().enumerate() {
        let message =
            refused_with(create_ai_task(db, task_input(*origin, c.p3, c.bot, &format!("never-{index}"))).await);
        assert_eq!(message, "the bot is not connected to this project", "origin {index}");
    }

    // A disabled connection, while the bot keeps serving the other project.
    set_enabled(&c.w, c.p1, c.v1, false).await.expect("disable");
    for (index, origin) in origins.iter().enumerate() {
        let message =
            refused_with(create_ai_task(db, task_input(*origin, c.p1, c.bot, &format!("disabled-{index}"))).await);
        assert_eq!(
            message, "the bot's connection to this project is disabled",
            "origin {index}"
        );
    }
    assert!(
        create_ai_task(db, task_input(DispatchOrigin::Request, c.p2, c.bot, "other-project"))
            .await
            .expect("the other project is unaffected")
            .is_some()
    );

    // A removed connection while the bot stays connected to another project.
    let current = n(
        db,
        "SELECT 1 FROM project_bot_connections WHERE project_id = $1",
        vec![c.p1.into()],
    )
    .await;
    assert_eq!(current, 1);
    let version: i32 = {
        use sea_orm::{ConnectionTrait, DbBackend, Statement};
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT version FROM project_bot_connections WHERE project_id = $1",
            vec![c.p1.into()],
        ))
        .await
        .expect("query")
        .expect("row")
        .try_get("", "version")
        .expect("version")
    };
    remove(&c.w, c.p1, version).await.expect("remove");
    let message = refused_with(create_ai_task(db, task_input(DispatchOrigin::Request, c.p1, c.bot, "removed")).await);
    assert_eq!(message, "the bot is not connected to this project");
    assert!(
        create_ai_task(db, task_input(DispatchOrigin::Request, c.p2, c.bot, "still-p2"))
            .await
            .expect("the bot still serves the other project")
            .is_some()
    );

    // A bot no connection made is not a runner bot: its tasks are unchanged.
    let plain = seed_bot_user(db, &c.w.tenant).await;
    assert!(
        create_ai_task(db, task_input(DispatchOrigin::Request, c.p3, plain, "plain"))
            .await
            .expect("an ordinary bot keeps working")
            .is_some()
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn assigning_a_bot_that_is_not_connected_here_creates_no_task_and_no_error() {
    let scratch = scratch_or_skip!("gate_assign");
    let db = &scratch.db;
    let c = cast(db, "a1").await;
    let issue = seed_work_item(db, &c.w.tenant, "Fix it").await;
    let input = |project: Uuid, key: &str| AssignmentTaskInput {
        project_id: project,
        issue_id: issue,
        assignee_id: c.bot,
        actor_id: c.w.owner,
        trigger: "issue.assigned",
        idempotency_key: key.to_owned(),
    };
    let count = || {
        n(
            db,
            "SELECT 1 FROM ai_tasks WHERE ai_participant_id = $1",
            vec![c.bot.into()],
        )
    };

    let queued = queue_assignment_ai_task(db, input(c.p1, "a-enabled"))
        .await
        .expect("assignment");
    assert_eq!(queued.ai_task_bots, vec![c.bot]);
    assert_eq!(count().await, 1);

    let never = queue_assignment_ai_task(db, input(c.p3, "a-never"))
        .await
        .expect("not an error");
    assert!(never.ai_task_bots.is_empty(), "no task, no trigger");
    set_enabled(&c.w, c.p1, c.v1, false).await.expect("disable");
    let disabled = queue_assignment_ai_task(db, input(c.p1, "a-disabled"))
        .await
        .expect("not an error");
    assert!(disabled.ai_task_bots.is_empty());
    assert_eq!(
        count().await,
        1,
        "no task was created for the unconnected or disabled project"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn mentioning_a_bot_that_is_not_connected_here_creates_no_task_and_the_comment_stays() {
    let scratch = scratch_or_skip!("gate_mention");
    let db = &scratch.db;
    let c = cast(db, "m1").await;
    let issue = seed_work_item(db, &c.w.tenant, "Mention").await;
    exec(
        db,
        "UPDATE work_items SET project_id = $2 WHERE id = $1",
        vec![issue.into(), c.p3.into()],
    )
    .await;
    exec(db, "UPDATE projects SET type_key = 'code_project'", vec![]).await;
    exec(db, "UPDATE projects SET type_key = 'code_project'", vec![]).await;
    let comment = |text: &str| {
        let state = c.w.state.clone();
        let owner = c.w.owner;
        let bot = c.bot;
        let text = text.to_owned();
        async move {
            crate::routes::comment::create_comment(
                State(state),
                Extension(claims_for(owner)),
                None,
                HeaderMap::new(),
                Path(issue),
                Json(crate::routes::comment::CreateCommentRequest {
                    content: Some(text),
                    body: None,
                    mentions: Some(vec![bot]),
                }),
            )
            .await
        }
    };
    comment("@Gate bot please look")
        .await
        .expect("the comment is still written");
    assert_eq!(
        n(db, "SELECT 1 FROM comments WHERE work_item_id = $1", vec![issue.into()]).await,
        1
    );
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM ai_tasks WHERE ai_participant_id = $1",
            vec![c.bot.into()]
        )
        .await,
        0,
        "a mention in a project the bot is not connected to starts nothing"
    );
    scratch.drop_self().await;
}
