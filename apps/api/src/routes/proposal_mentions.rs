//! Mentions and people of proposal comments (see `services::mentions`).

use std::collections::HashMap;

use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

use super::proposal::ProposalCommentRow;
use crate::{
    error::ApiError,
    services::{
        mentions::{self, Audience, Mention, Person},
        notifications::{self, NewNotification},
    },
};

/// A proposal comment as answered: the stored row plus the people it shows.
#[derive(Debug, Serialize)]
pub struct ProposalCommentView {
    #[serde(flatten)]
    pub row: ProposalCommentRow,
    /// `null` when the author id is not an account id.
    pub author: Option<Person>,
    pub mentions: Vec<Person>,
}

/// The mentions of a comment on a proposal of `workspace_id`. A proposal that belongs to no
/// workspace has nobody to mention.
pub(super) async fn resolve<C: ConnectionTrait>(
    db: &C,
    workspace_id: Option<Uuid>,
    content: &str,
    explicit: &[Uuid],
) -> Result<Vec<Mention>, ApiError> {
    match workspace_id {
        Some(workspace) => mentions::resolve_in_workspace(db, workspace, content, explicit).await,
        None => Ok(Vec::new()),
    }
}

/// Tells every mentioned person except the author.
pub(super) async fn notify<C: ConnectionTrait>(
    db: &C,
    workspace_id: Option<Uuid>,
    proposal_id: &str,
    actor_id: Uuid,
    resolved: &[Mention],
) -> Result<(), ApiError> {
    let Some(workspace) = workspace_id else {
        return Ok(());
    };
    let recipients: Vec<Uuid> = mentions::ids(resolved)
        .into_iter()
        .filter(|id| *id != actor_id)
        .collect();
    if recipients.is_empty() {
        return Ok(());
    }
    let actor_name = mentions::actor_display_name(db, actor_id).await?;
    for user_id in recipients {
        notifications::create(
            db,
            NewNotification {
                user_id,
                workspace_id: Some(workspace),
                kind: notifications::MENTIONED,
                title_key: "notification.mentionedTitle",
                content: mentions::notification_content(&actor_name),
                link: Some(format!("/proposals/{proposal_id}")),
                related_issue_id: None,
                related_comment_id: None,
                related_project_id: None,
                payload: json!({"proposal_id": proposal_id}),
            },
        )
        .await?;
    }
    Ok(())
}

/// Adds author and mentions to `rows`. `stored` are the mentions of each comment by comment id; a
/// comment without an entry mentions nobody.
pub(super) async fn show<C: ConnectionTrait>(
    db: &C,
    workspace_id: Option<Uuid>,
    rows: Vec<ProposalCommentRow>,
    stored: &HashMap<i64, Vec<Mention>>,
) -> Result<Vec<ProposalCommentView>, ApiError> {
    let audience = workspace_id.map_or(Audience::Anyone, Audience::of_workspace);
    let lists: Vec<&[Mention]> = stored.values().map(Vec::as_slice).collect();
    let authors = rows.iter().filter_map(|row| Uuid::parse_str(&row.author_id).ok());
    let shown = mentions::people_of_comments(db, audience, authors, &lists).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let mentioned = stored.get(&row.id).map_or(&[][..], Vec::as_slice);
            ProposalCommentView {
                author: Uuid::parse_str(&row.author_id)
                    .ok()
                    .and_then(|id| shown.get(&id).cloned()),
                mentions: mentions::mention_people(mentioned, &shown),
                row,
            }
        })
        .collect())
}

/// The stored mentions of the comments of one proposal, by comment id.
pub(super) async fn stored_for_proposal<C: ConnectionTrait>(
    db: &C,
    proposal_id: &str,
) -> Result<HashMap<i64, Vec<Mention>>, ApiError> {
    let rows = db
        .query_all(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT id, mentions FROM proposal_comments WHERE proposal_id = $1",
            vec![proposal_id.to_string().into()],
        ))
        .await?;
    let mut stored = HashMap::new();
    for row in rows {
        let id: i64 = row.try_get("", "id")?;
        let value: serde_json::Value = row.try_get("", "mentions")?;
        stored.insert(id, mentions::from_json(&value));
    }
    Ok(stored)
}
