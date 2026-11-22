//! Deleting a workspace, a project or a bot's user while a project bot connection exists: every
//! foreign key accepts the order, and nothing connection-related is left behind.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use sea_orm::{ConnectionTrait, TransactionTrait};
use uuid::Uuid;

use super::bot_connection_tests::{W, connect, existing_bot, expect_connect, new_bot, second_project, world};
use crate::routes::context::tenant_fixture::{count, state_for};
use crate::scratch_or_skip;

async fn rows(db: &sea_orm::DatabaseConnection, table: &str, column: &str, id: Uuid) -> i64 {
    count(
        db,
        &format!("SELECT COUNT(*)::bigint AS count FROM {table} WHERE {column}::text = $1::text"),
        vec![id.into()],
    )
    .await
}

async fn connected(db: &sea_orm::DatabaseConnection, tag: &str) -> (W, Uuid, Uuid, Uuid) {
    let w = world(db, tag).await;
    let p1 = w.tenant.project_id;
    let p2 = second_project(db, &w, &format!("T{tag}")).await;
    let first = expect_connect(&w, p1, new_bot("Teardown bot")).await;
    let bot = first.connection.bot.id;
    expect_connect(&w, p2, existing_bot(bot)).await;
    (w, bot, p1, p2)
}

#[tokio::test]
async fn deleting_a_workspace_with_connections_leaves_no_connection_rows() {
    let scratch = scratch_or_skip!("teardown_ws");
    let db = &scratch.db;
    let (w, bot, _p1, _p2) = connected(db, "w1").await;
    let ws = w.tenant.workspace_id;
    let tx = db.begin().await.expect("begin");
    crate::services::teardown::delete_workspace_content(&tx, ws)
        .await
        .expect("the workspace deletes");
    tx.commit().await.expect("commit");
    for table in ["bot_runners", "webhooks", "workspace_bots", "workspace_members"] {
        assert_eq!(rows(db, table, "workspace_id", ws).await, 0, "{table}");
    }
    assert_eq!(rows(db, "project_bot_connections", "bot_id", bot).await, 0);
    assert_eq!(rows(db, "ai_participants", "id", bot).await, 0);
    scratch.drop_self().await;
}

#[tokio::test]
async fn deleting_projects_one_by_one_releases_the_bot_with_the_last() {
    let scratch = scratch_or_skip!("teardown_project");
    let db = &scratch.db;
    let (w, bot, p1, p2) = connected(db, "p1").await;
    for (project, last) in [(p1, false), (p2, true)] {
        let tx = db.begin().await.expect("begin");
        crate::services::teardown::delete_project_content(&tx, project)
            .await
            .expect("project content goes");
        tx.execute(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "DELETE FROM projects WHERE id = $1",
            vec![project.into()],
        ))
        .await
        .expect("the project row deletes");
        tx.commit().await.expect("commit");
        assert_eq!(rows(db, "project_bot_connections", "project_id", project).await, 0);
        assert_eq!(
            rows(db, "bot_runners", "bot_id", bot).await,
            i64::from(!last),
            "runner row, last={last}"
        );
        assert_eq!(
            rows(db, "webhooks", "bot_user_id", bot).await,
            i64::from(!last),
            "managed webhook, last={last}"
        );
    }
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM workspace_bots WHERE id = $1 AND is_active",
            vec![bot.into()],
        )
        .await,
        0,
        "the token is revoked with the last project"
    );
    let _ = w;
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_bot_user_with_connections_is_disabled_by_the_admin_route_and_never_hard_deleted() {
    let scratch = scratch_or_skip!("teardown_user");
    let db = &scratch.db;
    let (_w, bot, _p1, _p2) = connected(db, "u1").await;
    // The application disables an account; it never removes the row (`routes::admin::delete_user`).
    crate::routes::admin::delete_user(axum::extract::State(state_for(db.clone())), axum::extract::Path(bot))
        .await
        .expect("the admin route disables the bot user");
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM users WHERE id = $1 AND NOT is_active",
            vec![bot.into()],
        )
        .await,
        1
    );
    // A hard delete of the row is refused by the managed webhook's reference, so no credential is
    // ever orphaned by it; the connection rows stay whole.
    let hard = sea_orm::ConnectionTrait::execute(
        db,
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "DELETE FROM users WHERE id = $1",
            vec![bot.into()],
        ),
    )
    .await;
    assert!(hard.is_err(), "the webhook keeps pointing at its bot");
    assert_eq!(rows(db, "bot_runners", "bot_id", bot).await, 1);
    assert_eq!(rows(db, "project_bot_connections", "bot_id", bot).await, 2);
    scratch.drop_self().await;
}

/// Connect and project deletion take the project row first, so connecting a bot to a project that is
/// being deleted waits instead of deadlocking.
#[tokio::test]
async fn releasing_a_project_locks_the_project_row_before_the_connection_rows() {
    let scratch = scratch_or_skip!("teardown_lock");
    let db = &scratch.db;
    let (_w, _bot, p1, _p2) = connected(db, "l1").await;
    let tx = db.begin().await.expect("begin");
    crate::services::bot_connections::release_for_project(&tx, p1)
        .await
        .expect("release");
    // Another writer cannot take the project row while the release is open.
    let other = db.begin().await.expect("begin");
    let attempt = other
        .execute(sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "SELECT id FROM projects WHERE id = $1 FOR NO KEY UPDATE NOWAIT",
            vec![p1.into()],
        ))
        .await;
    assert!(attempt.is_err(), "the project row is locked by the release");
    drop(other);
    tx.commit().await.expect("commit");
    scratch.drop_self().await;
}

#[tokio::test]
async fn connecting_while_the_project_is_deleted_does_not_deadlock() {
    let scratch = scratch_or_skip!("teardown_race");
    let db = &scratch.db;
    for round in 0..5 {
        let w = world(db, &format!("rc{round}")).await;
        let p1 = w.tenant.project_id;
        let p2 = second_project(db, &w, &format!("RC{round}")).await;
        let first = expect_connect(&w, p1, new_bot("Race bot")).await;
        let bot = first.connection.bot.id;
        let connect_task = connect(&w, p2, existing_bot(bot));
        let delete_task = async {
            let tx = db.begin().await.expect("begin");
            let released = crate::services::teardown::delete_project_content(&tx, p2).await;
            let deleted = match released {
                Ok(_) => tx
                    .execute(sea_orm::Statement::from_sql_and_values(
                        sea_orm::DbBackend::Postgres,
                        "DELETE FROM projects WHERE id = $1",
                        vec![p2.into()],
                    ))
                    .await
                    .map(|_| ()),
                Err(err) => return Err(format!("{err:?}")),
            };
            deleted.map_err(|err| err.to_string())?;
            tx.commit().await.map_err(|err| err.to_string())
        };
        let (connected, deleted) = tokio::join!(connect_task, delete_task);
        // Either order is fine; a database deadlock is not.
        if let Err(err) = &connected {
            let text = format!("{err:?}");
            assert!(!text.to_lowercase().contains("deadlock"), "connect: {text}");
        }
        if let Err(text) = &deleted {
            assert!(!text.to_lowercase().contains("deadlock"), "delete: {text}");
        }
    }
    scratch.drop_self().await;
}
