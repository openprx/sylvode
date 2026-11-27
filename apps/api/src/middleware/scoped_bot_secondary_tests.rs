//! The objects a request names beyond the one the middleware resolved (audit F1, F2, F3): a bot
//! connected to (or granted `write` in) one project must not reach a second project through a body
//! field. Each test seeds two projects, P1 where the bot may write and P2 where it holds nothing,
//! and replays the attack through the real handlers behind the real bot authentication.
//!
//! Rows are seeded directly, so these tests do not need the isolated apply worker. Needs
//! `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_lines)]

use axum::{
    Router,
    middleware::from_fn_with_state,
    routing::{get, post},
};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use uuid::Uuid;

use super::scoped_bot::REFUSAL;
use super::scoped_bot_flow_reads_tests::seed_object;
use super::scoped_bot_projects_tests::scoped_bot;
use super::scoped_bot_tests::{Cast, call, cast};
use crate::middleware::bot_auth::bot_or_user_auth_middleware;
use crate::routes::context::tenant_fixture::{count, exec, state_for};
use crate::routes::flow::{
    post_flow_v2_body_asset_copy, post_flow_v2_command, post_flow_v2_conversion, post_flow_v2_conversion_preview,
    post_flow_v2_reference,
};
use crate::routes::issue_pages::{link_issue_page, list_issue_pages};
use crate::scratch_or_skip;

fn app(db: &DatabaseConnection) -> Router {
    let state = state_for(db.clone());
    let layer = || from_fn_with_state(state.clone(), bot_or_user_auth_middleware);
    Router::new()
        .route(
            "/api/v2/flow/conversions/preview",
            post(post_flow_v2_conversion_preview).route_layer(layer()),
        )
        .route(
            "/api/v2/flow/conversions",
            post(post_flow_v2_conversion).route_layer(layer()),
        )
        .route(
            "/api/v2/flow/objects/{object_id}/commands",
            post(post_flow_v2_command).route_layer(layer()),
        )
        .route(
            "/api/v2/flow/objects/{object_id}/references",
            post(post_flow_v2_reference).route_layer(layer()),
        )
        .route(
            "/api/v2/flow/objects/{object_id}/body-assets/copy",
            post(post_flow_v2_body_asset_copy).route_layer(layer()),
        )
        .route(
            "/api/v1/issues/{issue_id}/pages",
            get(list_issue_pages).post(link_issue_page).route_layer(layer()),
        )
        .with_state(state)
}

/// Whether the answer is the one refusal every scoped-bot check gives (the API answers an error
/// with its code in the envelope).
fn is_refusal(answer: &(u16, Value)) -> bool {
    answer.1["code"] == 403 && answer.1["message"] == REFUSAL
}

struct Scene {
    c: Cast,
    app: Router,
    workspace: Uuid,
    /// The project-scoped bot (`write` in P1 only, default none) and the runner bot (connected to P1
    /// only): tokens and ids.
    scoped: (String, Uuid),
    runner: (String, Uuid),
    p1: Uuid,
    p2: Uuid,
    page1: Uuid,
    page2: Uuid,
    form1: Uuid,
    form2: Uuid,
}

async fn form_in(db: &DatabaseConnection, workspace: Uuid, project: Uuid, key: &str) -> Uuid {
    let form = Uuid::new_v4();
    exec(
        db,
        "INSERT INTO project_forms (id, workspace_id, project_id, key, name, schema) \
         VALUES ($1, $2, $3, $4, 'Target', $5::jsonb)",
        vec![
            form.into(),
            workspace.into(),
            project.into(),
            key.to_string().into(),
            json!({"version":"sv.form.schema.v2","fields":[{"field_id":"fld_text_one","key":"t","type":"text","required":false}]})
                .to_string()
                .into(),
        ],
    )
    .await;
    exec(
        db,
        "INSERT INTO form_permissions (workspace_id, project_id, form_id, subject_type, subject_id, policy) \
         VALUES ($1, $2, $3, 'role', 'member', $4::jsonb)",
        vec![
            workspace.into(),
            project.into(),
            form.into(),
            json!({"actions":{"form.view":true,"record.create":true}})
                .to_string()
                .into(),
        ],
    )
    .await;
    form
}

async fn scene(tag: &str, scratch_db: &DatabaseConnection) -> Scene {
    let db = scratch_db;
    let c = cast(db, tag).await;
    let workspace = c.w.tenant.workspace_id;
    exec(
        db,
        "INSERT INTO flow_workspace_settings (workspace_id, flow_enabled, bridge_enabled) VALUES ($1, true, true) \
         ON CONFLICT (workspace_id) DO UPDATE SET flow_enabled = true, bridge_enabled = true",
        vec![workspace.into()],
    )
    .await;
    let (p1, p2) = (c.on, c.never);
    let scoped = scoped_bot(db, &c, "scoped", &["read", "write"], "none", &[(p1, "write")]).await;
    let page1 = seed_object(db, workspace, Some(p1), "page one", false, 1).await;
    let page2 = seed_object(db, workspace, Some(p2), "page two secret", false, 1).await;
    let form1 = form_in(db, workspace, p1, "target_one").await;
    let form2 = form_in(db, workspace, p2, "target_two").await;
    let runner = (c.token.clone(), c.bot);
    Scene {
        app: app(db),
        c,
        workspace,
        scoped,
        runner,
        p1,
        p2,
        page1,
        page2,
        form1,
        form2,
    }
}

fn preview_body(source: Uuid, form: Uuid, key: &str) -> Value {
    json!({
        "source_object_id": source, "source_frontier": "", "target_type": "form_record",
        "mapping": {"target_form_id": form, "title": "Converted", "values": {"t": "x"}},
        "idempotency_key": key
    })
}

fn bots(s: &Scene) -> [(&'static str, String); 2] {
    [("project-scoped", s.scoped.0.clone()), ("runner", s.runner.0.clone())]
}

/// F1, injection: a body that names the bot's project in `project_id` but a page of another project
/// as the source is judged by the source, and a request that carries the extra field is refused.
#[tokio::test]
async fn a_conversion_is_judged_by_its_source_not_by_a_project_id_in_the_body() {
    let scratch = scratch_or_skip!("secondary_f1_inject");
    let s = scene("sx1", &scratch.db).await;
    for (kind, token) in bots(&s) {
        let mut body = preview_body(s.page2, s.form1, &format!("inject-{kind}"));
        body["project_id"] = json!(s.p1);
        let answer = call(&s.app, "POST", "/api/v2/flow/conversions/preview", &token, Some(body)).await;
        assert!(is_refusal(&answer), "{kind}: {answer:?}");
        let made = count(
            &scratch.db,
            "SELECT count(*) AS count FROM flow_conversion_previews WHERE source_object_id = $1",
            vec![s.page2.into()],
        )
        .await;
        assert_eq!(made, 0, "{kind}: no preview of the foreign page exists");
    }
    scratch.drop_self().await;
}

/// F1, target: a source in the bot's project converted into a form of another project is refused;
/// the same conversion into a form of the bot's own project is previewed (the control).
#[tokio::test]
async fn a_conversion_target_in_another_project_is_refused() {
    let scratch = scratch_or_skip!("secondary_f1_target");
    let s = scene("sx2", &scratch.db).await;
    for (kind, token) in bots(&s) {
        let own = call(
            &s.app,
            "POST",
            "/api/v2/flow/conversions/preview",
            &token,
            Some(preview_body(s.page1, s.form1, &format!("own-{kind}"))),
        )
        .await;
        assert!(own.0 == 200 && own.1["code"] == 0, "{kind} control: {own:?}");

        let foreign = call(
            &s.app,
            "POST",
            "/api/v2/flow/conversions/preview",
            &token,
            Some(preview_body(s.page1, s.form2, &format!("foreign-{kind}"))),
        )
        .await;
        assert!(is_refusal(&foreign), "{kind}: {foreign:?}");

        let mut new_form = json!({
            "source_object_id": s.page1, "source_frontier": "", "target_type": "form",
            "mapping": {"target_project_id": s.p2, "key": "stolen", "name": "Stolen"},
            "idempotency_key": format!("newform-{kind}")
        });
        let answer = call(
            &s.app,
            "POST",
            "/api/v2/flow/conversions/preview",
            &token,
            Some(new_form.clone()),
        )
        .await;
        assert!(is_refusal(&answer), "{kind} new form in P2: {answer:?}");
        new_form["mapping"]["target_project_id"] = json!(s.p1);
        let answer = call(
            &s.app,
            "POST",
            "/api/v2/flow/conversions/preview",
            &token,
            Some(new_form),
        )
        .await;
        assert!(
            !is_refusal(&answer),
            "{kind} new form in P1 is not a scope refusal: {answer:?}"
        );
    }
    let made = count(
        &scratch.db,
        "SELECT count(*) AS count FROM flow_conversion_previews WHERE target_project_id = $1",
        vec![s.p2.into()],
    )
    .await;
    assert_eq!(made, 0, "no preview targets the other project");
    scratch.drop_self().await;
}

/// F1, commit: a stored preview whose target is in another project (made before the bot lost it, or
/// by a person) is not committed by a bot that cannot write there.
#[tokio::test]
async fn a_conversion_commit_checks_the_stored_target() {
    let scratch = scratch_or_skip!("secondary_f1_commit");
    let s = scene("sx3", &scratch.db).await;
    for (kind, token, bot) in [
        ("project-scoped", s.scoped.0.clone(), s.scoped.1),
        ("runner", s.runner.0.clone(), s.runner.1),
    ] {
        let preview = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO flow_conversion_previews (id, workspace_id, actor_id, actor_kind, source_object_id, \
             source_frontier, target_type, target_project_id, target_form_id, target_schema_version, mapping, \
             permission_snapshot, idempotency_key, expires_at) \
             VALUES ($1, $2, $3, 'bot', $4, '', 'form_record', $5, $6, 1, '{}'::jsonb, '{}'::jsonb, $7, \
             now() + interval '10 minutes')",
            vec![
                preview.into(),
                s.workspace.into(),
                bot.into(),
                s.page1.into(),
                s.p2.into(),
                s.form2.into(),
                format!("seed-{kind}").into(),
            ],
        )
        .await;
        let answer = call(
            &s.app,
            "POST",
            "/api/v2/flow/conversions",
            &token,
            Some(json!({
                "preview_id": preview, "source_frontier": "", "target_schema_version": 1,
                "idempotency_key": format!("commit-{kind}"), "confirm": true
            })),
        )
        .await;
        assert!(is_refusal(&answer), "{kind}: {answer:?}");
    }
    let jobs = count(
        &scratch.db,
        "SELECT count(*) AS count FROM flow_conversion_jobs",
        vec![],
    )
    .await;
    assert_eq!(jobs, 0, "no job was started");
    scratch.drop_self().await;
}

/// F2, move: a bot moves a page of its project under a page of another project, or under a page of a
/// project where it holds only `read`: both refused.
#[tokio::test]
async fn a_move_destination_needs_write_in_its_project() {
    let scratch = scratch_or_skip!("secondary_f2_move");
    let s = scene("sx4", &scratch.db).await;
    let db = &scratch.db;
    let command = |target: Uuid, key: &str| {
        json!({
            "command": {"type": "move_object", "payload": {"target_object_id": target}},
            "idempotency_key": key
        })
    };
    // Out of the project. The bots hold `full_access` on the page they move, so only the destination
    // is in question.
    for (bot, level) in [(s.scoped.1, "full_access"), (s.runner.1, "full_access")] {
        exec(
            db,
            "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level) \
             VALUES ($1, $2, 'bot', $3, $4)",
            vec![s.workspace.into(), s.page1.into(), bot.into(), level.to_string().into()],
        )
        .await;
    }
    for (kind, token) in bots(&s) {
        let answer = call(
            &s.app,
            "POST",
            &format!("/api/v2/flow/objects/{}/commands", s.page1),
            &token,
            Some(command(s.page2, &format!("out-{kind}"))),
        )
        .await;
        assert!(is_refusal(&answer), "{kind}: {answer:?}");
    }
    // Into a project where the bot holds only `read`.
    let (mixed_token, mixed) = scoped_bot(
        db,
        &s.c,
        "mixed",
        &["read", "write"],
        "none",
        &[(s.p1, "write"), (s.p2, "read")],
    )
    .await;
    exec(
        db,
        "INSERT INTO flow_object_grants (workspace_id, object_id, principal_kind, principal_id, level) \
         VALUES ($1, $2, 'bot', $3, 'full_access')",
        vec![s.workspace.into(), s.page1.into(), mixed.into()],
    )
    .await;
    let answer = call(
        &s.app,
        "POST",
        &format!("/api/v2/flow/objects/{}/commands", s.page1),
        &mixed_token,
        Some(command(s.page2, "into-read-only")),
    )
    .await;
    assert!(is_refusal(&answer), "{answer:?}");
    scratch.drop_self().await;
}

/// F2, link: the far end of a relation is held to the same rule.
#[tokio::test]
async fn a_relation_to_a_page_of_another_project_is_refused() {
    let scratch = scratch_or_skip!("secondary_f2_link");
    let s = scene("sx5", &scratch.db).await;
    for (kind, token) in bots(&s) {
        let answer = call(
            &s.app,
            "POST",
            &format!("/api/v2/flow/objects/{}/commands", s.page1),
            &token,
            Some(json!({
                "command": {"type": "link", "payload": {"target_object_id": s.page2, "relation_type": "related"}},
                "idempotency_key": format!("link-{kind}")
            })),
        )
        .await;
        assert!(is_refusal(&answer), "{kind}: {answer:?}");
    }
    scratch.drop_self().await;
}

/// F2, asset copy: the source page of a copy is read-checked against the bot's level.
#[tokio::test]
async fn an_asset_copy_source_in_another_project_is_refused() {
    let scratch = scratch_or_skip!("secondary_f2_copy");
    let s = scene("sx6", &scratch.db).await;
    for (kind, token) in bots(&s) {
        let answer = call(
            &s.app,
            "POST",
            &format!("/api/v2/flow/objects/{}/body-assets/copy", s.page1),
            &token,
            Some(json!({"source_object_id": s.page2, "asset_id": Uuid::new_v4()})),
        )
        .await;
        assert!(is_refusal(&answer), "{kind}: {answer:?}");
    }
    scratch.drop_self().await;
}

/// F2, references: a reference to a form or a record of another project is refused; one to the bot's
/// own project passes the scope check.
#[tokio::test]
async fn a_reference_target_in_another_project_is_refused() {
    let scratch = scratch_or_skip!("secondary_f2_reference");
    let s = scene("sx7", &scratch.db).await;
    for (kind, token) in bots(&s) {
        let reference = |target_type: &str, target: Uuid, key: &str| json!({"target_type": target_type, "target_id": target, "display": {}, "idempotency_key": key});
        let path = format!("/api/v2/flow/objects/{}/references", s.page1);
        let answer = call(
            &s.app,
            "POST",
            &path,
            &token,
            Some(reference("form", s.form2, &format!("ref2-{kind}"))),
        )
        .await;
        assert!(is_refusal(&answer), "{kind} form in P2: {answer:?}");
        let record = Uuid::new_v4();
        exec(
            &scratch.db,
            "INSERT INTO form_records (id, workspace_id, project_id, form_id, title, values) \
             VALUES ($1, $2, $3, $4, 'secret record', '{}'::jsonb)",
            vec![record.into(), s.workspace.into(), s.p2.into(), s.form2.into()],
        )
        .await;
        let answer = call(
            &s.app,
            "POST",
            &path,
            &token,
            Some(reference("form_record", record, &format!("ref3-{kind}"))),
        )
        .await;
        assert!(is_refusal(&answer), "{kind} record in P2: {answer:?}");
        let answer = call(
            &s.app,
            "POST",
            &path,
            &token,
            Some(reference("form", s.form1, &format!("ref1-{kind}"))),
        )
        .await;
        assert!(
            !is_refusal(&answer),
            "{kind} form in P1 is not a scope refusal: {answer:?}"
        );
    }
    scratch.drop_self().await;
}

/// F3: an issue of P1 that links a page of P2 shows the bot neither the title nor a readable link;
/// a page of P1 stays readable; the bot cannot link the page of P2 itself.
#[tokio::test]
async fn page_links_apply_the_bot_project_level() {
    let scratch = scratch_or_skip!("secondary_f3");
    let s = scene("sx8", &scratch.db).await;
    let issue = Uuid::new_v4();
    exec(
        &scratch.db,
        "INSERT INTO work_items (id, project_id, title, state, created_by) VALUES ($1, $2, 'linker', 'todo', $3)",
        vec![issue.into(), s.p1.into(), s.c.w.owner.into()],
    )
    .await;
    for page in [s.page1, s.page2] {
        exec(
            &scratch.db,
            "INSERT INTO work_item_page_links (work_item_id, object_id, workspace_id, created_by) \
             VALUES ($1, $2, $3, $4)",
            vec![issue.into(), page.into(), s.workspace.into(), s.c.w.owner.into()],
        )
        .await;
    }
    for (kind, token) in bots(&s) {
        let (status, body) = call(&s.app, "GET", &format!("/api/v1/issues/{issue}/pages"), &token, None).await;
        assert_eq!(status, 200, "{kind}: {body}");
        let text = body.to_string();
        assert!(
            text.contains("page one"),
            "{kind}: the page of its own project is shown: {text}"
        );
        assert!(
            !text.contains("page two secret"),
            "{kind}: the title of the other project leaked: {text}"
        );
        let foreign = body["data"]
            .as_array()
            .expect("links")
            .iter()
            .find(|link| link["object_id"] == json!(s.page2))
            .expect("the link is listed");
        assert_ne!(foreign["restricted"], json!(false), "{kind}: {foreign}");

        exec(
            &scratch.db,
            "DELETE FROM work_item_page_links WHERE work_item_id = $1 AND object_id = $2",
            vec![issue.into(), s.page2.into()],
        )
        .await;
        let answer = call(
            &s.app,
            "POST",
            &format!("/api/v1/issues/{issue}/pages"),
            &token,
            Some(json!({"object_id": s.page2})),
        )
        .await;
        assert_ne!(
            answer.1["code"], 0,
            "{kind}: the bot linked a page it cannot read: {answer:?}"
        );
        let linked = count(
            &scratch.db,
            "SELECT count(*) AS count FROM work_item_page_links WHERE work_item_id = $1 AND object_id = $2",
            vec![issue.into(), s.page2.into()],
        )
        .await;
        assert_eq!(linked, 0, "{kind}: no link to the other project's page");
        exec(
            &scratch.db,
            "INSERT INTO work_item_page_links (work_item_id, object_id, workspace_id, created_by) \
             VALUES ($1, $2, $3, $4)",
            vec![issue.into(), s.page2.into(), s.workspace.into(), s.c.w.owner.into()],
        )
        .await;
    }
    scratch.drop_self().await;
}

/// An ordinary bot and a person are not affected: the same requests pass the scope check.
#[tokio::test]
async fn an_unrestricted_bot_is_not_limited() {
    let scratch = scratch_or_skip!("secondary_plain");
    let s = scene("sx9", &scratch.db).await;
    let answer = call(
        &s.app,
        "POST",
        "/api/v2/flow/conversions/preview",
        &s.c.plain_token,
        Some(preview_body(s.page1, s.form2, "plain-cross")),
    )
    .await;
    assert!(!is_refusal(&answer), "{answer:?}");
    scratch.drop_self().await;
}
