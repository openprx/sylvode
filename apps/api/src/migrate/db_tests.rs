use super::{
    MIGRATIONS, bootstrap_migration_ledger, migration_checksum, record_migration_if_absent, record_migration_with_conn,
    run_migrations,
};
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use std::collections::HashMap;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

/// A throwaway database plus the connection string needed to reach it again.
pub(crate) struct Scratch {
    db: DatabaseConnection,
    url: String,
    name: String,
    admin_url: String,
}

impl Scratch {
    /// Connection to the scratch database.
    pub(crate) fn connection(&self) -> &DatabaseConnection {
        &self.db
    }

    /// Connection string of the scratch database.
    pub(crate) fn url(&self) -> &str {
        &self.url
    }

    /// Second connection to the same database, for the concurrent start test.
    async fn second_connection(&self) -> anyhow::Result<DatabaseConnection> {
        Ok(Database::connect(&self.url).await?)
    }

    pub(crate) async fn drop_self(self) {
        let Self {
            db, name, admin_url, ..
        } = self;
        drop(db);
        let Ok(admin) = Database::connect(&admin_url).await else {
            return;
        };
        let statement = format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)");
        if let Err(err) = admin.execute_unprepared(&statement).await {
            eprintln!("could not drop scratch database {name}: {err}");
        }
    }
}

/// Creates an empty database named after the test. Returns `None` when the environment does
/// not offer a server, so the suite stays runnable without one.
pub(crate) async fn scratch(label: &str) -> Option<Scratch> {
    let admin_url = std::env::var(TEST_DATABASE_URL_ENV).ok()?;
    let admin = match Database::connect(&admin_url).await {
        Ok(admin) => admin,
        Err(err) => panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"),
    };

    let name = format!("sylvode_mig_{label}");
    let quoted = format!("\"{name}\"");
    if let Err(err) = admin
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS {quoted} WITH (FORCE)"))
        .await
    {
        panic!("could not reset scratch database {name}: {err}");
    }
    if let Err(err) = admin.execute_unprepared(&format!("CREATE DATABASE {quoted}")).await {
        panic!("could not create scratch database {name}: {err}");
    }

    let (prefix, _) = admin_url.rsplit_once('/')?;
    let url = format!("{prefix}/{name}");
    let db = match Database::connect(&url).await {
        Ok(db) => db,
        Err(err) => panic!("could not connect to scratch database {name}: {err}"),
    };
    Some(Scratch {
        db,
        url,
        name,
        admin_url,
    })
}

macro_rules! scratch_or_skip {
    ($label:expr) => {
        match scratch($label).await {
            Some(scratch) => scratch,
            None => {
                eprintln!("skipped: {} is not set", TEST_DATABASE_URL_ENV);
                return;
            }
        }
    };
}

/// Brings a database to the state before `stop_before` by applying and recording every earlier
/// file, so a test can start the runner on a database one release behind.
async fn seed_schema_before(db: &DatabaseConnection, stop_before: Option<&str>) {
    let (ledger_name, ledger_sql) = MIGRATIONS.first().expect("migrations exist");
    bootstrap_migration_ledger(db, ledger_name, ledger_sql)
        .await
        .expect("the ledger is created");
    record_migration_if_absent(db, ledger_name, &migration_checksum(ledger_sql))
        .await
        .expect("the ledger bootstrap is recorded");
    for (name, sql) in MIGRATIONS.iter().skip(1) {
        if stop_before.is_some_and(|limit| *name >= limit) {
            break;
        }
        if let Err(err) = db.execute_unprepared(sql).await {
            panic!("seeding {name} failed: {err}");
        }
        record_migration_with_conn(db, name, &migration_checksum(sql))
            .await
            .expect("the seeded file is recorded");
    }
}

async fn ledger(db: &DatabaseConnection) -> HashMap<String, String> {
    let rows = db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT name, status FROM schema_migrations".to_string(),
        ))
        .await
        .expect("the ledger is readable");
    let mut out = HashMap::with_capacity(rows.len());
    for row in rows {
        let name: String = row.try_get("", "name").expect("name column");
        let status: String = row.try_get("", "status").expect("status column");
        out.insert(name, status);
    }
    out
}

fn status_of<'a>(ledger: &'a HashMap<String, String>, name: &str) -> &'a str {
    ledger.get(name).map_or("<absent>", String::as_str)
}

#[tokio::test]
async fn a_fresh_database_applies_every_migration() {
    let scratch = scratch_or_skip!("fresh");

    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");

    let ledger = ledger(&scratch.db).await;
    assert_eq!(ledger.len(), MIGRATIONS.len(), "every migration is recorded");
    for (name, _) in MIGRATIONS {
        assert_eq!(status_of(&ledger, name), "applied", "{name} should have been executed");
    }

    scratch.drop_self().await;
}

/// Files are applied in name order on a fresh database.
#[tokio::test]
async fn a_fresh_database_applies_the_files_in_name_order() {
    let scratch = scratch_or_skip!("order_fresh");
    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");
    let applied = scalar_text(
        &scratch.db,
        "SELECT string_agg(name, ',' ORDER BY applied_at, name) AS v FROM schema_migrations",
    )
    .await;
    let expected: Vec<&str> = MIGRATIONS.iter().map(|(name, _)| *name).collect();
    assert_eq!(applied, expected.join(","), "ledger order is file name order");
    scratch.drop_self().await;
}

/// The runner decides per file from the ledger, not from the newest applied name. A file that
/// sorts before already applied ones (a migration merged after a higher number was applied) is
/// executed on the next start. Simulated with 0118 on a database whose newest file is applied:
/// its ledger row and its effect are removed, then the runner starts again.
#[tokio::test]
async fn a_file_that_sorts_before_applied_ones_is_applied_on_the_next_start() {
    let scratch = scratch_or_skip!("order_late_file");
    let db = &scratch.db;
    run_migrations(db).await.expect("a fresh database migrates cleanly");
    let late = "0118_label_rules.sql";
    let newest = MIGRATIONS.last().map(|(name, _)| *name).expect("migrations exist");
    assert!(late < newest, "the late file sorts before the newest applied one");

    db.execute_unprepared(
        "DROP INDEX labels_workspace_lower_name_key; \
         ALTER TABLE labels DROP CONSTRAINT labels_color_hex; \
         DELETE FROM schema_migrations WHERE name = '0118_label_rules.sql';",
    )
    .await
    .expect("the file is made absent");
    let newest_at = scalar_text(
        db,
        &format!("SELECT applied_at::text AS v FROM schema_migrations WHERE name = '{newest}'"),
    )
    .await;

    run_migrations(db).await.expect("the next start applies the late file");

    let ledger = ledger(db).await;
    assert_eq!(status_of(&ledger, late), "applied", "the late file was executed");
    assert_eq!(ledger.len(), MIGRATIONS.len());
    let index = scalar_text(
        db,
        "SELECT (to_regclass('labels_workspace_lower_name_key') IS NOT NULL)::text AS v",
    )
    .await;
    assert_eq!(index, "true", "its effect is back");
    let newest_after = scalar_text(
        db,
        &format!("SELECT applied_at::text AS v FROM schema_migrations WHERE name = '{newest}'"),
    )
    .await;
    assert_eq!(newest_after, newest_at, "files already applied are left alone");
    scratch.drop_self().await;
}

async fn scalar_text(db: &DatabaseConnection, sql: &str) -> String {
    db.query_one(Statement::from_string(DbBackend::Postgres, sql.to_string()))
        .await
        .expect("query runs")
        .expect("query returns a row")
        .try_get("", "v")
        .expect("v is text")
}

#[tokio::test]
async fn navigator_root_migration_fails_closed_on_duplicate_roots() {
    let scratch = scratch_or_skip!("navigator_root_duplicate");
    seed_schema_before(&scratch.db, Some("0059_flow_navigator_root.sql")).await;
    scratch
        .db
        .execute_unprepared(
            "INSERT INTO users (id, email, name, password_hash) VALUES \
             ('10000000-0000-0000-0000-000000000002', 'nr-duplicate@example.test', 'NR Duplicate', 'x'); \
             INSERT INTO workspaces (id, slug, name, created_by) VALUES \
             ('20000000-0000-0000-0000-000000000002', 'nr-duplicate', 'NR Duplicate', \
              '10000000-0000-0000-0000-000000000002'); \
             INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES \
             ('25000000-0000-0000-0000-000000000002', \
              '20000000-0000-0000-0000-000000000002', 'NRD', 'NR Duplicate', \
              '10000000-0000-0000-0000-000000000002'); \
             INSERT INTO flow_objects (id, workspace_id, object_type, project_id, parent_id) VALUES \
             ('30000000-0000-0000-0000-000000000002', \
              '20000000-0000-0000-0000-000000000002', 'navigator', \
              '25000000-0000-0000-0000-000000000002', NULL), \
             ('30000000-0000-0000-0000-000000000003', \
              '20000000-0000-0000-0000-000000000002', 'navigator', \
              '25000000-0000-0000-0000-000000000002', NULL);",
        )
        .await
        .expect("pre-0059 duplicate-root fixture is constructible");

    let err = scratch
        .db
        .execute_unprepared(include_str!("../../../../migrations/0059_flow_navigator_root.sql"))
        .await
        .expect_err("0059 must fail closed on duplicate canonical roots");
    assert!(
        err.to_string()
            .contains("project scope 25000000-0000-0000-0000-000000000002 has 2 navigator roots"),
        "{err}"
    );
    scratch.drop_self().await;
}

#[tokio::test]
async fn navigator_root_migration_materializes_project_scope_without_rewriting_it() {
    let scratch = scratch_or_skip!("navigator_root_scope");
    seed_schema_before(&scratch.db, Some("0059_flow_navigator_root.sql")).await;
    scratch
        .db
        .execute_unprepared(
            "INSERT INTO users (id, email, name, password_hash) VALUES \
             ('10000000-0000-0000-0000-000000000004', 'nr-scope@example.test', 'NR Scope', 'x'); \
             INSERT INTO workspaces (id, slug, name, created_by) VALUES \
             ('20000000-0000-0000-0000-000000000004', 'nr-scope', 'NR Scope', \
              '10000000-0000-0000-0000-000000000004'); \
             INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES \
             ('25000000-0000-0000-0000-000000000004', \
              '20000000-0000-0000-0000-000000000004', 'NR', 'NR Project', \
              '10000000-0000-0000-0000-000000000004'); \
             INSERT INTO flow_objects (id, workspace_id, object_type, project_id, parent_id) VALUES \
             ('30000000-0000-0000-0000-000000000004', \
              '20000000-0000-0000-0000-000000000004', 'page', \
              '25000000-0000-0000-0000-000000000004', NULL);",
        )
        .await
        .expect("pre-0059 incompatible projected root fixture is constructible");

    scratch
        .db
        .execute_unprepared(include_str!("../../../../migrations/0059_flow_navigator_root.sql"))
        .await
        .expect("0059 materializes the matching project root");
    let root = scratch
        .db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT page.project_id, page.parent_id, root.id AS root_id \
               FROM flow_objects page \
               JOIN flow_objects root ON root.id = page.parent_id \
              WHERE page.id = '30000000-0000-0000-0000-000000000004'::uuid \
                AND root.workspace_id = page.workspace_id \
                AND root.project_id IS NOT DISTINCT FROM page.project_id \
                AND flow_is_system_navigator_root( \
                      root.object_type, root.parent_id, root.governance_metadata \
                    )"
            .to_string(),
        ))
        .await
        .expect("project root query runs")
        .expect("project page is attached to its root");
    assert_eq!(
        root.try_get::<uuid::Uuid>("", "project_id").expect("page scope"),
        uuid::Uuid::parse_str("25000000-0000-0000-0000-000000000004").expect("literal uuid"),
        "the migration must preserve the projected page's scope"
    );
    assert_eq!(
        root.try_get::<uuid::Uuid>("", "parent_id").expect("page parent"),
        root.try_get::<uuid::Uuid>("", "root_id").expect("root id")
    );
    scratch.drop_self().await;
}

/// 0054 lands the whole Sylvode Flow v0.4 data layer in one migration: every table from
/// `contracts/domain-model-v1.md` plus the ADR-0011 delivery substrate must exist afterward.
#[tokio::test]
async fn flow_v04_data_layer_lands() {
    /// Every table the Sylvode Flow v0.4 data layer (migration 0054) must create.
    const FLOW_TABLES: &[&str] = &[
        "flow_objects",
        "flow_workspace_settings",
        "collab_documents",
        "collab_updates",
        "collab_tickets",
        "flow_relations",
        "flow_object_projections",
        "flow_projection_jobs",
        "flow_integrity_records",
        "flow_object_grants",
        "event_dispatch",
        "event_deliveries",
        "event_delivery_sources",
    ];

    let scratch = scratch_or_skip!("flow_v04");

    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");

    for table in FLOW_TABLES {
        let row = scratch
            .db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT to_regclass($1) IS NOT NULL AS present",
                vec![(*table).into()],
            ))
            .await
            .expect("probe query runs")
            .expect("probe query returns a row");
        let present: bool = row.try_get("", "present").expect("present column");
        assert!(present, "{table} must exist after migration 0054");
    }

    scratch.drop_self().await;
}

/// A failing migration stops the start without leaving a ledger row, and the next start retries
/// it once the cause is gone.
#[tokio::test]
async fn a_failing_migration_aborts_the_start_and_is_retried_later() {
    let scratch = scratch_or_skip!("failing");
    // 0005 creates `webhooks` without IF NOT EXISTS, so an unrelated table of that name is
    // enough to make exactly one migration fail.
    scratch
        .db
        .execute_unprepared("CREATE TABLE webhooks (blocker integer)")
        .await
        .expect("the blocker table is created");

    let err = run_migrations(&scratch.db)
        .await
        .expect_err("a failing migration must abort the start");
    assert!(
        err.to_string().contains("0005_webhooks.sql"),
        "the error has to name the migration, got: {err}"
    );

    let after_failure = ledger(&scratch.db).await;
    assert_eq!(status_of(&after_failure, "0005_webhooks.sql"), "<absent>");
    assert_eq!(status_of(&after_failure, "0004_sprints.sql"), "applied");

    scratch
        .db
        .execute_unprepared("DROP TABLE webhooks")
        .await
        .expect("the blocker table is dropped");
    run_migrations(&scratch.db)
        .await
        .expect("the retry succeeds once the cause is gone");
    let after_retry = ledger(&scratch.db).await;
    assert_eq!(status_of(&after_retry, "0005_webhooks.sql"), "applied");
    assert_eq!(after_retry.len(), MIGRATIONS.len());

    scratch.drop_self().await;
}

/// A `failed` row written by an older runner is retried: the file runs and the row is replaced.
#[tokio::test]
async fn a_failed_ledger_row_is_retried() {
    let scratch = scratch_or_skip!("failed_row");
    run_migrations(&scratch.db).await.expect("first start");
    // Make 0118 absent and leave a failed row for it, as the old runner did.
    scratch
        .db
        .execute_unprepared(
            "DROP INDEX labels_workspace_lower_name_key; \
             ALTER TABLE labels DROP CONSTRAINT labels_color_hex; \
             UPDATE schema_migrations SET status = 'failed', error = 'boom' WHERE name = '0118_label_rules.sql';",
        )
        .await
        .expect("the file is made absent");

    run_migrations(&scratch.db).await.expect("the failed file is retried");

    let ledger = ledger(&scratch.db).await;
    assert_eq!(status_of(&ledger, "0118_label_rules.sql"), "applied");
    let index = scalar_text(
        &scratch.db,
        "SELECT (to_regclass('labels_workspace_lower_name_key') IS NOT NULL)::text AS v",
    )
    .await;
    assert_eq!(index, "true", "its effect is back");
    scratch.drop_self().await;
}

/// A second start on a fully migrated database executes nothing and changes no ledger row.
#[tokio::test]
async fn a_second_start_is_a_no_op() {
    let scratch = scratch_or_skip!("second_start");
    run_migrations(&scratch.db).await.expect("first start");
    let before = scalar_text(
        &scratch.db,
        "SELECT string_agg(name || checksum || applied_at::text, ',' ORDER BY name) AS v FROM schema_migrations",
    )
    .await;
    run_migrations(&scratch.db).await.expect("second start");
    let after = scalar_text(
        &scratch.db,
        "SELECT string_agg(name || checksum || applied_at::text, ',' ORDER BY name) AS v FROM schema_migrations",
    )
    .await;
    assert_eq!(after, before, "the ledger is untouched by the second start");
    scratch.drop_self().await;
}

/// A recorded file whose content no longer matches the ledger stops the start.
#[tokio::test]
async fn a_changed_recorded_migration_stops_the_start() {
    let scratch = scratch_or_skip!("checksum_mismatch");
    run_migrations(&scratch.db).await.expect("first start");
    scratch
        .db
        .execute_unprepared("UPDATE schema_migrations SET checksum = 'edited' WHERE name = '0005_webhooks.sql'")
        .await
        .expect("the ledger is writable");
    let err = run_migrations(&scratch.db)
        .await
        .expect_err("a checksum mismatch must stop the start");
    assert!(
        err.to_string()
            .contains("0005_webhooks.sql changed after it was applied"),
        "got: {err}"
    );
    scratch.drop_self().await;
}

/// A ledger row without a shipped file stops the start.
#[tokio::test]
async fn a_recorded_migration_that_is_not_shipped_stops_the_start() {
    let scratch = scratch_or_skip!("recorded_not_shipped");
    run_migrations(&scratch.db).await.expect("first start");
    scratch
        .db
        .execute_unprepared(
            "INSERT INTO schema_migrations (name, checksum, status, applied_at) \
             VALUES ('9999_not_shipped.sql', 'x', 'applied', now())",
        )
        .await
        .expect("the ledger is writable");
    let err = run_migrations(&scratch.db)
        .await
        .expect_err("an unshipped record must stop the start");
    assert!(
        err.to_string()
            .contains("9999_not_shipped.sql is recorded but not shipped"),
        "got: {err}"
    );
    scratch.drop_self().await;
}

/// api and worker start together and both run the migrations. Without the advisory lock two
/// runners raced on the same DDL and one of them died on "relation already exists"; with
/// `restart: unless-stopped` in front of them that is a crash loop.
#[tokio::test]
async fn two_runners_starting_together_both_succeed() {
    let scratch = scratch_or_skip!("concurrent");
    let second = scratch.second_connection().await.expect("a second connection opens");

    let (first_result, second_result) = tokio::join!(run_migrations(&scratch.db), run_migrations(&second),);
    first_result.expect("the first runner succeeds");
    second_result.expect("the second runner succeeds");

    let ledger = ledger(&scratch.db).await;
    assert_eq!(ledger.len(), MIGRATIONS.len());
    assert!(!ledger.values().any(|status| status == "failed"));

    drop(second);
    scratch.drop_self().await;
}

/// The bootstrap used to resolve the ledger with `'public.schema_migrations'::regclass`,
/// which raises rather than returning NULL, so a deployment whose search_path does not
/// include public could not even create the ledger.
#[tokio::test]
async fn a_database_outside_the_public_schema_migrates() {
    let scratch = scratch_or_skip!("other_schema");
    scratch
        .db
        .execute_unprepared("CREATE SCHEMA app")
        .await
        .expect("the schema is created");
    scratch
        .db
        .execute_unprepared(&format!("ALTER DATABASE \"{}\" SET search_path TO app", scratch.name))
        .await
        .expect("the search_path is set");

    let relocated = scratch.second_connection().await.expect("a fresh connection opens");
    run_migrations(&relocated)
        .await
        .expect("a non public schema migrates cleanly");

    drop(relocated);
    scratch.drop_self().await;
}

/// Every operator-controlled column of the capability singleton, `updated_at` excluded so a
/// no-op re-run compares equal.
async fn flow_capability_row(db: &DatabaseConnection) -> serde_json::Value {
    let row = db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT (to_jsonb(c) - 'updated_at')::text AS row FROM flow_runtime_capabilities c WHERE singleton"
                .to_string(),
        ))
        .await
        .expect("the capability row is readable")
        .expect("the capability singleton exists");
    let text: String = row.try_get("", "row").expect("row column");
    serde_json::from_str(&text).expect("the capability row is JSON")
}

fn capability_flag(row: &serde_json::Value, column: &str) -> bool {
    row.get(column)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or_else(|| panic!("{column} is a boolean capability column: {row}"))
}

const DEFAULT_ON: [&str; 7] = [
    "personal_provisioning",
    "personal_writes",
    "external_mounts",
    "personal_forms_conversion",
    "block_transfers_enabled",
    "portable_page_packages_enabled",
    "portable_graph_packages_enabled",
];
/// Switched on by 0115_flow_default_on.sql, which also sets them on every fresh database
/// through the original CREATE/ADD COLUMN statements.
const DEFAULT_ON_SINCE_0115: [&str; 4] = [
    "scope_native_legacy",
    "personal_package_imports",
    "personal_scope_package_exports",
    "cross_workspace_forms_conversion",
];

/// What an ordinary user of a running installation does first: open the personal Flow home
/// and create a rich Page (the editor sends model5).
/// No capability is touched by hand.
async fn ordinary_user_creates_a_personal_page(db: &DatabaseConnection) -> Result<(), api::error::ApiError> {
    let user = uuid::Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Flow default user','user',true)",
        vec![user.into(), format!("{user}@flow-default.test").into()],
    ))
    .await
    .expect("the user is created");
    let (home, roots) = api::flow::scope::personal_home(db, user).await?;
    assert_eq!(home.kind, "personal");
    assert_eq!(home.owner_user_id, Some(user));
    assert_eq!(roots.len(), 1, "the personal home has exactly one navigator root");
    api::flow::scope::create_personal(
        db,
        user,
        api::flow::scope::CreateInput {
            editor_model: 5,
            scope_id: home.id,
            object_type: "page".to_string(),
            parent_object_id: None,
            project_id: None,
            title: "First personal Page".to_string(),
            idempotency_key: uuid::Uuid::new_v4().to_string(),
            message: None,
            initial_fields: vec![],
            initial_view: None,
        },
    )
    .await?;
    Ok(())
}

/// A fresh installation has personal Flow and the shipped Flow surfaces on without any
/// manual SQL, and an ordinary user can open their personal home and write a Page in it.
#[tokio::test]
async fn a_fresh_database_migrates_with_flow_capabilities_on() {
    let scratch = scratch_or_skip!("flow_caps_fresh");
    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");

    let row = flow_capability_row(&scratch.db).await;
    for column in DEFAULT_ON {
        assert!(capability_flag(&row, column), "{column} is on by default: {row}");
    }
    for column in DEFAULT_ON_SINCE_0115 {
        // 0149 renamed the first column; the list keeps the name 0115 used.
        let column = if column == "scope_native_legacy" {
            "workspace_editor_initialization"
        } else {
            column
        };
        assert!(capability_flag(&row, column), "{column} is on by default: {row}");
    }
    ordinary_user_creates_a_personal_page(&scratch.db)
        .await
        .expect("an ordinary user can use personal Flow on a fresh database");

    let caps = api::flow::scope::capabilities(&scratch.db)
        .await
        .expect("capabilities are readable");
    assert!(caps.personal_provisioning && caps.personal_writes && caps.external_mounts);
    assert!(caps.block_transfers_enabled, "block transfers are enabled");
    assert!(caps.portable_page_packages_enabled && caps.portable_graph_packages_enabled);

    scratch.drop_self().await;
}

const FLOW_DEFAULT_ON_MIGRATION: &str = "0115_flow_default_on.sql";

async fn insert_workspace(db: &DatabaseConnection, user: uuid::Uuid) -> uuid::Uuid {
    let workspace = uuid::Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO workspaces(id,slug,name,created_by) VALUES($1,$2,'Flow default',$3)",
        vec![workspace.into(), format!("fd-{workspace}").into(), user.into()],
    ))
    .await
    .expect("the workspace is created");
    workspace
}

async fn flow_enabled_row(db: &DatabaseConnection, workspace: uuid::Uuid) -> Option<bool> {
    db.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT flow_enabled FROM flow_workspace_settings WHERE workspace_id=$1",
        vec![workspace.into()],
    ))
    .await
    .expect("the settings row is readable")
    .map(|row| row.try_get::<bool>("", "flow_enabled").expect("flow_enabled column"))
}

async fn insert_user(db: &DatabaseConnection) -> uuid::Uuid {
    let user = uuid::Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO users(id,email,password_hash,name,role,is_active) VALUES($1,$2,'!','Flow default','user',true)",
        vec![user.into(), format!("{user}@flow-default.test").into()],
    ))
    .await
    .expect("the user is created");
    user
}

/// A fresh database gives every workspace a settings row with Flow on, and deleting the
/// workspace removes that row without error.
#[tokio::test]
async fn a_new_workspace_gets_a_flow_settings_row_with_flow_on() {
    let scratch = scratch_or_skip!("flow_ws_default");
    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");
    let user = insert_user(&scratch.db).await;
    let workspace = insert_workspace(&scratch.db, user).await;
    assert_eq!(flow_enabled_row(&scratch.db, workspace).await, Some(true));

    scratch
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM workspaces WHERE id=$1",
            vec![workspace.into()],
        ))
        .await
        .expect("a workspace with its settings row can be deleted");
    assert_eq!(flow_enabled_row(&scratch.db, workspace).await, None);
    scratch.drop_self().await;
}

/// An existing database that predates 0115 gets the four capabilities switched on once, a
/// settings row for every workspace that had none, and keeps a deliberate `flow_enabled=false`
/// row; a later operator switch-off survives executing the file again.
#[tokio::test]
async fn the_flow_default_on_migration_upgrades_an_existing_database_once() {
    let scratch = scratch_or_skip!("flow_default_on_upgrade");
    seed_schema_before(&scratch.db, Some(FLOW_DEFAULT_ON_MIGRATION)).await;
    let user = insert_user(&scratch.db).await;
    let without_row = insert_workspace(&scratch.db, user).await;
    let switched_off = insert_workspace(&scratch.db, user).await;
    scratch
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_workspace_settings(workspace_id,flow_enabled) VALUES($1,false) \
             ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled=false",
            vec![switched_off.into()],
        ))
        .await
        .expect("the deliberate switch-off is stored");
    scratch
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM flow_workspace_settings WHERE workspace_id=$1",
            vec![without_row.into()],
        ))
        .await
        .expect("the row of the old workspace is absent, as on an old database");
    scratch
        .db
        .execute_unprepared(
            "UPDATE flow_runtime_capabilities SET scope_native_legacy=false, personal_package_imports=false, \
             personal_scope_package_exports=false, cross_workspace_forms_conversion=false WHERE singleton",
        )
        .await
        .expect("the old defaults are restored");

    let sql = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == FLOW_DEFAULT_ON_MIGRATION)
        .map(|(_, sql)| *sql)
        .expect("0115 is registered");
    scratch
        .db
        .execute_unprepared(sql)
        .await
        .expect("0115 first execution succeeds");
    let first = flow_capability_row(&scratch.db).await;
    for column in DEFAULT_ON_SINCE_0115 {
        assert!(capability_flag(&first, column), "{column} is switched on: {first}");
    }
    assert_eq!(flow_enabled_row(&scratch.db, without_row).await, Some(true));
    assert_eq!(
        flow_enabled_row(&scratch.db, switched_off).await,
        Some(false),
        "a deliberate switch-off is not flipped"
    );
    let created_later = insert_workspace(&scratch.db, user).await;
    assert_eq!(flow_enabled_row(&scratch.db, created_later).await, Some(true));

    scratch
        .db
        .execute_unprepared("UPDATE flow_runtime_capabilities SET personal_scope_package_exports=false WHERE singleton")
        .await
        .expect("the operator switches an export capability off");
    let operator = flow_capability_row(&scratch.db).await;
    scratch
        .db
        .execute_unprepared(sql)
        .await
        .expect("0115 second execution succeeds");
    assert_eq!(
        flow_capability_row(&scratch.db).await,
        operator,
        "a second execution keeps the operator's choice"
    );
    assert_eq!(flow_enabled_row(&scratch.db, switched_off).await, Some(false));
    run_migrations(&scratch.db)
        .await
        .expect("the runner applies the files that are still unrecorded");
    scratch.drop_self().await;
}

/// 0117 repairs data that broke the sprint rules (two active sprints, names that differ only in
/// case, an end before the start, an unknown status), replaces the exact-case name constraint,
/// and does nothing the second time. The old state is rebuilt by dropping the new rules from a
/// fully migrated database.
#[tokio::test]
async fn sprint_rules_migration_fixes_data_and_reruns() {
    let scratch = scratch_or_skip!("sprint_rules_migration");
    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");
    let db = &scratch.db;
    db.execute_unprepared(
        "ALTER TABLE sprints DROP CONSTRAINT sprints_status_check; \
         ALTER TABLE sprints DROP CONSTRAINT sprints_dates_ordered; \
         DROP INDEX sprints_project_lower_name_key; \
         DROP INDEX sprints_one_active_per_project; \
         ALTER TABLE sprints ADD CONSTRAINT sprints_project_id_name_key UNIQUE (project_id, name)",
    )
    .await
    .expect("the pre-0117 rules are restored");

    let user = insert_user(db).await;
    let workspace = insert_workspace(db, user).await;
    let project = uuid::Uuid::new_v4();
    db.execute(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
         VALUES ($1, $2, 'SPR', 'sprints', $3, 'code_project')",
        vec![project.into(), workspace.into(), user.into()],
    ))
    .await
    .expect("the project is created");
    let [older_active, newer_active, bug_upper, bug_lower, backwards, bogus] =
        std::array::from_fn(|_| uuid::Uuid::new_v4());
    for (id, name, status, start, end, created, updated) in [
        (older_active, "Older", "active", None, None, "2030-01-01", "2030-01-01"),
        (newer_active, "Newer", "active", None, None, "2030-01-02", "2030-01-02"),
        (bug_upper, "Bug", "planned", None, None, "2030-01-03", "2030-01-03"),
        (bug_lower, "bug", "planned", None, None, "2030-01-04", "2030-01-04"),
        (
            backwards,
            "Backwards",
            "planned",
            Some("2030-03-10"),
            Some("2030-03-01"),
            "2030-01-05",
            "2030-01-05",
        ),
        (bogus, "Bogus", "paused", None, None, "2030-01-06", "2030-01-06"),
    ] {
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO sprints (id, project_id, name, status, start_date, end_date, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5::date, $6::date, $7::timestamptz, $8::timestamptz)",
            vec![
                id.into(),
                project.into(),
                name.into(),
                status.into(),
                start.map(str::to_string).into(),
                end.map(str::to_string).into(),
                created.into(),
                updated.into(),
            ],
        ))
        .await
        .expect("the old data is stored");
    }

    let sql = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == "0117_sprint_rules.sql")
        .map(|(_, sql)| *sql)
        .expect("0117 is registered");
    db.execute_unprepared(sql).await.expect("0117 first execution succeeds");

    let read = |id: uuid::Uuid| async move {
        let row = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT name, status, end_date::text AS end_date FROM sprints WHERE id = $1",
                vec![id.into()],
            ))
            .await
            .expect("the sprint is readable")
            .expect("the sprint exists");
        (
            row.try_get::<String>("", "name").expect("name"),
            row.try_get::<String>("", "status").expect("status"),
            row.try_get::<Option<String>>("", "end_date").expect("end_date"),
        )
    };
    assert_eq!(
        read(older_active).await.1,
        "planned",
        "the older active sprint is set back"
    );
    assert_eq!(read(newer_active).await.1, "active", "the newest active sprint stays");
    assert_eq!(read(bug_upper).await.0, "Bug", "the first of two names is kept");
    assert_eq!(
        read(bug_lower).await.0,
        format!("bug ({})", &bug_lower.to_string()[..8])
    );
    assert_eq!(
        read(backwards).await.2.as_deref(),
        Some("2030-03-10"),
        "end is lifted to start"
    );
    assert_eq!(read(bogus).await.1, "planned");
    let rules = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT conname FROM pg_constraint WHERE conrelid = 'sprints'::regclass ORDER BY conname",
            vec![],
        ))
        .await
        .expect("constraints are readable");
    let names: Vec<String> = rules
        .iter()
        .map(|row| row.try_get("", "conname").expect("conname"))
        .collect();
    assert!(
        !names.iter().any(|name| name == "sprints_project_id_name_key"),
        "{names:?}"
    );
    for expected in ["sprints_status_check", "sprints_dates_ordered"] {
        assert!(names.iter().any(|name| name == expected), "{expected} in {names:?}");
    }
    let indexes = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT indexname FROM pg_indexes WHERE tablename = 'sprints' AND indexname IN \
             ('sprints_project_lower_name_key', 'sprints_one_active_per_project')",
            vec![],
        ))
        .await
        .expect("indexes are readable");
    assert_eq!(indexes.len(), 2);

    // The rules now hold, and running the file again changes nothing.
    assert!(
        db.execute_unprepared(
            "UPDATE sprints SET status = 'active' WHERE id = (SELECT id FROM sprints WHERE name = 'Bogus')"
        )
        .await
        .is_err(),
        "a second active sprint is refused by the index"
    );
    let before = (
        read(older_active).await,
        read(newer_active).await,
        read(bug_lower).await,
        read(backwards).await,
        read(bogus).await,
    );
    db.execute_unprepared(sql)
        .await
        .expect("0117 second execution succeeds");
    let after = (
        read(older_active).await,
        read(newer_active).await,
        read(bug_lower).await,
        read(backwards).await,
        read(bogus).await,
    );
    assert_eq!(before, after, "a second execution changes nothing");
    scratch.drop_self().await;
}

/// 0118 rewrites colours that are not `#rrggbb`, lower-cases the rest, renames the later of two
/// labels whose names differ only in case, replaces the exact-case name constraint, and does
/// nothing the second time. The old state is rebuilt by dropping the new rules.
#[tokio::test]
async fn label_rules_migration_fixes_data_and_reruns() {
    let scratch = scratch_or_skip!("label_rules_migration");
    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");
    let db = &scratch.db;
    db.execute_unprepared(
        "ALTER TABLE labels DROP CONSTRAINT labels_color_hex; \
         DROP INDEX labels_workspace_lower_name_key; \
         ALTER TABLE labels ALTER COLUMN color SET DEFAULT '#gray'; \
         ALTER TABLE labels ADD CONSTRAINT labels_workspace_id_name_key UNIQUE (workspace_id, name)",
    )
    .await
    .expect("the pre-0118 rules are restored");

    let user = insert_user(db).await;
    let workspace = insert_workspace(db, user).await;
    let [gray, shouty, fine, bug_upper, bug_lower] = std::array::from_fn(|_| uuid::Uuid::new_v4());
    for (id, name, color, created) in [
        (gray, "gray one", "#gray", "2030-01-01"),
        (shouty, "shouty", "#AABBCC", "2030-01-02"),
        (fine, "fine", "#112233", "2030-01-03"),
        (bug_upper, "Bug", "#112233", "2030-01-04"),
        (bug_lower, "bug", "#112233", "2030-01-05"),
    ] {
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO labels (id, workspace_id, name, color, created_at) \
             VALUES ($1, $2, $3, $4, $5::timestamptz)",
            vec![id.into(), workspace.into(), name.into(), color.into(), created.into()],
        ))
        .await
        .expect("the old data is stored");
    }

    let sql = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == "0118_label_rules.sql")
        .map(|(_, sql)| *sql)
        .expect("0118 is registered");
    db.execute_unprepared(sql).await.expect("0118 first execution succeeds");

    let read = || async {
        let rows = db
            .query_all(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id::text AS id, name, color FROM labels WHERE workspace_id = $1 ORDER BY id",
                vec![workspace.into()],
            ))
            .await
            .expect("labels are readable");
        rows.iter()
            .map(|row| {
                (
                    row.try_get::<String>("", "id").expect("id"),
                    row.try_get::<String>("", "name").expect("name"),
                    row.try_get::<String>("", "color").expect("color"),
                )
            })
            .collect::<Vec<_>>()
    };
    let first = read().await;
    let label_row = |id: uuid::Uuid| {
        first
            .iter()
            .find(|row| row.0 == id.to_string())
            .expect("the label exists")
    };
    assert_eq!(label_row(gray).2, "#6b7280", "an invalid colour gets the default");
    assert_eq!(label_row(shouty).2, "#aabbcc", "colours are lower-cased");
    assert_eq!(label_row(fine).2, "#112233");
    assert_eq!(label_row(bug_upper).1, "Bug", "the first of two names is kept");
    assert_eq!(label_row(bug_lower).1, format!("bug ({})", &bug_lower.to_string()[..8]));
    let names: Vec<String> = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT conname FROM pg_constraint WHERE conrelid = 'labels'::regclass",
            vec![],
        ))
        .await
        .expect("constraints are readable")
        .iter()
        .map(|row| row.try_get("", "conname").expect("conname"))
        .collect();
    assert!(
        !names.iter().any(|name| name == "labels_workspace_id_name_key"),
        "{names:?}"
    );
    assert!(names.iter().any(|name| name == "labels_color_hex"), "{names:?}");
    assert!(
        db.execute_unprepared(&format!(
            "INSERT INTO labels (id, workspace_id, name, color) VALUES (gen_random_uuid(), '{workspace}', 'BUG', '#000000')"
        ))
        .await
        .is_err(),
        "the case-insensitive name index refuses a twin"
    );
    assert!(
        db.execute_unprepared(&format!(
            "INSERT INTO labels (id, workspace_id, name, color) VALUES (gen_random_uuid(), '{workspace}', 'new', 'red')"
        ))
        .await
        .is_err(),
        "the colour constraint refuses a non-hex colour"
    );

    db.execute_unprepared(sql)
        .await
        .expect("0118 second execution succeeds");
    assert_eq!(read().await, first, "a second execution changes nothing");
    scratch.drop_self().await;
}

const REGISTRATION_PROGRESS_MIGRATION: &str = "0107_flow_block_registration_progress.sql";

/// A database that already applied 0001-0106 and holds registry checkpoints gets the new column
/// with 0 on every existing row (those were written by complete registrations), and executing
/// the file again changes nothing. The 0106 state is reproduced by dropping the column from a
/// fully migrated database that has a registered Page.
#[tokio::test]
async fn the_registration_progress_migration_keeps_existing_checkpoints_complete() {
    let scratch = scratch_or_skip!("flow_registration_progress");
    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");
    ordinary_user_creates_a_personal_page(&scratch.db)
        .await
        .expect("a Page registers a checkpoint");
    let heads = scratch
        .db
        .query_all(Statement::from_string(
            DbBackend::Postgres,
            "SELECT accepted_head FROM flow_block_checkpoints ORDER BY page_id".to_string(),
        ))
        .await
        .expect("checkpoints readable");
    assert_eq!(heads.len(), 1, "the created Page has one checkpoint");
    scratch
        .db
        .execute_unprepared("ALTER TABLE flow_block_checkpoints DROP COLUMN pending_registrations")
        .await
        .expect("the 0106 state has no progress column");
    let sql = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == REGISTRATION_PROGRESS_MIGRATION)
        .map(|(_, sql)| *sql)
        .expect("0107 is registered");
    for _ in 0..2 {
        scratch.db.execute_unprepared(sql).await.expect("0107 executes");
        let row = scratch
            .db
            .query_one(Statement::from_string(
                DbBackend::Postgres,
                "SELECT pending_registrations, accepted_head FROM flow_block_checkpoints".to_string(),
            ))
            .await
            .expect("checkpoint readable")
            .expect("the checkpoint survives");
        assert_eq!(row.try_get::<i32>("", "pending_registrations").expect("column"), 0);
        assert_eq!(
            row.try_get::<i64>("", "accepted_head").expect("column"),
            heads
                .first()
                .expect("one checkpoint")
                .try_get::<i64>("", "accepted_head")
                .expect("column")
        );
    }
    assert!(
        scratch
            .db
            .execute_unprepared("UPDATE flow_block_checkpoints SET pending_registrations=-1")
            .await
            .is_err(),
        "a negative pending count is refused"
    );
    scratch.drop_self().await;
}

/// 0127 drops the anchor seal column and the other retired objects; every statement of it can run
/// again without effect.
#[tokio::test]
async fn the_legacy_objects_migration_retires_what_it_names_and_reruns() {
    let scratch = scratch_or_skip!("legacy_objects_migration");
    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");
    let db = &scratch.db;
    let sealed_column = || async {
        db.query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM pg_attribute WHERE attrelid = to_regclass('flow_block_checkpoints') \
             AND attname = 'legacy_anchors_sealed' AND NOT attisdropped"
                .to_string(),
        ))
        .await
        .expect("the column catalogue is readable")
        .expect("count query returns a row")
        .try_get::<i64>("", "n")
        .expect("n column")
    };
    assert_eq!(sealed_column().await, 0, "the seal column is gone");
    let sql = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == "0127_retire_legacy_objects.sql")
        .map(|(_, sql)| *sql)
        .expect("0127 is registered");
    db.execute_unprepared(sql)
        .await
        .expect("0127 second execution succeeds");
    assert_eq!(sealed_column().await, 0, "a second execution changes nothing");
    scratch.drop_self().await;
}

/// 0127 leaves `flow_package` as the only import kind and refuses to run over a legacy import
/// row without deleting anything.
#[tokio::test]
async fn the_legacy_objects_migration_keeps_only_package_imports() {
    let scratch = scratch_or_skip!("legacy_objects_imports");
    run_migrations(&scratch.db)
        .await
        .expect("a fresh database migrates cleanly");
    let db = &scratch.db;
    let present = |relation: &'static str| async move {
        db.query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT to_regclass($1) IS NOT NULL AS present",
            vec![relation.into()],
        ))
        .await
        .expect("probe query runs")
        .expect("probe query returns a row")
        .try_get::<bool>("", "present")
        .expect("present column")
    };
    assert!(!present("pages").await, "the legacy pages table is gone");
    assert!(
        !present("form_plugin_layouts").await,
        "the retired plugin layout table is gone"
    );

    let user = insert_user(db).await;
    let workspace = insert_workspace(db, user).await;
    let insert_job = |kind: &'static str, key: &'static str| async move {
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO flow_import_jobs (workspace_id, kind, mapping_hash, idempotency_key, request_body_hash) \
             VALUES ($1, $2, 'h', $3, repeat('0', 64))",
            vec![workspace.into(), kind.into(), key.into()],
        ))
        .await
    };
    assert!(
        insert_job("legacy_pages", "legacy").await.is_err(),
        "a legacy import job is refused by the check"
    );
    insert_job("flow_package", "package")
        .await
        .expect("a package import job is stored");

    // A legacy row that predates the check stops the migration and nothing is deleted.
    db.execute_unprepared("ALTER TABLE flow_import_jobs DROP CONSTRAINT flow_import_jobs_kind_check")
        .await
        .expect("the check is lifted for the fixture");
    insert_job("legacy_pages", "legacy")
        .await
        .expect("the legacy row is stored");
    let sql = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == "0127_retire_legacy_objects.sql")
        .map(|(_, sql)| *sql)
        .expect("0127 is registered");
    let refusal = db
        .execute_unprepared(sql)
        .await
        .expect_err("a legacy import row stops the migration");
    assert!(refusal.to_string().contains("legacy_pages"), "{refusal}");
    let rows = db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*) AS n FROM flow_import_jobs".to_string(),
        ))
        .await
        .expect("count query runs")
        .expect("count query returns a row")
        .try_get::<i64>("", "n")
        .expect("n column");
    assert_eq!(rows, 2, "the refusal deletes nothing");

    db.execute_unprepared("DELETE FROM flow_import_jobs WHERE kind = 'legacy_pages'")
        .await
        .expect("the legacy row is resolved");
    db.execute_unprepared(sql)
        .await
        .expect("0127 runs again once it is resolved");
    assert!(insert_job("legacy_pages", "again").await.is_err(), "the check is back");
    scratch.drop_self().await;
}

/// The low-code migrations (0133-0143) apply on a fresh database, each re-runs as a no-op.
/// 0136 also rewrites a v1 form schema it finds.
#[tokio::test]
async fn low_code_migrations_apply_and_rerun() {
    let scratch = scratch_or_skip!("low_code_rerun");
    let db = scratch.connection();
    run_migrations(db).await.expect("a fresh database migrates cleanly");
    let low_code: Vec<(&str, &str)> = MIGRATIONS
        .iter()
        .copied()
        .filter(|(name, _)| ("0133".."0144").contains(&name.get(..4).unwrap_or_default()))
        .collect();
    assert_eq!(low_code.len(), 11, "0133 through 0143 are registered");
    // 0141 drops `scenario_templates`, which 0136 still rewrites when it runs on its own, so the
    // table (an idempotent file, 0029) is back before each stand-alone re-run that needs it.
    let restore_scenario_templates = || async {
        let sql = MIGRATIONS
            .iter()
            .find(|(name, _)| name.starts_with("0029"))
            .map(|(_, sql)| *sql)
            .expect("0029 is registered");
        db.execute_unprepared(sql)
            .await
            .expect("0029 recreates scenario_templates");
    };
    restore_scenario_templates().await;
    for (name, sql) in &low_code {
        db.execute_unprepared(&format!("BEGIN; {sql}\nCOMMIT;"))
            .await
            .unwrap_or_else(|err| panic!("{name} does not re-run: {err}"));
    }

    restore_scenario_templates().await;
    db.execute_unprepared(
        r#"INSERT INTO scenario_templates (key, name, project_type_key, field_schema)
           SELECT 'mig_v1_check', 'v1 check', key,
                  '{"version":"openpr.form.schema.v1","fields":[
                     {"key":"qty","type":"integer","validation":{"min":1,"max":"10"},"default_value":2},
                     {"key":"code","type":"text","validation":{"pattern":"[A-Z]+","max_length":"8"},
                      "policy":{"read_only":true}},
                     {"key":"flag","type":"boolean","validation":{"max_length":3}},
                     {"key":"when","type":"datetime","validation":{"min":"2026-01-01T10:00"}}]}'::jsonb
             FROM project_types LIMIT 1"#,
    )
    .await
    .expect("a v1 template row is inserted");
    let sql_0136 = low_code
        .iter()
        .find(|(name, _)| name.starts_with("0136"))
        .map(|(_, sql)| *sql)
        .expect("0136 is registered");
    db.execute_unprepared(&format!("BEGIN; {sql_0136}\nCOMMIT;"))
        .await
        .expect("0136 rewrites the v1 row");
    let rewritten: serde_json::Value = serde_json::from_str(
        &scalar_text(
            db,
            "SELECT field_schema::text AS v FROM scenario_templates WHERE key = 'mig_v1_check'",
        )
        .await,
    )
    .expect("the rewritten schema is JSON");
    assert_eq!(rewritten["version"], "sv.form.schema.v2");
    let fields = &rewritten["fields"];
    assert_eq!(
        fields[0]["rules"]["validate"],
        serde_json::json!([{"rule": "min", "value": 1}, {"rule": "max", "value": 10}])
    );
    assert_eq!(fields[0]["default"], serde_json::json!({"value": 2}));
    assert!(fields[0].get("validation").is_none() && fields[0].get("default_value").is_none());
    assert_eq!(
        fields[1]["rules"]["validate"],
        serde_json::json!([{"rule": "pattern", "value": "^(?:[A-Z]+)$"}, {"rule": "max_length", "value": 8}])
    );
    assert_eq!(fields[1]["read_only"], true);
    assert!(fields[1].get("policy").is_none());
    assert!(
        fields[2].get("rules").is_none(),
        "a rule that does not fit the type is dropped"
    );
    assert_eq!(
        fields[3]["rules"]["validate"],
        serde_json::json!([{"rule": "min", "value": "2026-01-01T10:00:00Z"}])
    );

    scratch.drop_self().await;
}

/// 0142 cuts a stored record title longer than the renderer's cap to its first 1,024 characters
/// (characters, not bytes: the title is multi-byte), leaves a title within the cap alone, and a
/// second run changes nothing.
#[tokio::test]
async fn record_title_cap_migration_cuts_on_characters_and_reruns() {
    let scratch = scratch_or_skip!("record_title_cap");
    let db = scratch.connection();
    run_migrations(db).await.expect("a fresh database migrates cleanly");
    db.execute_unprepared(
        "INSERT INTO users (id, email, name, password_hash) VALUES \
         ('10000000-0000-0000-0000-000000000142', 'title-cap@example.test', 'Title Cap', 'x'); \
         INSERT INTO workspaces (id, slug, name, created_by) VALUES \
         ('20000000-0000-0000-0000-000000000142', 'title-cap', 'Title Cap', \
          '10000000-0000-0000-0000-000000000142'); \
         INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES \
         ('25000000-0000-0000-0000-000000000142', '20000000-0000-0000-0000-000000000142', \
          'TCP', 'Title Cap', '10000000-0000-0000-0000-000000000142'); \
         INSERT INTO project_forms (id, workspace_id, project_id, key, name, title_template, schema) VALUES \
         ('30000000-0000-0000-0000-000000000142', '20000000-0000-0000-0000-000000000142', \
          '25000000-0000-0000-0000-000000000142', 'tc', 'Title Cap', '{id}', \
          '{\"version\":\"sv.form.schema.v2\",\"fields\":[]}'::jsonb); \
         INSERT INTO form_records (id, workspace_id, project_id, form_id, title) VALUES \
         ('40000000-0000-0000-0000-000000000001', '20000000-0000-0000-0000-000000000142', \
          '25000000-0000-0000-0000-000000000142', '30000000-0000-0000-0000-000000000142', \
          repeat('\u{00e9}', 1500)), \
         ('40000000-0000-0000-0000-000000000002', '20000000-0000-0000-0000-000000000142', \
          '25000000-0000-0000-0000-000000000142', '30000000-0000-0000-0000-000000000142', \
          repeat('a', 1024));",
    )
    .await
    .expect("a form with a long and a capped title is inserted");
    let sql_0142 = MIGRATIONS
        .iter()
        .find(|(name, _)| name.starts_with("0142"))
        .map(|(_, sql)| *sql)
        .expect("0142 is registered");
    let lengths = "SELECT string_agg(char_length(title)::text || ':' || octet_length(title)::text, ',' \
                   ORDER BY id) AS v FROM form_records WHERE form_id = '30000000-0000-0000-0000-000000000142'";
    for run in ["first", "second"] {
        db.execute_unprepared(&format!("BEGIN; {sql_0142}\nCOMMIT;"))
            .await
            .unwrap_or_else(|err| panic!("the {run} run of 0142 fails: {err}"));
        assert_eq!(
            scalar_text(db, lengths).await,
            "1024:2048,1024:1024",
            "after the {run} run the long title holds 1,024 two-byte characters and the other is untouched"
        );
    }
    scratch.drop_self().await;
}

/// 0143 narrows the work item hint trigger to rows that actually changed, and re-runs as a no-op.
#[tokio::test]
async fn work_item_hint_trigger_fires_only_for_a_changed_row_and_reruns() {
    let scratch = scratch_or_skip!("work_item_hint_when");
    let db = scratch.connection();
    run_migrations(db).await.expect("a fresh database migrates cleanly");
    let sql_0143 = MIGRATIONS
        .iter()
        .find(|(name, _)| name.starts_with("0143"))
        .map(|(_, sql)| *sql)
        .expect("0143 is registered");
    let definition = "SELECT pg_get_triggerdef(oid) AS v FROM pg_trigger \
                      WHERE tgname = 'trg_sv_hint_work_item_update' AND NOT tgisinternal";
    for run in ["first", "second"] {
        db.execute_unprepared(&format!("BEGIN; {sql_0143}\nCOMMIT;"))
            .await
            .unwrap_or_else(|err| panic!("the {run} run of 0143 fails: {err}"));
        let def = scalar_text(db, definition).await;
        assert!(
            def.contains("WHEN") && def.contains("IS DISTINCT FROM"),
            "after the {run} run the update trigger is conditional: {def}"
        );
    }
    scratch.drop_self().await;
}
