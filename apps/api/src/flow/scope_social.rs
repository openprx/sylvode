//! Scope-native social application services. V1 adapters and v2 transports share normalized
//! thread rules; resource proofs, notifications, business event and receipt commit atomically.
use super::{
    collab::authz::PermissionLevel,
    comments::{self, CommentActor, FlowCommentView, NewComment},
    scope,
};
use crate::error::{ApiError, ApiErrorKind};
use sea_orm::{ConnectionTrait, DatabaseConnection, DatabaseTransaction, DbBackend, Statement, TransactionTrait};
use serde_json::json;
use uuid::Uuid;

async fn recipient_proofs(
    db: &DatabaseConnection,
    object: Uuid,
    ids: &[Uuid],
) -> Result<Vec<scope::AuthorizationProof>, ApiError> {
    let mut proofs = Vec::new();
    for user in ids {
        let active = db
            .query_one(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT id FROM users WHERE id=$1 AND is_active",
                vec![(*user).into()],
            ))
            .await?
            .is_some();
        if !active {
            continue;
        }
        match scope::authorize(db, object, *user, PermissionLevel::View).await {
            Ok(proof) => proofs.push(proof),
            Err(err) if matches!(err.kind(), ApiErrorKind::NotFound | ApiErrorKind::Forbidden) => {}
            Err(err) => return Err(err),
        }
    }
    Ok(proofs)
}
async fn actor_proof(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    minimum: PermissionLevel,
) -> Result<scope::AuthorizationProof, ApiError> {
    let proof = scope::authorize(db, object, user, minimum).await?;
    if !proof.resource_active {
        return Err(ApiError::policy_rejected("archived content is read-only"));
    }
    Ok(proof)
}
async fn begin_write(
    db: &DatabaseConnection,
    proofs: &[scope::AuthorizationProof],
) -> Result<DatabaseTransaction, ApiError> {
    let tx = db.begin().await?;
    let scope = proofs.first().ok_or_else(ApiError::internal)?.scope_id;
    if scope::legacy_workspace(&tx, scope).await?.is_none() {
        scope::require_personal_write(&tx).await?;
    }
    scope::fence_many(&tx, proofs).await?;
    Ok(tx)
}
fn key(provided: Option<&str>, operation: &str) -> String {
    provided.map_or_else(|| format!("{operation}:{}", Uuid::new_v4()), str::to_string)
}
async fn notify_mentions(
    tx: &DatabaseTransaction,
    identity: &scope::ObjectIdentity,
    actor: Uuid,
    recipients: &[scope::AuthorizationProof],
    event: Uuid,
) -> Result<(), ApiError> {
    let actor_name = comments::actor_label(tx, actor).await?;
    let link = identity.workspace_id.map_or_else(
        || format!("/flow/{}", identity.id),
        |workspace| format!("/workspace/{workspace}/flow/{}", identity.id),
    );
    for proof in recipients {
        if proof.principal_id == actor {
            continue;
        }
        tx.execute(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO notifications(id,user_id,workspace_id,type,kind,payload,title,content,link,is_read,created_at) VALUES($1,$2,$3,'mentioned','mentioned',$4,'notification.mentionedTitle',$5,$6,false,now())",vec![Uuid::new_v4().into(),proof.principal_id.into(),identity.workspace_id.into(),json!({"flow_object_id":identity.id,"source_flow_scope_id":identity.scope_id,"source_event_id":event}).into(),format!("mentioned_by:{actor_name}").into(),link.clone().into()])).await?;
    }
    Ok(())
}
pub async fn create_comment(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    mut input: NewComment,
    idempotency: Option<&str>,
) -> Result<FlowCommentView, ApiError> {
    input.body = comments::normalize_body(&input.body)?;
    input.mentions = comments::normalize_mentions(&input.mentions)?;
    input.block_id = comments::normalize_block_id(input.block_id)?;
    let actor = actor_proof(db, object, user, PermissionLevel::Comment).await?;
    let recipients = recipient_proofs(db, object, &input.mentions).await?;
    let mut proofs = vec![actor.clone()];
    proofs.extend(recipients.clone());
    let identity = scope::identity(db, object).await?;
    let operation = key(idempotency, "comment_create");
    let body = json!({"operation":"comment_create","object_id":object,"input":input});
    let tx = begin_write(db, &proofs).await?;
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &operation, &body).await?
    {
        tx.commit().await?;
        return serde_json::from_value(result).map_err(|_| ApiError::internal());
    }
    let actor_context = CommentActor {
        user_id: user,
        level: actor.level,
    };
    let view = comments::create_comment_on(
        &tx,
        identity.workspace_id,
        Some(identity.scope_id),
        object,
        &actor_context,
        input,
    )
    .await?;
    let event = super::scope_event::insert_for_object(
        &tx,
        &identity,
        user,
        "flow.comment.created",
        "flow_comment",
        view.id,
        &operation,
        json!({"object_id":object,"comment_id":view.id}),
        json!({"semantic_summary":{"action":"comment_create"}}),
    )
    .await?;
    notify_mentions(&tx, &identity, user, &recipients, event).await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &operation,
        body,
        object,
        event,
        serde_json::to_value(&view).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(view)
}
pub async fn update_comment(
    db: &DatabaseConnection,
    object: Uuid,
    comment: Uuid,
    user: Uuid,
    body: &str,
    mentions: Option<Vec<Uuid>>,
    idempotency: Option<&str>,
) -> Result<FlowCommentView, ApiError> {
    let actor = actor_proof(db, object, user, PermissionLevel::Comment).await?;
    let mentioned = mentions.as_deref().unwrap_or(&[]);
    let recipients = recipient_proofs(db, object, mentioned).await?;
    let mut proofs = vec![actor.clone()];
    proofs.extend(recipients.clone());
    let identity = scope::identity(db, object).await?;
    let operation = key(idempotency, "comment_update");
    let request = json!({"operation":"comment_update","object_id":object,"comment_id":comment,"body":comments::normalize_body(body)?,"mentions":mentions});
    let tx = begin_write(db, &proofs).await?;
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &operation, &request).await?
    {
        tx.commit().await?;
        return serde_json::from_value(result).map_err(|_| ApiError::internal());
    }
    let previous = comments::parse_mentions(&comments::load_visible(&tx, object, comment).await?.mentions);
    let fresh = recipients
        .into_iter()
        .filter(|proof| !previous.contains(&proof.principal_id))
        .collect::<Vec<_>>();
    let view = comments::update_comment_on(
        &tx,
        object,
        comment,
        &CommentActor {
            user_id: user,
            level: actor.level,
        },
        body,
        mentions,
    )
    .await?;
    let event = super::scope_event::insert_for_object(
        &tx,
        &identity,
        user,
        "flow.comment.updated",
        "flow_comment",
        comment,
        &operation,
        json!({"object_id":object,"comment_id":comment}),
        json!({"semantic_summary":{"action":"comment_update"}}),
    )
    .await?;
    notify_mentions(&tx, &identity, user, &fresh, event).await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &operation,
        request,
        object,
        event,
        serde_json::to_value(&view).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(view)
}
pub async fn delete_comment(
    db: &DatabaseConnection,
    object: Uuid,
    comment: Uuid,
    user: Uuid,
    idempotency: Option<&str>,
) -> Result<(), ApiError> {
    let actor = actor_proof(db, object, user, PermissionLevel::View).await?;
    let identity = scope::identity(db, object).await?;
    let operation = key(idempotency, "comment_delete");
    let request = json!({"operation":"comment_delete","object_id":object,"comment_id":comment});
    let tx = begin_write(db, std::slice::from_ref(&actor)).await?;
    if super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &operation, &request)
        .await?
        .is_some()
    {
        tx.commit().await?;
        return Ok(());
    }
    comments::delete_comment_on(
        &tx,
        object,
        comment,
        &CommentActor {
            user_id: user,
            level: actor.level,
        },
    )
    .await?;
    let event = super::scope_event::insert_for_object(
        &tx,
        &identity,
        user,
        "flow.comment.deleted",
        "flow_comment",
        comment,
        &operation,
        json!({"object_id":object,"comment_id":comment}),
        json!({"semantic_summary":{"action":"comment_delete"}}),
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &operation,
        request,
        object,
        event,
        json!({"deleted":true}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}
pub async fn resolve_comment(
    db: &DatabaseConnection,
    object: Uuid,
    comment: Uuid,
    user: Uuid,
    resolved: bool,
    idempotency: Option<&str>,
) -> Result<FlowCommentView, ApiError> {
    let actor = actor_proof(db, object, user, PermissionLevel::View).await?;
    let identity = scope::identity(db, object).await?;
    let operation = key(idempotency, "comment_resolve");
    let request = json!({"operation":"comment_resolve","object_id":object,"comment_id":comment,"resolved":resolved});
    let tx = begin_write(db, std::slice::from_ref(&actor)).await?;
    if let Some(result) =
        super::scope_commands::lock_and_replay(&tx, identity.scope_id, user, &operation, &request).await?
    {
        tx.commit().await?;
        return serde_json::from_value(result).map_err(|_| ApiError::internal());
    }
    let view = comments::set_resolved_on(
        &tx,
        object,
        comment,
        &CommentActor {
            user_id: user,
            level: actor.level,
        },
        resolved,
    )
    .await?;
    let event = super::scope_event::insert_for_object(
        &tx,
        &identity,
        user,
        "flow.comment.resolved",
        "flow_comment",
        comment,
        &operation,
        json!({"object_id":object,"comment_id":comment,"resolved":resolved}),
        json!({"semantic_summary":{"action":"comment_resolve"}}),
    )
    .await?;
    super::scope_commands::record(
        &tx,
        identity.scope_id,
        user,
        &operation,
        request,
        object,
        event,
        serde_json::to_value(&view).map_err(|_| ApiError::internal())?,
    )
    .await?;
    tx.commit().await?;
    Ok(view)
}

/// Mention contacts are existing source participants; full managers may also see that object's
/// explicit roster. No private ancestor identity or unrelated scope directory is expanded.
pub async fn mention_candidates(
    db: &DatabaseConnection,
    object: Uuid,
    user: Uuid,
    query: &str,
) -> Result<comments::MentionCandidateList, ApiError> {
    let proof = scope::authorize(db, object, user, PermissionLevel::View).await?;
    if proof.level < PermissionLevel::Comment {
        return Ok(comments::MentionCandidateList { items: Vec::new() });
    }
    let query = comments::normalize_mention_query(query)?;
    let owner = scope::get(db, proof.scope_id).await?.owner_user_id;
    let rows=db.query_all(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT DISTINCT u.id,u.name FROM users u WHERE u.is_active AND ($2='' OR strpos(lower(u.name),lower($2))>0) AND (u.id=$3 OR u.id IN(SELECT author_id FROM flow_comments WHERE object_id=$1 AND deleted_at IS NULL) OR u.id IN(SELECT cu.actor_id FROM collab_updates cu JOIN collab_documents d ON d.id=cu.document_id WHERE d.object_id=$1) OR ($4 AND u.id IN(SELECT principal_id FROM flow_object_grants WHERE object_id=$1 AND principal_kind='user'))) ORDER BY u.name,u.id LIMIT 201",vec![object.into(),query.into(),owner.into(),(proof.level>=PermissionLevel::FullAccess).into()])).await?;
    let mut items = Vec::new();
    for row in rows {
        let id: Uuid = row.try_get("", "id")?;
        match scope::authorize(db, object, id, PermissionLevel::View).await {
            Ok(_) => items.push(comments::MentionCandidate {
                user_id: id,
                name: row.try_get("", "name")?,
                kind: "participant".into(),
            }),
            Err(err) if matches!(err.kind(), ApiErrorKind::NotFound | ApiErrorKind::Forbidden) => {}
            Err(err) => return Err(err),
        }
        if items.len() >= 50 {
            break;
        }
    }
    if !scope::proof_current(db, &proof).await? {
        return Err(super::policy::authorization_read_unstable());
    }
    Ok(comments::MentionCandidateList { items })
}
