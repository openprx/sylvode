//! The work item as the API answers it: the one place that reads a work item together with its
//! project key and labels, and the one place that formats its identifier.

use std::collections::{HashMap, HashSet};

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::{ApiError, ApiErrorKind};
use crate::flow::collab::authz::{self, PermissionLevel};
use crate::flow::{repository as flow_repository, scope as flow_scope};

#[derive(Debug, Serialize)]
pub struct IssueResponse {
    pub id: Uuid,
    /// The short identifier, `{project key}-{sequence number}` such as `WEB-12`.
    pub identifier: String,
    pub project_id: Uuid,
    pub sprint_id: Option<Uuid>,
    /// The workspace milestone the item belongs to.
    pub milestone_id: Option<Uuid>,
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
    /// The Flow pages linked to the item, as the caller may see them. Present on single-item
    /// answers only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_links: Option<Vec<PageLink>>,
    /// How many Flow pages are linked to the item. Present on list answers only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_link_count: Option<i64>,
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
    milestone_id: Option<Uuid>,
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
                   wi.sprint_id, wi.milestone_id, wi.title, wi.description, wi.state, wi.priority, wi.assignee_id,
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
            milestone_id: row.milestone_id,
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
            page_links: None,
            page_link_count: None,
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

/// The most Flow pages one work item can link to.
pub const MAX_PAGE_LINKS: i64 = 50;

/// Whether the caller may read the page behind a link.
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PageAccess {
    Visible,
    Restricted,
}

/// Where the page is: in use, or in the trash (its links are kept and shown as such).
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PageLifecycle {
    Active,
    Archived,
}

impl PageLifecycle {
    fn parse(raw: &str) -> Self {
        if raw == "archived" {
            Self::Archived
        } else {
            Self::Active
        }
    }
}

/// One link from a work item to a Flow page, as a particular caller may see it.
#[derive(Debug, Serialize, Clone)]
pub struct PageLink {
    pub object_id: Uuid,
    /// The page's current title; `null` unless the caller may read the page.
    pub title: Option<String>,
    pub access: PageAccess,
    pub lifecycle: PageLifecycle,
    pub linked_at: String,
}

/// Who reads or changes the page links of a work item, resolved in the item's workspace.
#[derive(Debug, Clone)]
pub struct PageCaller {
    pub actor_id: Uuid,
    pub role: String,
    pub is_bot: bool,
    pub workspace_id: Uuid,
    /// A connected application holds no Flow scope: it never learns a page title.
    pub delegated: bool,
}

impl PageCaller {
    /// From the answer of `require_workspace_access` for the item's workspace.
    #[must_use]
    pub fn new(access: (Uuid, String, bool), workspace_id: Uuid) -> Self {
        Self {
            actor_id: access.0,
            role: access.1,
            is_bot: access.2,
            workspace_id,
            delegated: crate::oauth::delegated::current().is_some(),
        }
    }
}

/// A Flow object a link points at: its id and the workspace that owns it (none for a personal page).
#[derive(Debug, Clone, Copy)]
pub struct PageRef {
    pub id: Uuid,
    pub workspace_id: Option<Uuid>,
}

/// Whether an error only means "this caller gets nothing here" rather than a failure.
const fn is_denial(err: &ApiError) -> bool {
    matches!(
        err.kind(),
        ApiErrorKind::NotFound | ApiErrorKind::Forbidden | ApiErrorKind::FeatureDisabled
    )
}

/// The pages among `pages` the caller may read: at least `view`, as `GET /api/v2/flow/objects/{id}`
/// decides it.
///
/// A page of a workspace is judged by the caller's role there and the object's grants; a personal
/// page placed into a workspace by the owner, and the grants of its mount. A bot is a principal of
/// its own workspace only, and a connected application reads none.
///
/// # Errors
/// A database failure.
pub async fn viewable_pages(
    db: &impl ConnectionTrait,
    caller: &PageCaller,
    pages: &[PageRef],
) -> Result<HashSet<Uuid>, ApiError> {
    let mut visible = HashSet::new();
    if caller.delegated {
        return Ok(visible);
    }
    let mut by_workspace: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    let mut personal = Vec::new();
    for page in pages {
        match page.workspace_id {
            Some(workspace) => by_workspace.entry(workspace).or_default().push(page.id),
            None => personal.push(page.id),
        }
    }
    for (workspace, ids) in by_workspace {
        if !flow_repository::fetch_flow_enabled(db, workspace).await? {
            continue;
        }
        let (kind, role) = if workspace == caller.workspace_id {
            (if caller.is_bot { "bot" } else { "user" }, caller.role.clone())
        } else if caller.is_bot {
            continue;
        } else {
            ("user", workspace_role(db, workspace, caller.actor_id).await?)
        };
        for (id, level) in authz::effective_permissions(db, workspace, &ids, kind, caller.actor_id, &role).await? {
            if level >= PermissionLevel::View {
                visible.insert(id);
            }
        }
    }
    if !caller.is_bot {
        for id in personal {
            match flow_scope::authorize(db, id, caller.actor_id, PermissionLevel::View).await {
                Ok(_) => {
                    visible.insert(id);
                }
                Err(err) if is_denial(&err) => {}
                Err(err) => return Err(err),
            }
        }
    }
    Ok(visible)
}

/// The workspaces among `workspaces` that `user` has a seat in.
async fn member_workspaces(
    db: &impl ConnectionTrait,
    user: Uuid,
    workspaces: &[Uuid],
) -> Result<HashSet<Uuid>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct Seat {
        workspace_id: Uuid,
    }
    Ok(Seat::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT workspace_id FROM workspace_members WHERE user_id = $1 AND workspace_id = ANY($2)",
        vec![user.into(), workspaces.to_vec().into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|seat| seat.workspace_id)
    .collect())
}

/// The caller's role in a workspace, or the Flow guest role when they have no seat there.
async fn workspace_role(db: &impl ConnectionTrait, workspace: Uuid, user: Uuid) -> Result<String, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct RoleRow {
        role: String,
    }
    Ok(RoleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        vec![workspace.into(), user.into()],
    ))
    .one(db)
    .await?
    .map_or_else(|| "__flow_guest".to_string(), |row| row.role))
}

#[derive(Debug, FromQueryResult)]
struct LinkRow {
    object_id: Uuid,
    object_workspace_id: Option<Uuid>,
    lifecycle_status: String,
    title: String,
    created_at: platform::timestamp::Timestamp,
}

/// The Flow pages linked to a work item, oldest link first, as `caller` may see them: a page the
/// caller cannot read is listed as restricted, without its title.
///
/// # Errors
/// A database failure.
pub async fn page_links(
    db: &impl ConnectionTrait,
    caller: &PageCaller,
    issue_id: Uuid,
) -> Result<Vec<PageLink>, ApiError> {
    let rows = LinkRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT l.object_id, o.workspace_id AS object_workspace_id, o.lifecycle_status,
                   COALESCE(p.title, '') AS title, l.created_at
            FROM work_item_page_links l
            INNER JOIN flow_objects o ON o.id = l.object_id
            LEFT JOIN flow_object_projections p ON p.object_id = o.id
            WHERE l.work_item_id = $1
            ORDER BY l.created_at ASC, l.object_id ASC
            LIMIT $2
        ",
        vec![issue_id.into(), MAX_PAGE_LINKS.into()],
    ))
    .all(db)
    .await?;
    let refs: Vec<PageRef> = rows
        .iter()
        .map(|row| PageRef {
            id: row.object_id,
            workspace_id: row.object_workspace_id,
        })
        .collect();
    let visible = viewable_pages(db, caller, &refs).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let can_read = visible.contains(&row.object_id);
            PageLink {
                object_id: row.object_id,
                title: can_read.then_some(row.title),
                access: if can_read {
                    PageAccess::Visible
                } else {
                    PageAccess::Restricted
                },
                lifecycle: PageLifecycle::parse(&row.lifecycle_status),
                linked_at: row.created_at.to_rfc3339(),
            }
        })
        .collect())
}

/// Sets `page_links` on a single-item answer.
///
/// # Errors
/// A database failure.
pub async fn attach_page_links(
    db: &impl ConnectionTrait,
    caller: &PageCaller,
    issue: &mut IssueResponse,
) -> Result<(), ApiError> {
    issue.page_links = Some(page_links(db, caller, issue.id).await?);
    Ok(())
}

/// How many pages are linked to each of `issue_ids`. An item without a link has no entry.
///
/// # Errors
/// A database failure.
pub async fn page_link_counts(db: &impl ConnectionTrait, issue_ids: &[Uuid]) -> Result<HashMap<Uuid, i64>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct CountRow {
        work_item_id: Uuid,
        total: i64,
    }
    if issue_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = CountRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT work_item_id, COUNT(*) AS total
            FROM work_item_page_links
            WHERE work_item_id = ANY($1)
            GROUP BY work_item_id
        ",
        vec![issue_ids.to_vec().into()],
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|row| (row.work_item_id, row.total)).collect())
}

/// The links of an item for a webhook payload: `[{object_id, title}]`.
///
/// A delivery has no caller, so a title goes out only for a page of the item's own workspace that
/// an ordinary member with no grant of their own can read; any other page (a restricted one, a
/// personal page placed into the workspace) carries `title: null`.
///
/// # Errors
/// A database failure.
pub async fn page_links_for_webhook(db: &impl ConnectionTrait, issue_id: Uuid) -> Result<Vec<Value>, ApiError> {
    let rows = LinkRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT l.object_id, o.workspace_id AS object_workspace_id, o.lifecycle_status,
                   COALESCE(p.title, '') AS title, l.created_at
            FROM work_item_page_links l
            INNER JOIN flow_objects o ON o.id = l.object_id
            LEFT JOIN flow_object_projections p ON p.object_id = o.id
            WHERE l.work_item_id = $1
            ORDER BY l.created_at ASC, l.object_id ASC
            LIMIT $2
        ",
        vec![issue_id.into(), MAX_PAGE_LINKS.into()],
    ))
    .all(db)
    .await?;
    let Some(item_workspace) = item_workspace(db, issue_id).await? else {
        return Ok(Vec::new());
    };
    let own: Vec<Uuid> = rows
        .iter()
        .filter(|row| row.object_workspace_id == Some(item_workspace))
        .map(|row| row.object_id)
        .collect();
    let mut open = HashSet::new();
    if !own.is_empty() && flow_repository::fetch_flow_enabled(db, item_workspace).await? {
        for (id, level) in authz::effective_permissions(db, item_workspace, &own, "user", Uuid::nil(), "member").await?
        {
            if level >= PermissionLevel::View {
                open.insert(id);
            }
        }
    }
    Ok(rows
        .into_iter()
        .map(|row| {
            let title = open.contains(&row.object_id).then_some(row.title);
            json!({"object_id": row.object_id, "title": title})
        })
        .collect())
}

async fn item_workspace(db: &impl ConnectionTrait, issue_id: Uuid) -> Result<Option<Uuid>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct Row {
        workspace_id: Uuid,
    }
    Ok(Row::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT p.workspace_id FROM work_items wi INNER JOIN projects p ON p.id = wi.project_id WHERE wi.id = $1",
        vec![issue_id.into()],
    ))
    .one(db)
    .await?
    .map(|row| row.workspace_id))
}

/// One work item linked to a page, as the page side shows it.
#[derive(Debug, Serialize)]
pub struct PageWorkItem {
    pub id: Uuid,
    pub identifier: String,
    pub title: String,
    pub state: String,
    pub project_id: Uuid,
}

/// The work items linked to a page that the caller may read, and how many others there are.
#[derive(Debug, Serialize)]
pub struct PageWorkItems {
    pub items: Vec<PageWorkItem>,
    /// Linked items the caller may not read: counted, never described.
    pub hidden_count: i64,
    /// More links exist than were examined; the counts cover the first [`MAX_PAGE_SIDE_LINKS`].
    pub truncated: bool,
}

/// The most links of one page that a single answer examines.
const MAX_PAGE_SIDE_LINKS: usize = 500;

/// The work items linked to `object`, for a caller who may read the page.
///
/// A person reads the items of every workspace they belong to; a bot those of its own workspace
/// in the projects where its level reaches `read`.
///
/// # Errors
/// A database failure.
pub async fn work_items_of_page(
    db: &sea_orm::DatabaseConnection,
    caller: &PageCaller,
    object: Uuid,
) -> Result<PageWorkItems, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct ItemRow {
        id: Uuid,
        project_id: Uuid,
        workspace_id: Uuid,
        project_key: String,
        sequence_number: i32,
        title: String,
        state: String,
    }
    let mut rows = ItemRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT wi.id, wi.project_id, p.workspace_id, p.key AS project_key, wi.sequence_number,
                   wi.title, wi.state
            FROM work_item_page_links l
            INNER JOIN work_items wi ON wi.id = l.work_item_id
            INNER JOIN projects p ON p.id = wi.project_id
            WHERE l.object_id = $1
            ORDER BY l.created_at ASC, wi.id ASC
            LIMIT $2
        ",
        vec![
            object.into(),
            i64::try_from(MAX_PAGE_SIDE_LINKS + 1).unwrap_or(i64::MAX).into(),
        ],
    ))
    .all(db)
    .await?;
    let truncated = rows.len() > MAX_PAGE_SIDE_LINKS;
    rows.truncate(MAX_PAGE_SIDE_LINKS);

    let member_of = if caller.is_bot {
        HashSet::new()
    } else {
        let workspaces: Vec<Uuid> = rows.iter().map(|row| row.workspace_id).collect();
        member_workspaces(db, caller.actor_id, &workspaces).await?
    };

    let total = i64::try_from(rows.len()).unwrap_or(i64::MAX);
    let mut items = Vec::new();
    for row in rows {
        let readable = if caller.is_bot {
            row.workspace_id == caller.workspace_id
                && crate::middleware::scoped_bot::bot_level_in_project(db, caller.actor_id, row.project_id).await?
                    >= crate::middleware::scoped_bot::Level::Read
        } else {
            member_of.contains(&row.workspace_id)
        };
        if readable {
            items.push(PageWorkItem {
                id: row.id,
                identifier: identifier(&row.project_key, row.sequence_number),
                title: row.title,
                state: row.state,
                project_id: row.project_id,
            });
        }
    }
    let hidden_count = total - i64::try_from(items.len()).unwrap_or(0);
    Ok(PageWorkItems {
        items,
        hidden_count,
        truncated,
    })
}

/// How a page named by an activity entry looks to the caller. The entry only stores the page's id.
///
/// `removed` means the page no longer exists (it was purged); `restricted` that the caller may not
/// read it. Neither carries a title.
///
/// # Errors
/// A database failure.
pub async fn activity_pages(
    db: &impl ConnectionTrait,
    caller: &PageCaller,
    object_ids: &[Uuid],
) -> Result<HashMap<Uuid, Value>, ApiError> {
    #[derive(Debug, FromQueryResult)]
    struct PageRow {
        id: Uuid,
        workspace_id: Option<Uuid>,
        lifecycle_status: String,
        title: String,
    }
    if object_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = PageRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            SELECT o.id, o.workspace_id, o.lifecycle_status, COALESCE(p.title, '') AS title
            FROM flow_objects o
            LEFT JOIN flow_object_projections p ON p.object_id = o.id
            WHERE o.id = ANY($1)
        ",
        vec![object_ids.to_vec().into()],
    ))
    .all(db)
    .await?;
    let refs: Vec<PageRef> = rows
        .iter()
        .map(|row| PageRef {
            id: row.id,
            workspace_id: row.workspace_id,
        })
        .collect();
    let visible = viewable_pages(db, caller, &refs).await?;
    let mut answers: HashMap<Uuid, Value> = object_ids
        .iter()
        .map(|id| (*id, json!({"title": null, "access": "removed", "lifecycle": null})))
        .collect();
    for row in rows {
        let can_read = visible.contains(&row.id);
        answers.insert(
            row.id,
            json!({
                "title": can_read.then_some(row.title),
                "access": if can_read { "visible" } else { "restricted" },
                "lifecycle": PageLifecycle::parse(&row.lifecycle_status),
            }),
        );
    }
    Ok(answers)
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
