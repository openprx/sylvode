//! The Flow authorization epoch moves, so cached permissions and live sessions are re-evaluated,
//! whenever a bot's credentials stop working: rotate, the disable that switches the token off,
//! removal and the deletion of the last project.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use sea_orm::TransactionTrait;
use uuid::Uuid;

use super::bot_connection_tests::{
    W, existing_bot, expect_connect, new_bot, remove, rotate, second_project, set_enabled, world,
};
use crate::routes::context::tenant_fixture::{count, exec};
use crate::scratch_or_skip;

async fn epoch(db: &sea_orm::DatabaseConnection, w: &W) -> i64 {
    count(
        db,
        "SELECT authz_epoch AS count FROM flow_workspace_settings WHERE workspace_id = $1",
        vec![w.tenant.workspace_id.into()],
    )
    .await
}

async fn flow_world(db: &sea_orm::DatabaseConnection, tag: &str) -> W {
    let w = world(db, tag).await;
    exec(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled) VALUES ($1, true) ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = EXCLUDED.flow_enabled",
        vec![w.tenant.workspace_id.into()],
    )
    .await;
    w
}

#[tokio::test]
async fn rotate_advances_the_epoch() {
    let scratch = scratch_or_skip!("epoch_rotate");
    let db = &scratch.db;
    let w = flow_world(db, "r1").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Epoch bot")).await;
    let before = epoch(db, &w).await;
    rotate(&w, w.tenant.project_id, first.connection.version)
        .await
        .expect("rotate");
    assert_eq!(
        epoch(db, &w).await,
        before + 1,
        "the old token's sessions are re-evaluated"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn only_the_disable_that_switches_the_token_off_advances_the_epoch() {
    let scratch = scratch_or_skip!("epoch_disable");
    let db = &scratch.db;
    let w = flow_world(db, "d1").await;
    let p2 = second_project(db, &w, "ED2").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Epoch bot")).await;
    let bot = first.connection.bot.id;
    let second = expect_connect(&w, p2, existing_bot(bot)).await;
    let before = epoch(db, &w).await;
    // Another connection is still enabled: the token stays on and nothing moves.
    set_enabled(&w, p2, second.connection.version, false)
        .await
        .expect("disable p2");
    assert_eq!(epoch(db, &w).await, before);
    // The last enabled connection goes off: the token stops.
    let version = count(
        db,
        "SELECT version::bigint AS count FROM project_bot_connections WHERE project_id = $1",
        vec![w.tenant.project_id.into()],
    )
    .await;
    set_enabled(&w, w.tenant.project_id, i32::try_from(version).expect("version"), false)
        .await
        .expect("disable p1");
    assert_eq!(epoch(db, &w).await, before + 1);
    scratch.drop_self().await;
}

#[tokio::test]
async fn removing_the_last_connection_and_deleting_the_last_project_advance_the_epoch() {
    let scratch = scratch_or_skip!("epoch_remove");
    let db = &scratch.db;
    let w = flow_world(db, "e1").await;
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Epoch bot")).await;
    let before = epoch(db, &w).await;
    remove(&w, w.tenant.project_id, first.connection.version)
        .await
        .expect("remove");
    assert_eq!(epoch(db, &w).await, before + 1);

    // A deleted project hands the epoch back so the route can invalidate after its commit.
    let p2 = second_project(db, &w, "EE2").await;
    expect_connect(&w, p2, new_bot("Second epoch bot")).await;
    let before = epoch(db, &w).await;
    let tx = db.begin().await.expect("begin");
    let released = crate::services::teardown::delete_project_content(&tx, p2)
        .await
        .expect("teardown");
    tx.commit().await.expect("commit");
    assert_eq!(released, Some((w.tenant.workspace_id, before + 1)));
    let _ = Uuid::nil();
    scratch.drop_self().await;
}
