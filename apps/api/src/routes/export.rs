//! Project export: the document `sylvode.project_export` version 1, which is also what
//! [`super::import`] reads, as JSON or as a CSV of the work items.

use axum::{
    Extension,
    extract::{Path, Query, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use uuid::Uuid;

use crate::{
    error::ApiError,
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
    services::issue_view::identifier,
};

/// The `format` member of an export document.
pub const EXPORT_FORMAT: &str = "sylvode.project_export";
/// The `version` member of an export document.
pub const EXPORT_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
pub struct ExportQuery {
    pub format: Option<String>, // 'json' or 'csv', default 'json'
}

#[derive(Debug, Serialize)]
struct ProjectInfo {
    key: String,
    name: String,
    description: String,
}

#[derive(Debug, Serialize)]
struct IssueExport {
    identifier: String,
    title: String,
    description: String,
    state: String,
    priority: String,
    due_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
struct CommentExport {
    issue_identifier: String,
    author_name: String,
    body: String,
    created_at: String,
}

#[derive(Debug, Serialize)]
struct ProjectExport {
    format: &'static str,
    version: u32,
    exported_at: String,
    project: ProjectInfo,
    issues: Vec<IssueExport>,
    comments: Vec<CommentExport>,
}

#[derive(Debug, FromQueryResult)]
struct ProjectRow {
    workspace_id: Uuid,
    key: String,
    name: String,
    description: String,
}

#[derive(Debug, FromQueryResult)]
struct IssueRow {
    sequence_number: i32,
    title: String,
    description: String,
    state: String,
    priority: String,
    due_at: Option<Timestamp>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

#[derive(Debug, FromQueryResult)]
struct CommentRow {
    sequence_number: i32,
    author_name: Option<String>,
    body: String,
    created_at: Timestamp,
}

/// One CSV field: always quoted, a quote inside doubled.
fn csv_field(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn csv_of(export: &ProjectExport) -> String {
    let mut csv = String::from("Identifier,Title,State,Priority,Due,Description,Created At,Updated At\n");
    for issue in &export.issues {
        let fields = [
            issue.identifier.as_str(),
            issue.title.as_str(),
            issue.state.as_str(),
            issue.priority.as_str(),
            issue.due_at.as_deref().unwrap_or(""),
            issue.description.as_str(),
            issue.created_at.as_str(),
            issue.updated_at.as_str(),
        ];
        let line = fields
            .iter()
            .map(|field| csv_field(field))
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(csv, "{line}");
    }
    csv
}

/// `GET /api/v1/projects/{project_id}/export`
///
/// Answers the file itself (`Content-Disposition: attachment`); errors stay JSON envelopes.
pub async fn export_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    let format = query.format.as_deref().unwrap_or("json");
    if !matches!(format, "json" | "csv") {
        return Err(ApiError::BadRequest("Invalid format. Use 'json' or 'csv'".to_string()));
    }

    let project = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id, key, name, description FROM projects WHERE id = $1",
        vec![project_id.into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound("Project not found".to_string()))?;

    require_workspace_access(&state, &extensions, project.workspace_id).await?;

    let issues = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT sequence_number, title, description, state, priority, due_at, created_at, updated_at
            FROM work_items
            WHERE project_id = $1
            ORDER BY sequence_number
        ",
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    let comments = CommentRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.sequence_number, u.name AS author_name, c.body, c.created_at
            FROM comments c
            INNER JOIN work_items wi ON c.work_item_id = wi.id
            LEFT JOIN users u ON u.id = c.author_id
            WHERE wi.project_id = $1
            ORDER BY c.created_at, c.id
        ",
        vec![project_id.into()],
    ))
    .all(&state.db)
    .await?;

    let key = project.key.clone();
    let export = ProjectExport {
        format: EXPORT_FORMAT,
        version: EXPORT_VERSION,
        exported_at: Timestamp::now().to_rfc3339(),
        project: ProjectInfo {
            key: project.key,
            name: project.name,
            description: project.description,
        },
        issues: issues
            .into_iter()
            .map(|issue| IssueExport {
                identifier: identifier(&key, issue.sequence_number),
                title: issue.title,
                description: issue.description,
                state: issue.state,
                priority: issue.priority,
                due_at: issue.due_at.map(Timestamp::to_rfc3339),
                created_at: issue.created_at.to_rfc3339(),
                updated_at: issue.updated_at.to_rfc3339(),
            })
            .collect(),
        comments: comments
            .into_iter()
            .map(|comment| CommentExport {
                issue_identifier: identifier(&key, comment.sequence_number),
                author_name: comment.author_name.unwrap_or_default(),
                body: comment.body,
                created_at: comment.created_at.to_rfc3339(),
            })
            .collect(),
    };

    let (content, extension, content_type) = if format == "csv" {
        (csv_of(&export), "csv", "text/csv; charset=utf-8")
    } else {
        (
            serde_json::to_string_pretty(&export).map_err(|_| ApiError::Internal)?,
            "json",
            "application/json; charset=utf-8",
        )
    };
    // The project key is validated on creation; a value that cannot be a header is refused rather
    // than sent with a broken filename.
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"project_{key}_export.{extension}\""))
        .map_err(|_| ApiError::Internal)?;
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(content_type)),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        content,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn a_csv_field_is_quoted_with_quotes_doubled() {
        assert_eq!(csv_field("plain"), "\"plain\"");
        assert_eq!(csv_field("say \"hi\", then leave"), "\"say \"\"hi\"\", then leave\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_field(""), "\"\"");
    }
}
