//! Links between work items and Flow pages: list, link and unlink.
//!
//! A link is a plain pair stored in `work_item_page_links`. Linking needs the same right as
//! editing the item (`PUT /api/v1/issues/{id}`) and at least `view` on the page; unlinking needs
//! the right on the item only, so an item's editor can drop a link to a page they can no longer
//! read. A page title is read live and shown only to a caller who may read the page.

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{
    error::ApiError,
    flow::{collab::authz::PermissionLevel, scope as flow_scope},
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
    response::ApiResponse,
    routes::issue::build_auth_extensions,
    services::{
        issue_view::{MAX_PAGE_LINKS, PageCaller, PageLink, PageRef, load_issue, page_links, viewable_pages},
        issues::{IssueRef, insert_activity},
    },
    webhook_trigger::{TriggerContext, WebhookEvent, trigger_webhooks},
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkPageRequest {
    pub object_id: Uuid,
}

/// The work item a request is about, with the caller resolved in its workspace.
struct ItemContext {
    issue: IssueRef,
    caller: PageCaller,
    actor_id: Uuid,
}

async fn item_context(
    state: &AppState,
    claims: &JwtClaims,
    bot: Option<Extension<BotAuthContext>>,
    issue_id: Uuid,
) -> Result<ItemContext, ApiError> {
    let actor_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;
    let extensions = build_auth_extensions(claims.clone(), bot);
    let loaded = load_issue(&state.db, issue_id)
        .await?
        .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;
    let access = require_workspace_access(state, &extensions, loaded.workspace_id).await?;
    Ok(ItemContext {
        issue: IssueRef {
            id: issue_id,
            project_id: loaded.issue.project_id,
            workspace_id: loaded.workspace_id,
        },
        caller: PageCaller::new(access, loaded.workspace_id),
        actor_id,
    })
}

/// The `issue.updated` delivery for a change of the item's links.
fn trigger_links_changed(state: &AppState, item: &ItemContext, added: &[Uuid], removed: &[Uuid]) {
    trigger_webhooks(
        state.clone(),
        TriggerContext {
            event: WebhookEvent::IssueUpdated,
            workspace_id: item.issue.workspace_id,
            project_id: item.issue.project_id,
            actor_id: item.actor_id,
            issue_id: Some(item.issue.id),
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: Some(json!({"page_links": {"added": added, "removed": removed}})),
            mentions: Vec::new(),
            extra_data: None,
        },
    );
}

/// GET /`api/v1/issues/:issue_id/pages` - the Flow pages linked to a work item
pub async fn list_issue_pages(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let item = item_context(&state, &claims, bot, issue_id).await?;
    Ok(ApiResponse::success(
        page_links(&state.db, &item.caller, issue_id).await?,
    ))
}

#[derive(Debug, FromQueryResult)]
struct ObjectRow {
    id: Uuid,
    workspace_id: Option<Uuid>,
    object_type: String,
}

/// POST /`api/v1/issues/:issue_id/pages` - link a Flow page to a work item
///
/// Linking a pair that is already linked changes nothing and sends no event.
pub async fn link_issue_page(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(issue_id): Path<Uuid>,
    Json(req): Json<LinkPageRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let item = item_context(&state, &claims, bot, issue_id).await?;
    let page_not_found = || ApiError::NotFound("page not found".to_string());

    let tx = state.db.begin().await?;
    // One writer per item at a time, so the limit cannot be passed by two requests at once.
    tx.query_one(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id FROM work_items WHERE id = $1 FOR UPDATE",
        vec![issue_id.into()],
    ))
    .await?
    .ok_or_else(|| ApiError::NotFound("issue not found".to_string()))?;

    // The object and the authorization epoch of its workspace stay as they are until this
    // transaction ends, so the right checked below cannot be withdrawn in between.
    let object = ObjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, object_type FROM flow_objects WHERE id = $1 FOR SHARE",
        vec![req.object_id.into()],
    ))
    .one(&tx)
    .await?
    .ok_or_else(page_not_found)?;
    if let Some(workspace) = object.workspace_id {
        crate::flow::collab::authz::lock_epoch_for_share(&tx, workspace).await?;
    }
    let page_ref = PageRef {
        id: object.id,
        workspace_id: object.workspace_id,
    };
    // A page the caller cannot read is "not found", as everywhere in Flow: nothing about it, not
    // even that it exists or what it is, comes out of this route.
    if !viewable_pages(&tx, &item.caller, &[page_ref])
        .await?
        .contains(&object.id)
    {
        return Err(page_not_found());
    }
    if object.workspace_id.is_none() && !item.caller.is_bot {
        let proof = flow_scope::authorize(&tx, object.id, item.caller.actor_id, PermissionLevel::View).await?;
        flow_scope::fence(&tx, &proof).await?;
    }

    if object.object_type != "page" {
        return Err(ApiError::BadRequest("only a Flow page can be linked".to_string()));
    }
    let placed = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                SELECT 1
                FROM flow_objects o
                WHERE o.id = $1
                  AND (o.workspace_id = $2
                       OR EXISTS (SELECT 1 FROM flow_mounts m
                                  WHERE m.source_object_id = o.id AND m.destination_workspace_id = $2))
            ",
            vec![object.id.into(), item.issue.workspace_id.into()],
        ))
        .await?;
    if placed.is_none() {
        return Err(ApiError::BadRequest(
            "page is not available in this workspace".to_string(),
        ));
    }

    let existing = tx
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 FROM work_item_page_links WHERE work_item_id = $1 AND object_id = $2",
            vec![issue_id.into(), object.id.into()],
        ))
        .await?;
    let created = existing.is_none();
    if created {
        let count = tx
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT COUNT(*) AS total FROM work_item_page_links WHERE work_item_id = $1",
                vec![issue_id.into()],
            ))
            .await?
            .and_then(|row| row.try_get::<i64>("", "total").ok())
            .ok_or(ApiError::Internal)?;
        if count >= MAX_PAGE_LINKS {
            return Err(ApiError::BadRequest(format!(
                "a work item can link at most {MAX_PAGE_LINKS} pages"
            )));
        }
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r"
                INSERT INTO work_item_page_links (work_item_id, object_id, workspace_id, created_by)
                VALUES ($1, $2, $3, $4)
            ",
            vec![
                issue_id.into(),
                object.id.into(),
                item.issue.workspace_id.into(),
                item.actor_id.into(),
            ],
        ))
        .await?;
        insert_activity(
            &tx,
            &item.issue,
            item.actor_id,
            "page_linked",
            &json!({"object_id": object.id}),
            platform::timestamp::Timestamp::now(),
        )
        .await?;
    }
    let links = page_links(&tx, &item.caller, issue_id).await?;
    tx.commit().await?;

    if created {
        trigger_links_changed(&state, &item, &[object.id], &[]);
    }
    let link: PageLink = links
        .into_iter()
        .find(|link| link.object_id == object.id)
        .ok_or(ApiError::Internal)?;
    Ok(ApiResponse::success(link))
}

/// DELETE /`api/v1/issues/:issue_id/pages/:object_id` - remove the link between a work item and a page
///
/// Needs no right on the page. Unlinking a pair that is not linked changes nothing and sends no
/// event.
pub async fn unlink_issue_page(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((issue_id, object_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, ApiError> {
    let item = item_context(&state, &claims, bot, issue_id).await?;

    let tx = state.db.begin().await?;
    let removed = tx
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM work_item_page_links WHERE work_item_id = $1 AND object_id = $2",
            vec![issue_id.into(), object_id.into()],
        ))
        .await?
        .rows_affected();
    if removed > 0 {
        insert_activity(
            &tx,
            &item.issue,
            item.actor_id,
            "page_unlinked",
            &json!({"object_id": object_id}),
            platform::timestamp::Timestamp::now(),
        )
        .await?;
    }
    tx.commit().await?;

    if removed > 0 {
        trigger_links_changed(&state, &item, &[], &[object_id]);
    }
    Ok(ApiResponse::success(json!({})))
}
