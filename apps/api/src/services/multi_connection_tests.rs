//! Several bot connections in one project: each bot keeps its own settings, tasks and state, at
//! most one tmux connection per CLI type exists, and removing or disabling one leaves the others.
//!
//! Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_lines)]

use sea_orm::TransactionTrait;
use serde_json::{Value, json};
use uuid::Uuid;

use super::ai_task_fixture::seed_task;
use super::ai_task_service::{RunnerBotBlock, load_task_with_protocol, runner_bot_block};
use super::ai_task_view::build_task_view;
use super::bot_connection_tests::{
    done, existing_bot, expect_connect, n, new_bot, remove_for, set_enabled_for, text_of, update_for, world,
};
use super::bot_connections;
use crate::error::ApiError;
use crate::routes::context::tenant_fixture::count;
use crate::scratch_or_skip;

fn headless(name: &str, cli: &str, directory: &str) -> Value {
    let mut body = new_bot(name);
    body["channel"] = json!("headless");
    body["cli"] = json!(cli);
    body["working_directory"] = json!(directory);
    body
}

fn tmux_codex(name: &str, directory: &str) -> Value {
    let mut body = new_bot(name);
    body["channel"] = json!("tmux");
    body["cli"] = json!("codex");
    body["working_directory"] = json!(directory);
    body
}

#[tokio::test]
async fn a_claude_and_a_codex_tmux_connection_live_side_by_side_and_a_second_codex_is_accepted() {
    let scratch = scratch_or_skip!("mc_codex_tmux");
    let db = &scratch.db;
    let w = world(db, "x1").await;
    let project = w.tenant.project_id;

    let claude = expect_connect(&w, project, new_bot("Alpha")).await.connection;
    assert_eq!(claude.execution.session_name.as_deref(), Some("Px1-claude"));
    let codex = expect_connect(&w, project, tmux_codex("Beta", "/srv/dev/work/beta"))
        .await
        .connection;
    assert_eq!(codex.execution.channel, "tmux");
    assert_eq!(codex.execution.cli, "codex");
    assert_eq!(codex.execution.session_name.as_deref(), Some("Px1-codex"));

    let second = expect_connect(&w, project, tmux_codex("Gamma", "/srv/dev/work/gamma"))
        .await
        .connection;
    assert_eq!(second.execution.channel, "tmux");
    assert_eq!(second.execution.cli, "codex");
    assert_eq!(n(db, "SELECT 1 FROM workspace_bots", vec![]).await, 3);
    scratch.drop_self().await;
}

#[tokio::test]
async fn tmux_and_headless_connections_are_not_limited_per_cli() {
    let scratch = scratch_or_skip!("mc_unique");
    let db = &scratch.db;
    let w = world(db, "u1").await;
    let project = w.tenant.project_id;

    let alpha = expect_connect(&w, project, new_bot("Alpha")).await.connection;
    assert_eq!(alpha.execution.channel, "tmux");
    assert_eq!(alpha.execution.session_name.as_deref(), Some("Pu1-claude"));

    // A second bot connects over tmux with the same CLI.
    let zeta = expect_connect(&w, project, new_bot("Zeta")).await.connection;
    assert_eq!(zeta.execution.channel, "tmux");
    assert_eq!(zeta.execution.cli, "claude");

    // Headless connections are not limited: several, next to the tmux one.
    let beta = expect_connect(&w, project, headless("Beta", "codex", "/srv/dev/work/beta"))
        .await
        .connection;
    assert_eq!(beta.execution.session_name, None);
    let gamma = expect_connect(&w, project, headless("Gamma", "claude", "/srv/dev/work/gamma"))
        .await
        .connection;

    // A second bot may take the tmux channel for the CLI Alpha already holds.
    let switched = done(
        update_for(
            &w,
            project,
            gamma.bot.id,
            json!({"expected_version": gamma.version, "channel": "tmux"}),
        )
        .await,
    )
    .await;
    assert_eq!(switched.execution.channel, "tmux");
    assert_eq!(switched.execution.cli, "claude");
    let tmux_claude = count(
        db,
        "SELECT COUNT(*)::bigint AS count FROM project_bot_connections \
         WHERE project_id = $1 AND channel = 'tmux' AND cli = 'claude'",
        vec![project.into()],
    )
    .await;
    assert_eq!(
        tmux_claude, 3,
        "Alpha, Zeta and Gamma hold a tmux connection for claude"
    );
    // opencode cannot use tmux at all.
    let opencode = expect_connect(&w, project, headless("Delta", "opencode", "/srv/dev/work/delta"))
        .await
        .connection;
    let refused = update_for(
        &w,
        project,
        opencode.bot.id,
        json!({"expected_version": opencode.version, "channel": "tmux"}),
    )
    .await;
    assert!(
        matches!(refused, Err(ApiError::BadRequest(m)) if m == "cli: only claude and codex can use the tmux channel")
    );

    // The list is ordered by bot name.
    let names: Vec<String> = bot_connections::list_views(db, project)
        .await
        .expect("lists")
        .into_iter()
        .map(|view| view.bot.name)
        .collect();
    assert_eq!(names, ["Alpha", "Beta", "Delta", "Gamma", "Zeta"]);
    scratch.drop_self().await;
}

#[tokio::test]
async fn each_bot_gets_its_own_tasks_with_its_own_settings() {
    let scratch = scratch_or_skip!("mc_tasks");
    let db = &scratch.db;
    let w = world(db, "t1").await;
    let project = w.tenant.project_id;
    let alpha = expect_connect(&w, project, new_bot("Alpha")).await.connection;
    let beta = expect_connect(&w, project, headless("Beta", "codex", "/srv/dev/work/beta"))
        .await
        .connection;

    let view_for = |bot: Uuid| {
        let tenant = &w.tenant;
        async move {
            let task = seed_task(db, tenant, bot, "processing", 1, 0).await;
            let entry = load_task_with_protocol(db, task).await.expect("task loads");
            build_task_view(db, &entry.task, &entry.protocol)
                .await
                .expect("view builds")
        }
    };
    let alpha_view = view_for(alpha.bot.id).await;
    assert_eq!(alpha_view["execution"]["channel"], "tmux");
    assert_eq!(alpha_view["execution"]["cli"], "claude");
    assert_eq!(alpha_view["execution"]["working_directory"], "/srv/dev/work/svwl");
    assert!(alpha_view["execution"].get("session_name").is_none());
    let beta_view = view_for(beta.bot.id).await;
    assert_eq!(beta_view["execution"]["channel"], "headless");
    assert_eq!(beta_view["execution"]["cli"], "codex");
    assert_eq!(beta_view["execution"]["working_directory"], "/srv/dev/work/beta");
    assert!(beta_view["execution"].get("session_name").is_none());

    // The gate follows the bot's own connection.
    assert_eq!(runner_bot_block(db, project, alpha.bot.id).await.expect("gate"), None);
    done(set_enabled_for(&w, project, alpha.bot.id, alpha.version, false).await).await;
    assert_eq!(
        runner_bot_block(db, project, alpha.bot.id).await.expect("gate"),
        Some(RunnerBotBlock::Disabled)
    );
    assert_eq!(
        runner_bot_block(db, project, beta.bot.id).await.expect("gate"),
        None,
        "disabling one bot leaves the other running"
    );
    // The other bot's token and webhook stay on, and its connection is still enabled.
    assert_eq!(
        text_of(
            db,
            "SELECT (b.is_active::text || '/' || c.enabled::text || '/' || wh.active::text) AS v \
             FROM workspace_bots b JOIN project_bot_connections c ON c.bot_id = b.id \
             JOIN bot_runners r ON r.bot_id = b.id JOIN webhooks wh ON wh.id = r.webhook_id WHERE b.id = $1",
            vec![beta.bot.id.into()],
        )
        .await,
        "true/true/true"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn removing_one_connection_keeps_the_other_and_the_last_one_releases_its_bot() {
    let scratch = scratch_or_skip!("mc_remove");
    let db = &scratch.db;
    let w = world(db, "r2").await;
    let project = w.tenant.project_id;
    let alpha = expect_connect(&w, project, new_bot("Alpha")).await.connection;
    let beta = expect_connect(&w, project, headless("Beta", "claude", "/srv/dev/work/beta"))
        .await
        .connection;
    // Alpha also serves a second project, so it is not released with the first.
    let other = super::bot_connection_tests::second_project(db, &w, "R2B").await;
    expect_connect(&w, other, existing_bot(alpha.bot.id)).await;

    let removed = match remove_for(&w, project, alpha.bot.id, alpha.version).await {
        Ok(super::bot_connections::Outcome::Done(removed)) => removed,
        other => panic!("remove failed: {:?}", other.map(|_| ())),
    };
    assert_eq!(removed.bot_still_connected_to.len(), 1);
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM project_bot_connections WHERE project_id = $1",
            vec![project.into()]
        )
        .await,
        1
    );
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM ai_participants WHERE project_id = $1 AND id = $2::text",
            vec![project.into(), beta.bot.id.into()]
        )
        .await,
        1,
        "the other bot keeps its registration"
    );
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND is_active",
            vec![alpha.bot.id.into()]
        )
        .await,
        1,
        "Alpha still serves another project"
    );
    // Beta is the project's last connection of its bot: the teardown releases it.
    let beta_now = bot_connections::load_view(db, project, beta.bot.id)
        .await
        .expect("loads")
        .expect("exists");
    let last = match remove_for(&w, project, beta.bot.id, beta_now.version).await {
        Ok(super::bot_connections::Outcome::Done(removed)) => removed,
        other => panic!("remove failed: {:?}", other.map(|_| ())),
    };
    assert!(last.bot_still_connected_to.is_empty());
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM bot_runners WHERE bot_id = $1",
            vec![beta.bot.id.into()]
        )
        .await,
        0
    );
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM workspace_bots WHERE id = $1 AND is_active",
            vec![beta.bot.id.into()]
        )
        .await,
        0,
        "the last connection revokes the token"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn deleting_a_project_releases_every_connection_of_it() {
    let scratch = scratch_or_skip!("mc_project_delete");
    let db = &scratch.db;
    let w = world(db, "d2").await;
    let project = w.tenant.project_id;
    let alpha = expect_connect(&w, project, new_bot("Alpha")).await.connection;
    let beta = expect_connect(&w, project, headless("Beta", "claude", "/srv/dev/work/beta"))
        .await
        .connection;
    let tx = db.begin().await.expect("begin");
    crate::services::teardown::delete_project_content(&tx, project)
        .await
        .expect("the project content goes");
    tx.commit().await.expect("commit");
    for bot in [alpha.bot.id, beta.bot.id] {
        assert_eq!(
            n(db, "SELECT 1 FROM bot_runners WHERE bot_id = $1", vec![bot.into()]).await,
            0
        );
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM workspace_bots WHERE id = $1 AND is_active",
                vec![bot.into()]
            )
            .await,
            0
        );
    }
    assert_eq!(
        n(
            db,
            "SELECT 1 FROM project_bot_connections WHERE project_id = $1",
            vec![project.into()]
        )
        .await,
        0
    );
    scratch.drop_self().await;
}
