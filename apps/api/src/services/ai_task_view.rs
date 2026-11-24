//! The task view: what the task's own bot, and the runner acting for it, sees of a task.
//!
//! It is served only through the `/api/v1/ai/tasks/...` endpoints, which refuse everyone but the
//! task's bot, and it is never on the connected-application surface. It carries no token, secret
//! or data of another project.

use platform::timestamp::Timestamp;
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    error::ApiError,
    services::{
        ai_task_service::{AiTaskProtocolRow, AiTaskRow, is_active_status, trigger_kind_for_task},
        execution_settings::{self, ExecutionSettings},
    },
};

/// A mention comment longer than this many characters is cut and flagged `truncated`.
const MAX_COMMENT_CHARS: usize = 4000;

/// The steps the agent follows, by tool name.
const PROCEDURE: [&str; 4] = [
    "sylvode_task_ack",
    "work",
    "sylvode_task_progress",
    "sylvode_task_complete | sylvode_task_fail",
];

/// What a task is about, from `(task_type, reference_type)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    IssueAssignment,
    CommentMention,
    ReviewRequest,
    ProposalVote,
    Manual,
}

impl SourceKind {
    pub fn of(task: &AiTaskRow) -> Self {
        match (task.task_type.as_str(), task.reference_type.as_deref()) {
            ("issue_assigned", Some("work_item")) => Self::IssueAssignment,
            ("comment_requested", Some("comment")) => Self::CommentMention,
            ("review_requested", Some("comment")) => Self::ReviewRequest,
            ("vote_requested", Some("proposal")) => Self::ProposalVote,
            _ => Self::Manual,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IssueAssignment => "issue_assignment",
            Self::CommentMention => "comment_mention",
            Self::ReviewRequest => "review_request",
            Self::ProposalVote => "proposal_vote",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, FromQueryResult)]
struct ProjectRow {
    id: Uuid,
    key: String,
    name: String,
    workspace_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct IssueRow {
    id: Uuid,
    title: String,
    state: String,
    sequence_number: i32,
}

fn rfc3339(value: Option<Timestamp>) -> Value {
    value.map_or(Value::Null, |at| json!(at.to_rfc3339()))
}

/// The issue a task is about: the reference of an assignment, `payload.issue_id` of a comment task.
fn issue_id_of(task: &AiTaskRow, kind: SourceKind) -> Option<Uuid> {
    match kind {
        SourceKind::IssueAssignment => task.reference_id,
        SourceKind::CommentMention | SourceKind::ReviewRequest => task
            .payload
            .get("issue_id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok()),
        SourceKind::ProposalVote | SourceKind::Manual => None,
    }
}

async fn issue_view<C: ConnectionTrait>(
    db: &C,
    project: &ProjectRow,
    issue_id: Uuid,
) -> Result<Option<Value>, ApiError> {
    let issue = IssueRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, title, state, sequence_number FROM work_items WHERE id = $1 AND project_id = $2",
        vec![issue_id.into(), project.id.into()],
    ))
    .one(db)
    .await?;
    Ok(issue.map(|issue| {
        json!({
            "id": issue.id,
            "identifier": format!("{}-{}", project.key, issue.sequence_number),
            "title": issue.title,
            "state": issue.state,
            "url_path": format!("/workspace/{}/projects/{}/issues/{}", project.workspace_id, project.id, issue.id),
        })
    }))
}

/// The mention text: `payload.comment.body`, else the stored comment of this project.
async fn comment_view<C: ConnectionTrait>(
    db: &C,
    task: &AiTaskRow,
    project: &ProjectRow,
) -> Result<Option<Value>, ApiError> {
    let Some(comment_id) = task.reference_id else {
        return Ok(None);
    };
    let body = if let Some(body) = task
        .payload
        .get("comment")
        .and_then(|comment| comment.get("body"))
        .and_then(Value::as_str)
    {
        body.to_string()
    } else {
        #[derive(FromQueryResult)]
        struct Body {
            body: String,
        }
        let stored = Body::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT c.body FROM comments c INNER JOIN work_items wi ON wi.id = c.work_item_id \
             WHERE c.id = $1 AND wi.project_id = $2",
            vec![comment_id.into(), project.id.into()],
        ))
        .one(db)
        .await?;
        let Some(stored) = stored else {
            return Ok(None);
        };
        stored.body
    };
    let (body, truncated) = cut_chars(&body, MAX_COMMENT_CHARS);
    let mut view = json!({ "id": comment_id, "body": body });
    if truncated && let Some(object) = view.as_object_mut() {
        object.insert("truncated".to_string(), json!(true));
    }
    Ok(Some(view))
}

fn cut_chars(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((end, _)) => (text[..end].to_string(), true),
        None => (text.to_string(), false),
    }
}

#[derive(FromQueryResult)]
struct TitleRow {
    title: String,
}

/// The proposal's text id (`payload.proposal_id`, else `reference_id`) and its title, when the
/// proposal belongs to the task's workspace.
async fn proposal_view<C: ConnectionTrait>(
    db: &C,
    task: &AiTaskRow,
    project: &ProjectRow,
) -> Result<Option<Value>, ApiError> {
    let id = task
        .payload
        .get("proposal_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| task.reference_id.map(|id| id.to_string()));
    let Some(id) = id else {
        return Ok(None);
    };
    let title = TitleRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT title FROM proposals WHERE id = $1 AND workspace_id = $2",
        vec![id.clone().into(), project.workspace_id.into()],
    ))
    .one(db)
    .await?
    .map(|row| row.title);
    Ok(Some(json!({ "id": id, "title": title })))
}

/// What the server does for the agent when the task ends, as text for the agent to read.
fn reply_policy(kind: SourceKind, settings: Option<&ExecutionSettings>) -> Value {
    let (on_complete, on_fail) = match kind {
        SourceKind::IssueAssignment => {
            let state = settings.map_or("auto", |settings| settings.on_complete_state.as_str());
            let on_complete = match state {
                "none" => "comment".to_string(),
                "auto" => "comment+state:auto".to_string(),
                explicit => format!("comment+{explicit}"),
            };
            let labelled = settings.is_some_and(|settings| settings.on_fail_label_id.is_some());
            (
                on_complete,
                if labelled { "comment+label" } else { "comment" }.to_string(),
            )
        }
        SourceKind::CommentMention | SourceKind::ReviewRequest => ("comment".to_string(), "comment".to_string()),
        SourceKind::ProposalVote => ("vote".to_string(), "none".to_string()),
        SourceKind::Manual => ("none".to_string(), "none".to_string()),
    };
    json!({ "on_complete": on_complete, "on_fail": on_fail })
}

/// Builds the task view of `task`.
pub async fn build_task_view<C: ConnectionTrait>(
    db: &C,
    task: &AiTaskRow,
    protocol: &AiTaskProtocolRow,
) -> Result<Value, ApiError> {
    let project = ProjectRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, key, name, workspace_id FROM projects WHERE id = $1",
        vec![task.project_id.into()],
    ))
    .one(db)
    .await?
    .ok_or_else(|| ApiError::NotFound("project not found".to_string()))?;
    let settings = execution_settings::load(db, task.project_id, task.ai_participant_id).await?;
    let kind = SourceKind::of(task);

    let mut source = serde_json::Map::new();
    source.insert("kind".to_string(), json!(kind.as_str()));
    if let Some(issue_id) = issue_id_of(task, kind)
        && let Some(issue) = issue_view(db, &project, issue_id).await?
    {
        source.insert("issue".to_string(), issue);
    }
    if matches!(kind, SourceKind::CommentMention | SourceKind::ReviewRequest)
        && let Some(comment) = comment_view(db, task, &project).await?
    {
        source.insert("comment".to_string(), comment);
    }
    if kind == SourceKind::ProposalVote
        && let Some(proposal) = proposal_view(db, task, &project).await?
    {
        source.insert("proposal".to_string(), proposal);
    }

    let execution = settings.as_ref().map(|settings| {
        json!({
            "enabled": settings.enabled,
            "channel": settings.channel,
            "cli": settings.cli,
            "working_directory": settings.working_directory,
            "approval_gate": settings.approval_gate,
            "version": settings.version,
        })
    });

    Ok(json!({
        "task_id": task.id,
        "attempt": task.attempts,
        "max_attempts": task.max_attempts,
        "status": task.status,
        "active": is_active_status(&task.status),
        "task_type": task.task_type,
        "trigger_kind": trigger_kind_for_task(&task.task_type),
        "source": source,
        "project": {
            "id": project.id,
            "key": project.key,
            "name": project.name,
            "workspace_id": project.workspace_id,
        },
        "execution": execution,
        "plan": protocol.plan,
        "acknowledged_at": rfc3339(protocol.acknowledged_at),
        "last_progress_at": rfc3339(protocol.last_progress_at),
        "lease_expires_at": rfc3339(protocol.lease_expires_at),
        "runner_phase": protocol.runner_phase,
        "reply_policy": reply_policy(kind, settings.as_ref()),
        "procedure": PROCEDURE,
    }))
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::{build_task_view, cut_chars};
    use crate::routes::context::tenant_fixture::{
        exec, seed_bot_user, seed_connection, seed_proposal, seed_tenant, seed_work_item,
    };
    use crate::scratch_or_skip;
    use crate::services::ai_task_fixture::seed_task_for;
    use crate::services::ai_task_service::load_task_with_protocol;
    use serde_json::{Value, json};
    use uuid::Uuid;

    fn keys(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, inner) in map {
                    out.push(key.to_lowercase());
                    keys(inner, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|item| keys(item, out)),
            _ => {}
        }
    }

    #[test]
    fn text_is_cut_on_a_character_boundary() {
        assert_eq!(cut_chars("abc", 3), ("abc".to_string(), false));
        assert_eq!(cut_chars("abcd", 3), ("abc".to_string(), true));
        assert_eq!(cut_chars("ééé", 2), ("éé".to_string(), true));
    }

    async fn view_of(db: &sea_orm::DatabaseConnection, task: Uuid) -> Value {
        let entry = load_task_with_protocol(db, task).await.expect("task loads");
        build_task_view(db, &entry.task, &entry.protocol)
            .await
            .expect("view builds")
    }

    #[tokio::test]
    async fn an_assignment_view_names_the_issue_the_project_and_the_execution_settings() {
        let scratch = scratch_or_skip!("aitv_assignment");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "as").await;
        let bot = seed_bot_user(db, &tenant).await;
        let issue = seed_work_item(db, &tenant, "Fix the build").await;
        let task = seed_task_for(
            db,
            &tenant,
            bot,
            "processing",
            1,
            0,
            ("issue_assigned", Some("work_item"), Some(issue)),
            json!({"issue_id": issue.to_string()}),
        )
        .await;

        let without = view_of(db, task).await;
        assert!(without["execution"].is_null(), "no settings row gives null");
        assert_eq!(without["source"]["kind"], "issue_assignment");
        assert_eq!(without["source"]["issue"]["title"], "Fix the build");
        assert_eq!(without["source"]["issue"]["state"], "todo");
        let identifier = without["source"]["issue"]["identifier"].as_str().expect("identifier");
        assert!(identifier.starts_with("Pas-"), "{identifier}");
        assert_eq!(
            without["source"]["issue"]["url_path"],
            format!(
                "/workspace/{}/projects/{}/issues/{issue}",
                tenant.workspace_id, tenant.project_id
            )
        );
        assert!(without["source"].get("comment").is_none() && without["source"].get("proposal").is_none());
        assert_eq!(without["project"]["key"], "Pas");
        assert_eq!(without["project"]["workspace_id"], tenant.workspace_id.to_string());
        assert_eq!(without["trigger_kind"], "assigned");
        assert_eq!(
            without["reply_policy"],
            json!({"on_complete": "comment+state:auto", "on_fail": "comment"})
        );
        assert_eq!(without["active"], true);

        seed_connection(db, &tenant, tenant.project_id, bot, true, "/work/a").await;
        exec(
            db,
            "UPDATE project_bot_connections SET on_complete_state = 'state:done' WHERE project_id = $1",
            vec![tenant.project_id.into()],
        )
        .await;
        let with = view_of(db, task).await;
        assert_eq!(
            with["execution"],
            json!({"enabled": true, "channel": "tmux", "cli": "claude", "working_directory": "/work/a",
                   "approval_gate": "none", "version": 1})
        );
        assert_eq!(with["reply_policy"]["on_complete"], "comment+state:done");

        // The project's current key is used after a rename.
        exec(
            db,
            "UPDATE projects SET key = 'RENAMED' WHERE id = $1",
            vec![tenant.project_id.into()],
        )
        .await;
        let renamed = view_of(db, task).await;
        assert_eq!(renamed["project"]["key"], "RENAMED");
        assert!(renamed["execution"].get("session_name").is_none());

        let mut all = Vec::new();
        keys(&renamed, &mut all);
        for forbidden in ["token", "secret", "password", "credential"] {
            assert!(
                !all.iter().any(|key| key.contains(forbidden)),
                "a `{forbidden}` key is in the view: {all:?}"
            );
        }
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_mention_view_carries_the_comment_cut_at_4000_characters() {
        let scratch = scratch_or_skip!("aitv_mention");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "mn").await;
        let bot = seed_bot_user(db, &tenant).await;
        let issue = seed_work_item(db, &tenant, "Question").await;
        let comment_id = Uuid::new_v4();
        let long = "x".repeat(4500);
        let task = seed_task_for(
            db,
            &tenant,
            bot,
            "processing",
            1,
            0,
            ("comment_requested", Some("comment"), Some(comment_id)),
            json!({"issue_id": issue.to_string(), "comment": {"id": comment_id.to_string(), "body": long}}),
        )
        .await;
        let view = view_of(db, task).await;
        assert_eq!(view["source"]["kind"], "comment_mention");
        assert_eq!(view["trigger_kind"], "mention");
        assert_eq!(view["source"]["issue"]["title"], "Question");
        assert_eq!(view["source"]["comment"]["id"], comment_id.to_string());
        assert_eq!(
            view["source"]["comment"]["body"]
                .as_str()
                .expect("body")
                .chars()
                .count(),
            4000
        );
        assert_eq!(view["source"]["comment"]["truncated"], true);
        assert_eq!(
            view["reply_policy"],
            json!({"on_complete": "comment", "on_fail": "comment"})
        );

        let short = seed_task_for(
            db,
            &tenant,
            bot,
            "processing",
            1,
            0,
            ("review_requested", Some("comment"), Some(comment_id)),
            json!({"issue_id": issue.to_string(), "comment": {"id": comment_id.to_string(), "body": "short"}}),
        )
        .await;
        let view = view_of(db, short).await;
        assert_eq!(view["source"]["kind"], "review_request");
        assert_eq!(view["source"]["comment"]["body"], "short");
        assert!(view["source"]["comment"].get("truncated").is_none());
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_vote_view_takes_the_proposal_id_from_the_payload() {
        let scratch = scratch_or_skip!("aitv_vote");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "vt").await;
        let other = seed_tenant(db, "vu").await;
        let bot = seed_bot_user(db, &tenant).await;
        let proposal = seed_proposal(db, Some(tenant.workspace_id), tenant.member_id).await;
        let foreign = seed_proposal(db, Some(other.workspace_id), other.member_id).await;

        // `PROP-<uuid>` ids cannot be a task reference: the payload carries them.
        let task = seed_task_for(
            db,
            &tenant,
            bot,
            "processing",
            1,
            0,
            ("vote_requested", Some("proposal"), None),
            json!({"proposal_id": proposal}),
        )
        .await;
        let view = view_of(db, task).await;
        assert_eq!(view["source"]["kind"], "proposal_vote");
        assert_eq!(view["source"]["proposal"]["id"], proposal);
        assert_eq!(view["source"]["proposal"]["title"], "secret proposal");
        assert_eq!(view["reply_policy"]["on_complete"], "vote");

        let bare = Uuid::new_v4();
        let by_reference = seed_task_for(
            db,
            &tenant,
            bot,
            "processing",
            1,
            0,
            ("vote_requested", Some("proposal"), Some(bare)),
            json!({}),
        )
        .await;
        let view = view_of(db, by_reference).await;
        assert_eq!(
            view["source"]["proposal"]["id"],
            bare.to_string(),
            "falls back to the reference"
        );
        assert!(view["source"]["proposal"]["title"].is_null());

        // A proposal of another workspace is not described.
        let leak = seed_task_for(
            db,
            &tenant,
            bot,
            "processing",
            1,
            0,
            ("vote_requested", Some("proposal"), None),
            json!({"proposal_id": foreign}),
        )
        .await;
        let view = view_of(db, leak).await;
        assert!(view["source"]["proposal"]["title"].is_null());
        scratch.drop_self().await;
    }

    #[tokio::test]
    async fn a_manual_task_has_no_source_details() {
        let scratch = scratch_or_skip!("aitv_manual");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "mu").await;
        let bot = seed_bot_user(db, &tenant).await;
        let task = crate::services::ai_task_fixture::seed_task(db, &tenant, bot, "acknowledged", 1, 0).await;
        let view = view_of(db, task).await;
        assert_eq!(view["source"], json!({"kind": "manual"}));
        assert_eq!(view["reply_policy"], json!({"on_complete": "none", "on_fail": "none"}));
        assert_eq!(view["procedure"][0], "sylvode_task_ack");
        scratch.drop_self().await;
    }
}
