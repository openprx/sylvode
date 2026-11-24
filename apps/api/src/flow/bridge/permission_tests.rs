//! `target_permission` against a scratch `PostgreSQL`: workspace owners and admins hold every form
//! permission unless a policy row names their role (skipped without `OPENPR_TEST_DATABASE_URL`).
#![cfg(test)]
#![allow(clippy::indexing_slicing, clippy::print_stderr)]

use super::{BridgeActor, PolicyConfiguration, TargetView, target_permission};
use crate::flow::collab::authz::PermissionLevel;
use sea_orm::{ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

const TEST_DATABASE_URL_ENV: &str = "OPENPR_TEST_DATABASE_URL";

struct Fixture {
    db: DatabaseConnection,
    name: String,
    admin_url: String,
    ws: Uuid,
    project: Uuid,
    user: Uuid,
}

impl Fixture {
    async fn new(label: &str) -> Option<Self> {
        let Ok(admin_url) = std::env::var(TEST_DATABASE_URL_ENV) else {
            eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
            return None;
        };
        let admin = Database::connect(&admin_url)
            .await
            .unwrap_or_else(|err| panic!("{TEST_DATABASE_URL_ENV} is set but unusable: {err}"));
        let name = format!("sylvode_bridge_perm_{label}");
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
        for path in files {
            let sql = std::fs::read_to_string(&path).expect("a migration file is readable");
            db.execute_unprepared(&sql)
                .await
                .unwrap_or_else(|err| panic!("applying {} failed: {err}", path.display()));
        }
        let [ws, project, user] = std::array::from_fn(|_| Uuid::new_v4());
        exec(
            &db,
            "INSERT INTO users (id, email, password_hash, name, role, is_active) \
             VALUES ($1, $2, '!', 'owner', 'user', true)",
            vec![user.into(), format!("{user}@bridge.test").into()],
        )
        .await;
        exec(
            &db,
            "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'ws', $3)",
            vec![ws.into(), format!("ws-{ws}").into(), user.into()],
        )
        .await;
        exec(
            &db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
             VALUES ($1, $2, 'BRG', 'bridge', $3, 'code_project')",
            vec![project.into(), ws.into(), user.into()],
        )
        .await;
        Some(Self {
            db,
            name,
            admin_url,
            ws,
            project,
            user,
        })
    }

    async fn finish(self) {
        let Self {
            db, name, admin_url, ..
        } = self;
        drop(db);
        if let Ok(admin) = Database::connect(&admin_url).await {
            let _ = admin
                .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
                .await;
        }
    }
}

async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute(Statement::from_sql_and_values(DbBackend::Postgres, sql, values))
        .await
        .unwrap_or_else(|err| panic!("setup statement failed: {err}"));
}

async fn form_with_policies(api: &Fixture, policies: &[(&str, Value)]) -> TargetView {
    let form = Uuid::new_v4();
    exec(
        &api.db,
        "INSERT INTO project_forms (id, workspace_id, project_id, key, name, created_by) \
         VALUES ($1, $2, $3, $5, 'Bridge form', $4)",
        vec![
            form.into(),
            api.ws.into(),
            api.project.into(),
            api.user.into(),
            format!("f_{}", form.simple()).into(),
        ],
    )
    .await;
    for (role, policy) in policies {
        exec(
            &api.db,
            "INSERT INTO form_permissions (workspace_id, project_id, form_id, subject_type, subject_id, policy) \
             VALUES ($1, $2, $3, 'role', $4, $5)",
            vec![
                api.ws.into(),
                api.project.into(),
                form.into(),
                (*role).to_string().into(),
                policy.clone().into(),
            ],
        )
        .await;
    }
    TargetView {
        form_id: form,
        title: "Bridge form".to_string(),
        status: "active".to_string(),
        summary: json!({}),
        created_by: None,
    }
}

fn actor(api: &Fixture, role: &str) -> BridgeActor {
    BridgeActor {
        id: api.user,
        role: role.to_string(),
        is_bot: false,
        flow_level: PermissionLevel::FullAccess,
    }
}

fn member_policy() -> Value {
    json!({"actions": {"form.view": true, "record.create": true}})
}

#[tokio::test]
async fn an_admin_converts_into_a_form_whose_policy_names_only_members() {
    let Some(api) = Fixture::new("bridge_admin_default").await else {
        return;
    };
    let target = form_with_policies(&api, &[("member", member_policy())]).await;
    for role in ["admin", "owner"] {
        let decision = target_permission(&api.db, &actor(&api, role), &target)
            .await
            .expect("the permission is read")
            .expect("the form is visible");
        assert!(decision.allows("form.view"), "{role}");
        assert!(decision.allows("record.create"), "{role}");
        assert!(decision.allows("record.update"), "{role}");
        assert!(decision.allows("record.delete"), "{role}");
        assert!(!decision.allows("form.design"), "{role}");
        assert!(decision.field_allows("anything", "read"), "{role}");
        assert!(decision.field_allows("anything", "write"), "{role}");
    }
    api.finish().await;
}

#[tokio::test]
async fn a_member_stays_bound_by_the_member_policy() {
    let Some(api) = Fixture::new("bridge_member_policy").await else {
        return;
    };
    let forbids = json!({"actions": {"form.view": true, "record.create": false, "record.update": false}});
    let target = form_with_policies(&api, &[("member", forbids)]).await;
    let decision = target_permission(&api.db, &actor(&api, "member"), &target)
        .await
        .expect("the permission is read")
        .expect("the form is visible");
    assert!(decision.allows("form.view"));
    assert!(!decision.allows("record.create"));
    assert!(!decision.allows("record.update"));
    // Without any row a member keeps the read-only floor.
    let bare = form_with_policies(&api, &[]).await;
    let floor = target_permission(&api.db, &actor(&api, "member"), &bare)
        .await
        .expect("the permission is read")
        .expect("the form is visible");
    assert_eq!(floor.state.actions, ["form.view"]);
    assert_eq!(floor.state.configuration, PolicyConfiguration::Unconfigured);
    api.finish().await;
}

#[tokio::test]
async fn an_explicit_admin_row_is_honoured() {
    let Some(api) = Fixture::new("bridge_admin_row").await else {
        return;
    };
    let restricted = json!({
        "actions": {"form.view": true, "record.delete": false},
        "fields": {"secret": {"read": false, "write": false}}
    });
    let target = form_with_policies(&api, &[("admin", restricted)]).await;
    let decision = target_permission(&api.db, &actor(&api, "admin"), &target)
        .await
        .expect("the permission is read")
        .expect("the form is visible");
    assert!(decision.allows("record.update"));
    assert!(!decision.allows("record.delete"));
    assert!(!decision.field_allows("secret", "read"));
    assert!(!decision.field_allows("secret", "write"));
    // The owner has no row of its own: the full default applies to it, not the admin row.
    let owner = target_permission(&api.db, &actor(&api, "owner"), &target)
        .await
        .expect("the permission is read")
        .expect("the form is visible");
    assert!(owner.allows("record.delete"));
    api.finish().await;
}
