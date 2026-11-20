//! Shared fixtures for the database tests of this module: a scratch database with the real
//! migration files applied, optionally stopping short of this feature's migration.

use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use uuid::Uuid;

use super::tokens::{TokenKind, mint};
use crate::routes::context::tenant_fixture::exec;

pub const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

/// The migration under test is found by this suffix, so the tests do not name its number.
const OAUTH_MIGRATION_SUFFIX: &str = "_oauth_authorization_server.sql";

/// Which migrations a scratch database starts with.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// Everything, this feature included.
    Complete,
    /// Every migration that sorts before this feature's: a database that predates it.
    BeforeOauth,
}

pub struct Scratch {
    pub db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    pub async fn drop_self(self) {
        let Self { db, name, admin_url } = self;
        drop(db);
        let Ok(admin) = Database::connect(&admin_url).await else {
            return;
        };
        let _ = admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await;
    }

    /// Executes this feature's migration file once.
    pub async fn apply_oauth_migration(&self) -> Result<(), sea_orm::DbErr> {
        self.db.execute_unprepared(&oauth_migration_sql()).await.map(|_| ())
    }
}

fn migration_files() -> Vec<std::path::PathBuf> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("migrations directory is readable")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no migration file was found in {dir}");
    files
}

fn is_oauth_migration(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(OAUTH_MIGRATION_SUFFIX))
}

pub fn oauth_migration_sql() -> String {
    let path = migration_files()
        .into_iter()
        .find(|path| is_oauth_migration(path))
        .expect("the OAuth migration file exists");
    std::fs::read_to_string(path).expect("the OAuth migration is readable")
}

/// A scratch database, or `None` when no test server is configured.
pub async fn scratch(label: &str, start: Start) -> Option<Scratch> {
    let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
    let admin = Database::connect(&admin_url)
        .await
        .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));
    let name = format!("sylvode_oauth_{label}");
    admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .await
        .unwrap_or_else(|err| panic!("could not reset scratch database {name}: {err}"));
    admin
        .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
        .await
        .unwrap_or_else(|err| panic!("could not create scratch database {name}: {err}"));
    let (prefix, _) = admin_url.rsplit_once('/')?;
    let db = Database::connect(&format!("{prefix}/{name}"))
        .await
        .unwrap_or_else(|err| panic!("could not connect to scratch database {name}: {err}"));
    for path in migration_files() {
        if start == Start::BeforeOauth && is_oauth_migration(&path) {
            break;
        }
        let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
        db.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
    }
    Some(Scratch { db, name, admin_url })
}

macro_rules! oauth_scratch_or_skip {
    ($label:expr, $start:expr) => {
        match $crate::oauth::test_support::scratch($label, $start).await {
            Some(scratch) => scratch,
            None => {
                eprintln!(
                    "SKIPPED (no database): set {} to run this test",
                    $crate::oauth::test_support::TEST_DATABASE_URL_ENV
                );
                return;
            }
        }
    };
}
pub(crate) use oauth_scratch_or_skip;

#[derive(Debug, FromQueryResult)]
struct Count {
    n: i64,
}

/// `SELECT count(*) AS n ...` as a number.
pub async fn count(db: &impl ConnectionTrait, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
    Count::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .one(db)
        .await
        .expect("count query runs")
        .map_or(0, |row| row.n)
}

/// Inserts an active grant for the ramflux client and returns its id.
pub async fn seed_grant(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    workspace_id: Uuid,
    mode: &str,
    scopes: &[&str],
) -> Uuid {
    let grant_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO oauth_grants (id, client_id, user_id, workspace_id, scopes, resources, project_mode, \
         user_session_version, expires_at) \
         VALUES ($1, 'ramflux', $2, $3, $4, ARRAY['api','mcp'], $5, \
         (SELECT session_version FROM users WHERE id = $2), now() + interval '365 days')",
        vec![
            grant_id.into(),
            user_id.into(),
            workspace_id.into(),
            scopes.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>().into(),
            mode.into(),
        ],
    )
    .await;
    grant_id
}

/// Inserts a live access token for `grant_id` and returns its row id and the raw token.
pub async fn seed_access_token(
    db: &impl ConnectionTrait,
    grant_id: Uuid,
    resource: &str,
    scopes: &[&str],
) -> (Uuid, String) {
    let minted = mint(TokenKind::Access).expect("a token is minted");
    let token_id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO oauth_tokens (id, token_hash, kind, grant_id, client_id, resource, scopes, expires_at) \
         VALUES ($1, $2, 'access', $3, 'ramflux', $4, $5, now() + interval '10 minutes')",
        vec![
            token_id.into(),
            minted.hash.as_hex().into(),
            grant_id.into(),
            resource.into(),
            scopes.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>().into(),
        ],
    )
    .await;
    (token_id, minted.raw.expose().to_owned())
}
