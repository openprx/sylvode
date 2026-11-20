use axum::{
    Extension,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    error::ApiError,
    response::{ApiResponse, PaginatedData},
};

#[derive(Debug, Serialize, FromQueryResult)]
pub struct NotificationResponse {
    pub id: Uuid,
    pub user_id: Uuid,
    #[serde(rename = "type")]
    #[sea_orm(column_name = "type")]
    pub notification_type: String,
    pub title: String,
    pub content: String,
    pub link: Option<String>,
    pub related_issue_id: Option<Uuid>,
    pub related_comment_id: Option<Uuid>,
    pub related_project_id: Option<Uuid>,
    pub is_read: bool,
    pub created_at: platform::timestamp::Timestamp,
    pub read_at: Option<platform::timestamp::Timestamp>,
    pub issue_title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListNotificationsQuery {
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub limit: Option<u32>,
    pub unread_only: Option<bool>,
}

#[derive(Serialize)]
pub struct NotificationListData {
    #[serde(flatten)]
    pub page_data: PaginatedData<NotificationResponse>,
    pub unread_count: i64,
}

/// The extra predicate that limits notification statements for a connected application, and its
/// values, or `None` for every other caller.
///
/// A delegated request sees the user's notifications of the grant's workspace; a grant limited to
/// some projects further requires the notification to belong to one of them, directly or through
/// its work item. A notification whose project cannot be found is visible to a grant for all
/// projects only. `alias` is the notification table's alias with its dot (or empty) and
/// `first_param` the number of the first placeholder the fragment may use.
fn delegated_scope(alias: &str, first_param: usize) -> Option<(String, Vec<sea_orm::Value>)> {
    let context = crate::oauth::delegated::current()?;
    let mut sql = format!(" AND {alias}workspace_id = ${first_param}");
    let mut values: Vec<sea_orm::Value> = vec![context.workspace_id.into()];
    if let crate::oauth::scopes::ProjectScope::Selected(ids) = &context.projects {
        let ids_param = first_param + 1;
        sql = format!(
            "{sql} AND ({alias}related_project_id = ANY(${ids_param}) \
             OR {alias}related_issue_id IN (SELECT w.id FROM work_items w WHERE w.project_id = ANY(${ids_param})))"
        );
        values.push(ids.to_vec().into());
    }
    Some((sql, values))
}

/// The condition that keeps a joined work item inside the grant, so a notification that names an
/// item of another project (or workspace) shows no title for it to a connected application.
/// Uses the placeholders that [`delegated_scope`] numbers from `first_param`.
fn delegated_item_scope(first_param: usize) -> String {
    let Some(context) = crate::oauth::delegated::current() else {
        return String::new();
    };
    let projects = match &context.projects {
        crate::oauth::scopes::ProjectScope::All => String::new(),
        crate::oauth::scopes::ProjectScope::Selected(_) => format!(" AND p.id = ANY(${})", first_param + 1),
    };
    format!(" AND i.project_id IN (SELECT p.id FROM projects p WHERE p.workspace_id = ${first_param}{projects})")
}

/// GET /api/v1/notifications
pub async fn list_notifications(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Query(query): Query<ListNotificationsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let page = query.page.unwrap_or(1).max(1);
    let per_page = query.limit.or(query.per_page).unwrap_or(20).min(100);
    let offset = (page - 1) * per_page;
    let unread_only = query.unread_only.unwrap_or(false);

    let (count_scope, count_scope_values) = delegated_scope("", 2).unwrap_or_default();
    let mut count_values: Vec<sea_orm::Value> = vec![user_id.into()];
    count_values.extend(count_scope_values);

    // Get total count
    let count_query = if unread_only {
        format!("SELECT COUNT(*) as count FROM notifications WHERE user_id = $1 AND is_read = false{count_scope}")
    } else {
        format!("SELECT COUNT(*) as count FROM notifications WHERE user_id = $1{count_scope}")
    };

    let count_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            count_query,
            count_values.clone(),
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let total: i64 = count_result.try_get("", "count")?;

    // Get unread count
    let unread_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT COUNT(*) as count FROM notifications WHERE user_id = $1 AND is_read = false{count_scope}"),
            count_values,
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let unread_count: i64 = unread_result.try_get("", "count")?;

    // Get notifications
    let (list_scope, list_scope_values) = delegated_scope("n.", 4).unwrap_or_default();
    let item_scope = delegated_item_scope(4);
    let unread_clause = if unread_only { " AND n.is_read = false" } else { "" };
    let list_query = format!(
        "SELECT n.id, n.user_id, n.type as notification_type, n.title, n.content, n.link,
                n.related_issue_id, n.related_comment_id, n.related_project_id,
                n.is_read, n.created_at, n.read_at,
                i.title as issue_title
         FROM notifications n
         LEFT JOIN work_items i ON n.related_issue_id = i.id{item_scope}
         WHERE n.user_id = $1{unread_clause}{list_scope}
         ORDER BY n.created_at DESC
         LIMIT $2 OFFSET $3"
    );
    let mut list_values: Vec<sea_orm::Value> =
        vec![user_id.into(), i64::from(per_page).into(), i64::from(offset).into()];
    list_values.extend(list_scope_values);

    let notifications_result = state
        .db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            list_query,
            list_values,
        ))
        .await?;

    let notifications: Vec<NotificationResponse> = notifications_result
        .iter()
        .map(|r| NotificationResponse::from_query_result(r, ""))
        .collect::<Result<Vec<_>, _>>()?;

    let page_count = if per_page == 0 {
        if total == 0 { 0 } else { i64::from(u32::MAX) }
    } else {
        total / i64::from(per_page) + i64::from(total % i64::from(per_page) != 0)
    };
    let total_pages = u32::try_from(page_count).map_err(|_| ApiError::Internal)?;

    Ok(ApiResponse::success(NotificationListData {
        page_data: PaginatedData {
            items: notifications,
            total,
            page: i64::from(page),
            per_page: i64::from(per_page),
            total_pages: i64::from(total_pages),
        },
        unread_count,
    }))
}

#[derive(Debug, Serialize)]
pub struct UnreadCountData {
    pub count: i64,
}

/// GET /api/v1/notifications/unread-count
pub async fn get_unread_count(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let (scope_sql, scope_values) = delegated_scope("", 2).unwrap_or_default();
    let mut values: Vec<sea_orm::Value> = vec![user_id.into()];
    values.extend(scope_values);

    let unread_result = state
        .db
        .query_one(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!("SELECT COUNT(*) as count FROM notifications WHERE user_id = $1 AND is_read = false{scope_sql}"),
            values,
        ))
        .await?
        .ok_or_else(|| ApiError::Internal)?;

    let unread_count: i64 = unread_result.try_get("", "count")?;

    Ok(ApiResponse::success(UnreadCountData { count: unread_count }))
}

/// PATCH /api/v1/notifications/:id/read
pub async fn mark_notification_read(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(notification_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let now = platform::timestamp::Timestamp::now();

    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE notifications SET is_read = true, read_at = $1 WHERE id = $2 AND user_id = $3 AND is_read = false",
            vec![now.into(), notification_id.into(), user_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Notification not found or already read".to_string()));
    }

    Ok(ApiResponse::ok())
}

/// PATCH /api/v1/notifications/read-all
pub async fn mark_all_read(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let now = platform::timestamp::Timestamp::now();

    // Marking everything read through a connected application reaches the grant's notifications only.
    let (scope_sql, scope_values) = delegated_scope("", 3).unwrap_or_default();
    let mut values: Vec<sea_orm::Value> = vec![now.into(), user_id.into()];
    values.extend(scope_values);

    state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                "UPDATE notifications SET is_read = true, read_at = $1 WHERE user_id = $2 AND is_read = false{scope_sql}"
            ),
            values,
        ))
        .await?;

    Ok(ApiResponse::ok())
}

/// DELETE /api/v1/notifications/:id
pub async fn delete_notification(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path(notification_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| ApiError::Unauthorized("invalid user id".to_string()))?;

    let result = state
        .db
        .execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM notifications WHERE id = $1 AND user_id = $2",
            vec![notification_id.into(), user_id.into()],
        ))
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("Notification not found".to_string()));
    }

    Ok(ApiResponse::ok())
}
