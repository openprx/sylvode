#![allow(clippy::items_after_statements)]
//! Project import: adds the work items and comments of an export document (see
//! [`super::export`]) to an existing project.

use axum::{
    Extension, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use platform::{app::AppState, auth::JwtClaims, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

use super::export::{EXPORT_FORMAT, EXPORT_VERSION};
use crate::{
    error::{ApiError, ApiErrorKind},
    middleware::bot_auth::{BotAuthContext, require_workspace_access},
    response::ApiResponse,
    services::{
        issue_view::identifier,
        work_item_service::{NewWorkItem, insert_work_item},
        workflow_service::resolve_effective_workflow_for_project,
    },
};

#[derive(Debug, Deserialize)]
pub struct ImportIssue {
    pub identifier: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub state: String,
    pub priority: Option<String>,
    pub due_at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImportComment {
    pub issue_identifier: String,
    pub body: String,
    pub created_at: String,
}

/// The export document. Members this import does not use (`project`, `exported_at`, the author
/// names, the work items' timestamps) are accepted and ignored.
#[derive(Debug, Deserialize)]
pub struct ImportDocument {
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub issues: Vec<ImportIssue>,
    #[serde(default)]
    pub comments: Vec<ImportComment>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportProjectRequest {
    pub project_key: String,
    pub document: ImportDocument,
}

#[derive(Debug, Serialize)]
pub struct ImportResult {
    pub project_id: Uuid,
    pub project_key: String,
    pub issues_created: usize,
    pub comments_created: usize,
    /// The identifier each imported work item had in the document, and the one it has here.
    pub identifiers: BTreeMap<String, String>,
}

fn valid_priority(priority: &str) -> bool {
    matches!(priority, "low" | "medium" | "high" | "urgent")
}

/// Everything wrong with the document, before anything is written.
fn problems(document: &ImportDocument, states: &HashSet<String>) -> Vec<String> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for (index, issue) in document.issues.iter().enumerate() {
        let label = format!("issues[{index}] ({})", issue.identifier);
        if issue.identifier.trim().is_empty() {
            found.push(format!("issues[{index}]: identifier is required"));
        } else if !seen.insert(issue.identifier.as_str()) {
            found.push(format!("{label}: the identifier appears more than once"));
        }
        if issue.title.trim().is_empty() {
            found.push(format!("{label}: title is required"));
        }
        if !states.contains(&issue.state) {
            found.push(format!(
                "{label}: state `{}` is not a state of the project's workflow",
                issue.state
            ));
        }
        if let Some(priority) = &issue.priority
            && !valid_priority(priority)
        {
            found.push(format!("{label}: priority must be one of: low, medium, high, urgent"));
        }
        if let Some(due) = &issue.due_at
            && Timestamp::parse_rfc3339(due).is_err()
        {
            found.push(format!("{label}: due_at is not an RFC 3339 timestamp"));
        }
    }
    for (index, comment) in document.comments.iter().enumerate() {
        if !seen.contains(comment.issue_identifier.as_str()) {
            found.push(format!(
                "comments[{index}]: issue_identifier `{}` is not one of the document's issues",
                comment.issue_identifier
            ));
        }
        if Timestamp::parse_rfc3339(&comment.created_at).is_err() {
            found.push(format!("comments[{index}]: created_at is not an RFC 3339 timestamp"));
        }
        if comment.body.trim().is_empty() {
            found.push(format!("comments[{index}]: body is required"));
        }
    }
    found
}

/// POST /`api/v1/workspaces/:workspace_id/import/project`
pub async fn import_project(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Json(req): Json<ImportProjectRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let mut extensions = axum::http::Extensions::new();
    extensions.insert(claims);
    if let Some(Extension(bot_ctx)) = bot {
        extensions.insert(bot_ctx);
    }

    let (importer, role, _) = require_workspace_access(&state, &extensions, workspace_id).await?;
    if !matches!(role.as_str(), "owner" | "admin") {
        return Err(ApiError::Forbidden("only owners and admins can import".to_string()));
    }

    #[derive(Debug, FromQueryResult)]
    struct Target {
        id: Uuid,
        key: String,
    }
    let target = Target::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, key FROM projects WHERE workspace_id = $1 AND UPPER(key) = UPPER($2)",
        vec![workspace_id.into(), req.project_key.clone().into()],
    ))
    .one(&state.db)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("project {} not found", req.project_key)))?;

    let document = req.document;
    if document.format != EXPORT_FORMAT || document.version != EXPORT_VERSION {
        return Err(ApiError::typed(
            ApiErrorKind::UnsupportedFormat,
            format!("unsupported export format; expected {EXPORT_FORMAT} version {EXPORT_VERSION}"),
        ));
    }

    // Everything is checked before the first write.
    let workflow = resolve_effective_workflow_for_project(&state, target.id).await?;
    let states: HashSet<String> = workflow.states.iter().map(|state| state.key.clone()).collect();
    let errors = problems(&document, &states);
    if !errors.is_empty() {
        return Err(ApiError::typed_with_details(
            ApiErrorKind::InvalidUpdate,
            format!("import refused: {} problems", errors.len()),
            json!({ "errors": errors }),
        ));
    }

    // All or nothing. Nothing is sent to webhooks, notifications, activities or AI tasks for what
    // is imported.
    let tx = state.db.begin().await?;
    let now = Timestamp::now();
    let mut created: HashMap<&str, Uuid> = HashMap::new();
    let mut identifiers = BTreeMap::new();
    for issue in &document.issues {
        let id = Uuid::new_v4();
        let due_at = issue
            .due_at
            .as_deref()
            .map(Timestamp::parse_rfc3339)
            .transpose()
            .map_err(|_| ApiError::BadRequest("due_at is not an RFC 3339 timestamp".to_string()))?;
        insert_work_item(
            &tx,
            &NewWorkItem {
                id,
                project_id: target.id,
                title: &issue.title,
                description: &issue.description,
                state: &issue.state,
                priority: issue.priority.as_deref().unwrap_or("medium"),
                assignee_id: None,
                due_at,
                created_by: importer,
                sprint_id: None,
                now,
            },
        )
        .await?;
        #[derive(Debug, FromQueryResult)]
        struct Sequence {
            sequence_number: i32,
        }
        let sequence = Sequence::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT sequence_number FROM work_items WHERE id = $1",
            vec![id.into()],
        ))
        .one(&tx)
        .await?
        .ok_or(ApiError::Internal)?;
        identifiers.insert(
            issue.identifier.clone(),
            identifier(&target.key, sequence.sequence_number),
        );
        created.insert(issue.identifier.as_str(), id);
    }
    for comment in &document.comments {
        let issue_id = *created
            .get(comment.issue_identifier.as_str())
            .ok_or(ApiError::Internal)?;
        let at = Timestamp::parse_rfc3339(&comment.created_at)
            .map_err(|_| ApiError::BadRequest("created_at is not an RFC 3339 timestamp".to_string()))?;
        tx.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO comments (id, work_item_id, author_id, body, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6)",
            vec![
                Uuid::new_v4().into(),
                issue_id.into(),
                importer.into(),
                comment.body.clone().into(),
                at.into(),
                at.into(),
            ],
        ))
        .await?;
    }
    tx.commit().await?;

    Ok(ApiResponse::success(ImportResult {
        project_id: target.id,
        project_key: target.key,
        issues_created: document.issues.len(),
        comments_created: document.comments.len(),
        identifiers,
    }))
}

#[cfg(test)]
mod tests {
    use super::{ImportDocument, problems};
    use serde_json::json;
    use std::collections::HashSet;

    fn document(value: serde_json::Value) -> ImportDocument {
        serde_json::from_value(value).expect("a document")
    }

    #[test]
    fn every_problem_is_collected_before_anything_is_written() {
        let states: HashSet<String> = ["todo", "done"].iter().map(ToString::to_string).collect();
        let found = problems(
            &document(json!({
                "format": "sylvode.project_export", "version": 1,
                "issues": [
                    {"identifier": "A-1", "title": "fine", "state": "todo"},
                    {"identifier": "A-1", "title": " ", "state": "nope", "priority": "huge", "due_at": "tomorrow"}
                ],
                "comments": [
                    {"issue_identifier": "A-9", "body": "orphan", "created_at": "2030-01-01T00:00:00Z"},
                    {"issue_identifier": "A-1", "body": "", "created_at": "yesterday"}
                ]
            })),
            &states,
        );
        assert_eq!(found.len(), 8, "{found:#?}");
        assert!(found.iter().any(|line| line.contains("more than once")));
        assert!(found.iter().any(|line| line.contains("state `nope`")));
        assert!(found.iter().any(|line| line.contains("A-9")));
    }

    #[test]
    fn a_valid_document_has_no_problems_and_unknown_members_are_ignored() {
        let states: HashSet<String> = std::iter::once("todo".to_string()).collect();
        let found = problems(
            &document(json!({
                "format": "sylvode.project_export", "version": 1,
                "exported_at": "2030-01-01T00:00:00Z",
                "project": {"key": "A", "name": "a", "description": ""},
                "issues": [{"identifier": "A-1", "title": "t", "description": "d", "state": "todo",
                            "priority": "high", "due_at": "2030-01-02T00:00:00Z", "created_at": "x", "updated_at": "y"}],
                "comments": [{"issue_identifier": "A-1", "author_name": "n", "body": "b", "created_at": "2030-01-01T00:00:00Z"}]
            })),
            &states,
        );
        assert!(found.is_empty(), "{found:?}");
    }
}
