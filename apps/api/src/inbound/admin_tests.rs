//! The management routes for sources and rules against PostgreSQL, through the real router layer
//! with real session and bot credentials.
#![cfg(test)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::too_many_lines,
    clippy::similar_names,
    clippy::items_after_statements,
    clippy::print_stderr,
    clippy::unused_self,
    clippy::doc_markdown
)]

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
    middleware::from_fn_with_state,
    routing::{get, post},
};
use platform::{app::AppState, auth::JwtManager};
use sea_orm::ConnectionTrait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use uuid::Uuid;

use super::db_tests::{World, enabled};
use crate::{
    middleware::bot_auth::bot_or_user_auth_middleware,
    oauth::{
        delegated::{DelegatedAuthContext, with_context},
        scopes::{ProjectScope, ScopeSet, TokenResource},
    },
    routes::{context::tenant_fixture::exec, inbound_admin},
};

/// The management routes as `main.rs` registers them, each behind the session-or-bot layer.
fn router(state: &AppState) -> Router<AppState> {
    let auth = || from_fn_with_state(state.clone(), bot_or_user_auth_middleware);
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/inbound-sources",
            get(inbound_admin::list_sources)
                .post(inbound_admin::create_source)
                .route_layer(auth()),
        )
        .route(
            "/api/v1/inbound-sources/{id}",
            get(inbound_admin::get_source)
                .patch(inbound_admin::patch_source)
                .delete(inbound_admin::delete_source)
                .route_layer(auth()),
        )
        .route(
            "/api/v1/inbound-sources/{id}/rotate",
            post(inbound_admin::rotate_source).route_layer(auth()),
        )
        .route(
            "/api/v1/inbound-sources/{id}/events",
            get(inbound_admin::list_source_events).route_layer(auth()),
        )
        .route(
            "/api/v1/projects/{project_id}/alert-rules",
            get(inbound_admin::list_rules)
                .post(inbound_admin::create_rule)
                .route_layer(auth()),
        )
        .route(
            "/api/v1/projects/{project_id}/alert-rules/reorder",
            post(inbound_admin::reorder_rules).route_layer(auth()),
        )
        .route(
            "/api/v1/projects/{project_id}/alert-rules/test",
            post(inbound_admin::test_rules).route_layer(auth()),
        )
        .route(
            "/api/v1/issues/{id}/alert",
            get(crate::routes::inbound_events::get_issue_alert).route_layer(auth()),
        )
        .route(
            "/api/v1/alert-rules/{id}",
            get(inbound_admin::get_rule)
                .put(inbound_admin::replace_rule)
                .delete(inbound_admin::delete_rule)
                .route_layer(auth()),
        )
}

macro_rules! world {
    ($label:expr) => {{
        let scratch = crate::scratch_or_skip!($label);
        World::new(scratch, "adm").await
    }};
}

impl World {
    fn jwt(&self, user: Uuid) -> String {
        JwtManager::new(self.state.cfg.jwt_secret.expose(), 900, 3600)
            .issue_access_token(&user.to_string(), &format!("{user}@inbound.test"))
            .unwrap()
    }

    /// Calls a management route. Returns the HTTP status and the JSON body; the API's own routes
    /// carry a refusal in the body's `code`.
    async fn call(&self, method: Method, uri: &str, token: Option<&str>, body: Option<&Value>) -> (StatusCode, Value) {
        let router = router(&self.state).with_state(self.state.clone());
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => builder
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let response = router.oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn as_admin(&self, method: Method, uri: &str, body: Option<&Value>) -> Value {
        let token = self.jwt(self.admin);
        self.call(method, uri, Some(&token), body).await.1
    }

    fn sources_uri(&self) -> String {
        format!("/api/v1/projects/{}/inbound-sources", self.tenant.project_id)
    }

    fn rules_uri(&self) -> String {
        format!("/api/v1/projects/{}/alert-rules", self.tenant.project_id)
    }

    async fn bot_token(&self, permissions: &str) -> String {
        let raw = format!("opr_{}", "c".repeat(64));
        let hash = hex::encode(Sha256::digest(raw.as_bytes()));
        exec(
            self.db(),
            "INSERT INTO workspace_bots (id, workspace_id, name, token_hash, token_prefix, permissions, created_by) \
             VALUES ($1, $2, 'admin bot', $3, 'opr_cccc', $4::jsonb, $5)",
            vec![
                Uuid::new_v4().into(),
                self.tenant.workspace_id.into(),
                hash.into(),
                permissions.to_owned().into(),
                self.admin.into(),
            ],
        )
        .await;
        raw
    }
}

#[tokio::test]
async fn an_admin_creates_a_source_and_the_secrets_appear_only_in_that_answer() {
    let w = world!("adm_create");
    let created = w
        .as_admin(
            Method::POST,
            &w.sources_uri(),
            Some(&json!({ "name": "prx-waf edge-02", "rate_per_minute": 50 })),
        )
        .await;
    assert_eq!(created["code"], 0, "{created}");
    let token = created["data"]["token"].as_str().unwrap().to_owned();
    let secret = created["data"]["signing_secret"].as_str().unwrap().to_owned();
    assert!(token.starts_with("svi_") && secret.starts_with("svs_"));
    let id = created["data"]["source"]["id"].as_str().unwrap().to_owned();
    assert_eq!(created["data"]["source"]["acting_user_id"], json!(w.admin));
    assert_eq!(created["data"]["source"]["format"], "native_v1");
    assert_eq!(created["data"]["source"]["rate_per_minute"], 50);
    assert_eq!(created["data"]["source"]["require_signature"], true);
    assert_eq!(created["data"]["source"]["has_signing_secret"], true);

    // Only the hash is stored for the token.
    assert_eq!(
        w.n(&format!(
            "SELECT count(*) AS count FROM inbound_sources WHERE id = '{id}' AND token_hash = '{}' AND token_hash <> '{token}'",
            super::credentials::hash_token(&token)
        ))
        .await,
        1
    );

    // No later answer carries a secret, a hash or a token.
    let hash = super::credentials::hash_token(&token);
    for (method, uri) in [
        (Method::GET, w.sources_uri()),
        (Method::GET, format!("/api/v1/inbound-sources/{id}")),
        (Method::GET, format!("/api/v1/inbound-sources/{id}/events")),
    ] {
        let body = w.as_admin(method, &uri, None).await;
        assert_eq!(body["code"], 0, "{uri}: {body}");
        let text = body.to_string();
        for secret_text in [&token, &secret, &hash] {
            assert!(!text.contains(secret_text.as_str()), "{uri} leaked a secret");
        }
        assert!(
            !text.contains("token_hash") && !text.contains("\"signing_secret\""),
            "{uri}: {text}"
        );
    }
    // The new token works at the inbound endpoint, signed with the new secret.
    let event = w.event("created-1", "k", "info");
    let bytes = serde_json::to_vec(&event).unwrap();
    let ts = platform::timestamp::Timestamp::now().unix_timestamp();
    let headers = vec![
        ("authorization", format!("Bearer {token}")),
        ("content-type", "application/json".to_owned()),
        ("x-sylvode-timestamp", ts.to_string()),
        (
            "x-sylvode-signature",
            format!(
                "v1={}",
                super::auth::compute_signature(&secret, &ts.to_string(), &bytes)
            ),
        ),
    ];
    let (status, _, _) = w.send(enabled(), &headers, &bytes, super::INBOUND_EVENTS_PATH).await;
    assert_eq!(status, StatusCode::ACCEPTED);

    // The audit trail records the change without a secret.
    assert_eq!(
        w.n("SELECT count(*) AS count FROM business_events WHERE event_type = 'inbound_source.created'")
            .await,
        1
    );
    assert_eq!(
        w.n(&format!(
            "SELECT count(*) AS count FROM business_events WHERE payload::text LIKE '%{token}%' \
             OR payload::text LIKE '%{secret}%' OR payload::text LIKE '%{hash}%'"
        ))
        .await,
        0
    );
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn members_bots_outsiders_and_anonymous_callers_are_refused_and_nothing_changes() {
    let w = world!("adm_refuse");
    let body = json!({ "name": "should not exist" });
    let member = w.jwt(w.tenant.member_id);
    let (_, answer) = w.call(Method::POST, &w.sources_uri(), Some(&member), Some(&body)).await;
    assert_eq!(answer["code"], 403, "{answer}");
    let (_, answer) = w.call(Method::GET, &w.sources_uri(), Some(&member), None).await;
    assert_eq!(answer["code"], 403);

    // A bot with every permission is refused as well.
    let bot = w.bot_token(r#"["read","write","admin"]"#).await;
    for (method, uri, payload) in [
        (Method::POST, w.sources_uri(), Some(&body)),
        (Method::GET, w.sources_uri(), None),
        (
            Method::POST,
            w.rules_uri(),
            Some(&json!({ "name": "r", "action": "work_item" })),
        ),
        (Method::GET, w.rules_uri(), None),
    ] {
        let (_, answer) = w.call(method, &uri, Some(&bot), payload).await;
        assert_eq!(answer["code"], 403, "{uri}: {answer}");
    }

    // A person who is not in the workspace cannot even tell the project exists.
    let outsider = Uuid::new_v4();
    exec(
        w.db(),
        "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'out', 'user', true)",
        vec![outsider.into(), format!("{outsider}@inbound.test").into()],
    )
    .await;
    let outsider_token = w.jwt(outsider);
    let (_, answer) = w.call(Method::GET, &w.sources_uri(), Some(&outsider_token), None).await;
    assert_eq!(answer["code"], 404);

    let (_, answer) = w.call(Method::GET, &w.sources_uri(), None, None).await;
    assert_eq!(answer["code"], 401);
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_sources WHERE name = 'should not exist'")
            .await,
        0
    );
    assert_eq!(
        w.n("SELECT count(*) AS count FROM alert_rules WHERE name = 'r'").await,
        0
    );

    // The same refusals apply to an existing source and rule by id.
    let source_uri = format!("/api/v1/inbound-sources/{}", w.source_id);
    for (method, uri, payload) in [
        (Method::GET, source_uri.clone(), None),
        (Method::PATCH, source_uri.clone(), Some(json!({ "enabled": false }))),
        (Method::POST, format!("{source_uri}/rotate"), None),
        (Method::DELETE, source_uri.clone(), None),
        (Method::GET, format!("{source_uri}/events"), None),
        (Method::GET, format!("/api/v1/alert-rules/{}", w.rule_id), None),
        (Method::DELETE, format!("/api/v1/alert-rules/{}", w.rule_id), None),
    ] {
        for token in [&member, &bot] {
            let (_, answer) = w.call(method.clone(), &uri, Some(token), payload.as_ref()).await;
            assert_eq!(answer["code"], 403, "{method} {uri}: {answer}");
        }
    }
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_sources WHERE enabled").await,
        1
    );
    assert_eq!(w.n("SELECT count(*) AS count FROM alert_rules").await, 1);
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn a_delegated_caller_is_refused_even_with_every_scope() {
    let w = world!("adm_delegated");
    let context = || DelegatedAuthContext {
        grant_id: Uuid::new_v4(),
        token_id: Uuid::new_v4(),
        client_id: std::sync::Arc::from("ramflux"),
        user_id: w.admin,
        workspace_id: w.tenant.workspace_id,
        scopes: ScopeSet::all(),
        projects: ProjectScope::All,
        resource: TokenResource::Api,
        surface: crate::middleware::bot_auth::EventSurface::Rest,
        tool_name: None,
        request_id: Uuid::new_v4(),
    };
    // The handler layer on its own: the identity is a plain member inside a delegated request.
    let token = w.jwt(w.admin);
    let (_, answer) = with_context(context(), async {
        w.call(
            Method::POST,
            &w.sources_uri(),
            Some(&token),
            Some(&json!({ "name": "via delegation" })),
        )
        .await
    })
    .await;
    assert_eq!(answer["code"], 403, "{answer}");
    let (_, answer) = with_context(context(), async {
        w.call(Method::GET, &w.rules_uri(), Some(&token), None).await
    })
    .await;
    assert_eq!(answer["code"], 403);
    assert_eq!(w.n("SELECT count(*) AS count FROM inbound_sources").await, 1);

    // The routes are not on the delegated allow-list, so the route layer refuses them first.
    for entry in crate::oauth::delegated::DELEGATED_ROUTES {
        assert!(
            !entry.pattern.contains("inbound")
                && !entry.pattern.contains("alert-rules")
                && !entry.pattern.ends_with("/alert"),
            "{} {} must stay off the allow-list",
            entry.method,
            entry.pattern
        );
    }
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn a_source_can_be_edited_taken_over_rotated_and_deleted() {
    let w = world!("adm_lifecycle");
    let uri = format!("/api/v1/inbound-sources/{}", w.source_id);

    let patched = w
        .as_admin(
            Method::PATCH,
            &uri,
            Some(&json!({ "name": "renamed", "rate_per_minute": 7, "daily_cap": 70, "require_signature": false })),
        )
        .await;
    assert_eq!(patched["code"], 0, "{patched}");
    assert_eq!(patched["data"]["name"], "renamed");
    assert_eq!(patched["data"]["rate_per_minute"], 7);
    assert_eq!(patched["data"]["require_signature"], false);
    assert_eq!(patched["data"]["daily_cap"], 70);

    // Invalid values and fields that do not exist are refused.
    for bad in [
        json!({ "rate_per_minute": 0 }),
        json!({ "daily_cap": 1_000_001 }),
        json!({ "name": "" }),
        json!({ "ip_allowlist": ["192.0.2.1/24"] }),
        json!({ "ip_allowlist": ["::/0"] }),
    ] {
        let answer = w.as_admin(Method::PATCH, &uri, Some(&bad)).await;
        assert_eq!(answer["code"], 400, "{bad}: {answer}");
    }
    let token = w.jwt(w.admin);
    let (status, _) = w
        .call(
            Method::PATCH,
            &uri,
            Some(&token),
            Some(&json!({ "format_settings": {} })),
        )
        .await;
    assert!(status.is_client_error(), "an unknown field is refused");

    // Another admin takes the source over.
    let other = Uuid::new_v4();
    exec(
        w.db(),
        "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'o', 'user', true)",
        vec![other.into(), format!("{other}@inbound.test").into()],
    )
    .await;
    exec(
        w.db(),
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'owner')",
        vec![w.tenant.workspace_id.into(), other.into()],
    )
    .await;
    let other_token = w.jwt(other);
    let (_, taken) = w
        .call(
            Method::PATCH,
            &uri,
            Some(&other_token),
            Some(&json!({ "take_over": true })),
        )
        .await;
    assert_eq!(taken["data"]["acting_user_id"], json!(other), "{taken}");

    // Disabling stops the source at the endpoint.
    let disabled = w
        .as_admin(Method::PATCH, &uri, Some(&json!({ "enabled": false })))
        .await;
    assert_eq!(disabled["data"]["enabled"], false);
    let (status, _) = w.post(&w.event("e-1", "k", "info")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    w.as_admin(
        Method::PATCH,
        &uri,
        Some(&json!({ "enabled": true, "require_signature": true })),
    )
    .await;

    // Rotating invalidates the old token and secret at once and returns new ones once.
    let old_token = w.token.clone();
    let rotated = w.as_admin(Method::POST, &format!("{uri}/rotate"), None).await;
    assert_eq!(rotated["code"], 0, "{rotated}");
    let new_token = rotated["data"]["token"].as_str().unwrap().to_owned();
    let new_secret = rotated["data"]["signing_secret"].as_str().unwrap().to_owned();
    assert_ne!(new_token, old_token);
    assert_ne!(new_secret, w.secret);
    let (status, _) = w.post(&w.event("e-2", "k", "info")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the old credentials are dead");
    let event = serde_json::to_vec(&w.event("e-3", "k", "info")).unwrap();
    let ts = platform::timestamp::Timestamp::now().unix_timestamp().to_string();
    let headers = vec![
        ("authorization", format!("Bearer {new_token}")),
        ("content-type", "application/json".to_owned()),
        ("x-sylvode-timestamp", ts.clone()),
        (
            "x-sylvode-signature",
            format!("v1={}", super::auth::compute_signature(&new_secret, &ts, &event)),
        ),
    ];
    let (status, _, body) = w.send(enabled(), &headers, &event, super::INBOUND_EVENTS_PATH).await;
    // The acting user was taken over by another admin, who is an owner: still allowed.
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(
        w.n("SELECT count(*) AS count FROM business_events WHERE event_type = 'inbound_source.rotated'")
            .await,
        1
    );
    assert_eq!(
        w.n("SELECT count(*) AS count FROM business_events WHERE event_type = 'inbound_source.updated'")
            .await,
        4
    );

    // Events are listed with their payload, newest first, paged by `before`.
    let listed = w.as_admin(Method::GET, &format!("{uri}/events?limit=1"), None).await;
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);
    assert_eq!(listed["data"][0]["event_id"], "e-3");
    assert_eq!(listed["data"][0]["payload"]["requests"], 1240);
    let before = listed["data"][0]["received_at"].as_str().unwrap().to_owned();
    let again = w
        .as_admin(
            Method::GET,
            &format!("{uri}/events?before={}", before.replace('+', "%2B")),
            None,
        )
        .await;
    assert_eq!(again["data"].as_array().unwrap().len(), 0);
    let bad = w
        .as_admin(Method::GET, &format!("{uri}/events?before=yesterday"), None)
        .await;
    assert_eq!(bad["code"], 400);

    // Deleting removes the source with its events, its alert links and its bound rules; work items
    // stay.
    assert_eq!(w.post(&w.event("e-9", "k", "high")).await.0, StatusCode::UNAUTHORIZED);
    let items_before = w.n("SELECT count(*) AS count FROM work_items").await;
    let deleted = w.as_admin(Method::DELETE, &uri, None).await;
    assert_eq!(deleted["data"]["deleted"], true);
    assert_eq!(w.n("SELECT count(*) AS count FROM inbound_sources").await, 0);
    assert_eq!(w.n("SELECT count(*) AS count FROM inbound_events").await, 0);
    assert_eq!(w.n("SELECT count(*) AS count FROM work_items").await, items_before);
    assert_eq!(
        w.n("SELECT count(*) AS count FROM business_events WHERE event_type = 'inbound_source.deleted'")
            .await,
        1
    );
    let gone = w.as_admin(Method::GET, &uri, None).await;
    assert_eq!(gone["code"], 404);
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn source_creation_validates_its_input() {
    let w = world!("adm_source_input");
    for bad in [
        json!({ "name": "" }),
        json!({ "name": "x", "format": "prx_waf_v1" }),
        json!({ "name": "x", "rate_per_minute": 6001 }),
        json!({ "name": "x", "daily_cap": 0 }),
        json!({ "name": "x", "ip_allowlist": ["192.0.2.1/24"] }),
        json!({ "name": "x", "ip_allowlist": ["0.0.0.0/0"] }),
        json!({ "name": "x", "ip_allowlist": ["not an address"] }),
    ] {
        let answer = w.as_admin(Method::POST, &w.sources_uri(), Some(&bad)).await;
        assert_eq!(answer["code"], 400, "{bad}: {answer}");
    }
    let token = w.jwt(w.admin);
    let (status, _) = w
        .call(
            Method::POST,
            &w.sources_uri(),
            Some(&token),
            Some(&json!({ "name": "x", "source": "other" })),
        )
        .await;
    assert!(status.is_client_error());
    // Adapter settings no longer exist: the field is unknown.
    let (status, _) = w
        .call(
            Method::POST,
            &w.sources_uri(),
            Some(&token),
            Some(&json!({ "name": "x", "format_settings": {} })),
        )
        .await;
    assert!(status.is_client_error(), "format_settings is not a field any more");
    assert_eq!(w.n("SELECT count(*) AS count FROM inbound_sources").await, 1);
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn rules_are_created_validated_replaced_with_a_version_reordered_and_deleted() {
    let w = world!("adm_rules");
    // The seeded rule is the first; a created rule is appended.
    let created = w
        .as_admin(
            Method::POST,
            &w.rules_uri(),
            Some(&json!({
                "name": "WAF critical",
                "kind_patterns": ["waf.*"],
                "min_severity": "critical",
                "bot_user_id": w.bot,
                "instructions": "Look at {alert_identifier}",
                "notify_user_ids": [w.tenant.member_id],
            })),
        )
        .await;
    assert_eq!(created["code"], 0, "{created}");
    let rule = &created["data"]["rule"];
    assert_eq!(rule["action"], "work_item_and_task", "high and above default to a task");
    assert_eq!(rule["version"], 1);
    assert_eq!(rule["position"], 20);
    assert_eq!(
        created["data"]["warnings"],
        json!(["bot_has_no_active_webhook"]),
        "the save warns and does not refuse"
    );
    let rule_id = rule["id"].as_str().unwrap().to_owned();

    // An active webhook subscription of the bot clears the warning.
    exec(
        w.db(),
        "INSERT INTO webhooks (workspace_id, name, url, secret, events, active, created_by, bot_user_id) \
         VALUES ($1, 'bot hook', 'https://hooks.example.test/x', 's', '[\"issue.assigned\"]'::jsonb, true, $2, $3)",
        vec![w.tenant.workspace_id.into(), w.admin.into(), w.bot.into()],
    )
    .await;
    let uri = format!("/api/v1/alert-rules/{rule_id}");
    let current = w.as_admin(Method::GET, &uri, None).await;
    assert_eq!(current["data"]["name"], "WAF critical");

    let mut replacement = json!({
        "name": "WAF critical v2",
        "kind_patterns": ["waf.*", "sd.threat_detected"],
        "min_severity": "critical",
        "bot_user_id": w.bot,
        "notify_user_ids": [w.tenant.member_id],
        "max_tasks_per_hour": 9,
        "expected_version": 1,
    });
    let replaced = w.as_admin(Method::PUT, &uri, Some(&replacement)).await;
    assert_eq!(replaced["code"], 0, "{replaced}");
    assert_eq!(replaced["data"]["rule"]["version"], 2);
    assert_eq!(replaced["data"]["rule"]["max_tasks_per_hour"], 9);
    assert_eq!(replaced["data"]["warnings"], json!([]));
    // A stale version is refused and changes nothing.
    let stale = w.as_admin(Method::PUT, &uri, Some(&replacement)).await;
    assert_ne!(stale["code"], 0, "{stale}");
    assert_eq!(
        w.n(&format!(
            "SELECT count(*) AS count FROM alert_rules WHERE id = '{rule_id}' AND version = 2"
        ))
        .await,
        1
    );
    replacement.as_object_mut().unwrap().remove("expected_version");
    let missing = w.as_admin(Method::PUT, &uri, Some(&replacement)).await;
    assert_eq!(missing["code"], 400);

    // References are checked against the workspace and the project.
    let other_workspace_label = Uuid::new_v4();
    let other_ws = Uuid::new_v4();
    exec(
        w.db(),
        "INSERT INTO workspaces (id, slug, name, created_by) VALUES ($1, $2, 'other', $3)",
        vec![other_ws.into(), format!("ws-{other_ws}").into(), w.admin.into()],
    )
    .await;
    exec(
        w.db(),
        "INSERT INTO labels (id, workspace_id, name) VALUES ($1, $2, 'foreign')",
        vec![other_workspace_label.into(), other_ws.into()],
    )
    .await;
    let human_member = w.tenant.member_id;
    let not_a_bot = human_member;
    for (what, bad) in [
        ("a person as the bot", json!({ "name": "r", "bot_user_id": not_a_bot })),
        ("an unknown bot", json!({ "name": "r", "bot_user_id": Uuid::new_v4() })),
        (
            "a bot with a work_item action",
            json!({ "name": "r", "action": "work_item", "bot_user_id": w.bot }),
        ),
        (
            "a task action without a bot",
            json!({ "name": "r", "action": "work_item_and_task" }),
        ),
        (
            "a foreign label",
            json!({ "name": "r", "action": "work_item", "label_ids": [other_workspace_label] }),
        ),
        (
            "a bot as recipient",
            json!({ "name": "r", "action": "work_item", "notify_user_ids": [w.bot] }),
        ),
        (
            "an outsider as recipient",
            json!({ "name": "r", "action": "work_item", "notify_user_ids": [Uuid::new_v4()] }),
        ),
        (
            "an unknown source",
            json!({ "name": "r", "action": "work_item", "source_id": Uuid::new_v4() }),
        ),
        (
            "a bad pattern",
            json!({ "name": "r", "action": "work_item", "kind_patterns": ["*"] }),
        ),
        (
            "a version on create",
            json!({ "name": "r", "action": "work_item", "expected_version": 1 }),
        ),
    ] {
        let answer = w.as_admin(Method::POST, &w.rules_uri(), Some(&bad)).await;
        assert_eq!(answer["code"], 400, "{what}: {answer}");
    }
    assert_eq!(
        w.n("SELECT count(*) AS count FROM alert_rules").await,
        2,
        "no refused rule was stored"
    );

    // A rule may be bound to a source of the project.
    let bound = w
        .as_admin(
            Method::POST,
            &w.rules_uri(),
            Some(&json!({ "name": "bound", "action": "work_item", "source_id": w.source_id })),
        )
        .await;
    assert_eq!(bound["code"], 0, "{bound}");
    assert_eq!(
        bound["data"]["rule"]["notify_user_ids"],
        json!([w.admin]),
        "the author is the default recipient"
    );
    assert_eq!(bound["data"]["rule"]["action"], "work_item");

    // The list is ordered by position; reorder needs every rule exactly once.
    let listed = w.as_admin(Method::GET, &w.rules_uri(), None).await;
    let ids: Vec<String> = listed["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(ids.len(), 3);
    let mut reversed = ids.clone();
    reversed.reverse();
    let reorder_uri = format!("{}/reorder", w.rules_uri());
    let short = w
        .as_admin(Method::POST, &reorder_uri, Some(&json!({ "rule_ids": [ids[0]] })))
        .await;
    assert_eq!(short["code"], 400);
    let duplicated = w
        .as_admin(
            Method::POST,
            &reorder_uri,
            Some(&json!({ "rule_ids": [ids[0], ids[0], ids[1], ids[2]] })),
        )
        .await;
    assert_eq!(duplicated["code"], 400);
    let reordered = w
        .as_admin(Method::POST, &reorder_uri, Some(&json!({ "rule_ids": reversed })))
        .await;
    assert_eq!(reordered["code"], 0, "{reordered}");
    let after: Vec<String> = reordered["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(after, reversed);

    // Deleting a rule leaves its alerts and work items.
    let deleted = w.as_admin(Method::DELETE, &uri, None).await;
    assert_eq!(deleted["data"]["deleted"], true);
    assert_eq!(w.n("SELECT count(*) AS count FROM alert_rules").await, 2);
    assert_eq!(w.as_admin(Method::GET, &uri, None).await["code"], 404);

    for event in ["created", "updated", "deleted", "reordered"] {
        assert!(
            w.n(&format!(
                "SELECT count(*) AS count FROM business_events WHERE event_type = 'alert_rule.{event}'"
            ))
            .await
                >= 1,
            "{event}"
        );
    }
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn a_rule_created_through_the_api_drives_the_ingest_path() {
    let w = world!("adm_rule_e2e");
    exec(w.db(), "DELETE FROM alert_rules", vec![]).await;
    let created = w
        .as_admin(
            Method::POST,
            &w.rules_uri(),
            Some(&json!({
                "name": "api rule",
                "kind_patterns": ["waf.*"],
                "min_severity": "high",
                "bot_user_id": w.bot,
                "instructions": "Triage {alert_identifier}",
                "notify_user_ids": [w.tenant.member_id],
            })),
        )
        .await;
    assert_eq!(created["code"], 0, "{created}");
    assert_eq!(w.post(&w.event("e-1", "k", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.n("SELECT count(*) AS count FROM ai_tasks").await, 1);
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_events WHERE outcome = 'alert_opened'")
            .await,
        1
    );
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn the_ip_allow_list_is_validated_stored_normalised_listed_and_cleared() {
    let w = world!("adm_allowlist");
    let uri = format!("/api/v1/inbound-sources/{}", w.source_id);
    let patched = w
        .as_admin(
            Method::PATCH,
            &uri,
            Some(&json!({ "ip_allowlist": ["192.0.2.5", " 198.51.100.0/24 ", "2001:db8::/32", "192.0.2.5/32"] })),
        )
        .await;
    assert_eq!(patched["code"], 0, "{patched}");
    assert_eq!(
        patched["data"]["ip_allowlist"],
        json!(["192.0.2.5/32", "198.51.100.0/24", "2001:db8::/32"])
    );
    let shown = w.as_admin(Method::GET, &uri, None).await;
    assert_eq!(shown["data"]["ip_allowlist"][1], "198.51.100.0/24");
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_sources WHERE cardinality(ip_allowlist) = 3")
            .await,
        1
    );
    assert_eq!(
        w.n(
            "SELECT count(*) AS count FROM business_events WHERE event_type = 'inbound_source.updated' \
             AND payload::text LIKE '%198.51.100.0/24%'"
        )
        .await,
        1,
        "the change is audited with the new list"
    );

    // A refused entry changes nothing.
    let refused = w
        .as_admin(
            Method::PATCH,
            &uri,
            Some(&json!({ "ip_allowlist": ["192.0.2.0/24", "192.0.2.9/24"] })),
        )
        .await;
    assert_eq!(refused["code"], 400, "{refused}");
    let too_many: Vec<String> = (0..65).map(|index| format!("192.0.2.{index}")).collect();
    let refused = w
        .as_admin(Method::PATCH, &uri, Some(&json!({ "ip_allowlist": too_many })))
        .await;
    assert_eq!(refused["code"], 400);
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_sources WHERE cardinality(ip_allowlist) = 3")
            .await,
        1
    );

    // A patch that does not mention the list keeps it; an empty list clears it.
    w.as_admin(Method::PATCH, &uri, Some(&json!({ "name": "renamed" })))
        .await;
    assert_eq!(
        w.n("SELECT count(*) AS count FROM inbound_sources WHERE cardinality(ip_allowlist) = 3")
            .await,
        1
    );
    let cleared = w
        .as_admin(Method::PATCH, &uri, Some(&json!({ "ip_allowlist": [] })))
        .await;
    assert_eq!(cleared["data"]["ip_allowlist"], json!([]));

    // A list can be given when the source is created.
    let created = w
        .as_admin(
            Method::POST,
            &w.sources_uri(),
            Some(&json!({ "name": "with list", "ip_allowlist": ["203.0.113.0/24"] })),
        )
        .await;
    assert_eq!(
        created["data"]["source"]["ip_allowlist"],
        json!(["203.0.113.0/24"]),
        "{created}"
    );
    w.scratch.drop_self().await;
}

/// Row counts of every table a rule test must leave alone.
async fn footprint(w: &World) -> Vec<i64> {
    let mut counts = Vec::new();
    for table in [
        "inbound_events",
        "inbound_alerts",
        "work_items",
        "ai_tasks",
        "ai_task_events",
        "notifications",
        "business_events",
        "oauth_rate_hits",
        "activities",
    ] {
        counts.push(w.n(&format!("SELECT count(*) AS count FROM {table}")).await);
    }
    counts
}

#[tokio::test]
async fn the_rule_test_forecasts_the_outcome_and_creates_nothing() {
    let w = world!("adm_rule_test");
    let uri = format!("{}/test", w.rules_uri());
    let sample = |severity: &str, kind: &str, key: &str| {
        let mut event = w.event("sample-1", key, severity);
        event["kind"] = json!(kind);
        event
    };
    let ask = |body: Value| {
        let uri = uri.clone();
        let w = &w;
        async move { w.as_admin(Method::POST, &uri, Some(&body)).await }
    };
    let before = footprint(&w).await;

    // A high waf event: the seeded rule matches, a new alert and a task would start at once.
    let answer = ask(json!({ "source_id": w.source_id, "envelope": sample("high", "waf.attack_detected", "k") })).await;
    assert_eq!(answer["code"], 0, "{answer}");
    let data = &answer["data"];
    assert_eq!(data["matched"], true);
    assert_eq!(data["rule"]["id"], json!(w.rule_id));
    assert_eq!(data["alert"], "would_open");
    assert_eq!(data["task"], "would_start");
    assert_eq!(data["notify_recipients"], 1);
    assert_eq!(
        data["title"],
        "[HIGH] prx-waf edge-01 · waf.attack_detected · shop.example.com"
    );
    assert_eq!(data["caps"]["max_tasks_per_hour"], 6);
    assert_eq!(data["caps"]["active_for_rule"], 0);
    assert_eq!(data["rules_evaluated"][0]["reason"], "matched");

    // Below the rule's minimum, and a kind it does not list: nothing matches.
    let answer =
        ask(json!({ "source_id": w.source_id, "envelope": sample("medium", "waf.attack_detected", "k") })).await;
    assert_eq!(answer["data"]["matched"], false);
    assert_eq!(answer["data"]["alert"], "no_rule");
    assert_eq!(answer["data"]["rules_evaluated"][0]["reason"], "below_minimum_severity");
    let answer = ask(json!({ "source_id": w.source_id, "envelope": sample("high", "sd.threat", "k") })).await;
    assert_eq!(answer["data"]["rules_evaluated"][0]["reason"], "kind_not_listed");

    // The test counts against no limit and writes nothing.
    assert_eq!(
        footprint(&w).await,
        before,
        "the rule test created or counted something"
    );

    // With an open alert and a taken slot the forecast follows the real state.
    assert_eq!(w.post(&w.event("e-1", "k", "medium")).await.0, StatusCode::ACCEPTED);
    exec(w.db(), "UPDATE alert_rules SET min_severity = 'info'", vec![]).await;
    assert_eq!(w.post(&w.event("e-2", "k", "medium")).await.0, StatusCode::ACCEPTED);
    let after_real = footprint(&w).await;
    let answer =
        ask(json!({ "source_id": w.source_id, "envelope": sample("critical", "waf.attack_detected", "k") })).await;
    assert_eq!(answer["data"]["alert"], "would_attach");
    assert_eq!(answer["data"]["would_escalate"], true);
    assert_eq!(answer["data"]["task"], "already_active");
    let answer =
        ask(json!({ "source_id": w.source_id, "envelope": sample("critical", "waf.attack_detected", "other") })).await;
    assert_eq!(answer["data"]["alert"], "would_open");
    assert_eq!(answer["data"]["task"], "would_start");
    exec(w.db(), "UPDATE alert_rules SET max_active_tasks = 1", vec![]).await;
    let answer =
        ask(json!({ "source_id": w.source_id, "envelope": sample("critical", "waf.attack_detected", "other") })).await;
    assert_eq!(answer["data"]["task"], "would_queue");
    assert_eq!(answer["data"]["queue_cause"], "active_cap");
    assert_eq!(answer["data"]["caps"]["active_for_rule"], 1);
    // A stored event can be tested again by id.
    let stored = w
        .scratch
        .db
        .query_one(sea_orm::Statement::from_string(
            sea_orm::DbBackend::Postgres,
            "SELECT id FROM inbound_events WHERE event_id = 'e-1'".to_owned(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Uuid>("", "id")
        .unwrap();
    let answer = ask(json!({ "source_id": w.source_id, "inbound_event_id": stored })).await;
    assert_eq!(answer["data"]["matched"], true, "{answer}");
    assert_eq!(footprint(&w).await, after_real, "still nothing written");

    // Input errors.
    for bad in [
        json!({ "source_id": w.source_id }),
        json!({ "source_id": w.source_id, "envelope": sample("high", "waf.a", "k"), "inbound_event_id": stored }),
        json!({ "source_id": w.source_id, "envelope": { "spec": "nope" } }),
        json!({ "source_id": Uuid::new_v4(), "envelope": sample("high", "waf.a", "k") }),
    ] {
        let answer = ask(bad.clone()).await;
        assert_eq!(answer["code"], 400, "{bad}: {answer}");
    }
    let answer = ask(json!({ "source_id": w.source_id, "inbound_event_id": Uuid::new_v4() })).await;
    assert_eq!(answer["code"], 404);
    assert_eq!(footprint(&w).await, after_real);

    // Members, bots and anonymous callers are refused like the rest of the management API.
    let member = w.jwt(w.tenant.member_id);
    let body = json!({ "source_id": w.source_id, "envelope": sample("high", "waf.a", "k") });
    let (_, answer) = w.call(Method::POST, &uri, Some(&member), Some(&body)).await;
    assert_eq!(answer["code"], 403, "{answer}");
    let bot = w.bot_token(r#"["read","write","admin"]"#).await;
    let (_, answer) = w.call(Method::POST, &uri, Some(&bot), Some(&body)).await;
    assert_eq!(answer["code"], 403, "{answer}");
    let (_, answer) = w.call(Method::POST, &uri, None, Some(&body)).await;
    assert_eq!(answer["code"], 401);
    w.scratch.drop_self().await;
}

#[tokio::test]
async fn the_alert_of_a_work_item_is_readable_by_members_and_no_one_else() {
    let w = world!("adm_alert_view");
    assert_eq!(w.post(&w.event("e-1", "k", "high")).await.0, StatusCode::ACCEPTED);
    assert_eq!(w.post(&w.event("e-2", "k", "critical")).await.0, StatusCode::ACCEPTED);
    let item = w.n("SELECT count(*) AS count FROM work_items").await;
    assert_eq!(item, 1);
    use sea_orm::{DbBackend, FromQueryResult, Statement};
    #[derive(FromQueryResult)]
    struct Id {
        id: Uuid,
    }
    let issue = Id::find_by_statement(Statement::from_string(
        DbBackend::Postgres,
        "SELECT id FROM work_items".to_owned(),
    ))
    .one(w.db())
    .await
    .unwrap()
    .unwrap()
    .id;
    let uri = format!("/api/v1/issues/{issue}/alert");

    // A member sees the facts, and nothing a source or a rule keeps private.
    let member = w.jwt(w.tenant.member_id);
    let (status, answer) = w.call(Method::GET, &uri, Some(&member), None).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let data = &answer["data"];
    assert_eq!(data["issue_id"], json!(issue));
    assert_eq!(data["source"]["name"], "prx-waf edge-01");
    assert_eq!(data["source"]["id"], json!(w.source_id));
    assert_eq!(data["kind"], "waf.attack_detected");
    assert_eq!(data["severity"], "critical", "the highest severity seen");
    assert_eq!(data["occurrences"], 2);
    assert_eq!(data["open"], true);
    assert_eq!(data["rule"]["name"], "WAF high to triage");
    assert_eq!(data["task"]["status"], "pending");
    assert_eq!(data["task_state"], "created");
    assert!(data["first_seen_at"].is_string() && data["last_seen_at"].is_string());
    assert!(data["queued_at"].is_null());
    let text = answer.to_string();
    for private in [
        "instructions",
        "signing_secret",
        "token_hash",
        "payload",
        "notify_user_ids",
    ] {
        assert!(!text.contains(private), "the alert view leaks {private}");
    }

    // A waiting task shows when it will be suppressed.
    exec(
        w.db(),
        "UPDATE inbound_alerts SET task_state = 'waiting', task_queued_at = now() - interval '10 minutes'",
        vec![],
    )
    .await;
    let (_, answer) = w.call(Method::GET, &uri, Some(&member), None).await;
    assert_eq!(answer["data"]["task_state"], "waiting");
    assert!(answer["data"]["queued_at"].is_string() && answer["data"]["queue_expires_at"].is_string());

    // A read-only bot token may read it; a bot without any permission may not.
    let bot = w.bot_token(r#"["read"]"#).await;
    let (status, _) = w.call(Method::GET, &uri, Some(&bot), None).await;
    assert_eq!(status, StatusCode::OK);

    // An outsider of the workspace and an anonymous caller are refused.
    let outsider = Uuid::new_v4();
    exec(
        w.db(),
        "INSERT INTO users (id, email, password_hash, name, role, is_active) VALUES ($1, $2, '!', 'o', 'user', true)",
        vec![outsider.into(), format!("{outsider}@inbound.test").into()],
    )
    .await;
    let (_, answer) = w.call(Method::GET, &uri, Some(&w.jwt(outsider)), None).await;
    assert_eq!(answer["code"], 404, "{answer}");
    let (_, answer) = w.call(Method::GET, &uri, None, None).await;
    assert_eq!(answer["code"], 401);

    // A plain work item is not an alert; an unknown id is not found.
    let plain = Uuid::new_v4();
    exec(
        w.db(),
        "INSERT INTO work_items (id, project_id, title, description, state, priority, created_by, sequence_number) \
         SELECT $1, project_id, 'plain', '', 'todo', 'low', created_by, 9999 FROM work_items LIMIT 1",
        vec![plain.into()],
    )
    .await;
    let (_, answer) = w
        .call(
            Method::GET,
            &format!("/api/v1/issues/{plain}/alert"),
            Some(&member),
            None,
        )
        .await;
    assert_eq!(answer["code"], 404, "{answer}");
    let (_, answer) = w
        .call(
            Method::GET,
            &format!("/api/v1/issues/{}/alert", Uuid::new_v4()),
            Some(&member),
            None,
        )
        .await;
    assert_eq!(answer["code"], 404);

    // A delegated caller never reaches it: the route is off the allow-list.
    assert!(
        crate::oauth::delegated::DELEGATED_ROUTES
            .iter()
            .all(|entry| !entry.pattern.ends_with("/alert"))
    );
    w.scratch.drop_self().await;
}
