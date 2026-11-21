//! Casting a vote on a proposal, shared by the vote endpoint and the task reply routing.

use platform::{app::AppState, timestamp::Timestamp};
use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde_json::json;
use uuid::Uuid;

use crate::{
    entities::trust_score::ParticipantType,
    error::ApiError,
    routes::proposal::{
        ProposalRow, VoteRow, VoteTally, ensure_voting_window_open, primary_domain_for_proposal,
        proposal_tally_with_conn, resolve_project_id_for_proposal, resolve_vote_weight_for_proposal,
        resolve_workspace_id_for_project, validate_vote_choice, write_proposal_audit_log_with_conn,
    },
    services::permission_service::PermissionService,
    webhook_trigger::{TriggerContext, WebhookEvent},
};

/// Who votes: a person (`human`) or a bot (`ai`).
#[derive(Debug, Clone, Copy)]
pub struct VoteActor {
    pub user_id: Uuid,
    pub author_type: &'static str,
}

/// A vote that was just stored. The trigger context is for the caller to send after commit.
#[derive(Debug)]
pub struct CastVote {
    pub vote: VoteRow,
    pub tally: VoteTally,
    pub trigger: Option<TriggerContext>,
}

/// Casts `actor`'s vote on `proposal` with every check the vote endpoint applies.
///
/// The proposal must be in voting with its window open, and the choice must be `yes`, `no` or
/// `abstain`. A bot needs the voting permission in the proposal's domain and a reason of at least
/// the configured minimum length (50 characters unless the project sets another). The vote and
/// its audit record are written on `txn`; a second vote by the same voter is a `Conflict` that
/// writes nothing, so the transaction stays usable.
pub async fn cast_vote<C: ConnectionTrait>(
    state: &AppState,
    txn: &C,
    actor: &VoteActor,
    proposal: &ProposalRow,
    choice: &str,
    reason: Option<&str>,
) -> Result<CastVote, ApiError> {
    if proposal.status != "voting" {
        return Err(ApiError::BadRequest("proposal is not in voting status".to_string()));
    }
    ensure_voting_window_open(proposal)?;

    let choice = choice.trim().to_lowercase();
    if !validate_vote_choice(&choice) {
        return Err(ApiError::BadRequest("invalid vote choice".to_string()));
    }

    if actor.author_type == "ai" {
        let project_id = resolve_project_id_for_proposal(state, &proposal.id, &proposal.author_id).await?;
        let Some(project_id) = project_id else {
            return Err(ApiError::Forbidden("ai voting requires project context".to_string()));
        };

        let domain = primary_domain_for_proposal(proposal);
        let permission = PermissionService::new(state.db.clone());
        let can_vote = permission
            .can_vote(actor.user_id, project_id, &domain, ParticipantType::Ai)
            .await?;
        if !can_vote {
            return Err(ApiError::Forbidden(
                "ai participant has no voting permission in this domain".to_string(),
            ));
        }

        let min_reason = permission
            .ai_reason_min_length(actor.user_id, project_id)
            .await?
            .unwrap_or(50)
            .max(0);
        let min_reason = usize::try_from(min_reason).map_err(|_| ApiError::Internal)?;

        let reason_len = reason.map(str::trim).map(str::chars).map_or(0, Iterator::count);
        if reason_len < min_reason {
            return Err(ApiError::BadRequest(format!(
                "reason is required for AI vote and must be at least {min_reason} characters"
            )));
        }
    }

    let voter_id = actor.user_id.to_string();
    let reason = reason.map(|value| value.trim().to_string());
    let weight = resolve_vote_weight_for_proposal(state, &proposal.id, &voter_id, actor.author_type).await?;

    let vote = VoteRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r"
            INSERT INTO votes (proposal_id, voter_id, voter_type, choice, weight, reason, voted_at)
            VALUES ($1, $2, $3::author_type, $4::vote_choice, $5, $6, $7)
            ON CONFLICT DO NOTHING
            RETURNING id, proposal_id, voter_id, voter_type::text AS voter_type,
                      choice::text AS choice, weight, reason, voted_at
        ",
        vec![
            proposal.id.clone().into(),
            voter_id.clone().into(),
            actor.author_type.into(),
            choice.clone().into(),
            weight.into(),
            reason.into(),
            Timestamp::now().into(),
        ],
    ))
    .one(txn)
    .await?
    .ok_or_else(|| ApiError::Conflict("you have already voted on this proposal".to_string()))?;

    let tally = proposal_tally_with_conn(txn, &proposal.id).await?;

    write_proposal_audit_log_with_conn(
        txn,
        proposal,
        actor.user_id,
        "vote.created",
        "vote",
        Some(vote.id.to_string()),
        None,
        Some(json!({
            "proposal_id": proposal.id,
            "voter_id": vote.voter_id,
            "choice": choice,
            "weight": vote.weight,
        })),
        None,
    )
    .await?;

    let mut trigger = None;
    if let Some(project_id) = resolve_project_id_for_proposal(state, &proposal.id, &proposal.author_id).await?
        && let Some(workspace_id) = resolve_workspace_id_for_project(state, project_id).await?
    {
        trigger = Some(TriggerContext {
            event: WebhookEvent::ProposalVoteCast,
            workspace_id,
            project_id,
            actor_id: actor.user_id,
            issue_id: None,
            comment_id: None,
            label_id: None,
            sprint_id: None,
            changes: None,
            mentions: Vec::new(),
            extra_data: Some(json!({
                "proposal_id": vote.proposal_id,
                "vote": {
                    "id": vote.id,
                    "voter_id": vote.voter_id,
                    "choice": vote.choice,
                    "weight": vote.weight,
                    "reason": vote.reason,
                    "voted_at": vote.voted_at.to_rfc3339(),
                },
                "tally": tally,
            })),
        });
    }

    Ok(CastVote { vote, tally, trigger })
}
