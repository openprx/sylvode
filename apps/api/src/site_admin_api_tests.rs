//! Console site rules through the real router: slug rename while a draft, `published_at`, delete only
//! when not published, and the `url` fields. Uses the fixture of `item_api_tests` (`alice` owns the
//! workspace, `bob` is a member) against a scratch `PostgreSQL` (skipped without
//! `OPENPR_TEST_DATABASE_URL`).
#![allow(clippy::indexing_slicing, clippy::too_many_lines, clippy::doc_markdown)]

use super::item_api_tests::{Answer, Api};
use api::services::bots::{self, NewBot};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use sea_orm::TransactionTrait as _;
use serde_json::{Value, json};
use tower::ServiceExt as _;
use uuid::Uuid;

impl Api {
    async fn new_site(&self, slug: &str) -> Uuid {
        let answer = self
            .post(
                &format!("/api/v1/projects/{}/sites", self.project),
                self.alice,
                json!({"slug": slug, "title": "Help"}),
            )
            .await;
        assert_eq!(answer.code(), Some(0), "{}", answer.body);
        Uuid::parse_str(answer.data()["id"].as_str().expect("id")).expect("uuid")
    }

    async fn patch_site(&self, id: Uuid, user: Uuid, body: Value) -> Answer {
        self.call(Method::PATCH, &format!("/api/v1/sites/{id}"), user, Some(body))
            .await
    }

    async fn site_action(&self, id: Uuid, action: &str) -> Answer {
        self.post(&format!("/api/v1/sites/{id}/{action}"), self.alice, json!({}))
            .await
    }
}

#[tokio::test]
async fn a_draft_site_is_renamed_and_a_published_one_is_not() {
    let Some(api) = Api::new("site_slug_rename").await else {
        return;
    };
    let id = api.new_site("help-one").await;
    let other = api.new_site("help-two").await;

    let renamed = api.patch_site(id, api.alice, json!({"slug": "help-new"})).await;
    assert_eq!(renamed.code(), Some(0), "{}", renamed.body);
    assert_eq!(renamed.data()["slug"], json!("help-new"));

    // Same checks as creation: bad shape is 400, a taken slug is 409.
    let bad = api.patch_site(id, api.alice, json!({"slug": "Bad Slug"})).await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST, "{}", bad.body);
    let taken = api.patch_site(id, api.alice, json!({"slug": "help-two"})).await;
    assert_eq!(taken.status, StatusCode::CONFLICT, "{}", taken.body);
    assert!(taken.body.to_string().contains("slug_taken"), "{}", taken.body);

    // A member may not rename; the title still follows the old rule.
    let member = api.patch_site(id, api.bob, json!({"slug": "member-name"})).await;
    assert_eq!(member.status, StatusCode::FORBIDDEN, "{}", member.body);

    assert_eq!(api.site_action(id, "publish").await.code(), Some(0));
    let refused = api.patch_site(id, api.alice, json!({"slug": "help-late"})).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert!(refused.body.to_string().contains("site_not_draft"), "{}", refused.body);

    // Disabled is not a draft either.
    assert_eq!(api.site_action(other, "disable").await.code(), Some(0));
    let disabled = api.patch_site(other, api.alice, json!({"slug": "help-off"})).await;
    assert_eq!(disabled.status, StatusCode::CONFLICT, "{}", disabled.body);

    let empty = api.patch_site(id, api.alice, json!({})).await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST, "{}", empty.body);
    api.finish().await;
}

#[tokio::test]
async fn a_site_editor_and_a_bot_cannot_change_the_slug_and_rights_come_before_shapes() {
    let Some(api) = Api::new("site_slug_rights").await else {
        return;
    };
    let id = api.new_site("help-rights").await;
    let role = api
        .put(
            &format!("/api/v1/sites/{id}/roles/{}", api.bob),
            api.alice,
            json!({"role": "site_editor"}),
        )
        .await;
    assert_eq!(role.code(), Some(0), "{}", role.body);

    // The site editor still changes the title, but not the address.
    let titled = api.patch_site(id, api.bob, json!({"title": "Editor title"})).await;
    assert_eq!(titled.code(), Some(0), "{}", titled.body);
    let editor = api.patch_site(id, api.bob, json!({"slug": "editor-name"})).await;
    assert_eq!(editor.status, StatusCode::FORBIDDEN, "{}", editor.body);

    // A bot with write access is refused the slug as well.
    let tx = api.db.begin().await.expect("begin");
    let bot = bots::create(
        &tx,
        NewBot {
            workspace_id: api.ws,
            created_by: api.alice,
            name: "site bot",
            permissions: vec!["read".to_string(), "write".to_string()],
            transport_surface: "mcp_stdio".to_string(),
            expires_at: None,
        },
    )
    .await
    .expect("bot");
    tx.commit().await.expect("commit");
    let request = Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/v1/sites/{id}"))
        .header(header::AUTHORIZATION, format!("Bearer {}", bot.token))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({"slug": "bot-name"}).to_string()))
        .expect("request");
    let response = api.router.clone().oneshot(request).await.expect("answer");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    // Rights before shapes: a caller who may not rename gets 403 even for a malformed slug or title.
    let outsider = api.patch_site(id, api.carol, json!({"slug": "Bad Slug"})).await;
    assert_ne!(outsider.status, StatusCode::BAD_REQUEST, "{}", outsider.body);
    let plain = api.new_site("help-plain").await;
    let member = api.patch_site(plain, api.bob, json!({"slug": "Bad Slug"})).await;
    assert_eq!(member.status, StatusCode::FORBIDDEN, "{}", member.body);
    let member_title = api.patch_site(plain, api.bob, json!({"title": ""})).await;
    assert_eq!(member_title.status, StatusCode::FORBIDDEN, "{}", member_title.body);

    let after = api.get(&format!("/api/v1/sites/{id}"), api.alice).await;
    assert_eq!(after.data()["slug"], json!("help-rights"));
    api.finish().await;
}

#[tokio::test]
async fn publishing_sets_published_at_and_unpublishing_keeps_it() {
    let Some(api) = Api::new("site_published_at").await else {
        return;
    };
    let id = api.new_site("help-time").await;
    let fresh = api.get(&format!("/api/v1/sites/{id}"), api.alice).await;
    assert_eq!(fresh.data()["published_at"], Value::Null);

    let published = api.site_action(id, "publish").await;
    let first = published.data()["published_at"].clone();
    assert!(first.is_string(), "{}", published.body);

    // Publishing a published site again does not move the time.
    let again = api.site_action(id, "publish").await;
    assert_eq!(again.data()["published_at"], first);

    let unpublished = api.site_action(id, "unpublish").await;
    assert_eq!(unpublished.data()["status"], json!("draft"));
    assert_eq!(unpublished.data()["published_at"], first);

    let listed = api
        .get(&format!("/api/v1/projects/{}/sites", api.project), api.bob)
        .await;
    assert_eq!(listed.data()["items"][0]["published_at"], first, "{}", listed.body);
    api.finish().await;
}

#[tokio::test]
async fn a_site_is_deleted_only_when_not_published() {
    let Some(api) = Api::new("site_delete_published").await else {
        return;
    };
    let id = api.new_site("help-del").await;
    assert_eq!(api.site_action(id, "publish").await.code(), Some(0));
    let uri = format!("/api/v1/sites/{id}");
    let refused = api.call(Method::DELETE, &uri, api.alice, None).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
    assert!(refused.body.to_string().contains("site_published"), "{}", refused.body);

    assert_eq!(api.site_action(id, "disable").await.code(), Some(0));
    let deleted = api.call(Method::DELETE, &uri, api.alice, None).await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);

    let draft = api.new_site("help-del2").await;
    let deleted = api
        .call(Method::DELETE, &format!("/api/v1/sites/{draft}"), api.alice, None)
        .await;
    assert_eq!(deleted.code(), Some(0), "{}", deleted.body);
    api.finish().await;
}

#[tokio::test]
async fn the_url_follows_the_site_origin_or_is_null_when_nothing_serves_it() {
    let Some(api) = Api::new("site_url_fields").await else {
        return;
    };
    let id = api.new_site("help-url").await;
    // The test process serves no site app, so a site without its own origin has no link.
    let plain = api.get(&format!("/api/v1/sites/{id}"), api.alice).await;
    assert_eq!(plain.data()["url"], Value::Null, "{}", plain.body);
    assert_eq!(plain.data()["url_mode"], Value::Null);
    assert_eq!(plain.data()["installation"], Value::Null);

    let set = api
        .call(
            Method::PUT,
            &format!("/api/v1/sites/{id}/settings"),
            api.alice,
            Some(json!({"allowed_origin": "https://help.example.com"})),
        )
        .await;
    assert_eq!(set.code(), Some(0), "{}", set.body);
    assert_eq!(set.data()["url"], json!("https://help.example.com/"));
    assert_eq!(set.data()["url_mode"], json!("separate"));
    api.finish().await;
}
