//! Rows made by a project bot connection, as the Bots, Webhooks and Members endpoints see them:
//! reported as managed, protected from edit and delete, and free of secrets in every event.
//!
//! Needs `OPENPR_TEST_DATABASE_URL`; without it every test reports that it was skipped. The
//! connection rows are inserted directly: the connection service itself arrives later.
#![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::entities::webhook::UpdateWebhookRequest;
use crate::error::ApiError;
use crate::routes::context::tenant_fixture::{Tenant, claims_for, count, exec, seed_tenant, state_for};
use crate::routes::{bot, member, webhook};
use crate::scratch_or_skip;
use crate::services::bots::{self, ManagedPolicy, NewBot};
use crate::services::webhooks::{self, NewWebhook};

struct World {
    tenant: Tenant,
    /// The managed bot, its managed webhook and the project connection that manages them.
    bot: Uuid,
    webhook: Uuid,
    /// The bot's token and the webhook's signing secret, as returned at creation.
    token: String,
    secret: String,
    /// A bot and a webhook made on the ordinary pages.
    free_bot: Uuid,
    free_webhook: Uuid,
}

async fn world(db: &DatabaseConnection, tag: &str) -> World {
    let tenant = seed_tenant(db, tag).await;
    exec(
        db,
        "UPDATE workspace_members SET role = 'owner' WHERE workspace_id = $1 AND user_id = $2",
        vec![tenant.workspace_id.into(), tenant.member_id.into()],
    )
    .await;
    let tx = db.begin().await.expect("begin");
    let make_bot = |name: &'static str| NewBot {
        workspace_id: tenant.workspace_id,
        created_by: tenant.member_id,
        name,
        permissions: vec!["read".to_string(), "write".to_string()],
        transport_surface: "mcp_stdio".to_string(),
        expires_at: None,
    };
    let managed = bots::create(&tx, make_bot("managed bot")).await.expect("managed bot");
    let free = bots::create(&tx, make_bot("free bot")).await.expect("free bot");
    let make_webhook = |bot: Option<Uuid>, name: &str| NewWebhook {
        workspace_id: tenant.workspace_id,
        actor_id: tenant.member_id,
        name: name.to_string(),
        url: "https://runner.example.net/webhook".to_string(),
        secret: None,
        events: Vec::new(),
        active: true,
        bot_user_id: bot,
    };
    let managed_hook = webhooks::create(&tx, make_webhook(Some(managed.id), "managed"), ManagedPolicy::Allow)
        .await
        .expect("managed webhook");
    let free_hook = webhooks::create(&tx, make_webhook(None, "free"), ManagedPolicy::Refuse)
        .await
        .expect("free webhook");
    exec(
        &tx,
        "INSERT INTO bot_runners (bot_id, workspace_id, webhook_id, runner_url, created_by) \
         VALUES ($1, $2, $3, 'https://runner.example.net', $4)",
        vec![
            managed.id.into(),
            tenant.workspace_id.into(),
            managed_hook.webhook.id.into(),
            tenant.member_id.into(),
        ],
    )
    .await;
    exec(
        &tx,
        "INSERT INTO project_bot_connections (project_id, bot_id, working_directory, created_by) \
         VALUES ($1, $2, '/work/a', $3)",
        vec![tenant.project_id.into(), managed.id.into(), tenant.member_id.into()],
    )
    .await;
    tx.commit().await.expect("commit");
    World {
        tenant,
        bot: managed.id,
        webhook: managed_hook.webhook.id,
        token: managed.token,
        secret: managed_hook.secret,
        free_bot: free.id,
        free_webhook: free_hook.webhook.id,
    }
}

async fn json_of(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn patch(body: Value) -> Json<UpdateWebhookRequest> {
    Json(serde_json::from_value(body).expect("an update request"))
}

fn assert_managed_refusal<T: std::fmt::Debug>(result: &Result<T, ApiError>, what: &str) {
    match result {
        Err(ApiError::Conflict(message)) => assert_eq!(message, bots::MANAGED_REFUSAL, "{what}"),
        other => panic!("{what} must be refused with the managed 409, got {other:?}"),
    }
}

#[tokio::test]
async fn managed_by_is_derived_from_the_linking_rows() {
    let scratch = scratch_or_skip!("managed_by_derived");
    let db = &scratch.db;
    let w = world(db, "mb").await;
    let state = state_for(db.clone());
    let claims = claims_for(w.tenant.member_id);
    let ws = w.tenant.workspace_id;

    // Webhooks: list and get.
    let listed = json_of(
        webhook::list_webhooks(State(state.clone()), Extension(claims.clone()), Path(ws))
            .await
            .expect("list")
            .into_response(),
    )
    .await;
    let items = listed["data"]["items"].as_array().expect("items");
    let managed = |id: Uuid| {
        items
            .iter()
            .find(|item| item["id"] == json!(id))
            .map(|item| item["managed_by"].clone())
            .expect("the webhook is listed")
    };
    let expected = json!({"kind": "project_connection", "projects": [{"id": w.tenant.project_id, "name": null}]});
    let managed_hook = managed(w.webhook);
    assert_eq!(managed_hook["kind"], expected["kind"]);
    assert_eq!(managed_hook["projects"][0]["id"], expected["projects"][0]["id"]);
    assert!(managed(w.free_webhook).is_null(), "an ordinary webhook is not managed");

    let fetched = json_of(
        webhook::get_webhook(State(state.clone()), Extension(claims.clone()), Path((ws, w.webhook)))
            .await
            .expect("get")
            .into_response(),
    )
    .await;
    assert_eq!(fetched["data"]["managed_by"]["kind"], "project_connection");
    assert!(fetched["data"].get("secret").is_none());

    // Bots: list.
    let listed = json_of(
        bot::list_bots(State(state.clone()), Extension(claims.clone()), None, Path(ws))
            .await
            .expect("bots")
            .into_response(),
    )
    .await;
    let rows = listed["data"].as_array().expect("bot rows");
    let bot_row = |id: Uuid| {
        rows.iter()
            .find(|row| row["id"] == json!(id))
            .expect("bot listed")
            .clone()
    };
    assert_eq!(
        bot_row(w.bot)["managed_by"]["projects"][0]["id"],
        json!(w.tenant.project_id)
    );
    assert!(bot_row(w.free_bot)["managed_by"].is_null());

    // Removing the last connection un-marks both rows, with no flag to reset.
    exec(
        db,
        "DELETE FROM project_bot_connections WHERE project_id = $1",
        vec![w.tenant.project_id.into()],
    )
    .await;
    exec(db, "DELETE FROM bot_runners WHERE bot_id = $1", vec![w.bot.into()]).await;
    let after = json_of(
        webhook::get_webhook(State(state), Extension(claims), Path((ws, w.webhook)))
            .await
            .expect("get")
            .into_response(),
    )
    .await;
    assert!(after["data"]["managed_by"].is_null());
    scratch.drop_self().await;
}

#[tokio::test]
async fn managed_rows_refuse_every_edit_and_stay_unchanged() {
    let scratch = scratch_or_skip!("managed_guards");
    let db = &scratch.db;
    let w = world(db, "mg").await;
    let state = state_for(db.clone());
    let claims = claims_for(w.tenant.member_id);
    let ws = w.tenant.workspace_id;
    let snapshot = |sql: &'static str, id: Uuid| {
        let db = db.clone();
        async move {
            use sea_orm::{ConnectionTrait, DbBackend, Statement};
            db.query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                sql,
                vec![id.into()],
            ))
            .await
            .expect("query")
            .expect("row")
            .try_get::<String>("", "v")
            .expect("v")
        }
    };
    let hook_before = snapshot(
        "SELECT (secret || '|' || url || '|' || name || '|' || active::text) AS v FROM webhooks WHERE id = $1",
        w.webhook,
    )
    .await;
    let token_before = snapshot(
        "SELECT (token_prefix || '|' || is_active::text) AS v FROM workspace_bots WHERE id = $1",
        w.bot,
    )
    .await;

    let revoked = bot::revoke_bot(State(state.clone()), Extension(claims.clone()), None, Path((ws, w.bot))).await;
    assert_managed_refusal(&revoked.map(|_| ()), "revoking a managed bot");
    let patched = webhook::update_webhook(
        State(state.clone()),
        Extension(claims.clone()),
        Path((ws, w.webhook)),
        patch(json!({"name": "renamed", "active": false})),
    )
    .await;
    assert_managed_refusal(&patched.map(|_| ()), "patching a managed webhook");
    let rotated =
        webhook::rotate_webhook_secret(State(state.clone()), Extension(claims.clone()), Path((ws, w.webhook))).await;
    assert_managed_refusal(&rotated.map(|_| ()), "rotating the secret of a managed webhook");
    let deleted = webhook::delete_webhook(State(state.clone()), Extension(claims.clone()), Path((ws, w.webhook))).await;
    assert_managed_refusal(&deleted.map(|_| ()), "deleting a managed webhook");
    let removed = member::remove_member(State(state.clone()), Extension(claims.clone()), Path((ws, w.bot))).await;
    assert_managed_refusal(&removed.map(|_| ()), "removing the seat of a managed bot");

    assert_eq!(
        snapshot(
            "SELECT (secret || '|' || url || '|' || name || '|' || active::text) AS v FROM webhooks WHERE id = $1",
            w.webhook
        )
        .await,
        hook_before,
        "the managed webhook did not change"
    );
    assert_eq!(
        snapshot(
            "SELECT (token_prefix || '|' || is_active::text) AS v FROM workspace_bots WHERE id = $1",
            w.bot
        )
        .await,
        token_before,
        "the managed token did not change"
    );
    assert_eq!(
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
            vec![ws.into(), w.bot.into()],
        )
        .await,
        1,
        "the managed bot keeps its seat"
    );

    // Ordinary rows still work through the same routes.
    bot::revoke_bot(
        State(state.clone()),
        Extension(claims.clone()),
        None,
        Path((ws, w.free_bot)),
    )
    .await
    .expect("an ordinary bot can be revoked");
    webhook::update_webhook(
        State(state.clone()),
        Extension(claims.clone()),
        Path((ws, w.free_webhook)),
        patch(json!({"name": "renamed"})),
    )
    .await
    .expect("an ordinary webhook can be edited");
    webhook::rotate_webhook_secret(
        State(state.clone()),
        Extension(claims.clone()),
        Path((ws, w.free_webhook)),
    )
    .await
    .expect("an ordinary secret can be rotated");
    webhook::delete_webhook(State(state), Extension(claims), Path((ws, w.free_webhook)))
        .await
        .expect("an ordinary webhook can be deleted");
    scratch.drop_self().await;
}

#[tokio::test]
async fn a_webhook_cannot_name_a_managed_bot() {
    let scratch = scratch_or_skip!("managed_bot_user");
    let db = &scratch.db;
    let w = world(db, "bu").await;
    let state = state_for(db.clone());
    let claims = claims_for(w.tenant.member_id);
    let ws = w.tenant.workspace_id;

    let tx = db.begin().await.expect("begin");
    let created = webhooks::create(
        &tx,
        NewWebhook {
            workspace_id: ws,
            actor_id: w.tenant.member_id,
            name: "naming a managed bot".to_string(),
            url: "https://other.example.net/webhook".to_string(),
            secret: None,
            events: Vec::new(),
            active: false,
            bot_user_id: Some(w.bot),
        },
        ManagedPolicy::Refuse,
    )
    .await;
    match created {
        Err(ApiError::BadRequest(message)) => assert_eq!(message, bots::MANAGED_BOT_USER_REFUSAL),
        other => panic!("create must refuse a managed bot, got {other:?}"),
    }
    drop(tx);

    let updated = webhook::update_webhook(
        State(state),
        Extension(claims),
        Path((ws, w.free_webhook)),
        patch(json!({"bot_user_id": w.bot})),
    )
    .await;
    match updated {
        Err(ApiError::BadRequest(message)) => assert_eq!(message, bots::MANAGED_BOT_USER_REFUSAL),
        other => panic!("update must refuse a managed bot, got {:?}", other.map(|_| ())),
    }
    // An unmanaged bot is still accepted.
    let ok = webhooks::create(
        db,
        NewWebhook {
            workspace_id: ws,
            actor_id: w.tenant.member_id,
            name: "naming a free bot".to_string(),
            url: "https://other.example.net/webhook".to_string(),
            secret: None,
            events: Vec::new(),
            active: false,
            bot_user_id: Some(w.free_bot),
        },
        ManagedPolicy::Refuse,
    )
    .await;
    assert!(ok.is_ok(), "an unmanaged bot may be named");
    scratch.drop_self().await;
}

/// Creating, updating and deleting a webhook each write exactly one business event of their own,
/// for the Webhooks routes (`Refuse`) and for the connection service (`Allow`); only the
/// connection's events say so.
#[tokio::test]
async fn every_webhook_write_emits_its_business_event_once_for_both_callers() {
    let scratch = scratch_or_skip!("webhook_events");
    let db = &scratch.db;
    let w = world(db, "we").await;
    let ws = w.tenant.workspace_id;
    let actor = w.tenant.member_id;

    for policy in [ManagedPolicy::Refuse, ManagedPolicy::Allow] {
        let tx = db.begin().await.expect("begin");
        let created = webhooks::create(
            &tx,
            NewWebhook {
                workspace_id: ws,
                actor_id: actor,
                name: format!("{policy:?}"),
                url: "https://events.example.net/webhook".to_string(),
                secret: None,
                events: Vec::new(),
                active: false,
                bot_user_id: None,
            },
            policy,
        )
        .await
        .expect("create");
        let id = created.webhook.id;
        webhooks::update(
            &tx,
            ws,
            id,
            actor,
            serde_json::from_value(json!({"url": "https://events-moved.example.net/webhook"})).expect("request"),
            policy,
        )
        .await
        .expect("update");
        webhooks::delete(&tx, ws, id, actor, policy).await.expect("delete");
        tx.commit().await.expect("commit");

        for event in ["webhook.created", "webhook.updated", "webhook.deleted"] {
            let once = count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM business_events WHERE aggregate_id = $1 AND event_type = $2",
                vec![id.to_string().into(), event.into()],
            )
            .await;
            assert_eq!(once, 1, "{policy:?}: {event} is written once");
            let marked = count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM business_events \
                 WHERE aggregate_id = $1 AND event_type = $2 AND metadata->>'managed_by_connection' = 'true'",
                vec![id.to_string().into(), event.into()],
            )
            .await;
            assert_eq!(
                marked,
                i64::from(policy == ManagedPolicy::Allow),
                "{policy:?}: {event} marker"
            );
        }
    }
    scratch.drop_self().await;
}

/// The connection's own writes pass the guards, are marked in their events, and no event of any
/// bot or webhook write carries the token or a signing secret.
#[tokio::test]
async fn the_connection_policy_passes_and_no_event_carries_a_secret() {
    let scratch = scratch_or_skip!("managed_events");
    let db = &scratch.db;
    let w = world(db, "ev").await;
    let ws = w.tenant.workspace_id;

    let tx = db.begin().await.expect("begin");
    let rotated = webhooks::rotate_secret(&tx, ws, w.webhook, w.tenant.member_id, ManagedPolicy::Allow)
        .await
        .expect("the connection may rotate its webhook");
    let renamed = webhooks::update(
        &tx,
        ws,
        w.webhook,
        w.tenant.member_id,
        serde_json::from_value(json!({"url": "https://moved.example.net/webhook"})).expect("request"),
        ManagedPolicy::Allow,
    )
    .await
    .expect("the connection may move its webhook");
    assert_eq!(renamed.url, "https://moved.example.net/webhook");
    let token = bots::rotate_token(&tx, w.bot).await.expect("rotate the token");
    assert_ne!(token.token_prefix.len(), 0);
    tx.commit().await.expect("commit");

    let marked = count(
        db,
        "SELECT COUNT(*)::bigint AS count FROM business_events \
         WHERE aggregate_id = $1 AND metadata->>'managed_by_connection' = 'true'",
        vec![w.webhook.to_string().into()],
    )
    .await;
    assert_eq!(marked, 3, "created, rotated and updated are marked as the connection's");

    for secret in [
        w.secret.as_str(),
        rotated.secret.as_str(),
        w.token.as_str(),
        token.token.as_str(),
    ] {
        assert_eq!(
            count(
                db,
                "SELECT COUNT(*)::bigint AS count FROM business_events \
                 WHERE workspace_id = $1 AND (payload::text LIKE $2 OR metadata::text LIKE $2)",
                vec![ws.into(), format!("%{secret}%").into()],
            )
            .await,
            0,
            "an event carries a secret"
        );
    }
    scratch.drop_self().await;
}

/// The Members API cannot promote a managed runner bot, and the database refuses an admin seat for
/// one by any path, so its token never counts as a workspace admin.
#[tokio::test]
async fn a_managed_bot_cannot_be_given_an_admin_seat() {
    let scratch = scratch_or_skip!("managed_role");
    let db = &scratch.db;
    let w = world(db, "mr").await;
    let state = state_for(db.clone());
    let ws = w.tenant.workspace_id;
    let seat = || {
        count(
            db,
            "SELECT COUNT(*)::bigint AS count FROM workspace_members WHERE workspace_id = $1 AND user_id = $2 AND role = 'member'",
            vec![ws.into(), w.bot.into()],
        )
    };
    let promoted = member::update_member_role(
        State(state.clone()),
        Extension(claims_for(w.tenant.member_id)),
        Path((ws, w.bot)),
        Json(member::UpdateMemberRoleRequest {
            role: "admin".to_string(),
        }),
    )
    .await;
    assert_managed_refusal(&promoted.map(|_| ()), "changing the seat role of a managed bot");
    assert_eq!(seat().await, 1, "the seat stays member");

    // Any other path is refused by the database.
    let direct = sea_orm::ConnectionTrait::execute(
        db,
        sea_orm::Statement::from_sql_and_values(
            sea_orm::DbBackend::Postgres,
            "UPDATE workspace_members SET role = 'admin' WHERE workspace_id = $1 AND user_id = $2",
            vec![ws.into(), w.bot.into()],
        ),
    )
    .await;
    assert!(direct.is_err(), "a managed bot's seat cannot be admin or owner");
    assert_eq!(seat().await, 1);

    // An ordinary bot seat is not editable either: its role is its token's.
    member::update_member_role(
        State(state),
        Extension(claims_for(w.tenant.member_id)),
        Path((ws, w.free_bot)),
        Json(member::UpdateMemberRoleRequest {
            role: "admin".to_string(),
        }),
    )
    .await
    .err()
    .expect("a bot's role follows its token permissions, so its seat is not editable either");
    scratch.drop_self().await;
}
