//! Rows of the retired plugin protocols (1 and 2) stay in the database, unread and unwritten.
//! Real-database test, opt-in via `OPENPR_TEST_DATABASE_URL` like every other one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    clippy::indexing_slicing,
    clippy::items_after_statements
)]

use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{BindActor, BindRequest, bind_key, catalog};
use crate::error::{ApiError, ApiErrorKind};

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

struct Scratch {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
}

impl Scratch {
    async fn drop_self(self) {
        let Self { db, name, admin_url } = self;
        drop(db);
        let Ok(admin) = Database::connect(&admin_url).await else {
            return;
        };
        let _ = admin
            .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
            .await;
    }
}

async fn scratch(label: &str) -> Option<Scratch> {
    let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
    let admin = Database::connect(&admin_url)
        .await
        .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));
    let name = format!("sylvode_plugin_{label}");
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
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../migrations");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("migrations directory is readable")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no migration file was found in {dir}");
    for path in files {
        let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
        db.execute_unprepared(&sql)
            .await
            .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
    }
    Some(Scratch { db, name, admin_url })
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

#[derive(FromQueryResult)]
struct BindingRow {
    package_id: Option<Uuid>,
    generation: i64,
    grants: Vec<String>,
}

async fn binding(db: &DatabaseConnection, project: Uuid, key: &str) -> BindingRow {
    BindingRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT package_id,generation,grants FROM form_plugin_bindings WHERE project_id=$1 AND key=$2",
        vec![project.into(), key.into()],
    ))
    .one(db)
    .await
    .expect("binding query runs")
    .expect("the binding row exists")
}

fn v2_manifest(key: &str) -> Value {
    json!({
        "schema_version": "openpr.plugin.v2", "key": key, "name": "Legacy", "version": "1.0.0",
        "publisher": "Legacy", "host_protocol": {"min": 1, "max": 1}, "wasm_abi": "openpr.plugin.v1",
        "wasm": {"path": "plugin.wasm", "size": 1, "sha256": "0".repeat(64)},
        "ui": {"entry": {"path": "panel.html", "size": 1, "sha256": "0".repeat(64)},
               "slot": "form.record.detail", "bridge_version": 1, "form_key": "order_line"},
        "capabilities": []
    })
}

fn v3_manifest(key: &str) -> Value {
    json!({
        "schema_version": "openpr.plugin.v3", "key": key, "name": "Board", "version": "3.0.0",
        "publisher": "Author", "host_protocol": {"min": 3, "max": 3}, "profile": "ui", "bridge_version": 2,
        "entry": {"path": "panel.html", "size": 1, "sha256": "0".repeat(64)}, "wasm": null,
        "contributions": [{"id": "board", "kind": "view", "slot": "project.view", "label": "Board",
                           "view": "board", "route": "board", "form_key": null}],
        "reads": [], "capabilities": []
    })
}

async fn package(db: &DatabaseConnection, project: Uuid, owner: Uuid, version: &str, manifest: Value) -> Uuid {
    let key = manifest["key"].as_str().expect("manifest key").to_owned();
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO form_plugin_packages (id,project_id,key,version,manifest,package_sha256,wasm_bytes,html,installed_by)
         VALUES ($1,$2,$3,$4,$5,$6,'\\x00','<p></p>',$7)",
        vec![
            id.into(),
            project.into(),
            key.into(),
            version.into(),
            manifest.into(),
            id.to_string().into(),
            owner.into(),
        ],
    )
    .await;
    id
}

async fn legacy_binding(db: &DatabaseConnection, project: Uuid, key: &str, package: Uuid, generation: i64) {
    exec(
        db,
        "INSERT INTO form_plugin_bindings (project_id,key,package_id,generation,grants)
         VALUES ($1,$2,$3,$4,ARRAY['amount.preview'])",
        vec![project.into(), key.into(), package.into(), generation.into()],
    )
    .await;
}

fn request(package_id: Option<Uuid>, expected_generation: i64, grants: &[&str]) -> BindRequest {
    BindRequest {
        package_id,
        expected_generation,
        request_id: Uuid::new_v4(),
        grants: grants.iter().map(|grant| (*grant).to_owned()).collect(),
        placements: None,
    }
}

#[tokio::test]
async fn a_binding_to_a_retired_protocol_package_reads_as_unbound() {
    let Some(scratch) = scratch("legacy_rows").await else {
        eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
        return;
    };
    let db = &scratch.db;
    let (workspace, owner, project) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    exec(
        db,
        "INSERT INTO users (id,email,password_hash,name,role,is_active) VALUES ($1,'o@legacy.test','!','o','user',true)",
        vec![owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspaces (id,slug,name,created_by) VALUES ($1,$2,'legacy',$3)",
        vec![workspace.into(), format!("ws-{workspace}").into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id,user_id,role) VALUES ($1,$2,'owner')",
        vec![workspace.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO projects (id,workspace_id,key,name,created_by,type_key) VALUES ($1,$2,'LEG','legacy',$3,'code_project')",
        vec![project.into(), workspace.into(), owner.into()],
    )
    .await;
    let actor = BindActor {
        workspace_id: workspace,
        actor_id: owner,
    };

    // `shared`: a retired package is bound; a protocol 3 package of the same key is installed.
    let legacy = package(db, project, owner, "1.0.0", v2_manifest("shared")).await;
    let current = package(db, project, owner, "3.0.0", v3_manifest("shared")).await;
    legacy_binding(db, project, "shared", legacy, 3).await;
    // `only_legacy`: nothing but a retired package.
    let only = package(db, project, owner, "1.0.0", v2_manifest("only_legacy")).await;
    legacy_binding(db, project, "only_legacy", only, 1).await;
    // `retired_layout`: the retired layout row must never be touched.
    exec(
        db,
        "INSERT INTO form_plugin_layouts (package_id,placements) VALUES ($1,$2)",
        vec![
            legacy.into(),
            json!([{"id":"a","enabled":true,"position":50},{"id":"b","enabled":true,"position":50}]).into(),
        ],
    )
    .await;

    let listed = catalog(db, project, true).await.expect("the catalog loads");
    assert_eq!(listed.items.len(), 1, "only the protocol 3 package is listed");
    assert_eq!(listed.items[0].id, current);
    for row in &listed.bindings {
        assert_eq!(row.package_id, None, "{} reads as unbound", row.key);
        assert!(row.grants.is_empty(), "{} carries no grants", row.key);
    }
    let shared = listed.bindings.iter().find(|row| row.key == "shared").expect("shared");
    assert_eq!(shared.generation, 3, "the generation the client must send is kept");

    // A retired package cannot be bound or disabled through, and nothing is written.
    for (key, target) in [
        ("shared", Some(legacy)),
        ("only_legacy", Some(only)),
        ("only_legacy", None),
    ] {
        let error = bind_key(db, actor, project, key, request(target, 1, &[]), true)
            .await
            .expect_err("a retired package is not installed as far as binding goes");
        assert_eq!(error.kind(), ApiErrorKind::NotFound, "{key} {target:?}");
    }
    assert_eq!(binding(db, project, "only_legacy").await.package_id, Some(only));
    assert_eq!(binding(db, project, "only_legacy").await.generation, 1);

    // A protocol 3 package takes over the key at the generation the list showed.
    let stale = bind_key(
        db,
        actor,
        project,
        "shared",
        request(Some(current), 0, &["ui.display"]),
        true,
    )
    .await
    .expect_err("a stale generation is refused");
    assert!(matches!(stale, ApiError::VersionConflict(_)));
    let bound = bind_key(
        db,
        actor,
        project,
        "shared",
        request(Some(current), 3, &["ui.display"]),
        true,
    )
    .await
    .expect("a v3 package binds over a retired binding");
    assert_eq!(bound["generation"], 4);
    assert_eq!(bound["package_id"], json!(current));
    let row = binding(db, project, "shared").await;
    assert_eq!(
        (row.package_id, row.generation, row.grants),
        (Some(current), 4, vec!["ui.display".to_owned()])
    );

    // Disabling a key still bound to a retired package clears it like an unbound key.
    let other_legacy = package(db, project, owner, "1.0.0", v2_manifest("cleared")).await;
    package(db, project, owner, "3.0.0", v3_manifest("cleared")).await;
    legacy_binding(db, project, "cleared", other_legacy, 2).await;
    let disabled = bind_key(db, actor, project, "cleared", request(None, 2, &[]), true)
        .await
        .expect("a disable request clears a retired binding");
    assert_eq!(disabled["generation"], 3);
    assert_eq!(binding(db, project, "cleared").await.package_id, None);

    // Binding needs the extensions switch; the retired layout row is untouched.
    let off = bind_key(db, actor, project, "cleared", request(Some(current), 3, &[]), false).await;
    assert!(off.is_err());
    let layouts = Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT count(*) AS n FROM form_plugin_layouts WHERE package_id=$1",
        vec![legacy.into()],
    );
    #[derive(FromQueryResult)]
    struct Count {
        n: i64,
    }
    assert_eq!(Count::find_by_statement(layouts).one(db).await.unwrap().unwrap().n, 1);
    scratch.drop_self().await;
}
