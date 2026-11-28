//! Package pages against `PostgreSQL`: which `site_pages` rows the site store shows (admin list and
//! public routes both read `list_pages`) and which layout a page is served with. Every test starts
//! its own scratch database (skipped without `OPENPR_TEST_DATABASE_URL`).
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use sea_orm::{ConnectionTrait, DatabaseConnection, FromQueryResult};
use uuid::Uuid;

use super::store::{self, SiteRow};
use crate::routes::context::tenant_fixture::{Tenant, exec, seed_tenant};
use crate::scratch_or_skip;

const SPEC: &str = r#"{"schema":"sv.layout.v1","blocks":[]}"#;

async fn seed_site(db: &impl ConnectionTrait, tenant: &Tenant, slug: &str, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO sites (id, workspace_id, project_id, slug, title, status) \
         VALUES ($1, $2, $3, $4, '{\"en\":\"T\"}'::jsonb, $5)",
        vec![
            id.into(),
            tenant.workspace_id.into(),
            tenant.project_id.into(),
            slug.into(),
            status.into(),
        ],
    )
    .await;
    id
}

async fn seed_installation(
    db: &impl ConnectionTrait,
    tenant: &Tenant,
    key: &str,
    status: &str,
    generation: i32,
) -> Uuid {
    let id = Uuid::new_v4();
    let reason = if status == "disabled" { Some("admin") } else { None };
    exec(
        db,
        "INSERT INTO installations (id, workspace_id, scope_kind, project_id, package_key, status, disabled_reason, \
         active_generation, created_by_kind, created_by) VALUES ($1, $2, 'project', $3, $4, $5, $6, $7, 'user', $8)",
        vec![
            id.into(),
            tenant.workspace_id.into(),
            tenant.project_id.into(),
            key.into(),
            status.into(),
            reason.into(),
            generation.into(),
            tenant.member_id.into(),
        ],
    )
    .await;
    id
}

async fn seed_layout(
    db: &impl ConnectionTrait,
    tenant: &Tenant,
    site_id: Uuid,
    key: &str,
    package: Option<(Uuid, i32)>,
) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO layouts (id, workspace_id, scope_kind, scope_id, layout_key, spec, source, installation_id, generation) \
         VALUES ($1, $2, 'site', $3, $4, $5::jsonb, $6, $7, $8)",
        vec![
            id.into(),
            tenant.workspace_id.into(),
            site_id.into(),
            key.into(),
            SPEC.into(),
            if package.is_some() { "package" } else { "user" }.into(),
            package.map(|(installation, _)| installation).into(),
            package.map(|(_, generation)| generation).into(),
        ],
    )
    .await;
    id
}

async fn seed_page(
    db: &impl ConnectionTrait,
    site_id: Uuid,
    layout_id: Uuid,
    route: &str,
    audience: &str,
    package: Option<(Uuid, i32)>,
) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO site_pages (id, site_id, route, layout_id, audience, installation_id, generation) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        vec![
            id.into(),
            site_id.into(),
            route.into(),
            layout_id.into(),
            audience.into(),
            package.map(|(installation, _)| installation).into(),
            package.map(|(_, generation)| generation).into(),
        ],
    )
    .await;
    id
}

async fn routes(db: &DatabaseConnection, site_id: Uuid) -> Vec<String> {
    store::list_pages(db, site_id)
        .await
        .unwrap()
        .into_iter()
        .map(|page| page.route)
        .collect()
}

async fn site_row(db: &DatabaseConnection, site_id: Uuid) -> SiteRow {
    store::load_site(db, site_id).await.unwrap().expect("site exists")
}

#[tokio::test]
async fn package_page_is_listed_and_served_while_the_installation_is_active() {
    let scratch = scratch_or_skip!("site_pkg_pages_active");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "a").await;
    let site = seed_site(db, &tenant, "pkg-active", "published").await;
    let installation = seed_installation(db, &tenant, "acme/desk", "active", 1).await;
    let layout = seed_layout(db, &tenant, site, "home", Some((installation, 1))).await;
    let user_layout = seed_layout(db, &tenant, site, "mine", None).await;
    let page = seed_page(db, site, layout, "/", "visitor", Some((installation, 1))).await;
    seed_page(db, site, user_layout, "/about", "visitor", None).await;

    assert_eq!(routes(db, site).await, vec!["/".to_string(), "/about".to_string()]);
    let listed = store::list_pages(db, site).await.unwrap();
    let package = listed.iter().find(|row| row.id == page).unwrap();
    assert_eq!(package.installation_id, Some(installation));
    assert_eq!(package.generation, Some(1));
    assert!(store::load_page(db, page).await.unwrap().is_some());

    let row = site_row(db, site).await;
    let served = store::load_page_layout(db, &row, package).await.unwrap();
    assert_eq!(served.map(|layout| layout.id), Some(layout));
    // A package page's rows are never writable through the user-page lock.
    assert!(store::lock_page(db, page).await.unwrap().is_none());
    scratch.drop_self().await;
}

#[tokio::test]
async fn package_page_is_hidden_when_the_installation_is_not_active() {
    let scratch = scratch_or_skip!("site_pkg_pages_inactive");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "a").await;
    let site = seed_site(db, &tenant, "pkg-inactive", "published").await;
    for (status, key) in [("disabled", "acme/off"), ("revoked", "acme/gone")] {
        let installation = seed_installation(db, &tenant, key, status, 1).await;
        let layout = seed_layout(
            db,
            &tenant,
            site,
            key.rsplit('/').next().unwrap(),
            Some((installation, 1)),
        )
        .await;
        let route = format!("/{}", key.rsplit('/').next().unwrap());
        let page = seed_page(db, site, layout, &route, "visitor", Some((installation, 1))).await;
        assert!(
            !routes(db, site).await.contains(&route),
            "{status} installation hides its page"
        );
        assert!(store::load_page(db, page).await.unwrap().is_none());
        assert!(!store::layout_on_published_site(db, layout).await.unwrap());
    }
    assert!(routes(db, site).await.is_empty());
    scratch.drop_self().await;
}

#[derive(FromQueryResult)]
struct Id {
    site_id: Uuid,
}

fn manifest_with_page(route: &str) -> sv_package::Manifest {
    serde_json::from_value(serde_json::json!({
        "schema": "sv.package.v1",
        "kind": "page-template",
        "id": "acme/desk",
        "version": "1.0.0",
        "publisher": { "id": "acme", "key_id": "ed25519:00" },
        "sv": { "min": "0.5.0" },
        "summary": { "en": "Desk" },
        "license": "MIT",
        "layouts": [{ "key": "home", "path": "layouts/home.json" }],
        "pages": [{ "route": route, "layout": "home", "audience": "visitor" }],
        "files": {}
    }))
    .expect("manifest parses")
}

#[tokio::test]
async fn an_upgrade_replaces_the_pages_of_the_published_site() {
    use crate::extensions::install::artefacts::{PlacementScope, create_pages};
    use std::collections::BTreeMap;

    let scratch = scratch_or_skip!("site_pkg_pages_upgrade");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "a").await;
    let scope = PlacementScope {
        workspace_id: tenant.workspace_id,
        project_id: Some(tenant.project_id),
    };
    let installation = seed_installation(db, &tenant, "acme/desk", "active", 1).await;

    // Generation 1 creates the installation's site (a draft); a person publishes it.
    let layout_one = seed_layout(db, &tenant, Uuid::new_v4(), "home", Some((installation, 1))).await;
    let layouts = BTreeMap::from([("home".to_string(), layout_one)]);
    create_pages(
        db,
        &manifest_with_page("/"),
        &layouts,
        scope,
        installation,
        1,
        tenant.member_id,
    )
    .await
    .unwrap();
    let site = Id::find_by_statement(sea_orm::Statement::from_sql_and_values(
        sea_orm::DbBackend::Postgres,
        "SELECT site_id FROM site_pages WHERE installation_id = $1",
        vec![installation.into()],
    ))
    .one(db)
    .await
    .unwrap()
    .expect("the install created a page")
    .site_id;
    exec(
        db,
        "UPDATE sites SET status = 'published' WHERE id = $1",
        vec![site.into()],
    )
    .await;
    assert_eq!(routes(db, site).await, vec!["/".to_string()]);

    // Generation 2 becomes active: its pages land in the same, published site; generation 1's are gone.
    exec(
        db,
        "UPDATE installations SET active_generation = 2 WHERE id = $1",
        vec![installation.into()],
    )
    .await;
    let layout_two = seed_layout(db, &tenant, site, "home", Some((installation, 2))).await;
    let layouts = BTreeMap::from([("home".to_string(), layout_two)]);
    create_pages(
        db,
        &manifest_with_page("/help"),
        &layouts,
        scope,
        installation,
        2,
        tenant.member_id,
    )
    .await
    .unwrap();

    assert_eq!(routes(db, site).await, vec!["/help".to_string()]);
    let total = crate::routes::context::tenant_fixture::count(
        db,
        "SELECT COUNT(*) AS n FROM site_pages WHERE installation_id = $1",
        vec![installation.into()],
    )
    .await;
    assert_eq!(total, 1, "the old generation's rows are deleted");
    let sites = crate::routes::context::tenant_fixture::count(
        db,
        "SELECT COUNT(*) AS n FROM sites WHERE project_id = $1",
        vec![tenant.project_id.into()],
    )
    .await;
    assert_eq!(sites, 1, "the upgrade created no new site");
    scratch.drop_self().await;
}

#[tokio::test]
async fn an_operator_page_is_listed_with_its_audience_for_the_public_filter() {
    let scratch = scratch_or_skip!("site_pkg_pages_operator");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "a").await;
    let site = seed_site(db, &tenant, "pkg-operator", "published").await;
    let installation = seed_installation(db, &tenant, "acme/desk", "active", 1).await;
    let layout = seed_layout(db, &tenant, site, "inbox", Some((installation, 1))).await;
    seed_page(db, site, layout, "/inbox", "operator", Some((installation, 1))).await;

    // The public handlers skip `operator` pages for a caller without an operator session.
    let pages = store::list_pages(db, site).await.unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].audience, "operator");
    assert!(pages.iter().all(|page| page.audience == "operator"));
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_page_of_an_installation_in_another_project_or_workspace_is_never_shown() {
    let scratch = scratch_or_skip!("site_pkg_pages_foreign");
    let db = &scratch.db;
    let mine = seed_tenant(db, "a").await;
    let other = seed_tenant(db, "b").await;
    let site = seed_site(db, &mine, "pkg-foreign", "published").await;
    // An installation of another workspace, and one of another project in the same workspace.
    let foreign = seed_installation(db, &other, "acme/desk", "active", 1).await;
    let other_project = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'ZZ', 'Other', $3)",
        vec![other_project.into(), mine.workspace_id.into(), mine.member_id.into()],
    )
    .await;
    let sibling = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO installations (id, workspace_id, scope_kind, project_id, package_key, status, active_generation, \
         created_by_kind, created_by) VALUES ($1, $2, 'project', $3, 'acme/other', 'active', 1, 'user', $4)",
        vec![
            sibling.into(),
            mine.workspace_id.into(),
            other_project.into(),
            mine.member_id.into(),
        ],
    )
    .await;
    for (installation, route) in [(foreign, "/foreign"), (sibling, "/sibling")] {
        let layout = seed_layout(db, &mine, site, &route[1..], Some((installation, 1))).await;
        seed_page(db, site, layout, route, "visitor", Some((installation, 1))).await;
    }
    assert!(routes(db, site).await.is_empty());
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_package_page_serves_only_a_layout_of_its_own_installation_and_generation() {
    let scratch = scratch_or_skip!("site_pkg_pages_layout");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "a").await;
    let site = seed_site(db, &tenant, "pkg-layout", "published").await;
    let installation = seed_installation(db, &tenant, "acme/desk", "active", 1).await;
    let rival = seed_installation(db, &tenant, "acme/rival", "active", 1).await;
    let user_layout = seed_layout(db, &tenant, site, "mine", None).await;
    let rival_layout = seed_layout(db, &tenant, site, "rival", Some((rival, 1))).await;
    let page_on_user = seed_page(db, site, user_layout, "/a", "visitor", Some((installation, 1))).await;
    let page_on_rival = seed_page(db, site, rival_layout, "/b", "visitor", Some((installation, 1))).await;
    let row = site_row(db, site).await;
    for id in [page_on_user, page_on_rival] {
        let page = store::load_page(db, id).await.unwrap().unwrap();
        assert!(store::load_page_layout(db, &row, &page).await.unwrap().is_none());
    }
    scratch.drop_self().await;
}

#[tokio::test]
async fn store_writes_work_and_a_user_page_cannot_take_a_package_route() {
    let scratch = scratch_or_skip!("site_pkg_pages_writes");
    let db = &scratch.db;
    let tenant = seed_tenant(db, "a").await;
    let site = seed_site(db, &tenant, "pkg-writes", "draft").await;
    let installation = seed_installation(db, &tenant, "acme/desk", "active", 1).await;
    let package_layout = seed_layout(db, &tenant, site, "home", Some((installation, 1))).await;
    let user_layout = seed_layout(db, &tenant, site, "mine", None).await;
    seed_page(db, site, package_layout, "/help", "visitor", Some((installation, 1))).await;
    let values = |route: &'static str| store::PageValues {
        route,
        layout_id: user_layout,
        audience: "visitor",
        nav: None,
        position: 0,
    };

    // insert_page and update_page return the row (the RETURNING list needs the table alias).
    let created = store::insert_page(db, site, values("/mine"))
        .await
        .unwrap()
        .expect("inserted");
    assert_eq!(created.route, "/mine");
    assert_eq!(created.installation_id, None);
    let updated = store::update_page(db, created.id, values("/mine-2"))
        .await
        .unwrap()
        .expect("updated");
    assert_eq!(updated.route, "/mine-2");

    // A route of a visible package page is taken for user pages: insert, update and the draft check.
    assert!(store::insert_page(db, site, values("/help")).await.unwrap().is_none());
    assert!(
        store::update_page(db, created.id, values("/help"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(store::route_taken(db, site, "/help", None).await.unwrap());
    assert!(!store::route_taken(db, site, "/free", None).await.unwrap());

    // A package page is never changed through the user-page update.
    let package = store::list_pages(db, site)
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.route == "/help")
        .unwrap();
    assert!(
        store::update_page(db, package.id, values("/other"))
            .await
            .unwrap()
            .is_none()
    );
    scratch.drop_self().await;
}
