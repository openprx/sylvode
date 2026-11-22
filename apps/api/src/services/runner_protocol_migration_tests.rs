//! Migration 0110 against a real database server: it re-runs cleanly, leaves running tasks
//! alone and enforces the documented status set. The execution settings of a connection are
//! validated by the database too (checks of `project_bot_connections`, 0112).
//!
//! Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped. The file
//! is located by its name suffix, so these tests do not change when the migration is renumbered.
#![allow(clippy::expect_used, clippy::panic)]

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

use crate::routes::context::tenant_fixture::{Tenant, count, exec, seed_bot_runner, seed_bot_user, seed_tenant};
use crate::scratch_or_skip;

fn migration_sql() -> String {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let path = std::fs::read_dir(dir)
        .expect("migrations directory is readable")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_runner_task_protocol.sql"))
        })
        .expect("the runner task protocol migration file exists");
    std::fs::read_to_string(path).expect("the migration is readable")
}

async fn constraints_named(db: &DatabaseConnection, name: &str) -> i64 {
    count(
        db,
        "SELECT COUNT(*)::bigint AS count FROM pg_constraint WHERE conrelid = 'ai_tasks'::regclass AND conname = $1",
        vec![name.to_string().into()],
    )
    .await
}

async fn insert_settings(
    db: &DatabaseConnection,
    tenant: &Tenant,
    bot_id: Uuid,
    directory: &str,
    column: &str,
    value: &str,
) -> bool {
    // `column` and `value` override one default, so each case changes exactly one field.
    let (channel, cli, gate, state) = match column {
        "channel" => (value, "claude", "none", "auto"),
        "cli" => ("headless", value, "none", "auto"),
        "approval_gate" => ("tmux", "claude", value, "auto"),
        "on_complete_state" => ("tmux", "claude", "none", value),
        _ => ("tmux", "claude", "none", "auto"),
    };
    let outcome = db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO project_bot_connections \
             (project_id, bot_id, working_directory, channel, cli, approval_gate, on_complete_state) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
            vec![
                tenant.project_id.into(),
                bot_id.into(),
                directory.to_string().into(),
                channel.to_string().into(),
                cli.to_string().into(),
                gate.to_string().into(),
                state.to_string().into(),
            ],
        ))
        .await;
    if outcome.is_ok() {
        exec(
            db,
            "DELETE FROM project_bot_connections WHERE project_id = $1",
            vec![tenant.project_id.into()],
        )
        .await;
    }
    outcome.is_ok()
}

#[tokio::test]
async fn the_migration_runs_twice_and_leaves_running_tasks_alone() {
    let scratch = scratch_or_skip!("runner_protocol_replay");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "rp").await;
    let bot = seed_bot_user(db, &tenant).await;
    let task_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO ai_tasks (id, project_id, ai_participant_id, task_type, status, attempts, max_attempts, started_at) \
         VALUES ($1, $2, $3, 'issue_assigned', 'processing', 1, 3, now())",
        vec![task_id.into(), tenant.project_id.into(), bot.into()],
    )
    .await;

    db.execute_unprepared(&migration_sql())
        .await
        .expect("a second execution succeeds");

    assert_eq!(constraints_named(db, "ai_tasks_status_check").await, 1);
    assert_eq!(constraints_named(db, "ai_tasks_runner_phase_check").await, 1);
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM ai_tasks \
             WHERE id = $1 AND status = 'processing' AND lease_expires_at IS NULL AND runner_phase IS NULL",
            vec![task_id.into()],
        )
        .await,
        1,
        "an existing processing row is untouched"
    );

    let bogus = db
        .execute_unprepared("UPDATE ai_tasks SET status = 'bogus' WHERE id = (SELECT id FROM ai_tasks LIMIT 1)")
        .await;
    assert!(bogus.is_err(), "an undocumented status must be refused");
    let phase = db
        .execute_unprepared("UPDATE ai_tasks SET runner_phase = 'dancing'")
        .await;
    assert!(phase.is_err(), "an undocumented runner phase must be refused");
    db.execute_unprepared("UPDATE ai_tasks SET status = 'acknowledged', runner_phase = 'pushed'")
        .await
        .expect("the new values are accepted");

    scratch.drop_self().await;
}

#[tokio::test]
async fn execution_settings_are_validated_by_the_database() {
    let scratch = scratch_or_skip!("runner_protocol_settings");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "rs").await;
    let bot = seed_bot_user(db, &tenant).await;
    seed_bot_runner(db, &tenant, bot).await;

    let long_path = format!("/{}", "a".repeat(1024));
    assert_eq!(long_path.len(), 1025);
    for bad in [
        "relative",
        "/a/../b",
        "/a/./b",
        "//a",
        "/a//b",
        "/a\u{1}b",
        "/a/..",
        "/",
        long_path.as_str(),
    ] {
        assert!(
            !insert_settings(db, &tenant, bot, bad, "", "").await,
            "working directory {bad:?} must be refused"
        );
    }
    for good in ["/a", "/a/b_c", "/a/.hidden/b", "/a/b..c"] {
        assert!(
            insert_settings(db, &tenant, bot, good, "", "").await,
            "working directory {good:?} must be accepted"
        );
    }

    for (column, value) in [
        ("channel", "x"),
        ("cli", "x"),
        ("approval_gate", "x"),
        ("on_complete_state", "state:Bad"),
        ("on_complete_state", "state:"),
    ] {
        assert!(
            !insert_settings(db, &tenant, bot, "/a", column, value).await,
            "{column} = {value:?} must be refused"
        );
    }
    for (column, value) in [
        ("channel", "headless"),
        ("cli", "codex"),
        ("on_complete_state", "state:done"),
        ("on_complete_state", "none"),
    ] {
        assert!(
            insert_settings(db, &tenant, bot, "/a", column, value).await,
            "{column} = {value:?} must be accepted"
        );
    }

    scratch.drop_self().await;
}

#[tokio::test]
async fn the_tmux_channel_is_claude_only_in_the_database() {
    let scratch = scratch_or_skip!("runner_protocol_tmux_cli");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "rt").await;
    let bot = seed_bot_user(db, &tenant).await;
    seed_bot_runner(db, &tenant, bot).await;
    for cli in ["codex", "opencode"] {
        let outcome = db
            .execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO project_bot_connections (project_id, bot_id, working_directory, channel, cli) \
                 VALUES ($1, $2, '/a', 'tmux', $3)",
                vec![tenant.project_id.into(), bot.into(), cli.to_string().into()],
            ))
            .await;
        assert!(outcome.is_err(), "tmux with {cli} must be refused");
    }
    scratch.drop_self().await;
}
