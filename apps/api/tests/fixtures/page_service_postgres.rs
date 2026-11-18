#![allow(
    clippy::expect_used,
    reason = "test fixture infrastructure reports which setup or teardown operation failed"
)]

use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement, Value};

pub struct Scratch {
    pub db: DatabaseConnection,
    name: String,
    admin_url: String,
}
impl Scratch {
    pub async fn drop_self(self) {
        let Self { db, name, admin_url } = self;
        drop(db);
        let admin = Database::connect(&admin_url)
            .await
            .expect("connect to clean up the disposable database");
        admin
            .execute_unprepared(&format!("DROP DATABASE \"{name}\" WITH (FORCE)"))
            .await
            .expect("drop the disposable database");
    }
}
pub async fn scratch(label: &str) -> Option<Scratch> {
    let admin_url = std::env::var("OPENPR_TEST_DATABASE_URL").ok()?;
    let name = format!("sv_p3_{label}");
    assert!(name.len() < 63 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    let admin = Database::connect(&admin_url)
        .await
        .expect("connect to provision the disposable database");
    admin
        .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
        .await
        .expect("create the disposable database");
    let (prefix, _) = admin_url.rsplit_once('/').expect("test URL has a database path");
    let db = Database::connect(format!("{prefix}/{name}"))
        .await
        .expect("connect to the disposable database");
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../migrations");
    let mut files = std::fs::read_dir(dir)
        .expect("read the migrations directory")
        .map(|entry| entry.expect("read migration directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect::<Vec<_>>();
    files.sort();
    for path in files {
        db.execute_unprepared(&std::fs::read_to_string(path).expect("read migration file"))
            .await
            .expect("apply migration");
    }
    Some(Scratch { db, name, admin_url })
}
pub async fn exec<C: ConnectionTrait>(db: &C, sql: &str, values: Vec<Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .expect("execute fixture SQL");
}
