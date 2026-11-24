//! Migration 0114 against a real database server: a database that holds the earlier shape (one
//! connection per project, settings in `project_execution_settings`) ends in the same shape as a
//! new one, a second run changes nothing, and the unique index allows one tmux connection per CLI
//! type in a project.
//!
//! Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped.
#![allow(clippy::expect_used, clippy::panic)]

use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

use crate::routes::context::tenant_fixture::{count, exec, seed_bot_runner, seed_bot_user, seed_tenant};
use crate::scratch_or_skip;

const MIGRATION: &str = include_str!("../../../../migrations/0114_multi_bot_connections.sql");
/// The later migration that widens the tmux check to codex; the final shape includes it.
const TMUX_CODEX: &str = include_str!("../../../../migrations/0130_tmux_cli_codex.sql");

/// The tables as 0110 and 0112 created them before several connections per project.
const EARLIER_SHAPE: &str = "
DROP TABLE project_bot_connections;
CREATE TABLE project_execution_settings (
  project_id          UUID PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  enabled             BOOLEAN     NOT NULL DEFAULT FALSE,
  channel             TEXT        NOT NULL DEFAULT 'tmux',
  cli                 TEXT        NOT NULL DEFAULT 'claude',
  working_directory   TEXT        NOT NULL,
  on_complete_state   TEXT        NOT NULL DEFAULT 'auto',
  on_fail_label_id    UUID        NULL REFERENCES labels(id) ON DELETE SET NULL,
  approval_gate       TEXT        NOT NULL DEFAULT 'none',
  version             INTEGER     NOT NULL DEFAULT 1,
  updated_by          UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE project_bot_connections (
  project_id   UUID        PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
  bot_id       UUID        NOT NULL REFERENCES bot_runners(bot_id) ON DELETE CASCADE,
  version      INTEGER     NOT NULL DEFAULT 1,
  created_by   UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  updated_by   UUID        NULL REFERENCES users(id) ON DELETE SET NULL,
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_project_bot_connections_bot ON project_bot_connections (bot_id);
";

/// Columns, constraints and indexes of the connection table, in a form that can be compared.
async fn shape(db: &DatabaseConnection) -> String {
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT jsonb_build_object( \
               'columns', (SELECT jsonb_agg(jsonb_build_array(column_name, data_type, is_nullable, column_default) \
                  ORDER BY column_name) FROM information_schema.columns \
                  WHERE table_schema = current_schema() AND table_name = 'project_bot_connections'), \
               'constraints', (SELECT jsonb_agg(jsonb_build_array(conname, pg_get_constraintdef(oid)) ORDER BY conname) \
                  FROM pg_constraint WHERE conrelid = 'project_bot_connections'::regclass), \
               'indexes', (SELECT jsonb_agg(jsonb_build_array(indexname, indexdef) ORDER BY indexname) \
                  FROM pg_indexes WHERE schemaname = current_schema() AND tablename = 'project_bot_connections') \
             )::text AS shape"
                .to_string(),
        ))
        .await
        .expect("shape query runs")
        .expect("shape query returns a row");
    row.try_get("", "shape").expect("shape is text")
}

async fn relation_present(db: &DatabaseConnection, name: &str) -> bool {
    count(
        db,
        "SELECT (to_regclass($1) IS NOT NULL)::int::bigint AS count",
        vec![name.to_string().into()],
    )
    .await
        == 1
}

#[tokio::test]
async fn a_new_database_has_the_final_shape_and_the_migration_changes_nothing() {
    let scratch = scratch_or_skip!("multi_conn_fresh");
    let db = &scratch.db;
    let before = shape(db).await;
    assert!(!relation_present(db, "project_execution_settings").await);
    for run in 0..2 {
        db.execute_unprepared(MIGRATION)
            .await
            .unwrap_or_else(|err| panic!("run {run} of 0114 on a new database: {err}"));
        assert_eq!(shape(db).await, before, "run {run} changes nothing");
    }
    for needle in [
        "\"pbc_one_tmux_per_cli\"",
        "UNIQUE INDEX pbc_one_tmux_per_cli ON",
        "WHERE (channel = 'tmux'::text)",
        "pbc_tmux_cli_check",
        "PRIMARY KEY (project_id, bot_id)",
    ] {
        assert!(before.contains(needle), "{needle} missing from {before}");
    }
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_database_at_0113_is_brought_forward_and_a_second_run_changes_nothing() {
    let scratch = scratch_or_skip!("multi_conn_forward");
    let db = &scratch.db;
    let final_shape = shape(db).await;

    let tenant = seed_tenant(db, "mf").await;
    let bot = seed_bot_user(db, &tenant).await;
    seed_bot_runner(db, &tenant, bot).await;
    let other_project = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'PMF2', 'second', $3)",
        vec![
            other_project.into(),
            tenant.workspace_id.into(),
            tenant.member_id.into(),
        ],
    )
    .await;

    db.execute_unprepared(EARLIER_SHAPE)
        .await
        .expect("the earlier shape is built");
    exec(
        db,
        "INSERT INTO project_execution_settings \
         (project_id, enabled, channel, cli, working_directory, on_complete_state, version) \
         VALUES ($1, true, 'tmux', 'claude', '/work/a', 'none', 4), \
                ($2, false, 'tmux', 'codex', '/work/b', 'auto', 1)",
        vec![tenant.project_id.into(), other_project.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO project_bot_connections (project_id, bot_id, version) VALUES ($1, $3, 7), ($2, $3, 2)",
        vec![tenant.project_id.into(), other_project.into(), bot.into()],
    )
    .await;

    for run in 0..2 {
        db.execute_unprepared(MIGRATION)
            .await
            .unwrap_or_else(|err| panic!("run {run} of 0114 on the earlier shape: {err}"));
        db.execute_unprepared(TMUX_CODEX)
            .await
            .unwrap_or_else(|err| panic!("run {run} of 0130 after 0114: {err}"));
        assert_eq!(shape(db).await, final_shape, "run {run} ends in the final shape");
        assert!(!relation_present(db, "project_execution_settings").await);
    }

    let first = count(
        db,
        "SELECT COUNT(*)::bigint AS count FROM project_bot_connections \
         WHERE project_id = $1 AND bot_id = $2 AND enabled AND channel = 'tmux' AND cli = 'claude' \
           AND working_directory = '/work/a' AND on_complete_state = 'none' AND version = 7",
        vec![tenant.project_id.into(), bot.into()],
    )
    .await;
    assert_eq!(first, 1, "the settings are copied and the connection version kept");
    let moved = count(
        db,
        "SELECT COUNT(*)::bigint AS count FROM project_bot_connections \
         WHERE project_id = $1 AND NOT enabled AND channel = 'headless' AND cli = 'codex' \
           AND working_directory = '/work/b' AND version = 2",
        vec![other_project.into()],
    )
    .await;
    assert_eq!(moved, 1, "a tmux row for another CLI moves to headless");
    scratch.drop_self().await;
}

#[tokio::test]
async fn one_tmux_connection_per_cli_and_any_number_of_headless_ones() {
    let scratch = scratch_or_skip!("multi_conn_unique");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "mu").await;
    let (a, b, c) = (
        seed_bot_user(db, &tenant).await,
        seed_bot_user(db, &tenant).await,
        seed_bot_user(db, &tenant).await,
    );
    for bot in [a, b, c] {
        seed_bot_runner(db, &tenant, bot).await;
    }
    let insert = |bot: Uuid, channel: &'static str| {
        let project = tenant.project_id;
        async move {
            db.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO project_bot_connections (project_id, bot_id, working_directory, channel, cli) \
                 VALUES ($1, $2, '/work/a', $3, 'claude')",
                vec![project.into(), bot.into(), channel.to_string().into()],
            ))
            .await
        }
    };
    insert(a, "tmux").await.expect("the first tmux connection");
    let second = insert(b, "tmux")
        .await
        .expect_err("a second tmux connection for claude");
    assert!(second.to_string().contains("pbc_one_tmux_per_cli"), "{second}");
    insert(b, "headless").await.expect("headless next to tmux");
    insert(c, "headless").await.expect("a second headless connection");
    let twice = insert(a, "headless").await;
    assert!(twice.is_err(), "(project, bot) is the primary key");
    scratch.drop_self().await;
}
