//! Authorization of every connection route, the effect of a rotation on the token and the secret,
//! and the report route's callers: the cases the security review found untested.
#![cfg(test)]
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::too_many_lines)]

use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, Request, header},
    response::Response,
    routing::post,
};
use serde_json::json;
use tower::ServiceExt;
use uuid::Uuid;

use super::bot_connection_tests::{block_secret, block_token, expect_connect, new_bot, rotate, text_of, world};
use crate::error::ApiError;
use crate::oauth::delegated::{DelegatedAuthContext, with_context};
use crate::oauth::scopes::{ProjectScope, ScopeSet, TokenResource};
use crate::routes::context::tenant_fixture::{
    bot_context_for, claims_for, exec, seed_bot_user, seed_member, seed_tenant,
};
use crate::routes::project_bot_connection as route;
use crate::scratch_or_skip;

fn delegated(owner: Uuid, workspace: Uuid) -> DelegatedAuthContext {
    DelegatedAuthContext {
        grant_id: Uuid::new_v4(),
        token_id: Uuid::new_v4(),
        client_id: Arc::from("ramflux"),
        user_id: owner,
        workspace_id: workspace,
        scopes: ScopeSet::all(),
        projects: ProjectScope::All,
        resource: TokenResource::Api,
        surface: crate::flow::event_origin::EventSurface::Rest,
        tool_name: None,
        request_id: Uuid::new_v4(),
    }
}

type Pending<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, ApiError>> + 'a>>;

fn refused(result: Result<Response, ApiError>, who: &str, what: &str) {
    match result {
        Err(ApiError::Forbidden(_)) => {}
        other => panic!("{who} on {what}: expected a 403, got {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn every_write_route_refuses_members_bots_applications_and_outsiders() {
    let scratch = scratch_or_skip!("missing_authz");
    let db = &scratch.db;
    let w = world(db, "ma").await;
    let project = w.tenant.project_id;
    let first = expect_connect(&w, project, new_bot("Guarded bot")).await;
    let version = first.connection.version;
    let member = seed_member(db, &w.tenant, "member").await;
    let outsider = seed_tenant(db, "ma2").await.member_id;
    let seated_admin_bot = seed_bot_user(db, &w.tenant).await;
    exec(
        db,
        "INSERT INTO workspace_members (workspace_id, user_id, role) VALUES ($1, $2, 'admin') ON CONFLICT (workspace_id, user_id) DO UPDATE SET role = 'admin'",
        vec![w.tenant.workspace_id.into(), seated_admin_bot.into()],
    )
    .await;
    let admin_bot = bot_context_for(seated_admin_bot, w.tenant.workspace_id, &["read", "write", "admin"]);

    // Each caller: (name, user, bot context, delegated?).
    let callers: Vec<(&str, Uuid, Option<crate::middleware::bot_auth::BotAuthContext>, bool)> = vec![
        ("member", member, None, false),
        ("outsider", outsider, None, false),
        ("admin-seated bot", seated_admin_bot, Some(admin_bot), false),
        ("connected application", w.owner, None, true),
    ];
    for (who, user, bot, app) in callers {
        let state = || State(w.state.clone());
        let claims = || Extension(claims_for(user));
        let bot = || bot.clone().map(Extension);
        let calls: Vec<(&str, Pending<'_>)> = vec![
            (
                "PATCH",
                Box::pin(route::update_bot_connection(
                    state(),
                    claims(),
                    bot(),
                    Path((project, first.connection.bot.id)),
                    Json(serde_json::from_value(json!({"expected_version": version, "cli": "codex"})).expect("body")),
                )),
            ),
            (
                "disable",
                Box::pin(route::disable_bot_connection(
                    state(),
                    claims(),
                    bot(),
                    Path((project, first.connection.bot.id)),
                    Json(serde_json::from_value(json!({"expected_version": version})).expect("body")),
                )),
            ),
            (
                "enable",
                Box::pin(route::enable_bot_connection(
                    state(),
                    claims(),
                    bot(),
                    Path((project, first.connection.bot.id)),
                    Json(serde_json::from_value(json!({"expected_version": version})).expect("body")),
                )),
            ),
            (
                "rotate",
                Box::pin(route::rotate_bot_connection(
                    state(),
                    claims(),
                    bot(),
                    Path((project, first.connection.bot.id)),
                    HeaderMap::new(),
                    Json(serde_json::from_value(json!({"expected_version": version})).expect("body")),
                )),
            ),
            (
                "DELETE",
                Box::pin(route::remove_bot_connection(
                    state(),
                    claims(),
                    bot(),
                    Path((project, first.connection.bot.id)),
                    Query(route::RemoveQuery {
                        expected_version: version,
                    }),
                )),
            ),
        ];
        for (what, call) in calls {
            let result = if app {
                with_context(delegated(w.owner, w.tenant.workspace_id), call).await
            } else {
                call.await
            };
            refused(result, who, what);
        }
    }
    // Nothing changed: still connected, enabled, same version.
    let (current, enabled) = (
        text_of(
            db,
            "SELECT version::text AS v FROM project_bot_connections WHERE project_id = $1",
            vec![project.into()],
        )
        .await,
        text_of(
            db,
            "SELECT enabled::text AS v FROM project_bot_connections WHERE project_id = $1",
            vec![project.into()],
        )
        .await,
    );
    assert_eq!((current, enabled), (version.to_string(), "true".to_string()));
    scratch.drop_self().await;
}

/// A rotation started from one project replaces the token and the secret for the bot everywhere: the
/// old token no longer authenticates on the task callback routes, the new one does, and the
/// delivery signature is made with the new secret and verifies only against it.
#[tokio::test]
async fn rotating_from_one_project_replaces_the_token_and_the_secret_for_all() {
    let scratch = scratch_or_skip!("missing_rotate");
    let db = &scratch.db;
    let w = world(db, "mr").await;
    let p1 = w.tenant.project_id;
    let p2 = super::bot_connection_tests::second_project(db, &w, "MR2").await;
    let first = expect_connect(&w, p1, new_bot("Rotating bot")).await;
    let old_token = block_token(first.config_block.as_ref().expect("block"));
    let old_secret = block_secret(first.config_block.as_ref().expect("block"));
    let bot = first.connection.bot.id;
    let second =
        super::bot_connection_tests::expect_connect(&w, p2, super::bot_connection_tests::existing_bot(bot)).await;

    // The rotation is started from P1 (after P2 joined, P1's version moved).
    let version = second.connection.version;
    let p1_version = text_of(
        db,
        "SELECT version::text AS v FROM project_bot_connections WHERE project_id = $1",
        vec![p1.into()],
    )
    .await
    .parse::<i32>()
    .expect("version");
    assert!(p1_version >= 1 && version >= 1);
    let rotated = match rotate(&w, p1, p1_version).await.expect("rotate") {
        crate::services::bot_connections::Outcome::Done(rotated) => rotated,
        crate::services::bot_connections::Outcome::Conflict { .. } => panic!("a conflict"),
    };
    let new_token = block_token(&rotated.config_block);
    let new_secret = block_secret(&rotated.config_block);
    assert_ne!(old_token, new_token);
    assert_ne!(old_secret, new_secret);

    // The task protocol route a P2 task would use: only the new token authenticates.
    let state = crate::routes::context::tenant_fixture::state_for(db.clone());
    let app = Router::new()
        .route(
            "/api/v1/ai/tasks/{task_id}/progress",
            post(|| async { "ok" }).route_layer(axum::middleware::from_fn_with_state(
                state.clone(),
                crate::middleware::bot_auth::bot_or_user_auth_middleware,
            )),
        )
        .with_state(state);
    let call = |token: String| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::post(format!("/api/v1/ai/tasks/{}/progress", Uuid::new_v4()))
                        .header(header::AUTHORIZATION, format!("Bearer {token}"))
                        .header("x-sylvode-mcp-surface", "mcp_stdio")
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("answer");
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("body");
            String::from_utf8_lossy(&bytes).into_owned()
        }
    };
    assert!(
        call(old_token).await.contains("\"code\":401"),
        "the old token is refused"
    );
    assert_eq!(call(new_token).await, "ok");

    // The worker signs with `webhooks.secret`, which is now the new secret.
    let stored = text_of(
        db,
        "SELECT secret AS v FROM webhooks WHERE bot_user_id = $1",
        vec![bot.into()],
    )
    .await;
    assert_eq!(stored, new_secret);
    let body = r#"{"task_id":"t"}"#;
    let signature = crate::webhook_trigger::sign_payload(&stored, body).expect("sign");
    assert_ne!(
        signature,
        crate::webhook_trigger::sign_payload(&old_secret, body).expect("sign"),
        "the old secret no longer produces the delivery signature"
    );
    scratch.drop_self().await;
}

/// The report route: a person, a connected application and a bot no connection made are refused.
#[tokio::test]
async fn the_report_route_refuses_people_applications_and_unconnected_bots() {
    let scratch = scratch_or_skip!("missing_report");
    let db = &scratch.db;
    let w = world(db, "rp").await;
    let report = |bot: Option<crate::middleware::bot_auth::BotAuthContext>| {
        let state = w.state.clone();
        async move {
            crate::routes::ai_task_protocol::put_runner_report(
                State(state),
                bot.map(Extension),
                axum::body::Bytes::from_static(br#"{"components":[]}"#),
            )
            .await
        }
    };
    // A person (no bot context) and a connected application acting for the owner.
    assert!(matches!(report(None).await, Err(ApiError::Forbidden(_))));
    assert!(matches!(
        with_context(delegated(w.owner, w.tenant.workspace_id), report(None)).await,
        Err(ApiError::Forbidden(_))
    ));
    // A bot of the Bots page has no runner row.
    let plain = seed_bot_user(db, &w.tenant).await;
    assert!(matches!(
        report(Some(bot_context_for(plain, w.tenant.workspace_id, &["read", "write"]))).await,
        Err(ApiError::NotFound(_))
    ));
    // A `<script>` version is not stored.
    let first = expect_connect(&w, w.tenant.project_id, new_bot("Report bot")).await;
    let runner = bot_context_for(first.connection.bot.id, w.tenant.workspace_id, &["read", "write"]);
    crate::routes::ai_task_protocol::put_runner_report(
        State(w.state.clone()),
        Some(Extension(runner)),
        axum::body::Bytes::from_static(br#"{"generated_by":{"version":"<script>"},"components":[]}"#),
    )
    .await
    .expect("accepted without the version");
    let stored = text_of(
        db,
        "SELECT report::text AS v FROM bot_runners WHERE bot_id = $1",
        vec![first.connection.bot.id.into()],
    )
    .await;
    assert!(!stored.contains("script"), "{stored}");
    scratch.drop_self().await;
}
