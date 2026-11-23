//! The work item as the API answers it: the one place that reads a work item together with its
//! project key and labels, and the one place that formats its identifier.

use std::collections::HashMap;

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;

use crate::error::ApiError;

#[derive(Debug, Serialize)]
pub struct IssueResponse {
    pub id: Uuid,
    /// The short identifier, `{project key}-{sequence number}` such as `WEB-12`.
    pub identifier: String,
    pub project_id: Uuid,
    pub sprint_id: Option<Uuid>,
    pub title: String,
    pub description: String,
    pub state: String,
    pub priority: String,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<String>,
    pub created_by: Option<Uuid>,
    pub created_at: String,
    pub updated_at: String,
    pub proposal_id: Option<String>,
    pub governance_exempt: bool,
    pub governance_exempt_reason: Option<String>,
    /// Always an array, empty when the item has no label.
    pub labels: Vec<IssueLabelSummary>,
}

#[derive(Debug, Serialize, Clone)]
pub struct IssueLabelSummary {
    pub id: Uuid,
    pub name: String,
    pub color: String,
}

/// A work item answer with the workspace that owns it, for the caller's access check.
#[derive(Debug)]
pub struct LoadedIssue {
    pub workspace_id: Uuid,
    pub issue: IssueResponse,
}

/// The identifier of a work item: its project key and sequence number.
#[must_use]
pub fn identifier(project_key: &str, sequence_number: i32) -> String {
    format!("{project_key}-{sequence_number}")
}

#[derive(Debug, FromQueryResult)]
struct IssueRow {
    id: Uuid,
    project_id: Uuid,
    workspace_id: Uuid,
    project_key: String,
    sequence_number: i32,
    sprint_id: Option<Uuid>,
    title: String,
    description: String,
    state: String,
    priority: String,
    assignee_id: Option<Uuid>,
    due_at: Option<platform::timestamp::Timestamp>,
    created_by: Option<Uuid>,
    created_at: platform::timestamp::Timestamp,
    updated_at: platform::timestamp::Timestamp,
    proposal_id: Option<String>,
    governance_exempt: bool,
    governance_exempt_reason: Option<String>,
}

/// Reads one work item with its identifier and labels; `None` when it does not exist.
///
/// Takes any connection, so an update can read the row it just wrote inside its own transaction.
///
/// # Errors
/// A database failure.
pub async fn load_issue(db: &impl ConnectionTrait, issue_id: Uuid) -> Result<Option<LoadedIssue>, ApiError> {
    let row = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.id, wi.project_id, p.workspace_id, p.key AS project_key, wi.sequence_number,
                   wi.sprint_id, wi.title, wi.description, wi.state, wi.priority, wi.assignee_id,
                   wi.due_at, wi.created_by, wi.created_at, wi.updated_at,
                   wi.proposal_id, wi.governance_exempt, wi.governance_exempt_reason
            FROM work_items wi
            INNER JOIN projects p ON wi.project_id = p.id
            WHERE wi.id = $1
        ",
        vec![issue_id.into()],
    ))
    .one(db)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let labels = issue_labels(db, &[row.id]).await?.remove(&row.id).unwrap_or_default();
    Ok(Some(LoadedIssue {
        workspace_id: row.workspace_id,
        issue: IssueResponse {
            id: row.id,
            identifier: identifier(&row.project_key, row.sequence_number),
            project_id: row.project_id,
            sprint_id: row.sprint_id,
            title: row.title,
            description: row.description,
            state: row.state,
            priority: row.priority,
            assignee_id: row.assignee_id,
            due_at: row.due_at.map(platform::timestamp::Timestamp::to_rfc3339),
            created_by: row.created_by,
            created_at: row.created_at.to_rfc3339(),
            updated_at: row.updated_at.to_rfc3339(),
            proposal_id: row.proposal_id,
            governance_exempt: row.governance_exempt,
            governance_exempt_reason: row.governance_exempt_reason,
            labels,
        },
    }))
}

/// The labels of each of `issue_ids`, sorted by name. An item without a label has no entry.
///
/// # Errors
/// A database failure.
pub async fn issue_labels(
    db: &impl ConnectionTrait,
    issue_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<IssueLabelSummary>>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct IssueLabelRow {
        work_item_id: Uuid,
        label_id: Uuid,
        label_name: String,
        label_color: String,
    }

    let mut label_map: HashMap<Uuid, Vec<IssueLabelSummary>> = HashMap::new();
    if issue_ids.is_empty() {
        return Ok(label_map);
    }
    let rows = IssueLabelRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wil.work_item_id, l.id AS label_id, l.name AS label_name, l.color AS label_color
            FROM work_item_labels wil
            INNER JOIN labels l ON wil.label_id = l.id
            WHERE wil.work_item_id = ANY($1)
            ORDER BY l.name ASC
        ",
        vec![issue_ids.to_vec().into()],
    ))
    .all(db)
    .await?;
    for row in rows {
        label_map.entry(row.work_item_id).or_default().push(IssueLabelSummary {
            id: row.label_id,
            name: row.label_name,
            color: row.label_color,
        });
    }
    Ok(label_map)
}

#[cfg(test)]
mod tests {
    use super::identifier;

    #[test]
    fn the_identifier_is_the_project_key_and_the_sequence_number() {
        assert_eq!(identifier("WEB", 12), "WEB-12");
        assert_eq!(identifier("qlb", 1), "qlb-1");
    }
}
