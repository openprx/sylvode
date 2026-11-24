use super::{MIGRATIONS, MigrationOptions, migration_is_present, run_migrations_with};
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

/// Reproduces the pre-ledger runner: every file executed straight against the database with
/// no ledger to record it. `stop_before` leaves the database half built.
async fn seed_pre_ledger_schema(db: &DatabaseConnection, stop_before: Option<&str>) {
    for (name, sql) in MIGRATIONS.iter().skip(1) {
        if stop_before.is_some_and(|limit| *name >= limit) {
            break;
        }
        if let Err(err) = db.execute_unprepared(sql).await {
            panic!("seeding {name} failed: {err}");
        }
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

/// Every object the ledger claims is present has to be there.
async fn assert_schema_complete(db: &DatabaseConnection) {
    for (name, _) in MIGRATIONS {
        let present = migration_is_present(db, name).await.expect("probe runs");
        let probe_exists = super::migration_probe(name).is_some_and(|probe| probe.statement().is_some());
        assert!(!probe_exists || present, "{name} left no trace in the database");
    }
}

#[tokio::test]
async fn a_fresh_database_applies_every_migration() {
    let scratch = scratch_or_skip!("fresh");

    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("a fresh database migrates cleanly");

    let ledger = ledger(&scratch.db).await;
    assert_eq!(ledger.len(), MIGRATIONS.len(), "every migration is recorded");
    for (name, _) in MIGRATIONS {
        assert_eq!(status_of(&ledger, name), "applied", "{name} should have been executed");
    }
    assert_schema_complete(&scratch.db).await;

    scratch.drop_self().await;
}

/// Files are applied in name order on a fresh database.
#[tokio::test]
async fn a_fresh_database_applies_the_files_in_name_order() {
    let scratch = scratch_or_skip!("order_fresh");
    run_migrations_with(&scratch.db, MigrationOptions::default())
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
    run_migrations_with(db, MigrationOptions::default())
        .await
        .expect("a fresh database migrates cleanly");
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

    run_migrations_with(db, MigrationOptions::default())
        .await
        .expect("the next start applies the late file");

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

async fn config_version_of(db: &DatabaseConnection, project: &str, bot: &str) -> String {
    scalar_text(
        db,
        &format!(
            "SELECT config_updated_at::text AS v FROM ai_participants \
             WHERE project_id = '{project}' AND id = '{bot}'"
        ),
    )
    .await
}

/// 0112 is after the adoption cutoff, so it re-runs on an adopted database, and 0092 (edited so
/// that its trigger no longer names the dropped columns) re-runs after it on a replay start.
/// Both must succeed any number of times and leave the same schema and the same versions.
#[tokio::test]
async fn bot_connections_migration_reruns_and_replaying_0092_after_it_succeeds() {
    let scratch = scratch_or_skip!("bot_connections_replay");
    let db = &scratch.db;
    run_migrations_with(db, MigrationOptions::default())
        .await
        .expect("a fresh database migrates cleanly");

    let dropped = scalar_text(
        db,
        "SELECT count(*)::text AS v FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = 'ai_participants' \
           AND column_name IN ('model', 'provider', 'api_endpoint', 'capabilities')",
    )
    .await;
    assert_eq!(dropped, "0", "the four stored-only columns are gone");
    let key = scalar_text(
        db,
        "SELECT string_agg(a.attname, ',' ORDER BY k.ord) AS v \
         FROM pg_index i \
         CROSS JOIN LATERAL unnest(i.indkey) WITH ORDINALITY AS k(attnum, ord) \
         JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum \
         WHERE i.indrelid = 'ai_participants'::regclass AND i.indisprimary",
    )
    .await;
    assert_eq!(key, "project_id,id", "one registration per bot per project");
    let learning_fk = scalar_text(
        db,
        "SELECT count(*)::text AS v FROM pg_constraint \
         WHERE conrelid = 'ai_learning_records'::regclass AND contype = 'f' \
           AND confrelid = 'ai_participants'::regclass",
    )
    .await;
    assert_eq!(
        learning_fk, "0",
        "learning records keep no foreign key to the participant"
    );

    // A bot registered in two projects.
    let user = "00000000-0000-0000-0000-0000000000a1";
    let ws = "00000000-0000-0000-0000-0000000000b1";
    let (p1, p2) = (
        "00000000-0000-0000-0000-0000000000c1",
        "00000000-0000-0000-0000-0000000000c2",
    );
    let bot = "bot-registered-twice";
    let setup = format!(
        "INSERT INTO users(id,email,password_hash,name,role,is_active) \
           VALUES ('{user}','a1@example.test','','Owner','user',true); \
         INSERT INTO workspaces(id,slug,name,created_by) VALUES ('{ws}','bc-ws','BC',  '{user}'); \
         INSERT INTO projects(id,workspace_id,key,name,created_by) VALUES \
           ('{p1}','{ws}','BCA','A','{user}'),('{p2}','{ws}','BCB','B','{user}'); \
         INSERT INTO ai_participants(id,project_id,name,registered_by) VALUES \
           ('{bot}','{p1}','Bot','{user}'),('{bot}','{p2}','Bot','{user}');"
    );
    db.execute_unprepared(&setup)
        .await
        .expect("the same bot id registers in two projects");

    let before = config_version_of(db, p1, bot).await;
    let heartbeat = format!("UPDATE ai_participants SET last_active_at = clock_timestamp() WHERE project_id = '{p1}'");
    db.execute_unprepared(&heartbeat).await.expect("heartbeat");
    assert_eq!(
        config_version_of(db, p1, bot).await,
        before,
        "a heartbeat is not a configuration edit"
    );
    let rename = format!("UPDATE ai_participants SET name = 'Renamed' WHERE project_id = '{p1}'");
    db.execute_unprepared(&rename).await.expect("rename");
    let after = config_version_of(db, p1, bot).await;
    assert_ne!(after, before, "a configuration edit advances the version");

    for (name, sql) in [
        (
            "0112 again",
            include_str!("../../../../migrations/0112_bot_connections.sql"),
        ),
        (
            "0092 after 0112",
            include_str!("../../../../migrations/0092_ai_participant_config_version.sql"),
        ),
        (
            "0112 after 0092",
            include_str!("../../../../migrations/0112_bot_connections.sql"),
        ),
    ] {
        db.execute_unprepared(sql)
            .await
            .unwrap_or_else(|err| panic!("{name} must succeed: {err}"));
        assert_eq!(
            config_version_of(db, p1, bot).await,
            after,
            "{name} preserves versions and rows"
        );
    }
    let trigger = scalar_text(
        db,
        "SELECT pg_get_triggerdef(oid) AS v FROM pg_trigger \
         WHERE tgrelid = 'ai_participants'::regclass AND tgname = 'ai_participant_config_version'",
    )
    .await;
    for column in ["model", "provider", "api_endpoint", "capabilities"] {
        assert!(
            !trigger.contains(column),
            "the trigger must not name {column}: {trigger}"
        );
    }
    for table in ["bot_runners", "project_bot_connections"] {
        let present = scalar_text(db, &format!("SELECT (to_regclass('{table}') IS NOT NULL)::text AS v")).await;
        assert_eq!(present, "true", "{table} exists");
    }

    scratch.drop_self().await;
}

async fn flow_forms_bridge_schema_snapshot(db: &DatabaseConnection) -> String {
    db.query_one(Statement::from_string(
        DbBackend::Postgres,
        "SELECT jsonb_build_object( \
             'columns', ( \
               SELECT coalesce(jsonb_agg(jsonb_build_array(table_name, column_name, data_type, \
                 is_nullable, column_default) ORDER BY table_name, ordinal_position), '[]'::jsonb) \
               FROM information_schema.columns \
               WHERE table_schema = current_schema() AND ( \
                 table_name IN ('flow_bridge_references', 'flow_conversion_previews', \
                   'flow_conversion_jobs', 'flow_object_lineage') OR \
                 (table_name = 'workspace_bots' AND column_name = 'transport_surface') OR \
                 (table_name = 'flow_workspace_settings' AND column_name = 'bridge_enabled') \
               ) \
             ), \
             'constraints', ( \
               SELECT coalesce(jsonb_agg(jsonb_build_array(rel.relname, con.conname, \
                 pg_get_constraintdef(con.oid)) ORDER BY rel.relname, con.conname), '[]'::jsonb) \
               FROM pg_constraint con JOIN pg_class rel ON rel.oid = con.conrelid \
               JOIN pg_namespace ns ON ns.oid = rel.relnamespace \
               WHERE ns.nspname = current_schema() AND rel.relname IN ( \
                 'workspace_bots', 'flow_bridge_references', 'flow_conversion_previews', \
                 'flow_conversion_jobs', 'flow_object_lineage') \
             ), \
             'indexes', ( \
               SELECT coalesce(jsonb_agg(jsonb_build_array(tablename, indexname, indexdef) \
                 ORDER BY tablename, indexname), '[]'::jsonb) \
               FROM pg_indexes WHERE schemaname = current_schema() AND tablename IN ( \
                 'flow_bridge_references', 'flow_conversion_previews', \
                 'flow_conversion_jobs', 'flow_object_lineage') \
             ), \
             'view', pg_get_viewdef('flow_forms_bridge_schema_guard'::regclass, true), \
             'rows', jsonb_build_array( \
               (SELECT count(*) FROM flow_bridge_references), \
               (SELECT count(*) FROM flow_conversion_previews), \
               (SELECT count(*) FROM flow_conversion_jobs), \
               (SELECT count(*) FROM flow_object_lineage) \
             ) \
           )::text AS snapshot"
            .to_string(),
    ))
    .await
    .expect("bridge schema snapshot query runs")
    .expect("bridge schema snapshot query returns one row")
    .try_get("", "snapshot")
    .expect("bridge schema snapshot is text")
}

/// 0062 is after the adoption cutoff, so an existing database may execute it after some or
/// all of its objects already exist. Execute the exact migration twice and compare the
/// complete bridge schema plus row counts; syntax-only `IF NOT EXISTS` scans are insufficient.
#[tokio::test]
async fn flow_forms_bridge_migration_is_replay_safe() {
    let scratch = scratch_or_skip!("flow_forms_bridge_replay");
    seed_pre_ledger_schema(&scratch.db, Some("0062_flow_forms_bridge.sql")).await;

    let migration = std::env::var("OPENPR_TEST_FLOW_BRIDGE_MIGRATION_PATH").map_or_else(
        |_| include_str!("../../../../migrations/0062_flow_forms_bridge.sql").to_string(),
        |path| std::fs::read_to_string(path).expect("test migration override is readable"),
    );
    scratch
        .db
        .execute_unprepared(&migration)
        .await
        .expect("0062 first execution succeeds");
    let first = flow_forms_bridge_schema_snapshot(&scratch.db).await;

    scratch
        .db
        .execute_unprepared(&migration)
        .await
        .expect("0062 second execution succeeds");
    let second = flow_forms_bridge_schema_snapshot(&scratch.db).await;
    assert_eq!(second, first, "0062 replay must preserve schema objects and row counts");

    scratch.drop_self().await;
}

/// ADR-0018 NR-1..NR-4 is a data migration, not just a final CHECK constraint: it must create
/// one addressable root aggregate, reparent compatible legacy rows, survive replay, and make
/// the old NULL-parent page shape impossible afterward.
#[tokio::test]
async fn navigator_root_migration_materializes_reparents_and_replays() {
    let scratch = scratch_or_skip!("navigator_root_replay");
    seed_pre_ledger_schema(&scratch.db, Some("0059_flow_navigator_root.sql")).await;

    scratch
        .db
        .execute_unprepared(
            "INSERT INTO users (id, email, name, password_hash) VALUES \
             ('10000000-0000-0000-0000-000000000001', 'nr-replay@example.test', 'NR Replay', 'x'); \
             INSERT INTO workspaces (id, slug, name, created_by) VALUES \
             ('20000000-0000-0000-0000-000000000001', 'nr-replay', 'NR Replay', \
              '10000000-0000-0000-0000-000000000001'); \
             INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES \
             ('25000000-0000-0000-0000-000000000001', \
              '20000000-0000-0000-0000-000000000001', 'NRP', 'NR Project', \
              '10000000-0000-0000-0000-000000000001'); \
             INSERT INTO flow_objects (id, workspace_id, object_type, project_id, parent_id) VALUES \
             ('30000000-0000-0000-0000-000000000001', \
              '20000000-0000-0000-0000-000000000001', 'page', NULL, NULL), \
             ('30000000-0000-0000-0000-000000000011', \
              '20000000-0000-0000-0000-000000000001', 'page', \
              '25000000-0000-0000-0000-000000000001', NULL), \
             ('30000000-0000-0000-0000-000000000012', \
              '20000000-0000-0000-0000-000000000001', 'navigator', \
              '25000000-0000-0000-0000-000000000001', NULL); \
             UPDATE flow_objects \
                SET governance_metadata = '{\"legacy_key\":\"preserved\"}'::jsonb \
              WHERE id = '30000000-0000-0000-0000-000000000012'; \
             INSERT INTO collab_documents ( \
               id, object_id, engine, format_version, snapshot, snapshot_frontier, snapshot_seq, \
               head_frontier, head_seq, byte_count, update_count \
             ) VALUES ( \
               '35000000-0000-0000-0000-000000000012', \
               '30000000-0000-0000-0000-000000000012', 'loro', 'loro-1', \
               decode('00','hex'), decode('00','hex'), 0, decode('00','hex'), 0, 1, 0 \
             ); \
             INSERT INTO flow_object_projections ( \
               object_id, document_seq, document_frontier, title, state, plain_text, projection_version \
             ) VALUES ( \
               '30000000-0000-0000-0000-000000000012', 0, decode('00','hex'), \
               'Legacy navigator', '{\"nodes\":{}}'::jsonb, '', 1 \
             );",
        )
        .await
        .expect("legacy compatible root page fixture is valid before 0059");

    let migration = include_str!("../../../../migrations/0059_flow_navigator_root.sql");
    scratch
        .db
        .execute_unprepared(migration)
        .await
        .expect("0059 materializes the root and reparents the compatible page");

    let first = scratch
        .db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT root.id AS root_id, page.parent_id, \
                    count(doc.id)::bigint AS document_count, \
                    count(projection.object_id)::bigint AS projection_count \
               FROM flow_objects root \
               JOIN flow_objects page ON page.id = \
                    '30000000-0000-0000-0000-000000000001'::uuid \
               LEFT JOIN collab_documents doc ON doc.object_id = root.id \
               LEFT JOIN flow_object_projections projection ON projection.object_id = root.id \
              WHERE root.workspace_id = '20000000-0000-0000-0000-000000000001'::uuid \
                AND root.object_type = 'navigator' AND root.project_id IS NULL \
                AND root.parent_id IS NULL \
              GROUP BY root.id, page.parent_id"
                .to_string(),
        ))
        .await
        .expect("root aggregate query runs")
        .expect("root aggregate exists");
    let root_id: uuid::Uuid = first.try_get("", "root_id").expect("root id");
    let parent_id: uuid::Uuid = first.try_get("", "parent_id").expect("page parent id");
    assert_eq!(parent_id, root_id, "legacy top-level page must be attached to the root");
    assert_eq!(first.try_get::<i64>("", "document_count").expect("document count"), 1);
    assert_eq!(
        first.try_get::<i64>("", "projection_count").expect("projection count"),
        1
    );

    let adopted = scratch
        .db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT page.parent_id, page.project_id, root.governance_metadata, \
                    count(doc.id)::bigint AS document_count, \
                    count(projection.object_id)::bigint AS projection_count \
               FROM flow_objects root \
               JOIN flow_objects page ON page.id = \
                    '30000000-0000-0000-0000-000000000011'::uuid \
               LEFT JOIN collab_documents doc ON doc.object_id = root.id \
               LEFT JOIN flow_object_projections projection ON projection.object_id = root.id \
              WHERE root.id = '30000000-0000-0000-0000-000000000012'::uuid \
              GROUP BY page.parent_id, page.project_id, root.governance_metadata"
                .to_string(),
        ))
        .await
        .expect("adopted project root query runs")
        .expect("adopted project root remains");
    assert_eq!(
        adopted
            .try_get::<uuid::Uuid>("", "parent_id")
            .expect("project page parent"),
        uuid::Uuid::parse_str("30000000-0000-0000-0000-000000000012").expect("literal uuid"),
        "projected legacy page must attach to its own scope root"
    );
    assert_eq!(
        adopted.try_get::<uuid::Uuid>("", "project_id").expect("project scope"),
        uuid::Uuid::parse_str("25000000-0000-0000-0000-000000000001").expect("literal uuid"),
        "reparenting must not rewrite project scope"
    );
    let metadata: serde_json::Value = adopted.try_get("", "governance_metadata").expect("metadata");
    assert_eq!(metadata["system_role"], "workspace_navigator_root");
    assert_eq!(
        metadata["legacy_key"], "preserved",
        "adoption must preserve unrelated metadata"
    );
    assert_eq!(adopted.try_get::<i64>("", "document_count").expect("document count"), 1);
    assert_eq!(
        adopted
            .try_get::<i64>("", "projection_count")
            .expect("projection count"),
        1,
        "adoption must preserve, not duplicate, the projection"
    );

    scratch
        .db
        .execute_unprepared(migration)
        .await
        .expect("0059 is replay-safe");
    let replayed = scratch
        .db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*)::bigint AS roots, \
                    min(id::text) FILTER (WHERE project_id IS NULL) AS root_id \
               FROM flow_objects \
              WHERE workspace_id = '20000000-0000-0000-0000-000000000001'::uuid \
                AND object_type = 'navigator' AND parent_id IS NULL"
                .to_string(),
        ))
        .await
        .expect("replay census runs")
        .expect("replay census row");
    assert_eq!(replayed.try_get::<i64>("", "roots").expect("root count"), 2);
    assert_eq!(
        replayed.try_get::<String>("", "root_id").expect("root id"),
        root_id.to_string()
    );

    scratch
        .db
        .execute_unprepared(
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES \
             ('20000000-0000-0000-0000-000000000005', 'nr-future', 'NR Future', \
              '10000000-0000-0000-0000-000000000001')",
        )
        .await
        .expect("a workspace created after 0059 commits with its root aggregate");
    let future = scratch
        .db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*)::bigint AS roots, count(doc.id)::bigint AS documents, \
                    count(projection.object_id)::bigint AS projections \
               FROM flow_objects root \
               LEFT JOIN collab_documents doc ON doc.object_id = root.id \
               LEFT JOIN flow_object_projections projection ON projection.object_id = root.id \
              WHERE root.workspace_id = '20000000-0000-0000-0000-000000000005'::uuid \
                AND root.object_type = 'navigator' AND root.project_id IS NULL \
                AND root.parent_id IS NULL"
                .to_string(),
        ))
        .await
        .expect("future root query runs")
        .expect("future root census row");
    assert_eq!(future.try_get::<i64>("", "roots").expect("future roots"), 1);
    assert_eq!(future.try_get::<i64>("", "documents").expect("future documents"), 1);
    assert_eq!(future.try_get::<i64>("", "projections").expect("future projections"), 1);

    let violations = scratch
        .db
        .query_one(Statement::from_string(
            DbBackend::Postgres,
            "SELECT count(*)::bigint AS n FROM flow_object_navigator_root_violations".to_string(),
        ))
        .await
        .expect("root invariant monitor query runs")
        .expect("root invariant monitor row");
    assert_eq!(violations.try_get::<i64>("", "n").expect("violation count"), 0);

    let mutation = scratch
        .db
        .execute_unprepared(
            "INSERT INTO flow_objects (workspace_id, object_type, project_id, parent_id) VALUES \
             ('20000000-0000-0000-0000-000000000001', 'page', NULL, NULL)",
        )
        .await;
    assert!(
        mutation.is_err(),
        "NR-4 mutation survived: removing flow_objects_non_navigator_parent_check must make this assertion red"
    );

    scratch.drop_self().await;
}

#[tokio::test]
async fn navigator_root_migration_fails_closed_on_duplicate_roots() {
    let scratch = scratch_or_skip!("navigator_root_duplicate");
    seed_pre_ledger_schema(&scratch.db, Some("0059_flow_navigator_root.sql")).await;
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
    seed_pre_ledger_schema(&scratch.db, Some("0059_flow_navigator_root.sql")).await;
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

    run_migrations_with(&scratch.db, MigrationOptions::default())
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

/// A database built by the pre-ledger runner keeps its schema: the early files, which are not
/// idempotent, must be recognised rather than replayed.
#[tokio::test]
async fn an_existing_database_is_adopted_rather_than_replayed() {
    let scratch = scratch_or_skip!("existing");
    seed_pre_ledger_schema(&scratch.db, None).await;

    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("an existing database migrates cleanly");

    let ledger = ledger(&scratch.db).await;
    assert_eq!(status_of(&ledger, "0001_init.sql"), "adopted");
    assert_eq!(status_of(&ledger, "0005_webhooks.sql"), "adopted");
    assert_eq!(status_of(&ledger, "0022_bot_tokens.sql"), "adopted");
    assert_eq!(status_of(&ledger, "0038_form_permissions.sql"), "adopted");
    // Past the frozen cutoff nothing is ever adopted; both files are idempotent and re-run.
    assert_eq!(
        status_of(&ledger, "0048_delivery_lease_and_pickup_indexes.sql"),
        "applied"
    );
    assert!(
        !ledger.values().any(|status| status == "failed"),
        "no migration may fail on an existing database"
    );
    assert_schema_complete(&scratch.db).await;

    scratch.drop_self().await;
}

/// The regression that made the previous revision dangerous: a database whose early files
/// succeeded and whose forms files never did. The blanket `to_regclass('public.users')`
/// predicate claimed all of them, so the missing tables were never created again. Per file
/// probes have to adopt what is there and execute what is not.
#[tokio::test]
async fn a_half_built_database_only_adopts_what_it_actually_has() {
    let scratch = scratch_or_skip!("half_built");
    seed_pre_ledger_schema(&scratch.db, Some("0030_universal_forms.sql")).await;

    assert!(
        !migration_is_present(&scratch.db, "0038_form_permissions.sql")
            .await
            .expect("probe runs"),
        "the fixture must start without the forms tables"
    );

    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("a half built database heals");

    let ledger = ledger(&scratch.db).await;
    assert_eq!(status_of(&ledger, "0001_init.sql"), "adopted");
    assert_eq!(status_of(&ledger, "0029_scenario_templates.sql"), "adopted");
    assert_eq!(status_of(&ledger, "0030_universal_forms.sql"), "applied");
    assert_eq!(status_of(&ledger, "0038_form_permissions.sql"), "applied");
    assert!(
        migration_is_present(&scratch.db, "0038_form_permissions.sql")
            .await
            .expect("probe runs"),
        "the forms tables have to exist after the run"
    );
    assert_schema_complete(&scratch.db).await;

    scratch.drop_self().await;
}

/// Adoption used to be a bare loop entered only while the ledger was empty, so an
/// interruption left it half claimed and the next start executed the remaining early files
/// against a database that already had them. Adoption is now re-derived per file.
#[tokio::test]
async fn an_interrupted_adoption_resumes_on_the_next_start() {
    let scratch = scratch_or_skip!("interrupted");
    seed_pre_ledger_schema(&scratch.db, None).await;

    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("first run adopts the database");

    // Same shape as an adoption killed after 0004: a ledger with a handful of rows in it.
    scratch
        .db
        .execute_unprepared("DELETE FROM schema_migrations WHERE name > '0004'")
        .await
        .expect("the ledger is writable");

    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("the interrupted adoption resumes instead of replaying");

    let ledger = ledger(&scratch.db).await;
    assert_eq!(
        status_of(&ledger, "0005_webhooks.sql"),
        "adopted",
        "0005 creates webhooks without IF NOT EXISTS; executing it here would fail"
    );
    assert_eq!(status_of(&ledger, "0022_bot_tokens.sql"), "adopted");
    assert!(!ledger.values().any(|status| status == "failed"));

    scratch.drop_self().await;
}

/// The escape hatch used to be a trap: one replay recorded the non idempotent files as
/// `failed`, and an ordinary start retries `failed` rows, so the API could never start again
/// without the switch. A replay may now report failures but must never downgrade a recorded
/// success, and the ordinary start afterwards has to work.
#[tokio::test]
async fn a_replay_never_leaves_the_database_unable_to_start() {
    let scratch = scratch_or_skip!("replay");
    seed_pre_ledger_schema(&scratch.db, None).await;
    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("first run adopts the database");

    run_migrations_with(
        &scratch.db,
        MigrationOptions {
            replay: true,
            continue_on_error: false,
        },
    )
    .await
    .expect("a replay reports failures without aborting");

    let after_replay = ledger(&scratch.db).await;
    assert!(
        !after_replay.values().any(|status| status == "failed"),
        "a replay must not downgrade a recorded success to failed"
    );

    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("an ordinary start after a replay still works");
    assert_schema_complete(&scratch.db).await;

    scratch.drop_self().await;
}

/// A genuine failure aborts the start, is recorded with its error, and is retried once the
/// cause is gone.
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

    let err = run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect_err("a failing migration must abort the start");
    assert!(
        err.to_string().contains("0005_webhooks.sql"),
        "the error has to name the migration, got: {err}"
    );

    let after_failure = ledger(&scratch.db).await;
    assert_eq!(status_of(&after_failure, "0005_webhooks.sql"), "failed");
    assert_eq!(status_of(&after_failure, "0004_sprints.sql"), "applied");

    scratch
        .db
        .execute_unprepared("DROP TABLE webhooks")
        .await
        .expect("the blocker table is dropped");
    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("the retry succeeds once the cause is gone");
    let after_retry = ledger(&scratch.db).await;
    assert_eq!(status_of(&after_retry, "0005_webhooks.sql"), "applied");
    assert_schema_complete(&scratch.db).await;

    scratch.drop_self().await;
}

/// The degradation channel an operator needs: start with the failure recorded and visible,
/// rather than being locked out of a database that cannot be inspected.
#[tokio::test]
async fn continue_on_error_starts_despite_a_failed_migration() {
    let scratch = scratch_or_skip!("continue_on_error");
    scratch
        .db
        .execute_unprepared("CREATE TABLE webhooks (blocker integer)")
        .await
        .expect("the blocker table is created");

    run_migrations_with(
        &scratch.db,
        MigrationOptions {
            replay: false,
            continue_on_error: true,
        },
    )
    .await
    .expect("the degraded start comes up");

    let ledger = ledger(&scratch.db).await;
    assert_eq!(status_of(&ledger, "0005_webhooks.sql"), "failed");
    assert_eq!(
        status_of(&ledger, "0030_universal_forms.sql"),
        "applied",
        "the run continues past the failure instead of stopping"
    );

    scratch.drop_self().await;
}

/// A ledger row is a claim, not proof. When the object it claims is gone the start has to
/// stop and name it, because the previous revision started up healthy with the forms tables
/// missing and answered 500 for every forms request instead.
#[tokio::test]
async fn a_ledger_claim_without_the_object_stops_the_start() {
    let scratch = scratch_or_skip!("gap");
    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("a fresh database migrates cleanly");

    scratch
        .db
        .execute_unprepared("DROP TABLE form_permissions CASCADE")
        .await
        .expect("the forms table is dropped");

    let err = run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect_err("a missing object must stop the start");
    assert!(
        err.to_string().contains("0038_form_permissions.sql"),
        "the error has to name the migration, got: {err}"
    );

    run_migrations_with(
        &scratch.db,
        MigrationOptions {
            replay: false,
            continue_on_error: true,
        },
    )
    .await
    .expect("the operator can still start to inspect the database");

    scratch.drop_self().await;
}

/// api and worker start together and both run the migrations. Without the advisory lock two
/// runners raced on the same DDL and one of them died on "relation already exists"; with
/// `restart: unless-stopped` in front of them that is a crash loop.
#[tokio::test]
async fn two_runners_starting_together_both_succeed() {
    let scratch = scratch_or_skip!("concurrent");
    let second = scratch.second_connection().await.expect("a second connection opens");

    let (first_result, second_result) = tokio::join!(
        run_migrations_with(&scratch.db, MigrationOptions::default()),
        run_migrations_with(&second, MigrationOptions::default()),
    );
    first_result.expect("the first runner succeeds");
    second_result.expect("the second runner succeeds");

    let ledger = ledger(&scratch.db).await;
    assert_eq!(ledger.len(), MIGRATIONS.len());
    assert!(!ledger.values().any(|status| status == "failed"));
    assert_schema_complete(&scratch.db).await;

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
    run_migrations_with(&relocated, MigrationOptions::default())
        .await
        .expect("a non public schema migrates cleanly");
    assert_schema_complete(&relocated).await;

    drop(relocated);
    scratch.drop_self().await;
}

const CAPABILITY_DEFAULT_MIGRATION: &str = "0106_flow_capabilities_default_on.sql";

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

fn reader_floor(row: &serde_json::Value) -> i64 {
    row.get("minimum_reader_version")
        .and_then(serde_json::Value::as_i64)
        .expect("minimum_reader_version is an integer")
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
/// and create a rich Page (the editor sends model5). The leases stand in for the api and
/// worker processes, which register them on start; no capability is touched by hand.
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

async fn register_running_readers(db: &DatabaseConnection) {
    for component in ["api", "worker", "maintenance"] {
        api::flow::scope_runtime::renew(db, uuid::Uuid::new_v4(), component)
            .await
            .expect("the reader lease is registered");
    }
}

/// A fresh installation has personal Flow and the shipped Flow surfaces on without any
/// manual SQL, and an ordinary user can open their personal home and write a Page in it.
#[tokio::test]
async fn a_fresh_database_migrates_with_flow_capabilities_on() {
    let scratch = scratch_or_skip!("flow_caps_fresh");
    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("a fresh database migrates cleanly");

    let row = flow_capability_row(&scratch.db).await;
    for column in DEFAULT_ON {
        assert!(capability_flag(&row, column), "{column} is on by default: {row}");
    }
    for column in DEFAULT_ON_SINCE_0115 {
        assert!(capability_flag(&row, column), "{column} is on by default: {row}");
    }
    assert_eq!(
        reader_floor(&row),
        i64::from(api::flow::scope_runtime::READER_VERSION),
        "the reader floor matches what the shipped components register"
    );

    // Before any component has registered, writes fail closed instead of erroring or
    // degrading; the same request succeeds once the api and worker leases exist.
    let blocked = ordinary_user_creates_a_personal_page(&scratch.db)
        .await
        .expect_err("no reader lease is registered yet");
    assert!(
        format!("{blocked:?}").contains("scope_runtime_not_ready"),
        "missing leases are reported as a runtime that is not ready: {blocked:?}"
    );
    register_running_readers(&scratch.db).await;
    ordinary_user_creates_a_personal_page(&scratch.db)
        .await
        .expect("an ordinary user can use personal Flow on a fresh database");

    let caps = api::flow::scope::capabilities(&scratch.db)
        .await
        .expect("capabilities are readable");
    assert!(caps.personal_provisioning && caps.personal_writes && caps.external_mounts);
    assert!(caps.block_transfers_supported, "block transfers are advertised");
    assert!(caps.portable_page_packages_supported && caps.portable_graph_packages_supported);

    scratch.drop_self().await;
}

/// An operator who turns personal Flow off keeps it off: neither a replay nor a re-run of
/// 0106 after its ledger row is gone may switch a capability back on.
#[tokio::test]
async fn an_operator_disable_survives_a_migration_rerun() {
    let scratch = scratch_or_skip!("flow_caps_operator_off");
    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("a fresh database migrates cleanly");
    scratch
        .db
        .execute_unprepared(
            "UPDATE flow_runtime_capabilities SET external_mounts=false, personal_writes=false, \
             personal_provisioning=false, personal_forms_conversion=false, block_transfers_enabled=false, \
             portable_page_packages_enabled=false, portable_graph_packages_enabled=false WHERE singleton",
        )
        .await
        .expect("the documented operator switch-off satisfies the CHECK constraints");
    let disabled = flow_capability_row(&scratch.db).await;

    run_migrations_with(
        &scratch.db,
        MigrationOptions {
            replay: true,
            continue_on_error: false,
        },
    )
    .await
    .expect("a replay runs");
    assert_eq!(
        flow_capability_row(&scratch.db).await,
        disabled,
        "a replay keeps the operator's choice"
    );

    scratch
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM schema_migrations WHERE name=$1",
            vec![CAPABILITY_DEFAULT_MIGRATION.into()],
        ))
        .await
        .expect("the ledger row is removed");
    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("an ordinary start re-executes 0106");
    assert_eq!(
        status_of(&ledger(&scratch.db).await, CAPABILITY_DEFAULT_MIGRATION),
        "applied",
        "0106 was executed again"
    );
    assert_eq!(
        flow_capability_row(&scratch.db).await,
        disabled,
        "a re-execution keeps the operator's choice"
    );

    register_running_readers(&scratch.db).await;
    let refused = ordinary_user_creates_a_personal_page(&scratch.db)
        .await
        .expect_err("personal Flow is off");
    assert!(
        format!("{refused:?}").contains("personal_creation_disabled"),
        "the switch-off is enforced: {refused:?}"
    );

    scratch.drop_self().await;
}

/// An existing database that predates 0106 is switched on once, keeps everything an operator
/// turned on (including capabilities that stay off by default) and never sees its reader floor
/// lowered; executing the file again changes nothing.
#[tokio::test]
async fn the_flow_capability_default_migration_is_idempotent_and_never_lowers() {
    let scratch = scratch_or_skip!("flow_caps_idempotent");
    seed_pre_ledger_schema(&scratch.db, Some(CAPABILITY_DEFAULT_MIGRATION)).await;
    let before = flow_capability_row(&scratch.db).await;
    for column in DEFAULT_ON {
        assert!(!capability_flag(&before, column), "{column} starts off before 0106");
    }
    scratch
        .db
        .execute_unprepared(
            "UPDATE flow_runtime_capabilities SET minimum_reader_version=6, personal_package_imports=false, \
             scope_native_legacy=false WHERE singleton",
        )
        .await
        .expect("the operator settings are applied");

    let sql = MIGRATIONS
        .iter()
        .find(|(name, _)| *name == CAPABILITY_DEFAULT_MIGRATION)
        .map(|(_, sql)| *sql)
        .expect("0106 is registered");
    scratch
        .db
        .execute_unprepared(sql)
        .await
        .expect("0106 first execution succeeds");
    let first = flow_capability_row(&scratch.db).await;
    for column in DEFAULT_ON {
        assert!(capability_flag(&first, column), "{column} is switched on: {first}");
    }
    assert!(
        !capability_flag(&first, "personal_package_imports"),
        "operator choice kept"
    );
    assert!(!capability_flag(&first, "scope_native_legacy"), "operator choice kept");
    assert!(capability_flag(&first, "personal_scope_package_exports"));
    assert!(capability_flag(&first, "cross_workspace_forms_conversion"));
    assert_eq!(reader_floor(&first), 6, "the reader floor is never lowered");

    scratch
        .db
        .execute_unprepared(sql)
        .await
        .expect("0106 second execution succeeds");
    assert_eq!(
        flow_capability_row(&scratch.db).await,
        first,
        "a second execution is a no-op"
    );

    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("the runner adopts the database and re-executes the post-cutoff files");
    // The runner also executes 0115 for the first time, which switches the four capabilities
    // 0106 leaves alone on once; nothing else changes and the reader floor stays.
    let mut expected = first.clone();
    for column in DEFAULT_ON_SINCE_0115 {
        expected[column] = serde_json::Value::Bool(true);
    }
    assert_eq!(
        flow_capability_row(&scratch.db).await,
        expected,
        "the runner's execution only adds the 0115 switch-on"
    );
    assert_schema_complete(&scratch.db).await;

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
    run_migrations_with(&scratch.db, MigrationOptions::default())
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
    seed_pre_ledger_schema(&scratch.db, Some(FLOW_DEFAULT_ON_MIGRATION)).await;
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
    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("the runner adopts the database and re-executes the post-cutoff files");
    assert_schema_complete(&scratch.db).await;
    scratch.drop_self().await;
}

/// 0117 repairs data that broke the sprint rules (two active sprints, names that differ only in
/// case, an end before the start, an unknown status), replaces the exact-case name constraint,
/// and does nothing the second time. The old state is rebuilt by dropping the new rules from a
/// fully migrated database.
#[tokio::test]
async fn sprint_rules_migration_fixes_data_and_reruns() {
    let scratch = scratch_or_skip!("sprint_rules_migration");
    run_migrations_with(&scratch.db, MigrationOptions::default())
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
    run_migrations_with(&scratch.db, MigrationOptions::default())
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
    run_migrations_with(&scratch.db, MigrationOptions::default())
        .await
        .expect("a fresh database migrates cleanly");
    register_running_readers(&scratch.db).await;
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
    assert_schema_complete(&scratch.db).await;
    scratch.drop_self().await;
}

/// 0127 drops the anchor seal column and the other retired objects; every statement of it can run
/// again without effect.
#[tokio::test]
async fn the_legacy_objects_migration_retires_what_it_names_and_reruns() {
    let scratch = scratch_or_skip!("legacy_objects_migration");
    run_migrations_with(&scratch.db, MigrationOptions::default())
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
    run_migrations_with(&scratch.db, MigrationOptions::default())
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
